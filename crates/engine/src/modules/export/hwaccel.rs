//! Which hardware encoders this machine actually has.
//!
//! Three different questions get confused here, so this file keeps them apart:
//!
//! 1. **Is the encoder compiled into FFmpeg?** `avcodec_find_encoder_by_name`
//!    answers that, cheaply and without touching a device.
//! 2. **Is there a device it can drive?** A build with `h264_vaapi` in it on a
//!    box with no `/dev/dri` is the normal case on a headless server, and on a
//!    laptop whose GPU is claimed by another process the device node exists and
//!    initialisation still fails.
//! 3. **Does it actually encode?** This is the one that cannot be answered by
//!    looking. `/dev/dri/renderD128` exists on a machine whose driver supports
//!    decode and not encode, and `av1_vaapi` is in every modern FFmpeg build
//!    while the Raptor Lake iGPU this was developed on reports `VAProfileAV1Profile0`
//!    for `VAEntrypointVLD` only — decode, no encode. Both of those look
//!    available and are not.
//!
//! Questions 1 and 2 are answered by *looking*. Question 3 is answered by
//! opening the device and encoding one 320×240 frame, because there is no
//! honest cheaper answer, and reporting an encoder that fails at export time is
//! worse than a two-hundred-millisecond probe. The result is cached for the
//! process, so the cost is paid once.
//!
//! A missing device node is normal, not exceptional, so every failure path
//! returns "not available" rather than an error, and the whole probe
//! additionally runs inside `catch_unwind` — a panic while enumerating optional
//! hardware must not take the app with it.
//!
//! ## Why software is still the default
//!
//! Hardware encoders are two to ten times faster and meaningfully worse at the
//! same bitrate, and how much worse depends on the vendor, the driver version
//! and the generation of the chip. x264 at a given CRF looks the same on every
//! machine; `h264_vaapi` does not. A user who picks hardware has decided that
//! the trade is worth it. A user who picked nothing gets the predictable
//! result, even though it is slower — a surprising default is worse than a slow
//! one, because the surprise is only discovered after the upload.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::presets::{Fps, Quality, VideoCodec};

/// A hardware encoding API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HwAccel {
    /// x264 and friends. Always available, always the default.
    Software,
    /// Mesa/Intel/AMD on Linux, through libva.
    Vaapi,
    /// Intel Quick Sync, through oneVPL/MediaSDK.
    Qsv,
    /// NVIDIA, through NVENC. Linux and Windows.
    Nvenc,
    /// Apple. Stub — detection returns nothing on other platforms.
    VideoToolbox,
    /// AMD on Windows, through AMF. Stub.
    Amf,
    /// Windows Media Foundation, the vendor-agnostic fallback there. Stub.
    MediaFoundation,
}

impl HwAccel {
    pub fn label(self) -> &'static str {
        match self {
            HwAccel::Software => "Software",
            HwAccel::Vaapi => "VAAPI",
            HwAccel::Qsv => "Intel Quick Sync",
            HwAccel::Nvenc => "NVIDIA NVENC",
            HwAccel::VideoToolbox => "VideoToolbox",
            HwAccel::Amf => "AMD AMF",
            HwAccel::MediaFoundation => "Media Foundation",
        }
    }

    /// The FFmpeg encoder name for a codec on this API, if the pair exists.
    pub fn encoder_name(self, codec: VideoCodec) -> Option<&'static str> {
        match (self, codec) {
            (HwAccel::Software, _) => Some(codec.software_encoder()),
            (HwAccel::Vaapi, VideoCodec::H264) => Some("h264_vaapi"),
            (HwAccel::Vaapi, VideoCodec::H265) => Some("hevc_vaapi"),
            (HwAccel::Vaapi, VideoCodec::Vp9) => Some("vp9_vaapi"),
            (HwAccel::Vaapi, VideoCodec::Av1) => Some("av1_vaapi"),
            (HwAccel::Qsv, VideoCodec::H264) => Some("h264_qsv"),
            (HwAccel::Qsv, VideoCodec::H265) => Some("hevc_qsv"),
            (HwAccel::Qsv, VideoCodec::Av1) => Some("av1_qsv"),
            (HwAccel::Nvenc, VideoCodec::H264) => Some("h264_nvenc"),
            (HwAccel::Nvenc, VideoCodec::H265) => Some("hevc_nvenc"),
            (HwAccel::Nvenc, VideoCodec::Av1) => Some("av1_nvenc"),
            (HwAccel::VideoToolbox, VideoCodec::H264) => Some("h264_videotoolbox"),
            (HwAccel::VideoToolbox, VideoCodec::H265) => Some("hevc_videotoolbox"),
            (HwAccel::Amf, VideoCodec::H264) => Some("h264_amf"),
            (HwAccel::Amf, VideoCodec::H265) => Some("hevc_amf"),
            (HwAccel::MediaFoundation, VideoCodec::H264) => Some("h264_mf"),
            (HwAccel::MediaFoundation, VideoCodec::H265) => Some("hevc_mf"),
            _ => None,
        }
    }

    /// Whether frames can be handed to this encoder in ordinary system memory.
    ///
    /// NVENC, VideoToolbox, AMF and Media Foundation upload a software frame
    /// themselves. VAAPI and QSV want a frame that already lives in a hardware
    /// frame pool — an `AVHWFramesContext` — and handing them a YUV420P buffer
    /// fails at `avcodec_open2`. [`super::hwframes`] builds that pool, so this
    /// is no longer a refusal: it is the flag that tells [`super::encoder`]
    /// whether to allocate one.
    pub fn accepts_software_frames(self) -> bool {
        !matches!(self, HwAccel::Vaapi | HwAccel::Qsv)
    }

    /// The libav hardware device and surface format this API needs, when it
    /// needs a frame pool at all.
    pub fn frame_pool(
        self,
    ) -> Option<(ffmpeg_next::ffi::AVHWDeviceType, ffmpeg_next::format::Pixel)> {
        use ffmpeg_next::ffi::AVHWDeviceType;
        use ffmpeg_next::format::Pixel;
        match self {
            HwAccel::Vaapi => Some((AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, Pixel::VAAPI)),
            HwAccel::Qsv => Some((AVHWDeviceType::AV_HWDEVICE_TYPE_QSV, Pixel::QSV)),
            _ => None,
        }
    }

    /// The software pixel format frames are converted to before they reach this
    /// encoder.
    ///
    /// VAAPI and QSV surfaces are NV12 — that is what the fixed-function
    /// hardware reads, and asking for a planar 4:2:0 pool gets `EINVAL` from
    /// `av_hwframe_ctx_init` on the Intel driver. Everything else takes the
    /// planar format every software encoder wants.
    pub fn upload_format(self) -> ffmpeg_next::format::Pixel {
        use ffmpeg_next::format::Pixel;
        match self {
            // NVENC takes system-memory NV12 as its native input. Asking for it
            // is what lets the export convert on the GPU and read back 1.5
            // bytes a pixel instead of 4 plus a swscale pass on the CPU.
            HwAccel::Vaapi | HwAccel::Qsv | HwAccel::Nvenc => Pixel::NV12,
            _ => Pixel::YUV420P,
        }
    }

    /// Private encoder options that express a quality target on this API.
    ///
    /// CRF is an x264/x265 concept. Every hardware encoder has its own spelling
    /// of "constant quality", and several ignore `bit_rate` entirely unless
    /// told which rate control mode to use, which is how a hardware export ends
    /// up at 2 Mbit/s when the user asked for 12.
    pub fn quality_options(self, quality: Quality) -> Vec<(&'static str, String)> {
        match (self, quality) {
            (HwAccel::Software, Quality::Crf(crf)) => {
                vec![("crf", crf.to_string())]
            }
            (HwAccel::Nvenc, Quality::Crf(crf)) => vec![
                ("rc", "vbr".into()),
                ("cq", crf.to_string()),
                // NVENC in constant-quality mode still honours bit_rate as a
                // cap unless it is explicitly zeroed.
                ("b", "0".into()),
            ],
            (HwAccel::Vaapi, Quality::Crf(crf)) => {
                vec![("rc_mode", "CQP".into()), ("qp", crf.to_string())]
            }
            (HwAccel::Qsv, Quality::Crf(crf)) => vec![
                ("look_ahead", "0".into()),
                ("global_quality", crf.to_string()),
            ],
            (HwAccel::VideoToolbox, Quality::Crf(crf)) => {
                // VideoToolbox takes a 0..100 quality where higher is better,
                // the opposite direction and scale to CRF.
                let q = 100u32.saturating_sub(u32::from(crf) * 2);
                vec![("q", q.to_string())]
            }
            // AMF and Media Foundation are stubs; bitrate mode is the only
            // thing that behaves the same everywhere, so a CRF request there
            // falls through to the bitrate the caller computed.
            (_, Quality::Crf(_)) => Vec::new(),
            (_, Quality::Bitrate(_)) => Vec::new(),
        }
    }

    /// Every rate-control configuration worth trying, best first.
    ///
    /// This exists because a hardware encoder's rate-control modes are a
    /// property of the *driver*, not of FFmpeg, and there is no way to ask
    /// which are supported that does not amount to trying one. `h264_vaapi`
    /// with `rc_mode=CQP` fails at `avcodec_open2` with "Rate control mode CQP
    /// is not supported" on drivers that lack it — including several AMD ones
    /// — and succeeds on the Intel iHD driver we develop against. So the
    /// encoder walks this list and opens with the first rung that works,
    /// instead of assuming.
    ///
    /// The rungs after the first all carry a bitrate, because that is the point
    /// of the fallback: an encoder that cannot do constant quality has to be
    /// told a number, and the number the user gave us was a CRF.
    pub fn rate_control_ladder(self, quality: Quality, fallback_bitrate: u64) -> Vec<RateControl> {
        let primary = RateControl {
            label: match quality {
                Quality::Crf(_) => "constant quality",
                Quality::Bitrate(_) => "bitrate",
            },
            bit_rate: match quality {
                Quality::Crf(_) => 0,
                Quality::Bitrate(bits) => bits,
            },
            options: self
                .quality_options(quality)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        };

        // A bitrate request has nothing to fall back *to*: `bit_rate` on the
        // codec context is understood by every encoder that exists.
        let Quality::Crf(_) = quality else {
            return vec![primary];
        };

        let mut ladder = vec![primary];
        match self {
            HwAccel::Software => {}
            HwAccel::Vaapi => {
                ladder.push(RateControl {
                    label: "variable bitrate",
                    bit_rate: fallback_bitrate,
                    options: vec![("rc_mode".into(), "VBR".into())],
                });
                ladder.push(RateControl {
                    label: "constant bitrate",
                    bit_rate: fallback_bitrate,
                    options: vec![("rc_mode".into(), "CBR".into())],
                });
                ladder.push(RateControl {
                    label: "the driver's own default",
                    bit_rate: fallback_bitrate,
                    options: Vec::new(),
                });
            }
            _ => {
                ladder.push(RateControl {
                    label: "bitrate",
                    bit_rate: fallback_bitrate,
                    options: Vec::new(),
                });
            }
        }
        ladder
    }
}

/// One attempt at configuring rate control on an encoder.
///
/// Split into "what goes on the codec context" and "what goes in the private
/// option dictionary" because the two are applied at different moments —
/// `bit_rate` before `avcodec_open2`, the dictionary *during* it — and because
/// mixing them is how a hardware export ends up at 2 Mbit/s when the user asked
/// for 12.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateControl {
    /// For the log line when a rung is skipped, in user-facing prose.
    pub label: &'static str,
    /// `AVCodecContext::bit_rate`. Zero means "do not set one", which is what
    /// every constant-quality mode wants — a bitrate left lying around is
    /// treated as a cap by NVENC and as a target by VAAPI.
    pub bit_rate: u64,
    pub options: Vec<(String, String)>,
}

/// A bits-per-second target for an encoder that turned out not to do constant
/// quality.
///
/// Derived from the CRF rather than from the resolution alone, because the
/// user's CRF is the only statement of intent we have. The exponential is the
/// usual x264 rule of thumb — six points of CRF is a factor of two in bitrate —
/// anchored so that CRF 23 at 1080p30 lands near 9 Mbit/s, which is roughly
/// what x264 produces there. It is an estimate and it is only ever used on the
/// path where the alternative is no bitrate at all.
pub fn fallback_bitrate(width: u32, height: u32, fps: Fps, quality: Quality) -> u64 {
    let crf = match quality {
        Quality::Bitrate(bits) => return bits,
        Quality::Crf(crf) => f64::from(crf),
    };
    let bits_per_pixel = (0.15 * 2f64.powf((23.0 - crf) / 6.0)).clamp(0.01, 1.0);
    let pixels_per_second = f64::from(width) * f64::from(height) * fps.as_f64();
    // A floor and a ceiling so a degenerate canvas or a nonsense CRF cannot
    // produce a bitrate the muxer chokes on.
    ((bits_per_pixel * pixels_per_second) as u64).clamp(250_000, 200_000_000)
}

/// One encoder the user may choose in the dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HwEncoder {
    /// Stable id the frontend sends back: `"nvenc_h264"`.
    pub id: String,
    pub accel: HwAccel,
    pub codec: VideoCodec,
    /// What FFmpeg calls it.
    pub encoder_name: String,
    pub label: String,
    /// Present in this FFmpeg build *and* backed by a device we can see.
    pub available: bool,
    /// Available and something this module can actually drive today. An
    /// unusable entry is still listed, with a `note`, because "your GPU is not
    /// supported yet" is a better answer than a silently missing option.
    pub usable: bool,
    pub note: Option<String>,
}

/// Every hardware encoder worth offering on this machine.
///
/// Software is not included: it is not a choice, it is what happens when no
/// choice is made. Returns an empty list rather than failing when nothing is
/// present, which is the common case.
///
/// Cached for the process. The trial encode behind `usable` costs on the order
/// of a hundred milliseconds per encoder and the answer cannot change while the
/// app runs — a GPU is not hot-plugged mid-session.
pub fn detect() -> Vec<HwEncoder> {
    static DETECTED: OnceLock<Vec<HwEncoder>> = OnceLock::new();
    DETECTED
        .get_or_init(|| {
            // Enumerating optional vendor hardware is exactly the kind of code
            // that aborts on a broken driver. A panic here would take the
            // export dialog — and with it the app — down for a feature the user
            // did not ask for.
            std::panic::catch_unwind(probe).unwrap_or_else(|_| {
                tracing::warn!("hardware encoder detection panicked; offering software only");
                Vec::new()
            })
        })
        .clone()
}

fn probe() -> Vec<HwEncoder> {
    crate::modules::media::ensure_initialized();

    let mut found = Vec::new();
    for (accel, present) in [
        (HwAccel::Vaapi, vaapi_device_present()),
        (HwAccel::Qsv, qsv_device_present()),
        (HwAccel::Nvenc, nvidia_device_present()),
        (HwAccel::VideoToolbox, videotoolbox_present()),
        (HwAccel::Amf, windows_present()),
        (HwAccel::MediaFoundation, windows_present()),
    ] {
        if !present {
            continue;
        }
        for codec in [VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1] {
            let Some(name) = accel.encoder_name(codec) else {
                continue;
            };
            if !encoder_exists(name) {
                continue;
            }

            // The moment of truth: open the device and encode a frame. Anything
            // short of that is a guess — see the note at the top of the file
            // about `av1_vaapi` on a chip that only decodes AV1.
            let note = match super::encoder::trial_encode(name, accel) {
                Ok(()) => None,
                Err(error) => Some(unusable_note(accel, error)),
            };

            found.push(HwEncoder {
                id: format!("{}_{}", accel_slug(accel), codec_slug(codec)),
                accel,
                codec,
                encoder_name: name.to_string(),
                label: format!("{} ({})", codec.label(), accel.label()),
                available: true,
                usable: note.is_none(),
                note,
            });
        }
    }
    found
}

/// Turn a failed trial encode into something a person can act on.
///
/// The libav error alone ("Invalid argument", "Function not implemented") tells
/// the user nothing, and this string goes straight into the export dialog.
fn unusable_note(accel: HwAccel, error: super::ExportError) -> String {
    tracing::debug!(accel = accel.label(), %error, "hardware encoder failed its trial encode");
    format!(
        "{} is present but this driver could not encode a test frame with it ({error}). \
         Software encoding still works.",
        accel.label()
    )
}

/// Whether this FFmpeg build carries an encoder by that name.
///
/// A lookup in a static table — it opens nothing and cannot block.
pub fn encoder_exists(name: &str) -> bool {
    crate::modules::media::ensure_initialized();
    ffmpeg_next::encoder::find_by_name(name).is_some()
}

fn accel_slug(accel: HwAccel) -> &'static str {
    match accel {
        HwAccel::Software => "software",
        HwAccel::Vaapi => "vaapi",
        HwAccel::Qsv => "qsv",
        HwAccel::Nvenc => "nvenc",
        HwAccel::VideoToolbox => "videotoolbox",
        HwAccel::Amf => "amf",
        HwAccel::MediaFoundation => "mediafoundation",
    }
}

fn codec_slug(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
        VideoCodec::Vp9 => "vp9",
        VideoCodec::Av1 => "av1",
    }
}

/// Find `id` among the detected encoders.
pub fn find(id: &str) -> Option<HwEncoder> {
    detect().into_iter().find(|e| e.id == id)
}

// ---------------------------------------------------------------------------
// Device probes
// ---------------------------------------------------------------------------

/// Any DRM render node at all. `renderD*` rather than `card*`: the card node
/// needs the session's DRM master, the render node does not, which is the whole
/// reason it exists.
#[cfg(target_os = "linux")]
fn render_nodes() -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn render_nodes() -> Vec<std::path::PathBuf> {
    Vec::new()
}

fn vaapi_device_present() -> bool {
    !render_nodes().is_empty()
}

/// A render node whose PCI vendor is Intel.
///
/// QSV on a Mesa/AMD node initialises and then produces garbage or hangs, so
/// the vendor check is not pedantry. `/sys/class/drm/renderD128/device/vendor`
/// holds `0x8086` for Intel.
fn qsv_device_present() -> bool {
    render_nodes().iter().any(|node| {
        let Some(name) = node.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        let vendor = Path::new("/sys/class/drm").join(name).join("device/vendor");
        std::fs::read_to_string(vendor)
            .map(|v| v.trim().eq_ignore_ascii_case("0x8086"))
            .unwrap_or(false)
    })
}

/// The NVIDIA kernel module, not the userspace library: a machine can have
/// `libnvidia-encode` from a stale package with no card in it.
fn nvidia_device_present() -> bool {
    Path::new("/dev/nvidiactl").exists() || Path::new("/proc/driver/nvidia/version").exists()
}

#[cfg(target_os = "macos")]
fn videotoolbox_present() -> bool {
    // Every Mac since 2011 has it; the encoder being in the build is the real
    // question, and the caller checks that next. Left as a stub because none of
    // this is exercised on the Linux target we develop against.
    true
}

#[cfg(not(target_os = "macos"))]
fn videotoolbox_present() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn windows_present() -> bool {
    // Stub: on Windows, presence of the encoder in the build is a weak proxy
    // for a driver that works. Doing this properly means enumerating adapters
    // through DXGI, which belongs here when the Windows target is real.
    true
}

#[cfg(not(target_os = "windows"))]
fn windows_present() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_never_panics_and_never_offers_software() {
        // The point of the test is the call itself: on CI there is no GPU, on a
        // dev box there is, and neither may fail.
        let found = detect();
        assert!(found.iter().all(|e| e.accel != HwAccel::Software));
        assert!(found.iter().all(|e| e.available));
    }

    #[test]
    fn detected_encoders_have_unique_ids() {
        let mut ids: Vec<String> = detect().into_iter().map(|e| e.id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn frame_pool_encoders_are_the_ones_that_get_a_pool() {
        // VAAPI and QSV are the two that need an AVHWFramesContext.
        assert!(!HwAccel::Vaapi.accepts_software_frames());
        assert!(!HwAccel::Qsv.accepts_software_frames());
        assert!(HwAccel::Nvenc.accepts_software_frames());
        assert!(HwAccel::Software.accepts_software_frames());

        assert!(HwAccel::Vaapi.frame_pool().is_some());
        assert!(HwAccel::Qsv.frame_pool().is_some());
        assert!(HwAccel::Nvenc.frame_pool().is_none());
        assert!(HwAccel::Software.frame_pool().is_none());

        // The pool's software format is what the scaler has to target.
        assert_eq!(
            HwAccel::Vaapi.upload_format(),
            ffmpeg_next::format::Pixel::NV12
        );
        assert_eq!(
            HwAccel::Software.upload_format(),
            ffmpeg_next::format::Pixel::YUV420P
        );
    }

    #[test]
    fn an_unusable_encoder_always_says_why() {
        // The decision table the dialog reads: a listed encoder is either
        // usable, or carries prose explaining what went wrong. Never neither,
        // never both.
        for encoder in detect() {
            assert_eq!(
                encoder.note.is_some(),
                !encoder.usable,
                "{} said usable={} note={:?}",
                encoder.id,
                encoder.usable,
                encoder.note
            );
        }
    }

    #[test]
    fn the_rate_control_ladder_starts_where_the_user_asked_and_ends_somewhere_that_works() {
        let ladder = HwAccel::Vaapi.rate_control_ladder(Quality::Crf(22), 9_000_000);
        // First rung is constant quality with no bitrate: a bitrate left set
        // makes VAAPI target it and ignore the QP.
        assert_eq!(ladder[0].bit_rate, 0);
        assert!(ladder[0]
            .options
            .contains(&("rc_mode".to_string(), "CQP".to_string())));
        assert!(ladder[0]
            .options
            .contains(&("qp".to_string(), "22".to_string())));

        // Every later rung carries a bitrate, because a driver that refused
        // constant quality needs a number.
        assert!(ladder.len() > 1);
        assert!(ladder[1..].iter().all(|rung| rung.bit_rate == 9_000_000));
        // And the last rung asks for nothing at all, so there is always a rung
        // that cannot be refused for naming an unsupported mode.
        assert!(ladder.last().unwrap().options.is_empty());
    }

    #[test]
    fn a_bitrate_request_has_no_ladder_to_climb_down() {
        // `bit_rate` is understood by every encoder that exists, so there is
        // nothing to fall back to and trying would only slow the open down.
        for accel in [HwAccel::Software, HwAccel::Vaapi, HwAccel::Nvenc] {
            let ladder = accel.rate_control_ladder(Quality::Bitrate(12_000_000), 9_000_000);
            assert_eq!(ladder.len(), 1, "{accel:?}");
            assert_eq!(ladder[0].bit_rate, 12_000_000);
        }
    }

    #[test]
    fn software_has_no_fallback_because_crf_always_works() {
        let ladder = HwAccel::Software.rate_control_ladder(Quality::Crf(20), 9_000_000);
        assert_eq!(ladder.len(), 1);
        assert_eq!(
            ladder[0].options,
            vec![("crf".to_string(), "20".to_string())]
        );
        assert_eq!(ladder[0].bit_rate, 0);
    }

    #[test]
    fn the_fallback_bitrate_moves_the_right_way_and_stays_sane() {
        let fps = Fps::THIRTY;
        let at = |crf: u8| fallback_bitrate(1920, 1080, fps, Quality::Crf(crf));

        // Six points of CRF is a factor of two, which is the whole rule.
        let ratio = at(17) as f64 / at(23) as f64;
        assert!((ratio - 2.0).abs() < 0.01, "ratio was {ratio}");
        assert!(at(18) > at(23) && at(23) > at(28));

        // CRF 23 at 1080p30 should land near what x264 produces there.
        assert!((6_000_000..=12_000_000).contains(&at(23)), "{}", at(23));

        // Larger canvases and higher rates cost more, monotonically.
        assert!(
            fallback_bitrate(3840, 2160, fps, Quality::Crf(23))
                > fallback_bitrate(1920, 1080, fps, Quality::Crf(23))
        );
        assert!(
            fallback_bitrate(1920, 1080, Fps::SIXTY, Quality::Crf(23))
                > fallback_bitrate(1920, 1080, Fps::THIRTY, Quality::Crf(23))
        );

        // A bitrate request passes straight through.
        assert_eq!(
            fallback_bitrate(1920, 1080, fps, Quality::Bitrate(4_000_000)),
            4_000_000
        );
        // Degenerate inputs cannot produce a bitrate the muxer chokes on.
        assert!(fallback_bitrate(2, 2, fps, Quality::Crf(51)) >= 250_000);
        assert!(fallback_bitrate(7680, 4320, Fps::SIXTY, Quality::Crf(0)) <= 200_000_000);
    }

    #[test]
    fn every_accel_names_its_h264_encoder() {
        for accel in [
            HwAccel::Software,
            HwAccel::Vaapi,
            HwAccel::Qsv,
            HwAccel::Nvenc,
            HwAccel::VideoToolbox,
            HwAccel::Amf,
            HwAccel::MediaFoundation,
        ] {
            assert!(accel.encoder_name(VideoCodec::H264).is_some());
        }
        // VP9 exists on VAAPI and nowhere else we support.
        assert!(HwAccel::Nvenc.encoder_name(VideoCodec::Vp9).is_none());
    }

    #[test]
    fn quality_options_speak_each_encoders_dialect() {
        assert_eq!(
            HwAccel::Software.quality_options(Quality::Crf(20)),
            vec![("crf", "20".to_string())]
        );
        let nvenc = HwAccel::Nvenc.quality_options(Quality::Crf(20));
        assert!(nvenc.contains(&("cq", "20".to_string())));
        // Without this, NVENC clamps to whatever bit_rate happens to be set.
        assert!(nvenc.contains(&("b", "0".to_string())));

        // A bitrate target is expressed through the codec context, not through
        // private options, on every backend.
        assert!(HwAccel::Nvenc
            .quality_options(Quality::Bitrate(12_000_000))
            .is_empty());
    }

    #[test]
    fn a_missing_encoder_name_is_false_not_a_panic() {
        assert!(!encoder_exists("definitely_not_an_encoder"));
    }
}
