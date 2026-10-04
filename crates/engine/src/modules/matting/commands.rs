//! The shell-facing "Remove background" API: what the app, the CLI and MCP
//! call.
//!
//! `matting_remove_background` is the toggle: one undoable edit that records
//! the setting (model and version) on the clip's compositing material, and,
//! when turning it on, a bake job that fills the matte cache for the clip's
//! source range. `matting_remove_background_with` picks the model (people
//! or objects); `matting_select_object` keeps the object under the user's
//! clicks (MobileSAM, propagated over the clip); `matting_set_invert` cuts
//! the matte's subject out instead of keeping it. The job is polled with `matting_status` and stopped with
//! `matting_cancel`; the preview shows each frame's matte as soon as it is
//! written, and the clip unmatted where none is yet. `matting_ensure` is the
//! export's blocking form: it bakes whatever is missing for the whole
//! project before the first frame is rendered, or fails saying why.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

use serde::Deserialize;

use super::bake::{self, BakeJob, BakeOutcome, BakeProgress};
use super::cache;
pub use super::BackgroundMode;
use crate::modules::compositing::commands::compositing_set_background;
use crate::modules::ml::{matte, segment};
use crate::modules::project::compositing::{
    BackgroundRemoval, MatteTarget, ObjectPrompt, PromptPoint,
};
use crate::modules::project::document::{Micros, Project, Segment};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// What the toggle answers: the edited project, and the bake it started.
#[derive(Serialize)]
pub struct BackgroundResponse {
    /// `None` when the clip already had this setting: nothing to undo, only
    /// frames to bake.
    #[serde(flatten)]
    pub edit: Option<EditResponse>,
    /// The bake job, when the matte had frames to make.
    pub job: Option<u64>,
}

/// A bake's state, as `matting_status` reports it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BakeStatus {
    pub segment_id: String,
    pub progress: BakeProgress,
    /// Set once the job has ended.
    pub finished: Option<Result<BakeOutcome, String>>,
}

/// How much of a clip's matte is baked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub baked: u32,
    pub total: u32,
}

struct Job {
    cancel: AtomicBool,
    status: Mutex<BakeStatus>,
    /// The cache key it fills, so a second bake of the same frames joins
    /// the running one instead of racing it.
    key: String,
}

/// Cache keys whose last bake failed, with why: `matting_queue_missing`
/// does not start them again after every edit (a selection that selects
/// nothing, an objects matte on a machine without a GPU). An explicit
/// `matting_bake` tries again.
fn failed() -> &'static Mutex<HashMap<String, String>> {
    static FAILED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FAILED.get_or_init(Default::default)
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// The bake for `segment` in `project`: its source range of its video file,
/// with the setting it has. `Err` in words when the clip cannot have its
/// background removed.
pub fn job_for(project: &Project, segment: &Segment) -> Result<BakeJob, String> {
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("Remove background works on video clips")?;
    let setting = project
        .materials
        .compositing_of(segment)
        .and_then(|m| m.background.clone())
        .ok_or("the clip does not have Remove background on")?;
    let range = (
        segment.source_range.start,
        (segment.source_range.end() - 1).max(segment.source_range.start),
    );
    Ok(BakeJob {
        path: video.path.clone(),
        fps: video.fps,
        source_size: (video.width, video.height),
        range,
        setting,
    })
}

/// The setting this build writes for people.
pub fn current_model() -> BackgroundRemoval {
    setting_for(BackgroundMode::People)
}

/// The setting this build writes for `mode`.
pub fn setting_for(mode: BackgroundMode) -> BackgroundRemoval {
    let model = mode.model();
    BackgroundRemoval {
        model: model.into(),
        version: matte::version_of(model)
            .expect("the registry lists every matting model")
            .into(),
        prompt: None,
        invert: false,
        cut: true,
        grade: MatteTarget::Whole,
        effects: MatteTarget::Whole,
    }
}

/// Turn "Remove background" (people) on or off for a video clip. On: one
/// undoable edit, then a bake of the clip's matte in the background (its
/// job id is in the answer; `None` when every frame was already baked).
/// Off: the edit only; the baked frames stay in the cache for an undo.
pub fn matting_remove_background(
    state: &Arc<AppState>,
    segment_id: String,
    enabled: bool,
) -> Result<BackgroundResponse, String> {
    matting_remove_background_with(state, segment_id, enabled.then_some(BackgroundMode::People))
}

/// Turn "Remove background" on with `mode`'s model, or off with `None`.
/// Switching from one model to another keeps the clip's "invert" choice and
/// where its grade and effects apply. Off keeps the matte (uncut) while the
/// grade or the effects still apply by it ([`matting_set_target`]).
pub fn matting_remove_background_with(
    state: &Arc<AppState>,
    segment_id: String,
    mode: Option<BackgroundMode>,
) -> Result<BackgroundResponse, String> {
    let current = current_setting(state, &segment_id)?;
    let setting = match mode {
        Some(mode) => Some(BackgroundRemoval {
            invert: current.as_ref().is_some_and(|s| s.invert),
            grade: current
                .as_ref()
                .map(|s| s.grade.clone())
                .unwrap_or_default(),
            effects: current
                .as_ref()
                .map(|s| s.effects.clone())
                .unwrap_or_default(),
            ..setting_for(mode)
        }),
        None => current
            .map(|s| BackgroundRemoval { cut: false, ..s })
            .filter(BackgroundRemoval::is_used),
    };
    set_and_bake(state, segment_id, setting)
}

/// Cut the clip by its matte ("Remove background" on) or stop cutting it,
/// keeping the matte as it is: its model, a selected object's clicks, and
/// where the grade and the effects apply. Off on a matte that steers
/// nothing else removes it. One undoable edit.
pub fn matting_set_cut(
    state: &Arc<AppState>,
    segment_id: String,
    cut: bool,
) -> Result<BackgroundResponse, String> {
    let current = current_setting(state, &segment_id)?
        .ok_or("the clip has no matte; turn Remove background on with a model")?;
    let setting = Some(BackgroundRemoval { cut, ..current }).filter(BackgroundRemoval::is_used);
    set_and_bake(state, segment_id, setting)
}

/// What a matte can steer besides the cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MattePart {
    /// The colour grade (the Adjust tab).
    Grade,
    /// The clip's effects.
    Effects,
}

impl MattePart {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "grade" | "colour" | "color" | "adjust" => Ok(MattePart::Grade),
            "effects" | "effect" | "fx" => Ok(MattePart::Effects),
            other => Err(format!("{other:?} is not grade or effects")),
        }
    }
}

/// Apply the clip's grade or its effects to the whole clip, only the
/// matte's subject, or only the rest ("grade only the person", "blur only
/// the background"), without duplicating the clip. A clip without a matte
/// gets the people matte (Robust Video Matting), uncut, which is baked in
/// the background; a clip with one (people, objects, a selected object)
/// uses it as it is. Back to the whole clip on a matte that neither cuts
/// nor steers anything else removes it. One undoable edit.
pub fn matting_set_target(
    state: &Arc<AppState>,
    segment_id: String,
    part: MattePart,
    target: MatteTarget,
) -> Result<BackgroundResponse, String> {
    if let MatteTarget::Other(name) = &target {
        return Err(format!("{name:?} is not whole, subject or background"));
    }
    let current = current_setting(state, &segment_id)?;
    let mut setting = match current.clone() {
        Some(setting) => setting,
        None if target.is_whole() => {
            return Ok(BackgroundResponse {
                edit: None,
                job: None,
            })
        }
        None => BackgroundRemoval {
            cut: false,
            ..setting_for(BackgroundMode::People)
        },
    };
    match part {
        MattePart::Grade => setting.grade = target,
        MattePart::Effects => setting.effects = target,
    }
    let setting = Some(setting).filter(BackgroundRemoval::is_used);
    set_and_bake(state, segment_id, setting)
}

/// A point on the canvas (fractions of its width and height, top-left
/// origin), as the player or a rendered frame shows it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanvasPoint {
    pub x: f32,
    pub y: f32,
    /// On the object to keep (`true`), or on a part to leave out.
    #[serde(default = "yes")]
    pub keep: bool,
}

fn yes() -> bool {
    true
}

/// "Select object": keep the object under `points` (canvas fractions, at
/// timeline time `time`) and remove the rest, on every frame of the clip.
/// The clicks are turned into points of the clip's source frame, recorded
/// as the setting (one undoable edit), and the matte is baked in the
/// background: the clicked frame first, then forwards and backwards over
/// the clip (`matting/object.rs`). `invert` cuts the object out instead;
/// `None` keeps the clip's current choice.
pub fn matting_select_object(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
    points: Vec<CanvasPoint>,
    invert: Option<bool>,
) -> Result<BackgroundResponse, String> {
    if !points.iter().any(|p| p.keep) {
        return Err("click at least once on the object to keep".into());
    }
    let (prompt, current) = state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        project
            .materials
            .video(&segment.material_id)
            .ok_or("Select object works on video clips")?;
        if !segment.target_range.contains(time) {
            return Err(
                "the playhead is not on the clip; move it onto the clip and click again".into(),
            );
        }
        let source_time = project
            .materials
            .time_map(segment)
            .clamped_source_time(time);
        let mut prompt = ObjectPrompt {
            time: source_time,
            points: Vec::new(),
        };
        for p in &points {
            // `canvas_to_source` works in clip space: -1..1, y up.
            let at = crate::modules::tracking::follow::canvas_to_source(
                project,
                segment,
                time,
                [p.x * 2.0 - 1.0, 1.0 - p.y * 2.0],
            )
            .ok_or("the clip is not visible on the canvas at the playhead")?;
            if !(0.0..=1.0).contains(&at[0]) || !(0.0..=1.0).contains(&at[1]) {
                return Err("a click is outside the clip's picture".to_string());
            }
            prompt.points.push(PromptPoint {
                x: at[0],
                y: at[1],
                keep: p.keep,
            });
        }
        let current = project
            .materials
            .compositing_of(segment)
            .and_then(|m| m.background.clone());
        Ok((prompt, current))
    })??;
    let setting = BackgroundRemoval {
        model: segment::MODEL.into(),
        version: segment::model_version().into(),
        prompt: Some(prompt),
        invert: invert.unwrap_or(current.as_ref().is_some_and(|s| s.invert)),
        cut: true,
        grade: current
            .as_ref()
            .map(|s| s.grade.clone())
            .unwrap_or_default(),
        effects: current.map(|s| s.effects).unwrap_or_default(),
    };
    set_and_bake(state, segment_id, Some(setting))
}

/// Cut the matte's subject out (`true`) or keep only it (`false`). One
/// undoable edit; the mattes are the same either way, so nothing re-bakes.
pub fn matting_set_invert(
    state: &Arc<AppState>,
    segment_id: String,
    invert: bool,
) -> Result<EditResponse, String> {
    let current = current_setting(state, &segment_id)?
        .ok_or("the clip does not have Remove background on")?;
    compositing_set_background(
        state,
        segment_id,
        Some(BackgroundRemoval { invert, ..current }),
    )
}

fn current_setting(
    state: &Arc<AppState>,
    segment_id: &str,
) -> Result<Option<BackgroundRemoval>, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        project
            .materials
            .video(&segment.material_id)
            .ok_or("Remove background works on video clips")?;
        Ok(project
            .materials
            .compositing_of(segment)
            .and_then(|m| m.background.clone()))
    })?
}

/// Record `setting` on the clip (one edit) and bake what it is missing.
fn set_and_bake(
    state: &Arc<AppState>,
    segment_id: String,
    setting: Option<BackgroundRemoval>,
) -> Result<BackgroundResponse, String> {
    let current = current_setting(state, &segment_id)?;
    let on = setting.is_some();
    // The same setting again (the same clicks, the same model) is a request
    // to bake what is missing, not an edit.
    let edit = if current == setting {
        None
    } else {
        Some(compositing_set_background(
            state,
            segment_id.clone(),
            setting,
        )?)
    };
    let job = if on {
        matting_bake(state, segment_id)?
    } else {
        None
    };
    Ok(BackgroundResponse { edit, job })
}

/// Bake whatever is missing of `segment_id`'s matte, in the background.
/// `None` when nothing is; a bake already running over the same frames is
/// joined (its id is returned) rather than started twice.
pub fn matting_bake(state: &Arc<AppState>, segment_id: String) -> Result<Option<u64>, String> {
    let job = state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        job_for(project, segment)
    })??;
    // A setting this build cannot bake fails now, in words, not on the
    // bake's thread.
    job.kind()?;
    let key = job.key()?;
    if cache::best(&key).is_some_and(|(_, times)| job.missing(&times).is_empty()) {
        return Ok(None);
    }
    {
        let jobs = jobs().lock();
        if let Some((id, _)) = jobs.iter().find(|(_, j)| {
            j.key == key && j.status.lock().finished.is_none() && !j.cancel.load(Ordering::Relaxed)
        }) {
            // One bake per file at a time: two would write the same frames
            // and share one worker anyway. The second clip's frames are
            // picked up by the next `matting_bake` the app makes when this
            // one ends.
            return Ok(Some(*id));
        }
    }
    failed().lock().remove(&key);
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
    let handle = Arc::new(Job {
        cancel: AtomicBool::new(false),
        status: Mutex::new(BakeStatus {
            segment_id: segment_id.clone(),
            ..BakeStatus::default()
        }),
        key,
    });
    jobs().lock().insert(id, Arc::clone(&handle));
    std::thread::Builder::new()
        .name("chukcut-matting".into())
        .spawn(move || {
            let result = bake::run(&job, &handle.cancel, |p| {
                handle.status.lock().progress = p.clone();
            });
            match &result {
                Ok(outcome) => tracing::info!(
                    written = outcome.written,
                    seconds = outcome.seconds,
                    provider = ?outcome.provider,
                    "matte baked"
                ),
                Err(error) => {
                    tracing::warn!(%error, "background removal failed");
                    failed().lock().insert(handle.key.clone(), error.clone());
                }
            }
            handle.status.lock().finished = Some(result);
        })
        .map_err(|e| format!("could not start the matting thread: {e}"))?;
    Ok(Some(id))
}

pub fn matting_status(job: u64) -> Option<BakeStatus> {
    jobs().lock().get(&job).map(|j| j.status.lock().clone())
}

/// Stop a bake. The frames written so far stay.
pub fn matting_cancel(job: u64) {
    if let Some(job) = jobs().lock().get(&job) {
        job.cancel.store(true, Ordering::Relaxed);
        // Stopped by the user: the next edit must not start it again on
        // its own ("Finish missing frames" does).
        failed().lock().insert(job.key.clone(), "stopped".into());
    }
}

/// Drop a finished job's record.
pub fn matting_forget(job: u64) {
    jobs().lock().remove(&job);
}

/// How much of `segment_id`'s matte is in the cache.
pub fn matting_coverage(state: &Arc<AppState>, segment_id: String) -> Result<Coverage, String> {
    let job = state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        job_for(project, segment)
    })??;
    coverage(&job)
}

fn coverage(job: &BakeJob) -> Result<Coverage, String> {
    let times = job.best()?.map(|(_, times)| times).unwrap_or_default();
    let total = job.frame_times(job.range.0, job.range.1).len() as u32;
    let missing = job.missing(&times).len() as u32;
    Ok(Coverage {
        baked: total.saturating_sub(missing),
        total,
    })
}

/// Bake every missing matte `project` needs, now, on this thread: what an
/// export calls before rendering, so a delivered file never has a
/// background the preview did not. `progress` gets a sentence and a
/// fraction of the whole. Fails in words when a matte cannot be made (no
/// ML worker, a model version this build lacks) — an export must not quietly
/// ship the background the user removed.
pub fn matting_ensure(
    project: &Project,
    progress: &dyn Fn(&str, f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut todo = Vec::new();
    // The timeline being exported, and the insides of compound clips, which
    // the compositor renders nested (other timelines are not exported).
    let compounds = project
        .materials
        .sequences
        .iter()
        .filter(|s| s.kind == crate::modules::sequence::SequenceKind::Compound)
        .flat_map(|s| &s.tracks);
    for track in project.tracks.iter().chain(compounds) {
        for segment in &track.segments {
            let on = project
                .materials
                .compositing_of(segment)
                .is_some_and(|m| m.background.is_some());
            if !on || project.materials.video(&segment.material_id).is_none() {
                continue;
            }
            let job = job_for(project, segment)?;
            let missing = coverage(&job)?;
            if missing.baked < missing.total {
                todo.push((job, missing.total - missing.baked));
            }
        }
    }
    if todo.is_empty() {
        return Ok(());
    }
    progress("Preparing background removal", 0.0);
    let total: u32 = todo.iter().map(|(_, n)| *n).sum::<u32>().max(1);
    let mut before = 0u32;
    for (job, frames) in &todo {
        let outcome = bake::run(job, cancel, |p| {
            let share = p.done as f32 / p.total.max(1) as f32 * *frames as f32;
            progress(
                "Removing backgrounds",
                ((before as f32 + share) / total as f32).min(1.0),
            );
        })
        .map_err(|e| {
            format!("Remove background is on for a clip, but its matte cannot be made: {e}")
        })?;
        if outcome.cancelled {
            return Err("cancelled".into());
        }
        before += frames;
    }
    Ok(())
}

/// Start a bake for every clip of the open project whose matte is missing
/// frames: after a trim made a clip longer, a speed change, an undo that
/// brought a clip back, or a cache clean-up. Clips whose bake runs already
/// are joined, not started twice. Returns the jobs. Reads the media files'
/// heads and lists the cache: call it off the UI thread.
pub fn matting_queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    let wanted: Vec<(String, BakeJob)> = state.with_project(|project| {
        let compounds = project
            .materials
            .sequences
            .iter()
            .filter(|s| s.kind == crate::modules::sequence::SequenceKind::Compound)
            .flat_map(|s| &s.tracks);
        project
            .tracks
            .iter()
            .chain(compounds)
            .flat_map(|t| &t.segments)
            .filter_map(|segment| {
                let job = job_for(project, segment).ok()?;
                Some((segment.id.clone(), job))
            })
            .collect()
    })?;
    let mut started = Vec::new();
    for (segment_id, job) in wanted {
        // A setting this build cannot bake (an old model version) is the
        // export's to report; it must not stop the others here.
        if job.kind().is_err() || coverage(&job).is_ok_and(|c| c.baked >= c.total) {
            continue;
        }
        if job
            .key()
            .is_ok_and(|key| failed().lock().contains_key(&key))
        {
            continue;
        }
        match matting_bake(state, segment_id) {
            Ok(Some(job)) => started.push(job),
            Ok(None) => {}
            Err(error) => tracing::debug!(%error, "no background bake"),
        }
    }
    started.sort_unstable();
    started.dedup();
    Ok(started)
}

/// The jobs running now, for the app's status: (job, segment id, progress).
pub fn matting_running() -> Vec<(u64, String, BakeProgress)> {
    let mut running: Vec<_> = jobs()
        .lock()
        .iter()
        .filter_map(|(id, j)| {
            let status = j.status.lock();
            status
                .finished
                .is_none()
                .then(|| (*id, status.segment_id.clone(), status.progress.clone()))
        })
        .collect();
    running.sort_by_key(|(id, _, _)| *id);
    running
}

/// What the matte cache holds, for Settings.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MatteCacheInfo {
    pub bytes: u64,
    /// Directories: one per clip source, model, selection and provider.
    pub sets: u32,
}

/// The size of the matte cache. Walks it: call it off the UI thread.
pub fn matting_cache_info() -> MatteCacheInfo {
    let root = cache::root();
    let sets = std::fs::read_dir(&root)
        .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count() as u32)
        .unwrap_or(0);
    MatteCacheInfo {
        bytes: crate::modules::ml::download::size_of(&root),
        sets,
    }
}

/// Delete every baked matte. Running bakes are stopped first; a clip with
/// Remove background on bakes again when it is next shown or exported.
pub fn matting_cache_clear() -> Result<(), String> {
    for job in jobs().lock().values() {
        job.cancel.store(true, Ordering::Relaxed);
    }
    let root = cache::root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .map_err(|e| format!("could not delete {}: {e}", root.display()))?;
    }
    Ok(())
}
