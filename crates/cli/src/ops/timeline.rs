//! Putting clips on the timeline and cutting them: append, place, split,
//! delete, move, trim, and a clip's own transform, speed and volume.

use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::project::{Micros, Project};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::gesture::{self, Edge};
use chukcut_engine::modules::timeline::ops::EditCommand;
use chukcut_engine::modules::tracking::commands as tracking_commands;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};

fn inserted_id(command: &EditCommand) -> Option<String> {
    match command {
        EditCommand::InsertSegment { segment, .. } => Some(segment.id.clone()),
        EditCommand::Composite { commands, .. } => commands.iter().find_map(inserted_id),
        _ => None,
    }
}

fn placed(session: &Session, verb: &str, command: EditCommand) -> CliResult<Outcome> {
    let id = inserted_id(&command).ok_or("the edit placed nothing")?;
    timeline_commands::timeline_apply(&session.state, command)?;
    let clip = session.with(|p| summary::clip_by_id(p, &id));
    Ok(Outcome::changed(
        format!(
            "{verb} clip {} at {:.3} s, {:.3} s long",
            clip["ref"].as_str().unwrap_or(&id),
            clip["start"].as_f64().unwrap_or(0.0),
            clip["duration"].as_f64().unwrap_or(0.0)
        ),
        json!({"clip": clip}),
    ))
}

/// Put an imported material at the end of the first lane of its kind.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct AppendArgs {
    /// The material: its id, file name or path (see `info`).
    pub material: String,
}

impl Operation for AppendArgs {
    const NAME: &'static str = "append";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let command = session.with(|p| -> CliResult<EditCommand> {
            let id = select::material(p, &self.material)?;
            Ok(gesture::append(p, &id)?)
        })?;
        placed(session, "appended", command)
    }
}

/// Put an imported material on the timeline at a time. Without `track`, the
/// first lane of the right kind with room is used, and a new lane is added
/// above when none has room.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct PlaceArgs {
    /// The material: its id, file name or path.
    pub material: String,
    /// Where the clip starts on the timeline.
    #[arg(long)]
    pub at: Time,
    /// The lane: index, name or id.
    #[arg(long)]
    pub track: Option<String>,
    /// How long the clip is. Defaults to the rest of the material (3 s for a
    /// still).
    #[arg(long)]
    pub duration: Option<Time>,
    /// Where in the material the clip starts reading.
    #[arg(long = "from")]
    #[serde(rename = "from")]
    pub source_in: Option<Time>,
}

impl Operation for PlaceArgs {
    const NAME: &'static str = "place";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let command = session.with(|p| -> CliResult<EditCommand> {
            let id = select::material(p, &self.material)?;
            let track = self
                .track
                .as_deref()
                .map(|t| select::track(p, t))
                .transpose()?;
            Ok(gesture::place(
                p,
                &id,
                track.as_deref(),
                self.at.resolve(fps),
                self.duration.map(|d| d.resolve(fps)),
                self.source_in.map_or(0, |t| t.resolve(fps)),
            )?)
        })?;
        placed(session, "placed", command)
    }
}

/// Cut a clip in two at a time. Without `clip`, every unlocked clip under
/// that time is cut, as the app's split at the playhead does.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct SplitArgs {
    /// Timeline time to cut at.
    #[arg(long)]
    pub at: Time,
    /// The clip: id, id prefix or `lane:index`.
    #[arg(long)]
    pub clip: Option<String>,
}

impl Operation for SplitArgs {
    const NAME: &'static str = "split";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let at = self.at.resolve(session.fps());
        let before: Vec<String> = session.with(all_ids);
        match &self.clip {
            Some(reference) => {
                let id = session.with(|p| select::clip(p, reference))?;
                timeline_commands::timeline_split(&session.state, id, at)?;
            }
            None => {
                timeline_commands::timeline_split_all(&session.state, at)?;
            }
        }
        let created: Vec<serde_json::Value> = session.with(|p| {
            all_ids(p)
                .into_iter()
                .filter(|id| !before.contains(id))
                .map(|id| summary::clip_by_id(p, &id))
                .collect()
        });
        Ok(Outcome::changed(
            format!(
                "split at {:.3} s; {} new clip(s)",
                seconds(at),
                created.len()
            ),
            json!({"at": seconds(at), "created": created}),
        ))
    }
}

fn all_ids(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
        .collect()
}

/// Take clips off the timeline. With `ripple`, everything after each clip on
/// its lane moves left to close the gap. Clips that a tracked overlay follows
/// are baked to keyframes first, so the overlay keeps its motion.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct DeleteArgs {
    /// The clips: ids, id prefixes or `lane:index`.
    #[arg(required = true)]
    pub clips: Vec<String>,
    /// Close the gap each clip leaves.
    #[arg(long)]
    #[serde(default)]
    pub ripple: bool,
}

impl Operation for DeleteArgs {
    const NAME: &'static str = "delete";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.clips.is_empty() {
            return Err(CliError::usage("delete needs at least one clip"));
        }
        // Resolve every reference before anything moves: `0:1` means the
        // clip that is there now, not after the first deletion.
        let ids: Vec<String> = session.with(|p| {
            self.clips
                .iter()
                .map(|r| select::clip(p, r))
                .collect::<CliResult<_>>()
        })?;
        // A ripple delete shifts the lane, so each one is built against the
        // document the previous one left; without ripple they are one step.
        let groups: Vec<Vec<String>> = if self.ripple {
            ids.iter().map(|id| vec![id.clone()]).collect()
        } else {
            vec![ids.clone()]
        };
        for group in groups {
            let (commands, followers) = session.with(|p| -> CliResult<_> {
                let mut commands = Vec::new();
                for id in &group {
                    if p.segment(id).is_none() {
                        // Already gone with a linked partner.
                        continue;
                    }
                    commands.extend(gesture::remove(p, id, self.ripple)?);
                }
                Ok((
                    commands,
                    tracking_commands::tracking_dependent_followers(p, &group),
                ))
            })?;
            if commands.is_empty() {
                continue;
            }
            if followers.is_empty() {
                timeline_commands::timeline_apply_many(&session.state, commands, "Delete".into())?;
            } else {
                tracking_commands::tracking_bake_and_delete(
                    &session.state,
                    group,
                    commands,
                    "Delete".into(),
                )?;
            }
        }
        Ok(Outcome::changed(
            format!("deleted {} clip(s)", ids.len()),
            json!({"deleted": ids}),
        ))
    }
}

/// Move a clip to another time, and optionally to another lane of its kind.
/// Refused when another clip is in the way.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct MoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The clip's new start time.
    #[arg(long)]
    pub to: Time,
    /// The lane to move it to: index, name or id.
    #[arg(long)]
    pub track: Option<String>,
}

impl Operation for MoveArgs {
    const NAME: &'static str = "move";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let (id, command) = session.with(|p| -> CliResult<_> {
            let id = select::clip(p, &self.clip)?;
            let track = self
                .track
                .as_deref()
                .map(|t| select::track(p, t))
                .transpose()?;
            let command = gesture::move_to(p, &id, track.as_deref(), self.to.resolve(fps))?;
            Ok((id, command))
        })?;
        timeline_commands::timeline_apply(&session.state, command)?;
        let clip = session.with(|p| summary::clip_by_id(p, &id));
        Ok(Outcome::changed(
            format!(
                "moved clip to {:.3} s",
                clip["start"].as_f64().unwrap_or(0.0)
            ),
            json!({"clip": clip}),
        ))
    }
}

/// Trim a clip by moving its edges to timeline times, or by giving it a new
/// length. Edges are clamped to the material and to one frame. With
/// `ripple`, later clips on the lane follow the clip's end, and a head trim
/// keeps the clip's start where it is.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct TrimArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Move the clip's start edge to this timeline time.
    #[arg(long)]
    pub head: Option<Time>,
    /// Move the clip's end edge to this timeline time.
    #[arg(long)]
    pub tail: Option<Time>,
    /// Give the clip this length, keeping its start.
    #[arg(long, conflicts_with = "tail")]
    pub duration: Option<Time>,
    /// Shift the rest of the lane with the clip's end.
    #[arg(long)]
    #[serde(default)]
    pub ripple: bool,
}

impl Operation for TrimArgs {
    const NAME: &'static str = "trim";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let id = session.with(|p| select::clip(p, &self.clip))?;
        if self.head.is_none() && self.tail.is_none() && self.duration.is_none() {
            return Err(CliError::usage("trim needs --head, --tail or --duration"));
        }
        let mut changed = false;
        // One edge at a time, each built against the document the previous
        // one left, so their clamping sees the clip as it now is.
        let edges: [(Edge, Option<Time>); 2] = [(Edge::Head, self.head), (Edge::Tail, self.tail)];
        for (edge, to) in edges {
            if let Some(to) = to {
                changed |= trim_one(session, &id, edge, to.resolve(fps), self.ripple)?;
            }
        }
        if let Some(duration) = self.duration {
            let start = session.with(|p| p.segment(&id).map(|(_, s)| s.target_range.start));
            let start = start.ok_or("the clip is gone")?;
            changed |= trim_one(
                session,
                &id,
                Edge::Tail,
                start + duration.resolve(fps),
                self.ripple,
            )?;
        }
        let clip = session.with(|p| summary::clip_by_id(p, &id));
        Ok(Outcome {
            message: if changed {
                format!(
                    "trimmed clip to {:.3}..{:.3} s",
                    clip["start"].as_f64().unwrap_or(0.0),
                    clip["end"].as_f64().unwrap_or(0.0)
                )
            } else {
                "nothing to trim; the clip is already there".into()
            },
            data: json!({"clip": clip}),
            mutated: changed,
        })
    }
}

fn trim_one(session: &Session, id: &str, edge: Edge, to: Micros, ripple: bool) -> CliResult<bool> {
    let commands = session.with(|p| gesture::trim_edge(p, id, edge, to, ripple))?;
    if commands.is_empty() {
        return Ok(false);
    }
    timeline_commands::timeline_apply_many(&session.state, commands, "Trim".into())?;
    Ok(true)
}

/// Set a clip's position, scale, rotation, opacity, flips, volume or speed.
/// Position is in canvas units: 0,0 is the centre, 1 is the edge, +y is up.
/// Speed keeps the part of the file the clip shows, so 2 halves its length
/// and pulls later clips in.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ClipSetArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Horizontal position in canvas units: 0 is the centre, 1 the right edge.
    #[arg(long, allow_hyphen_values = true)]
    pub x: Option<f32>,
    /// Vertical position in canvas units: 0 is the centre, 1 the top edge.
    #[arg(long, allow_hyphen_values = true)]
    pub y: Option<f32>,
    /// Uniform scale; 1 is the clip's fitted size.
    #[arg(long)]
    pub scale: Option<f32>,
    /// Degrees, clockwise.
    #[arg(long, allow_hyphen_values = true)]
    pub rotation: Option<f32>,
    /// 0..1.
    #[arg(long)]
    pub opacity: Option<f32>,
    /// Mirror left to right.
    #[arg(long)]
    pub flip_h: Option<bool>,
    /// Mirror top to bottom.
    #[arg(long)]
    pub flip_v: Option<bool>,
    /// Linear gain; 1 is as recorded, 0 is silent.
    #[arg(long)]
    pub volume: Option<f32>,
    /// Playback rate; 1 is normal.
    #[arg(long)]
    pub speed: Option<f32>,
}

impl Operation for ClipSetArgs {
    const NAME: &'static str = "clip_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let (before, volume) = session.with(|p| {
            p.segment(&id)
                .map(|(_, s)| (s.transform, s.volume))
                .expect("selected")
        });
        let mut after = before;
        if let Some(x) = self.x {
            after.position[0] = x;
        }
        if let Some(y) = self.y {
            after.position[1] = y;
        }
        if let Some(s) = self.scale {
            after.scale = [s, s];
        }
        if let Some(r) = self.rotation {
            after.rotation = r;
        }
        if let Some(o) = self.opacity {
            after.opacity = o;
        }
        if let Some(f) = self.flip_h {
            after.flip_h = f;
        }
        if let Some(f) = self.flip_v {
            after.flip_v = f;
        }
        if let Some(field) = after.non_finite_field() {
            return Err(CliError::usage(format!("{field} must be a finite number")));
        }
        let mut commands = Vec::new();
        let transform_changed =
            serde_json::to_value(before).ok() != serde_json::to_value(after).ok();
        if transform_changed {
            commands.push(EditCommand::SetTransform {
                segment_id: id.clone(),
                before,
                after,
            });
        }
        if let Some(v) = self.volume {
            if !v.is_finite() || v < 0.0 {
                return Err(CliError::usage("volume must be zero or more"));
            }
            if (v - volume).abs() > f32::EPSILON {
                commands.push(EditCommand::SetVolume {
                    segment_id: id.clone(),
                    before: volume,
                    after: v,
                });
            }
        }
        let mut changed = false;
        if !commands.is_empty() {
            timeline_commands::timeline_apply_many(&session.state, commands, "Change clip".into())?;
            changed = true;
        }
        if let Some(speed) = self.speed {
            inspector_commands::inspector_set_speed(&session.state, id.clone(), speed)?;
            changed = true;
        }
        let clip = session.with(|p| summary::clip_by_id(p, &id));
        Ok(Outcome {
            message: if changed {
                "clip changed".into()
            } else {
                "nothing to change".into()
            },
            data: json!({"clip": clip}),
            mutated: changed,
        })
    }
}
