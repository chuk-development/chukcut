//! The export dialog's choices, and how they become an [`ExportRequest`].
//!
//! The dialog offers CapCut's vocabulary — "1080p", "Recommended", "mp4" —
//! and the engine takes exact numbers. This file is the whole translation,
//! kept free of UI types so it can be tested on its own.

use std::path::{Path, PathBuf};

use chukcut_engine::modules::export::hwaccel::HwEncoder;
use chukcut_engine::modules::export::{
    AudioCodec, Container, ExportMemory, ExportOverrides, ExportPreset, ExportRequest, Quality,
    VideoCodec,
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

    pub(crate) fn short_side(self) -> u32 {
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
        Resolution::closest_short_side(width.min(height))
    }

    /// The option whose short side is nearest `short`.
    pub(crate) fn closest_short_side(short: u32) -> Resolution {
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
    /// ProRes 422 HQ, for a master. Always .mov with PCM sound.
    ProRes,
}

impl Codec {
    pub(crate) const ALL: [Codec; 4] = [Codec::H264, Codec::Hevc, Codec::Av1, Codec::ProRes];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
            Codec::ProRes => "ProRes",
        }
    }

    pub(crate) fn video_codec(self) -> VideoCodec {
        match self {
            Codec::H264 => VideoCodec::H264,
            Codec::Hevc => VideoCodec::H265,
            Codec::Av1 => VideoCodec::Av1,
            Codec::ProRes => VideoCodec::ProRes,
        }
    }

    fn from_video_codec(codec: VideoCodec) -> Codec {
        match codec {
            VideoCodec::H265 => Codec::Hevc,
            VideoCodec::Av1 => Codec::Av1,
            VideoCodec::ProRes => Codec::ProRes,
            // VP9 is not offered in the dialog; a preset or remembered
            // setting that names it gets the nearest thing that is.
            VideoCodec::H264 | VideoCodec::Vp9 | VideoCodec::Gif => Codec::H264,
        }
    }

    /// ProRes has no quality knob: its data rate follows from the profile.
    pub(crate) fn has_quality(self) -> bool {
        self != Codec::ProRes
    }

    /// The CRF for a quality level. Each encoder has its own scale: x265's
    /// number is about two points "quieter" than x264's for the same picture,
    /// and SVT-AV1 counts to 63.
    fn crf(self, level: Bitrate) -> u8 {
        let (lower, recommended, higher) = match self {
            Codec::H264 | Codec::ProRes => (26, 20, 16),
            Codec::Hevc => (28, 22, 18),
            Codec::Av1 => (40, 32, 24),
        };
        match level {
            Bitrate::Lower => lower,
            Bitrate::Higher => higher,
            Bitrate::Recommended | Bitrate::Custom => recommended,
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

/// What the file is: a video, sound only, or an animated GIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputKind {
    Video,
    Audio,
    Gif,
}

impl OutputKind {
    pub(crate) const ALL: [OutputKind; 3] = [OutputKind::Video, OutputKind::Audio, OutputKind::Gif];

    pub(crate) fn label(self) -> &'static str {
        match self {
            OutputKind::Video => "Video",
            OutputKind::Audio => "Audio only",
            OutputKind::Gif => "GIF",
        }
    }
}

/// The file type of a sound-only export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioFormat {
    Aac,
    Mp3,
    Wav,
}

impl AudioFormat {
    pub(crate) const ALL: [AudioFormat; 3] = [AudioFormat::Aac, AudioFormat::Mp3, AudioFormat::Wav];

    pub(crate) fn label(self) -> &'static str {
        match self {
            AudioFormat::Aac => "AAC (.m4a)",
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Wav => "WAV",
        }
    }

    fn container(self) -> Container {
        match self {
            AudioFormat::Aac => Container::M4a,
            AudioFormat::Mp3 => Container::Mp3,
            AudioFormat::Wav => Container::Wav,
        }
    }

    fn codec(self) -> AudioCodec {
        match self {
            AudioFormat::Aac => AudioCodec::Aac,
            AudioFormat::Mp3 => AudioCodec::Mp3,
            AudioFormat::Wav => AudioCodec::Pcm,
        }
    }
}

/// Frame rates for a GIF: every one of them divides into the format's
/// hundredths of a second evenly or nearly so.
pub(crate) const GIF_RATES: [(f64, &str); 4] = [
    (10.0, "10 fps"),
    (12.5, "12.5 fps"),
    (15.0, "15 fps"),
    (25.0, "25 fps"),
];

/// CapCut's frame-rate list, with the NTSC rates spelled as people know them.
pub(crate) const FRAME_RATES: [(f64, &str); 8] = [
    (23.976, "23.976 fps"),
    (24.0, "24 fps"),
    (25.0, "25 fps"),
    (29.97, "29.97 fps"),
    (30.0, "30 fps"),
    (50.0, "50 fps"),
    (59.94, "59.94 fps"),
    (60.0, "60 fps"),
];

/// Audio bitrates offered for AAC and MP3, in bits per second.
pub(crate) const AUDIO_BITRATES: [(u32, &str); 4] = [
    (128_000, "128 kbps"),
    (192_000, "192 kbps"),
    (256_000, "256 kbps"),
    (320_000, "320 kbps"),
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
    pub kind: OutputKind,
    pub audio_format: AudioFormat,
    /// The preset these choices were set from, while the user has not
    /// changed anything a preset decides. While it is set the export asks
    /// for the preset itself, so it is exactly the preset.
    pub preset: Option<String>,
    /// Export only this part of the timeline.
    pub range: Option<(Micros, Micros)>,
    /// Use a GPU encoder when one works for the codec.
    pub use_hardware: bool,
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
            kind: OutputKind::Video,
            audio_format: AudioFormat::Aac,
            preset: None,
            range: None,
            use_hardware: true,
        }
    }

    /// The choices a dialog opens with: the remembered ones when there are
    /// any, else the defaults for the project.
    pub(crate) fn opening(
        project: &Project,
        directory: PathBuf,
        memory: Option<&ExportMemory>,
    ) -> Self {
        let mut choices = Self::for_project(project, directory);
        if let Some(memory) = memory {
            choices.restore(project, memory);
        }
        choices
    }

    /// Set every choice a preset decides. The name, folder and range stay.
    pub(crate) fn apply_preset(&mut self, project: &Project, preset: &ExportPreset) {
        let fitted = preset.for_project(project);
        self.kind = if fitted.container.is_audio_only() {
            OutputKind::Audio
        } else if fitted.container == Container::Gif {
            OutputKind::Gif
        } else {
            OutputKind::Video
        };
        if self.kind != OutputKind::Audio {
            self.resolution = Resolution::closest_short_side(fitted.width.min(fitted.height));
            self.fps = fitted.fps.as_f64();
        }
        if self.kind == OutputKind::Video {
            self.codec = Codec::from_video_codec(fitted.video_codec);
            self.format = if fitted.container == Container::Mov {
                Format::Mov
            } else {
                Format::Mp4
            };
            self.set_quality(fitted.quality);
        }
        self.audio_format = match fitted.audio_codec {
            AudioCodec::Mp3 => AudioFormat::Mp3,
            AudioCodec::Pcm => AudioFormat::Wav,
            _ => AudioFormat::Aac,
        };
        self.audio = fitted.audio_codec != AudioCodec::None;
        if self.audio && fitted.audio_codec != AudioCodec::Pcm {
            self.audio_bitrate = fitted.audio_bitrate;
        }
        self.loudness_target = fitted.loudness_target;
        self.preset = Some(preset.id.clone());
        self.snap_fps();
    }

    /// Put the frame rate on the nearest entry of the list the dialog shows
    /// for this kind of file, so the picker never shows nothing.
    fn snap_fps(&mut self) {
        let list: &[(f64, &str)] = match self.kind {
            OutputKind::Gif => &GIF_RATES,
            _ => &FRAME_RATES,
        };
        let fps = self.fps;
        if let Some(rate) = list
            .iter()
            .map(|(rate, _)| *rate)
            .min_by(|a, b| (a - fps).abs().total_cmp(&(b - fps).abs()))
        {
            self.fps = rate;
        }
    }

    /// Switch between video, sound only and GIF. A GIF's frame rate and
    /// silence do not carry over into a video, nor a video's 4K into a GIF:
    /// each side gets the nearest setting that makes sense for it.
    pub(crate) fn set_kind(&mut self, kind: OutputKind) {
        if kind == self.kind {
            return;
        }
        let nearest = |list: &[(f64, &str)], fps: f64| {
            list.iter()
                .map(|(rate, _)| *rate)
                .min_by(|a, b| (a - fps).abs().total_cmp(&(b - fps).abs()))
                .unwrap_or(fps)
        };
        match kind {
            OutputKind::Gif => {
                self.fps = nearest(&GIF_RATES, self.fps.min(15.0));
                if self.resolution.short_side() > 720 {
                    self.resolution = Resolution::P480;
                }
            }
            OutputKind::Video => {
                if self.kind == OutputKind::Gif {
                    self.fps = nearest(&FRAME_RATES, 30.0);
                    self.audio = true;
                }
            }
            OutputKind::Audio => {}
        }
        self.kind = kind;
    }

    /// Show a quality on the bitrate control: the level whose CRF is
    /// nearest, or a custom bitrate.
    fn set_quality(&mut self, quality: Quality) {
        match quality {
            Quality::Bitrate(bits) => {
                self.bitrate = Bitrate::Custom;
                self.custom_mbps = bits as f64 / 1_000_000.0;
            }
            Quality::Crf(crf) => {
                let codec = self.codec;
                self.bitrate = [Bitrate::Lower, Bitrate::Recommended, Bitrate::Higher]
                    .into_iter()
                    .min_by_key(|level| codec.crf(*level).abs_diff(crf))
                    .unwrap_or(Bitrate::Recommended);
            }
        }
    }

    /// Put remembered settings back: the preset when there was one and it
    /// still exists, else each remembered override.
    pub(crate) fn restore(&mut self, project: &Project, memory: &ExportMemory) {
        if let Some(directory) = memory.directory.as_ref().map(PathBuf::from) {
            if directory.is_absolute() && directory.is_dir() {
                self.directory = directory;
            }
        }
        self.use_hardware = memory.use_hardware;
        if let Some(preset) = memory
            .preset_id
            .as_deref()
            .and_then(chukcut_engine::modules::export::job::find_preset)
        {
            self.apply_preset(project, &preset);
            self.audio = memory.include_audio || self.kind == OutputKind::Audio;
            return;
        }
        let o = &memory.overrides;
        match o.container {
            Some(Container::Gif) => self.kind = OutputKind::Gif,
            Some(c) if c.is_audio_only() => {
                self.kind = OutputKind::Audio;
                self.audio_format = match c {
                    Container::Mp3 => AudioFormat::Mp3,
                    Container::Wav => AudioFormat::Wav,
                    _ => AudioFormat::Aac,
                };
            }
            Some(Container::Mov) => {
                self.kind = OutputKind::Video;
                self.format = Format::Mov;
            }
            Some(_) => {
                self.kind = OutputKind::Video;
                self.format = Format::Mp4;
            }
            None => {}
        }
        if let (Some(w), Some(h)) = (o.width, o.height) {
            self.resolution = Resolution::closest_short_side(w.min(h));
        }
        if let Some(fps) = o.fps {
            self.fps = fps;
        }
        if let Some(codec) = o.video_codec {
            if self.kind == OutputKind::Video {
                self.codec = Codec::from_video_codec(codec);
            }
        }
        if let Some(quality) = o.quality {
            self.set_quality(quality);
        }
        if let Some(bits) = o.audio_bitrate {
            self.audio_bitrate = bits;
        }
        self.loudness_target = o.loudness_target;
        self.audio = memory.include_audio;
        self.preset = None;
        // A remembered rate that this kind of file does not offer (an old
        // setting, another version) would leave the picker empty.
        self.snap_fps();
    }

    /// What to remember of these choices for the next export.
    pub(crate) fn memory(&self, project: &Project) -> ExportMemory {
        ExportMemory {
            preset_id: self.preset.clone(),
            overrides: self.overrides(project),
            include_audio: self.audio,
            use_hardware: self.use_hardware,
            directory: Some(self.directory.to_string_lossy().into_owned()),
        }
    }

    /// The file extension the export will have.
    pub(crate) fn extension(&self) -> &'static str {
        match self.kind {
            OutputKind::Video => {
                if self.codec == Codec::ProRes {
                    "mov"
                } else {
                    self.format.label()
                }
            }
            OutputKind::Audio => self.audio_format.container().extension(),
            OutputKind::Gif => "gif",
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
        self.directory.join(format!("{name}.{}", self.extension()))
    }

    pub(crate) fn overrides(&self, project: &Project) -> ExportOverrides {
        let (width, height) = self.size(project);
        let loudness = (self.audio || self.kind == OutputKind::Audio)
            .then_some(self.loudness_target)
            .flatten();
        let base = ExportOverrides {
            loudness_target: loudness,
            // An explicit "off": a preset's target must not come back through
            // a choice the user switched off.
            loudness_off: loudness.is_none(),
            sample_rate: Some(48_000),
            ..Default::default()
        };
        match self.kind {
            OutputKind::Audio => ExportOverrides {
                audio_codec: Some(self.audio_format.codec()),
                audio_bitrate: (self.audio_format != AudioFormat::Wav)
                    .then_some(self.audio_bitrate.min(320_000)),
                container: Some(self.audio_format.container()),
                ..base
            },
            OutputKind::Gif => ExportOverrides {
                width: Some(width),
                height: Some(height),
                fps: Some(self.fps),
                video_codec: Some(VideoCodec::Gif),
                audio_codec: Some(AudioCodec::None),
                container: Some(Container::Gif),
                loudness_target: None,
                loudness_off: true,
                ..base
            },
            OutputKind::Video => {
                let prores = self.codec == Codec::ProRes;
                ExportOverrides {
                    width: Some(width),
                    height: Some(height),
                    fps: Some(self.fps),
                    video_codec: Some(self.codec.video_codec()),
                    quality: Some(self.quality()),
                    audio_codec: Some(match (self.audio, prores) {
                        (false, _) => AudioCodec::None,
                        (true, true) => AudioCodec::Pcm,
                        (true, false) => AudioCodec::Aac,
                    }),
                    audio_bitrate: (self.audio && !prores).then_some(self.audio_bitrate),
                    container: Some(if prores {
                        Container::Mov
                    } else {
                        self.format.container()
                    }),
                    ..base
                }
            }
        }
    }

    /// The request for `export_start`. `hardware` is an encoder id from
    /// [`pick_hardware`], or `None` for the software encoder.
    ///
    /// While a preset is chosen and untouched, the request names the preset
    /// and nothing else, so what is exported is exactly that preset.
    pub(crate) fn request(&self, project: &Project, hardware: Option<String>) -> ExportRequest {
        let hardware = hardware.filter(|_| self.use_hardware && self.kind == OutputKind::Video);
        let (preset_id, overrides) = match &self.preset {
            Some(id) => (Some(id.clone()), None),
            None => (None, Some(self.overrides(project))),
        };
        ExportRequest {
            output_path: self.output_path().to_string_lossy().into_owned(),
            preset_id,
            overrides,
            hardware,
            include_audio: self.audio || self.kind == OutputKind::Audio,
            range: self.range,
        }
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
    format!("about {}", bytes_label(bytes))
}

/// "220 MB", "1.5 GB", "3.4 MB", "640 KB".
pub(crate) fn bytes_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", (bytes as f64 / 1000.0).max(1.0))
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
    fn a_preset_sets_the_choices_and_is_exported_as_itself() {
        let p = project(1080, 1920, 25.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        let tiktok = ExportPreset::by_id("tiktok").unwrap();
        choices.apply_preset(&p, &tiktok);
        assert_eq!(choices.resolution, Resolution::P1080);
        assert_eq!(choices.fps, 30.0);
        assert_eq!(choices.codec, Codec::H264);
        assert_eq!(choices.loudness_target, Some(-14.0));
        let request = choices.request(&p, None);
        assert_eq!(request.preset_id.as_deref(), Some("tiktok"));
        assert!(request.overrides.is_none());
        assert_eq!(request.output_path, "/out/Clip.mp4");

        // Once the user changes something, the choices are exported as
        // they are shown.
        choices.preset = None;
        let request = choices.request(&p, None);
        assert!(request.preset_id.is_none());
        let o = request.overrides.unwrap();
        assert_eq!(o.loudness_target, Some(-14.0));
        assert!(!o.loudness_off);
    }

    #[test]
    fn sound_only_and_gif_choices_ask_for_their_own_files() {
        let p = project(1920, 1080, 30.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.apply_preset(&p, &ExportPreset::by_id("audio_mp3").unwrap());
        assert_eq!(choices.kind, OutputKind::Audio);
        assert_eq!(choices.output_path(), PathBuf::from("/out/Clip.mp3"));
        choices.preset = None;
        let request = choices.request(&p, Some("nvenc_h264".into()));
        assert!(request.hardware.is_none(), "no GPU encoder for sound");
        assert!(request.include_audio);
        let o = request.overrides.unwrap();
        assert_eq!(o.container, Some(Container::Mp3));
        assert_eq!(o.audio_codec, Some(AudioCodec::Mp3));
        assert_eq!(o.width, None);

        choices.kind = OutputKind::Gif;
        choices.resolution = Resolution::P480;
        let o = choices.request(&p, None).overrides.unwrap();
        assert_eq!(o.container, Some(Container::Gif));
        assert_eq!((o.width, o.height), (Some(854), Some(480)));
        assert!(o.loudness_off);
    }

    #[test]
    fn switching_from_a_gif_to_a_video_brings_back_sound_and_a_video_rate() {
        let p = project(1080, 1920, 30.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.apply_preset(&p, &ExportPreset::by_id("gif").unwrap());
        assert_eq!((choices.fps, choices.audio), (15.0, false));
        choices.set_kind(OutputKind::Video);
        assert_eq!((choices.fps, choices.audio), (30.0, true));
        choices.resolution = Resolution::K4;
        choices.set_kind(OutputKind::Gif);
        assert_eq!(choices.resolution, Resolution::P480);
        assert_eq!(choices.fps, 15.0);
    }

    #[test]
    fn a_remembered_rate_the_list_lacks_lands_on_the_nearest_one() {
        let p = project(1080, 1920, 30.0);
        let mut memory = ExportChoices::for_project(&p, PathBuf::new()).memory(&p);
        memory.overrides.fps = Some(15.0);
        let restored = ExportChoices::opening(&p, PathBuf::new(), Some(&memory));
        assert_eq!(restored.fps, 23.976);
        let film = project(1920, 1080, 23.976);
        let mut choices = ExportChoices::for_project(&film, PathBuf::new());
        choices.apply_preset(&film, &ExportPreset::by_id("master_prores").unwrap());
        assert!((choices.fps - 23.976).abs() < 1e-9);
    }

    #[test]
    fn prores_is_always_a_quicktime_file_with_pcm_sound() {
        let p = project(1920, 1080, 30.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.codec = Codec::ProRes;
        assert_eq!(choices.output_path(), PathBuf::from("/out/Clip.mov"));
        let o = choices.request(&p, None).overrides.unwrap();
        assert_eq!(o.container, Some(Container::Mov));
        assert_eq!(o.audio_codec, Some(AudioCodec::Pcm));
    }

    #[test]
    fn remembered_choices_come_back_as_they_were() {
        let p = project(1080, 1920, 30.0);
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.resolution = Resolution::P720;
        choices.codec = Codec::Hevc;
        choices.bitrate = Bitrate::Higher;
        choices.fps = 60.0;
        choices.format = Format::Mov;
        choices.loudness_target = Some(-16.0);
        choices.audio_bitrate = 320_000;
        let memory = choices.memory(&p);
        let restored = ExportChoices::opening(&p, PathBuf::from("/elsewhere"), Some(&memory));
        assert_eq!(restored.resolution, Resolution::P720);
        assert_eq!(restored.codec, Codec::Hevc);
        assert_eq!(restored.bitrate, Bitrate::Higher);
        assert_eq!(restored.fps, 60.0);
        assert_eq!(restored.format, Format::Mov);
        assert_eq!(restored.loudness_target, Some(-16.0));
        assert_eq!(restored.audio_bitrate, 320_000);
        assert!(restored.preset.is_none());

        // A remembered preset comes back as the preset.
        let mut choices = ExportChoices::for_project(&p, PathBuf::from("/out"));
        choices.apply_preset(&p, &ExportPreset::by_id("youtube_shorts").unwrap());
        let restored = ExportChoices::opening(&p, PathBuf::from("/out"), Some(&choices.memory(&p)));
        assert_eq!(restored.preset.as_deref(), Some("youtube_shorts"));
        assert_eq!(restored.loudness_target, Some(-14.0));
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
