//! The body landmark commands: analyse a clip's people, read them, and make
//! text or a sticker follow a body part.
//!
//! Analysis runs as a background job in `analysis::jobs` (kind `Body`, so
//! the status line, `analysis_status` and the CLI see it with the others)
//! and writes the cache, not the document. Following a body part writes the
//! document like following a face does: the part's pose becomes an ordinary
//! motion track stamped `body` and the overlay gets an ordinary follow link,
//! one undo step.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::analyse::{self, Job};
use super::shape::{self, BodyPart};
use super::track::{self, BodyFrame};
use crate::modules::analysis::jobs::{self, JobKind};
use crate::modules::landmarks::commands::Coverage;
use crate::modules::project::document::{Id, Micros};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::tracking::edit as tracking_edit;
use crate::modules::tracking::{
    FollowMode, TrackSample, TrackSettings, TrackingCommand, TrackingMaterial, FLAG_LOST,
};
use crate::shell::Channel;
use crate::state::AppState;

/// The tracker stamp of a track made from a body part
/// (`TrackSettings::tracker`).
pub const BODY_TRACKER: &str = "body";

/// Start finding the people of a video clip (on any timeline, or inside a
/// compound clip) in the background. Returns the analysis job
/// (`analysis_status`, `analysis_cancel`).
pub fn body_analyse(
    state: &Arc<AppState>,
    segment_id: String,
    channel: Option<Channel<jobs::JobEvent>>,
) -> Result<u64, String> {
    let job = state.with_project(|p| Job::for_segment(p, &segment_id))??;
    jobs::spawn(JobKind::Body, segment_id, channel, move |ctx| {
        let found = analyse::run(&job, Some(ctx))?;
        let (_, total) = job.coverage();
        Ok(format!("Found a person in {found} of {total} frames"))
    })
}

/// How much of a clip's frames have body landmarks.
pub fn body_coverage(state: &Arc<AppState>, segment_id: String) -> Result<Coverage, String> {
    let job = state.with_project(|p| Job::for_segment(p, &segment_id))??;
    let (analysed, total) = job.coverage();
    Ok(Coverage { analysed, total })
}

/// One keypoint as a CLI or an agent reads it.
#[derive(Debug, Clone, Serialize)]
pub struct KeypointInfo {
    pub name: &'static str,
    /// Fractions of the displayed source frame.
    pub x: f32,
    pub y: f32,
    /// About 0..1; below 0.3 the point was not seen.
    pub confidence: f32,
}

/// One person as a CLI or an agent reads them.
#[derive(Debug, Clone, Serialize)]
pub struct PersonInfo {
    /// "Person 1" is id 0: the order people were first seen in.
    pub id: u8,
    pub score: f32,
    /// x, y, width, height of the seen keypoints, fractions.
    pub bbox: Option<[f32; 4]>,
    pub keypoints: Vec<KeypointInfo>,
}

/// The people of `segment_id` at timeline time `at`, from its body track.
/// Empty when the frame has nobody or was not analysed.
pub fn body_people(
    state: &Arc<AppState>,
    segment_id: String,
    at: Micros,
) -> Result<Vec<PersonInfo>, String> {
    let (job, source) = state.with_project(|p| -> Result<_, String> {
        let job = Job::for_segment(p, &segment_id)?;
        let (_, _, segment) =
            crate::modules::sequence::find_segment(p, &segment_id).ok_or("unknown clip")?;
        Ok((job, p.materials.time_map(segment).clamped_source_time(at)))
    })??;
    let frames = track::read(&job.track)?;
    Ok(track::people_at(&frames, source, job.period() * 2)
        .into_iter()
        .map(|person| PersonInfo {
            id: person.id,
            score: person.score,
            bbox: person.bbox(),
            keypoints: person
                .points
                .iter()
                .zip(chukcut_ml_worker::rtmpose::NAMES)
                .map(|(p, name)| KeypointInfo {
                    name,
                    x: p[0],
                    y: p[1],
                    confidence: p[2],
                })
                .collect(),
        })
        .collect())
}

/// Find the people of a clip now, unless they are found: what following
/// does first. Blocking.
fn ensure(job: &Job, cancel: &AtomicBool) -> Result<(), String> {
    let (analysed, total) = job.coverage();
    if (Coverage { analysed, total }).done() {
        return Ok(());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    analyse::run(job, None).map(|_| ())
}

/// What "follow a body part" is asked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowBody {
    /// The text, sticker or picture that follows.
    pub overlay_id: Id,
    /// The video clip whose person it follows.
    pub target_segment_id: Id,
    #[serde(default)]
    pub part: BodyPart,
    #[serde(default)]
    pub mode: FollowMode,
    /// Which person: 0 is the first one seen (`body_people`'s `id`).
    #[serde(default)]
    pub person: u8,
    /// The timeline instant the overlay is placed at now: attaching does
    /// not move it there.
    pub at: Micros,
}

/// Make an overlay follow a body part of a person in a video clip, as one
/// undo step: the part's pose becomes a motion track, and the overlay
/// follows it like any tracked object. Analyses the clip first when it has
/// no body landmarks (blocking: call it from a job thread or a CLI).
pub fn body_follow(
    state: &Arc<AppState>,
    request: FollowBody,
    cancel: &AtomicBool,
) -> Result<EditResponse, String> {
    let (job, media_id, aspect) = state.with_project(|p| -> Result<_, String> {
        let (_, target) = p
            .segment(&request.target_segment_id)
            .ok_or("the clip to follow is not on the open timeline")?;
        let job = Job::of(p, target)?;
        let video = p
            .materials
            .video(&target.material_id)
            .ok_or("only a video clip has a person to follow")?;
        p.segment(&request.overlay_id)
            .ok_or("the clip that should follow is no longer on the timeline")?;
        Ok((
            job,
            video.id.clone(),
            video.width.max(1) as f32 / video.height.max(1) as f32,
        ))
    })??;
    ensure(&job, cancel)?;
    let frames = track::read(&job.track)?;
    let samples = part_samples(
        &frames,
        job.range.start,
        job.range.end(),
        request.person,
        request.part,
        aspect,
    );
    if !samples.iter().any(|s| !s.is_lost()) {
        return Err(match frames.iter().any(|f| !f.people.is_empty()) {
            true => format!(
                "the {} of person {} was not seen in the clip",
                request.part.label().to_lowercase(),
                request.person as u32 + 1
            ),
            false => "no person was found in the clip".into(),
        });
    }
    let track = TrackingMaterial::new(
        media_id,
        TrackSettings {
            tracker: BODY_TRACKER.into(),
            // Keypoints shake by a few pixels from frame to frame, more
            // than a face's; smoothing hides it and can be changed later.
            smoothing: 0.4,
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
            label: "Follow body part".into(),
            commands: vec![add, attach],
        };
        state.history.write().apply_tracking(project, command)?;
    }
    crate::modules::voice::commands::respond(state)
}

/// The part's pose per analysed frame in `[start, end)`, as motion track
/// samples. A frame where the person or the part is not seen holds the
/// last pose, flagged lost, so the follower stays put rather than jumping.
pub fn part_samples(
    frames: &[BodyFrame],
    start: Micros,
    end: Micros,
    person: u8,
    part: BodyPart,
    aspect: f32,
) -> Vec<TrackSample> {
    let mut out: Vec<TrackSample> = Vec::new();
    for frame in frames.iter().filter(|f| f.t >= start && f.t < end) {
        let pose = frame
            .people
            .iter()
            .find(|p| p.id == person)
            .and_then(|p| shape::pose(p, part, aspect));
        match pose {
            Some(pose) => out.push(TrackSample {
                t: frame.t,
                x: pose.x,
                y: pose.y,
                w: pose.w,
                h: pose.h,
                a: pose.angle,
                c: pose.confidence.min(1.0),
                f: 0,
            }),
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
    use crate::modules::body::shape::kp;
    use crate::modules::body::track::{Person, KEYPOINTS};

    fn person_at(id: u8, x: f32) -> Person {
        let mut points = [[0.0f32; 3]; KEYPOINTS];
        points[kp::L_HIP] = [x + 0.05, 0.6, 0.9];
        points[kp::R_HIP] = [x - 0.05, 0.6, 0.9];
        points[kp::L_SHOULDER] = [x + 0.08, 0.3, 0.9];
        points[kp::R_SHOULDER] = [x - 0.08, 0.3, 0.9];
        Person {
            id,
            score: 0.9,
            points,
        }
    }

    #[test]
    fn a_part_becomes_track_samples_and_a_lost_frame_holds_the_last_pose() {
        let frames = vec![
            BodyFrame {
                t: 0,
                people: vec![person_at(0, 0.4), person_at(1, 0.8)],
            },
            BodyFrame {
                t: 33_333,
                people: vec![person_at(1, 0.8), person_at(0, 0.45)],
            },
            BodyFrame {
                t: 66_667,
                people: vec![person_at(1, 0.8)],
            },
            BodyFrame {
                t: 100_000,
                people: vec![person_at(0, 0.5)],
            },
        ];
        let samples = part_samples(&frames, 0, 1_000_000, 0, BodyPart::Hips, 1.0);
        assert_eq!(samples.len(), 4);
        assert!((samples[1].x - 0.45).abs() < 1e-5);
        assert!(samples[2].is_lost() && (samples[2].x - 0.45).abs() < 1e-5);
        assert!((samples[3].x - 0.5).abs() < 1e-5 && (samples[3].y - 0.6).abs() < 1e-5);
        // Person 2 by id, wherever they are in the list.
        let other = part_samples(&frames, 0, 1_000_000, 1, BodyPart::Hips, 1.0);
        assert!(other[..3].iter().all(|s| (s.x - 0.8).abs() < 1e-5));
        // A part that is never seen gives nothing.
        assert!(part_samples(&frames, 0, 1_000_000, 0, BodyPart::LeftHand, 1.0).is_empty());
    }
}
