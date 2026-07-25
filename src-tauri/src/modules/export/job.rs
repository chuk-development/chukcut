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
}

/// A resolved, validated export. Everything the job needs and nothing it has to
/// decide.
#[derive(Debug, Clone)]
pub struct ExportSettings {
    pub output_path: PathBuf,
    pub preset: ExportPreset,
    pub video: VideoStreamSpec,
    pub audio: Option<AudioStreamSpec>,
    /// Frames the walk will produce, from the project duration at the output
    /// rate.
    pub total_frames: u64,
    pub duration: Micros,
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

    let duration = project.duration();
    if duration <= 0 {
        return Err(ExportError::Settings(
            "the timeline is empty, so there is nothing to export".into(),
        ));
    }
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
}

/// Render, encode and mux the whole timeline.
///
/// Blocking: the caller owns the thread. Sends progress as it goes and one
/// terminal message before returning.
pub fn run_export(job: &ExportJob, sink: &dyn ProgressSink) -> Result<ExportOutcome> {
    let settings = &job.settings;
    let mut tracker =
        ProgressTracker::new(job.job_id.clone(), settings.total_frames, Instant::now());
    sink.send(tracker.snapshot(ExportStage::Preparing, 0, Instant::now()));

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
        mixed = audio::mix_timeline(
            &job.project,
            job.audio.as_ref(),
            spec.sample_rate,
            spec.channels,
            &job.cancel,
        )?;
    }

    let audio_rate = settings.audio.as_ref().map(|a| a.sample_rate).unwrap_or(0);
    let mut audio_cursor = 0usize;

    walk_frames(fps, settings.total_frames, &job.cancel, |index, time| {
        let rgba = job
            .compositor
            .render_frame(&job.project, time, size, job.sources.as_ref())
            .map_err(|source| ExportError::Render {
                frame: index,
                source,
            })?;
        writer.write_video_frame(&rgba, index)?;

        // Hand the muxer the audio that belongs *before* the next video frame,
        // so the interleaving stays tight and a player never has to buffer
        // seconds of one stream to find the other.
        if !mixed.is_empty() {
            let until = audio::frames_for(fps.frame_time(index + 1), audio_rate)
                .min(mixed.len() / channels);
            if until > audio_cursor {
                writer.write_audio(&mixed[audio_cursor * channels..until * channels])?;
                audio_cursor = until;
            }
        }

        let now = Instant::now();
        if tracker.should_emit(index, now) {
            sink.send(tracker.snapshot(ExportStage::Encoding, index + 1, now));
        }
        Ok(())
    })?;

    // Whatever the last frame boundary did not cover — a mix is exactly as long
    // as the project, and the last frame starts before the project ends.
    if !mixed.is_empty() && audio_cursor * channels < mixed.len() {
        writer.write_audio(&mixed[audio_cursor * channels..])?;
    }

    Ok(settings.total_frames)
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
