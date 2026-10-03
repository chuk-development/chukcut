//! The shell-facing tracking API: what the app, a CLI and an MCP server call.
//!
//! Analysis runs as a background job (`tracking_start` returns at once with a
//! job id); its progress is polled with `tracking_status` or streamed through
//! a `Channel`, and `tracking_cancel` stops it keeping the frames done so far.
//! When a job ends, **one** edit puts the result into the document — a new
//! track (and the follow link, when an overlay asked for one) or the
//! re-tracked samples — so the undo history gets one step, not one per frame.
//!
//! Every other command is a plain undoable edit through
//! `DocumentHistory::apply_tracking`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::edit::{self, TrackingCommand};
use super::job::{self, Direction, TrackJob};
use super::model::{FollowMode, TrackSample, TrackSettings, TrackingMaterial, FLAG_ANCHOR};
use crate::modules::project::document::{Id, Micros, Project};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::shell::Channel;
use crate::state::AppState;

/// What to track, and what to do with the result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartTracking {
    /// The video clip whose pixels are tracked.
    pub target_segment_id: Id,
    /// Timeline time the box was drawn at.
    pub at: Micros,
    /// The box, in the target clip's **source** frame: centre x, centre y,
    /// width, height, as fractions. See `follow::canvas_to_source` for turning
    /// a box drawn on the player into this.
    pub rect: [f32; 4],
    #[serde(default)]
    pub direction: Direction,
    /// Attach this overlay to the new track when the job ends.
    #[serde(default)]
    pub overlay_id: Option<Id>,
    #[serde(default)]
    pub mode: FollowMode,
    /// Re-track this existing track from `at` instead of making a new one:
    /// the samples on the far side of `at` are replaced, the ones before kept.
    #[serde(default)]
    pub retrack: Option<Id>,
}

/// A job's state, as `tracking_status` reports it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct JobStatus {
    pub done: u32,
    pub total: u32,
    /// The newest sample, for the live box on the player.
    pub latest: Option<TrackSample>,
    /// The file the samples belong to.
    pub media_id: Id,
    /// Set once the job has ended.
    pub finished: Option<Result<JobOutcome, String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobOutcome {
    pub track_id: Id,
    pub frames: usize,
    pub cancelled: bool,
    /// Wall time of the analysis, for the speed the UI reports.
    pub seconds: f64,
}

/// One message on a job's channel.
#[derive(Debug, Clone, Serialize)]
pub struct TrackingEvent {
    pub job: u64,
    pub done: u32,
    pub total: u32,
    pub latest: Option<TrackSample>,
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<JobStatus>,
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// Start tracking. Returns the job id at once; the work runs on its own
/// thread and holds no lock while it decodes.
pub fn tracking_start(
    state: &Arc<AppState>,
    request: StartTracking,
    channel: Option<Channel<TrackingEvent>>,
) -> Result<u64, String> {
    let (job, existing) = state.with_project(|project| prepare(project, &request))??;
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let handle = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(JobStatus {
            media_id: existing.media_id.clone(),
            ..Default::default()
        }),
    });
    jobs().lock().insert(id, Arc::clone(&handle));

    let state = Arc::clone(state);
    std::thread::Builder::new()
        .name("chukcut-tracking".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let result = job::run(&job, &handle.cancel, |progress| {
                {
                    let mut status = handle.status.lock();
                    status.done = progress.done;
                    status.total = progress.total;
                    status.latest = progress.latest;
                }
                if let Some(channel) = &channel {
                    let _ = channel.send(TrackingEvent {
                        job: id,
                        done: progress.done,
                        total: progress.total,
                        latest: progress.latest,
                    });
                }
            });
            let seconds = started.elapsed().as_secs_f64();
            let finished = result.and_then(|outcome| {
                let frames = outcome.samples.len();
                tracing::info!(
                    frames,
                    seconds,
                    fps = frames as f64 / seconds.max(1e-6),
                    size = ?outcome.frame_size,
                    cancelled = outcome.cancelled,
                    "tracking finished"
                );
                let track_id = commit(&state, &request, &job, existing, outcome.samples)?;
                Ok(JobOutcome {
                    track_id,
                    frames,
                    cancelled: outcome.cancelled,
                    seconds,
                })
            });
            if let Err(error) = &finished {
                tracing::warn!(%error, "tracking failed");
            }
            handle.status.lock().finished = Some(finished);
        })
        .map_err(|e| format!("could not start the tracking thread: {e}"))?;
    Ok(id)
}

/// The job, ready to run, and the track it will write into (a fresh one, or
/// the one being re-tracked).
fn prepare(
    project: &Project,
    request: &StartTracking,
) -> Result<(TrackJob, TrackingMaterial), String> {
    let (_, target) = project
        .segment(&request.target_segment_id)
        .ok_or("the clip to track is no longer on the timeline")?;
    let video = project
        .materials
        .video(&target.material_id)
        .ok_or("only a video clip can be tracked")?;
    if !request.rect.iter().all(|v| v.is_finite())
        || request.rect[2] <= 0.0
        || request.rect[3] <= 0.0
    {
        return Err("draw a box around the object first".into());
    }
    let start = project
        .materials
        .time_map(target)
        .clamped_source_time(request.at);
    let range = (
        target.source_range.start,
        (target.source_range.end() - 1).max(target.source_range.start),
    );

    let (existing, direction, init_angle) = match &request.retrack {
        Some(track_id) => {
            let track = project
                .materials
                .tracking(track_id)
                .ok_or("that track is no longer in the project")?
                .clone();
            if track.media_id != video.id {
                return Err("that track belongs to another file".into());
            }
            let angle = track.pose_at(start).map_or(0.0, |p| p.angle);
            let direction = match request.direction {
                Direction::Both => Direction::Forward,
                other => other,
            };
            (track, direction, angle)
        }
        None => (
            TrackingMaterial::new(video.id.clone(), TrackSettings::default(), Vec::new()),
            request.direction,
            0.0,
        ),
    };
    let rect = request.rect.map(|v| v.clamp(-1.0, 2.0));
    let job = TrackJob {
        path: video.path.clone(),
        fps: video.fps,
        source_size: (video.width, video.height),
        start,
        init: rect,
        init_angle,
        range,
        direction,
        analysis_size: existing.settings.analysis_size.max(64),
    };
    Ok((job, existing))
}

/// Put a finished run into the document as one undo step.
fn commit(
    state: &Arc<AppState>,
    request: &StartTracking,
    job: &TrackJob,
    mut track: TrackingMaterial,
    samples: Vec<TrackSample>,
) -> Result<Id, String> {
    if samples.is_empty() {
        return Err("the tracker produced no frames".into());
    }
    let mut guard = state.project.write();
    let project = guard.as_mut().ok_or("no project is open")?;

    let command = if request.retrack.is_some() {
        // Only one side of the start is re-tracked; everything else stays.
        let forward = job.direction != Direction::Backward;
        let from = samples
            .iter()
            .find(|s| s.f & FLAG_ANCHOR != 0)
            .map_or(job.start, |s| s.t);
        track.splice(from, forward, samples);
        TrackingCommand::Composite {
            label: "Re-track".into(),
            commands: vec![edit::replace_track(project, track.clone())?],
        }
    } else {
        track.samples = samples;
        track.settings.analysis_size = job.analysis_size;
        let mut commands = vec![edit::add_track(track.clone())];
        if let Some(overlay) = &request.overlay_id {
            let mut scratch = project.clone();
            commands[0].apply(&mut scratch)?;
            match edit::attach(
                &scratch,
                overlay,
                &track.id,
                &request.target_segment_id,
                request.mode,
                request.at,
            ) {
                Ok(attach) => commands.push(attach),
                // The overlay went away while the job ran: keep the track.
                Err(error) => tracing::warn!(%error, "could not attach the overlay"),
            }
        }
        TrackingCommand::Composite {
            label: "Track object".into(),
            commands,
        }
    };
    state.history.write().apply_tracking(project, command)?;
    let snapshot = project.clone();
    drop(guard);
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&snapshot, origin);
    Ok(track.id)
}

/// A job's progress and, once it has ended, its result.
pub fn tracking_status(job: u64) -> Option<JobStatus> {
    jobs().lock().get(&job).map(|j| j.status.lock().clone())
}

/// Stop a job. The frames done so far are kept and committed.
pub fn tracking_cancel(job: u64) {
    if let Some(job) = jobs().lock().get(&job) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

/// Drop a finished job's record.
pub fn tracking_forget(job: u64) {
    jobs().lock().remove(&job);
}

fn apply(
    state: &Arc<AppState>,
    build: impl FnOnce(&Project) -> Result<TrackingCommand, String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = build(project)?;
        state.history.write().apply_tracking(project, command)?;
    }
    respond(state)
}

/// Make an overlay follow an existing track, without moving it at `at`.
pub fn tracking_attach(
    state: &Arc<AppState>,
    overlay_id: Id,
    track_id: Id,
    target_segment_id: Id,
    mode: FollowMode,
    at: Micros,
) -> Result<EditResponse, String> {
    apply(state, |p| {
        edit::attach(p, &overlay_id, &track_id, &target_segment_id, mode, at)
    })
}

/// Change how an overlay follows: position, plus scale, plus rotation.
pub fn tracking_set_mode(
    state: &Arc<AppState>,
    overlay_id: Id,
    mode: FollowMode,
) -> Result<EditResponse, String> {
    apply(state, |p| {
        let (_, overlay) = p
            .segment(&overlay_id)
            .ok_or("the clip is no longer on the timeline")?;
        let link = p
            .materials
            .follow_of(overlay)
            .ok_or("the clip does not follow a track")?
            .clone();
        if link.mode == mode {
            return Err("the clip already follows that way".into());
        }
        let mut changed = link.clone();
        changed.mode = mode;
        Ok(TrackingCommand::Composite {
            label: "Change tracking".into(),
            commands: vec![TrackingCommand::SetFollow {
                id: link.id.clone(),
                before: Some(link),
                after: Some(changed),
            }],
        })
    })
}

/// Stop an overlay following, leaving it where it is shown at `at`.
pub fn tracking_detach(
    state: &Arc<AppState>,
    overlay_id: Id,
    at: Micros,
) -> Result<EditResponse, String> {
    apply(state, |p| edit::detach(p, &overlay_id, at))
}

/// Replace the follow with keyframes on the overlay.
pub fn tracking_bake(state: &Arc<AppState>, overlay_id: Id) -> Result<EditResponse, String> {
    apply(state, |p| edit::bake(p, &overlay_id))
}

/// Delete clips, baking every overlay that follows one of them to keyframes
/// first, as one undo step. `delete` is what the delete gesture produced (the
/// removals and any ripple), exactly as `timeline_apply_many` takes it;
/// `deleted` names the clips going away, which decides who is baked.
pub fn tracking_bake_and_delete(
    state: &Arc<AppState>,
    deleted: Vec<Id>,
    delete: Vec<EditCommand>,
    label: String,
) -> Result<EditResponse, String> {
    apply(state, |p| {
        edit::bake_and_delete(p, &deleted, delete, &label)
    })
}

/// The overlays that follow one of `deleted` and would stop moving if those
/// clips went — what the delete prompt asks about.
pub fn tracking_dependent_followers(project: &Project, deleted: &[Id]) -> Vec<Id> {
    super::validate::dependent_followers(project, deleted)
}

/// Delete a track; its followers stay where they are at `at`.
pub fn tracking_remove(
    state: &Arc<AppState>,
    track_id: Id,
    at: Micros,
) -> Result<EditResponse, String> {
    apply(state, |p| edit::remove_track(p, &track_id, at))
}

/// Set a track's smoothing, `0..1`.
pub fn tracking_set_smoothing(
    state: &Arc<AppState>,
    track_id: Id,
    smoothing: f32,
) -> Result<EditResponse, String> {
    if !smoothing.is_finite() {
        return Err("smoothing must be a number".into());
    }
    apply(state, |p| {
        let mut track = p
            .materials
            .tracking(&track_id)
            .ok_or("that track is no longer in the project")?
            .clone();
        let smoothing = smoothing.clamp(0.0, 1.0);
        if (track.settings.smoothing - smoothing).abs() < 1e-4 {
            return Err("nothing to change".into());
        }
        track.settings.smoothing = smoothing;
        Ok(TrackingCommand::Composite {
            label: "Smooth track".into(),
            commands: vec![edit::replace_track(p, track)?],
        })
    })
}

/// The reply to an edit, with the working copy written on the way out — the
/// same as every other module's `respond`.
fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);
    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::timeline::commands::{timeline_redo, timeline_undo};

    fn state() -> Arc<AppState> {
        let state = AppState::new();
        let mut project = super::super::follow::tests::project();
        project.materials.follows.clear();
        project.tracks[1].segments[0].extras.clear();
        *state.project.write() = Some(project);
        state
    }

    fn follows(state: &AppState) -> bool {
        state
            .with_project(|p| {
                let (_, overlay) = p.segment("o").unwrap();
                p.materials.follow_of(overlay).is_some()
            })
            .unwrap()
    }

    #[test]
    fn tracking_edits_share_the_one_undo_stack() {
        let state = state();
        let response = tracking_attach(
            &state,
            "o".into(),
            "track".into(),
            "v".into(),
            FollowMode::Position,
            0,
        )
        .unwrap();
        assert_eq!(response.undo_label.as_deref(), Some("Follow track"));
        assert!(follows(&state));
        tracking_set_smoothing(&state, "track".into(), 0.4).unwrap();

        timeline_undo(&state).unwrap();
        let smoothing = state
            .with_project(|p| p.materials.tracking("track").unwrap().settings.smoothing)
            .unwrap();
        assert_eq!(smoothing, 0.0);
        timeline_undo(&state).unwrap();
        assert!(!follows(&state));
        timeline_redo(&state).unwrap();
        assert!(follows(&state));

        tracking_bake(&state, "o".into()).unwrap();
        assert!(!follows(&state));
        timeline_undo(&state).unwrap();
        assert!(follows(&state));
    }

    #[test]
    fn a_refused_edit_is_not_recorded() {
        let state = state();
        assert!(tracking_detach(&state, "o".into(), 0).is_err());
        assert!(!state.history.read().can_undo());
    }
}
