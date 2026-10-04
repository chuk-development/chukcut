//! Picture and sound analysis: scene changes, stabilisation, beats and
//! reframing.
//!
//! The four analyses are background jobs in the engine. A CLI command starts
//! the job, reports its progress through `ctx.progress`, and returns when the
//! job has put its result into the document as one undo step.

use chukcut_engine::modules::analysis::commands::{
    self as analysis_commands, DetectScenes, Reframe, SubjectCue,
};
use chukcut_engine::modules::project::{Micros, Project, TrackKind};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};

/// Wait for analysis job `job`, reporting progress, and return its sentence.
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

/// What a clip carries after an analysis, in timeline seconds.
fn analysis_json(project: &Project, segment_id: &str) -> Value {
    let a = analysis_commands::analysis_of(project, segment_id);
    json!({
        "clip": segment_id,
        "scenes": a.scenes.iter().map(|t| seconds(*t)).collect::<Vec<_>>(),
        "beats": a.beats.iter().map(|t| seconds(*t)).collect::<Vec<_>>(),
        "bpm": a.bpm,
        "stabilise": a.stabilise.map(|s| json!({
            "enabled": s.enabled,
            "strength": s.strength,
            "crop": s.crop,
        })),
    })
}

fn clip_ids(session: &Session, refs: &[String]) -> CliResult<Vec<String>> {
    session.with(|p| refs.iter().map(|r| select::clip(p, r)).collect())
}

/// Every clip on a video lane, for the beat edits without a clip list.
fn video_clips(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.locked)
        .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------

/// Find the shot changes in a video clip and mark them on it. With `split`,
/// also cut the clip at each change, in the same undo step. Blocks until the
/// analysis is done; progress goes to stderr.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ScenesDetectArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// 0..1; 0.5 (the default) finds hard cuts and ignores fast motion.
    #[arg(long)]
    pub sensitivity: Option<f32>,
    /// Also split the clip at every change found.
    #[arg(long)]
    #[serde(default)]
    pub split: bool,
}

impl Operation for ScenesDetectArgs {
    const NAME: &'static str = "scenes_detect";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let sensitivity = self.sensitivity.unwrap_or(0.5);
        if !sensitivity.is_finite() {
            return Err(CliError::usage("sensitivity is a number from 0 to 1"));
        }
        let job = analysis_commands::analysis_detect_scenes(
            &session.state,
            DetectScenes {
                segment_id: id.clone(),
                sensitivity,
                split: self.split,
            },
            None,
        )?;
        let message = wait(job, "Finding scene changes", ctx)?;
        let mut data = session.with(|p| analysis_json(p, &id));
        if self.split {
            data["clips"] = session.with(|p| {
                let track = p.segment(&id).map(|(t, _)| t.id.clone());
                json!(p
                    .tracks
                    .iter()
                    .filter(|t| Some(&t.id) == track.as_ref())
                    .flat_map(|t| t.segments.iter())
                    .map(|s| summary::clip_by_id(p, &s.id))
                    .collect::<Vec<_>>())
            });
        }
        Ok(Outcome::changed(message, data))
    }
}

/// Split a clip at the scene changes `scenes detect` found in it, as one
/// undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ScenesSplitArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for ScenesSplitArgs {
    const NAME: &'static str = "scenes_split";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let before = session.with(|p| p.tracks.iter().map(|t| t.segments.len()).sum::<usize>());
        analysis_commands::analysis_split_at_scenes(&session.state, id)?;
        let after = session.with(|p| p.tracks.iter().map(|t| t.segments.len()).sum::<usize>());
        Ok(Outcome::changed(
            format!("split at scene changes; {} new clip(s)", after - before),
            json!({"new_clips": after - before}),
        ))
    }
}

/// Forget the scene changes marked on a clip.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ScenesClearArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for ScenesClearArgs {
    const NAME: &'static str = "scenes_clear";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        analysis_commands::analysis_clear_scenes(&session.state, id.clone())?;
        Ok(Outcome::changed(
            "scene marks cleared",
            session.with(|p| analysis_json(p, &id)),
        ))
    }
}

// ---------------------------------------------------------------------------
// Stabilisation
// ---------------------------------------------------------------------------

fn crop_value(raw: &str) -> CliResult<Option<f32>> {
    if raw.trim().eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    raw.trim()
        .trim_end_matches('%')
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| Some(if raw.contains('%') { v / 100.0 } else { v }))
        .ok_or_else(|| {
            CliError::usage(format!("crop is auto or a fraction such as 0.1, not {raw}"))
        })
}

/// Measure a video clip's camera shake and stabilise it. The camera path is
/// cached, so a second run on the same clip is fast. Blocks until done.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StabiliseArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// 0..1: how much of the shake goes (light 0.35, medium 0.6, strong 0.85,
    /// tripod 1). Defaults to 0.6.
    #[arg(long)]
    pub strength: Option<f32>,
    /// How much of the picture is cropped to hide the moving edges: auto
    /// (the default) or a fraction up to 0.3.
    #[arg(long)]
    pub crop: Option<String>,
}

impl Operation for StabiliseArgs {
    const NAME: &'static str = "stabilise";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let crop = self.crop.as_deref().map(crop_value).transpose()?.flatten();
        let job = analysis_commands::analysis_stabilise(
            &session.state,
            id.clone(),
            self.strength.unwrap_or(0.6),
            crop,
            None,
        )?;
        let message = wait(job, "Measuring camera shake", ctx)?;
        Ok(Outcome::changed(
            message,
            session.with(|p| analysis_json(p, &id)),
        ))
    }
}

/// Change how a stabilised clip is stabilised: on or off, strength, crop.
/// The clip must have been stabilised once with `stabilise`.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StabiliseSetArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Switch the stabilisation on (true) or off (false).
    #[arg(long)]
    pub enabled: Option<bool>,
    /// 0..1.
    #[arg(long)]
    pub strength: Option<f32>,
    /// auto, or a fraction up to 0.3.
    #[arg(long)]
    pub crop: Option<String>,
}

impl Operation for StabiliseSetArgs {
    const NAME: &'static str = "stabilise_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        if self.enabled.is_none() && self.strength.is_none() && self.crop.is_none() {
            return Err(CliError::usage(
                "stabilise-set needs --enabled, --strength or --crop",
            ));
        }
        let crop = self.crop.as_deref().map(crop_value).transpose()?;
        analysis_commands::analysis_set_stabilise(
            &session.state,
            id.clone(),
            self.enabled,
            self.strength,
            crop,
        )?;
        Ok(Outcome::changed(
            "stabilisation changed",
            session.with(|p| analysis_json(p, &id)),
        ))
    }
}

/// Take a clip's stabilisation away.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StabiliseRemoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for StabiliseRemoveArgs {
    const NAME: &'static str = "stabilise_remove";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        analysis_commands::analysis_remove_stabilise(&session.state, id.clone())?;
        Ok(Outcome::changed(
            "stabilisation removed",
            session.with(|p| analysis_json(p, &id)),
        ))
    }
}

// ---------------------------------------------------------------------------
// Beats
// ---------------------------------------------------------------------------

/// Find the beats in a clip's sound and mark them on the clip that plays
/// it (a video clip's linked sound, or a music clip). Blocks until done.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BeatsDetectArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for BeatsDetectArgs {
    const NAME: &'static str = "beats_detect";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let job = analysis_commands::analysis_detect_beats(&session.state, id.clone(), None)?;
        let message = wait(job, "Finding the beat", ctx)?;
        // The beats sit on the clip that plays the sound, which is the linked
        // audio clip when `clip` is a picture.
        let data = session.with(|p| {
            let own = analysis_json(p, &id);
            if own["beats"].as_array().is_some_and(|b| !b.is_empty()) {
                return own;
            }
            chukcut_engine::modules::voice::cleanup::audible_segment(p, &id)
                .map(|sound| analysis_json(p, &sound))
                .unwrap_or(own)
        });
        Ok(Outcome::changed(message, data))
    }
}

/// Forget the beats marked on a clip's sound.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BeatsClearArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for BeatsClearArgs {
    const NAME: &'static str = "beats_clear";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        analysis_commands::analysis_clear_beats(&session.state, id.clone())?;
        Ok(Outcome::changed(
            "beat marks cleared",
            session.with(|p| analysis_json(p, &id)),
        ))
    }
}

/// Cut video clips on the beats found by `beats detect`, keeping every
/// `every`-th beat. Without clips, every clip on the video lanes. One undo
/// step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BeatsCutArgs {
    /// The clips to cut: ids, id prefixes or `lane:index`.
    #[serde(default)]
    pub clips: Vec<String>,
    /// Cut on every n-th beat. Defaults to 1.
    #[arg(long)]
    pub every: Option<usize>,
}

impl Operation for BeatsCutArgs {
    const NAME: &'static str = "beats_cut";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let ids = if self.clips.is_empty() {
            session.with(video_clips)
        } else {
            clip_ids(session, &self.clips)?
        };
        let before = session.with(|p| p.tracks.iter().map(|t| t.segments.len()).sum::<usize>());
        analysis_commands::analysis_auto_cut_to_beat(
            &session.state,
            ids,
            self.every.unwrap_or(1).max(1),
        )?;
        let after = session.with(|p| p.tracks.iter().map(|t| t.segments.len()).sum::<usize>());
        Ok(Outcome::changed(
            format!(
                "cut to the beat; {} new clip(s)",
                after.saturating_sub(before)
            ),
            json!({"new_clips": after.saturating_sub(before)}),
        ))
    }
}

/// Move each cut between the clips onto the nearest beat within
/// `tolerance`, by rolling the cut, so nothing else moves. Without clips,
/// every clip on the video lanes. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BeatsSnapArgs {
    /// The clips whose cuts move: ids, id prefixes or `lane:index`.
    #[serde(default)]
    pub clips: Vec<String>,
    /// How far a cut may move. Defaults to half a beat at the slowest tempo
    /// marked, or 250 ms.
    #[arg(long)]
    pub tolerance: Option<Time>,
}

impl Operation for BeatsSnapArgs {
    const NAME: &'static str = "beats_snap";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let ids = if self.clips.is_empty() {
            session.with(video_clips)
        } else {
            clip_ids(session, &self.clips)?
        };
        let tolerance: Micros = match self.tolerance {
            Some(t) => t.resolve(session.fps()),
            None => {
                // The app's default: half a beat at the slowest tempo marked.
                let bpm = session.with(|p| {
                    p.tracks
                        .iter()
                        .flat_map(|t| t.segments.iter())
                        .filter_map(|s| analysis_commands::analysis_of(p, &s.id).bpm)
                        .fold(f32::INFINITY, f32::min)
                });
                if bpm.is_finite() && bpm > 0.0 {
                    (30.0 / bpm * 1e6) as Micros
                } else {
                    250_000
                }
            }
        };
        analysis_commands::analysis_snap_cuts_to_beats(&session.state, ids, tolerance)?;
        Ok(Outcome::changed(
            "cuts snapped to the beat",
            json!({"tolerance": seconds(tolerance)}),
        ))
    }
}

// ---------------------------------------------------------------------------
// Reframe
// ---------------------------------------------------------------------------

fn ratio(raw: &str) -> CliResult<(u32, u32)> {
    let bad = || {
        CliError::usage(format!(
            "{raw:?} is not a ratio; write W:H, for example 9:16"
        ))
    };
    let (w, h) = raw
        .split_once(':')
        .or_else(|| raw.split_once('x'))
        .or_else(|| raw.split_once('/'))
        .ok_or_else(bad)?;
    let w: u32 = w.trim().parse().map_err(|_| bad())?;
    let h: u32 = h.trim().parse().map_err(|_| bad())?;
    if w == 0 || h == 0 {
        return Err(bad());
    }
    Ok((w, h))
}

/// Follow the subject of each clip with a window of the canvas's shape,
/// written as position keyframes. With `ratio`, the project switches to that
/// shape in the same undo step. Without clips, every clip that fills its
/// frame on a visible video lane. Blocks until done.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ReframeArgs {
    /// The clips: ids, id prefixes or `lane:index`.
    #[serde(default)]
    pub clips: Vec<String>,
    /// Switch the project to this shape, as W:H (9:16, 1:1, 16:9, 4:5).
    #[arg(long)]
    pub ratio: Option<String>,
    /// What to follow: `auto` (faces when the ML worker can run the face
    /// detector, downloading it on first use; saliency otherwise), `faces`
    /// (fail without the detector) or `saliency` (no model).
    #[arg(long)]
    #[serde(default)]
    pub subject: Option<String>,
}

impl Operation for ReframeArgs {
    const NAME: &'static str = "reframe";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let ratio = self.ratio.as_deref().map(ratio).transpose()?;
        let subject = match self.subject.as_deref() {
            None | Some("auto") => SubjectCue::Auto,
            Some("faces") => SubjectCue::Faces,
            Some("saliency") => SubjectCue::Saliency,
            Some(other) => {
                return Err(CliError::refused(format!(
                    "--subject is auto, faces or saliency, not {other}"
                )))
            }
        };
        let ids = if self.clips.is_empty() {
            session.with(analysis_commands::reframe_candidates)
        } else {
            clip_ids(session, &self.clips)?
        };
        if ids.is_empty() && ratio.is_none() {
            return Err(CliError::refused("there are no clips to reframe"));
        }
        let job = analysis_commands::analysis_reframe(
            &session.state,
            Reframe {
                segment_ids: ids.clone(),
                ratio,
                subject,
            },
            None,
        )?;
        let message = wait(job, "Following the subject", ctx)?;
        let data = session.with(|p| {
            json!({
                "canvas": {"width": p.canvas.width, "height": p.canvas.height},
                "clips": ids.iter().map(|id| summary::clip_by_id(p, id)).collect::<Vec<_>>(),
            })
        });
        Ok(Outcome::changed(message, data))
    }
}

/// What analysis a clip carries: scene changes and beats in timeline
/// seconds, the tempo, and its stabilisation settings. Changes nothing.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AnalysisArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for AnalysisArgs {
    const NAME: &'static str = "analysis";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let data = session.with(|p| analysis_json(p, &id));
        let scenes = data["scenes"].as_array().map_or(0, Vec::len);
        let beats = data["beats"].as_array().map_or(0, Vec::len);
        Ok(Outcome::read(
            format!("{scenes} scene change(s), {beats} beat(s)"),
            data,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_and_crops() {
        assert_eq!(ratio("9:16").unwrap(), (9, 16));
        assert_eq!(ratio("1080x1920").unwrap(), (1080, 1920));
        assert!(ratio("0:1").is_err());
        assert!(ratio("wide").is_err());
        assert_eq!(crop_value("auto").unwrap(), None);
        assert_eq!(crop_value("0.1").unwrap(), Some(0.1));
        assert_eq!(crop_value("10%").unwrap(), Some(0.1));
        assert!(crop_value("lots").is_err());
    }
}
