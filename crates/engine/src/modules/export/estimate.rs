//! How big an export will be, before it runs.
//!
//! Two answers, because one is instant and the other is right:
//!
//! - [`quick`] is arithmetic. A bitrate export is `bitrate × duration`, a
//!   sound-only file is the audio bitrate times the duration (exact for
//!   PCM), and a CRF export uses a calibrated bits-per-pixel table. The table
//!   is where it goes wrong: a CRF encode spends what the *picture* needs,
//!   and a static talking head and a confetti cannon at the same CRF differ
//!   tenfold. The old estimate was this table alone, and it said 24 MB for a
//!   3.4 MB file.
//! - [`sampled`] encodes a few short windows of the real timeline with the
//!   real settings and extrapolates from the bytes the encoder produced.
//!   Seconds of work, and within a few percent on the material that fooled
//!   the table.
//!
//! Both add the audio (from its bitrate: AAC and MP3 run at it, PCM is
//! exact) and the container's own overhead — the header and the per-packet
//! index MP4 and Matroska keep, which is 1–2 % of a typical file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;

use crate::modules::project::document::{Micros, MICROS_PER_SECOND};

use super::job::{run_export, ExportJob, ExportSettings};
use super::presets::{AudioCodec, Container, Fps, Quality, VideoCodec};
use super::{ExportError, Result};

/// How an estimate was made, so the dialog can say "about" or "measured".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateMethod {
    /// From a bitrate the encoder is held to.
    Bitrate,
    /// From the calibrated CRF table: a guess that depends on the picture.
    Table,
    /// From encoding samples of the timeline.
    Sampled,
    /// Uncompressed sound: the size is arithmetic.
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SizeEstimate {
    /// The whole file.
    pub bytes: u64,
    pub video_bytes: u64,
    pub audio_bytes: u64,
    /// Headers, index and per-packet framing.
    pub overhead_bytes: u64,
    pub method: EstimateMethod,
    /// Timeline seconds that were encoded to make this estimate; zero for an
    /// arithmetic one.
    pub sampled_seconds: f64,
}

impl SizeEstimate {
    /// Whether this is a guess that a sample could improve.
    pub fn is_rough(&self) -> bool {
        self.method == EstimateMethod::Table
    }
}

fn seconds(duration: Micros) -> f64 {
    duration.max(0) as f64 / MICROS_PER_SECOND as f64
}

/// Bits per pixel per frame at CRF 23, for typical edited footage.
///
/// Calibrated against x264 `medium` on mixed phone footage — talking heads,
/// b-roll, titles — at 1080p30, where CRF 23 lands around 4–5 Mbit/s. The
/// other codecs are the same quality target at their usual efficiency over
/// H.264.
fn bits_per_pixel_at_23(codec: VideoCodec) -> f64 {
    match codec {
        VideoCodec::H264 => 0.075,
        VideoCodec::H265 => 0.045,
        VideoCodec::Vp9 => 0.05,
        VideoCodec::Av1 => 0.038,
        // ProRes 422 HQ is a fixed data rate: 220 Mbit/s at 1080p29.97.
        VideoCodec::ProRes => 3.54,
        // Dithered GIF frames compress poorly; LZW over a Bayer pattern
        // keeps roughly a bit per pixel, less where frames repeat.
        VideoCodec::Gif => 0.9,
    }
}

/// The table's video bitrate for a CRF export, in bits per second.
fn table_bitrate(codec: VideoCodec, crf: u8, width: u32, height: u32, fps: Fps) -> f64 {
    let pixels = f64::from(width) * f64::from(height);
    let per_pixel = if codec.uses_quality() {
        // Six CRF points double or halve the size: x264's own rule.
        let base = bits_per_pixel_at_23(codec) * 2f64.powf((23.0 - f64::from(crf)) / 6.0);
        // Larger frames compress better per pixel: detail is spread over
        // more of them. Normalised to 1080p.
        base * (1920.0 * 1080.0 / pixels.max(1.0)).powf(0.25)
    } else {
        bits_per_pixel_at_23(codec)
    };
    per_pixel * pixels * fps.as_f64()
}

/// Bytes the audio stream will take.
fn audio_bytes(settings: &ExportSettings) -> u64 {
    let Some(spec) = &settings.audio else {
        return 0;
    };
    let bits = settings
        .preset
        .audio_codec
        .bits_per_second(spec.bitrate, spec.sample_rate);
    (bits as f64 * seconds(settings.duration) / 8.0) as u64
}

/// The container's own bytes: a header and an index entry or frame header per
/// packet.
fn overhead_bytes(settings: &ExportSettings) -> u64 {
    let duration = seconds(settings.duration);
    let video_packets = if settings.audio_only {
        0.0
    } else {
        settings.total_frames as f64
    };
    let audio_packets = match &settings.audio {
        Some(spec) => {
            let per_packet = match settings.preset.audio_codec {
                AudioCodec::Mp3 => 1152.0,
                AudioCodec::Opus => 960.0,
                _ => 1024.0,
            };
            duration * f64::from(spec.sample_rate) / per_packet
        }
        None => 0.0,
    };
    let (header, per_packet) = match settings.preset.container {
        // stsz, stts and stco entries, plus the moov header.
        Container::Mp4 | Container::Mov | Container::M4a => (2_000.0, 12.0),
        // A block header per packet and a cluster every few seconds.
        Container::Mkv | Container::Webm => (1_000.0, 14.0),
        // A graphic control extension and an image descriptor per frame.
        Container::Gif => (800.0, 20.0),
        // Every MP3 frame carries its own 4-byte header inside the packet.
        Container::Mp3 => (400.0, 0.0),
        Container::Wav => (44.0, 0.0),
    };
    (header + per_packet * (video_packets + audio_packets)) as u64
}

/// The instant estimate.
pub fn quick(settings: &ExportSettings) -> SizeEstimate {
    let duration = seconds(settings.duration);
    let audio = audio_bytes(settings);
    let overhead = overhead_bytes(settings);
    let (video, method) = if settings.audio_only {
        let method = if settings.preset.audio_codec == AudioCodec::Pcm {
            EstimateMethod::Exact
        } else {
            EstimateMethod::Bitrate
        };
        (0, method)
    } else {
        let codec = settings.preset.video_codec;
        let (width, height) = settings.size();
        match settings.video.quality {
            Quality::Bitrate(bits) if codec.uses_quality() => (
                (bits as f64 * duration / 8.0) as u64,
                EstimateMethod::Bitrate,
            ),
            quality => {
                let crf = match quality {
                    Quality::Crf(crf) => crf,
                    Quality::Bitrate(_) => 23,
                };
                let bits = table_bitrate(codec, crf, width, height, settings.fps());
                ((bits * duration / 8.0) as u64, EstimateMethod::Table)
            }
        }
    };
    SizeEstimate {
        bytes: video + audio + overhead,
        video_bytes: video,
        audio_bytes: audio,
        overhead_bytes: overhead,
        method,
        sampled_seconds: 0.0,
    }
}

/// The timeline windows a sample encodes: `(start, length)` relative to the
/// export's own start.
///
/// The whole export when it is short. Otherwise three windows at a sixth,
/// half and five sixths of the way through, so an intro, the middle and an
/// outro all count. Each window is a whole number of keyframe intervals
/// (two seconds; see `encoder::GOP_SECONDS`), because every window starts a
/// fresh encoder with a keyframe, and a window shorter than a GOP would
/// count keyframes the real file does not have.
pub fn sample_windows(duration: Micros) -> Vec<(Micros, Micros)> {
    const GOP: Micros = 2 * MICROS_PER_SECOND;
    if duration <= 8 * MICROS_PER_SECOND {
        return vec![(0, duration.max(0))];
    }
    let length = if duration >= 120 * MICROS_PER_SECOND {
        2 * GOP
    } else {
        GOP
    };
    [1, 3, 5]
        .into_iter()
        .map(|sixth| {
            let centre = duration * sixth / 6;
            let start = (centre - length / 2).clamp(0, duration - length);
            (start, length)
        })
        .collect()
}

/// Encode samples of the export and extrapolate.
///
/// `job` is the export as it would run. Each window is exported on its own,
/// video only, into a temporary file next to the cache, and only the encoded
/// video bytes are counted. Audio and overhead come from [`quick`]: AAC and
/// MP3 run at their bitrate, so sampling them would only cost time.
///
/// A sound-only export or a bitrate export has nothing to sample and gets
/// the quick answer.
pub fn sampled(job: &ExportJob) -> Result<SizeEstimate> {
    let settings = &job.settings;
    let quick = quick(settings);
    if quick.method != EstimateMethod::Table {
        return Ok(quick);
    }

    let dir = crate::modules::workspace::paths::cache_root().join("export-estimate");
    std::fs::create_dir_all(&dir).map_err(|source| ExportError::Open {
        path: dir.clone(),
        source,
    })?;

    let mut bytes = 0u64;
    let mut sampled: Micros = 0;
    for (index, (start, length)) in sample_windows(settings.duration).into_iter().enumerate() {
        if job.cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        let mut window = settings.clone();
        window.audio = None;
        window.loudness_target = None;
        window.range_start = settings.range_start + start;
        window.duration = length;
        window.total_frames = settings.fps().frame_count(length);
        window.output_path = dir.join(format!(
            "sample-{}-{index}.{}",
            uuid::Uuid::new_v4(),
            settings.preset.container.extension()
        ));
        let path = window.output_path.clone();
        let part = ExportJob {
            job_id: format!("{}-estimate-{index}", job.job_id),
            project: job.project.clone(),
            settings: window,
            compositor: Arc::clone(&job.compositor),
            sources: Arc::clone(&job.sources),
            audio: Arc::clone(&job.audio),
            cancel: Arc::clone(&job.cancel),
        };
        let outcome = run_export(&part, &());
        let _ = std::fs::remove_file(&path);
        let outcome = outcome?;
        if outcome.cancelled {
            return Err(ExportError::Cancelled);
        }
        bytes += outcome.writer.video_bytes;
        sampled += length;
    }
    if sampled <= 0 {
        return Ok(quick);
    }

    let video = (bytes as f64 * settings.duration as f64 / sampled as f64) as u64;
    Ok(SizeEstimate {
        bytes: video + quick.audio_bytes + quick.overhead_bytes,
        video_bytes: video,
        method: EstimateMethod::Sampled,
        sampled_seconds: seconds(sampled),
        ..quick
    })
}

/// A cancel flag for an estimate the caller does not intend to stop.
pub fn never_cancelled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::export::job::{resolve_settings, ExportOverrides, ExportRequest};
    use crate::modules::export::testing::project;

    fn settings(preset: &str, overrides: Option<ExportOverrides>, seconds: i64) -> ExportSettings {
        let project = project(1080, 1920, seconds * MICROS_PER_SECOND);
        resolve_settings(
            &project,
            &ExportRequest {
                output_path: "/x/out.mp4".into(),
                preset_id: Some(preset.into()),
                overrides,
                hardware: None,
                include_audio: true,
                range: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn a_bitrate_export_is_bitrate_times_duration_plus_audio_and_overhead() {
        // X: 8 Mbit/s video, 128 kbit/s audio, one minute.
        let estimate = quick(&settings("x_twitter", None, 60));
        assert_eq!(estimate.method, EstimateMethod::Bitrate);
        assert_eq!(estimate.video_bytes, 60_000_000);
        assert_eq!(estimate.audio_bytes, 960_000);
        // 1800 video and ~2813 audio packets at 12 bytes, plus the header.
        assert!(
            (50_000..70_000).contains(&estimate.overhead_bytes),
            "{}",
            estimate.overhead_bytes
        );
        assert_eq!(
            estimate.bytes,
            estimate.video_bytes + estimate.audio_bytes + estimate.overhead_bytes
        );
    }

    #[test]
    fn pcm_is_exact_and_mp3_runs_at_its_bitrate() {
        let wav = quick(&settings("audio_wav", None, 10));
        assert_eq!(wav.method, EstimateMethod::Exact);
        assert_eq!(wav.video_bytes, 0);
        // 48 kHz × 2 ch × 2 bytes × 10 s, and a 44-byte header.
        assert_eq!(wav.bytes, 1_920_000 + 44);
        let mp3 = quick(&settings("audio_mp3", None, 10));
        assert_eq!(mp3.audio_bytes, 400_000);
    }

    #[test]
    fn the_table_halves_the_size_every_six_crf_points() {
        let at = |crf| {
            quick(&settings(
                "tiktok",
                Some(ExportOverrides {
                    quality: Some(Quality::Crf(crf)),
                    ..Default::default()
                }),
                60,
            ))
            .video_bytes as f64
        };
        let ratio = at(20) / at(26);
        assert!((ratio - 2.0).abs() < 0.01, "{ratio}");
        assert!(quick(&settings("tiktok", None, 60)).is_rough());
    }

    #[test]
    fn prores_hq_at_1080p30_is_about_220_megabits() {
        let estimate = quick(&settings(
            "master_prores",
            Some(ExportOverrides {
                width: Some(1920),
                height: Some(1080),
                fps: Some(29.97),
                ..Default::default()
            }),
            1,
        ));
        let mbit = estimate.video_bytes as f64 * 8.0 / 1e6;
        assert!((200.0..240.0).contains(&mbit), "{mbit}");
    }

    #[test]
    fn short_exports_are_sampled_whole_and_long_ones_in_three_gop_windows() {
        assert_eq!(sample_windows(5_000_000), vec![(0, 5_000_000)]);
        let windows = sample_windows(60_000_000);
        assert_eq!(windows.len(), 3);
        for (start, length) in &windows {
            assert_eq!(*length, 2_000_000);
            assert!(*start >= 0 && start + length <= 60_000_000);
        }
        assert_eq!(windows[1].0, 29_000_000);
        // Long exports sample two GOPs a window.
        assert!(sample_windows(600_000_000)
            .iter()
            .all(|(_, length)| *length == 4_000_000));
    }
}
