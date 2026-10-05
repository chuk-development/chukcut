//! What a clip shows and how fast: crop, tone curves, freeze frame and speed
//! curves.

use std::borrow::Cow;
use std::str::FromStr;

use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::inspector::edit::GradeEdit;
use chukcut_engine::modules::project::grade::CurveChannel;
use chukcut_engine::modules::project::{Crop, Micros, Project, SpeedPoint, SpeedPreset};
use chukcut_engine::modules::speed::commands as speed_commands;
use chukcut_engine::modules::speed::edit::CurveChange;
use chukcut_engine::modules::timeline::commands as timeline_commands;
use clap::Args;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use super::{enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};

fn clip_outcome(session: &Session, id: &str, message: String) -> Outcome {
    let clip = session.with(|p| summary::clip_by_id(p, id));
    Outcome::changed(message, json!({"clip": clip}))
}

// ---------------------------------------------------------------------------
// Crop
// ---------------------------------------------------------------------------

/// Crop a clip's picture. Edges are fractions of the source frame: left and
/// top from 0, right and bottom up to 1. An edge that is not given keeps its
/// current value. `clear` removes the crop and its keyframes.
///
/// With `at`, the crop is a keyframe at that timeline time on all four edges
/// (the one there changes); edges not given keep the crop shown there, and
/// `remove` takes the crop keyframe there away. A crop that has keyframes is
/// only changed with `at`.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CropArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The left edge, 0..1 from the left of the frame.
    #[arg(long)]
    pub left: Option<f32>,
    /// The top edge, 0..1 from the top of the frame.
    #[arg(long)]
    pub top: Option<f32>,
    /// The right edge, 0..1 from the left of the frame.
    #[arg(long)]
    pub right: Option<f32>,
    /// The bottom edge, 0..1 from the top of the frame.
    #[arg(long)]
    pub bottom: Option<f32>,
    /// Remove the crop, keyframes included.
    #[arg(long, conflicts_with_all = ["left", "top", "right", "bottom", "at", "remove"])]
    #[serde(default)]
    pub clear: bool,
    /// Key the crop at this timeline time instead of setting the clip's one
    /// rectangle.
    #[arg(long)]
    #[serde(default)]
    pub at: Option<Time>,
    /// With `at`: remove the crop keyframe at that time.
    #[arg(long, requires = "at", conflicts_with_all = ["left", "top", "right", "bottom"])]
    #[serde(default)]
    pub remove: bool,
}

impl CropArgs {
    fn edges_given(&self) -> bool {
        self.left.is_some() || self.top.is_some() || self.right.is_some() || self.bottom.is_some()
    }

    /// `base` with the given edges written over it.
    fn over(&self, base: Option<Crop>) -> CliResult<Crop> {
        let mut crop = base.unwrap_or_default();
        if let Some(v) = self.left {
            crop.left = v;
        }
        if let Some(v) = self.top {
            crop.top = v;
        }
        if let Some(v) = self.right {
            crop.right = v;
        }
        if let Some(v) = self.bottom {
            crop.bottom = v;
        }
        if let Some(field) = crop.non_finite_field() {
            return Err(CliError::usage(format!("{field} must be a finite number")));
        }
        Ok(crop)
    }

    fn run_at(self, session: &mut Session, id: String, at: &Time) -> CliResult<Outcome> {
        let time = at.resolve(session.fps());
        if self.remove {
            let keyed = session.with(|p| {
                p.segment(&id).is_some_and(|(_, s)| {
                    let relative = time - s.target_range.start;
                    let tolerance = (500_000.0 / p.fps.max(1.0)) as Micros;
                    s.keyframes
                        .iter()
                        .filter(|t| t.property.is_crop())
                        .any(|t| {
                            t.keyframes
                                .iter()
                                .any(|k| (k.time - relative).abs() <= tolerance)
                        })
                })
            });
            if !keyed {
                return Err(CliError::refused("there is no crop keyframe at that time"));
            }
            inspector_commands::inspector_toggle_crop_keyframe(&session.state, id.clone(), time)?;
            return Ok(clip_outcome(
                session,
                &id,
                format!("crop keyframe at {:.3} s removed", seconds(time)),
            ));
        }
        if !self.edges_given() {
            return Err(CliError::usage(
                "crop --at needs --left, --top, --right, --bottom or --remove",
            ));
        }
        let shown = session.with(|p| p.segment(&id).and_then(|(_, s)| s.crop_at(time)));
        let crop = self.over(shown)?;
        inspector_commands::inspector_set_crop_at(&session.state, id.clone(), Some(crop), time)?;
        Ok(clip_outcome(
            session,
            &id,
            format!("crop keyframe at {:.3} s", seconds(time)),
        ))
    }
}

impl Operation for CropArgs {
    const NAME: &'static str = "crop";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        if let Some(at) = self.at {
            return self.run_at(session, id, &at);
        }
        if self.remove {
            return Err(CliError::usage("crop --remove needs --at"));
        }
        let (current, keyed) = session.with(|p| {
            p.segment(&id)
                .map_or((None, false), |(_, s)| (s.crop, s.has_crop_keyframes()))
        });
        let crop = if self.clear {
            None
        } else {
            if !self.edges_given() {
                return Err(CliError::usage(
                    "crop needs --left, --top, --right, --bottom or --clear",
                ));
            }
            if keyed {
                return Err(CliError::refused(
                    "the crop has keyframes; give --at to set it at a time, or --clear it first",
                ));
            }
            Some(self.over(current)?)
        };
        if crop.is_none() && current.is_none() && !keyed {
            return Ok(Outcome::read(
                "the clip has no crop",
                json!({"clip": session.with(|p| summary::clip_by_id(p, &id))}),
            ));
        }
        inspector_commands::inspector_set_crop(&session.state, id.clone(), crop)?;
        Ok(clip_outcome(
            session,
            &id,
            if crop.is_some() {
                "crop set".into()
            } else {
                "crop removed".into()
            },
        ))
    }
}

// ---------------------------------------------------------------------------
// Tone curves
// ---------------------------------------------------------------------------

/// One control point of a tone curve: input and output level, both 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint(pub [f32; 2]);

impl FromStr for CurvePoint {
    type Err = String;
    fn from_str(raw: &str) -> Result<Self, String> {
        let bad = || format!("{raw:?} is not a curve point; write x,y with both in 0..1");
        let (x, y) = raw
            .split_once(',')
            .or_else(|| raw.split_once(':'))
            .ok_or_else(bad)?;
        let x: f32 = x.trim().parse().map_err(|_| bad())?;
        let y: f32 = y.trim().parse().map_err(|_| bad())?;
        CurvePoint::checked([x, y]).ok_or_else(bad)
    }
}

impl CurvePoint {
    fn checked(p: [f32; 2]) -> Option<Self> {
        p.iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            .then_some(CurvePoint(p))
    }
}

impl<'de> Deserialize<'de> for CurvePoint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Pair([f32; 2]),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Pair(p) => CurvePoint::checked(p)
                .ok_or_else(|| serde::de::Error::custom("curve points are [x, y] in 0..1")),
            Raw::Text(t) => t.parse().map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for CurvePoint {
    fn schema_name() -> Cow<'static, str> {
        "CurvePoint".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A curve point: [x, y] or \"x,y\", both in 0..1.",
            "type": ["array", "string"],
            "items": {"type": "number", "minimum": 0, "maximum": 1},
        })
    }
    fn inline_schema() -> bool {
        true
    }
}

const CHANNELS: &[&str] = &["master", "red", "green", "blue"];

/// Set one tone curve of a clip's grade from control points, or reset it.
/// Points are input,output pairs in 0..1; the engine joins them with a
/// smooth curve that never overshoots. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CurveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// master (all channels), red, green or blue.
    #[arg(long, default_value = "master")]
    #[serde(default = "master")]
    pub channel: String,
    /// A control point as x,y in 0..1; give two or more.
    #[arg(long = "point", required_unless_present = "reset")]
    #[serde(default)]
    pub points: Vec<CurvePoint>,
    /// Put this curve back to a straight line.
    #[arg(long, conflicts_with = "points")]
    #[serde(default)]
    pub reset: bool,
}

fn master() -> String {
    "master".into()
}

fn curves_json(edit: &GradeEdit) -> Value {
    json!(edit.grade.curves)
}

impl Operation for CurveArgs {
    const NAME: &'static str = "curve";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let channel: CurveChannel = enum_named("curve channel", &self.channel, CHANNELS)?;
        let points: Vec<[f32; 2]> = if self.reset {
            Vec::new()
        } else {
            let mut points: Vec<[f32; 2]> = self.points.iter().map(|p| p.0).collect();
            if points.len() < 2 {
                return Err(CliError::usage("a curve needs two or more points"));
            }
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
            if points.windows(2).any(|w| w[0][0] == w[1][0]) {
                return Err(CliError::usage("two curve points have the same x"));
            }
            points
        };
        let current = session.with(|p| {
            p.segment(&id)
                .map(|(_, s)| GradeEdit::of(p.materials.color_adjust_of(s)))
                .unwrap_or_default()
        });
        let mut wanted = current.clone();
        *wanted.grade.curves.get_mut(channel) = points.clone();
        if wanted == current {
            return Ok(Outcome::read(
                "the curve is already that",
                json!({"clip": id, "curves": curves_json(&current)}),
            ));
        }
        inspector_commands::inspector_set_curve(&session.state, id.clone(), channel, points)?;
        let after = session.with(|p| {
            p.segment(&id)
                .map(|(_, s)| GradeEdit::of(p.materials.color_adjust_of(s)))
                .unwrap_or_default()
        });
        Ok(Outcome::changed(
            format!(
                "{} curve {}",
                self.channel.to_ascii_lowercase(),
                if self.reset { "reset" } else { "set" }
            ),
            json!({"clip": id, "curves": curves_json(&after)}),
        ))
    }
}

// ---------------------------------------------------------------------------
// Freeze frame
// ---------------------------------------------------------------------------

fn all_ids(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
        .collect()
}

/// Hold the frame of a video clip at a time: the clip is cut there, a still
/// of that frame goes in between, and everything after it on the clip's lane
/// and its linked lanes moves right. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FreezeArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The timeline time of the frame to hold.
    #[arg(long)]
    pub at: Time,
    /// How long the still lasts. Defaults to 3 s.
    #[arg(long)]
    pub duration: Option<Time>,
}

impl Operation for FreezeArgs {
    const NAME: &'static str = "freeze";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let at = self.at.resolve(fps);
        let duration = self.duration.map_or(3_000_000, |d| d.resolve(fps));
        if duration <= 0 {
            return Err(CliError::usage("a freeze frame needs a duration above 0"));
        }
        let before = session.with(all_ids);
        timeline_commands::timeline_freeze_frame(&session.state, id, at, duration)?;
        let created: Vec<Value> = session.with(|p| {
            all_ids(p)
                .into_iter()
                .filter(|id| !before.contains(id))
                .map(|id| summary::clip_by_id(p, &id))
                .collect()
        });
        let still = created
            .iter()
            .find(|c| c["kind"] == "image")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(Outcome::changed(
            format!(
                "froze the frame at {:.3} s for {:.3} s",
                seconds(at),
                seconds(duration)
            ),
            json!({"still": still, "created": created}),
        ))
    }
}

// ---------------------------------------------------------------------------
// Speed curves
// ---------------------------------------------------------------------------

const PRESETS: &[&str] = &[
    "montage",
    "hero",
    "bullet",
    "jump_cut",
    "flash_in",
    "flash_out",
];

/// One speed point: a time into the clip's media from the clip's in point,
/// and the speed there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedAt {
    pub at: Time,
    pub speed: f32,
}

impl FromStr for SpeedAt {
    type Err = String;
    fn from_str(raw: &str) -> Result<Self, String> {
        let bad =
            || format!("{raw:?} is not a speed point; write TIME=SPEED, for example 1.5s=0.3");
        let (at, speed) = raw.split_once('=').ok_or_else(bad)?;
        Ok(SpeedAt {
            at: at.trim().parse()?,
            speed: speed
                .trim()
                .trim_end_matches('x')
                .parse()
                .ok()
                .filter(|v: &f32| v.is_finite())
                .ok_or_else(bad)?,
        })
    }
}

impl<'de> Deserialize<'de> for SpeedAt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Object { at: Time, speed: f32 },
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Object { at, speed } => Ok(SpeedAt { at, speed }),
            Raw::Text(t) => t.parse().map_err(serde::de::Error::custom),
        }
    }
}

impl JsonSchema for SpeedAt {
    fn schema_name() -> Cow<'static, str> {
        "SpeedAt".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A speed point: {\"at\": time, \"speed\": 0.5} or \"1.5s=0.5\". The time is into the clip's media, from the clip's in point, at normal speed.",
            "type": ["object", "string"],
            "properties": {
                "at": {"type": ["number", "string"]},
                "speed": {"type": "number"},
            },
        })
    }
    fn inline_schema() -> bool {
        true
    }
}

/// Give a clip a speed curve (a ramp), from a preset or from points, or
/// remove it. The clip's length follows the curve, and later clips on its
/// lanes move with its end. A clip on a curve plays no sound. One undo step.
/// `catalog speed_presets` lists the presets.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SpeedCurveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// montage, hero, bullet, jump_cut, flash_in or flash_out.
    #[arg(long, conflicts_with_all = ["points", "remove"])]
    pub preset: Option<String>,
    /// A point as TIME=SPEED: the time into the clip's media from its in
    /// point, and the speed there (0.1 to 10). Give one or more.
    #[arg(long = "point", conflicts_with = "remove")]
    #[serde(default)]
    pub points: Vec<SpeedAt>,
    /// Go back to the clip's constant speed.
    #[arg(long)]
    #[serde(default)]
    pub remove: bool,
}

fn curve_json(project: &Project, segment_id: &str) -> Value {
    let Some((_, segment)) = project.segment(segment_id) else {
        return Value::Null;
    };
    match project.materials.speed_curve_of(segment) {
        None => Value::Null,
        Some(curve) => json!({
            "preset": curve.preset,
            "points": curve
                .points
                .iter()
                .map(|p| json!({
                    "at": seconds(p.source - segment.source_range.start),
                    "speed": p.speed,
                }))
                .collect::<Vec<_>>(),
        }),
    }
}

impl Operation for SpeedCurveArgs {
    const NAME: &'static str = "speed_curve";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let change = if self.remove {
            CurveChange::Remove
        } else if let Some(name) = &self.preset {
            let preset: SpeedPreset = enum_named("speed preset", name, PRESETS)?;
            CurveChange::Preset { preset }
        } else if !self.points.is_empty() {
            let start: Micros = session
                .with(|p| p.segment(&id).map(|(_, s)| s.source_range.start))
                .ok_or("the clip is gone")?;
            let mut points: Vec<SpeedPoint> = self
                .points
                .iter()
                .map(|p| SpeedPoint {
                    source: start + p.at.resolve(fps),
                    speed: p.speed,
                })
                .collect();
            points.sort_by_key(|p| p.source);
            CurveChange::Points { points }
        } else {
            return Err(CliError::usage(
                "speed-curve needs --preset, --point or --remove",
            ));
        };
        let had = session.with(|p| {
            p.segment(&id)
                .and_then(|(_, s)| p.materials.speed_curve_of(s))
                .is_some()
        });
        if matches!(change, CurveChange::Remove) && !had {
            return Ok(Outcome::read(
                "the clip has no speed curve",
                json!({"clip": session.with(|p| summary::clip_by_id(p, &id))}),
            ));
        }
        speed_commands::speed_set_curve(&session.state, id.clone(), change)?;
        let (clip, curve) = session.with(|p| (summary::clip_by_id(p, &id), curve_json(p, &id)));
        let length = clip["duration"].as_f64().unwrap_or(0.0);
        Ok(Outcome::changed(
            if curve.is_null() {
                format!("speed curve removed; the clip is {length:.3} s long")
            } else {
                format!("speed curve set; the clip is {length:.3} s long")
            },
            json!({"clip": clip, "curve": curve}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_and_speed_points_parse() {
        assert_eq!(
            "0.25,0.3".parse::<CurvePoint>().unwrap(),
            CurvePoint([0.25, 0.3])
        );
        assert!("1.5,0".parse::<CurvePoint>().is_err());
        let p: CurvePoint = serde_json::from_str("[0, 1]").unwrap();
        assert_eq!(p, CurvePoint([0.0, 1.0]));
        let s: SpeedAt = "1.5s=0.3".parse().unwrap();
        assert_eq!(s.at.resolve(30.0), 1_500_000);
        assert_eq!(s.speed, 0.3);
        let s: SpeedAt = serde_json::from_str(r#"{"at": "15f", "speed": 2}"#).unwrap();
        assert_eq!(s.at.resolve(30.0), 500_000);
        assert!("fast".parse::<SpeedAt>().is_err());
    }
}
