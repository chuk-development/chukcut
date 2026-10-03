//! Export presets, and the frame-rate arithmetic every other file here needs.
//!
//! A preset is data, not behaviour: a resolution, a frame rate, a codec, a
//! quality target and a container. The export dialog lists them, the job
//! resolves one into encoder settings, and nothing else in the module knows
//! that "YouTube 1080p" exists.
//!
//! ## Why frame rate is a rational and not an `f64`
//!
//! The document stores `fps` as `f64` because that is what the UI edits, but an
//! encoder needs an exact time base, and the three rates broadcast video
//! actually uses — 23.976, 29.97, 59.94 — are not representable as decimals.
//! They are 24000/1001, 30000/1001 and 60000/1001. Encoding a 29.97 fps
//! timeline with a 1/30 time base drifts one frame every thousand frames, which
//! is a lip-sync error of one frame after 33 seconds and half a second after an
//! hour. [`Fps`] keeps the fraction, and [`Fps::from_f64`] snaps the decimals
//! the UI produces back onto it.

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Micros, Project, MICROS_PER_SECOND};

// ---------------------------------------------------------------------------
// Frame rate
// ---------------------------------------------------------------------------

/// An exact frame rate, `num / den` frames per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fps {
    pub num: u32,
    pub den: u32,
}

impl Fps {
    /// 23.976 — film transferred to NTSC.
    pub const FILM_NTSC: Fps = Fps {
        num: 24_000,
        den: 1001,
    };
    pub const FILM: Fps = Fps { num: 24, den: 1 };
    /// 25 — PAL, and what most of Europe shoots.
    pub const PAL: Fps = Fps { num: 25, den: 1 };
    /// 29.97 — NTSC.
    pub const NTSC: Fps = Fps {
        num: 30_000,
        den: 1001,
    };
    pub const THIRTY: Fps = Fps { num: 30, den: 1 };
    pub const FIFTY: Fps = Fps { num: 50, den: 1 };
    /// 59.94 — NTSC at double rate.
    pub const NTSC_DOUBLE: Fps = Fps {
        num: 60_000,
        den: 1001,
    };
    pub const SIXTY: Fps = Fps { num: 60, den: 1 };

    /// Every rate we snap a decimal onto, in the order they are tried.
    const KNOWN: [Fps; 8] = [
        Fps::FILM_NTSC,
        Fps::FILM,
        Fps::PAL,
        Fps::NTSC,
        Fps::THIRTY,
        Fps::FIFTY,
        Fps::NTSC_DOUBLE,
        Fps::SIXTY,
    ];

    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// The rate the document stores, as an exact fraction.
    ///
    /// A value within 0.05% of a broadcast rate becomes that rate: `29.97` is
    /// how a UI writes 30000/1001, and reconstructing the fraction is the
    /// difference between an export that stays in sync and one that drifts.
    /// Anything else is approximated over a 1000-tick denominator, which is
    /// exact for the rates people type by hand (30, 48, 12.5).
    ///
    /// The tolerance has to be tight and the match has to be the *closest*
    /// candidate, not the first: 24 and 23.976 are only 0.1% apart, so a
    /// generous window would quietly turn a 24 fps export into a 23.976 one.
    pub fn from_f64(value: f64) -> Self {
        if !value.is_finite() || value <= 0.0 {
            return Fps::THIRTY;
        }
        let closest = Fps::KNOWN.into_iter().min_by(|a, b| {
            let da = (value - a.as_f64()).abs();
            let db = (value - b.as_f64()).abs();
            da.total_cmp(&db)
        });
        if let Some(candidate) = closest {
            if (value - candidate.as_f64()).abs() <= candidate.as_f64() * 0.0005 {
                return candidate;
            }
        }
        let num = (value * 1000.0).round().max(1.0) as u32;
        let den = 1000;
        let divisor = gcd(num, den);
        Fps::new(num / divisor, den / divisor)
    }

    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            return 0.0;
        }
        self.num as f64 / self.den as f64
    }

    /// The encoder time base: one tick is one frame, so a frame's PTS is its
    /// index. Returned as `(numerator, denominator)`, i.e. the reciprocal of
    /// the rate.
    pub fn time_base(self) -> (i32, i32) {
        (self.den as i32, self.num as i32)
    }

    /// The presentation time of frame `index` on the timeline.
    ///
    /// Computed from the index rather than accumulated per frame: adding
    /// 33_333 µs thirty times gives 999_990 µs, and an hour of that is two
    /// seconds of drift. Rounding rather than truncating keeps the error inside
    /// half a microsecond instead of letting it grow with the index.
    pub fn frame_time(self, index: u64) -> Micros {
        if self.num == 0 {
            return 0;
        }
        let numerator = index as i128 * self.den as i128 * MICROS_PER_SECOND as i128;
        let denominator = self.num as i128;
        ((numerator + denominator / 2) / denominator) as Micros
    }

    /// How many frames cover `duration`.
    ///
    /// Rounded up, because the last fraction of a frame is still visible: a
    /// 1.5 second timeline at 30 fps is 45 frames, and a 1.51 second one is 46
    /// — dropping that frame would truncate the export.
    pub fn frame_count(self, duration: Micros) -> u64 {
        if duration <= 0 || self.den == 0 {
            return 0;
        }
        let numerator = duration as i128 * self.num as i128;
        let denominator = self.den as i128 * MICROS_PER_SECOND as i128;
        ((numerator + denominator - 1) / denominator) as u64
    }
}

impl std::fmt::Display for Fps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{:.3}", self.as_f64())
        }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

// ---------------------------------------------------------------------------
// Codecs, containers, quality
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    Av1,
    /// Apple ProRes 422 HQ, 10-bit 4:2:2. An intermediate for a master file,
    /// not a delivery codec: about 220 Mbit/s at 1080p30.
    #[serde(rename = "prores")]
    ProRes,
    /// An animated GIF: 256 colours a frame, no sound.
    Gif,
}

impl VideoCodec {
    /// The software encoder FFmpeg knows this by.
    pub fn software_encoder(self) -> &'static str {
        match self {
            VideoCodec::H264 => "libx264",
            VideoCodec::H265 => "libx265",
            VideoCodec::Vp9 => "libvpx-vp9",
            // libaom is the reference encoder and is glacial — minutes per
            // frame at default settings. SVT-AV1 is the only AV1 encoder worth
            // offering in an editor. A build without it gets a clear "this
            // build of FFmpeg has no libsvtav1 encoder" rather than a slow
            // surprise.
            VideoCodec::Av1 => "libsvtav1",
            // prores_ks rather than the older `prores` encoder: it is the one
            // with real profile control, and its HQ output is what editing
            // applications expect a ProRes file to be.
            VideoCodec::ProRes => "prores_ks",
            VideoCodec::Gif => "gif",
        }
    }

    /// The CRF-equivalent range this codec accepts. Used for validation, and by
    /// the UI to draw a slider that cannot produce a rejected value.
    pub fn crf_range(self) -> (u8, u8) {
        match self {
            // x264 and x265 both take 0..51, though the same number is roughly
            // six points "quieter" on x265.
            VideoCodec::H264 | VideoCodec::H265 => (0, 51),
            VideoCodec::Vp9 => (0, 63),
            VideoCodec::Av1 => (0, 63),
            // No rate control at all; see `uses_quality`.
            VideoCodec::ProRes | VideoCodec::Gif => (0, 63),
        }
    }

    /// Whether a CRF or a bitrate means anything to this codec.
    ///
    /// ProRes picks its data rate from the profile and the frame size, and the
    /// GIF encoder is lossless over its palette. Both ignore a quality target,
    /// so validation must not reject one and the size estimate must not use
    /// one.
    pub fn uses_quality(self) -> bool {
        !matches!(self, VideoCodec::ProRes | VideoCodec::Gif)
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265 / HEVC",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
            VideoCodec::ProRes => "ProRes 422 HQ",
            VideoCodec::Gif => "GIF",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    /// Uncompressed 16-bit PCM: WAV files and masters.
    Pcm,
    /// Export with no audio stream at all.
    None,
}

impl AudioCodec {
    pub fn encoder_name(self) -> Option<&'static str> {
        match self {
            // The native FFmpeg AAC encoder, not libfdk_aac: fdk is not in a
            // stock LGPL build and a preset that only works on a custom FFmpeg
            // is a support burden. Native AAC has been fine above ~128 kbps for
            // years.
            AudioCodec::Aac => Some("aac"),
            AudioCodec::Opus => Some("libopus"),
            AudioCodec::Mp3 => Some("libmp3lame"),
            AudioCodec::Pcm => Some("pcm_s16le"),
            AudioCodec::None => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioCodec::Aac => "AAC",
            AudioCodec::Opus => "Opus",
            AudioCodec::Mp3 => "MP3",
            AudioCodec::Pcm => "PCM 16-bit",
            AudioCodec::None => "No audio",
        }
    }

    /// Bits per second this codec really produces for a stereo stream.
    ///
    /// The configured bitrate for the lossy codecs; PCM has no bitrate
    /// setting and is exactly `rate × 2 channels × 16 bits`.
    pub fn bits_per_second(self, configured: u32, sample_rate: u32) -> u64 {
        match self {
            AudioCodec::None => 0,
            AudioCodec::Pcm => u64::from(sample_rate) * 2 * 16,
            _ => u64::from(configured),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    Webm,
    Gif,
    /// AAC in an MPEG-4 audio file.
    M4a,
    Mp3,
    Wav,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::Webm => "webm",
            Container::Gif => "gif",
            Container::M4a => "m4a",
            Container::Mp3 => "mp3",
            Container::Wav => "wav",
        }
    }

    /// A file with sound and no picture.
    pub fn is_audio_only(self) -> bool {
        matches!(self, Container::M4a | Container::Mp3 | Container::Wav)
    }

    /// Whether this container will carry that pair of codecs.
    ///
    /// Muxers are stricter than people expect: WebM is VP9/AV1 plus Opus or
    /// Vorbis and nothing else, and while MP4 can technically hold Opus,
    /// enough players choke on it that offering the combination is a bug
    /// report waiting to happen. An audio-only container ignores `video`.
    pub fn accepts(self, video: VideoCodec, audio: AudioCodec) -> bool {
        match self {
            Container::Mp4 => {
                matches!(video, VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Av1)
                    && matches!(audio, AudioCodec::Aac | AudioCodec::None)
            }
            // QuickTime is also where ProRes and PCM masters live.
            Container::Mov => {
                matches!(
                    video,
                    VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Av1 | VideoCodec::ProRes
                ) && matches!(audio, AudioCodec::Aac | AudioCodec::Pcm | AudioCodec::None)
            }
            Container::Webm => {
                matches!(video, VideoCodec::Vp9 | VideoCodec::Av1)
                    && matches!(audio, AudioCodec::Opus | AudioCodec::None)
            }
            // Matroska takes everything except a GIF stream, which is why it
            // is the escape hatch.
            Container::Mkv => video != VideoCodec::Gif,
            Container::Gif => video == VideoCodec::Gif && audio == AudioCodec::None,
            Container::M4a => audio == AudioCodec::Aac,
            Container::Mp3 => audio == AudioCodec::Mp3,
            Container::Wav => audio == AudioCodec::Pcm,
        }
    }
}

/// How the encoder is told what quality to aim for.
///
/// Two genuinely different modes, not two spellings of one: CRF targets a
/// visual quality and lets the file size fall where it may, bitrate targets a
/// file size and lets quality fall where it may. Uploads want the first,
/// hardware playback limits and upload caps want the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Quality {
    /// Constant rate factor, in the codec's own scale.
    Crf(u8),
    /// Average bits per second.
    Bitrate(u64),
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Where a preset is listed in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetCategory {
    /// TikTok, Reels, Shorts, X.
    Social,
    Youtube,
    /// A high-quality file to keep or to hand on to another application.
    Master,
    /// Sound only.
    Audio,
    Gif,
    #[default]
    Custom,
}

impl PresetCategory {
    pub fn label(self) -> &'static str {
        match self {
            PresetCategory::Social => "Social",
            PresetCategory::Youtube => "YouTube",
            PresetCategory::Master => "Master",
            PresetCategory::Audio => "Audio only",
            PresetCategory::Gif => "GIF",
            PresetCategory::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    /// Stable machine id. The frontend sends this back in an `ExportRequest`.
    pub id: String,
    pub label: String,
    /// One line the dialog can show under the label.
    pub description: String,
    /// The output size. With `follow_canvas`, the box whose shorter side the
    /// output's shorter side gets; zero in both means "the canvas's own
    /// size". See [`ExportPreset::for_project`].
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
    pub video_codec: VideoCodec,
    pub quality: Quality,
    pub audio_codec: AudioCodec,
    /// Bits per second for the audio stream.
    pub audio_bitrate: u32,
    pub sample_rate: u32,
    pub container: Container,
    /// Bring the mix to this integrated loudness (LUFS), true peak at
    /// −1 dBTP. `None` keeps the mix as edited.
    #[serde(default)]
    pub loudness_target: Option<f32>,
    /// Take the canvas's aspect ratio rather than `width × height`'s.
    ///
    /// "1080p for TikTok" of a 16:9 canvas is 1920×1080, not a 1080×1920
    /// frame with the picture letterboxed into its middle: the preset names
    /// a resolution class and the composition keeps the shape it was framed
    /// in. When the shapes disagree, [`ExportPreset::warnings`] says so.
    #[serde(default)]
    pub follow_canvas: bool,
    /// Use the project's frame rate instead of `fps`. For targets that take
    /// every common rate (YouTube, a master), where converting 25 to 30 would
    /// only add judder.
    #[serde(default)]
    pub follow_project_fps: bool,
    #[serde(default)]
    pub category: PresetCategory,
    /// The shape the platform shows full-screen, as `(width, height)`.
    #[serde(default)]
    pub platform_aspect: Option<(u32, u32)>,
    /// The longest video the platform takes, in seconds.
    #[serde(default)]
    pub max_seconds: Option<u32>,
    /// Saved by the user rather than built in.
    #[serde(default)]
    pub user: bool,
}

/// The id of the escape-hatch preset, which takes its resolution and rate from
/// the project rather than from a platform's requirements.
pub const CUSTOM_PRESET_ID: &str = "custom";

/// Old preset ids that files and scripts may still name.
const ALIASES: [(&str, &str); 1] = [("vertical_1080x1920", "tiktok")];

impl ExportPreset {
    /// Every built-in preset, in the order the dialog should list them.
    pub fn all() -> Vec<ExportPreset> {
        vec![
            tiktok(),
            instagram_reels(),
            youtube_shorts(),
            instagram_square(),
            x_twitter(),
            youtube_1080p(),
            youtube_4k(),
            master_prores(),
            master_h264(),
            master_hevc(),
            audio_aac(),
            audio_mp3(),
            audio_wav(),
            gif(),
            custom_template(),
        ]
    }

    /// A built-in preset by id. User presets live in [`super::store`].
    pub fn by_id(id: &str) -> Option<ExportPreset> {
        let id = ALIASES
            .iter()
            .find(|(old, _)| *old == id)
            .map_or(id, |(_, new)| new);
        ExportPreset::all().into_iter().find(|p| p.id == id)
    }

    /// The custom preset filled in from a project: canvas size, project rate.
    ///
    /// "Custom" with nothing chosen should mean "what I am looking at", not an
    /// arbitrary default — an export that silently changes the aspect ratio of
    /// the composition the user framed is the worst possible surprise.
    pub fn custom_for(project: &Project) -> ExportPreset {
        ExportPreset {
            width: even(project.canvas.width),
            height: even(project.canvas.height),
            fps: Fps::from_f64(project.fps),
            ..custom_template()
        }
    }

    /// This preset made concrete for a project: the size fitted to the
    /// canvas and the frame rate taken from the project where the preset
    /// says so. Everything else is unchanged.
    pub fn for_project(&self, project: &Project) -> ExportPreset {
        let mut preset = self.clone();
        let (cw, ch) = (project.canvas.width, project.canvas.height);
        if preset.follow_canvas {
            if preset.width == 0 || preset.height == 0 {
                preset.width = even(cw);
                preset.height = even(ch);
            } else {
                let short = preset.width.min(preset.height);
                (preset.width, preset.height) = fit_short_side(short, cw, ch);
            }
        }
        if preset.follow_project_fps {
            preset.fps = Fps::from_f64(project.fps);
        }
        preset
    }

    /// What the user should know before exporting `project` with this
    /// preset: a canvas the platform will show with bars, a video longer
    /// than it accepts. Prose, one line each.
    pub fn warnings(&self, project: &Project) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((aw, ah)) = self.platform_aspect {
            let (cw, ch) = (project.canvas.width.max(1), project.canvas.height.max(1));
            let canvas = f64::from(cw) / f64::from(ch);
            let platform = f64::from(aw) / f64::from(ah.max(1));
            if (canvas / platform - 1.0).abs() > 0.02 {
                out.push(format!(
                    "{} shows {aw}:{ah} full screen; this canvas is {}, so the video plays with bars",
                    self.label,
                    aspect_label(cw, ch)
                ));
            }
        }
        if let Some(limit) = self.max_seconds {
            let seconds = project.duration() as f64 / MICROS_PER_SECOND as f64;
            if seconds > f64::from(limit) {
                out.push(format!(
                    "{} takes videos up to {}; this one is {}",
                    self.label,
                    clock(f64::from(limit)),
                    clock(seconds)
                ));
            }
        }
        out
    }

    /// The output file extension this preset implies.
    pub fn extension(&self) -> &'static str {
        self.container.extension()
    }

    /// Reject a preset that cannot be encoded, with prose the dialog can show.
    ///
    /// This runs on presets the frontend sends back and on the result of
    /// applying user overrides, so it is the one place that has to be complete.
    pub fn validate(&self) -> Result<(), String> {
        if self.fps.num == 0 || self.fps.den == 0 {
            return Err("the export frame rate must be larger than zero".into());
        }
        if self.fps.as_f64() > 300.0 {
            return Err("the export frame rate must be at most 300 frames per second".into());
        }
        if !self.container.is_audio_only() {
            self.validate_video()?;
        } else if self.audio_codec == AudioCodec::None {
            return Err(format!(
                "a .{} file is sound only, so it cannot be exported without audio",
                self.container.extension()
            ));
        }
        if !matches!(self.audio_codec, AudioCodec::None | AudioCodec::Pcm) {
            if !(32_000..=512_000).contains(&self.audio_bitrate) {
                return Err("the audio bitrate must be between 32 and 512 kbit/s".into());
            }
            // LAME's ceiling; asking for more fails at open with a message
            // nobody can act on.
            if self.audio_codec == AudioCodec::Mp3 && self.audio_bitrate > 320_000 {
                return Err("an MP3 bitrate can be at most 320 kbit/s".into());
            }
        }
        if self.audio_codec != AudioCodec::None
            && !matches!(self.sample_rate, 44_100 | 48_000 | 96_000)
        {
            return Err("the audio sample rate must be 44.1, 48 or 96 kHz".into());
        }
        if !self.container.accepts(self.video_codec, self.audio_codec) {
            return Err(if self.container.is_audio_only() {
                format!(
                    "a .{} file cannot carry {} audio",
                    self.container.extension(),
                    self.audio_codec.label()
                )
            } else if self.container == Container::Gif && self.audio_codec != AudioCodec::None {
                "a GIF has no sound; switch the audio off".to_string()
            } else {
                format!(
                    "a .{} file cannot carry {} video with {} audio",
                    self.container.extension(),
                    self.video_codec.label(),
                    self.audio_codec.label()
                )
            });
        }
        Ok(())
    }

    fn validate_video(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("the export resolution must be larger than zero".into());
        }
        // 4:2:0 chroma is subsampled by two in both directions, so an odd
        // dimension has half a chroma sample at the edge. Every encoder here
        // either refuses or silently rounds; refusing loudly is better.
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err(format!(
                "the export resolution {}x{} must be even in both dimensions",
                self.width, self.height
            ));
        }
        if self.width > 16_384 || self.height > 16_384 {
            return Err("the export resolution is beyond what any encoder accepts".into());
        }
        if !self.video_codec.uses_quality() {
            return Ok(());
        }
        match self.quality {
            Quality::Crf(crf) => {
                let (lo, hi) = self.video_codec.crf_range();
                if crf < lo || crf > hi {
                    return Err(format!(
                        "a quality of {crf} is outside the {}..{hi} range {} accepts",
                        lo,
                        self.video_codec.label()
                    ));
                }
            }
            Quality::Bitrate(bits) => {
                if bits < 100_000 {
                    return Err(
                        "a video bitrate below 100 kbit/s will not produce a usable file".into(),
                    );
                }
                if bits > 2_000_000_000 {
                    return Err("that video bitrate is implausibly high".into());
                }
            }
        }
        Ok(())
    }
}

/// Round a dimension down to even. See `validate`.
fn even(value: u32) -> u32 {
    value - (value % 2)
}

/// The size with `short` as its shorter side and the canvas's aspect ratio,
/// both sides even (4:2:0 needs that).
pub fn fit_short_side(short: u32, canvas_width: u32, canvas_height: u32) -> (u32, u32) {
    let (cw, ch) = (
        f64::from(canvas_width.max(2)),
        f64::from(canvas_height.max(2)),
    );
    let short = short.max(2);
    let long = |ratio: f64| (((f64::from(short) * ratio) / 2.0).round() as u32 * 2).max(2);
    let short = even(short);
    if cw >= ch {
        (long(cw / ch), short)
    } else {
        (short, long(ch / cw))
    }
}

/// "16:9", "9:16", "4:3", or "1.85:1" when no small ratio fits.
fn aspect_label(width: u32, height: u32) -> String {
    let divisor = gcd(width, height);
    let (w, h) = (width / divisor, height / divisor);
    if w <= 32 && h <= 32 {
        format!("{w}:{h}")
    } else {
        format!("{:.2}:1", f64::from(width) / f64::from(height.max(1)))
    }
}

/// "2:20", "10:00", "1:00:00".
fn clock(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The loudness every social platform normalises to. Mixing to it means
/// their normaliser leaves the sound alone instead of turning it down — or,
/// worse, up through a limiter.
const PLATFORM_LUFS: f32 = -14.0;

/// Podcast and spoken-word loudness (Apple's and most hosts' target).
const SPOKEN_LUFS: f32 = -16.0;

fn social(id: &str, label: &str, description: &str) -> ExportPreset {
    ExportPreset {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        width: 1080,
        height: 1920,
        // 30 rather than 60: all three platforms re-encode to 30 for most of
        // their delivery ladder, so exporting 60 doubles the upload and the
        // encode time to produce frames that get dropped.
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        // Slightly softer than the YouTube preset because these platforms cap
        // uploads (TikTok around 10 Mbit/s) and re-encode aggressively.
        quality: Quality::Crf(21),
        audio_codec: AudioCodec::Aac,
        // They deliver ~128 kbit/s AAC. Handing them 192 keeps the one lossy
        // generation we control from being the same bitrate as theirs, which
        // is where cascaded AAC starts to sound obviously bad.
        audio_bitrate: 192_000,
        sample_rate: 48_000,
        container: Container::Mp4,
        loudness_target: Some(PLATFORM_LUFS),
        follow_canvas: true,
        follow_project_fps: false,
        category: PresetCategory::Social,
        platform_aspect: Some((9, 16)),
        max_seconds: None,
        user: false,
    }
}

fn tiktok() -> ExportPreset {
    ExportPreset {
        // TikTok's uploader takes ten minutes.
        max_seconds: Some(600),
        quality: Quality::Crf(20),
        ..social("tiktok", "TikTok", "1080p vertical, H.264, −14 LUFS")
    }
}

fn instagram_reels() -> ExportPreset {
    ExportPreset {
        // Longer uploads are posted as an ordinary video, not as a Reel.
        max_seconds: Some(180),
        ..social(
            "instagram_reels",
            "Instagram Reels",
            "1080p vertical, H.264, −14 LUFS",
        )
    }
}

fn youtube_shorts() -> ExportPreset {
    ExportPreset {
        max_seconds: Some(180),
        quality: Quality::Crf(20),
        audio_bitrate: 256_000,
        ..social(
            "youtube_shorts",
            "YouTube Shorts",
            "1080p vertical, H.264, −14 LUFS",
        )
    }
}

fn instagram_square() -> ExportPreset {
    ExportPreset {
        width: 1080,
        height: 1080,
        // A fixed square: the feed shows 1:1 whatever the canvas, and a
        // non-square canvas gets a warning rather than a different shape.
        follow_canvas: false,
        platform_aspect: Some((1, 1)),
        max_seconds: None,
        ..social("instagram_square", "Instagram square", "1080x1080, H.264")
    }
}

fn x_twitter() -> ExportPreset {
    ExportPreset {
        width: 1920,
        height: 1080,
        // X re-encodes everything to a low bitrate and has been seen to
        // reject uploads far above its recommendation, so this one asks for
        // a size, not a quality.
        quality: Quality::Bitrate(8_000_000),
        audio_bitrate: 128_000,
        platform_aspect: None,
        // The limit for accounts without a subscription.
        max_seconds: Some(140),
        ..social(
            "x_twitter",
            "X / Twitter",
            "1080p, 8 Mbit/s H.264, −14 LUFS",
        )
    }
}

fn youtube_1080p() -> ExportPreset {
    ExportPreset {
        id: "youtube_1080p".into(),
        label: "YouTube 1080p".into(),
        description: "1080p, H.264, high quality upload".into(),
        width: 1920,
        height: 1080,
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        // YouTube re-encodes every upload, so the file we hand it only has to
        // be clean — anything we save by compressing harder is quality thrown
        // away before their encoder ever sees it. CRF 20 is visually
        // transparent on 1080p H.264 and lands around 8–12 Mbit/s, which is
        // what their own upload guidance asks for.
        quality: Quality::Crf(20),
        audio_codec: AudioCodec::Aac,
        // 384 kbit/s stereo is YouTube's documented recommendation. Their
        // encoder is going to make this ~128 kbit/s Opus regardless; giving it
        // a clean source costs a megabyte a minute.
        audio_bitrate: 384_000,
        sample_rate: 48_000,
        container: Container::Mp4,
        loudness_target: Some(PLATFORM_LUFS),
        follow_canvas: true,
        // YouTube plays 24, 25, 30, 50 and 60 natively; converting the
        // project's rate would only add judder.
        follow_project_fps: true,
        category: PresetCategory::Youtube,
        platform_aspect: Some((16, 9)),
        max_seconds: None,
        user: false,
    }
}

fn youtube_4k() -> ExportPreset {
    ExportPreset {
        id: "youtube_4k".into(),
        label: "YouTube 4K".into(),
        description: "2160p, H.265, high quality upload".into(),
        width: 3840,
        height: 2160,
        // HEVC rather than H.264 purely for size: H.264 needs 70–90 Mbit/s to
        // hold up at 2160p, HEVC gets there around 35–45, which is the range
        // YouTube recommends for a 4K upload. They accept both.
        video_codec: VideoCodec::H265,
        // CRF is offset between the two encoders — x265's 22 is roughly x264's
        // 20 — so this is the same quality target as the 1080p preset, not a
        // looser one.
        quality: Quality::Crf(22),
        ..youtube_1080p()
    }
}

fn master(id: &str, label: &str, description: &str) -> ExportPreset {
    ExportPreset {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        // The canvas's own size and the project's own rate: a master is the
        // composition as it is, before any platform's demands.
        width: 0,
        height: 0,
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        quality: Quality::Crf(14),
        audio_codec: AudioCodec::Aac,
        audio_bitrate: 320_000,
        sample_rate: 48_000,
        container: Container::Mp4,
        // A master keeps the mix as it was edited; loudness is a delivery
        // decision.
        loudness_target: None,
        follow_canvas: true,
        follow_project_fps: true,
        category: PresetCategory::Master,
        platform_aspect: None,
        max_seconds: None,
        user: false,
    }
}

fn master_prores() -> ExportPreset {
    ExportPreset {
        video_codec: VideoCodec::ProRes,
        audio_codec: AudioCodec::Pcm,
        container: Container::Mov,
        ..master(
            "master_prores",
            "Master · ProRes 422 HQ",
            "Canvas size, ProRes 422 HQ 10-bit, PCM sound, .mov",
        )
    }
}

fn master_h264() -> ExportPreset {
    master(
        "master_h264",
        "Master · H.264",
        "Canvas size, H.264 at CRF 14, AAC 320 kbit/s",
    )
}

fn master_hevc() -> ExportPreset {
    ExportPreset {
        video_codec: VideoCodec::H265,
        quality: Quality::Crf(16),
        ..master(
            "master_hevc",
            "Master · HEVC",
            "Canvas size, H.265 at CRF 16, AAC 320 kbit/s",
        )
    }
}

fn audio_only(
    id: &str,
    label: &str,
    description: &str,
    codec: AudioCodec,
    container: Container,
) -> ExportPreset {
    ExportPreset {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        width: 0,
        height: 0,
        // Not a picture rate: the export reports progress in steps of one
        // frame of this rate, and nothing else reads it.
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        quality: Quality::Crf(20),
        audio_codec: codec,
        audio_bitrate: 256_000,
        sample_rate: 48_000,
        container,
        loudness_target: Some(SPOKEN_LUFS),
        follow_canvas: false,
        follow_project_fps: false,
        category: PresetCategory::Audio,
        platform_aspect: None,
        max_seconds: None,
        user: false,
    }
}

fn audio_aac() -> ExportPreset {
    audio_only(
        "audio_aac",
        "Audio · AAC",
        "Sound only, AAC 256 kbit/s .m4a, −16 LUFS",
        AudioCodec::Aac,
        Container::M4a,
    )
}

fn audio_mp3() -> ExportPreset {
    ExportPreset {
        audio_bitrate: 320_000,
        ..audio_only(
            "audio_mp3",
            "Audio · MP3",
            "Sound only, MP3 320 kbit/s, −16 LUFS",
            AudioCodec::Mp3,
            Container::Mp3,
        )
    }
}

fn audio_wav() -> ExportPreset {
    ExportPreset {
        // WAV is for further work in another application, so the mix stays
        // as edited.
        loudness_target: None,
        ..audio_only(
            "audio_wav",
            "Audio · WAV",
            "Sound only, 16-bit 48 kHz PCM .wav",
            AudioCodec::Pcm,
            Container::Wav,
        )
    }
}

fn gif() -> ExportPreset {
    ExportPreset {
        id: "gif".into(),
        label: "GIF".into(),
        description: "480p, 15 fps animated GIF, no sound".into(),
        width: 480,
        height: 480,
        // 15 rather than the project's rate: a GIF's size grows with every
        // frame and nobody watches one for smooth motion.
        fps: Fps::new(15, 1),
        video_codec: VideoCodec::Gif,
        quality: Quality::Crf(20),
        audio_codec: AudioCodec::None,
        audio_bitrate: 128_000,
        sample_rate: 48_000,
        container: Container::Gif,
        loudness_target: None,
        follow_canvas: true,
        follow_project_fps: false,
        category: PresetCategory::Gif,
        platform_aspect: None,
        max_seconds: None,
        user: false,
    }
}

fn custom_template() -> ExportPreset {
    ExportPreset {
        id: CUSTOM_PRESET_ID.into(),
        label: "Custom".into(),
        description: "Project canvas and frame rate".into(),
        width: 1080,
        height: 1920,
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        quality: Quality::Crf(20),
        audio_codec: AudioCodec::Aac,
        audio_bitrate: 192_000,
        sample_rate: 48_000,
        container: Container::Mp4,
        loudness_target: None,
        follow_canvas: false,
        follow_project_fps: false,
        category: PresetCategory::Custom,
        platform_aspect: None,
        max_seconds: None,
        user: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_rates_snap_back_onto_their_fractions() {
        assert_eq!(Fps::from_f64(23.976), Fps::FILM_NTSC);
        assert_eq!(Fps::from_f64(23.98), Fps::FILM_NTSC);
        assert_eq!(Fps::from_f64(24.0), Fps::FILM);
        assert_eq!(Fps::from_f64(25.0), Fps::PAL);
        assert_eq!(Fps::from_f64(29.97), Fps::NTSC);
        assert_eq!(Fps::from_f64(29.970029), Fps::NTSC);
        assert_eq!(Fps::from_f64(30.0), Fps::THIRTY);
        assert_eq!(Fps::from_f64(59.94), Fps::NTSC_DOUBLE);
        assert_eq!(Fps::from_f64(60.0), Fps::SIXTY);
    }

    #[test]
    fn unknown_rates_become_an_exact_fraction() {
        assert_eq!(Fps::from_f64(12.5).as_f64(), 12.5);
        assert_eq!(Fps::from_f64(48.0), Fps::new(48, 1));
        // Nonsense must not produce a division by zero downstream.
        assert_eq!(Fps::from_f64(0.0), Fps::THIRTY);
        assert_eq!(Fps::from_f64(f64::NAN), Fps::THIRTY);
        assert_eq!(Fps::from_f64(-5.0), Fps::THIRTY);
    }

    #[test]
    fn time_base_is_the_reciprocal_of_the_rate() {
        assert_eq!(Fps::THIRTY.time_base(), (1, 30));
        assert_eq!(Fps::NTSC.time_base(), (1001, 30_000));
        assert_eq!(Fps::FILM_NTSC.time_base(), (1001, 24_000));
        assert_eq!(Fps::PAL.time_base(), (1, 25));
    }

    #[test]
    fn frame_times_are_computed_from_the_index_not_accumulated() {
        // 25 fps divides microseconds exactly.
        assert_eq!(Fps::PAL.frame_time(0), 0);
        assert_eq!(Fps::PAL.frame_time(1), 40_000);
        assert_eq!(Fps::PAL.frame_time(25), 1_000_000);

        // 30 fps does not: one frame is 33333.33… µs. An hour in, the index
        // form is still within a microsecond of the true time; accumulating
        // 33_333 would be 120 ms early.
        assert_eq!(Fps::THIRTY.frame_time(1), 33_333);
        assert_eq!(Fps::THIRTY.frame_time(3), 100_000);
        assert_eq!(Fps::THIRTY.frame_time(108_000), 3_600_000_000);
    }

    #[test]
    fn fractional_rates_keep_their_1001_denominator() {
        // 29.97: one frame is 1001/30000 s = 33366.67 µs.
        assert_eq!(Fps::NTSC.frame_time(1), 33_367);
        assert_eq!(Fps::NTSC.frame_time(30_000), 1_001_000_000);
        // An hour of wall clock is 107_892 frames, not the 108_000 an even 30
        // would give. Rounding the rate to 30 would put the last frame of an
        // hour-long export 3.6 seconds out.
        assert_eq!(Fps::NTSC.frame_time(107_892), 3_599_996_400);
        assert_eq!(Fps::THIRTY.frame_time(107_892), 3_596_400_000);

        // 23.976: one frame is 1001/24000 s = 41708.33 µs.
        assert_eq!(Fps::FILM_NTSC.frame_time(1), 41_708);
        assert_eq!(Fps::FILM_NTSC.frame_time(24_000), 1_001_000_000);

        // 59.94.
        assert_eq!(Fps::NTSC_DOUBLE.frame_time(1), 16_683);
        assert_eq!(Fps::NTSC_DOUBLE.frame_time(60_000), 1_001_000_000);
    }

    #[test]
    fn frame_counts_round_up_so_the_tail_is_not_truncated() {
        assert_eq!(Fps::THIRTY.frame_count(1_000_000), 30);
        // A frame and a half of content is two frames of output.
        assert_eq!(Fps::THIRTY.frame_count(50_000), 2);
        assert_eq!(Fps::THIRTY.frame_count(33_333), 1);
        assert_eq!(Fps::THIRTY.frame_count(33_334), 2);

        // Fractional rates: one second of 29.97 is 30 frames, and the last one
        // starts at 967_633 µs — inside the second, so it counts.
        assert_eq!(Fps::NTSC.frame_count(1_000_000), 30);
        assert_eq!(Fps::NTSC.frame_count(1_001_000_000), 30_000);
        assert_eq!(Fps::FILM_NTSC.frame_count(1_001_000_000), 24_000);
        assert_eq!(Fps::NTSC_DOUBLE.frame_count(1_001_000_000), 60_000);

        assert_eq!(Fps::THIRTY.frame_count(0), 0);
        assert_eq!(Fps::THIRTY.frame_count(-1), 0);
    }

    #[test]
    fn every_frame_of_a_long_export_stays_inside_the_timeline() {
        // Three hours at 59.94 is where i64 microseconds times a numerator
        // overflows if the arithmetic is not widened.
        let fps = Fps::NTSC_DOUBLE;
        let duration = 3 * 3600 * MICROS_PER_SECOND;
        let count = fps.frame_count(duration);
        assert!(fps.frame_time(count - 1) < duration);
        assert!(fps.frame_time(count) >= duration);
    }

    fn canvas(width: u32, height: u32, fps: f64) -> Project {
        use crate::modules::project::document::CanvasConfig;
        Project::new(
            "p",
            CanvasConfig {
                width,
                height,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            fps,
        )
    }

    #[test]
    fn every_builtin_preset_is_encodable_on_every_canvas_shape() {
        for project in [
            canvas(1080, 1920, 30.0),
            canvas(1920, 1080, 25.0),
            canvas(1081, 1081, 29.97),
        ] {
            for preset in ExportPreset::all() {
                preset
                    .for_project(&project)
                    .validate()
                    .unwrap_or_else(|e| panic!("preset {} is invalid: {e}", preset.id));
            }
        }
    }

    #[test]
    fn the_platform_presets_exist() {
        for id in [
            "tiktok",
            "instagram_reels",
            "youtube_shorts",
            "youtube_1080p",
            "youtube_4k",
            "x_twitter",
            "master_prores",
            "master_h264",
            "audio_aac",
            "audio_mp3",
            "audio_wav",
            "gif",
        ] {
            assert!(ExportPreset::by_id(id).is_some(), "{id} is missing");
        }
        // The old id still resolves, to its successor.
        assert_eq!(
            ExportPreset::by_id("vertical_1080x1920").map(|p| p.id),
            Some("tiktok".to_string())
        );
    }

    #[test]
    fn a_preset_takes_the_canvas_shape_with_its_own_short_side() {
        let tiktok = ExportPreset::by_id("tiktok").unwrap();
        let vertical = tiktok.for_project(&canvas(1080, 1920, 30.0));
        assert_eq!((vertical.width, vertical.height), (1080, 1920));
        // A 16:9 composition stays 16:9 — the class "1080p" is kept, not
        // the shape.
        let wide = tiktok.for_project(&canvas(1920, 1080, 30.0));
        assert_eq!((wide.width, wide.height), (1920, 1080));
        // A 4:5 canvas at 4K short side.
        let k4 = ExportPreset::by_id("youtube_4k").unwrap();
        let p = k4.for_project(&canvas(1080, 1350, 30.0));
        assert_eq!((p.width, p.height), (2160, 2700));
        // The GIF is small whatever the canvas.
        let gif = ExportPreset::by_id("gif").unwrap();
        let p = gif.for_project(&canvas(1920, 1080, 30.0));
        assert_eq!((p.width, p.height), (854, 480));
    }

    #[test]
    fn a_master_is_the_canvas_at_the_project_rate() {
        let master = ExportPreset::by_id("master_prores").unwrap();
        let p = master.for_project(&canvas(1440, 1080, 23.976));
        assert_eq!((p.width, p.height), (1440, 1080));
        assert_eq!(p.fps, Fps::FILM_NTSC);
        assert_eq!(p.container, Container::Mov);
        // Social presets keep their own 30.
        let reels = ExportPreset::by_id("instagram_reels").unwrap();
        assert_eq!(
            reels.for_project(&canvas(1080, 1920, 25.0)).fps,
            Fps::THIRTY
        );
        // YouTube follows the project.
        let yt = ExportPreset::by_id("youtube_1080p").unwrap();
        assert_eq!(yt.for_project(&canvas(1920, 1080, 25.0)).fps, Fps::PAL);
    }

    #[test]
    fn platform_presets_carry_a_loudness_target_and_masters_do_not() {
        for id in [
            "tiktok",
            "instagram_reels",
            "youtube_shorts",
            "x_twitter",
            "youtube_1080p",
        ] {
            assert_eq!(
                ExportPreset::by_id(id).unwrap().loudness_target,
                Some(-14.0),
                "{id}"
            );
        }
        for id in ["master_prores", "master_h264", "audio_wav", "gif", "custom"] {
            assert_eq!(
                ExportPreset::by_id(id).unwrap().loudness_target,
                None,
                "{id}"
            );
        }
    }

    #[test]
    fn warnings_name_a_wrong_shape_and_an_overlong_video() {
        let tiktok = ExportPreset::by_id("tiktok").unwrap();
        assert!(tiktok.warnings(&canvas(1080, 1920, 30.0)).is_empty());
        let wide = tiktok.warnings(&canvas(1920, 1080, 30.0));
        assert_eq!(wide.len(), 1);
        assert!(
            wide[0].contains("9:16") && wide[0].contains("16:9"),
            "{wide:?}"
        );

        // An empty timeline is never too long.
        let x = ExportPreset::by_id("x_twitter").unwrap();
        assert!(x.warnings(&canvas(1920, 1080, 30.0)).is_empty());
    }

    #[test]
    fn audio_only_files_need_sound_and_ignore_the_picture() {
        let mut wav = ExportPreset::by_id("audio_wav").unwrap();
        assert!(
            wav.validate().is_ok(),
            "zero size is fine without a picture"
        );
        wav.audio_codec = AudioCodec::None;
        assert!(wav.validate().unwrap_err().contains("sound only"));
        let mut mp3 = ExportPreset::by_id("audio_mp3").unwrap();
        mp3.audio_codec = AudioCodec::Aac;
        assert!(mp3.validate().is_err());
        mp3.audio_codec = AudioCodec::Mp3;
        mp3.audio_bitrate = 384_000;
        assert!(mp3.validate().unwrap_err().contains("320"));
    }

    #[test]
    fn a_gif_has_no_sound_and_prores_needs_a_quicktime_file() {
        let mut gif = ExportPreset::by_id("gif")
            .unwrap()
            .for_project(&canvas(480, 480, 30.0));
        gif.audio_codec = AudioCodec::Aac;
        assert!(gif.validate().unwrap_err().contains("no sound"));
        let mut prores = ExportPreset::by_id("master_prores")
            .unwrap()
            .for_project(&canvas(1920, 1080, 30.0));
        // No quality target applies, so an out-of-range one is not an error.
        prores.quality = Quality::Crf(99);
        assert!(prores.validate().is_ok());
        prores.container = Container::Mp4;
        assert!(prores.validate().is_err());
    }

    #[test]
    fn short_sides_fit_any_canvas_with_even_sides() {
        assert_eq!(fit_short_side(1080, 1080, 1920), (1080, 1920));
        assert_eq!(fit_short_side(720, 1920, 1080), (1280, 720));
        let (w, h) = fit_short_side(480, 1001, 1000);
        assert_eq!((w % 2, h % 2), (0, 0));
        assert_eq!(fit_short_side(1080, 2560, 1080), (2560, 1080));
    }

    #[test]
    fn a_preset_survives_json() {
        for preset in ExportPreset::all() {
            let text = serde_json::to_string(&preset).unwrap();
            let back: ExportPreset = serde_json::from_str(&text).unwrap();
            assert_eq!(back, preset);
        }
        // A preset written before the new fields existed still reads.
        let old = r#"{"id":"x","label":"X","description":"","width":1280,"height":720,
            "fps":{"num":30,"den":1},"video_codec":"h264","quality":{"kind":"crf","value":20},
            "audio_codec":"aac","audio_bitrate":192000,"sample_rate":48000,"container":"mp4"}"#;
        let preset: ExportPreset = serde_json::from_str(old).unwrap();
        assert!(!preset.follow_canvas && preset.loudness_target.is_none());
    }

    #[test]
    fn preset_ids_are_unique() {
        let mut ids: Vec<String> = ExportPreset::all().into_iter().map(|p| p.id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn odd_resolutions_are_rejected() {
        let mut preset = youtube_1080p();
        preset.height = 1081;
        assert!(preset.validate().unwrap_err().contains("even"));
    }

    #[test]
    fn quality_outside_the_codec_range_is_rejected() {
        let mut preset = youtube_1080p();
        preset.quality = Quality::Crf(60);
        assert!(preset.validate().is_err());

        // The same number is legal for VP9, whose scale runs to 63.
        preset.video_codec = VideoCodec::Vp9;
        preset.container = Container::Mkv;
        assert!(preset.validate().is_ok());
    }

    #[test]
    fn containers_reject_codecs_they_cannot_carry() {
        let mut preset = youtube_1080p();
        preset.container = Container::Webm;
        assert!(preset.validate().is_err());

        preset.video_codec = VideoCodec::Vp9;
        preset.audio_codec = AudioCodec::Opus;
        assert!(preset.validate().is_ok());
    }

    #[test]
    fn the_custom_preset_follows_the_project() {
        use crate::modules::project::document::CanvasConfig;

        let project = Project::new(
            "p",
            CanvasConfig {
                width: 1440,
                height: 1080,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            29.97,
        );
        let preset = ExportPreset::custom_for(&project);
        assert_eq!((preset.width, preset.height), (1440, 1080));
        assert_eq!(preset.fps, Fps::NTSC);
        assert!(preset.validate().is_ok());
    }

    #[test]
    fn an_odd_canvas_is_rounded_rather_than_rejected() {
        use crate::modules::project::document::CanvasConfig;

        let project = Project::new(
            "p",
            CanvasConfig {
                width: 1081,
                height: 607,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        let preset = ExportPreset::custom_for(&project);
        assert_eq!((preset.width, preset.height), (1080, 606));
        assert!(preset.validate().is_ok());
    }
}
