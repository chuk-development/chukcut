//! The export dialog's choices, and how they become an [`ExportRequest`].
//!
//! The dialog offers CapCut's vocabulary — "1080p", "Recommended", "mp4" —
//! and the engine takes exact numbers. This file is the whole translation,
//! kept free of UI types so it can be tested on its own.

use std::path::{Path, PathBuf};

use chukcut_engine::modules::export::hwaccel::{fallback_bitrate, HwEncoder};
use chukcut_engine::modules::export::{
    AudioCodec, Container, ExportOverrides, ExportRequest, Fps, Quality, VideoCodec,
};
use chukcut_engine::modules::project::{Micros, Project};

/// The output's short side. The long side follows the canvas's aspect ratio,
/// so "1080p" of a 9:16 project is 1080×1920 and of a 16:9 one 1920×1080.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolution {
    P480,
    P720,
    P1080,
    K2,
    K4,
}

impl Resolution {
    pub(crate) const ALL: [Resolution; 5] = [
        Resolution::P480,
        Resolution::P720,
        Resolution::P1080,
        Resolution::K2,
        Resolution::K4,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Resolution::P480 => "480p",
            Resolution::P720 => "720p",
            Resolution::P1080 => "1080p",
            Resolution::K2 => "2K",
            Resolution::K4 => "4K",
        }
    }

    fn short_side(self) -> u32 {
        match self {
            Resolution::P480 => 480,
            Resolution::P720 => 720,
            Resolution::P1080 => 1080,
            Resolution::K2 => 1440,
            Resolution::K4 => 2160,
        }
    }

    /// The option closest to the canvas, so the default export is the size
    /// the user framed.
    pub(crate) fn closest_to(width: u32, height: u32) -> Resolution {
        let short = width.min(height);
        Resolution::ALL
            .into_iter()
            .min_by_key(|r| r.short_side().abs_diff(short))
            .unwrap_or(Resolution::P1080)
    }

    /// The output size for a canvas, both sides even (4:2:0 needs that).
    pub(crate) fn size_for(self, canvas_width: u32, canvas_height: u32) -> (u32, u32) {
        let short = self.short_side();
        let (cw, ch) = (canvas_width.max(2) as f64, canvas_height.max(2) as f64);
        let long = |ratio: f64| even((short as f64 * ratio).round() as u32);
        if cw >= ch {
            (long(cw / ch), short)
        } else {
            (short, long(ch / cw))
        }
    }
}

fn even(value: u32) -> u32 {
    (value + 1) & !1
}

/// CapCut's bitrate choice. The first three are quality targets (CRF), the
/// last a fixed average bitrate the user types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bitrate {
    Lower,
    Recommended,
    Higher,
    Custom,
}

impl Bitrate {
    pub(crate) const ALL: [Bitrate; 4] = [
        Bitrate::Lower,
        Bitrate::Recommended,
        Bitrate::Higher,
        Bitrate::Custom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Bitrate::Lower => "Lower",
            Bitrate::Recommended => "Recommended",
            Bitrate::Higher => "Higher",
            Bitrate::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub(crate) const ALL: [Codec; 3] = [Codec::H264, Codec::Hevc, Codec::Av1];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }

    pub(crate) fn video_codec(self) -> VideoCodec {
        match self {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::H265,
            Codec::Av1 => VideoCodec::Av1,
        }
    }

    /// The CRF for a quality level. Each encoder has its own scale: x265's
    /// number is about two points "quieter" than x264's for the same picture,
    /// and SVT-AV1 counts to 63.
    fn crf(self, level: Bitrate) -> u8 {
        let (lower, recommended, higher) = match self {
            Codec::H264 => (26, 20, 16),
            Codec::Hevc => (28, 22, 18),
            Codec::Av1 => (40, 32, 24),
        };
        match level {
            Bitrate::Lower => lower,
            Bitrate::Higher => higher,
            Bitrate::Recommended | Bitrate::Custom => recommended,
        }
    }

    /// How much smaller than H.264 this codec is at the same quality. Only
    /// for the size estimate; the encoder decides the real number.
    fn size_factor(self) -> f64 {
        match self {
            Codec::H264 => 1.0,
            Codec::Hevc => 0.6,
            Codec::Av1 => 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Mp4,
    Mov,
}

impl Format {
    pub(crate) const ALL: [Format; 2] = [Format::Mp4, Format::Mov];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::Mov => "mov",
        }
    }

    fn container(self) -> Container {
        match self {
            Format::Mp4 => Container::Mp4,
            Format::Mov => Container::Mov,
        }
    }
}

/// CapCut's frame-rate list, with the NTSC rates spelled as people know them.
pub(crate) const FRAME_RATES: [(f64, &str); 7] = [
    (24.0, "24 fps"),
    (25.0, "25 fps"),
    (29.97, "29.97 fps"),
    (30.0, "30 fps"),
    (50.0, "50 fps"),
    (59.94, "59.94 fps"),
    (60.0, "60 fps"),
];

/// Audio bitrates offered for AAC, in bits per second.
pub(crate) const AUDIO_BITRATES: [(u32, &str); 3] = [
    (128_000, "AAC · 128 kbps"),
    (192_000, "AAC · 192 kbps"),
    (320_000, "AAC · 320 kbps"),
];

/// Everything the dialog lets the user choose.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportChoices {
    /// The file name without extension.
    pub name: String,
    pub directory: PathBuf,
    pub resolution: Resolution,
    pub bitrate: Bitrate,
    /// For [`Bitrate::Custom`], in Mbit/s.
    pub custom_mbps: f64,
    pub codec: Codec,
    pub format: Format,
    pub fps: f64,
    pub audio: bool,
    pub audio_bitrate: u32,
    /// Integrated loudness to bring the mix to, in LUFS; `None` is off.
    pub loudness_target: Option<f32>,
}

impl ExportChoices {
    /// The choices a fresh dialog opens with: the project's own size and
    /// rate, H.264 at the recommended quality, into `directory`.
    pub(crate) fn for_project(project: &Project, directory: PathBuf) -> Self {
        let fps = FRAME_RATES
            .iter()
            .map(|(rate, _)| *rate)
            .min_by(|a, b| (a - project.fps).abs().total_cmp(&(b - project.fps).abs()))
            .unwrap_or(30.0);
        Self {
            name: project.name.clone(),
            directory,
            resolution: Resolution::closest_to(project.canvas.width, project.canvas.height),
            bitrate: Bitrate::Recommended,
            custom_mbps: 10.0,
            codec: Codec::H264,
            format: Format::Mp4,
            fps,
            audio: true,
            audio_bitrate: 192_000,
            loudness_target: None,
        }
    }

    pub(crate) fn size(&self, project: &Project) -> (u32, u32) {
        self.resolution
            .size_for(project.canvas.width, project.canvas.height)
    }

    pub(crate) fn quality(&self) -> Quality {
        match self.bitrate {
            Bitrate::Custom => {
                Quality::Bitrate((self.custom_mbps.clamp(0.1, 2000.0) * 1_000_000.0) as u64)
            }
            level => Quality::Crf(self.codec.crf(level)),
        }
    }

    /// Where the file goes. A name the user cleared falls back to "export",
    /// and a path separator typed into the name cannot climb out of the
    /// chosen folder.
    pub(crate) fn output_path(&self) -> PathBuf {
        let name = self.name.trim().replace('/', "_");
        let name = if name.is_empty() {
            "export".to_string()
        } else {
            name
        };
        self.directory
            .join(format!("{name}.{}", self.format.label()))
    }

    pub(crate) fn overrides(&self, project: &Project) -> ExportOverrides {
        let (width, height) = self.size(project);
        ExportOverrides {
            width: Some(width),
            height: Some(height),
            fps: Some(self.fps),
            video_codec: Some(self.codec.video_codec()),
            quality: Some(self.quality()),
            audio_codec: Some(if self.audio {
                AudioCodec::Aac
            } else {
                AudioCodec::None
            }),
            audio_bitrate: self.audio.then_some(self.audio_bitrate),
            sample_rate: None,
            container: Some(self.format.container()),
            loudness_target: self.audio.then_some(self.loudness_target).flatten(),
        }
    }

    /// The request for `export_start`. `hardware` is an encoder id from
    /// [`pick_hardware`], or `None` for the software encoder.
    pub(crate) fn request(&self, project: &Project, hardware: Option<String>) -> ExportRequest {
        ExportRequest {
            output_path: self.output_path().to_string_lossy().into_owned(),
            preset_id: None,
            overrides: Some(self.overrides(project)),
            hardware,
            include_audio: self.audio,
            range: None,
        }
    }

    /// Roughly how big the file will be, in bytes. CRF has no size, so this
    /// uses the engine's own CRF-to-bitrate rule of thumb, scaled for the
    /// codec.
    pub(crate) fn estimated_bytes(&self, project: &Project) -> u64 {
        let (width, height) = self.size(project);
        let quality = self.quality();
        let video = fallback_bitrate(width, height, Fps::from_f64(self.fps), quality) as f64;
        let video = match quality {
            Quality::Crf(_) => video * self.codec.size_factor(),
            Quality::Bitrate(_) => video,
        };
        let audio = if self.audio {
            self.audio_bitrate as f64
        } else {
            0.0
        };
        let seconds = project.duration().max(0) as f64 / 1_000_000.0;
        ((video + audio) * seconds / 8.0) as u64
    }
}

/// The hardware encoder to use for `codec`: the first one that passed its
/// trial encode. NVENC, VAAPI and QSV come back from detection in the
/// engine's order of preference.
pub(crate) fn pick_hardware(encoders: &[HwEncoder], codec: Codec) -> Option<&HwEncoder> {
    encoders
        .iter()
        .find(|encoder| encoder.usable && encoder.codec == codec.video_codec())
}

/// "about 220 MB".
pub(crate) fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("about {:.1} GB", mb / 1000.0)
    } else if mb >= 10.0 {
        format!("about {mb:.0} MB")
    } else {
        format!("about {mb:.1} MB")
    }
}

/// "3m 42s", "12s", "1h 02m".
pub(crate) fn duration_label(duration: Micros) -> String {
    let total = (duration.max(0) as f64 / 1_000_000.0).round() as i64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// Where the folder of the last export is kept: one path, plain text.
fn last_directory_file() -> PathBuf {
    chukcut_engine::modules::workspace::paths::config_root().join("last-export-folder")
}

/// Keep `directory` as the folder the next export starts in.
pub(crate) fn remember_directory(directory: &Path) {
    let file = last_directory_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Losing this costs one click in the next export; not worth an error.
    let _ = std::fs::write(file, directory.to_string_lossy().as_bytes());
}

/// The folder new exports go to: the last one used when it still exists,
/// else `~/Videos` when it exists, else home.
pub(crate) fn default_directory() -> PathBuf {
    let last = std::fs::read_to_string(last_directory_file())
        .map(|text| PathBuf::from(text.trim()))
        .ok()
        .filter(|path| path.is_absolute() && path.is_dir());
    if let Some(last) = last {
        return last;
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let videos = home.join("Videos");
    if videos.is_dir() {
        videos
    } else {
        home
    }
}

/// A path shortened for a one-line field: `~` for home.
pub(crate) fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::export::HwAccel;
    use chukcut_engine::modules::project::CanvasConfig;

    fn project(width: u32, height: u32, fps: f64) -> Project {
        Project::new(
            "Clip",
            CanvasConfig {
                width,
                height,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            fps,
        )
    }

    #[test]
    fn the_short_side_is_the_named_resolution_whatever_the_orientation() {
        assert_eq!(Resolution::P1080.size_for(1080, 1920), (1080, 1920));
        assert_eq!(Resolution::P1080.size_for(1920, 1080), (1920, 1080));
        assert_eq!(Resolution::P720.size_for(1080, 1920), (720, 1280));
        assert_eq!(Resolution::K4.size_for(1920, 1080), (3840, 2160));
        assert_eq!(Resolution::P480.size_for(1080, 1080), (480, 480));
    }

    #[test]
    fn an_odd_aspect_ratio_still_gives_even_sides() {
        // 480 * 4/3 = 640, 480 * 21/9 = 1120, 720 * 1.85 = 1332.
        let (w, h) = Resolution::P720.size_for(1850, 1000);
        assert_eq!(h, 720);
        assert_eq!(w % 2, 0);
        let (w, h) = Resolution::P480.size_for(1001, 1000);
        assert_eq!((w % 2, h % 2), (0, 0));
    }

    #[test]
    fn the_dialog_opens_at_the_projects_size_and_rate() {
        let choices = ExportChoices::for_project(&project(1080, 1920, 29.97), PathBuf::from("/x"));
        assert_eq!(choices.resolution, Resolution::P1080);
        assert_eq!(choices.fps, 29.97);
        let choices = ExportChoices::for_project(&project(3840, 2160, 60.0), PathBuf::from("/x"));
        assert_eq!(choices.resolution, Resolution::K4);
        assert_eq!(choices.fps, 60.0);
    }

    #[test]
    fn quality_levels_map_onto_each_codecs_own_scale() {
        let mut choices = ExportChoices::for_project(&project(1080, 1920, 30.0), PathBuf::new());
        assert_eq!(choices.quality(), Quality::Crf(20));
        choices.bitrate = Bitrate::Higher;
        assert_eq!(choices.quality(), Quality::Crf(16));
        choices.codec = Codec::Av1;
        assert_eq!(choices.quality(), Quality::Crf(24));
        choices.bitrate = Bitrate::Custom;
        choices.custom_mbps = 12.5;
        assert_eq!(choices.quality(), Quality::Bitrate(12_500_000));
    }

    #[test]
    fn the_request_carries_every_choice() {
        let p = project(1920, 1080, 30.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.codec = Codec::Hevc;
        choices.format = Format::Mov;
        choices.resolution = Resolution::P720;
        choices.audio = false;
        let request = choices.request(&p, None);
        assert_eq!(request.output_path, "/out/Clip.mov");
        assert!(!request.include_audio);
        let o = request.overrides.expect("overrides");
        assert_eq!((o.width, o.height), (Some(1280), Some(720)));
        assert_eq!(o.video_codec, Some(VideoCodec::H265));
        assert_eq!(o.container, Some(Container::Mov));
        assert_eq!(o.audio_codec, Some(AudioCodec::None));
    }

    #[test]
    fn a_name_cannot_leave_the_chosen_folder() {
        let mut choices = ExportChoices::for_project(&project(1080, 1920, 30.0), "/out".into());
        choices.name = "../../etc/x".into();
        assert_eq!(choices.output_path(), PathBuf::from("/out/.._.._etc_x.mp4"));
        choices.name = "   ".into();
        assert_eq!(choices.output_path(), PathBuf::from("/out/export.mp4"));
    }

    #[test]
    fn the_hardware_encoder_must_match_the_codec_and_be_usable() {
        let encoder = |id: &str, codec, usable| HwEncoder {
            id: id.into(),
            accel: HwAccel::Nvenc,
            codec,
            encoder_name: id.into(),
            label: id.into(),
            available: true,
            usable,
            note: None,
        };
        let encoders = [
            encoder("broken_h264", VideoCodec::H264, false),
            encoder("nvenc_hevc", VideoCodec::H265, true),
            encoder("nvenc_h264", VideoCodec::H264, true),
        ];
        assert_eq!(
            pick_hardware(&encoders, Codec::H264).map(|e| e.id.as_str()),
            Some("nvenc_h264")
        );
        assert_eq!(
            pick_hardware(&encoders, Codec::Hevc).map(|e| e.id.as_str()),
            Some("nvenc_hevc")
        );
        assert!(pick_hardware(&encoders, Codec::Av1).is_none());
    }

    #[test]
    fn labels_read_like_capcuts_footer() {
        assert_eq!(duration_label(222_000_000), "3m 42s");
        assert_eq!(duration_label(12_400_000), "12s");
        assert_eq!(duration_label(3_720_000_000), "1h 02m");
        assert_eq!(size_label(220_000_000), "about 220 MB");
        assert_eq!(size_label(1_500_000_000), "about 1.5 GB");
    }
}
