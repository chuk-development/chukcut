//! Machine-learning edits on a clip: isolate voice, face landmarks,
//! retouch, text or stickers that follow a face, body landmarks and
//! following a body part. Each edit is one engine
//! command (and one undo step); the slow part, a model run in the ML
//! worker, is cached.

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::analysis::commands as analysis_commands;
use chukcut_engine::modules::body::commands::{self as body, FollowBody};
use chukcut_engine::modules::body::shape::BodyPart;
use chukcut_engine::modules::landmarks::commands::{self as landmarks, FollowFace, RetouchSetting};
use chukcut_engine::modules::landmarks::shape::Anchor;
use chukcut_engine::modules::tracking::FollowMode;
use chukcut_engine::modules::voice::commands::{self as voice_commands, IsolationSetting};
use chukcut_engine::modules::voice::isolate::Keep;
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};
use crate::On;

static NEVER: AtomicBool = AtomicBool::new(false);

/// Isolate the voice of a clip: separate speech from music and noise with a
/// model (HTDemucs, in the ML worker), or with --keep background keep only
/// the music. The isolated sound is rendered into the cache now unless
/// --no-render is given (then the app or the export renders it).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct IsolateVoiceArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// How much of what is not kept goes, 0..1. Default 1.
    #[arg(long)]
    pub strength: Option<f32>,
    /// What to keep: voice (default) or background.
    #[arg(long)]
    pub keep: Option<String>,
    /// Turn voice isolation off.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
    /// Only change the setting; leave the render to the app or the export.
    #[arg(long)]
    #[serde(default)]
    pub no_render: bool,
}

impl Operation for IsolateVoiceArgs {
    const NAME: &'static str = "isolate_voice";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let setting = if self.off {
            None
        } else {
            let strength = self.strength.unwrap_or(1.0);
            if !(0.0..=1.0).contains(&strength) {
                return Err(CliError::usage("--strength is 0..1"));
            }
            let keep: Keep = match &self.keep {
                Some(k) => enum_named("part to keep", k, &["voice", "background"])?,
                None => Keep::Voice,
            };
            Some(IsolationSetting { strength, keep })
        };
        // The same setting again is a request to make what is missing (a
        // cleared cache), not an edit.
        let current = voice_commands::voice_cleanup(&session.state, id.clone())?
            .and_then(|c| c.isolate)
            .map(|i| IsolationSetting {
                strength: i.strength,
                keep: i.keep,
            });
        let changed = current != setting;
        if changed {
            voice_commands::voice_set_isolation(&session.state, id.clone(), setting)?;
        }
        let mut rendered = None;
        if setting.is_some() && !self.no_render {
            let progress = |f: f32| ctx.progress("Isolating the voice", Some(f));
            rendered = Some(voice_commands::voice_isolation_render(
                &session.state,
                id.clone(),
                &NEVER,
                &progress,
            )?);
        }
        let cleanup = voice_commands::voice_cleanup(&session.state, id.clone())?;
        Ok(Outcome {
            message: match (&setting, &rendered) {
                (None, _) => "voice isolation off".to_string(),
                (Some(_), Some(r)) => format!("voice isolated in {:.1} s", r.seconds),
                (Some(_), None) => "voice isolation set; it renders on first use".to_string(),
            },
            data: json!({"clip": id, "cleanup": cleanup, "render": rendered}),
            mutated: changed,
        })
    }
}

/// Wait for an analysis job, reporting its progress.
fn wait(job: u64, label: &str, ctx: &Ctx) -> CliResult<String> {
    loop {
        let Some(status) = analysis_commands::analysis_status(job) else {
            return Err(CliError::refused("the analysis job vanished"));
        };
        if let Some(done) = status.finished {
            analysis_commands::analysis_forget(job);
            return done.map_err(CliError::refused);
        }
        ctx.progress(label, Some(status.fraction));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Find the faces of a clip that has none analysed yet; quiet when it has.
fn ensure_landmarks(session: &Session, id: &str, ctx: &Ctx) -> CliResult<Option<String>> {
    let coverage = landmarks::landmarks_coverage(&session.state, id.to_string())?;
    if coverage.done() {
        return Ok(None);
    }
    let job = landmarks::landmarks_analyse(&session.state, id.to_string(), None)?;
    wait(job, "Finding faces", ctx).map(Some)
}

/// Find the faces in a video clip (MediaPipe face mesh, 478 points per
/// face, in the ML worker) and print those at a time: the box, the score and
/// the named points retouch uses. Analyses the clip first if it has to.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FaceLandmarksArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The timeline time to read. Default: the clip's start.
    #[arg(long)]
    pub at: Option<Time>,
    /// Print all 478 points, not only the named ones.
    #[arg(long)]
    #[serde(default)]
    pub points: bool,
}

impl Operation for FaceLandmarksArgs {
    const NAME: &'static str = "face_landmarks";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let analysed = ensure_landmarks(session, &id, ctx)?;
        let start = session
            .with(|p| p.segment(&id).map(|(_, s)| s.target_range.start))
            .unwrap_or(0);
        let at = self.at.map_or(start, |t| t.resolve(session.fps()));
        let faces = landmarks::landmarks_faces(&session.state, id.clone(), at, self.points)?;
        let coverage = landmarks::landmarks_coverage(&session.state, id.clone())?;
        Ok(Outcome::read(
            format!(
                "{} face(s) at {:.2} s; {} of {} frames analysed",
                faces.len(),
                seconds(at),
                coverage.analysed,
                coverage.total
            ),
            json!({"clip": id, "at": seconds(at), "faces": faces, "coverage": coverage, "analysis": analysed}),
        ))
    }
}

/// Retouch the faces in a video clip: smooth skin, brighter eyes and teeth,
/// a slimmer jaw. Start from a preset (natural, soft, bright, sculpt) — or,
/// on a clip already retouched, from its values — and change any value,
/// 0..100. Finds the faces first unless --no-analyse.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RetouchArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// natural (default), soft, bright or sculpt.
    #[arg(long)]
    pub preset: Option<String>,
    /// How strongly all of it applies, 0..100. Default 100.
    #[arg(long)]
    pub strength: Option<f32>,
    #[arg(long)]
    pub smooth: Option<f32>,
    #[arg(long)]
    pub eyes: Option<f32>,
    #[arg(long)]
    pub teeth: Option<f32>,
    #[arg(long)]
    pub slim: Option<f32>,
    /// Take the retouch off.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
    /// Only change the setting; the faces are found on first use.
    #[arg(long)]
    #[serde(default)]
    pub no_analyse: bool,
}

impl Operation for RetouchArgs {
    const NAME: &'static str = "retouch";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let setting = if self.off {
            None
        } else {
            // A preset when one is named; else the clip's own values when it
            // is retouched already; else "natural".
            let current = session.with(|p| {
                p.segment(&id).and_then(|(_, s)| {
                    p.materials
                        .effects_of(s)
                        .into_iter()
                        .find(|e| e.kind == chukcut_engine::modules::fx::catalog::RETOUCH)
                        .map(RetouchSetting::of)
                })
            });
            let mut s = match (&self.preset, current) {
                (None, Some(current)) => current,
                (preset, _) => {
                    let preset = preset.as_deref().unwrap_or("natural");
                    RetouchSetting::preset(preset).ok_or_else(|| {
                        CliError::usage(format!(
                            "there is no retouch preset {preset:?}; choose natural, soft, bright or sculpt"
                        ))
                    })?
                }
            };
            for (slot, value) in [
                (&mut s.strength, self.strength),
                (&mut s.smooth, self.smooth),
                (&mut s.eyes, self.eyes),
                (&mut s.teeth, self.teeth),
                (&mut s.slim, self.slim),
            ] {
                if let Some(v) = value {
                    if !(0.0..=100.0).contains(&v) {
                        return Err(CliError::usage("retouch values are 0..100"));
                    }
                    *slot = v;
                }
            }
            Some(s)
        };
        landmarks::landmarks_set_retouch(&session.state, id.clone(), setting)?;
        let analysed = if setting.is_some() && !self.no_analyse {
            ensure_landmarks(session, &id, ctx)?
        } else {
            None
        };
        Ok(Outcome::changed(
            match setting {
                None => "retouch off".to_string(),
                Some(s) => format!(
                    "retouch: smooth {}, eyes {}, teeth {}, slim {} at {} %",
                    s.smooth, s.eyes, s.teeth, s.slim, s.strength
                ),
            },
            json!({"clip": id, "retouch": setting, "analysis": analysed}),
        ))
    }
}

/// Make a title, sticker or picture follow a face in a video clip: its
/// position (and with --mode, scale and rotation) rides with the face. The
/// face's pose becomes a motion track, so `track-set` and `track` work on it
/// afterwards. Finds the faces first if the clip has none analysed.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FollowFaceArgs {
    /// The clip that follows: id, id prefix or `lane:index`.
    pub clip: String,
    /// The video clip whose face it follows.
    #[arg(long)]
    pub face_of: String,
    /// face (default), eyes, forehead, nose, mouth or chin.
    #[arg(long)]
    pub anchor: Option<String>,
    /// position (default), position_scale or position_scale_rotation.
    #[arg(long)]
    pub mode: Option<String>,
    /// Which face, largest first. Default 0.
    #[arg(long)]
    pub face: Option<usize>,
    /// The timeline time the follower is placed at as it is now. Default:
    /// the follower's start.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for FollowFaceArgs {
    const NAME: &'static str = "follow_face";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (overlay, target, start) = session.with(|p| -> CliResult<_> {
            let overlay = select::clip(p, &self.clip)?;
            let target = select::clip(p, &self.face_of)?;
            let start = p.segment(&overlay).map_or(0, |(_, s)| s.target_range.start);
            Ok((overlay, target, start))
        })?;
        let anchor: Anchor = match &self.anchor {
            Some(a) => enum_named(
                "face anchor",
                a,
                &["face", "eyes", "forehead", "nose", "mouth", "chin"],
            )?,
            None => Anchor::Face,
        };
        let mode: FollowMode = match &self.mode {
            Some(m) => enum_named(
                "follow mode",
                m,
                &["position", "position_scale", "position_scale_rotation"],
            )?,
            None => FollowMode::Position,
        };
        let at = self.at.map_or(start, |t| t.resolve(session.fps()));
        ensure_landmarks(session, &target, ctx)?;
        landmarks::landmarks_follow_face(
            &session.state,
            FollowFace {
                overlay_id: overlay.clone(),
                target_segment_id: target.clone(),
                anchor,
                mode,
                face: self.face.unwrap_or(0),
                at,
            },
            &NEVER,
        )?;
        Ok(Outcome::changed(
            format!(
                "follows the {} of the face in {target}",
                anchor.label().to_lowercase()
            ),
            json!({"clip": overlay, "face_of": target, "anchor": anchor, "mode": mode}),
        ))
    }
}

/// Find the people of a clip that has none analysed yet; quiet when it has.
fn ensure_bodies(session: &Session, id: &str, ctx: &Ctx) -> CliResult<Option<String>> {
    let coverage = body::body_coverage(&session.state, id.to_string())?;
    if coverage.done() {
        return Ok(None);
    }
    let job = body::body_analyse(&session.state, id.to_string(), None)?;
    wait(job, "Finding people", ctx).map(Some)
}

/// Find the people in a video clip (RTMPose, 17 body keypoints per person,
/// with a YOLOX person detector, in the ML worker) and print those at a
/// time: each person's id, box, score and keypoints. Analyses the clip
/// first if it has to.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BodyLandmarksArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The timeline time to read. Default: the clip's start.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for BodyLandmarksArgs {
    const NAME: &'static str = "body_landmarks";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let analysed = ensure_bodies(session, &id, ctx)?;
        let start = session
            .with(|p| p.segment(&id).map(|(_, s)| s.target_range.start))
            .unwrap_or(0);
        let at = self.at.map_or(start, |t| t.resolve(session.fps()));
        let people = body::body_people(&session.state, id.clone(), at)?;
        let coverage = body::body_coverage(&session.state, id.clone())?;
        Ok(Outcome::read(
            format!(
                "{} person(s) at {:.2} s; {} of {} frames analysed",
                people.len(),
                seconds(at),
                coverage.analysed,
                coverage.total
            ),
            json!({"clip": id, "at": seconds(at), "people": people, "coverage": coverage, "analysis": analysed}),
        ))
    }
}

/// Make a title, sticker or picture follow a body part of a person in a
/// video clip — a hand, the head, the hips: its position (and with --mode,
/// scale and rotation) rides with the part. The part's pose becomes a
/// motion track, so `track-set` and `track` work on it afterwards. Finds the
/// people first if the clip has none analysed.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FollowBodyArgs {
    /// The clip that follows: id, id prefix or `lane:index`.
    pub clip: String,
    /// The video clip whose person it follows.
    #[arg(long)]
    pub body_of: String,
    /// head, shoulders, chest (default), hips, body, left_hand, right_hand,
    /// left_elbow, right_elbow, left_knee, right_knee, left_foot or
    /// right_foot. Left and right are the person's own.
    #[arg(long)]
    pub part: Option<String>,
    /// position (default), position_scale or position_scale_rotation.
    #[arg(long)]
    pub mode: Option<String>,
    /// Which person: 1 is the first one seen (`body-landmarks` lists them
    /// with id 0). Default 1.
    #[arg(long)]
    pub person: Option<u32>,
    /// The timeline time the follower is placed at as it is now. Default:
    /// the follower's start.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for FollowBodyArgs {
    const NAME: &'static str = "follow_body";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (overlay, target, start) = session.with(|p| -> CliResult<_> {
            let overlay = select::clip(p, &self.clip)?;
            let target = select::clip(p, &self.body_of)?;
            let start = p.segment(&overlay).map_or(0, |(_, s)| s.target_range.start);
            Ok((overlay, target, start))
        })?;
        let part = match &self.part {
            Some(name) => BodyPart::parse(name).ok_or_else(|| {
                CliError::usage(format!(
                    "there is no body part {name:?}; choose head, shoulders, chest, hips, body, \
                     left_hand, right_hand, left_elbow, right_elbow, left_knee, right_knee, \
                     left_foot or right_foot"
                ))
            })?,
            None => BodyPart::Chest,
        };
        let mode: FollowMode = match &self.mode {
            Some(m) => enum_named(
                "follow mode",
                m,
                &["position", "position_scale", "position_scale_rotation"],
            )?,
            None => FollowMode::Position,
        };
        let person = match self.person {
            None => 0,
            Some(n) if (1..=256).contains(&n) => (n - 1) as u8,
            Some(_) => return Err(CliError::usage("--person counts from 1")),
        };
        let at = self.at.map_or(start, |t| t.resolve(session.fps()));
        ensure_bodies(session, &target, ctx)?;
        body::body_follow(
            &session.state,
            FollowBody {
                overlay_id: overlay.clone(),
                target_segment_id: target.clone(),
                part,
                mode,
                person,
                at,
            },
            &NEVER,
        )?;
        Ok(Outcome::changed(
            format!(
                "follows the {} of person {} in {target}",
                part.label().to_lowercase(),
                person as u32 + 1
            ),
            json!({"clip": overlay, "body_of": target, "part": part, "person": person, "mode": mode}),
        ))
    }
}

/// Top-level subcommands for the ML edits.
#[derive(Subcommand)]
pub enum AiCommand {
    /// Separate a clip's voice from music and noise (or keep only the music).
    IsolateVoice(On<IsolateVoiceArgs>),
    /// Find the faces in a video clip and print their landmarks.
    FaceLandmarks(On<FaceLandmarksArgs>),
    /// Retouch the faces in a video clip.
    Retouch(On<RetouchArgs>),
    /// Make a title or sticker follow a face.
    FollowFace(On<FollowFaceArgs>),
    /// Find the people in a video clip and print their body keypoints.
    BodyLandmarks(On<BodyLandmarksArgs>),
    /// Make a title or sticker follow a body part (a hand, the head, …).
    FollowBody(On<FollowBodyArgs>),
}

impl AiCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::IsolateVoice(o) => crate::on(o, dry, ctx),
            Self::FaceLandmarks(o) => crate::on(o, dry, ctx),
            Self::Retouch(o) => crate::on(o, dry, ctx),
            Self::FollowFace(o) => crate::on(o, dry, ctx),
            Self::BodyLandmarks(o) => crate::on(o, dry, ctx),
            Self::FollowBody(o) => crate::on(o, dry, ctx),
        }
    }
}
