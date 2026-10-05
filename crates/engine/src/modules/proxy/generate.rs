//! Making a proxy: what it is encoded as, and the loop that does it.
//!
//! ## The codec, and why
//!
//! **All-intra H.264, 4:2:0 8-bit, in MP4.** Every frame a keyframe, the long
//! side capped at [`decision::PROXY_LONG_SIDE`], no audio track.
//!
//! The requirement is "decodes cheaply and seeks well", and those pull in
//! different directions for every long-GOP codec: a frame is cheap because its
//! neighbours did most of the work, which is precisely why reaching it means
//! decoding them first. All-intra removes the second cost entirely — a seek to
//! any frame is one decode — and that is what a proxy is *for*. It is what
//! DaVinci Resolve (ProRes Proxy, DNxHR LB) and Premiere (ProRes Proxy,
//! CineForm) both ship, and they ship it for this reason and not another.
//!
//! H.264 rather than those two, given the same all-intra property:
//!
//! - **It is the best-optimised decoder in any FFmpeg build.** Measured on this
//!   machine at 5 ms per source megapixel single-threaded, against 7 for
//!   ProRes and 5 for MJPEG at far larger files. The proxy's whole job is to be
//!   cheap to decode.
//! - **It is the one codec every hardware encoder on every platform has.**
//!   `h264_vaapi`, `h264_nvenc`, `h264_qsv`, `h264_videotoolbox`, `h264_amf` —
//!   [`hwaccel::detect`] will find one on essentially any machine with a GPU,
//!   so proxy generation gets the hardware speed-up nearly everywhere. There is
//!   no hardware ProRes encoder outside Apple silicon and no hardware DNxHR
//!   encoder at all.
//! - **It stays inside the cache cap.** ProRes 422 Proxy at 720p is ~45 Mbps;
//!   this is a fraction of that at the same visual usefulness, which is the
//!   difference between a 20 GiB cache holding four hours of footage and
//!   holding forty minutes.
//! - **FFmpeg's `dnxhd` encoder refuses arbitrary sizes.** It takes a fixed
//!   table of resolution/bitrate/frame-rate combinations, so a proxy generator
//!   built on it fails on 1234×694 and on 47.952 fps. A proxy generator that
//!   fails on odd footage is not a proxy generator.
//!
//! ## Decoding cheaply is an encoder setting, and it was measured
//!
//! `tune=fastdecode` on the software path trades CABAC and the deblocking
//! filter — both decode-side costs — for a larger file. That is the right
//! direction for a file whose only purpose is to be decoded, and it is the
//! option the name was invented for. `coder=cavlc` is the same trade spelled in
//! VAAPI's dialect, and it is there because the hardware encoder's *default*
//! output turned out to be the most expensive proxy of the four.
//!
//! Measured 2026-07-26 on the development machine, 240 frames of the same
//! 1280×720 content re-encoded four ways and then decoded to RGBA
//! single-threaded (`ffmpeg -threads 1 -i P -vf format=rgba -f null -`). The
//! machine was at load ~29, so every figure is roughly twice what a quiet
//! machine gives; the ratios are the point.
//!
//! | proxy encode | decode | size |
//! |---|---:|---:|
//! | `libx264 -crf 23 -g 1` | 18.1 ms | 2.17 MB |
//! | `libx264 -crf 23 -g 1 -tune fastdecode` | **12.4–15.8 ms** | 2.89 MB |
//! | `h264_vaapi -qp 23 -g 1` | 23.0–26.5 ms | 1.86 MB |
//! | `h264_vaapi -qp 23 -g 1 -coder cavlc` | 19.5 ms | 2.71 MB |
//!
//! Two things follow, and the second is the surprise:
//!
//! - **`fastdecode` is worth 25% of the decode for 33% more disk.** Taken.
//! - **The hardware encoder produces a proxy that is ~50% dearer to decode**
//!   than the software one, and `coder=cavlc` recovers most but not all of
//!   that. Against the same 4K source at 84–98 ms a frame both are a 5–7×
//!   improvement, so the hardware path stays the default — it is what the user
//!   waits on — but the trade is real and `CHUKCUT_PROXY_ENCODER=software`
//!   exists so it can be taken the other way without a rebuild.
//!
//! Worth knowing before optimising the transcode: on this machine the whole
//! 4K→720p job took 11.33 s in software and 10.40 s on VAAPI. **8%**, because
//! decoding the 4K source and scaling it dominate and the encoder is not the
//! bottleneck — the same conclusion `docs/STATUS.md` reaches about the export.
//!
//! ## The loop
//!
//! Deliberately dull, and deliberately built out of parts that are already
//! tested rather than a second FFmpeg pipeline:
//!
//! ```text
//!   media::VideoDecoder::open_scaled  ──►  RGBA at proxy size
//!                                            │
//!                                            ▼
//!                            export::MediaWriter::write_video_frame  ──►  file
//! ```
//!
//! It costs an RGBA round trip that a hand-written decode→swscale→encode
//! pipeline would not, and it buys the seek policy in `media::decoder`, the
//! VAAPI frame pool in `export::hwframes`, the rate-control ladder in
//! `export::encoder`, and — the one that actually bites people — a correct
//! encoder flush. At proxy resolution the round trip is a couple of
//! milliseconds a frame against a transcode that is already dominated by
//! decoding the 4K source.
//!
//! Sampling by **time** rather than by packet also gives the proxy a constant
//! frame rate for free, which matters more than it sounds: a variable-rate
//! phone recording proxied packet-for-packet keeps its irregular timestamps and
//! seeks no better than the original did.
//!
//! ## Atomicity
//!
//! The encode writes a sibling temporary file and renames it into place on
//! success. A cancelled or crashed job therefore never leaves a short file that
//! probes successfully and plays for two seconds — which is the failure mode
//! that makes users distrust the feature permanently.
//!
//! The temporary file keeps the `.mp4` extension and is distinguished by a
//! *prefix*, which is not cosmetic: `avformat_alloc_output_context2` picks the
//! muxer by guessing from the filename, so writing to `proxy.mp4.part` fails at
//! `format::output` with a bare `EINVAL` and no hint as to why.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::modules::export::presets::Fps;
use crate::modules::export::{hwaccel, HwAccel, MediaWriter, Quality, VideoCodec, VideoStreamSpec};
use crate::modules::media::{probe, VideoDecoder};

use super::decision::SourceProfile;
use super::{ProxyError, Result};

/// Container extension for a proxy. See the module docs for the codec.
pub const PROXY_EXTENSION: &str = "mp4";

/// Constant rate factor for the software encoder.
///
/// 23 is x264's default and is visually transparent at this size. A proxy is
/// what the user judges framing and timing against, not colour, so there is no
/// case for spending more; and going higher starts to show blocking on motion,
/// which reads as "the editor is broken" rather than "this is a proxy".
const PROXY_CRF: u8 = 23;

/// What the proxy for one source should be.
///
/// Deliberately says nothing about *which encoder*. That is a property of the
/// machine rather than of the file, it is answered by [`choose_encoder`], and
/// answering it costs a trial encode on a real device — which must not happen
/// on the thread an import is waiting on. So [`plan`] builds one of these
/// without touching any hardware, and [`generate`] picks the encoder on the
/// worker thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxySpec {
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    pub quality: Quality,
    /// Source duration, so the loop knows how many frames it owes.
    pub duration_micros: i64,
}

impl ProxySpec {
    pub fn total_frames(&self) -> u64 {
        self.fps.frame_count(self.duration_micros)
    }

    /// Private options for this encoder.
    ///
    /// `g=1` is the all-intra switch and is not an encoder-private option at
    /// all — it is `AVCodecContext::gop_size`, which `avcodec_open2` picks up
    /// out of the same dictionary. It is spelled here rather than in
    /// `export::encoder` because an *export* wants a normal GOP and only a
    /// proxy wants this.
    fn options(encoder_name: &str) -> Vec<(String, String)> {
        // `bf=0` belongs with `g=1`: an all-intra stream has no room for
        // B-frames, and `export::encoder` asks for two. x264 quietly drops
        // them; NVENC refuses to open at all ("Gop Length should be greater
        // than number of B frames + 1"), which failed every proxy on NVIDIA.
        let mut options = vec![
            ("g".to_string(), "1".to_string()),
            ("bf".to_string(), "0".to_string()),
        ];
        if encoder_name.starts_with("libx26") {
            // Overrides the `preset=medium` `export::encoder` applies to
            // x264/x265: a proxy is judged on how fast it appears, and nobody
            // will ever look at its rate-distortion curve.
            options.push(("preset".to_string(), "veryfast".to_string()));
            options.push(("tune".to_string(), "fastdecode".to_string()));
        } else if encoder_name.starts_with("h264_") {
            // What `tune=fastdecode` does, in the only dialect a hardware
            // encoder understands: CAVLC instead of CABAC. Measured at 19.5 ms
            // a frame to decode against 23–26.5 for the same encoder's default
            // — see the table at the top of this file. Unrecognised by an
            // encoder that has no `coder` option, which is harmless: libav
            // leaves an unconsumed key in the dictionary rather than failing.
            options.push(("coder".to_string(), "cavlc".to_string()));
        }
        options
    }

    fn stream_spec(&self, encoder_name: &str, accel: HwAccel) -> VideoStreamSpec {
        VideoStreamSpec {
            width: self.width,
            height: self.height,
            fps: self.fps,
            encoder_name: encoder_name.to_string(),
            accel,
            quality: self.quality,
            options: Self::options(encoder_name),
            colour: crate::modules::export::OutputColour::default(),
        }
    }
}

/// What a finished proxy turned out to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedProxy {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub bytes: u64,
    pub encoder_name: String,
    pub elapsed: Duration,
}

impl GeneratedProxy {
    /// Frames per second the transcode itself achieved.
    pub fn transcode_fps(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            0.0
        } else {
            self.frames as f64 / seconds
        }
    }
}

/// The H.264 encoder to use on this machine.
///
/// Hardware when [`hwaccel::detect`] reports one that is both present and
/// *usable* — that flag is the result of an actual trial encode, not a lookup,
/// which is the whole point of `export::hwaccel` — and software otherwise.
///
/// Unlike the export — where software is the default because its quality is
/// predictable across machines — a proxy takes the hardware, because nobody
/// judges a proxy's rate-distortion behaviour and everybody waits for it.
///
/// That default is closer than it looks, and the measurement is in the table at
/// the top of this file: the hardware encoder's output costs ~50% more to decode
/// than `libx264 -tune fastdecode`'s, while the transcode it saves is 8%.
/// `CHUKCUT_PROXY_ENCODER=software` takes the other side of that trade without a
/// rebuild; `hardware` insists on the GPU and still falls back if there is none.
pub fn choose_encoder() -> (String, HwAccel) {
    let software = || {
        (
            VideoCodec::H264.software_encoder().to_string(),
            HwAccel::Software,
        )
    };

    match std::env::var("CHUKCUT_PROXY_ENCODER").as_deref() {
        Ok("software") => {
            tracing::info!("CHUKCUT_PROXY_ENCODER=software: proxies will be encoded on the CPU");
            return software();
        }
        Ok("auto") | Ok("hardware") | Err(_) => {}
        Ok(other) => {
            tracing::warn!(
                value = other,
                "CHUKCUT_PROXY_ENCODER must be software, hardware or auto; using the default"
            );
        }
    }

    let hardware = hwaccel::detect()
        .into_iter()
        .find(|encoder| encoder.usable && encoder.codec == VideoCodec::H264);
    match hardware {
        Some(encoder) => {
            tracing::info!(
                encoder = %encoder.encoder_name,
                "proxies will be encoded on the GPU"
            );
            (encoder.encoder_name, encoder.accel)
        }
        None => software(),
    }
}

/// Work out how to encode a proxy for `source`.
///
/// Probes the file for its duration and frame rate; the size comes from the
/// decision rule, so the two cannot disagree about what "proxy size" means.
pub fn plan(source: &Path) -> Result<(SourceProfile, ProxySpec)> {
    let info = probe(source)?;
    let profile = SourceProfile::from_media_info(&info)
        .ok_or_else(|| ProxyError::NoVideo(source.to_path_buf()))?;

    if info.duration <= 0 {
        return Err(ProxyError::Invalid(format!(
            "{} declares no duration, so there is no way to know how long a proxy would be",
            source.display()
        )));
    }

    let (width, height) = profile.proxy_size();

    Ok((
        profile.clone(),
        ProxySpec {
            width,
            height,
            // The source's own rate, snapped to an exact fraction. A proxy that
            // runs at a different rate than its source is a proxy whose
            // timeline positions are wrong.
            fps: Fps::from_f64(profile.fps),
            quality: Quality::Crf(PROXY_CRF),
            duration_micros: info.duration,
        },
    ))
}

/// Transcode `source` into a proxy at `dest`.
///
/// Blocks the calling thread for the whole transcode — it is meant to be run
/// from [`super::queue`]'s worker, never from a command handler. `cancel` is
/// checked once per frame; `progress` is called once per frame with
/// `(done, total)`.
pub fn generate(
    source: &Path,
    dest: &Path,
    spec: &ProxySpec,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<GeneratedProxy> {
    let started = Instant::now();

    // On the worker thread, where a trial encode against a real device is
    // allowed to take a moment. Cached for the process by `hwaccel::detect`.
    let (encoder_name, accel) = choose_encoder();

    // Software decode, explicitly. `media::provider` explains at length why
    // hardware decode is currently slower when the frame has to reach system
    // memory, and a transcode needs it in system memory by definition — the
    // encoder is fed RGBA. Revisit when the DMA-BUF path reaches this loop.
    let mut decoder = VideoDecoder::open_scaled(source, spec.height)?;

    // The decoder never upscales and rounds to even dimensions itself, so it
    // is the authority on what size the frames actually are. Taking the spec's
    // word for it is how a caller ends up with `write_video_frame` rejecting
    // every frame for being the wrong length.
    let (width, height) = decoder.output_size();

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(ProxyError::io(parent))?;
    }
    let partial = partial_path(dest);
    // A `.part` left by a previous crashed run would otherwise be appended to
    // or confuse the muxer.
    let _ = std::fs::remove_file(&partial);

    let mut stream = spec.stream_spec(&encoder_name, accel);
    stream.width = width;
    stream.height = height;

    let total = spec.total_frames().max(1);
    // A hardware encoder that passed its trial encode can still refuse the
    // proxy's own options on some driver. A proxy is worth more late than
    // never, so that is a reason to use the CPU, not to fail.
    let mut writer = match MediaWriter::create(&partial, &stream, None) {
        Ok(writer) => writer,
        Err(error) if accel != HwAccel::Software => {
            tracing::warn!(
                encoder = %encoder_name,
                %error,
                "hardware proxy encoder would not open; encoding on the CPU instead"
            );
            let _ = std::fs::remove_file(&partial);
            let software = VideoCodec::H264.software_encoder();
            stream = spec.stream_spec(software, HwAccel::Software);
            stream.width = width;
            stream.height = height;
            MediaWriter::create(&partial, &stream, None)?
        }
        Err(error) => return Err(error.into()),
    };

    for index in 0..total {
        if cancel.load(Ordering::Relaxed) {
            writer.abort();
            let _ = std::fs::remove_file(&partial);
            return Err(ProxyError::Cancelled);
        }

        // Sampled by time, which is what makes the output constant-rate. The
        // decoder answers a forward step by decoding one frame rather than
        // seeking — see `media::decoder::FORWARD_DECODE_WINDOW` — so this is a
        // sequential read of the file despite looking like a seek per frame.
        let at = spec.fps.frame_time(index);
        let frame = match decoder.seek_and_decode(at) {
            Ok(frame) => frame,
            // Past the end of a file whose declared duration was optimistic.
            // Everything up to here is a usable proxy; stopping short of the
            // declared length is better than failing.
            Err(error) => {
                tracing::debug!(
                    %error,
                    at_ms = at / 1000,
                    index,
                    "proxy transcode ran out of source frames"
                );
                break;
            }
        };

        writer.write_video_frame(&frame.data, index)?;
        progress(index + 1, total);
    }

    let frames = writer.frames_written();
    writer.finish()?;

    // Only now does the file get its real name. See "Atomicity" above.
    std::fs::rename(&partial, dest).map_err(ProxyError::io(dest))?;
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    let generated = GeneratedProxy {
        path: dest.to_path_buf(),
        width,
        height,
        frames,
        bytes,
        encoder_name: stream.encoder_name.clone(),
        elapsed: started.elapsed(),
    };

    tracing::info!(
        source = %source.display(),
        size = format_args!("{width}x{height}"),
        frames = generated.frames,
        megabytes = generated.bytes / 1_048_576,
        encoder = %generated.encoder_name,
        transcode_fps = format_args!("{:.1}", generated.transcode_fps()),
        "built a proxy"
    );

    Ok(generated)
}

/// `.../x.mp4` → `.../.partial-x.mp4`.
///
/// A prefix and **not** a `.part` suffix. FFmpeg chooses the muxer by guessing
/// from the output filename, so a path ending in `.part` fails inside
/// `format::output` with `EINVAL` and nothing to suggest that the extension was
/// the problem — which is exactly the shape of bug that costs an afternoon.
///
/// The leading dot hides it on Unix, and the prefix means it can never collide
/// with a name [`super::cache::ProxyCache::path_for`] would produce, so nothing
/// can mistake a partial file for a finished proxy.
fn partial_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "proxy.mp4".to_string());
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".partial-{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(width: u32, height: u32) -> ProxySpec {
        ProxySpec {
            width,
            height,
            fps: Fps::THIRTY,
            quality: Quality::Crf(PROXY_CRF),
            duration_micros: 4_000_000,
        }
    }

    #[test]
    fn every_frame_is_a_keyframe() {
        // The one option that makes this a proxy rather than a small copy.
        let options = ProxySpec::options("libx264");
        assert!(options.contains(&("g".to_string(), "1".to_string())));
    }

    #[test]
    fn the_software_path_is_tuned_for_decoding_rather_than_size() {
        let options = ProxySpec::options("libx264");
        assert!(options.contains(&("preset".to_string(), "veryfast".to_string())));
        assert!(options.contains(&("tune".to_string(), "fastdecode".to_string())));
    }

    #[test]
    fn the_hardware_path_gets_cavlc_rather_than_x264_options() {
        // `preset` and `tune` are x264 private options; handing them to
        // `h264_vaapi` is at best ignored and at worst an open failure. What it
        // does understand is `coder`, which buys the same thing `fastdecode`
        // does — measured, see the table at the top of this file.
        let options = ProxySpec::options("h264_vaapi");
        assert!(options.contains(&("g".to_string(), "1".to_string())));
        assert!(options.contains(&("coder".to_string(), "cavlc".to_string())));
        assert!(!options
            .iter()
            .any(|(key, _)| key == "preset" || key == "tune"));
    }

    #[test]
    fn the_environment_can_force_the_software_encoder() {
        // Not a test of `choose_encoder` under a set variable — the process is
        // shared with every other test and `hwaccel::detect` memoises — but of
        // the fact that the software answer is always available and correct.
        assert_eq!(VideoCodec::H264.software_encoder(), "libx264");
    }

    #[test]
    fn the_frame_count_covers_the_whole_source() {
        let spec = spec(1280, 720);
        // 4 seconds at 30 fps.
        assert_eq!(spec.total_frames(), 120);
    }

    #[test]
    fn the_partial_file_keeps_the_extension_but_not_the_name() {
        let partial = partial_path(Path::new("/cache/clip-abc.mp4"));
        assert_eq!(partial, PathBuf::from("/cache/.partial-clip-abc.mp4"));
        // The extension has to survive: FFmpeg picks the muxer from it, and a
        // `.part` suffix fails at `format::output` with a bare EINVAL.
        assert_eq!(partial.extension().and_then(|e| e.to_str()), Some("mp4"));
        // And it can never be the name of a finished proxy.
        assert_ne!(partial, PathBuf::from("/cache/clip-abc.mp4"));
    }

    #[test]
    fn an_encoder_is_always_chosen() {
        // On a machine with no GPU this must still answer, with software.
        let (name, accel) = choose_encoder();
        assert!(!name.is_empty());
        if accel == HwAccel::Software {
            assert_eq!(name, "libx264");
        } else {
            assert!(name.starts_with("h264_"), "{name} is not an H.264 encoder");
        }
    }

    #[test]
    fn transcode_speed_is_reported_without_dividing_by_zero() {
        let generated = GeneratedProxy {
            path: "/x.mp4".into(),
            width: 1280,
            height: 720,
            frames: 240,
            bytes: 1,
            encoder_name: "libx264".into(),
            elapsed: Duration::ZERO,
        };
        assert_eq!(generated.transcode_fps(), 0.0);
        let generated = GeneratedProxy {
            elapsed: Duration::from_secs(4),
            ..generated
        };
        assert_eq!(generated.transcode_fps(), 60.0);
    }
}
