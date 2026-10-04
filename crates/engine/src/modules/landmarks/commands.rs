//! The face landmark commands: analyse a clip's faces, read them, make text
//! or a sticker follow a face, and keep the tracks the retouch effect needs.
//!
//! Analysis runs as a background job in `analysis::jobs` (so the status
//! line, `analysis_status` and the CLI see it with the others) and writes
//! the cache, not the document. Following a face *does* write the document:
//! the face's pose becomes an ordinary motion track (`tracking::model`) and
//! the overlay gets an ordinary follow link, one undo step — so trimming,
//! moving, smoothing, detaching and baking to keyframes work as for any
//! tracked object.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::analyse::{self, Job};
use super::shape::{self, Anchor};
use super::track;
use crate::modules::analysis::jobs::{self, JobKind};
use crate::modules::fx::catalog::RETOUCH;
use crate::modules::project::document::{Id, Micros, Project};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::tracking::edit as tracking_edit;
use crate::modules::tracking::{
    FollowMode, TrackSample, TrackSettings, TrackingCommand, TrackingMaterial, FLAG_LOST,
};
use crate::shell::Channel;
use crate::state::AppState;

/// The tracker stamp of a track made from a face (`TrackSettings::tracker`).
pub const FACE_TRACKER: &str = "face";
/// How much of a clip's frames must be analysed before it counts as done.
/// A file's last frame can be a few microseconds past the container's
/// duration and never decode; that must not leave a clip "missing" forever.
const DONE_SHARE: f32 = 0.98;

/// Start finding the faces of a video clip in the background. Returns the
/// analysis job (`analysis_status`, `analysis_cancel`).
pub fn landmarks_analyse(
    state: &Arc<AppState>,
    segment_id: String,
    channel: Option<Channel<jobs::JobEvent>>,
) -> Result<u64, String> {
    let job = state.with_project(|p| Job::for_segment(p, &segment_id))??;
    jobs::spawn(JobKind::Landmarks, segment_id, channel, move |ctx| {
        let found = analyse::run(&job, Some(ctx))?;
        let (_, total) = job.coverage();
        Ok(format!("Found a face in {found} of {total} frames"))
    })
}

/// How much of a clip's frames have landmarks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Coverage {
    pub analysed: usize,
    pub total: usize,
}

impl Coverage {
    pub fn done(&self) -> bool {
        self.total == 0 || self.analysed as f32 >= self.total as f32 * DONE_SHARE
    }
}

pub fn landmarks_coverage(state: &Arc<AppState>, segment_id: String) -> Result<Coverage, String> {
    let job = state.with_project(|p| Job::for_segment(p, &segment_id))??;
    let (analysed, total) = job.coverage();
    Ok(Coverage { analysed, total })
}

/// One face as a CLI or an agent reads it.
#[derive(Debug, Clone, Serialize)]
pub struct FaceInfo {
    pub score: f32,
    /// x, y, width, height, fractions of the displayed source frame.
    pub bbox: [f32; 4],
    /// The named points the retouch effect uses (`shape::key`), fractions.
    pub key_points: Vec<[f32; 2]>,
    /// All 478, when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f32; 2]>>,
}

/// The faces of `segment_id` at timeline time `at`, from its landmark track.
/// Empty when the frame has no face or was not analysed.
pub fn landmarks_faces(
    state: &Arc<AppState>,
    segment_id: String,
    at: Micros,
    all_points: bool,
) -> Result<Vec<FaceInfo>, String> {
    let (job, source) = state.with_project(|p| -> Result<_, String> {
        let job = Job::for_segment(p, &segment_id)?;
        let (_, _, segment) =
            crate::modules::sequence::find_segment(p, &segment_id).ok_or("unknown clip")?;
        Ok((job, p.materials.time_map(segment).clamped_source_time(at)))
    })??;
    let frames = track::read(&job.track)?;
    Ok(track::faces_at(&frames, source, job.period() * 2)
        .into_iter()
        .map(|face| {
            let (mut x0, mut y0, mut x1, mut y1) = (1f32, 1f32, 0f32, 0f32);
            for p in face.points.iter().take(468) {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            FaceInfo {
                score: face.score,
                bbox: [x0, y0, x1 - x0, y1 - y0],
                key_points: shape::key_points(&face).to_vec(),
                points: all_points.then(|| face.points.clone()),
            }
        })
        .collect())
}

/// The clips that need landmarks: those with a retouch effect switched on,
/// on every timeline and inside compound clips.
pub fn needing(project: &Project) -> Vec<Id> {
    crate::modules::sequence::all_tracks(project)
        .flat_map(|t| t.segments.iter())
        .filter(|s| project.materials.video(&s.material_id).is_some())
        .filter(|s| {
            project
                .materials
                .effects_of(s)
                .iter()
                .any(|e| e.enabled && e.kind == RETOUCH)
        })
        .map(|s| s.id.clone())
        .collect()
}

/// The clips of `project` that need landmarks and lack them, with how many
/// frames each lacks — on every timeline and inside compound clips. Reads
/// the track files: call it off the UI thread.
pub fn missing(project: &Project) -> Vec<(Id, Job, usize)> {
    needing(project)
        .into_iter()
        .filter_map(|id| Job::for_segment(project, &id).ok().map(|job| (id, job)))
        .filter_map(|(id, job)| {
            let (analysed, total) = job.coverage();
            (!(Coverage { analysed, total }).done())
                .then(|| (id, job, total.saturating_sub(analysed)))
        })
        .collect()
}

/// Start an analysis for every clip that needs landmarks and lacks them:
/// after a retouch effect was added, a clip made longer, a cache cleared —
/// on every timeline and inside compound clips. Returns the jobs started.
/// A clip already being analysed is skipped.
pub fn landmarks_queue_missing(state: &Arc<AppState>) -> Result<Vec<u64>, String> {
    // A copy, so the track files are read without the project lock.
    let project = state.project.read().clone().ok_or("no project is open")?;
    let wanted = missing(&project);
    let mut started = Vec::new();
    for (id, _, _) in wanted {
        if let Ok(job) = landmarks_analyse(state, id, None) {
            started.push(job);
        }
    }
    Ok(started)
}

/// Analyse, now, every clip of `project` that needs landmarks and lacks
/// them: what an export does before it renders, so the retouch it draws is
/// the one the preview showed. Fails in words when the face model cannot
/// run.
pub fn landmarks_ensure(project: &Project, cancel: &AtomicBool) -> Result<(), String> {
    for (_, job, _) in missing(project) {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        analyse::run(&job, None)
            .map_err(|e| format!("could not find the faces to retouch: {e}"))?;
    }
    Ok(())
}

/// The retouch effect's five values, `0..100` each (`catalog::RETOUCH`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RetouchSetting {
    pub strength: f32,
    pub smooth: f32,
    pub eyes: f32,
    pub teeth: f32,
    pub slim: f32,
}

impl RetouchSetting {
    /// A named look from `fx::retouch::PRESETS`, at full strength.
    pub fn preset(name: &str) -> Option<Self> {
        let (_, [smooth, eyes, teeth, slim]) = crate::modules::fx::retouch::PRESETS
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))?;
        Some(Self {
            strength: 100.0,
            smooth: *smooth,
            eyes: *eyes,
            teeth: *teeth,
            slim: *slim,
        })
    }

    /// The setting a retouch effect holds now.
    pub fn of(effect: &crate::modules::project::EffectMaterial) -> Self {
        let get = |id: &str, default: f32| effect.number_at(id, 0, default);
        Self {
            strength: get("strength", 100.0),
            smooth: get("smooth", 35.0),
            eyes: get("eyes", 15.0),
            teeth: get("teeth", 20.0),
            slim: get("slim", 0.0),
        }
    }
}

/// Retouch a video clip's faces with `setting`, change it, or take the
/// retouch off with `None`, as one undo step. The landmarks it needs are
/// found afterwards (`landmarks_queue_missing` in the app, `landmarks_ensure`
/// before an export); until then the clip is drawn as it is.
pub fn landmarks_set_retouch(
    state: &Arc<AppState>,
    segment_id: String,
    setting: Option<RetouchSetting>,
) -> Result<EditResponse, String> {
    use crate::modules::fx::edit as fx_edit;
    use crate::modules::project::effects::EffectValue;
    if let Some(s) = &setting {
        for v in [s.strength, s.smooth, s.eyes, s.teeth, s.slim] {
            if !v.is_finite() || !(0.0..=100.0).contains(&v) {
                return Err("retouch values are between 0 and 100".into());
            }
        }
    }
    crate::modules::fx::commands::commit(state, |project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        if project.materials.video(&segment.material_id).is_none() {
            return Err("retouch works on a video clip's faces".into());
        }
        let existing = project
            .materials
            .effects_of(segment)
            .into_iter()
            .find(|e| e.kind == RETOUCH)
            .cloned();
        let fill = |material: &mut crate::modules::project::EffectMaterial, s: RetouchSetting| {
            for (id, v) in [
                ("strength", s.strength),
                ("smooth", s.smooth),
                ("eyes", s.eyes),
                ("teeth", s.teeth),
                ("slim", s.slim),
            ] {
                material.keyframes.remove(id);
                material.params.insert(id.into(), EffectValue::Number(v));
            }
        };
        match (existing, setting) {
            (None, None) => Err("the clip is not retouched".into()),
            (Some(effect), None) => {
                fx_edit::remove_command(project, &segment_id, &effect.id).map(|c| (None, c))
            }
            (None, Some(s)) => {
                let (mut material, command) = fx_edit::add_command(project, &segment_id, RETOUCH)?;
                fill(&mut material, s);
                Ok((Some(material), command))
            }
            (Some(effect), Some(s)) => {
                let mut updated = effect.clone();
                updated.enabled = true;
                fill(&mut updated, s);
                fx_edit::replace_command(project, &segment_id, &effect.id, updated, "Retouch")
                    .map(|(m, c)| (Some(m), c))
            }
        }
    })
}

/// What "follow a face" is asked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowFace {
    /// The text, sticker or picture that follows.
    pub overlay_id: Id,
    /// The video clip whose face it follows.
    pub target_segment_id: Id,
    #[serde(default)]
    pub anchor: Anchor,
    #[serde(default)]
    pub mode: FollowMode,
    /// Which face, largest first.
    #[serde(default)]
    pub face: usize,
    /// The timeline instant the overlay is placed at now: attaching does
    /// not move it there.
    pub at: Micros,
}

/// Make an overlay follow a face of a video clip, as one undo step: the
/// face's pose becomes a motion track, and the overlay follows it like any
/// tracked object. Analyses the clip first when it has no landmarks
/// (blocking: call it from a job thread or a CLI).
pub fn landmarks_follow_face(
    state: &Arc<AppState>,
    request: FollowFace,
    cancel: &AtomicBool,
) -> Result<EditResponse, String> {
    let (job, media_id, aspect) = state.with_project(|p| -> Result<_, String> {
        let job = Job::for_segment(p, &request.target_segment_id)?;
        let (_, target) = p
            .segment(&request.target_segment_id)
            .ok_or("unknown clip")?;
        let video = p
            .materials
            .video(&target.material_id)
            .ok_or("only a video clip has a face to follow")?;
        p.segment(&request.overlay_id)
            .ok_or("the clip that should follow is no longer on the timeline")?;
        Ok((
            job,
            video.id.clone(),
            video.width.max(1) as f32 / video.height.max(1) as f32,
        ))
    })??;
    let (analysed, total) = job.coverage();
    if !(Coverage { analysed, total }).done() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        analyse::run(&job, None)?;
    }
    let frames = track::read(&job.track)?;
    let samples = face_samples(
        &frames,
        job.range.start,
        job.range.end(),
        request.face,
        request.anchor,
        aspect,
    );
    if !samples.iter().any(|s| !s.is_lost()) {
        return Err("no face was found in the clip".into());
    }
    let track = TrackingMaterial::new(
        media_id,
        TrackSettings {
            tracker: FACE_TRACKER.into(),
            // Landmarks shake by a pixel or two from frame to frame; a
            // little smoothing hides it and can be changed afterwards.
            smoothing: 0.3,
            ..TrackSettings::default()
        },
        samples,
    );
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let add = tracking_edit::add_track(track.clone());
        let mut scratch = project.clone();
        add.apply(&mut scratch)?;
        let attach = tracking_edit::attach(
            &scratch,
            &request.overlay_id,
            &track.id,
            &request.target_segment_id,
            request.mode,
            request.at,
        )?;
        let command = TrackingCommand::Composite {
            label: "Follow face".into(),
            commands: vec![add, attach],
        };
        state.history.write().apply_tracking(project, command)?;
    }
    crate::modules::voice::commands::respond(state)
}

/// The face's pose per analysed frame in `[start, end)`, as motion track
/// samples. A frame without that face holds the last pose seen, flagged
/// lost, so the follower stays put rather than jumping.
pub fn face_samples(
    frames: &[track::FaceFrame],
    start: Micros,
    end: Micros,
    face: usize,
    anchor: Anchor,
    aspect: f32,
) -> Vec<TrackSample> {
    let mut out: Vec<TrackSample> = Vec::new();
    for frame in frames.iter().filter(|f| f.t >= start && f.t < end) {
        match frame.faces.get(face) {
            Some(f) => {
                let pose = shape::pose(f, anchor, aspect);
                out.push(TrackSample {
                    t: frame.t,
                    x: pose.x,
                    y: pose.y,
                    w: pose.w,
                    h: pose.h,
                    a: pose.angle,
                    c: f.score,
                    f: 0,
                });
            }
            None => {
                if let Some(last) = out.last().copied() {
                    out.push(TrackSample {
                        t: frame.t,
                        c: 0.0,
                        f: FLAG_LOST,
                        ..last
                    });
                }
            }
        }
    }
    // Keep an angle that turns past ±180° counting, as the tracker does, so
    // interpolation never takes the long way round.
    for i in 1..out.len() {
        let mut a = out[i].a;
        while a - out[i - 1].a > 180.0 {
            a -= 360.0;
        }
        while a - out[i - 1].a < -180.0 {
            a += 360.0;
        }
        out[i].a = a;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::landmarks::track::{Face, FaceFrame, POINTS};

    fn face_at(x: f32) -> Face {
        let mut points = vec![[x, 0.5]; POINTS];
        for (i, dx) in [(33usize, -0.1f32), (133, -0.05), (263, 0.1), (362, 0.05)] {
            points[i] = [x + dx, 0.45];
        }
        points[234] = [x - 0.15, 0.5];
        points[454] = [x + 0.15, 0.5];
        points[10] = [x, 0.3];
        points[152] = [x, 0.7];
        Face { score: 0.9, points }
    }

    #[test]
    fn a_face_becomes_track_samples_and_a_lost_frame_holds_the_last_pose() {
        let frames = vec![
            FaceFrame {
                t: 0,
                faces: vec![face_at(0.4)],
            },
            FaceFrame {
                t: 33_333,
                faces: vec![face_at(0.45)],
            },
            FaceFrame {
                t: 66_667,
                faces: vec![],
            },
            FaceFrame {
                t: 100_000,
                faces: vec![face_at(0.5)],
            },
        ];
        let samples = face_samples(&frames, 0, 1_000_000, 0, Anchor::Face, 1.0);
        assert_eq!(samples.len(), 4);
        assert!((samples[1].x - 0.45).abs() < 1e-5);
        assert!(samples[2].is_lost() && (samples[2].x - 0.45).abs() < 1e-5);
        assert!((samples[3].x - 0.5).abs() < 1e-5);
        assert!((samples[0].w - 0.3).abs() < 1e-4);
        // A range that starts on a frame without the face starts at the next
        // one that has it; a face that is never there gives nothing.
        let later = face_samples(&frames, 50_000, 1_000_000, 0, Anchor::Face, 1.0);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].t, 100_000);
        assert!(!later[0].is_lost());
        assert!(face_samples(&frames, 0, 1_000_000, 1, Anchor::Face, 1.0).is_empty());
    }
}
