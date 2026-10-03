//! The export job: settings resolution, the frame loop, progress, cancellation.
//!
//! ## Why the project is a snapshot
//!
//! An export of a five minute timeline is minutes of work. Holding the document
//! lock for that would freeze every edit, every undo and every preview frame
//! until it finished — the app would look hung. So the command clones the
//! `Project` once, drops the lock, and the job renders from its own copy. The
//! user keeps editing; the export produces the cut they asked for rather than
//! whatever the timeline drifted into halfway through.
//!
//! ## Why cancellation is an atomic flag and not a channel
//!
//! Because the check has to be *cheap enough to do every frame*. The flag is
//! read once per frame between the render and the encode, which is the only
//! place the job is ever idle enough to stop cleanly, and a relaxed load of an
//! `AtomicBool` costs nothing next to compositing a 4K frame. Closing the
//! dialog sets it; within one frame the job unwinds, deletes the partial file
//! and reports `Cancelled`.
//!
//! ## Why the frame walk is its own function
//!
//! [`walk_frames`] takes a closure, so the loop's semantics — the frame times
//! it produces, when it checks the flag, what it does when the closure fails —
//! are testable without a GPU, an encoder or a media file. The real job passes
//! a closure that renders and encodes; the tests pass one that counts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Micros, Project};
use crate::modules::render::{Compositor, SourceProvider};

use super::audio::{self, AudioSource, SilentAudioSource};
use super::encoder::{AudioStreamSpec, MediaWriter, VideoStreamSpec};
use super::hwaccel::{self, HwAccel, HwEncoder};
use super::presets::{AudioCodec, ExportPreset, Fps, Quality, VideoCodec, CUSTOM_PRESET_ID};
use super::{ExportError, Result};

/// How often progress goes over the channel while encoding.
///
/// Five updates a second is smooth to a human and cheap to serialise. Sending
/// per frame would put 60 messages a second through the IPC bridge to move a
/// progress bar by a pixel.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// Request and settings
// ---------------------------------------------------------------------------

/// What the frontend asks for.
#[derive(Debug, Clone, Deserialize)]
pub struct ExportRequest {
    /// Where to write. The extension is corrected to match the container.
    pub output_path: String,
    /// A preset id from `export_presets`. Absent or `"custom"` means "the
    /// project's own canvas and frame rate".
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub overrides: Option<ExportOverrides>,
    /// A hardware encoder id from `export_presets`. Absent means software,
    /// which is the deliberate default; see `hwaccel`.
    #[serde(default)]
    pub hardware: Option<String>,
    #[serde(default = "yes")]
    pub include_audio: bool,
    /// Export only `(start, end)` of the timeline, in microseconds. Clamped to
    /// the project, and the file's timestamps are rebased so it starts at zero
    /// — a range export is a complete video of that slice, not a fragment.
    /// Absent means the whole project.
    #[serde(default)]
    pub range: Option<(Micros, Micros)>,
}

fn yes() -> bool {
    true
}

/// Per-field changes on top of a preset. Everything optional; anything absent
/// keeps the preset's value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExportOverrides {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<f64>,
    #[serde(default)]
    pub video_codec: Option<VideoCodec>,
    #[serde(default)]
    pub quality: Option<Quality>,
    #[serde(default)]
    pub audio_codec: Option<AudioCodec>,
    #[serde(default)]
    pub audio_bitrate: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub container: Option<super::presets::Container>,
    /// Bring the mix to this integrated loudness (LUFS) with true peaks at
    /// −1 dBTP. Absent leaves the mix at the level it was edited at.
    #[serde(default)]
    pub loudness_target: Option<f32>,
}

/// A resolved, validated export. Everything the job needs and nothing it has to
/// decide.
#[derive(Debug, Clone)]
pub struct ExportSettings {
    pub output_path: PathBuf,
    pub preset: ExportPreset,
    pub video: VideoStreamSpec,
    pub audio: Option<AudioStreamSpec>,
    /// Frames the walk will produce, from the exported duration at the output
    /// rate.
    pub total_frames: u64,
    /// The exported duration: the whole project, or the requested range of it.
    pub duration: Micros,
    /// Where on the timeline the export begins. `0` for a whole-project
    /// export; the range's clamped start otherwise. Frame `i` of the file
    /// shows the timeline at `range_start + fps.frame_time(i)`, while its PTS
    /// stays `i` — which is the rebase to zero.
    pub range_start: Micros,
    /// Integrated loudness to normalise the mix to, in LUFS; see
    /// `modules::loudness`.
    pub loudness_target: Option<f32>,
}

impl ExportSettings {
    pub fn size(&self) -> (u32, u32) {
        (self.video.width, self.video.height)
    }

    pub fn fps(&self) -> Fps {
        self.video.fps
    }
}

/// What the export dialog needs to draw itself, in one round trip.
#[derive(Debug, Clone, Serialize)]
pub struct ExportOptions {
    pub presets: Vec<ExportPreset>,
    /// Hardware encoders this machine has. Empty is normal.
    pub hardware: Vec<HwEncoder>,
    pub default_preset_id: String,
}

pub fn export_options() -> ExportOptions {
    ExportOptions {
        presets: ExportPreset::all(),
        hardware: hwaccel::detect(),
        default_preset_id: CUSTOM_PRESET_ID.to_string(),
    }
}

/// Turn a request plus the project into settings, or into prose explaining why
/// it cannot be done.
pub fn resolve_settings(project: &Project, request: &ExportRequest) -> Result<ExportSettings> {
    let mut preset = match request.preset_id.as_deref() {
        None | Some(CUSTOM_PRESET_ID) => ExportPreset::custom_for(project),
        Some(id) => ExportPreset::by_id(id).ok_or_else(|| {
            ExportError::Settings(format!("there is no export preset called {id}"))
        })?,
    };

    if let Some(overrides) = &request.overrides {
        if let Some(width) = overrides.width {
            preset.width = width;
        }
        if let Some(height) = overrides.height {
            preset.height = height;
        }
        if let Some(fps) = overrides.fps {
            preset.fps = Fps::from_f64(fps);
        }
        if let Some(codec) = overrides.video_codec {
            preset.video_codec = codec;
        }
        if let Some(quality) = overrides.quality {
            preset.quality = quality;
        }
        if let Some(codec) = overrides.audio_codec {
            preset.audio_codec = codec;
        }
        if let Some(bitrate) = overrides.audio_bitrate {
            preset.audio_bitrate = bitrate;
        }
        if let Some(rate) = overrides.sample_rate {
            preset.sample_rate = rate;
        }
        if let Some(container) = overrides.container {
            preset.container = container;
        }
    }

    // Choosing "H.265 (NVENC)" is choosing a codec as much as an encoder, so
    // the hardware selection wins over the preset's codec rather than
    // conflicting with it.
    let hardware = match request.hardware.as_deref() {
        None => None,
        Some(id) => {
            let found = hwaccel::find(id).ok_or_else(|| {
                ExportError::Settings(format!(
                    "the hardware encoder {id} is not available on this machine"
                ))
            })?;
            if !found.usable {
                return Err(ExportError::Settings(found.note.clone().unwrap_or_else(
                    || format!("the hardware encoder {id} cannot be used yet"),
                )));
            }
            preset.video_codec = found.codec;
            Some(found)
        }
    };

    // Opus only encodes at 48/24/16/12/8 kHz, and 48 is the only one worth
    // exporting at. Correcting it silently is friendlier than rejecting a
    // combination the dialog let the user pick.
    if preset.audio_codec == AudioCodec::Opus && preset.sample_rate != 48_000 {
        preset.sample_rate = 48_000;
    }

    preset.validate().map_err(ExportError::Settings)?;

    let project_duration = project.duration();
    if project_duration <= 0 {
        return Err(ExportError::Settings(
            "the timeline is empty, so there is nothing to export".into(),
        ));
    }
    // The range is clamped rather than rejected: the marks live in the UI and
    // the project keeps being edited under them, so a mark just past the last
    // clip is an ordinary state, not a user error. Only a range that clamps to
    // nothing is refused, because that export would be zero frames.
    let (range_start, duration) = match request.range {
        None => (0, project_duration),
        Some((a, b)) => {
            let start = a.min(b).clamp(0, project_duration);
            let end = a.max(b).clamp(0, project_duration);
            if end <= start {
                return Err(ExportError::Settings(
                    "the export range lies outside the timeline, so there is nothing to export"
                        .into(),
                ));
            }
            (start, end - start)
        }
    };
    let total_frames = preset.fps.frame_count(duration);
    if total_frames == 0 {
        return Err(ExportError::Settings(
            "the timeline is shorter than a single frame at this frame rate".into(),
        ));
    }

    let (accel, encoder_name) = match &hardware {
        Some(hw) => (hw.accel, hw.encoder_name.clone()),
        None => (
            HwAccel::Software,
            preset.video_codec.software_encoder().to_string(),
        ),
    };

    let video = VideoStreamSpec {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        encoder_name,
        accel,
        quality: preset.quality,
        options: Vec::new(),
    };

    let audio = match (request.include_audio, preset.audio_codec.encoder_name()) {
        (true, Some(name)) => Some(AudioStreamSpec {
            encoder_name: name.to_string(),
            sample_rate: preset.sample_rate,
            // Stereo regardless of the sources: a mono export of a stereo mix
            // silently folds down and a 5.1 export has no path through the
            // compositor's world. Everything the timeline produces is a stereo
            // bed.
            channels: 2,
            bitrate: preset.audio_bitrate,
        }),
        _ => None,
    };

    Ok(ExportSettings {
        output_path: with_extension(Path::new(&request.output_path), preset.extension()),
        preset,
        video,
        audio,
        total_frames,
        duration,
        range_start,
        loudness_target: request
            .overrides
            .as_ref()
            .and_then(|o| o.loudness_target)
            .filter(|t| t.is_finite() && (-40.0..=-5.0).contains(t)),
    })
}

/// Force the file extension to match the container.
///
/// The muxer is chosen from the extension, so a "YouTube 1080p" export written
/// to `clip.webm` would produce a WebM file full of H.264 — which is not a
/// thing. Correcting the name is better than either failing or lying.
fn with_extension(path: &Path, extension: &str) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(existing) if existing.eq_ignore_ascii_case(extension) => path.to_path_buf(),
        _ => path.with_extension(extension),
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStage {
    Preparing,
    MixingAudio,
    Encoding,
    Finalizing,
    Done,
    Cancelled,
    Failed,
}

impl ExportStage {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ExportStage::Done | ExportStage::Cancelled | ExportStage::Failed
        )
    }
}

/// One progress message. Mirrored by the frontend's `ExportProgress` type.
#[derive(Debug, Clone, Serialize)]
pub struct ExportProgress {
    pub job_id: String,
    pub stage: ExportStage,
    /// Frames finished so far.
    pub frame: u64,
    pub total_frames: u64,
    /// `frame / total_frames`, clamped to 0..1, so the UI does not repeat the
    /// division.
    pub fraction: f32,
    /// Frames per second achieved so far. Not the output frame rate.
    pub fps: f64,
    pub elapsed_seconds: f64,
    /// `None` until at least one frame is done — an ETA from no samples is a
    /// made-up number, and a progress bar that starts by claiming four hours is
    /// worse than one that says nothing yet.
    pub remaining_seconds: Option<f64>,
    /// Set on the terminal message, so the UI can offer "show in folder".
    pub output_path: Option<String>,
    /// User-facing prose on failure.
    pub message: Option<String>,
}

/// Where progress goes.
///
/// A trait rather than the Tauri channel directly so the job can be driven from
/// a test, and so nothing in here has to know about IPC.
pub trait ProgressSink: Send + Sync {
    fn send(&self, progress: ExportProgress);
}

/// Discards everything. For an export nobody is watching.
impl ProgressSink for () {
    fn send(&self, _: ExportProgress) {}
}

/// A closure as a sink.
///
/// A wrapper rather than a blanket `impl ProgressSink for F: Fn(..)` because
/// the command layer implements the trait for Tauri's `Channel`, and a blanket
/// impl over every callable would make the compiler refuse that as a possible
/// overlap.
pub struct FnSink<F>(pub F);

impl<F> ProgressSink for FnSink<F>
where
    F: Fn(ExportProgress) + Send + Sync,
{
    fn send(&self, progress: ExportProgress) {
        (self.0)(progress)
    }
}

/// Rate limiting and the ETA arithmetic, kept apart from the loop so both can
/// be tested with a fake clock.
#[derive(Debug)]
pub struct ProgressTracker {
    job_id: String,
    total: u64,
    started: Instant,
    last_emit: Option<Instant>,
    interval: Duration,
}

impl ProgressTracker {
    pub fn new(job_id: impl Into<String>, total: u64, started: Instant) -> Self {
        Self {
            job_id: job_id.into(),
            total,
            started,
            last_emit: None,
            interval: PROGRESS_INTERVAL,
        }
    }

    /// Whether frame `frame` should produce a message now.
    ///
    /// The first and last frame always do: the first proves the export started
    /// and the last is what closes the dialog. Everything between is rate
    /// limited.
    pub fn should_emit(&mut self, frame: u64, now: Instant) -> bool {
        let due = match self.last_emit {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };
        let edge = frame == 0 || (self.total > 0 && frame + 1 >= self.total);
        if due || edge {
            self.last_emit = Some(now);
            true
        } else {
            false
        }
    }

    pub fn snapshot(&self, stage: ExportStage, frame: u64, now: Instant) -> ExportProgress {
        let elapsed = now.duration_since(self.started).as_secs_f64();
        let fraction = if self.total == 0 {
            0.0
        } else {
            (frame as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        };
        ExportProgress {
            job_id: self.job_id.clone(),
            stage,
            frame,
            total_frames: self.total,
            fraction,
            fps: if elapsed > 0.0 {
                frame as f64 / elapsed
            } else {
                0.0
            },
            elapsed_seconds: elapsed,
            remaining_seconds: estimate_remaining(elapsed, frame, self.total),
            output_path: None,
            message: None,
        }
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        now.duration_since(self.started)
    }
}

/// Seconds left, from the average rate so far.
///
/// Averaging over the whole run rather than the last second on purpose: an
/// instantaneous rate swings wildly — a cut to a still image encodes ten times
/// faster than a cut to grain — and an ETA that jumps between "20 seconds" and
/// "4 minutes" twice a second is read as broken. The average converges and only
/// ever misleads at the very start, which is why the first frame reports
/// `None`.
pub fn estimate_remaining(elapsed_seconds: f64, done: u64, total: u64) -> Option<f64> {
    if done == 0 || total == 0 || elapsed_seconds <= 0.0 || done >= total {
        return None;
    }
    let rate = done as f64 / elapsed_seconds;
    if rate <= 0.0 {
        return None;
    }
    Some((total - done) as f64 / rate)
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

/// Everything one export needs, assembled by the command layer.
pub struct ExportJob {
    pub job_id: String,
    /// A snapshot. The document lock is not held while this runs.
    pub project: Project,
    pub settings: ExportSettings,
    pub compositor: Arc<Compositor>,
    pub sources: Arc<dyn SourceProvider>,
    pub audio: Arc<dyn AudioSource>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ExportOutcome {
    pub output_path: PathBuf,
    pub frames: u64,
    pub duration: Micros,
    pub elapsed: Duration,
    pub cancelled: bool,
    /// What the encoder side of each frame cost. Carried out of the job
    /// because the `MediaWriter` that measured it is dropped before this
    /// returns, and a benchmark that cannot see the breakdown can only report
    /// that something got faster.
    pub writer: super::encoder::WriterStats,
}

/// Render, encode and mux the whole timeline.
///
/// Blocking: the caller owns the thread. Sends progress as it goes and one
/// terminal message before returning.
/// One line per clip whose media cannot be read: the material was removed from
/// the project, or its file is gone from disk.
///
/// Public because the refusal message is a contract worth testing — "2 clips
/// reference media that is missing" with each clip named is what turns a
/// refused export from a mystery into a fixable timeline.
pub fn missing_media(project: &Project) -> Vec<String> {
    let mut lines = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            let at = segment.target_range.start as f64 / 1_000_000.0;
            let materials = &project.materials;
            match materials.kind_of(&segment.material_id) {
                None => lines.push(format!(
                    "the clip at {at:.1}s on \"{}\" references media that was removed from the \
                     project",
                    track.name
                )),
                Some(_) => {
                    let path = materials
                        .video(&segment.material_id)
                        .map(|m| m.path.as_str())
                        .or_else(|| {
                            materials
                                .audio(&segment.material_id)
                                .map(|m| m.path.as_str())
                        })
                        .or_else(|| {
                            materials
                                .image(&segment.material_id)
                                .map(|m| m.path.as_str())
                        });
                    if let Some(path) = path {
                        if !Path::new(path).exists() {
                            lines.push(format!(
                                "the clip at {at:.1}s on \"{}\": {path} is gone from disk",
                                track.name
                            ));
                        }
                    }
                }
            }
        }
    }
    lines
}

pub fn run_export(job: &ExportJob, sink: &dyn ProgressSink) -> Result<ExportOutcome> {
    let settings = &job.settings;
    let mut tracker =
        ProgressTracker::new(job.job_id.clone(), settings.total_frames, Instant::now());
    sink.send(tracker.snapshot(ExportStage::Preparing, 0, Instant::now()));

    // Refuse missing media up front, by name. The preview composites a
    // placeholder for an offline clip because an editor must keep working; a
    // delivered file with dark-red fields in it is a different matter, and the
    // strict compositor's per-frame error could only say "a source failed" —
    // this says which clips and why, before a single frame is rendered.
    let missing = missing_media(&job.project);
    if !missing.is_empty() {
        let error = ExportError::Settings(format!(
            "{count} clip{s} reference{verb} media that is missing:\n{list}",
            count = missing.len(),
            s = if missing.len() == 1 { "" } else { "s" },
            verb = if missing.len() == 1 { "s" } else { "" },
            list = missing.join("\n"),
        ));
        tracing::error!(%error, "the export was refused over missing media");
        let mut failed = tracker.snapshot(ExportStage::Failed, 0, Instant::now());
        failed.message = Some(error.to_string());
        sink.send(failed);
        return Err(error);
    }

    // Open the file first: a codec that is not in this build, a directory that
    // does not exist or a path that is not writable all fail here, in
    // milliseconds, instead of after the audio mix.
    let mut writer = match MediaWriter::create(
        &settings.output_path,
        &settings.video,
        settings.audio.as_ref(),
    ) {
        Ok(writer) => writer,
        Err(error) => {
            // Before the start block exists, so this is the only record that
            // this export was ever attempted.
            tracing::error!(
                output = %settings.output_path.display(),
                encoder = %settings.video.encoder_name,
                %error,
                "the export could not open its output file"
            );
            let mut failed = tracker.snapshot(ExportStage::Failed, 0, Instant::now());
            failed.message = Some(error.to_string());
            sink.send(failed);
            return Err(error);
        }
    };

    let result = encode_all(job, &mut writer, &mut tracker, sink);

    match result {
        Ok(frames) => {
            sink.send(tracker.snapshot(ExportStage::Finalizing, frames, Instant::now()));
            // `finish` is what flushes the encoders; the last second of video
            // is still inside libavcodec until it runs.
            // Read before `finish`, which consumes the writer.
            let writer_stats = writer.stats();
            writer.finish()?;
            let elapsed = tracker.elapsed(Instant::now());
            let mut done = tracker.snapshot(ExportStage::Done, frames, Instant::now());
            done.fraction = 1.0;
            done.remaining_seconds = Some(0.0);
            done.output_path = Some(settings.output_path.display().to_string());
            sink.send(done);
            Ok(ExportOutcome {
                output_path: settings.output_path.clone(),
                frames,
                duration: settings.duration,
                elapsed,
                cancelled: false,
                writer: writer_stats,
            })
        }
        Err(error) => {
            let cancelled = error.is_cancellation();
            // Either way the file is a fragment with no trailer; leaving it
            // would put something that looks like a video in the user's folder.
            writer.abort();
            let stage = if cancelled {
                ExportStage::Cancelled
            } else {
                ExportStage::Failed
            };
            let mut message = tracker.snapshot(stage, 0, Instant::now());
            message.message = Some(error.to_string());
            sink.send(message);
            if cancelled {
                Ok(ExportOutcome {
                    output_path: settings.output_path.clone(),
                    frames: 0,
                    duration: settings.duration,
                    elapsed: tracker.elapsed(Instant::now()),
                    cancelled: true,
                    writer: Default::default(),
                })
            } else {
                Err(error)
            }
        }
    }
}

fn encode_all(
    job: &ExportJob,
    writer: &mut MediaWriter,
    tracker: &mut ProgressTracker,
    sink: &dyn ProgressSink,
) -> Result<u64> {
    let settings = &job.settings;
    let fps = settings.fps();
    let size = settings.size();

    // The whole mix up front. Interleaving decode with encode would bound the
    // memory, but the mix is also what tells us the audio is decodable at all,
    // and finding that out after ten minutes of video is worse than a second of
    // waiting and a buffer.
    let mut mixed: Vec<f32> = Vec::new();
    let mut channels = 0usize;
    if let Some(spec) = &settings.audio {
        sink.send(tracker.snapshot(ExportStage::MixingAudio, 0, Instant::now()));
        channels = spec.channels.max(1) as usize;
        // A clip's denoised sound is a cached render; a cleared cache is
        // rebuilt here, with the settings the document records, rather than
        // exporting the noisy original the user never heard.
        crate::modules::voice::denoise::ensure_rendered(&job.project, &job.cancel)
            .map_err(|error| ExportError::Audio(anyhow::anyhow!(error)))?;
        mixed = audio::mix_timeline(
            &job.project,
            job.audio.as_ref(),
            spec.sample_rate,
            spec.channels,
            &job.cancel,
        )
        // The mix runs before the block below, so a failure here would
        // otherwise leave a log with no trace of the export at all.
        .map_err(|error| {
            if !error.is_cancellation() {
                tracing::error!(%error, "the audio mix failed; no frame was encoded");
            }
            error
        })?;
        // The mix is always the whole project — the mixer's arithmetic places
        // every segment at its absolute time — so a range export takes its
        // slice of the finished bed. Mixing only the range instead would mean
        // teaching every segment placement about an offset for a buffer that
        // is cheap next to one second of encoding.
        if settings.range_start > 0 || settings.duration < job.project.duration() {
            mixed = audio::slice_range(
                mixed,
                channels,
                spec.sample_rate,
                settings.range_start,
                settings.duration,
            );
        }
        if let Some(target) = settings.loudness_target {
            let report = crate::modules::loudness::normalize_in_place(
                &mut mixed,
                channels,
                spec.sample_rate,
                target as f64,
                crate::modules::loudness::TRUE_PEAK_CEILING,
            )
            .map_err(|error| ExportError::Audio(anyhow::anyhow!(error)))?;
            tracing::info!(
                target,
                before = ?report.before.integrated,
                after = ?report.after.integrated,
                gain_db = report.gain_db,
                "normalised the mix"
            );
        }
    }

    let audio_rate = settings.audio.as_ref().map(|a| a.sample_rate).unwrap_or(0);
    let mut audio_cursor = 0usize;

    // Which colour conversion the frames take. NV12 on the GPU is worth about a
    // third of a 1080p export frame — see `docs/research/zero-copy-encode.md` —
    // but it is only useful when the encoder is fed NV12 in the first place,
    // and it needs a compute pass that a very old device might not build. So:
    // ask for it when it helps, and drop back to the RGBA path for the whole
    // export the first time it does not work, rather than per frame.
    //
    // The two conditions are named rather than inlined because the log block
    // below has to say *which* of them ruled the faster path out. A machine we
    // cannot see is the normal case for a bug report.
    let encoder_wants_nv12 = writer.wants_nv12();
    let gpu_convert_allowed = gpu_color_convert();
    let mut gpu_nv12 = encoder_wants_nv12 && gpu_convert_allowed;

    // And whether the frame can skip system memory entirely. This needs three
    // things at once — a Vulkan device that exports DMA-BUF memory, a VAAPI
    // driver that imports it, and an encoder with a surface pool — so it is
    // established by trying and it degrades to the NV12-readback path above,
    // which itself degrades to swscale. Three tiers, each a strict improvement
    // on the one below, and an export happens on whichever the machine reaches.
    let zero_copy_allowed = zero_copy_enabled();
    let mut zero_copy = (gpu_nv12 && zero_copy_allowed)
        .then(|| ZeroCopy::new(job, size))
        .flatten();

    let chosen = FramePathChoice::of(
        zero_copy.is_some(),
        gpu_nv12,
        encoder_wants_nv12,
        gpu_convert_allowed,
        zero_copy_allowed,
        &settings.video.encoder_name,
    );
    tracing::info!("{}", describe_export(settings, &chosen));

    // What the frame loop had to give up on halfway through, if anything. Both
    // fallbacks below are silent in the finished file — a demoted export is
    // slower and otherwise identical — so the only place they can ever be seen
    // is here.
    let mut fallbacks: Vec<String> = Vec::new();
    let mut written = 0u64;
    let mut observed_nv12: Option<(usize, usize, usize, usize)> = None;

    let walk = walk_frames(fps, settings.total_frames, &job.cancel, |index, time| {
        // `time` is relative to the export — frame 0 is time 0 — and the
        // compositor wants the timeline's clock. The offset here and the
        // index-based PTS everywhere below are together what rebases a range
        // export to start at zero.
        // Sampled just inside the frame rather than on its first microsecond;
        // see `SAMPLE_SLACK` for the duplicated frame at a cut that prevents.
        let time = settings.range_start + time + crate::modules::project::SAMPLE_SLACK;
        // Every clip of this frame decoded at once, one thread per clip,
        // rather than one after another inside the compositor.
        job.sources
            .prefetch(job.compositor.context(), &job.project, time, size);
        if let Some(state) = zero_copy.as_mut() {
            match state.frame(job, writer, size, index, time) {
                Ok(()) => {
                    on_frame_written(
                        writer,
                        &mixed,
                        channels,
                        audio_rate,
                        fps,
                        index,
                        &mut audio_cursor,
                    )?;
                    written = index + 1;
                    let now = Instant::now();
                    if tracker.should_emit(index, now) {
                        sink.send(tracker.snapshot(ExportStage::Encoding, index + 1, now));
                    }
                    return Ok(());
                }
                Err(source) => {
                    tracing::warn!(
                        frame = index,
                        error = %source,
                        "the zero-copy path failed; falling back to a readback for the rest \
                         of this export"
                    );
                    fallbacks.push(format!(
                        "zero-copy gave up at frame {index} ({source}); the rest of the export \
                         read frames back"
                    ));
                    zero_copy = None;
                }
            }
        }
        if gpu_nv12 {
            match job
                .compositor
                .render_nv12(&job.project, time, size, job.sources.as_ref())
            {
                Ok(nv12) => {
                    let (y_stride, uv_stride) = (nv12.y_stride, nv12.uv_stride);
                    let offset = nv12.uv_offset();
                    // Measured rather than predicted, once: what the block
                    // above printed is what `Nv12Layout` says this size should
                    // be, and the whole reason to log strides is that the two
                    // can disagree.
                    observed_nv12.get_or_insert((y_stride, uv_stride, offset, nv12.data.len()));
                    writer.write_video_frame_nv12(
                        &nv12.data[..offset],
                        y_stride,
                        &nv12.data[offset..],
                        uv_stride,
                        index,
                    )?;
                }
                Err(source) => {
                    // A source that genuinely failed will fail the same way on
                    // the RGBA path a line below, and will be reported there
                    // with the frame number attached. What is being caught here
                    // is a device that cannot run the compute pass.
                    tracing::warn!(
                        frame = index,
                        error = %source,
                        "the GPU colour conversion failed; falling back to swscale for the \
                         rest of this export"
                    );
                    fallbacks.push(format!(
                        "the GPU NV12 conversion gave up at frame {index} ({source}); the rest \
                         of the export went through swscale"
                    ));
                    gpu_nv12 = false;
                }
            }
        }
        if !gpu_nv12 {
            let rgba = job
                .compositor
                .render_frame(&job.project, time, size, job.sources.as_ref())
                .map_err(|source| ExportError::Render {
                    frame: index,
                    source,
                })?;
            writer.write_video_frame(&rgba, index)?;
        }

        on_frame_written(
            writer,
            &mixed,
            channels,
            audio_rate,
            fps,
            index,
            &mut audio_cursor,
        )?;

        written = index + 1;
        let now = Instant::now();
        if tracker.should_emit(index, now) {
            sink.send(tracker.snapshot(ExportStage::Encoding, index + 1, now));
        }
        Ok(())
    });

    // Before the `?`, so a cancelled or failed export says how far it got
    // rather than saying nothing at all — which is the case somebody is most
    // likely to be reading the log for.
    tracing::info!(
        "{}",
        describe_outcome(
            settings,
            walk.as_ref().err(),
            written,
            tracker.elapsed(Instant::now()),
            final_path_label(zero_copy.is_some(), gpu_nv12),
            observed_nv12,
            &fallbacks,
        )
    );
    walk?;

    // Whatever the last frame boundary did not cover — a mix is exactly as long
    // as the project, and the last frame starts before the project ends.
    if !mixed.is_empty() && audio_cursor * channels < mixed.len() {
        writer.write_audio(&mixed[audio_cursor * channels..])?;
    }

    Ok(settings.total_frames)
}

// ---------------------------------------------------------------------------
// The export log
// ---------------------------------------------------------------------------
//
// An export that comes out wrong is reported by someone who cannot see any of
// this, and the three frame paths produce the same file when they work — so
// when one of them does not, nothing in the output says which ran. These two
// blocks are the only record. They are formatted as a block rather than as
// fields because they are read by a person scrolling a log, and every number in
// them was chosen because it has at some point been the answer: strides
// especially, since a stride disagreement is invisible in every other
// symptom except the picture.

/// Which of the three frame paths an export took, and why not a better one.
struct FramePathChoice {
    label: &'static str,
    /// `None` on the top tier. Prose, because whoever reads it is diagnosing a
    /// machine they cannot log into.
    demoted_because: Option<String>,
    /// Whether frames reach the encoder as NV12 at all. The swscale tier hands
    /// over RGBA and the conversion happens inside `encoder.rs`.
    nv12: bool,
}

impl FramePathChoice {
    fn of(
        zero_copy: bool,
        gpu_nv12: bool,
        encoder_wants_nv12: bool,
        gpu_convert_allowed: bool,
        zero_copy_allowed: bool,
        encoder_name: &str,
    ) -> Self {
        if zero_copy {
            return Self {
                label: "zero-copy DMA-BUF (tier 1 of 3)",
                demoted_because: None,
                nv12: true,
            };
        }
        if gpu_nv12 {
            return Self {
                label: "GPU NV12 readback (tier 2 of 3)",
                demoted_because: Some(
                    if zero_copy_allowed {
                        "no zero-copy: the compositor could not export a DMA-BUF ring this \
                         encoder can import"
                    } else {
                        "no zero-copy: switched off for this process (set_zero_copy)"
                    }
                    .to_string(),
                ),
                nv12: true,
            };
        }
        Self {
            label: "swscale (tier 3 of 3)",
            demoted_because: Some(if !encoder_wants_nv12 {
                format!(
                    "no GPU colour conversion: {encoder_name} is fed YUV420P, so NV12 from the \
                     GPU would move the conversion rather than remove it"
                )
            } else if !gpu_convert_allowed {
                "no GPU colour conversion: switched off for this process (set_gpu_color_convert)"
                    .to_string()
            } else {
                "no GPU colour conversion: the device has no RGBA to NV12 compute pass".to_string()
            }),
            nv12: false,
        }
    }
}

/// What the frame loop ended up on, which is not always what it started on.
fn final_path_label(zero_copy: bool, gpu_nv12: bool) -> &'static str {
    match (zero_copy, gpu_nv12) {
        (true, _) => "zero-copy DMA-BUF (tier 1 of 3)",
        (false, true) => "GPU NV12 readback (tier 2 of 3)",
        (false, false) => "swscale (tier 3 of 3)",
    }
}

/// The block logged before the first frame.
fn describe_export(settings: &ExportSettings, chosen: &FramePathChoice) -> String {
    let (width, height) = settings.size();
    let fps = settings.fps();
    let quality = match settings.video.quality {
        Quality::Crf(crf) => format!("crf {crf}"),
        Quality::Bitrate(bits) => format!("{} target", bitrate(bits)),
    };
    // What a rate-control mode that insists on a number is given. Every VAAPI
    // mode does, whatever the preset says, so this is the bitrate a hardware
    // export actually runs at — see `hwaccel::fallback_bitrate`.
    let effective = hwaccel::fallback_bitrate(width, height, fps, settings.video.quality);

    let audio = match &settings.audio {
        Some(spec) => format!(
            "{} · {} · {} Hz · {} ch",
            spec.encoder_name,
            bitrate(u64::from(spec.bitrate)),
            spec.sample_rate,
            spec.channels
        ),
        None => "none".to_string(),
    };

    let layout = crate::modules::render::Nv12Layout::for_size(width, height);
    let nv12 = if chosen.nv12 {
        format!(
            "y_stride {} B · uv_stride {} B · uv_offset {} B · total {} B",
            layout.y_stride,
            layout.uv_stride,
            layout.uv_offset(),
            layout.total_bytes()
        )
    } else {
        // Not omitted even here: knowing that nothing in this process chose the
        // strides is itself the answer when a picture comes out sheared.
        format!(
            "not used — RGBA leaves the compositor and libswscale converts inside the encoder \
             (an NV12 frame of this size would be y_stride {} B · uv_stride {} B · uv_offset \
             {} B · total {} B)",
            layout.y_stride,
            layout.uv_stride,
            layout.uv_offset(),
            layout.total_bytes()
        )
    };

    let mut block = format!(
        "export starting\n  \
           output     {}\n  \
           container  {}\n  \
           video      {}x{} @ {:.3} fps · {} · {} (≈ {})\n  \
           encoder    {} · {}\n  \
           audio      {}\n  \
           length     {} frames, {:.3} s\n  \
           path       {}\n  \
           nv12       {}",
        settings.output_path.display(),
        settings.preset.container.extension(),
        width,
        height,
        fps.as_f64(),
        settings.preset.video_codec.label(),
        quality,
        bitrate(effective),
        settings.video.encoder_name,
        accel_label(settings.video.accel),
        audio,
        settings.total_frames,
        settings.duration as f64 / 1_000_000.0,
        chosen.label,
        nv12,
    );
    if settings.range_start > 0 {
        block.push_str(&format!(
            "\n  range      timeline {:.3} s to {:.3} s, rebased to start at zero",
            settings.range_start as f64 / 1_000_000.0,
            (settings.range_start + settings.duration) as f64 / 1_000_000.0,
        ));
    }
    if let Some(reason) = &chosen.demoted_because {
        block.push_str(&format!("\n  why        {reason}"));
    }
    block
}

/// The block logged after the last frame, or after the one that stopped it.
#[allow(clippy::too_many_arguments)]
fn describe_outcome(
    settings: &ExportSettings,
    error: Option<&ExportError>,
    frames: u64,
    elapsed: Duration,
    path: &str,
    observed_nv12: Option<(usize, usize, usize, usize)>,
    fallbacks: &[String],
) -> String {
    let seconds = elapsed.as_secs_f64();
    let headline = match error {
        None => "export finished",
        Some(error) if error.is_cancellation() => "export cancelled",
        Some(_) => "export failed",
    };

    let mut block = format!(
        "{headline}\n  \
           output     {}\n  \
           frames     {} of {} written\n  \
           wall       {:.2} s ({:.1} fps mean)\n  \
           path       {} at the end",
        settings.output_path.display(),
        frames,
        settings.total_frames,
        seconds,
        if seconds > 0.0 {
            frames as f64 / seconds
        } else {
            0.0
        },
        path,
    );

    if let Some((y_stride, uv_stride, uv_offset, total)) = observed_nv12 {
        block.push_str(&format!(
            "\n  nv12       as rendered: y_stride {y_stride} B · uv_stride {uv_stride} B · \
             uv_offset {uv_offset} B · total {total} B"
        ));
    }

    if fallbacks.is_empty() {
        block.push_str("\n  fallbacks  none");
    } else {
        for (index, fallback) in fallbacks.iter().enumerate() {
            let label = if index == 0 { "fallbacks" } else { "         " };
            block.push_str(&format!("\n  {label}  {fallback}"));
        }
    }

    if let Some(error) = error {
        block.push_str(&format!("\n  error      {error}"));
    }
    block
}

fn accel_label(accel: HwAccel) -> String {
    match accel {
        HwAccel::Software => "software".to_string(),
        other => format!("hardware ({})", other.label()),
    }
}

/// A bit rate at the precision anyone reads it at.
fn bitrate(bits_per_second: u64) -> String {
    if bits_per_second >= 1_000_000 {
        format!("{:.1} Mb/s", bits_per_second as f64 / 1_000_000.0)
    } else {
        format!("{} kb/s", bits_per_second / 1_000)
    }
}

/// Hand the muxer the audio that belongs *before* the next video frame.
///
/// Keeps the interleaving tight, so a player never has to buffer seconds of one
/// stream to find the other. A function rather than a closure because all three
/// video paths end here and duplicating it is how one of them ends up silent.
#[allow(clippy::too_many_arguments)]
fn on_frame_written(
    writer: &mut MediaWriter,
    mixed: &[f32],
    channels: usize,
    audio_rate: u32,
    fps: Fps,
    index: u64,
    audio_cursor: &mut usize,
) -> Result<()> {
    if mixed.is_empty() {
        return Ok(());
    }
    let until =
        audio::frames_for(fps.frame_time(index + 1), audio_rate).min(mixed.len() / channels);
    if until > *audio_cursor {
        writer.write_audio(&mixed[*audio_cursor * channels..until * channels])?;
        *audio_cursor = until;
    }
    Ok(())
}

/// The zero-copy export path's state: a rotation of exported buffers and the
/// surfaces currently mapped from them.
///
/// Linux and Vulkan only; [`Self::new`] answers `None` everywhere else and the
/// caller falls back.
#[cfg(target_os = "linux")]
struct ZeroCopy {
    ring: crate::modules::render::dmabuf::Nv12Ring,
    layout: crate::modules::render::Nv12Layout,
    /// The surface mapped from each ring slot, kept so its reference count can
    /// be asked before that slot is written again. `None` for a slot that has
    /// not been used yet.
    held: Vec<Option<ffmpeg_next::util::frame::Video>>,
    slot: usize,
}

#[cfg(target_os = "linux")]
impl ZeroCopy {
    fn new(job: &ExportJob, size: (u32, u32)) -> Option<Self> {
        use crate::modules::render::dmabuf::{Nv12Ring, RING};
        use crate::modules::render::Nv12Layout;

        let layout = Nv12Layout::for_size(size.0, size.1);
        let ring = Nv12Ring::new(job.compositor.context(), layout.total_bytes() as u64)?;
        Some(Self {
            ring,
            layout,
            held: (0..RING).map(|_| None).collect(),
            slot: 0,
        })
    }

    /// Composite frame `index` straight into an exported buffer and encode it.
    fn frame(
        &mut self,
        job: &ExportJob,
        writer: &mut MediaWriter,
        size: (u32, u32),
        index: u64,
        time: Micros,
    ) -> Result<()> {
        use crate::modules::render::dmabuf::DRM_FORMAT_MOD_LINEAR;
        use std::os::fd::AsFd;

        // Refuse to overwrite memory the encoder is still reading. With
        // sixteen slots against two B-frames this should never fire; if it
        // does, saying so and falling back is the only honest option, because
        // writing anyway produces a torn picture that nothing downstream would
        // report.
        if let Some(previous) = &self.held[self.slot] {
            if super::hwframes::surface_is_shared(previous) {
                return Err(ExportError::Settings(
                    "the encoder is still holding every exported buffer".into(),
                ));
            }
        }
        // Dropped before the buffer is rewritten, not after: this releases our
        // reference to the previous mapping.
        self.held[self.slot] = None;

        let slot = self.slot;
        self.slot = (self.slot + 1) % self.ring.len();

        let exported = self.ring.take();
        job.compositor
            .render_nv12_into(
                &job.project,
                time,
                size,
                job.sources.as_ref(),
                exported.buffer(),
            )
            .map_err(|source| ExportError::Render {
                frame: index,
                source,
            })?;

        let surface = writer.write_video_frame_dmabuf(
            &super::hwframes::Nv12Dmabuf {
                fd: exported.fd().as_fd(),
                size: self.layout.total_bytes(),
                width: size.0,
                height: size.1,
                y_offset: 0,
                y_stride: self.layout.y_stride,
                uv_offset: self.layout.uv_offset(),
                uv_stride: self.layout.uv_stride,
                modifier: DRM_FORMAT_MOD_LINEAR,
            },
            index,
        )?;
        self.held[slot] = Some(surface);
        Ok(())
    }
}

/// Walk `0..total` at `fps`, checking `cancel` before every frame.
///
/// The flag is checked *before* the work rather than after, so a cancel that
/// arrives during frame N stops before N+1 rather than after it, and a job
/// cancelled before it starts renders nothing at all.
pub fn walk_frames<F>(fps: Fps, total: u64, cancel: &AtomicBool, mut on_frame: F) -> Result<u64>
where
    F: FnMut(u64, Micros) -> Result<()>,
{
    for index in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        on_frame(index, fps.frame_time(index))?;
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Injected dependencies and running jobs
// ---------------------------------------------------------------------------

/// Where PCM comes from, when something has provided it.
///
/// Video frames do not need a registry — the command builds a
/// `MediaSourceProvider` from the same project snapshot it exports. Audio does,
/// because `media` has no PCM reader yet and this is the seam it plugs into
/// when it grows one; see [`super::audio::AudioSource`].
static AUDIO: parking_lot::RwLock<Option<Arc<dyn AudioSource>>> = parking_lot::RwLock::new(None);

pub fn register_audio_source(source: Arc<dyn AudioSource>) {
    *AUDIO.write() = Some(source);
}

/// The registered source, or silence.
///
/// Falling back rather than failing is deliberate: an export with no audio
/// decoder wired up still produces a correct, silent video, which is a visible
/// and diagnosable result. Refusing to export at all would look like a broken
/// button.
pub fn audio_source() -> Arc<dyn AudioSource> {
    match AUDIO.read().clone() {
        Some(source) => source,
        None => {
            tracing::warn!("no audio source is registered; exporting a silent audio track");
            Arc::new(SilentAudioSource)
        }
    }
}

/// Whether an export may convert RGBA to NV12 on the GPU.
///
/// A process-wide switch rather than a field on [`ExportJob`] because the only
/// caller that wants it off is a benchmark measuring what the GPU conversion
/// bought, and threading a flag through every construction site to serve one
/// measurement is the wrong trade. Nothing in the app turns it off.
static GPU_COLOUR_CONVERT: AtomicBool = AtomicBool::new(true);

pub fn set_gpu_color_convert(enabled: bool) {
    GPU_COLOUR_CONVERT.store(enabled, Ordering::Relaxed);
}

pub fn gpu_color_convert() -> bool {
    GPU_COLOUR_CONVERT.load(Ordering::Relaxed)
}

/// Whether an export may hand the encoder the compositor's own memory.
///
/// The third tier, and off-switchable for the same reason as the second: the
/// only way to know what a stage costs is to run the export without it.
static ZERO_COPY: AtomicBool = AtomicBool::new(true);

pub fn set_zero_copy(enabled: bool) {
    ZERO_COPY.store(enabled, Ordering::Relaxed);
}

pub fn zero_copy_enabled() -> bool {
    ZERO_COPY.load(Ordering::Relaxed)
}

/// Cancel flags for the jobs currently running, keyed by job id.
///
/// A `Vec` because there is never more than a handful: exports are heavy and
/// the UI runs one at a time.
static JOBS: parking_lot::Mutex<Vec<(String, Arc<AtomicBool>)>> =
    parking_lot::Mutex::new(Vec::new());

/// Register a new job and get its cancel flag.
pub fn begin_job(job_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    JOBS.lock().push((job_id.to_string(), flag.clone()));
    flag
}

/// Ask a job to stop. `false` when it already finished, which is not an error:
/// the dialog closing after the last frame is a race nobody can win.
pub fn cancel_job(job_id: &str) -> bool {
    let jobs = JOBS.lock();
    match jobs.iter().find(|(id, _)| id == job_id) {
        Some((_, flag)) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Cancel everything. For shutdown.
pub fn cancel_all() {
    for (_, flag) in JOBS.lock().iter() {
        flag.store(true, Ordering::Relaxed);
    }
}

pub fn end_job(job_id: &str) {
    JOBS.lock().retain(|(id, _)| id != job_id);
}

pub fn active_jobs() -> usize {
    JOBS.lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
        MICROS_PER_SECOND,
    };

    fn project(duration: Micros) -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/tmp/v1.mp4".into(),
            width: 1920,
            height: 1080,
            duration,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s1".into(),
            material_id: "v1".into(),
            target_range: TimeRange::new(0, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(track);
        project
    }

    fn request(path: &str) -> ExportRequest {
        ExportRequest {
            output_path: path.into(),
            preset_id: None,
            overrides: None,
            hardware: None,
            include_audio: true,
            range: None,
        }
    }

    // -- settings ---------------------------------------------------------

    #[test]
    fn the_default_request_follows_the_project() {
        let project = project(2 * MICROS_PER_SECOND);
        let settings = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap();
        assert_eq!(settings.size(), (1080, 1920));
        assert_eq!(settings.total_frames, 60);
        assert_eq!(settings.video.encoder_name, "libx264");
        assert_eq!(settings.video.accel, HwAccel::Software);
        assert!(settings.audio.is_some());
    }

    #[test]
    fn a_preset_overrides_the_canvas() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.preset_id = Some("youtube_1080p".into());
        let settings = resolve_settings(&project, &req).unwrap();
        assert_eq!(settings.size(), (1920, 1080));
        assert_eq!(settings.preset.quality, Quality::Crf(20));
    }

    #[test]
    fn overrides_are_applied_on_top_of_the_preset() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.preset_id = Some("youtube_1080p".into());
        req.overrides = Some(ExportOverrides {
            fps: Some(59.94),
            quality: Some(Quality::Bitrate(12_000_000)),
            ..Default::default()
        });
        let settings = resolve_settings(&project, &req).unwrap();
        assert_eq!(settings.fps(), Fps::NTSC_DOUBLE);
        assert_eq!(settings.preset.quality, Quality::Bitrate(12_000_000));
        // One second of 59.94 is 60 frames, not 59.
        assert_eq!(settings.total_frames, 60);
    }

    // -- the export range --------------------------------------------------

    #[test]
    fn a_range_shortens_the_export_and_remembers_where_it_starts() {
        let project = project(4 * MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.range = Some((MICROS_PER_SECOND, 3 * MICROS_PER_SECOND));
        let settings = resolve_settings(&project, &req).unwrap();

        assert_eq!(settings.range_start, MICROS_PER_SECOND);
        assert_eq!(settings.duration, 2 * MICROS_PER_SECOND);
        // 30 fps custom preset: two seconds is sixty frames, counted from the
        // range and not from the project.
        assert_eq!(settings.total_frames, 60);
    }

    #[test]
    fn a_range_is_clamped_to_the_project_not_rejected() {
        let project = project(2 * MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        // Starts before zero, ends past the timeline: the marks outlived an
        // edit, which is an ordinary state.
        req.range = Some((-MICROS_PER_SECOND, 10 * MICROS_PER_SECOND));
        let settings = resolve_settings(&project, &req).unwrap();

        assert_eq!(settings.range_start, 0);
        assert_eq!(settings.duration, 2 * MICROS_PER_SECOND);
        assert_eq!(settings.total_frames, 60);
    }

    #[test]
    fn an_inverted_range_is_ordered_rather_than_refused() {
        let project = project(4 * MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.range = Some((3 * MICROS_PER_SECOND, MICROS_PER_SECOND));
        let settings = resolve_settings(&project, &req).unwrap();

        assert_eq!(settings.range_start, MICROS_PER_SECOND);
        assert_eq!(settings.duration, 2 * MICROS_PER_SECOND);
    }

    #[test]
    fn a_range_entirely_past_the_timeline_is_refused_in_prose() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.range = Some((5 * MICROS_PER_SECOND, 9 * MICROS_PER_SECOND));
        let error = resolve_settings(&project, &req).unwrap_err();
        assert!(error.to_string().contains("nothing to export"));
    }

    #[test]
    fn a_whole_project_request_has_no_offset() {
        let project = project(2 * MICROS_PER_SECOND);
        let settings = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap();
        assert_eq!(settings.range_start, 0);
        assert_eq!(settings.duration, 2 * MICROS_PER_SECOND);
    }

    #[test]
    fn the_start_block_names_the_range_when_there_is_one() {
        let project = project(4 * MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.range = Some((MICROS_PER_SECOND, 3 * MICROS_PER_SECOND));
        let settings = resolve_settings(&project, &req).unwrap();

        let chosen = FramePathChoice::of(true, true, true, true, true, "h264_vaapi");
        let block = describe_export(&settings, &chosen);
        assert!(
            block.contains("range      timeline 1.000 s to 3.000 s"),
            "{block}"
        );

        // And a whole-project export does not mention one.
        let whole = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap();
        assert!(!describe_export(&whole, &chosen).contains("range      "));
    }

    #[test]
    fn an_unknown_preset_is_rejected_in_prose() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.preset_id = Some("vimeo_8k".into());
        let error = resolve_settings(&project, &req).unwrap_err();
        assert!(error.to_string().contains("vimeo_8k"));
    }

    #[test]
    fn an_empty_timeline_cannot_be_exported() {
        let project = Project::new("t", CanvasConfig::default(), 30.0);
        let error = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap_err();
        assert!(error.to_string().contains("nothing to export"));
    }

    #[test]
    fn the_extension_is_forced_to_match_the_container() {
        let project = project(MICROS_PER_SECOND);
        let settings = resolve_settings(&project, &request("/tmp/clip.webm")).unwrap();
        assert_eq!(settings.output_path, PathBuf::from("/tmp/clip.mp4"));

        // A name with no extension gets one rather than being mangled.
        let settings = resolve_settings(&project, &request("/tmp/clip")).unwrap();
        assert_eq!(settings.output_path, PathBuf::from("/tmp/clip.mp4"));

        // A name with dots in it keeps them.
        let settings = resolve_settings(&project, &request("/tmp/my.clip.v2.mp4")).unwrap();
        assert_eq!(settings.output_path, PathBuf::from("/tmp/my.clip.v2.mp4"));
    }

    #[test]
    fn audio_can_be_left_out() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.include_audio = false;
        assert!(resolve_settings(&project, &req).unwrap().audio.is_none());
    }

    #[test]
    fn opus_is_snapped_to_the_only_rate_it_encodes_well() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.webm");
        req.overrides = Some(ExportOverrides {
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: Some(AudioCodec::Opus),
            sample_rate: Some(44_100),
            container: Some(super::super::presets::Container::Webm),
            ..Default::default()
        });
        let settings = resolve_settings(&project, &req).unwrap();
        assert_eq!(settings.audio.as_ref().unwrap().sample_rate, 48_000);
        assert_eq!(settings.output_path, PathBuf::from("/tmp/out.webm"));
    }

    #[test]
    fn an_unknown_hardware_encoder_is_rejected() {
        let project = project(MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.hardware = Some("nvenc_h264".into());
        // Either this machine has NVENC and the settings resolve, or it does
        // not and the error names the encoder. Both are correct; what must not
        // happen is a panic or a silent fallback to software.
        match resolve_settings(&project, &req) {
            Ok(settings) => assert_eq!(settings.video.encoder_name, "h264_nvenc"),
            Err(error) => assert!(error.to_string().contains("nvenc_h264")),
        }
    }

    // -- the export log ---------------------------------------------------

    /// The block is what a bug report is diagnosed from, so every fact in it is
    /// asserted here rather than trusted to survive an edit.
    #[test]
    fn the_start_block_states_what_this_export_is() {
        let project = project(2 * MICROS_PER_SECOND);
        let mut req = request("/tmp/out.mp4");
        req.preset_id = Some("youtube_1080p".into());
        let settings = resolve_settings(&project, &req).unwrap();

        let chosen = FramePathChoice::of(true, true, true, true, true, "h264_vaapi");
        let block = describe_export(&settings, &chosen);

        assert!(block.contains("/tmp/out.mp4"), "{block}");
        assert!(block.contains("1920x1080"), "{block}");
        assert!(block.contains("30.000 fps"), "{block}");
        assert!(block.contains("H.264"), "{block}");
        assert!(block.contains("crf 20"), "{block}");
        assert!(block.contains("libx264 · software"), "{block}");
        assert!(block.contains("mp4"), "{block}");
        assert!(block.contains("zero-copy DMA-BUF (tier 1 of 3)"), "{block}");
        // The strides are the reason this exists.
        assert!(
            block.contains(
                "y_stride 1920 B · uv_stride 1920 B · uv_offset 2073600 B · total 3110400 B"
            ),
            "{block}"
        );
        // Nothing to explain on the top tier.
        assert!(!block.contains("why "), "{block}");
    }

    #[test]
    fn a_demoted_export_says_which_tier_and_why() {
        let project = project(MICROS_PER_SECOND);
        let settings = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap();

        // Tier 2: zero-copy was allowed and the ring could not be built.
        let ring_failed = FramePathChoice::of(false, true, true, true, true, "h264_vaapi");
        let block = describe_export(&settings, &ring_failed);
        assert!(block.contains("GPU NV12 readback (tier 2 of 3)"), "{block}");
        assert!(block.contains("could not export a DMA-BUF ring"), "{block}");
        // Still NV12, so the strides are the ones in use — and for this canvas
        // they are wider than the frame. 1080 pads to 1152: see
        // `render::nv12::ROW_ALIGN`, which exists because the hardware encoder
        // misreads any plane whose pitch is not aligned. Printing the padded
        // number is the point of logging it at all.
        assert!(block.contains("y_stride 1152 B"), "{block}");

        // Tier 2 for the other reason.
        let switched_off = FramePathChoice::of(false, true, true, true, false, "h264_vaapi");
        assert!(describe_export(&settings, &switched_off).contains("set_zero_copy"));

        // Tier 3: a software encoder wants YUV420P, so neither GPU path helps.
        let software = FramePathChoice::of(false, false, false, true, true, "libx264");
        let block = describe_export(&settings, &software);
        assert!(block.contains("swscale (tier 3 of 3)"), "{block}");
        assert!(block.contains("libx264 is fed YUV420P"), "{block}");
        // The layout is labelled as not in use, and the numbers are still
        // there — "what would this size be" is the first question asked when a
        // swscale export comes out sheared.
        assert!(block.contains("not used"), "{block}");
        assert!(block.contains("y_stride 1152 B"), "{block}");

        // Tier 3 because the compute pass would not build on this device.
        let no_compute = FramePathChoice::of(false, false, true, true, true, "h264_vaapi");
        assert!(describe_export(&settings, &no_compute).contains("no RGBA to NV12 compute pass"));
    }

    #[test]
    fn the_end_block_reports_the_run_and_any_fallback() {
        let project = project(2 * MICROS_PER_SECOND);
        let settings = resolve_settings(&project, &request("/tmp/out.mp4")).unwrap();

        let clean = describe_outcome(
            &settings,
            None,
            60,
            Duration::from_secs(3),
            final_path_label(true, true),
            None,
            &[],
        );
        assert!(clean.starts_with("export finished"), "{clean}");
        assert!(clean.contains("60 of 60 written"), "{clean}");
        assert!(clean.contains("3.00 s (20.0 fps mean)"), "{clean}");
        assert!(clean.contains("fallbacks  none"), "{clean}");

        let demoted = describe_outcome(
            &settings,
            None,
            60,
            Duration::from_secs(6),
            final_path_label(false, true),
            Some((1088, 1088, 2088960, 3133440)),
            &["zero-copy gave up at frame 12 (the encoder is still holding …)".to_string()],
        );
        assert!(
            demoted.contains("GPU NV12 readback (tier 2 of 3) at the end"),
            "{demoted}"
        );
        assert!(
            demoted.contains("zero-copy gave up at frame 12"),
            "{demoted}"
        );
        // What the compositor really produced, not what the layout predicted.
        assert!(
            demoted.contains("as rendered: y_stride 1088 B · uv_stride 1088 B · uv_offset 2088960 B · total 3133440 B"),
            "{demoted}"
        );

        let cancelled = describe_outcome(
            &settings,
            Some(&ExportError::Cancelled),
            7,
            Duration::from_secs(1),
            final_path_label(false, false),
            None,
            &[],
        );
        assert!(cancelled.starts_with("export cancelled"), "{cancelled}");
        assert!(cancelled.contains("7 of 60 written"), "{cancelled}");

        let failed = describe_outcome(
            &settings,
            Some(&ExportError::Settings(
                "the muxer refused the packet".into(),
            )),
            7,
            Duration::from_secs(1),
            final_path_label(false, false),
            None,
            &[],
        );
        assert!(failed.starts_with("export failed"), "{failed}");
        assert!(failed.contains("the muxer refused the packet"), "{failed}");
    }

    // -- progress and ETA -------------------------------------------------

    #[test]
    fn there_is_no_eta_before_the_first_frame() {
        assert_eq!(estimate_remaining(0.0, 0, 100), None);
        assert_eq!(estimate_remaining(5.0, 0, 100), None);
        // Nor after the last one.
        assert_eq!(estimate_remaining(5.0, 100, 100), None);
    }

    #[test]
    fn the_eta_follows_the_average_rate() {
        // 10 frames in 1 s = 10 fps; 90 left is 9 s.
        assert_eq!(estimate_remaining(1.0, 10, 100), Some(9.0));
        // Half way through a 20 s job is 10 s left.
        assert_eq!(estimate_remaining(10.0, 50, 100), Some(10.0));
    }

    #[test]
    fn progress_is_rate_limited_but_never_misses_the_edges() {
        let start = Instant::now();
        let mut tracker = ProgressTracker::new("job", 100, start);

        // The first frame always reports.
        assert!(tracker.should_emit(0, start));
        // The next one, immediately after, does not.
        assert!(!tracker.should_emit(1, start));
        assert!(!tracker.should_emit(2, start + Duration::from_millis(50)));
        // Once the interval has passed it does again.
        assert!(tracker.should_emit(3, start + PROGRESS_INTERVAL));
        // And the last frame reports regardless of when the previous one did.
        assert!(tracker.should_emit(99, start + PROGRESS_INTERVAL));
    }

    #[test]
    fn a_snapshot_reports_fraction_rate_and_eta() {
        let start = Instant::now();
        let tracker = ProgressTracker::new("job", 100, start);
        let progress = tracker.snapshot(ExportStage::Encoding, 25, start + Duration::from_secs(5));

        assert_eq!(progress.frame, 25);
        assert_eq!(progress.total_frames, 100);
        assert!((progress.fraction - 0.25).abs() < 1e-6);
        assert!((progress.fps - 5.0).abs() < 1e-6);
        assert!((progress.elapsed_seconds - 5.0).abs() < 1e-3);
        assert!((progress.remaining_seconds.unwrap() - 15.0).abs() < 1e-2);
        assert_eq!(progress.stage, ExportStage::Encoding);
    }

    #[test]
    fn a_zero_frame_job_does_not_divide_by_zero() {
        let start = Instant::now();
        let tracker = ProgressTracker::new("job", 0, start);
        let progress = tracker.snapshot(ExportStage::Preparing, 0, start);
        assert_eq!(progress.fraction, 0.0);
        assert_eq!(progress.remaining_seconds, None);
    }

    // -- the frame walk ---------------------------------------------------

    #[test]
    fn the_walk_visits_every_frame_at_the_right_instant() {
        let cancel = AtomicBool::new(false);
        let mut seen: Vec<(u64, Micros)> = Vec::new();
        let count = walk_frames(Fps::THIRTY, 4, &cancel, |index, time| {
            seen.push((index, time));
            Ok(())
        })
        .unwrap();

        assert_eq!(count, 4);
        assert_eq!(seen, vec![(0, 0), (1, 33_333), (2, 66_667), (3, 100_000)]);
    }

    #[test]
    fn cancelling_mid_walk_stops_within_one_frame() {
        let cancel = AtomicBool::new(false);
        let mut rendered = 0u64;
        let error = walk_frames(Fps::THIRTY, 1_000, &cancel, |index, _| {
            rendered += 1;
            if index == 9 {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.is_cancellation());
        // Frame 9 finished, frame 10 never started.
        assert_eq!(rendered, 10);
    }

    #[test]
    fn a_job_cancelled_before_it_starts_renders_nothing() {
        let cancel = AtomicBool::new(true);
        let mut rendered = 0;
        let error = walk_frames(Fps::THIRTY, 100, &cancel, |_, _| {
            rendered += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(error.is_cancellation());
        assert_eq!(rendered, 0);
    }

    #[test]
    fn a_failing_frame_stops_the_walk_and_keeps_its_error() {
        let cancel = AtomicBool::new(false);
        let error = walk_frames(Fps::THIRTY, 100, &cancel, |index, _| {
            if index == 3 {
                Err(ExportError::Settings("the disk filled up".into()))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(!error.is_cancellation());
        assert!(error.to_string().contains("disk filled up"));
    }

    // -- the job registry -------------------------------------------------

    #[test]
    fn a_registered_job_can_be_cancelled_by_id() {
        let id = format!("test-{}", uuid::Uuid::new_v4());
        let flag = begin_job(&id);
        assert!(!flag.load(Ordering::Relaxed));

        assert!(cancel_job(&id));
        assert!(flag.load(Ordering::Relaxed));

        end_job(&id);
        // Cancelling a finished job is a no-op, not an error: the dialog can
        // close after the last frame and there is no way to prevent that race.
        assert!(!cancel_job(&id));
    }

    #[test]
    fn cancelling_one_job_does_not_touch_another() {
        let a = format!("test-{}", uuid::Uuid::new_v4());
        let b = format!("test-{}", uuid::Uuid::new_v4());
        let flag_a = begin_job(&a);
        let flag_b = begin_job(&b);

        cancel_job(&a);
        assert!(flag_a.load(Ordering::Relaxed));
        assert!(!flag_b.load(Ordering::Relaxed));

        end_job(&a);
        end_job(&b);
    }

    #[test]
    fn the_fallback_audio_source_answers_when_nothing_is_registered() {
        // Whatever else the test binary did, an export must always find a
        // source to ask; silence is the answer when nothing is wired up.
        let _ = audio_source();
    }
}
