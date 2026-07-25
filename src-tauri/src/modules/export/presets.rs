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
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265 / HEVC",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Opus,
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
            AudioCodec::None => None,
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
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::Webm => "webm",
        }
    }

    /// Whether this container will carry that pair of codecs.
    ///
    /// Muxers are stricter than people expect: WebM is VP9/AV1 plus Opus or
    /// Vorbis and nothing else, and while MP4 can technically hold Opus,
    /// enough players choke on it that offering the combination is a bug
    /// report waiting to happen.
    pub fn accepts(self, video: VideoCodec, audio: AudioCodec) -> bool {
        match self {
            Container::Mp4 | Container::Mov => {
                matches!(video, VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Av1)
                    && matches!(audio, AudioCodec::Aac | AudioCodec::None)
            }
            Container::Webm => {
                matches!(video, VideoCodec::Vp9 | VideoCodec::Av1)
                    && matches!(audio, AudioCodec::Opus | AudioCodec::None)
            }
            // Matroska takes everything, which is why it is the escape hatch.
            Container::Mkv => true,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPreset {
    /// Stable machine id. The frontend sends this back in an `ExportRequest`.
    pub id: String,
    pub label: String,
    /// One line the dialog can show under the label.
    pub description: String,
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
}

/// The id of the escape-hatch preset, which takes its resolution and rate from
/// the project rather than from a platform's requirements.
pub const CUSTOM_PRESET_ID: &str = "custom";

impl ExportPreset {
    /// Every built-in preset, in the order the dialog should list them.
    pub fn all() -> Vec<ExportPreset> {
        vec![
            youtube_1080p(),
            youtube_4k(),
            vertical_1080x1920(),
            instagram_square(),
            custom_template(),
        ]
    }

    pub fn by_id(id: &str) -> Option<ExportPreset> {
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

    /// The output file extension this preset implies.
    pub fn extension(&self) -> &'static str {
        self.container.extension()
    }

    /// Reject a preset that cannot be encoded, with prose the dialog can show.
    ///
    /// This runs on presets the frontend sends back and on the result of
    /// applying user overrides, so it is the one place that has to be complete.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("the export resolution must be larger than zero".into());
        }
        // 4:2:0 chroma is subsampled by two in both directions, so an odd
        // dimension has half a chroma sample at the edge. Every encoder here
        // either refuses or silently rounds; refusing loudly is better.
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(format!(
                "the export resolution {}x{} must be even in both dimensions",
                self.width, self.height
            ));
        }
        if self.width > 16_384 || self.height > 16_384 {
            return Err("the export resolution is beyond what any encoder accepts".into());
        }
        if self.fps.num == 0 || self.fps.den == 0 {
            return Err("the export frame rate must be larger than zero".into());
        }
        if self.fps.as_f64() > 300.0 {
            return Err("the export frame rate must be at most 300 frames per second".into());
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
        if self.audio_codec != AudioCodec::None {
            if !(32_000..=512_000).contains(&self.audio_bitrate) {
                return Err("the audio bitrate must be between 32 and 512 kbit/s".into());
            }
            if !matches!(self.sample_rate, 44_100 | 48_000 | 96_000) {
                return Err("the audio sample rate must be 44.1, 48 or 96 kHz".into());
            }
        }
        if !self.container.accepts(self.video_codec, self.audio_codec) {
            return Err(format!(
                "a .{} file cannot carry {} video",
                self.container.extension(),
                self.video_codec.label()
            ));
        }
        Ok(())
    }
}

/// Round a dimension down to even. See `validate`.
fn even(value: u32) -> u32 {
    value - (value % 2)
}

fn youtube_1080p() -> ExportPreset {
    ExportPreset {
        id: "youtube_1080p".into(),
        label: "YouTube 1080p".into(),
        description: "1920x1080, H.264, high quality upload".into(),
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
    }
}

fn youtube_4k() -> ExportPreset {
    ExportPreset {
        id: "youtube_4k".into(),
        label: "YouTube 4K".into(),
        description: "3840x2160, H.265, high quality upload".into(),
        width: 3840,
        height: 2160,
        fps: Fps::THIRTY,
        // HEVC rather than H.264 purely for size: H.264 needs 70–90 Mbit/s to
        // hold up at 2160p, HEVC gets there around 35–45, which is the range
        // YouTube recommends for a 4K upload. They accept both.
        video_codec: VideoCodec::H265,
        // CRF is offset between the two encoders — x265's 22 is roughly x264's
        // 20 — so this is the same quality target as the 1080p preset, not a
        // looser one.
        quality: Quality::Crf(22),
        audio_codec: AudioCodec::Aac,
        audio_bitrate: 384_000,
        sample_rate: 48_000,
        container: Container::Mp4,
    }
}

fn vertical_1080x1920() -> ExportPreset {
    ExportPreset {
        id: "vertical_1080x1920".into(),
        label: "TikTok / Reels / Shorts".into(),
        description: "1080x1920 vertical, H.264".into(),
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
    }
}

fn instagram_square() -> ExportPreset {
    ExportPreset {
        id: "instagram_square".into(),
        label: "Instagram square".into(),
        description: "1080x1080, H.264".into(),
        width: 1080,
        height: 1080,
        fps: Fps::THIRTY,
        video_codec: VideoCodec::H264,
        quality: Quality::Crf(21),
        audio_codec: AudioCodec::Aac,
        audio_bitrate: 192_000,
        sample_rate: 48_000,
        container: Container::Mp4,
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

    #[test]
    fn every_builtin_preset_is_encodable() {
        for preset in ExportPreset::all() {
            preset
                .validate()
                .unwrap_or_else(|e| panic!("preset {} is invalid: {e}", preset.id));
        }
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
