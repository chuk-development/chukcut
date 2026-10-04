//! The shell-facing analysis API: what the app, a CLI and an MCP server call.
//!
//! The four analyses are background jobs: `analysis_detect_scenes`,
//! `analysis_stabilise`, `analysis_detect_beats` and `analysis_reframe`
//! return a job id at once; `analysis_status` / `analysis_jobs` report
//! progress, `analysis_cancel` stops one, `analysis_wait` blocks for a CLI.
//! When a job ends, **one** edit puts its result into the document.
//!
//! The edits that use a result — split at scene changes, the stabilisation
//! settings, auto-cut to beat, snap cuts to beats — are plain undoable
//! commands that return at once.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::edits;
use super::frames::Walk;
use super::jobs::{self, JobContext, JobEvent, JobKind, JobStatus};
use super::reframe::{self, Axis, PathPoint};
use super::scenes;
use super::stabilise::{self, Applied};
use super::store::{self, Beats, CameraPath, NewEntry, SceneCuts, Stabilise};
use crate::modules::ml::faces::FaceDetector;
use crate::modules::ml::MlError;
use crate::modules::project::configure::{ConfigureCommand, ProjectConfig};
use crate::modules::project::document::{Id, Micros, Project, TimeRange, TrackKind};
use crate::modules::sequence;
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::modules::voice::cleanup::{audible_segment, original_source};
use crate::shell::Channel;
use crate::state::AppState;

pub use jobs::JobStatus as AnalysisJob;

// --- jobs -----------------------------------------------------------------------

/// A job's progress and, once it has ended, its result.
pub fn analysis_status(job: u64) -> Option<JobStatus> {
    jobs::status(job)
}

/// Every analysis job the shell has not forgotten yet, oldest first.
pub fn analysis_jobs() -> Vec<JobStatus> {
    jobs::all()
}

/// Stop a job; nothing it found is committed.
pub fn analysis_cancel(job: u64) {
    jobs::cancel(job)
}

/// Drop a finished job's record.
pub fn analysis_forget(job: u64) {
    jobs::forget(job)
}

/// Block until a job ends. For the CLI and tests.
pub fn analysis_wait(job: u64) -> Result<String, String> {
    jobs::wait(job)
}

// --- shared plumbing -------------------------------------------------------------

/// The picture `segment_id` shows, for a frame walk: path, frame rate, the
/// clip's source range, and the media id.
fn video_of(project: &Project, segment_id: &str) -> Result<(String, f64, TimeRange, Id), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("only a video clip can be analysed for that")?;
    let source = segment
        .source_range
        .intersect(&TimeRange::new(0, video.duration.max(1)));
    let source = source.ok_or("the clip shows none of its file")?;
    Ok((video.path.clone(), video.fps, source, video.id.clone()))
}

/// What a picture analysis reads for one clip: a video's file, or a
/// compound clip's sequence rendered. Either way `range` and every time the
/// analysis finds are the clip's source time, so its time map — speed and
/// speed curve included — puts the results on the timeline.
struct Picture {
    path: String,
    fps: f64,
    range: TimeRange,
    media_id: Id,
    sequence: Option<Arc<Project>>,
}

impl Picture {
    fn walk(&self, height: u32, max_rate: Option<f64>) -> Walk {
        Walk {
            path: self.path.clone(),
            fps: self.fps,
            range: self.range,
            height,
            max_rate,
            sequence: self.sequence.clone(),
        }
    }
}

/// [`Picture`] of a video clip or a compound clip. A compound clip is drawn
/// on `canvas` — the project's when `None`.
fn picture_of(
    project: &Project,
    segment_id: &str,
    canvas: Option<(u32, u32)>,
) -> Result<Picture, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let id = &segment.material_id;
    if project.materials.sequence(id).is_none() {
        let (path, fps, range, media_id) = video_of(project, segment_id)
            .map_err(|_| "only a video or a compound clip can be analysed for that".to_string())?;
        return Ok(Picture {
            path,
            fps,
            range,
            media_id,
            sequence: None,
        });
    }
    let mut view = sequence::nested(project, id).ok_or("that compound clip's contents are gone")?;
    if let Some((w, h)) = canvas {
        view.canvas.width = w.max(2);
        view.canvas.height = h.max(2);
    }
    let range = segment
        .source_range
        .intersect(&TimeRange::new(0, view.duration().max(1)))
        .ok_or("the compound clip shows none of its contents")?;
    let size = (view.canvas.width, view.canvas.height);
    let digest = sequence::digest::digest(&project.materials, size, id).unwrap_or(0);
    Ok(Picture {
        path: format!("{}{id}#{digest:016x}", super::cache::SEQUENCE_PREFIX),
        fps: project.fps,
        range,
        media_id: id.clone(),
        sequence: Some(Arc::new(view)),
    })
}

/// Put `entries` into the pool, then apply what `build` makes of the
/// document, as one undo step. The entries come back out if the edit is
/// refused, so a refusal leaves the document exactly as it was.
fn commit(
    state: &Arc<AppState>,
    entries: Vec<NewEntry>,
    build: impl FnOnce(&Project) -> Result<EditCommand, String>,
) -> Result<(), String> {
    let mut guard = state.project.write();
    let project = guard.as_mut().ok_or("no project is open")?;
    let ids: Vec<Id> = entries.iter().map(|(id, _)| id.clone()).collect();
    for (id, value) in entries {
        project.materials.extras.insert(id, value);
    }
    let result = build(project).and_then(|command| state.history.write().apply(project, command));
    if result.is_err() {
        for id in &ids {
            project.materials.extras.remove(id);
        }
    }
    result?;
    let snapshot = project.clone();
    drop(guard);
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&snapshot, origin);
    Ok(())
}

fn edit(
    state: &Arc<AppState>,
    entries: Vec<NewEntry>,
    build: impl FnOnce(&Project) -> Result<EditCommand, String>,
) -> Result<EditResponse, String> {
    commit(state, entries, build)?;
    crate::modules::voice::commands::respond(state)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

// --- scenes ----------------------------------------------------------------------

/// What to detect scenes in, and whether to cut there straight away.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectScenes {
    pub segment_id: Id,
    /// `0..=1`; 0.5 finds hard cuts and ignores fast motion.
    #[serde(default = "half")]
    pub sensitivity: f32,
    /// Also split the clip at every cut found, in the same undo step.
    #[serde(default)]
    pub split: bool,
}

fn half() -> f32 {
    0.5
}

/// Find the shot changes in a video clip and mark them on it.
pub fn analysis_detect_scenes(
    state: &Arc<AppState>,
    request: DetectScenes,
    channel: Option<Channel<JobEvent>>,
) -> Result<u64, String> {
    let picture = state.with_project(|p| picture_of(p, &request.segment_id, None))??;
    let state = Arc::clone(state);
    let segment_id = request.segment_id.clone();
    jobs::spawn(JobKind::Scenes, segment_id.clone(), channel, move |ctx| {
        let walk = picture.walk(scenes::ANALYSIS_HEIGHT, None);
        let (range, media_id) = (picture.range, picture.media_id.clone());
        let scores = scenes::score(&walk, Some(ctx))?;
        let cuts: Vec<Micros> = scenes::cut_times(&scores, request.sensitivity)
            .into_iter()
            .filter(|&t| t > range.start && t < range.end())
            .collect();
        let count = cuts.len();
        let entry = store::new_entry(&SceneCuts {
            media_id,
            analysed: range,
            cuts,
            sensitivity: request.sensitivity.clamp(0.0, 1.0),
        });
        let id = entry.0.clone();
        let split = request.split && count > 0;
        let label = if split {
            "Split at scene changes"
        } else {
            "Detect scenes"
        };
        commit(&state, vec![entry], |project| {
            let swap = store::swap_entry(project, &segment_id, store::SCENES, Some(&id), label)?;
            if !split {
                return Ok(swap);
            }
            let mut scratch = project.clone();
            swap.apply(&mut scratch)?;
            let (_, segment) = scratch
                .segment(&segment_id)
                .ok_or("the clip is no longer on the timeline")?;
            let times = edits::scene_cut_times(&scratch, segment);
            let cut = edits::split_at_times(&scratch, &segment_id, &times, label)?;
            Ok(EditCommand::Composite {
                label: label.into(),
                commands: vec![swap, cut],
            })
        })?;
        Ok(match (count, split) {
            (0, _) => "No scene changes found".into(),
            (n, true) => format!("Split at {}", plural(n, "scene change", "scene changes")),
            (n, false) => format!("Found {}", plural(n, "scene change", "scene changes")),
        })
    })
}

/// Split a clip at the scene changes found in it, as one undo step.
pub fn analysis_split_at_scenes(
    state: &Arc<AppState>,
    segment_id: Id,
) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        if store::entry_of::<SceneCuts>(project, segment).is_none() {
            return Err("detect the scenes of this clip first".into());
        }
        let times = edits::scene_cut_times(project, segment);
        edits::split_at_times(project, &segment_id, &times, "Split at scene changes")
    })
}

/// Forget a clip's scene changes.
pub fn analysis_clear_scenes(
    state: &Arc<AppState>,
    segment_id: Id,
) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |p| {
        store::swap_entry(p, &segment_id, store::SCENES, None, "Clear scene marks")
    })
}

// --- stabilisation ---------------------------------------------------------------

/// Measure a clip's camera shake and stabilise it with `strength` (`0..=1`)
/// and `crop` (`None`: just enough). A clip measured before is not measured
/// again (the camera path is cached). A compound clip is measured on its
/// rendered contents.
pub fn analysis_stabilise(
    state: &Arc<AppState>,
    segment_id: Id,
    strength: f32,
    crop: Option<f32>,
    channel: Option<Channel<JobEvent>>,
) -> Result<u64, String> {
    if !strength.is_finite() || crop.is_some_and(|c| !c.is_finite()) {
        return Err("strength and crop must be numbers".into());
    }
    // A video's file, or a compound clip's contents rendered on the canvas
    // it is drawn on: the camera path is measured in the clip's source time
    // either way, and `stabilise::resolve` reads it back through the clip's
    // time map, so a compound clip's speed and curve are followed.
    let picture = state.with_project(|p| picture_of(p, &segment_id, None))??;
    let state = Arc::clone(state);
    let id = segment_id.clone();
    jobs::spawn(
        JobKind::Stabilise,
        segment_id,
        channel,
        move |ctx: &JobContext| {
            let walk = picture.walk(stabilise::ANALYSIS_HEIGHT, None);
            let camera = stabilise::measure(&walk, &picture.media_id, Some(ctx))?;
            let settings = Stabilise {
                motion_id: String::new(),
                enabled: true,
                strength: strength.clamp(0.0, 1.0),
                crop: crop.map(|c| c.clamp(0.0, stabilise::MAX_CROP)),
            };
            let motion = store::new_entry(&camera);
            let settings = Stabilise {
                motion_id: motion.0.clone(),
                ..settings
            };
            let applied = Applied::build(&settings, &camera);
            let entry = store::new_entry(&settings);
            let entry_id = entry.0.clone();
            commit(&state, vec![motion, entry], |project| {
                store::swap_entry(project, &id, store::STABILISE, Some(&entry_id), "Stabilise")
            })?;
            Ok(format!(
                "Stabilised · cropped {:.0} %",
                applied.crop * 100.0
            ))
        },
    )
}

/// Change how a stabilised clip is stabilised: on or off, strength, crop.
/// `None` keeps a value; `crop: Some(None)` makes the crop automatic.
pub fn analysis_set_stabilise(
    state: &Arc<AppState>,
    segment_id: Id,
    enabled: Option<bool>,
    strength: Option<f32>,
    crop: Option<Option<f32>>,
) -> Result<EditResponse, String> {
    let (current, label) = state.with_project(|p| {
        let (_, segment) = p
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        let (_, current) = stabilise::settings_of(p, segment)
            .ok_or("this clip has not been analysed for stabilisation yet")?;
        let label = match enabled {
            Some(true) => "Stabilise",
            Some(false) => "Turn off stabilisation",
            None => "Change stabilisation",
        };
        Ok::<_, String>((current, label))
    })??;
    let next = Stabilise {
        enabled: enabled.unwrap_or(current.enabled),
        strength: strength
            .filter(|s| s.is_finite())
            .map_or(current.strength, |s| s.clamp(0.0, 1.0)),
        crop: crop.map_or(current.crop, |c| {
            c.filter(|v| v.is_finite())
                .map(|v| v.clamp(0.0, stabilise::MAX_CROP))
        }),
        motion_id: current.motion_id.clone(),
    };
    if next == current {
        return Err("nothing to change".into());
    }
    let entry = store::new_entry(&next);
    let id = entry.0.clone();
    edit(state, vec![entry], |p| {
        store::swap_entry(p, &segment_id, store::STABILISE, Some(&id), label)
    })
}

/// Take a clip's stabilisation away entirely.
pub fn analysis_remove_stabilise(
    state: &Arc<AppState>,
    segment_id: Id,
) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |p| {
        store::swap_entry(
            p,
            &segment_id,
            store::STABILISE,
            None,
            "Remove stabilisation",
        )
    })
}

// --- beats -----------------------------------------------------------------------

/// Find the beats of a clip's sound and mark them on the clip that plays it.
pub fn analysis_detect_beats(
    state: &Arc<AppState>,
    segment_id: Id,
    channel: Option<Channel<JobEvent>>,
) -> Result<u64, String> {
    let (sound_id, input, range, media_id) = state.with_project(|p| {
        let sound = audible_segment(p, &segment_id).ok_or("the clip has no sound")?;
        let (_, heard) = p
            .segment(&sound)
            .ok_or("the clip is no longer on the timeline")?;
        // A compound clip's sound is its contents' mix, in its sequence's
        // time — the compound clip's source time.
        if let Some(duration) = sequence::duration_of(p, &heard.material_id)
            .filter(|_| p.materials.sequence(&heard.material_id).is_some())
        {
            let range = heard
                .source_range
                .intersect(&TimeRange::new(0, duration.max(1)))
                .ok_or("the compound clip plays none of its contents")?;
            let input = Sound::Mix(Box::new(p.clone()));
            return Ok::<_, String>((sound.clone(), input, range, heard.material_id.clone()));
        }
        let (path, duration) = original_source(p, heard).ok_or("the clip has no sound")?;
        let range = heard
            .source_range
            .intersect(&TimeRange::new(0, duration.max(1)))
            .ok_or("the clip plays none of its sound")?;
        Ok((
            sound.clone(),
            Sound::File(path),
            range,
            heard.material_id.clone(),
        ))
    })??;
    let state = Arc::clone(state);
    jobs::spawn(JobKind::Beats, sound_id.clone(), channel, move |ctx| {
        let mono = match &input {
            Sound::File(path) => {
                super::beats::decode(path, range, ctx.cancel_flag(), |f| ctx.progress(f * 0.8))?
            }
            Sound::Mix(project) => {
                let stereo = sequence::audio::mix_of(
                    project,
                    &media_id,
                    range,
                    super::beats::RATE,
                    ctx.cancel_flag(),
                )
                .map_err(|e| {
                    if ctx.cancelled() {
                        jobs::CANCELLED.into()
                    } else {
                        e
                    }
                })?;
                ctx.progress(0.8);
                stereo
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|frame| (frame[0] + frame[1]) / 2.0)
                    .collect()
            }
        };
        if ctx.cancelled() {
            return Err(jobs::CANCELLED.into());
        }
        let grid = super::beats::detect(&mono).ok_or("no steady beat was found in this sound")?;
        ctx.progress(0.95);
        let beats: Vec<Micros> = grid
            .beats
            .iter()
            .map(|t| range.start + t)
            .filter(|t| *t < range.end())
            .collect();
        let count = beats.len();
        let bpm = (grid.bpm * 10.0).round() / 10.0;
        let entry = store::new_entry(&Beats {
            media_id,
            analysed: range,
            beats,
            bpm,
        });
        let id = entry.0.clone();
        commit(&state, vec![entry], |p| {
            store::swap_entry(p, &sound_id, store::BEATS, Some(&id), "Detect beats")
        })?;
        Ok(format!("{bpm:.0} BPM · {}", plural(count, "beat", "beats")))
    })
}

/// Where beat detection hears a clip.
enum Sound {
    File(String),
    /// A compound clip: the document, whose sequence `media_id` is mixed.
    Mix(Box<Project>),
}

/// Forget a clip's beats.
pub fn analysis_clear_beats(state: &Arc<AppState>, segment_id: Id) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |p| {
        let sound = audible_segment(p, &segment_id).unwrap_or(segment_id.clone());
        store::swap_entry(p, &sound, store::BEATS, None, "Clear beat marks")
    })
}

/// Split the video clips `segment_ids` on every `every`-th beat on the
/// timeline, as one undo step.
pub fn analysis_auto_cut_to_beat(
    state: &Arc<AppState>,
    segment_ids: Vec<Id>,
    every: usize,
) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |p| {
        edits::auto_cut_to_beat(p, &segment_ids, every)
    })
}

/// Roll each cut the clips `segment_ids` touch onto the nearest beat within
/// `tolerance`, as one undo step.
pub fn analysis_snap_cuts_to_beats(
    state: &Arc<AppState>,
    segment_ids: Vec<Id>,
    tolerance: Micros,
) -> Result<EditResponse, String> {
    edit(state, Vec::new(), |p| {
        edits::snap_cuts_to_beats(p, &segment_ids, tolerance.max(0))
    })
}

// --- reframe ---------------------------------------------------------------------

/// What to reframe, and for which shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reframe {
    pub segment_ids: Vec<Id>,
    /// Switch the project to this shape (width, height) in the same undo
    /// step; `None` reframes for the canvas the project has.
    #[serde(default)]
    pub ratio: Option<(u32, u32)>,
    /// What the window follows. `Auto` uses faces when the ML worker can
    /// run the face detector (downloading it on first use) and saliency
    /// otherwise.
    #[serde(default)]
    pub subject: SubjectCue,
}

/// What auto reframe looks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectCue {
    /// Faces when the ML worker is available, saliency otherwise.
    #[default]
    Auto,
    /// Faces; the job fails when the face detector cannot run.
    Faces,
    /// Motion, contrast and skin tone only; no model, no download.
    Saliency,
}

/// The clips "reframe the project" takes over: pictures on visible video
/// lanes that fill their frame the plain way.
pub fn reframe_candidates(project: &Project) -> Vec<Id> {
    project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.hidden && !t.locked)
        .flat_map(|t| t.segments.iter())
        .filter(|s| {
            let pool = &project.materials;
            (pool.video(&s.material_id).is_some()
                || pool.image(&s.material_id).is_some()
                || pool.sequence(&s.material_id).is_some())
                && !pool.is_effect_clip(s)
                && edits::is_full_frame(s)
        })
        .map(|s| s.id.clone())
        .collect()
}

struct ReframeClip {
    segment_id: Id,
    axis: Axis,
    /// The window as a fraction of the uncropped frame.
    fraction: f32,
    /// The frames to walk; `None` for a still, which is filled and centred.
    walk: Option<Walk>,
}

/// Find the subject of each clip and move a window of the new shape along
/// it, written as position keyframes on the clip filled to the canvas. With a
/// `ratio`, the project switches shape in the same undo step.
pub fn analysis_reframe(
    state: &Arc<AppState>,
    request: Reframe,
    channel: Option<Channel<JobEvent>>,
) -> Result<u64, String> {
    let (clips, config) = state.with_project(|p| {
        let mut config = ProjectConfig::of(p);
        if let Some(ratio) = request.ratio {
            let (w, h) = edits::canvas_for_ratio((p.canvas.width, p.canvas.height), ratio);
            config.width = w;
            config.height = h;
        }
        let canvas_aspect = config.width as f32 / config.height.max(1) as f32;
        let mut clips = Vec::new();
        for id in &request.segment_ids {
            let Some((_, segment)) = p.segment(id) else {
                continue;
            };
            let Some(aspect) = edits::picture_aspect(p, segment) else {
                continue;
            };
            let Some((axis, fraction)) = reframe::window_fraction(aspect, canvas_aspect) else {
                continue;
            };
            let crop = crate::modules::render::layout::crop_uv(segment.crop)
                .unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let extent = match axis {
                Axis::Horizontal => crop[2] - crop[0],
                Axis::Vertical => crop[3] - crop[1],
            };
            // A compound clip is read on a canvas of its contents' shape,
            // so the frames are the picture the window moves across.
            let content = edits::content_size(p, segment);
            let walk = picture_of(p, id, content)
                .ok()
                .map(|picture| picture.walk(reframe::ANALYSIS_HEIGHT, Some(reframe::RATE)));
            clips.push(ReframeClip {
                segment_id: id.clone(),
                axis,
                fraction: fraction * extent,
                walk,
            });
        }
        (clips, config)
    })?;
    if clips.is_empty() && request.ratio.is_none() {
        return Err("the clip already has the canvas's shape".into());
    }
    let owner = request
        .segment_ids
        .first()
        .cloned()
        .unwrap_or_else(|| "project".into());
    let state = Arc::clone(state);
    let switch = request.ratio.is_some();
    let cue = request.subject;
    jobs::spawn(JobKind::Reframe, owner, channel, move |ctx| {
        // The face detector, when it is wanted and can run. Preparing it may
        // download the model and a runtime on first use; `Auto` falls back
        // to saliency on any failure, `Faces` fails the job.
        let wants_faces = cue != SubjectCue::Saliency && clips.iter().any(|c| c.walk.is_some());
        let mut fallback: Option<String> = None;
        let detector = if wants_faces {
            match FaceDetector::prepare(&|_, _| {}, ctx.cancel_flag()) {
                Ok(detector) => Some(detector),
                Err(MlError::Cancelled) => return Err(jobs::CANCELLED.into()),
                Err(e) if cue == SubjectCue::Faces => return Err(e.to_string()),
                Err(e) => {
                    tracing::info!("auto reframe without faces: {e}");
                    fallback = Some(e.to_string());
                    None
                }
            }
        } else {
            None
        };
        let total = clips.len().max(1) as f32;
        let mut paths: Vec<(Id, Axis, Vec<PathPoint>)> = Vec::new();
        let (mut face_frames, mut frames) = (0usize, 0usize);
        for (i, clip) in clips.iter().enumerate() {
            let span = (i as f32 / total, (i + 1) as f32 / total);
            let path = match &clip.walk {
                Some(walk) => {
                    let mut walk = walk.clone();
                    let mut detect = |rgba: &[u8], w: usize, h: usize| {
                        let detector = detector.as_ref()?;
                        match detector.detect(rgba, w, h) {
                            Ok(faces) => Some(
                                faces
                                    .iter()
                                    .map(|f| reframe::Subject {
                                        bbox: [
                                            f.bbox[0] / w as f32,
                                            f.bbox[1] / h as f32,
                                            f.bbox[2] / w as f32,
                                            f.bbox[3] / h as f32,
                                        ],
                                        score: f.score,
                                    })
                                    .collect(),
                            ),
                            Err(e) => {
                                tracing::warn!("face detection failed on a frame: {e}");
                                None
                            }
                        }
                    };
                    let faces: Option<reframe::FaceCue> = if detector.is_some() {
                        walk.height = crate::modules::ml::faces::DETECTION_HEIGHT;
                        Some(&mut detect)
                    } else {
                        None
                    };
                    let analysis =
                        reframe::analyse(&walk, clip.axis, clip.fraction, Some(ctx), span, faces)?;
                    face_frames += analysis.face_frames;
                    frames += analysis.targets.len();
                    let smooth = reframe::smooth(&analysis, clip.fraction);
                    reframe::simplify(&smooth, 0.004)
                }
                None => Vec::new(),
            };
            paths.push((clip.segment_id.clone(), clip.axis, path));
        }
        let count = paths.len();
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let configure = ConfigureCommand::new(project, config.clone());
        // The keyframes are computed against the canvas they are for.
        let mut scratch = project.clone();
        if switch {
            configure.apply(&mut scratch)?;
        }
        let mut commands = Vec::new();
        for (id, axis, path) in &paths {
            let Ok(command) = edits::reframe_command(&scratch, id, *axis, path) else {
                continue;
            };
            command.apply(&mut scratch)?;
            commands.push(command);
        }
        let label = if switch { "Reframe" } else { "Auto reframe" };
        let edit = EditCommand::Composite {
            label: label.into(),
            commands,
        };
        if switch && !configure.is_noop() {
            state.history.write().apply_configure_and_edit(
                project,
                configure,
                edit,
                label.into(),
            )?;
        } else {
            state.history.write().apply(project, edit)?;
        }
        let snapshot = project.clone();
        drop(guard);
        let origin = state.project_path.read().clone();
        crate::modules::project::autosave::schedule(&snapshot, origin);
        let mut message = if switch {
            format!(
                "Reframed to {}×{} · {}",
                config.width,
                config.height,
                plural(count, "clip", "clips")
            )
        } else {
            format!("Reframed {}", plural(count, "clip", "clips"))
        };
        if let Some(detector) = &detector {
            if face_frames > 0 {
                message.push_str(&format!(
                    " · followed faces in {}% of frames ({})",
                    (face_frames * 100 / frames.max(1)),
                    detector.provider
                ));
            } else {
                message.push_str(" · no faces found");
            }
        } else if let Some(why) = fallback {
            tracing::info!("reframed on saliency: {why}");
        }
        Ok(message)
    })
}

// --- what a clip carries, for drawing ---------------------------------------------

/// The analysis a clip carries, in timeline time, for the timeline and the
/// inspector to draw.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClipAnalysis {
    /// Scene changes inside the clip.
    pub scenes: Vec<Micros>,
    /// Beats inside the clip.
    pub beats: Vec<Micros>,
    pub bpm: Option<f32>,
    /// The clip's stabilisation, when it has been analysed.
    pub stabilise: Option<Stabilise>,
}

/// What `segment_id` carries. Cheap; the UI may call it per frame.
pub fn analysis_of(project: &Project, segment_id: &str) -> ClipAnalysis {
    let Some((_, segment)) = project.segment(segment_id) else {
        return ClipAnalysis::default();
    };
    if segment.extras.is_empty() || project.materials.extras.is_empty() {
        return ClipAnalysis::default();
    }
    ClipAnalysis {
        scenes: edits::scene_cut_times(project, segment),
        beats: edits::segment_beats(project, segment),
        bpm: store::entry_of::<Beats>(project, segment).map(|(_, b)| b.bpm),
        stabilise: stabilise::settings_of(project, segment).map(|(_, s)| s),
    }
}

/// Whether the camera path of a clip's stabilisation covers what it shows
/// now; a clip extended past what was measured holds the last correction.
pub fn stabilise_covers(project: &Project, segment_id: &str) -> bool {
    let Some((_, segment)) = project.segment(segment_id) else {
        return false;
    };
    let Some((_, settings)) = stabilise::settings_of(project, segment) else {
        return false;
    };
    store::entry::<CameraPath>(project, &settings.motion_id).is_some_and(|p| {
        p.analysed.start <= segment.source_range.start
            && p.analysed.end() >= segment.source_range.end()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::timeline::commands::{timeline_redo, timeline_undo};

    fn state() -> Arc<AppState> {
        let state = AppState::new();
        *state.project.write() = Some(edits::tests::project());
        state
    }

    fn put_path(state: &AppState) -> Id {
        let path = CameraPath {
            media_id: "v".into(),
            analysed: TimeRange::new(0, 20_000_000),
            aspect: 16.0 / 9.0,
            samples: vec![],
        };
        let (id, value) = store::new_entry(&path);
        state
            .with_project_mut(|p| p.materials.extras.insert(id.clone(), value))
            .unwrap();
        id
    }

    #[test]
    fn stabilisation_settings_are_undoable_steps() {
        let state = state();
        let motion = put_path(&state);
        let entry = store::new_entry(&Stabilise {
            motion_id: motion,
            enabled: true,
            strength: 0.6,
            crop: None,
        });
        let id = entry.0.clone();
        commit(&state, vec![entry], |p| {
            store::swap_entry(p, "a", store::STABILISE, Some(&id), "Stabilise")
        })
        .unwrap();
        let response = analysis_set_stabilise(&state, "a".into(), Some(false), None, None).unwrap();
        assert_eq!(
            response.undo_label.as_deref(),
            Some("Turn off stabilisation")
        );
        let enabled = |state: &AppState| {
            state
                .with_project(|p| analysis_of(p, "a").stabilise.map(|s| s.enabled))
                .unwrap()
        };
        assert_eq!(enabled(&state), Some(false));
        timeline_undo(&state).unwrap();
        assert_eq!(enabled(&state), Some(true));
        timeline_undo(&state).unwrap();
        assert_eq!(enabled(&state), None);
        timeline_redo(&state).unwrap();
        assert_eq!(enabled(&state), Some(true));
        // Changing nothing is refused and not recorded.
        assert!(analysis_set_stabilise(&state, "a".into(), Some(true), None, None).is_err());
    }

    #[test]
    fn a_refused_edit_leaves_no_entry_behind() {
        let state = state();
        let before = state.with_project(|p| p.materials.extras.len()).unwrap();
        let entry = store::new_entry(&Beats {
            media_id: "v".into(),
            analysed: TimeRange::new(0, 1),
            beats: vec![],
            bpm: 120.0,
        });
        let id = entry.0.clone();
        assert!(commit(&state, vec![entry], |p| {
            store::swap_entry(p, "missing", store::BEATS, Some(&id), "x")
        })
        .is_err());
        assert_eq!(
            state.with_project(|p| p.materials.extras.len()).unwrap(),
            before
        );
    }

    #[test]
    fn reframe_candidates_skip_placed_clips() {
        let state = state();
        state
            .with_project_mut(|p| p.tracks[0].segments[1].transform.scale = [0.5, 0.5])
            .unwrap();
        let ids = state.with_project(reframe_candidates).unwrap();
        assert_eq!(ids, vec!["a".to_string()]);
    }

    #[test]
    fn a_ratio_switch_and_its_reframe_undo_as_one() {
        let state = state();
        let mut guard = state.project.write();
        let project = guard.as_mut().unwrap();
        let mut config = ProjectConfig::of(project);
        config.width = 1080;
        config.height = 1920;
        let configure = ConfigureCommand::new(project, config);
        let mut scratch = project.clone();
        configure.apply(&mut scratch).unwrap();
        let edit = edits::reframe_command(&scratch, "a", Axis::Horizontal, &[]).unwrap();
        state
            .history
            .write()
            .apply_configure_and_edit(project, configure, edit, "Reframe".into())
            .unwrap();
        assert_eq!(project.canvas.width, 1080);
        assert!(project.segment("a").unwrap().1.transform.scale[0] > 3.0);
        drop(guard);
        let response = timeline_undo(&state).unwrap();
        let _ = response;
        state
            .with_project(|p| {
                assert_eq!(p.canvas.width, 1920);
                assert_eq!(p.segment("a").unwrap().1.transform.scale[0], 1.0);
            })
            .unwrap();
    }
}
