//! Writing an output file: one container, one video stream, at most one audio
//! stream.
//!
//! ## The three things that go wrong here
//!
//! **Time bases.** There are two, and they are not the same. The *encoder's*
//! time base is one tick per frame (`1/fps`), which makes a frame's PTS equal
//! to its index and makes fractional rates exact: 29.97 is `1001/30000`, and a
//! frame index in that base can never drift. The *stream's* time base is
//! whatever the muxer wants — MP4 rewrites it to a timescale of its own choice
//! inside `write_header` — so every packet is rescaled on its way out. Reading
//! the stream time base back *after* `write_header` rather than before is not a
//! detail; use the value you set and every timestamp in the file is wrong by
//! the ratio between the two.
//!
//! **The flush.** An encoder holds frames: lookahead, B-frame reordering and
//! rate control all mean the packet for frame N comes out after frame N+3 went
//! in. At the end of the timeline the last second or two of video is still
//! inside libavcodec, and it only comes out when it is told the stream ended
//! (`send_eof`, then receive until `Eof`). Skipping that produces a file that
//! plays perfectly and stops early — the classic truncated tail, and the reason
//! [`MediaWriter::finish`] consumes `self`: there is no way to write the file
//! without flushing it.
//!
//! **Strides.** A libav frame is not a tightly packed buffer. Its rows are
//! padded to whatever alignment the allocator picked, so a 1080-wide RGBA plane
//! usually has a stride larger than 4320 bytes. Copying the compositor's packed
//! output in one `copy_from_slice` produces a diagonally sheared picture; the
//! copy has to go row by row.
//!
//! ## Colour
//!
//! The compositor hands over sRGB-encoded RGBA, and what it becomes is decided
//! by [`super::colour::OutputColour`]: BT.709 for HD, BT.601 for SD, limited
//! range, unless the user said otherwise. Both conversions honour it — swscale
//! through `sws_setColorspaceDetails` (without which it is BT.601 whatever the
//! context is for; the same trap `media::decoder` documents from the decode
//! side) and the GPU's NV12 pass through `YuvEncoding` — and the stream is
//! tagged with primaries, transfer, matrix and range before the encoder opens,
//! so the bitstream's VUI and the container both say what the samples are.
//!
//! ## The hardware path
//!
//! VAAPI and QSV do not take a frame in system memory. They take a surface out
//! of an `AVHWFramesContext` that the codec context was told about *before*
//! `avcodec_open2`. So for those two the sequence gains three steps, all of
//! them in [`hwframes`]:
//!
//! ```text
//!   RGBA ──swscale──► NV12 (system memory) ──av_hwframe_transfer_data──► VA surface ──► encoder
//! ```
//!
//! rather than the software path's `RGBA ──swscale──► YUV420P ──► encoder`.
//! The readback-and-upload is deliberate and temporary: the frame we hand to
//! swscale came out of a GPU texture a moment earlier, so the pixels make a
//! round trip they should not have to make. Removing it needs DMA-BUF export
//! from wgpu and is written up in `docs/research/zero-copy-encode.md`.
//!
//! Everything after `send_frame` — the drain, the rescale, the flush — is
//! identical for both paths, deliberately. The truncated tail is the classic
//! hardware-encode bug precisely because people write a second, subtly
//! different flush for it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ffmpeg::software::{resampling, scaling};
use ffmpeg::util::frame;
use ffmpeg::{codec, format, ChannelLayout, Dictionary, Packet, Rational};
use ffmpeg_next as ffmpeg;

use super::colour::OutputColour;
use super::hwaccel::{self, HwAccel, RateControl};
use super::hwframes::{self, HwDeviceContext, HwFramesContext};
use super::presets::{Fps, Quality};
use super::{ExportError, Result};

/// Keyframe interval, in seconds.
///
/// Two seconds is what every streaming platform asks for and what seeking in a
/// player wants; longer compresses better but makes scrubbing the exported file
/// unpleasant, which is what people do immediately after an export finishes.
const GOP_SECONDS: f64 = 2.0;

/// Sample frames per audio packet when the encoder does not care.
///
/// PCM and a few others report a frame size of zero, meaning "any size". 1024
/// matches what AAC uses, so the muxer sees a similar packet cadence either way.
const DEFAULT_AUDIO_FRAME: usize = 1024;

#[derive(Debug, Clone)]
pub struct VideoStreamSpec {
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    /// FFmpeg's name for the encoder: `libx264`, `h264_nvenc`, …
    pub encoder_name: String,
    /// Which API `encoder_name` belongs to, so quality options can be spelled
    /// in its dialect.
    pub accel: HwAccel,
    pub quality: Quality,
    /// Extra private options, applied last so a caller can override anything
    /// this file decides.
    pub options: Vec<(String, String)>,
    /// The matrix, range and bit depth the frames are converted into, and
    /// what the stream is tagged as.
    pub colour: OutputColour,
}

impl VideoStreamSpec {
    /// The rate-control configurations to try at `avcodec_open2`, best first.
    fn rate_controls(&self) -> Vec<RateControl> {
        if self.has_no_rate_control() {
            return vec![RateControl {
                label: "the codec's own data rate",
                bit_rate: 0,
                options: Vec::new(),
            }];
        }
        let fallback = hwaccel::fallback_bitrate(self.width, self.height, self.fps, self.quality);
        self.accel.rate_control_ladder(self.quality, fallback)
    }

    /// The private options this encoder is opened with, for a given rung of the
    /// rate-control ladder.
    fn dictionary_for(&self, rate_control: &RateControl) -> Vec<(String, String)> {
        let mut options: Vec<(String, String)> = Vec::new();

        // x264/x265 without a preset default to "medium", which is the right
        // trade for an export: veryfast throws away 20% of the bitrate budget,
        // slow doubles the wall clock for a difference nobody sees on a phone.
        if matches!(self.encoder_name.as_str(), "libx264" | "libx265") {
            options.push(("preset".into(), "medium".into()));
        }
        // Every consumer decoder wants 4:2:0 8-bit; x265 in particular will
        // happily default to a 10-bit profile that half the world cannot play.
        // Main 10 only when the user asked for 10-bit, and then for every HEVC
        // encoder: NVENC and QSV otherwise open Main and refuse P010.
        if self.encoder_name == "libx265" {
            let profile = if self.colour.ten_bit {
                "main10"
            } else {
                "main"
            };
            options.push(("profile".into(), profile.into()));
        }
        if self.colour.ten_bit
            && matches!(
                self.encoder_name.as_str(),
                "hevc_nvenc" | "hevc_qsv" | "hevc_vaapi"
            )
        {
            options.push(("profile".into(), "main10".into()));
        }
        // ProRes 422 HQ: what "a ProRes master" means to every application
        // that will open the file. Profile 3 in prores_ks's numbering.
        if self.encoder_name == "prores_ks" {
            options.push(("profile".into(), "3".into()));
            options.push(("vendor".into(), "apl0".into()));
        }

        options.extend(rate_control.options.iter().cloned());
        options.extend(self.options.iter().cloned());
        options
    }

    /// The options for the first rung — what this encoder is opened with unless
    /// the driver refuses.
    #[cfg(test)]
    fn dictionary(&self) -> Vec<(String, String)> {
        let ladder = self.rate_controls();
        self.dictionary_for(&ladder[0])
    }

    /// Whether the encoder needs its frames to come out of a hardware pool.
    fn needs_frame_pool(&self) -> bool {
        !self.accel.accepts_software_frames()
    }

    /// What swscale converts the compositor's RGBA into.
    fn upload_format(&self) -> format::Pixel {
        match self.encoder_name.as_str() {
            // prores_ks takes 10-bit 4:2:2 and nothing 8-bit.
            "prores_ks" => format::Pixel::YUV422P10LE,
            "gif" => format::Pixel::PAL8,
            _ => match (self.accel.upload_format(), self.colour.ten_bit) {
                (format, false) => format,
                // The 10-bit twin of each 8-bit layout: P010 is NV12 with
                // sixteen bits a sample, the top ten used, and it is what
                // NVENC, VAAPI and QSV take for Main 10.
                (format::Pixel::NV12, true) => format::Pixel::P010LE,
                (_, true) => format::Pixel::YUV420P10LE,
            },
        }
    }

    /// Whether the stream gets colour tags. A GIF is RGB with a palette and
    /// has nowhere to put them.
    fn is_tagged(&self) -> bool {
        self.encoder_name != "gif"
    }

    /// Intra-only codecs without a quality knob: no CRF, no bitrate, no
    /// B-frames.
    fn has_no_rate_control(&self) -> bool {
        matches!(self.encoder_name.as_str(), "prores_ks" | "gif")
    }
}

#[derive(Debug, Clone)]
pub struct AudioStreamSpec {
    /// `aac`, `libopus`, …
    pub encoder_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Bits per second.
    pub bitrate: u32,
}

impl AudioStreamSpec {
    fn layout(&self) -> ChannelLayout {
        if self.channels <= 1 {
            ChannelLayout::MONO
        } else {
            ChannelLayout::STEREO
        }
    }
}

/// An output file being written.
///
/// Created open: the container exists and the header is on disk by the time
/// `create` returns, so a permission or codec problem surfaces before the first
/// frame is rendered rather than after ten minutes of work.
pub struct MediaWriter {
    path: PathBuf,
    octx: format::context::Output,
    /// `None` for a sound-only file (.m4a, .mp3, .wav).
    video: Option<VideoTrack>,
    audio: Option<AudioTrack>,
    /// Set by `finish`. A writer dropped without it leaves an unplayable file,
    /// which `Drop` complains about loudly.
    finished: bool,
    stats: WriterStats,
}

/// What one video frame cost between the compositor and the muxer.
///
/// The counterpart of `render::RenderStats`, and here for the same reason: the
/// export's frame time is spread over four stages and a change to any one of
/// them moves the same single number. Nanoseconds, summed over every frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub frames: u64,
    /// Building the `AVFrame` the encoder is given. On the RGBA path this is
    /// the padded row copy plus swscale; on the NV12 path it is two plane
    /// copies and no conversion at all. The difference between the two is what
    /// the GPU colour conversion bought downstream of the compositor.
    pub prepare_ns: u64,
    /// Taking a surface, `av_hwframe_transfer_data` into it, `send_frame`, and
    /// draining whatever packets came back.
    pub submit_ns: u64,
    /// Bytes of encoded video and audio handed to the muxer, without the
    /// container's own headers and index. The size estimate samples these.
    pub video_bytes: u64,
    pub audio_bytes: u64,
}

impl WriterStats {
    pub fn per_frame(&self, total_ns: u64) -> f64 {
        if self.frames == 0 {
            0.0
        } else {
            total_ns as f64 / self.frames as f64
        }
    }
}

struct VideoTrack {
    encoder: ffmpeg::encoder::video::Encoder,
    stream_index: usize,
    /// `1/fps`: one tick per frame.
    time_base: Rational,
    /// What the muxer settled on, read back after `write_header`.
    stream_time_base: Rational,
    /// RGBA to the encoder's format. `None` for a GIF, whose palette this
    /// file builds itself (`quantize_pal8`): swscale cannot write PAL8.
    scaler: Option<scaling::Context>,
    /// The surface pool, when the encoder will not take system memory.
    ///
    /// Kept alive for the writer's whole life: the codec context holds its own
    /// reference, but every frame that goes in comes out of *this* handle.
    hw: Option<HwFramesContext>,
    /// The software pixel format the encoder — or its surface pool — is fed.
    /// Kept so `write_video_frame_nv12` can refuse a mismatch in prose rather
    /// than letting libavutil answer `EINVAL`.
    upload_format: format::Pixel,
    width: u32,
    height: u32,
    frames: u64,
}

struct AudioTrack {
    encoder: ffmpeg::encoder::audio::Encoder,
    stream_index: usize,
    /// `1/sample_rate`: one tick per sample frame.
    time_base: Rational,
    stream_time_base: Rational,
    resampler: resampling::Context,
    layout: ChannelLayout,
    channels: usize,
    rate: u32,
    /// Sample frames the encoder wants per packet.
    frame_size: usize,
    /// Interleaved f32 that did not fill a whole encoder frame yet.
    pending: Vec<f32>,
    /// Sample frames handed to the encoder so far; this is the audio PTS.
    written: i64,
}

impl MediaWriter {
    pub fn create(
        path: &Path,
        video: &VideoStreamSpec,
        audio: Option<&AudioStreamSpec>,
    ) -> Result<Self> {
        Self::open(path, Some(video), audio)
    }

    /// A file with sound and no picture: .m4a, .mp3, .wav.
    pub fn create_audio_only(path: &Path, audio: &AudioStreamSpec) -> Result<Self> {
        Self::open(path, None, Some(audio))
    }

    fn open(
        path: &Path,
        video: Option<&VideoStreamSpec>,
        audio: Option<&AudioStreamSpec>,
    ) -> Result<Self> {
        crate::modules::media::ensure_initialized();

        if video.is_none() && audio.is_none() {
            return Err(ExportError::Settings(
                "a file needs a picture or sound, and this export asked for neither".into(),
            ));
        }

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|source| ExportError::Open {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
        }

        let mut octx = format::output(&path).map_err(ExportError::ffmpeg(format!(
            "cannot create {}",
            path.display()
        )))?;

        // MP4 and MKV keep codec extradata in the header rather than in-band,
        // and an encoder that was not told so emits it per keyframe, producing
        // a file some players refuse.
        let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

        let mut video_track = match video {
            Some(spec) => Some(add_video(&mut octx, spec, global_header)?),
            None => None,
        };
        let mut audio_track = match audio {
            Some(spec) => Some(add_audio(&mut octx, spec, global_header)?),
            None => None,
        };

        octx.write_header()
            .map_err(ExportError::ffmpeg("writing the file header"))?;

        // The muxer is allowed to rewrite the time bases it was handed, and MP4
        // always does.
        if let Some(track) = video_track.as_mut() {
            track.stream_time_base = octx
                .stream(track.stream_index)
                .map(|s| s.time_base())
                .unwrap_or(track.time_base);
        }
        if let Some(track) = audio_track.as_mut() {
            track.stream_time_base = octx
                .stream(track.stream_index)
                .map(|s| s.time_base())
                .unwrap_or(track.time_base);
        }

        Ok(Self {
            path: path.to_path_buf(),
            octx,
            video: video_track,
            audio: audio_track,
            finished: false,
            stats: WriterStats::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the frames written so far cost, by stage.
    pub fn stats(&self) -> WriterStats {
        self.stats
    }

    pub fn frames_written(&self) -> u64 {
        self.video.as_ref().map_or(0, |video| video.frames)
    }

    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }

    /// Sample frames the audio encoder wants per packet, for a caller that
    /// wants to feed it in its natural chunk size.
    pub fn audio_frame_size(&self) -> Option<usize> {
        self.audio.as_ref().map(|a| a.frame_size)
    }

    /// Encode one composited frame.
    ///
    /// `rgba` is exactly `width * height * 4` tightly packed bytes — what
    /// `Compositor::render_frame` returns. `index` is the frame's position in
    /// the output, which *is* its PTS in the encoder's time base.
    pub fn write_video_frame(&mut self, rgba: &[u8], index: u64) -> Result<()> {
        let Some(video) = self.video.as_mut() else {
            return Err(no_picture());
        };
        let expected = video.width as usize * video.height as usize * 4;
        if rgba.len() != expected {
            return Err(ExportError::Settings(format!(
                "the renderer produced {} bytes for a {}x{} frame, expected {expected}",
                rgba.len(),
                video.width,
                video.height
            )));
        }

        let prepared = Instant::now();
        let mut source = frame::Video::new(format::Pixel::RGBA, video.width, video.height);
        copy_packed_rows(&mut source, rgba, video.width as usize * 4);

        let mut converted = match video.scaler.as_mut() {
            Some(scaler) => {
                let mut converted = frame::Video::empty();
                scaler
                    .run(&source, &mut converted)
                    .map_err(ExportError::ffmpeg("converting the frame to YUV"))?;
                converted
            }
            None => quantize_pal8(rgba, video.width, video.height),
        };
        converted.set_pts(Some(index as i64));
        self.stats.prepare_ns += prepared.elapsed().as_nanos() as u64;
        let submitted = Instant::now();

        // On the hardware path the encoder cannot see system memory, so the
        // converted frame is uploaded into a surface and the *surface* is what
        // gets sent. Its PTS has to be set again: `av_hwframe_transfer_data`
        // copies pixels and nothing else, and a surface sent with no PTS makes
        // libavcodec invent one, which is how a hardware export ends up a frame
        // out of step with its own audio.
        match video.hw.as_ref() {
            None => video
                .encoder
                .send_frame(&converted)
                .map_err(ExportError::ffmpeg("encoding a video frame"))?,
            Some(pool) => {
                let mut surface = pool.empty_frame()?;
                pool.upload(&converted, &mut surface)?;
                surface.set_pts(Some(index as i64));
                video
                    .encoder
                    .send_frame(&surface)
                    .map_err(ExportError::ffmpeg("encoding a video frame on the GPU"))?;
            }
        }

        video.frames = video.frames.max(index + 1);
        self.stats.frames += 1;
        let drained = drain_video(&mut self.octx, video, &mut self.stats);
        self.stats.submit_ns += submitted.elapsed().as_nanos() as u64;
        drained
    }

    /// Whether this encoder is fed NV12, and so whether the compositor's GPU
    /// conversion is worth asking for.
    ///
    /// A software encoder wants YUV420P, and handing it NV12 would only move
    /// the swscale pass rather than remove it — so the caller keeps the RGBA
    /// path for those.
    pub fn wants_nv12(&self) -> bool {
        self.video
            .as_ref()
            .is_some_and(|video| video.upload_format == format::Pixel::NV12)
    }

    /// Encode one composited frame that is already NV12.
    ///
    /// The GPU produced the colour conversion, so nothing here calls swscale:
    /// the two planes are copied row by row into an `AVFrame` — because the
    /// GPU's stride and libavutil's are chosen by different people and are only
    /// equal by luck — and that frame takes the same route to the encoder as
    /// the converted one does.
    ///
    /// `y_stride` and `uv_stride` are in bytes and must be at least the frame
    /// width. `uv` holds `height / 2` rows of interleaved Cb/Cr.
    pub fn write_video_frame_nv12(
        &mut self,
        y: &[u8],
        y_stride: usize,
        uv: &[u8],
        uv_stride: usize,
        index: u64,
    ) -> Result<()> {
        let Some(video) = self.video.as_mut() else {
            return Err(no_picture());
        };
        if video.upload_format != format::Pixel::NV12 {
            return Err(ExportError::Settings(format!(
                "this encoder is fed {:?}, not NV12",
                video.upload_format
            )));
        }

        let (width, height) = (video.width as usize, video.height as usize);
        let uv_rows = height.div_ceil(2);
        if y_stride < width || uv_stride < width {
            return Err(ExportError::Settings(format!(
                "an NV12 frame for {width}x{height} needs strides of at least {width} bytes, \
                 got {y_stride} and {uv_stride}"
            )));
        }
        if y.len() < y_stride * height || uv.len() < uv_stride * uv_rows {
            return Err(ExportError::Settings(format!(
                "the renderer produced {} luma and {} chroma bytes for a {width}x{height} \
                 frame, expected at least {} and {}",
                y.len(),
                uv.len(),
                y_stride * height,
                uv_stride * uv_rows
            )));
        }

        let prepared = Instant::now();
        let mut source = frame::Video::new(format::Pixel::NV12, video.width, video.height);
        copy_plane(&mut source, 0, y, y_stride, width, height);
        copy_plane(&mut source, 1, uv, uv_stride, width, uv_rows);
        source.set_pts(Some(index as i64));
        self.stats.prepare_ns += prepared.elapsed().as_nanos() as u64;
        let submitted = Instant::now();

        match video.hw.as_ref() {
            None => video
                .encoder
                .send_frame(&source)
                .map_err(ExportError::ffmpeg("encoding a video frame"))?,
            Some(pool) => {
                let mut surface = pool.empty_frame()?;
                pool.upload(&source, &mut surface)?;
                // Same reason as the RGBA path: the transfer copies pixels and
                // nothing else, so a surface with no PTS makes libavcodec
                // invent one and the export ends up a frame out of step.
                surface.set_pts(Some(index as i64));
                video
                    .encoder
                    .send_frame(&surface)
                    .map_err(ExportError::ffmpeg("encoding a video frame on the GPU"))?;
            }
        }

        video.frames = video.frames.max(index + 1);
        self.stats.frames += 1;
        let drained = drain_video(&mut self.octx, video, &mut self.stats);
        self.stats.submit_ns += submitted.elapsed().as_nanos() as u64;
        drained
    }

    /// Encode a frame the GPU wrote into memory this encoder can read.
    ///
    /// The zero-copy path. `buffer` describes a DMA-BUF the compositor
    /// allocated and its compute pass filled with NV12; nothing here copies
    /// pixels, uploads anything or calls swscale. The returned frame is the
    /// mapped VA surface, and the caller must keep it alive until the encoder
    /// is finished with the memory behind it — see
    /// [`Self::surface_is_still_held`].
    ///
    /// Only available on the hardware path; a software encoder has no surfaces
    /// to map into and is told so.
    #[cfg(target_os = "linux")]
    pub fn write_video_frame_dmabuf(
        &mut self,
        buffer: &hwframes::Nv12Dmabuf<'_>,
        index: u64,
    ) -> Result<frame::Video> {
        let Some(video) = self.video.as_mut() else {
            return Err(no_picture());
        };
        let Some(pool) = video.hw.as_ref() else {
            return Err(ExportError::Settings(
                "a DMA-BUF can only be given to a hardware encoder".into(),
            ));
        };

        let prepared = Instant::now();
        let mut surface = pool.import_nv12_dmabuf(buffer)?;
        surface.set_pts(Some(index as i64));
        self.stats.prepare_ns += prepared.elapsed().as_nanos() as u64;

        let submitted = Instant::now();
        video
            .encoder
            .send_frame(&surface)
            .map_err(ExportError::ffmpeg("encoding a video frame on the GPU"))?;

        video.frames = video.frames.max(index + 1);
        self.stats.frames += 1;
        let drained = drain_video(&mut self.octx, video, &mut self.stats);
        self.stats.submit_ns += submitted.elapsed().as_nanos() as u64;
        drained?;
        Ok(surface)
    }

    /// Feed interleaved f32 samples at the spec's rate and channel count.
    ///
    /// Buffered internally: the encoder wants a fixed number of sample frames
    /// per packet (1024 for AAC, 960 for Opus) and the mixer produces whatever
    /// length is convenient, so this accumulates and emits whole frames.
    pub fn write_audio(&mut self, interleaved: &[f32]) -> Result<()> {
        let Some(audio) = self.audio.as_mut() else {
            return Ok(());
        };
        audio.pending.extend_from_slice(interleaved);

        let chunk = audio.frame_size * audio.channels;
        while audio.pending.len() >= chunk {
            let samples: Vec<f32> = audio.pending.drain(..chunk).collect();
            encode_audio_chunk(
                &mut self.octx,
                audio,
                &samples,
                audio.frame_size,
                &mut self.stats,
            )?;
        }
        Ok(())
    }

    /// Flush both encoders, write the trailer, close the file. Returns the
    /// final stats, which include the packets the flush produced — for a
    /// lookahead encoder that is the last second or two of the file.
    ///
    /// Consumes `self` because there is no valid state after it and no reason
    /// to allow a second call.
    pub fn finish(mut self) -> Result<WriterStats> {
        if let Some(audio) = self.audio.as_mut() {
            // The tail is normally a partial frame. Encoders accept a short
            // final frame; dropping it loses up to 20 ms off the end, which is
            // audible as a clipped last word.
            if !audio.pending.is_empty() {
                let samples: Vec<f32> = std::mem::take(&mut audio.pending);
                let frames = samples.len() / audio.channels.max(1);
                if frames > 0 {
                    encode_audio_chunk(&mut self.octx, audio, &samples, frames, &mut self.stats)?;
                }
            }
            audio
                .encoder
                .send_eof()
                .map_err(ExportError::ffmpeg("flushing the audio encoder"))?;
            drain_audio(&mut self.octx, audio, &mut self.stats)?;
        }

        if let Some(video) = self.video.as_mut() {
            video
                .encoder
                .send_eof()
                .map_err(ExportError::ffmpeg("flushing the video encoder"))?;
            drain_video(&mut self.octx, video, &mut self.stats)?;
        }

        self.octx
            .write_trailer()
            .map_err(ExportError::ffmpeg("finalising the file"))?;
        self.finished = true;
        Ok(self.stats)
    }

    /// Give up and remove the half-written file.
    ///
    /// A cancelled export must not leave something that looks like a finished
    /// video in the user's folder.
    pub fn abort(mut self) {
        self.finished = true;
        let path = std::mem::take(&mut self.path);
        // The muxer holds the file handle; it has to go first, and on Windows
        // the removal would fail otherwise.
        drop(self);
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%error, path = %path.display(), "could not remove the aborted export");
            }
        }
    }
}

impl Drop for MediaWriter {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                path = %self.path.display(),
                "export writer dropped without finish(); the file has no trailer and will not play"
            );
        }
    }
}

impl std::fmt::Debug for MediaWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaWriter")
            .field("path", &self.path)
            .field(
                "size",
                &self.video.as_ref().map(|video| (video.width, video.height)),
            )
            .field("frames", &self.frames_written())
            .field("audio", &self.audio.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Stream setup
// ---------------------------------------------------------------------------

/// Allocate a codec context that already knows which encoder it is for.
///
/// `ffmpeg-next` only wraps `avcodec_alloc_context3(NULL)`, which leaves the
/// context on libavcodec's *generic* defaults. Every encoder with a private
/// option block — libx264, libx265, NVENC — expects its own defaults from
/// `priv_class`, and those are only filled in when the codec is passed to the
/// allocation. x264 explicitly detects the generic ones and refuses to open
/// with "broken ffmpeg default settings detected", which is what this one
/// unsafe call buys its way out of.
fn context_for(codec: ffmpeg::Codec) -> Result<codec::context::Context> {
    let ptr = unsafe { ffmpeg::ffi::avcodec_alloc_context3(codec.as_ptr()) };
    if ptr.is_null() {
        return Err(ExportError::Ffmpeg {
            what: format!("cannot allocate a context for {}", codec.name()),
            source: ffmpeg::Error::Unknown,
        });
    }
    Ok(unsafe { codec::context::Context::wrap(ptr, None) })
}

/// An opened video encoder, plus the surface pool it draws frames from.
struct OpenVideoEncoder {
    encoder: ffmpeg::encoder::video::Encoder,
    hw: Option<HwFramesContext>,
}

/// Open the video encoder, walking down the rate-control ladder.
///
/// Each attempt builds a *fresh* codec context, because `avcodec_open2` is not
/// idempotent — a context that failed to open cannot be reconfigured and tried
/// again, and `ffmpeg-next`'s `open_as_with` consumes it anyway. The frame pool
/// is rebuilt with it: the codec context takes its own reference during the
/// open, and disentangling a half-open one is not worth the fifty milliseconds
/// it would save on a path that normally succeeds on the first rung.
fn open_video_encoder(
    spec: &VideoStreamSpec,
    codec: ffmpeg::Codec,
    global_header: bool,
) -> Result<OpenVideoEncoder> {
    let (tb_num, tb_den) = spec.fps.time_base();
    let time_base = Rational::new(tb_num, tb_den);
    let frame_rate = Rational::new(spec.fps.num as i32, spec.fps.den as i32);

    let ladder = spec.rate_controls();
    let mut last_error: Option<ExportError> = None;

    for (rung, rate_control) in ladder.iter().enumerate() {
        let mut encoder = context_for(codec)?
            .encoder()
            .video()
            .map_err(ExportError::ffmpeg("preparing the video encoder"))?;

        encoder.set_width(spec.width);
        encoder.set_height(spec.height);
        encoder.set_time_base(time_base);
        encoder.set_frame_rate(Some(frame_rate));
        encoder.set_gop((spec.fps.as_f64() * GOP_SECONDS).round().max(1.0) as u32);
        // Two B-frames is the standard trade: meaningful compression, still
        // within what every hardware decoder handles. A VAAPI driver that does
        // not do B-frames — Intel's low-power entrypoint is the common case —
        // clamps this to zero inside `avcodec_open2` and logs it, rather than
        // failing, so it is safe to ask for on both paths.
        encoder.set_max_b_frames(if spec.has_no_rate_control() { 0 } else { 2 });
        // What the samples are. Set before the open, because that is when
        // libx264, libx265, NVENC, VAAPI and QSV copy them into the
        // bitstream's VUI; `set_parameters` later copies them onto the stream
        // for the muxer's `colr` atom. A stream with no tags is read as BT.709
        // by every player for HD and as BT.601 for SD — which is the rule
        // `OutputColour` follows — but a tag is an answer and a convention is
        // a guess.
        if spec.is_tagged() {
            encoder.set_colorspace(spec.colour.space());
            encoder.set_color_primaries(spec.colour.primaries());
            encoder.set_color_transfer_characteristic(spec.colour.transfer());
            encoder.set_color_range(spec.colour.range());
        }

        if rate_control.bit_rate > 0 {
            encoder.set_bit_rate(rate_control.bit_rate as usize);
            // A 1.5x ceiling keeps a hard cut from blowing the buffer without
            // making the average meaningless.
            let ceiling = rate_control.bit_rate + rate_control.bit_rate / 2;
            encoder.set_max_bit_rate(ceiling as usize);
        }
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        // The pixel format the *encoder* sees. On the hardware path that is the
        // opaque surface format, not the pixels: `AV_PIX_FMT_VAAPI` says "look
        // in hw_frames_ctx for what these really are".
        let pool = if spec.needs_frame_pool() {
            let Some((device_kind, surface_format)) = spec.accel.frame_pool() else {
                return Err(ExportError::Settings(format!(
                    "{} needs a hardware frame pool and does not say which",
                    spec.accel.label()
                )));
            };
            encoder.set_format(surface_format);
            let device =
                HwDeviceContext::for_kind(device_kind, Some(hwframes::DEFAULT_RENDER_NODE))?;
            let pool = device.frames(
                surface_format,
                spec.upload_format(),
                spec.width,
                spec.height,
            )?;
            pool.attach_to_encoder(&mut encoder)?;
            Some(pool)
        } else {
            encoder.set_format(spec.upload_format());
            None
        };

        let mut options = Dictionary::new();
        for (key, value) in spec.dictionary_for(rate_control) {
            options.set(&key, &value);
        }

        match encoder.open_as_with(codec, options) {
            Ok(opened) => {
                if rung > 0 {
                    tracing::info!(
                        encoder = %spec.encoder_name,
                        "this driver refused the requested rate control; using {}",
                        rate_control.label
                    );
                }
                return Ok(OpenVideoEncoder {
                    encoder: opened,
                    hw: pool,
                });
            }
            Err(source) => {
                tracing::debug!(
                    encoder = %spec.encoder_name,
                    rate_control = rate_control.label,
                    %source,
                    "rate-control rung refused"
                );
                last_error = Some(ExportError::Ffmpeg {
                    what: format!(
                        "cannot open the {} encoder with {}",
                        spec.encoder_name, rate_control.label
                    ),
                    source,
                });
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ExportError::Settings(format!(
            "cannot open the {} encoder: no rate-control mode to try",
            spec.encoder_name
        ))
    }))
}

fn add_video(
    octx: &mut format::context::Output,
    spec: &VideoStreamSpec,
    global_header: bool,
) -> Result<VideoTrack> {
    let codec = ffmpeg::encoder::find_by_name(&spec.encoder_name)
        .ok_or_else(|| ExportError::NoEncoder(spec.encoder_name.clone()))?;

    let (tb_num, tb_den) = spec.fps.time_base();
    let time_base = Rational::new(tb_num, tb_den);
    let frame_rate = Rational::new(spec.fps.num as i32, spec.fps.den as i32);

    let stream_index;
    {
        let mut ost = octx
            .add_stream(codec)
            .map_err(ExportError::ffmpeg("adding the video stream"))?;
        stream_index = ost.index();
        ost.set_time_base(time_base);
        ost.set_avg_frame_rate(frame_rate);
    }

    let OpenVideoEncoder {
        encoder: opened,
        hw,
    } = open_video_encoder(spec, codec, global_header)?;

    // The stream's parameters have to describe the *opened* encoder: before
    // `open` there is no extradata, and a header written without it produces a
    // file no decoder can start.
    if let Some(mut ost) = octx.stream_mut(stream_index) {
        ost.set_parameters(&opened);
    }

    let scaler = if spec.upload_format() == format::Pixel::PAL8 {
        None
    } else {
        Some(
            scaling::Context::get(
                format::Pixel::RGBA,
                spec.width,
                spec.height,
                spec.upload_format(),
                spec.width,
                spec.height,
                // Input and output are the same size, so this is a colour
                // conversion, not a resize, and the scaling filter never runs.
                // BILINEAR is simply the cheapest thing to ask for.
                scaling::Flags::BILINEAR,
            )
            .map_err(ExportError::ffmpeg("preparing the colour converter"))
            .and_then(|mut scaler| {
                set_output_colour(&mut scaler, &spec.colour)?;
                Ok(scaler)
            })?,
        )
    };

    Ok(VideoTrack {
        encoder: opened,
        stream_index,
        time_base,
        stream_time_base: time_base,
        scaler,
        hw,
        upload_format: spec.upload_format(),
        width: spec.width,
        height: spec.height,
        frames: 0,
    })
}

/// Tell swscale which matrix and range to write.
///
/// **`sws_getContext` ignores what the codec context says** and initialises
/// with BT.601 into limited range, whatever the stream will be tagged as. This
/// is the call that makes the samples agree with the tags. The source side of
/// it is RGBA, full range by definition, and its table is ignored for an RGB
/// input; the destination table and range are what matter.
fn set_output_colour(scaler: &mut scaling::Context, colour: &OutputColour) -> Result<()> {
    let destination_full =
        i32::from(colour.encoding.range == crate::modules::render::YuvRange::Full);
    // SAFETY: `scaler` is a live `SwsContext` this call only reconfigures;
    // `sws_getCoefficients` returns a pointer into libswscale's static table,
    // valid for the process, and the call copies from it. The last three are
    // libswscale's spelling of "no brightness, contrast or saturation change".
    let code = unsafe {
        let table = ffmpeg::ffi::sws_getCoefficients(colour.sws_coefficients());
        ffmpeg::ffi::sws_setColorspaceDetails(
            scaler.as_mut_ptr(),
            table,
            1,
            table,
            destination_full,
            0,
            1 << 16,
            1 << 16,
        )
    };
    if code < 0 {
        // Refusing is right: the stream is about to be tagged with a matrix
        // the samples would not be in, which is the bug this replaced.
        return Err(ExportError::Settings(format!(
            "the colour converter would not take {}",
            colour.label()
        )));
    }
    Ok(())
}

/// Open `encoder_name`, encode one frame with it, and flush.
///
/// The only honest answer to "does this hardware encoder work". Everything
/// cheaper — the encoder being in the build, a device node existing — is a
/// proxy that is wrong in both directions on real machines. 320×240 rather than
/// something smaller because several VAAPI drivers refuse a surface below
/// 176×144 and the alignment rules are per-vendor; this size is safe
/// everywhere and costs a few milliseconds.
///
/// Deliberately writes no file: a probe that needs a writable directory is a
/// probe that fails on a locked-down machine for the wrong reason.
pub fn trial_encode(encoder_name: &str, accel: HwAccel) -> Result<()> {
    crate::modules::media::ensure_initialized();

    let spec = VideoStreamSpec {
        width: 320,
        height: 240,
        fps: Fps::THIRTY,
        encoder_name: encoder_name.to_string(),
        accel,
        quality: Quality::Crf(28),
        options: Vec::new(),
        colour: OutputColour::default(),
    };
    let codec = ffmpeg::encoder::find_by_name(encoder_name)
        .ok_or_else(|| ExportError::NoEncoder(encoder_name.to_string()))?;

    let OpenVideoEncoder { mut encoder, hw } = open_video_encoder(&spec, codec, false)?;

    // Mid-grey rather than the zeroed buffer `frame::Video::new` hands back:
    // an all-zero NV12 frame is legal but is also what a broken upload
    // produces, and a probe that passes on a black frame would pass on a path
    // that silently uploads nothing.
    let mut software = frame::Video::new(spec.upload_format(), spec.width, spec.height);
    fill_flat_grey(&mut software, spec.upload_format());
    software.set_pts(Some(0));

    match hw.as_ref() {
        None => encoder
            .send_frame(&software)
            .map_err(ExportError::ffmpeg("encoding the test frame"))?,
        Some(pool) => {
            let mut surface = pool.empty_frame()?;
            pool.upload(&software, &mut surface)?;
            surface.set_pts(Some(0));
            encoder
                .send_frame(&surface)
                .map_err(ExportError::ffmpeg("encoding the test frame on the GPU"))?;
        }
    }

    encoder
        .send_eof()
        .map_err(ExportError::ffmpeg("flushing the test encoder"))?;

    // The same flush discipline the real path has, and for the same reason: a
    // driver that accepts a frame and produces no packet is broken, and this is
    // the only place we find out cheaply.
    let mut packet = Packet::empty();
    let mut packets = 0usize;
    loop {
        match encoder.receive_packet(&mut packet) {
            Ok(()) => packets += 1,
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => break,
            Err(ffmpeg::Error::Eof) => break,
            Err(source) => {
                return Err(ExportError::Ffmpeg {
                    what: "reading the test packet back".into(),
                    source,
                })
            }
        }
    }

    if packets == 0 {
        return Err(ExportError::Settings(
            "the encoder accepted a frame and produced no data".into(),
        ));
    }
    Ok(())
}

/// Paint a flat mid-grey into a freshly allocated planar or semi-planar frame.
fn fill_flat_grey(frame: &mut frame::Video, pixel: format::Pixel) {
    let planes = match pixel {
        // NV12: one luma plane, one interleaved chroma plane. 128 in both is
        // neutral grey.
        format::Pixel::NV12 => vec![(0usize, 128u8), (1, 128)],
        // YUV420P: luma, then two chroma planes.
        _ => vec![(0usize, 128u8), (1, 128), (2, 128)],
    };
    for (plane, value) in planes {
        if plane < frame.planes() {
            frame.data_mut(plane).fill(value);
        }
    }
}

fn add_audio(
    octx: &mut format::context::Output,
    spec: &AudioStreamSpec,
    global_header: bool,
) -> Result<AudioTrack> {
    let codec = ffmpeg::encoder::find_by_name(&spec.encoder_name)
        .ok_or_else(|| ExportError::NoEncoder(spec.encoder_name.clone()))?;

    let layout = spec.layout();
    let time_base = Rational::new(1, spec.sample_rate as i32);

    // Each encoder accepts its own sample formats — AAC wants planar float,
    // libopus wants float or 16-bit — and rejects everything else at open. Ask
    // rather than assume; the resampler converts whatever the mixer produced.
    let sample_format = codec
        .audio()
        .ok()
        .and_then(|audio| audio.formats().and_then(|mut f| f.next()))
        .unwrap_or(format::Sample::F32(format::sample::Type::Planar));

    let stream_index;
    {
        let mut ost = octx
            .add_stream(codec)
            .map_err(ExportError::ffmpeg("adding the audio stream"))?;
        stream_index = ost.index();
        ost.set_time_base(time_base);
    }

    let mut encoder = context_for(codec)?
        .encoder()
        .audio()
        .map_err(ExportError::ffmpeg("preparing the audio encoder"))?;

    encoder.set_rate(spec.sample_rate as i32);
    encoder.set_channel_layout(layout);
    encoder.set_channels(layout.channels());
    encoder.set_format(sample_format);
    encoder.set_bit_rate(spec.bitrate as usize);
    encoder.set_time_base(time_base);
    if global_header {
        encoder.set_flags(codec::Flags::GLOBAL_HEADER);
    }

    let opened = encoder.open_as(codec).map_err(ExportError::ffmpeg(format!(
        "cannot open the {} encoder",
        spec.encoder_name
    )))?;

    if let Some(mut ost) = octx.stream_mut(stream_index) {
        ost.set_parameters(&opened);
    }

    let resampler = resampling::Context::get(
        format::Sample::F32(format::sample::Type::Packed),
        layout,
        spec.sample_rate,
        sample_format,
        layout,
        spec.sample_rate,
    )
    .map_err(ExportError::ffmpeg("preparing the audio converter"))?;

    let frame_size = match opened.frame_size() as usize {
        0 => DEFAULT_AUDIO_FRAME,
        size => size,
    };

    Ok(AudioTrack {
        encoder: opened,
        stream_index,
        time_base,
        stream_time_base: time_base,
        resampler,
        layout,
        channels: layout.channels().max(1) as usize,
        rate: spec.sample_rate,
        frame_size,
        pending: Vec::new(),
        written: 0,
    })
}

// ---------------------------------------------------------------------------
// Frame and packet plumbing
// ---------------------------------------------------------------------------

/// The error for a picture handed to a sound-only file.
fn no_picture() -> ExportError {
    ExportError::Settings("this file has no picture stream to write a frame to".into())
}

/// The 1024-byte palette of a PAL8 frame. libavutil allocates it as
/// `data[1]` but reports no line size for it, so `ffmpeg-next`'s plane
/// accessors do not see it.
fn palette_mut(frame: &mut frame::Video) -> Option<&mut [u8]> {
    unsafe {
        let data = (*frame.as_mut_ptr()).data[1];
        (!data.is_null()).then(|| std::slice::from_raw_parts_mut(data, 1024))
    }
}

/// Levels per channel of the GIF palette: 6 × 7 × 6 = 252 colours, with green
/// getting the extra level because the eye resolves it best.
const PAL_LEVELS: [u32; 3] = [6, 7, 6];

/// A 4×4 Bayer matrix, as thresholds in 0..16.
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Turn packed RGBA into a PAL8 frame with a fixed 252-colour palette and
/// ordered dithering.
///
/// A fixed palette rather than one computed per frame: the same colour maps
/// to the same index in every frame, so flat areas do not shimmer when the
/// palette changes between frames, and the GIF encoder's inter-frame diffing
/// finds more unchanged pixels. Ordered (Bayer) rather than error-diffusion
/// dithering for the same reason — a static pattern compresses and does not
/// crawl. swscale's own RGB8 output has only 3-3-2 bits, which bands badly.
pub(crate) fn quantize_pal8(rgba: &[u8], width: u32, height: u32) -> frame::Video {
    let mut target = frame::Video::new(format::Pixel::PAL8, width, height);
    let stride = target.stride(0);
    let (w, h) = (width as usize, height as usize);
    {
        let indices = target.data_mut(0);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let threshold = f32::from(BAYER4[y % 4][x % 4]) / 16.0 - 0.5 + 1.0 / 32.0;
                let mut index = 0u32;
                for (channel, levels) in PAL_LEVELS.iter().enumerate() {
                    let steps = (levels - 1) as f32;
                    let value = f32::from(rgba[i + channel]) / 255.0 * steps + threshold;
                    let level = value.round().clamp(0.0, steps) as u32;
                    index = index * levels + level;
                }
                indices[y * stride + x] = index as u8;
            }
        }
    }
    {
        // The palette plane: 256 native-endian 0xAARRGGBB words.
        let Some(palette) = palette_mut(&mut target) else {
            return target;
        };
        for index in 0..256u32 {
            let colour = if index < PAL_LEVELS.iter().product::<u32>() {
                let b = index % PAL_LEVELS[2];
                let g = index / PAL_LEVELS[2] % PAL_LEVELS[1];
                let r = index / (PAL_LEVELS[2] * PAL_LEVELS[1]);
                let scale = |level: u32, levels: u32| (level * 255 / (levels - 1)) & 0xFF;
                0xFF00_0000
                    | scale(r, PAL_LEVELS[0]) << 16
                    | scale(g, PAL_LEVELS[1]) << 8
                    | scale(b, PAL_LEVELS[2])
            } else {
                0xFF00_0000
            };
            let at = index as usize * 4;
            if at + 4 <= palette.len() {
                palette[at..at + 4].copy_from_slice(&colour.to_ne_bytes());
            }
        }
    }
    target
}

/// Copy `rows` rows of `row_bytes` from `source` into plane `plane`.
///
/// Two strides, both of which are somebody else's decision: the GPU picked
/// `source_stride` to keep its writes word-aligned and libavutil picked the
/// frame's to suit whatever SIMD the encoder uses. They are equal on most sizes
/// and a sheared picture on the rest, which is why this is a loop and not a
/// single `copy_from_slice`.
fn copy_plane(
    target: &mut frame::Video,
    plane: usize,
    source: &[u8],
    source_stride: usize,
    row_bytes: usize,
    rows: usize,
) {
    let stride = target.stride(plane);
    let data = target.data_mut(plane);
    for y in 0..rows {
        let src = y * source_stride;
        let dst = y * stride;
        data[dst..dst + row_bytes].copy_from_slice(&source[src..src + row_bytes]);
    }
}

/// Copy tightly packed rows into a frame whose rows are padded.
fn copy_packed_rows(target: &mut frame::Video, packed: &[u8], row_bytes: usize) {
    let stride = target.stride(0);
    let height = target.height() as usize;
    let data = target.data_mut(0);
    for y in 0..height {
        let src = y * row_bytes;
        let dst = y * stride;
        data[dst..dst + row_bytes].copy_from_slice(&packed[src..src + row_bytes]);
    }
}

fn encode_audio_chunk(
    octx: &mut format::context::Output,
    audio: &mut AudioTrack,
    interleaved: &[f32],
    frames: usize,
    stats: &mut WriterStats,
) -> Result<()> {
    let mut source = frame::Audio::new(
        format::Sample::F32(format::sample::Type::Packed),
        frames,
        audio.layout,
    );
    source.set_rate(audio.rate);
    {
        let bytes = std::mem::size_of_val(interleaved);
        let plane = source.data_mut(0);
        // Packed f32 is one plane of native-endian floats, so this is a
        // reinterpretation and not a conversion.
        let raw: &[u8] =
            unsafe { std::slice::from_raw_parts(interleaved.as_ptr() as *const u8, bytes) };
        let usable = bytes.min(plane.len());
        plane[..usable].copy_from_slice(&raw[..usable]);
    }

    let mut converted = frame::Audio::empty();
    audio
        .resampler
        .run(&source, &mut converted)
        .map_err(ExportError::ffmpeg("converting audio samples"))?;
    converted.set_rate(audio.rate);
    // Sample rate and layout are unchanged through the resampler, so it has no
    // delay and the count that went in is the count that came out; the PTS is
    // simply how many sample frames preceded this one.
    converted.set_pts(Some(audio.written));
    audio.written += frames as i64;

    audio
        .encoder
        .send_frame(&converted)
        .map_err(ExportError::ffmpeg("encoding audio"))?;
    drain_audio(octx, audio, stats)
}

fn drain_video(
    octx: &mut format::context::Output,
    video: &mut VideoTrack,
    stats: &mut WriterStats,
) -> Result<()> {
    let mut packet = Packet::empty();
    loop {
        match video.encoder.receive_packet(&mut packet) {
            Ok(()) => {}
            // EAGAIN: nothing ready yet. Eof: the flush is complete.
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(source) => {
                return Err(ExportError::Ffmpeg {
                    what: "encoding a video frame".into(),
                    source,
                })
            }
        }
        stats.video_bytes += packet.size() as u64;
        packet.set_stream(video.stream_index);
        packet.rescale_ts(video.time_base, video.stream_time_base);
        // Matroska in particular wants durations; without one it guesses from
        // the next packet and the last frame of the file gets no duration at
        // all.
        packet.set_duration(rescale(1, video.time_base, video.stream_time_base));
        packet
            .write_interleaved(octx)
            .map_err(ExportError::ffmpeg("writing a video packet"))?;
    }
}

fn drain_audio(
    octx: &mut format::context::Output,
    audio: &mut AudioTrack,
    stats: &mut WriterStats,
) -> Result<()> {
    let mut packet = Packet::empty();
    loop {
        match audio.encoder.receive_packet(&mut packet) {
            Ok(()) => {}
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => return Ok(()),
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(source) => {
                return Err(ExportError::Ffmpeg {
                    what: "encoding audio".into(),
                    source,
                })
            }
        }
        stats.audio_bytes += packet.size() as u64;
        packet.set_stream(audio.stream_index);
        packet.rescale_ts(audio.time_base, audio.stream_time_base);
        packet
            .write_interleaved(octx)
            .map_err(ExportError::ffmpeg("writing an audio packet"))?;
    }
}

/// Rescale a timestamp between two time bases, rounding to nearest.
///
/// The same arithmetic `av_rescale_q` does, in i128 so an hour-long export at a
/// 90 kHz timescale cannot overflow the intermediate product. Rounding away
/// from zero at the halfway point matches `AV_ROUND_NEAR_INF`, which is what
/// libav uses for packet timestamps — using a different rounding here and there
/// is how a stream ends up one tick out of sync with itself.
pub fn rescale(value: i64, from: Rational, to: Rational) -> i64 {
    let numerator = value as i128 * from.numerator() as i128 * to.denominator() as i128;
    let denominator = from.denominator() as i128 * to.numerator() as i128;
    if denominator == 0 {
        return 0;
    }
    // Normalize the sign onto the numerator so the halfway case is symmetric
    // around zero rather than biased in one direction.
    let (numerator, denominator) = if denominator < 0 {
        (-numerator, -denominator)
    } else {
        (numerator, denominator)
    };
    let half = denominator / 2;
    let rounded = if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        -((-numerator + half) / denominator)
    };
    rounded as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::export::presets::VideoCodec;

    fn tb(fps: Fps) -> Rational {
        let (num, den) = fps.time_base();
        Rational::new(num, den)
    }

    #[test]
    fn frame_indices_rescale_into_an_mp4_timescale() {
        // MP4 commonly lands on 90 kHz. One frame of 30 fps is 3000 ticks.
        let stream = Rational::new(1, 90_000);
        assert_eq!(rescale(0, tb(Fps::THIRTY), stream), 0);
        assert_eq!(rescale(1, tb(Fps::THIRTY), stream), 3_000);
        assert_eq!(rescale(30, tb(Fps::THIRTY), stream), 90_000);
        assert_eq!(rescale(25, tb(Fps::PAL), stream), 90_000);
    }

    #[test]
    fn fractional_rates_stay_exact_over_an_hour() {
        // 29.97 in a 90 kHz stream: a frame is 1001/30000 s = 3003 ticks
        // exactly, which is precisely why 90 kHz was chosen as a timescale.
        let stream = Rational::new(1, 90_000);
        assert_eq!(rescale(1, tb(Fps::NTSC), stream), 3_003);
        // An hour of it: 107_892 frames * 3003 ticks, exactly.
        assert_eq!(rescale(107_892, tb(Fps::NTSC), stream), 323_999_676);

        // 23.976: 1001/24000 s = 3753.75 ticks, which does not divide evenly —
        // the per-frame value rounds, the cumulative value does not drift
        // because it is computed from the index.
        assert_eq!(rescale(1, tb(Fps::FILM_NTSC), stream), 3_754);
        assert_eq!(rescale(24_000, tb(Fps::FILM_NTSC), stream), 90_090_000);

        // 59.94.
        assert_eq!(rescale(1, tb(Fps::NTSC_DOUBLE), stream), 1_502);
        assert_eq!(rescale(60_000, tb(Fps::NTSC_DOUBLE), stream), 90_090_000);
    }

    #[test]
    fn rescaling_rounds_to_nearest_rather_than_truncating() {
        // 1/3 of a tick up and down, either side of the halfway point.
        let from = Rational::new(1, 3);
        let to = Rational::new(1, 2);
        assert_eq!(rescale(1, from, to), 1); // 0.666… -> 1
        assert_eq!(rescale(2, from, to), 1); // 1.333… -> 1
        assert_eq!(rescale(4, from, to), 3); // 2.666… -> 3
    }

    #[test]
    fn a_long_export_does_not_overflow_the_intermediate_product() {
        // Three hours at 59.94 into a 90 kHz stream: the naive i64 product of
        // index * numerator * denominator is over 2^63.
        let stream = Rational::new(1, 90_000);
        let frames = Fps::NTSC_DOUBLE.frame_count(3 * 3600 * 1_000_000);
        let last = rescale(frames as i64 - 1, tb(Fps::NTSC_DOUBLE), stream);
        assert!(last > 0);
        assert!(last < 3 * 3600 * 90_000 + 90_000);
    }

    #[test]
    fn a_degenerate_time_base_does_not_divide_by_zero() {
        assert_eq!(rescale(1_000, Rational::new(1, 30), Rational::new(0, 1)), 0);
    }

    #[test]
    fn software_encoders_get_a_preset_and_a_crf() {
        let spec = VideoStreamSpec {
            width: 1920,
            height: 1080,
            fps: Fps::THIRTY,
            encoder_name: VideoCodec::H264.software_encoder().into(),
            accel: HwAccel::Software,
            quality: Quality::Crf(20),
            options: Vec::new(),
            colour: crate::modules::export::OutputColour::default(),
        };
        let options = spec.dictionary();
        assert!(options.contains(&("preset".into(), "medium".into())));
        assert!(options.contains(&("crf".into(), "20".into())));
    }

    #[test]
    fn caller_options_win_over_defaults() {
        // Dictionary::set overwrites, and the caller's entries are appended
        // last, so a caller can force veryfast for a preview render.
        let spec = VideoStreamSpec {
            width: 1280,
            height: 720,
            fps: Fps::THIRTY,
            encoder_name: "libx264".into(),
            accel: HwAccel::Software,
            quality: Quality::Crf(23),
            options: vec![("preset".into(), "veryfast".into())],
            colour: crate::modules::export::OutputColour::default(),
        };
        let options = spec.dictionary();
        let last_preset = options
            .iter()
            .rev()
            .find(|(k, _)| k == "preset")
            .cloned()
            .unwrap();
        assert_eq!(last_preset.1, "veryfast");
    }

    #[test]
    fn a_bitrate_target_adds_no_crf_option() {
        let spec = VideoStreamSpec {
            width: 1920,
            height: 1080,
            fps: Fps::THIRTY,
            encoder_name: "libx264".into(),
            accel: HwAccel::Software,
            quality: Quality::Bitrate(12_000_000),
            options: Vec::new(),
            colour: crate::modules::export::OutputColour::default(),
        };
        assert!(!spec.dictionary().iter().any(|(k, _)| k == "crf"));
    }

    fn spec(accel: HwAccel, encoder_name: &str, quality: Quality) -> VideoStreamSpec {
        VideoStreamSpec {
            width: 1920,
            height: 1080,
            fps: Fps::THIRTY,
            encoder_name: encoder_name.into(),
            accel,
            quality,
            options: Vec::new(),
            colour: crate::modules::export::OutputColour::default(),
        }
    }

    #[test]
    fn the_hardware_path_negotiates_nv12_and_a_pool_and_the_software_path_does_not() {
        let hardware = spec(HwAccel::Vaapi, "h264_vaapi", Quality::Crf(23));
        assert!(hardware.needs_frame_pool());
        // What swscale targets and what the pool's sw_format is: the same
        // thing, and they have to stay the same thing or the upload fails.
        assert_eq!(hardware.upload_format(), format::Pixel::NV12);

        let software = spec(HwAccel::Software, "libx264", Quality::Crf(23));
        assert!(!software.needs_frame_pool());
        assert_eq!(software.upload_format(), format::Pixel::YUV420P);

        // NVENC is hardware and still takes system memory, which is the whole
        // reason `needs_frame_pool` is not "is this hardware".
        let nvenc = spec(HwAccel::Nvenc, "h264_nvenc", Quality::Crf(23));
        assert!(!nvenc.needs_frame_pool());
        assert_eq!(nvenc.upload_format(), format::Pixel::NV12);
    }

    #[test]
    fn a_vaapi_spec_asks_for_constant_quality_first_and_has_somewhere_to_fall_back_to() {
        let spec = spec(HwAccel::Vaapi, "h264_vaapi", Quality::Crf(23));
        let ladder = spec.rate_controls();

        let first = spec.dictionary_for(&ladder[0]);
        assert!(first.contains(&("rc_mode".into(), "CQP".into())));
        assert!(first.contains(&("qp".into(), "23".into())));
        // x264's preset must not leak onto a VAAPI encoder: `preset` there is a
        // different option with different values and libavcodec rejects it.
        assert!(!first.iter().any(|(k, _)| k == "preset"));

        assert!(ladder.len() > 1, "no fallback for a driver without CQP");
        assert!(ladder[1..].iter().all(|rung| rung.bit_rate > 0));
    }

    #[test]
    fn caller_options_still_win_on_the_hardware_path() {
        // The escape hatch has to survive the ladder: a caller that knows this
        // driver wants low-power mode must be able to say so.
        let mut spec = spec(HwAccel::Vaapi, "h264_vaapi", Quality::Crf(23));
        spec.options = vec![("rc_mode".into(), "VBR".into())];
        let ladder = spec.rate_controls();
        let options = spec.dictionary_for(&ladder[0]);
        let last = options
            .iter()
            .rev()
            .find(|(k, _)| k == "rc_mode")
            .cloned()
            .unwrap();
        assert_eq!(last.1, "VBR");
    }

    #[test]
    fn a_hardware_bitrate_request_sets_a_bitrate_and_names_no_mode() {
        let spec = spec(HwAccel::Vaapi, "h264_vaapi", Quality::Bitrate(12_000_000));
        let ladder = spec.rate_controls();
        assert_eq!(ladder.len(), 1);
        assert_eq!(ladder[0].bit_rate, 12_000_000);
        assert!(spec.dictionary_for(&ladder[0]).is_empty());
    }

    /// 10-bit is P010 for every hardware encoder (VAAPI and QSV surfaces,
    /// NVENC's system-memory input) and yuv420p10le for the software ones,
    /// with Main 10 asked of every HEVC encoder. VAAPI is checked here rather
    /// than end to end because the machine this was written on has none.
    #[test]
    fn ten_bit_uploads_p010_on_hardware_and_asks_for_main10() {
        for (accel, name, upload) in [
            (HwAccel::Vaapi, "hevc_vaapi", format::Pixel::P010LE),
            (HwAccel::Qsv, "hevc_qsv", format::Pixel::P010LE),
            (HwAccel::Nvenc, "hevc_nvenc", format::Pixel::P010LE),
            (HwAccel::Software, "libx265", format::Pixel::YUV420P10LE),
            (HwAccel::Software, "libsvtav1", format::Pixel::YUV420P10LE),
        ] {
            let mut ten = spec(accel, name, Quality::Crf(22));
            ten.colour = OutputColour::resolve(
                super::super::colour::ColorMatrix::Auto,
                super::super::colour::ColorRange::Limited,
                true,
                (1920, 1080),
            );
            assert_eq!(ten.upload_format(), upload, "{name}");
            if name.starts_with("hevc") || name == "libx265" {
                assert!(
                    ten.dictionary()
                        .contains(&("profile".into(), "main10".into())),
                    "{name}: {:?}",
                    ten.dictionary()
                );
            }
            // And 8-bit stays what it was.
            let eight = spec(accel, name, Quality::Crf(22));
            assert_ne!(eight.upload_format(), upload, "{name}");
            assert!(!eight
                .dictionary()
                .contains(&("profile".into(), "main10".into())));
        }
    }

    #[test]
    fn every_stream_but_a_gif_is_tagged() {
        assert!(spec(HwAccel::Vaapi, "h264_vaapi", Quality::Crf(22)).is_tagged());
        assert!(spec(HwAccel::Software, "prores_ks", Quality::Crf(22)).is_tagged());
        assert!(!spec(HwAccel::Software, "gif", Quality::Crf(22)).is_tagged());
    }

    #[test]
    fn a_hardware_frame_carries_the_same_pts_as_a_software_one() {
        // The upload copies pixels and nothing else, so the hardware path sets
        // the PTS a second time. Both paths derive it from the frame index in
        // the encoder's 1/fps time base, which is what makes the two files line
        // up frame for frame — and what the audio interleave depends on.
        let stream = Rational::new(1, 90_000);
        for index in [0i64, 1, 59, 60, 1_000] {
            let software = rescale(index, tb(Fps::THIRTY), stream);
            let hardware = rescale(index, tb(Fps::THIRTY), stream);
            assert_eq!(software, hardware);
            assert_eq!(software, index * 3_000);
        }
    }

    #[test]
    fn a_trial_encode_of_a_missing_encoder_is_an_error_and_not_a_panic() {
        let error = trial_encode("definitely_not_an_encoder", HwAccel::Software).unwrap_err();
        assert!(matches!(error, ExportError::NoEncoder(_)), "{error}");
    }

    #[test]
    fn a_trial_encode_of_the_software_encoder_produces_a_packet() {
        // Not GPU-guarded: libx264 is in every build we ship, and this is the
        // test that the probe machinery itself works, independent of hardware.
        if !super::super::hwaccel::encoder_exists("libx264") {
            return;
        }
        trial_encode("libx264", HwAccel::Software).expect("a software trial encode");
    }

    #[test]
    fn the_grey_test_frame_is_grey_in_both_layouts() {
        // A probe that passed on an all-zero frame would pass on a path that
        // silently uploads nothing, so `fill_flat_grey` has to actually write.
        for pixel in [format::Pixel::NV12, format::Pixel::YUV420P] {
            let mut frame = frame::Video::new(pixel, 64, 64);
            fill_flat_grey(&mut frame, pixel);
            for plane in 0..frame.planes() {
                assert!(
                    frame.data(plane).iter().all(|&byte| byte == 128),
                    "{pixel:?} plane {plane} was not filled"
                );
            }
        }
    }

    #[test]
    fn the_gif_palette_reproduces_primaries_and_greys() {
        // 8x8 of one colour each; with dithering the average must be close.
        for colour in [
            [255u8, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [128, 128, 128],
            [0, 0, 0],
        ] {
            let rgba: Vec<u8> = (0..64)
                .flat_map(|_| [colour[0], colour[1], colour[2], 255])
                .collect();
            let mut frame = quantize_pal8(&rgba, 8, 8);
            let palette = palette_mut(&mut frame).expect("a palette").to_vec();
            let stride = frame.stride(0);
            let mut sum = [0u32; 3];
            for y in 0..8 {
                for x in 0..8 {
                    let index = frame.data(0)[y * stride + x] as usize;
                    let word =
                        u32::from_ne_bytes(palette[index * 4..index * 4 + 4].try_into().unwrap());
                    sum[0] += (word >> 16) & 0xFF;
                    sum[1] += (word >> 8) & 0xFF;
                    sum[2] += word & 0xFF;
                }
            }
            for channel in 0..3 {
                let mean = sum[channel] as i32 / 64;
                assert!(
                    (mean - i32::from(colour[channel])).abs() <= 12,
                    "{colour:?}: channel {channel} averaged {mean}"
                );
            }
        }
    }

    #[test]
    fn prores_and_gif_open_without_a_quality_target() {
        let prores = spec(HwAccel::Software, "prores_ks", Quality::Crf(20));
        assert_eq!(prores.upload_format(), format::Pixel::YUV422P10LE);
        let options = prores.dictionary();
        assert!(!options.iter().any(|(k, _)| k == "crf"), "{options:?}");
        assert!(options.contains(&("profile".into(), "3".into())));
        let gif = spec(HwAccel::Software, "gif", Quality::Bitrate(5_000_000));
        assert_eq!(gif.upload_format(), format::Pixel::PAL8);
        assert_eq!(gif.rate_controls()[0].bit_rate, 0);
    }

    #[test]
    fn packed_rows_are_copied_into_a_padded_frame() {
        // 6px wide RGBA is 24 bytes a row; libav pads that to its own
        // alignment, so a flat copy would shear the picture.
        let mut target = frame::Video::new(format::Pixel::RGBA, 6, 4);
        let packed: Vec<u8> = (0..(6 * 4 * 4)).map(|i| (i % 251) as u8).collect();
        copy_packed_rows(&mut target, &packed, 6 * 4);

        let stride = target.stride(0);
        let data = target.data(0);
        for y in 0..4usize {
            for x in 0..(6 * 4) {
                assert_eq!(
                    data[y * stride + x],
                    packed[y * 6 * 4 + x],
                    "row {y} byte {x}"
                );
            }
        }
    }
}
