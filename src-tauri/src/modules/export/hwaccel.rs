//! Which hardware encoders this machine actually has.
//!
//! Two different questions get confused here, so this file keeps them apart:
//!
//! 1. **Is the encoder compiled into FFmpeg?** `avcodec_find_encoder_by_name`
//!    answers that, cheaply and without touching a device.
//! 2. **Is there a device it can drive?** A build with `h264_vaapi` in it on a
//!    box with no `/dev/dri` is the normal case on a headless server, and on a
//!    laptop whose GPU is claimed by another process the device node exists and
//!    initialisation still fails.
//!
//! Detection answers both by *looking*, never by initialising: no device is
//! opened, no encoder is started, nothing here can hang on a wedged driver or
//! abort inside a vendor blob. A missing device node is normal, not
//! exceptional, so every failure path returns "not available" rather than an
//! error, and the whole probe additionally runs inside `catch_unwind` — a panic
//! while enumerating optional hardware must not take the app with it.
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

use serde::{Deserialize, Serialize};

use super::presets::{Quality, VideoCodec};

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
    /// frame pool — `AVHWFramesContext`, which `ffmpeg-next` does not wrap —
    /// so handing them a YUV420P buffer fails at `avcodec_open2`. Until that is
    /// built, those two are listed and refused rather than offered and broken.
    pub fn accepts_software_frames(self) -> bool {
        !matches!(self, HwAccel::Vaapi | HwAccel::Qsv)
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
pub fn detect() -> Vec<HwEncoder> {
    // Enumerating optional vendor hardware is exactly the kind of code that
    // aborts on a broken driver. A panic here would take the export dialog —
    // and with it the app — down for a feature the user did not ask for.
    std::panic::catch_unwind(probe).unwrap_or_else(|_| {
        tracing::warn!("hardware encoder detection panicked; offering software only");
        Vec::new()
    })
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
            let usable = accel.accepts_software_frames();
            found.push(HwEncoder {
                id: format!("{}_{}", accel_slug(accel), codec_slug(codec)),
                accel,
                codec,
                encoder_name: name.to_string(),
                label: format!("{} ({})", codec.label(), accel.label()),
                available: true,
                usable,
                note: (!usable).then(|| {
                    format!(
                        "{} encoding needs a hardware frame pool, which is not wired up yet",
                        accel.label()
                    )
                }),
            });
        }
    }
    found
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
    fn frame_pool_encoders_are_listed_but_refused() {
        // VAAPI and QSV are the two that need an AVHWFramesContext.
        assert!(!HwAccel::Vaapi.accepts_software_frames());
        assert!(!HwAccel::Qsv.accepts_software_frames());
        assert!(HwAccel::Nvenc.accepts_software_frames());
        assert!(HwAccel::Software.accepts_software_frames());

        for encoder in detect() {
            assert_eq!(encoder.usable, encoder.accel.accepts_software_frames());
            assert_eq!(encoder.note.is_some(), !encoder.usable);
        }
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
