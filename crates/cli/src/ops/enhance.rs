//! "Remove object" and "Enhance quality" from the command line: a clip's
//! frames remade by models in the ML worker and baked before the operation
//! returns.

use chukcut_engine::modules::enhance::commands as enhance;
use chukcut_engine::modules::enhance::{ObjectRemoval, Stroke};
use chukcut_engine::modules::matting::commands::CanvasPoint;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::Time;

/// Remove an object from a video clip ("Remove object"): LaMa paints over
/// it in every frame (the ML worker; the model downloads on first use; a
/// GPU bundle makes it 10–50 times faster than the CPU). Say what to remove
/// with clicks on the object (--at and --point, canvas fractions; it is
/// followed over the clip like select_object follows it), boxes (--box,
/// source fractions) or strokes (--stroke, source fractions), in any mix:
/// together they replace what the clip removed before, or add to it with
/// --add. --off switches it off. Without any of them, says what the clip
/// has, how much is made and what a bake would take. Bakes before it
/// returns unless --no-bake. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RemoveObjectArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The timeline time of the frame the points are on.
    #[arg(long)]
    pub at: Option<Time>,
    /// A point on the object to remove: x,y as canvas fractions. Repeatable.
    #[arg(long = "point")]
    #[serde(default)]
    pub points: Vec<String>,
    /// A point on a part to keep: x,y as canvas fractions. Repeatable.
    #[arg(long = "exclude")]
    #[serde(default)]
    pub exclude: Vec<String>,
    /// A box to remove in every frame: x,y,width,height as fractions of the
    /// source picture (top-left origin). Repeatable.
    #[arg(long = "box")]
    #[serde(default)]
    pub boxes: Vec<String>,
    /// A stroke to remove in every frame: points `x,y x,y …` as fractions
    /// of the source picture. Repeatable.
    #[arg(long = "stroke")]
    #[serde(default)]
    pub strokes: Vec<String>,
    /// The strokes' brush radius, a fraction of the source's shorter side
    /// (default 0.02).
    #[arg(long)]
    pub radius: Option<f32>,
    /// How far the mask grows past the object, a fraction of the shorter
    /// side (default 0.012).
    #[arg(long)]
    pub grow: Option<f32>,
    /// Add to what the clip removes already instead of replacing it.
    #[arg(long)]
    #[serde(default)]
    pub add: bool,
    /// Switch Remove object off.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
    /// Set it and return without baking (an export bakes what is missing).
    #[arg(long)]
    #[serde(default)]
    pub no_bake: bool,
}

fn numbers(raw: &str, n: usize, what: &str) -> CliResult<Vec<f32>> {
    let values: Vec<f32> = raw
        .split(',')
        .map(|v| v.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| CliError::usage(format!("{raw:?} is not {what}")))?;
    if values.len() != n || !values.iter().all(|v| (0.0..=1.0).contains(v)) {
        return Err(CliError::usage(format!(
            "{raw:?} is not {what} with every value between 0 and 1"
        )));
    }
    Ok(values)
}

fn canvas_point(raw: &str, keep: bool) -> CliResult<CanvasPoint> {
    let v = numbers(raw, 2, "a point x,y")?;
    Ok(CanvasPoint {
        x: v[0],
        y: v[1],
        keep,
    })
}

/// Wait for bake `job`, with progress, and say what it did.
fn wait(ctx: &Ctx, job: Option<u64>) -> CliResult<Value> {
    let Some(job) = job else {
        return Ok(json!({"baked": 0, "note": "every frame was made already"}));
    };
    let mut warned = false;
    let done = enhance::enhance_wait(job, |status| {
        let p = &status.progress;
        if let (Some(warning), false) = (&status.warning, warned) {
            ctx.progress(warning, None);
            warned = true;
        }
        if p.total > 0 {
            ctx.progress(
                &format!("{}: {} of {}", p.stage, p.done, p.total),
                Some(p.done as f32 / p.total as f32),
            );
        }
    })
    .map_err(|error| CliError::refused(format!("the frames could not be made: {error}")))?;
    Ok(json!({
        "baked": done.written,
        "frames": done.frames,
        "seconds": done.seconds,
        "provider": done.provider,
        "model_ms_per_frame": done.model_millis_per_frame,
        "cancelled": done.cancelled,
    }))
}

/// The clip's settings, how much is made and what the rest would take.
fn report(session: &Session, id: &str) -> Value {
    let settings = enhance::enhance_settings(&session.state, id.to_string())
        .map(|s| json!(s))
        .unwrap_or_else(|e| json!({"error": e}));
    let coverage = enhance::enhance_coverage(&session.state, id.to_string())
        .map(|c| json!({"baked": c.baked, "total": c.total}))
        .unwrap_or(Value::Null);
    let estimate = enhance::enhance_estimate(&session.state, id.to_string())
        .map(|e| json!(e))
        .unwrap_or(Value::Null);
    json!({
        "clip": session.with(|p| summary::clip_by_id(p, id)),
        "settings": settings,
        "coverage": coverage,
        "estimate": estimate,
    })
}

/// Bake (unless `no_bake`, which stops a started bake) and build the reply.
fn bake_and_reply(
    session: &Session,
    ctx: &Ctx,
    id: &str,
    response: enhance::EnhanceResponse,
    no_bake: bool,
    message: String,
) -> CliResult<Outcome> {
    let edited = response.edit.is_some();
    let mut value = report(session, id);
    if no_bake {
        if let Some(job) = response.job {
            enhance::enhance_cancel(job);
        }
    } else {
        value["bake"] = wait(ctx, response.job)?;
        value["coverage"] = report(session, id)["coverage"].clone();
    }
    Ok(if edited {
        Outcome::changed(message, value)
    } else {
        Outcome::read(message, value)
    })
}

impl Operation for RemoveObjectArgs {
    const NAME: &'static str = "remove_object";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let state = session.state.clone();
        if self.off {
            let response = enhance::enhance_set_removal(&state, id.clone(), None)?;
            return bake_and_reply(
                session,
                ctx,
                &id,
                response,
                true,
                "remove object off".into(),
            );
        }
        let clicked = !self.points.is_empty();
        let painted = !self.boxes.is_empty() || !self.strokes.is_empty();
        if !clicked && !painted && self.grow.is_none() {
            let value = report(session, &id);
            let on = value["settings"]["removal"].is_object();
            return Ok(Outcome::read(
                if on {
                    "the clip has an object removed"
                } else {
                    "the clip has no object removed"
                },
                value,
            ));
        }
        let current = enhance::enhance_settings(&state, id.clone())?.removal;
        let mut removal = match (&current, self.add) {
            (Some(current), true) => current.clone(),
            _ => ObjectRemoval::new(),
        };
        if !clicked && !painted {
            // Only --grow: the same mask, grown differently.
            removal = current
                .clone()
                .ok_or_else(|| CliError::usage("the clip has no object removed to grow"))?;
        }
        for raw in &self.boxes {
            let v = numbers(raw, 4, "a box x,y,width,height")?;
            removal.boxes.push([v[0], v[1], v[2], v[3]]);
        }
        let radius = self.radius.unwrap_or(0.02);
        for raw in &self.strokes {
            let points = raw
                .split_whitespace()
                .map(|p| numbers(p, 2, "a point x,y").map(|v| [v[0], v[1]]))
                .collect::<CliResult<Vec<_>>>()?;
            if points.is_empty() {
                return Err(CliError::usage("a --stroke needs points: \"x,y x,y …\""));
            }
            removal.strokes.push(Stroke { points, radius });
        }
        if let Some(grow) = self.grow {
            removal.grow = grow;
        }
        if clicked {
            let at = self
                .at
                .ok_or_else(|| CliError::usage("--point needs --at, the time of its frame"))?;
            let at = at.resolve(session.with(|p| p.fps));
            let mut points = Vec::new();
            for raw in &self.points {
                points.push(canvas_point(raw, true)?);
            }
            for raw in &self.exclude {
                points.push(canvas_point(raw, false)?);
            }
            removal.prompt = Some(enhance::enhance_prompt_at(&state, id.clone(), at, points)?);
        }
        let response = enhance::enhance_set_removal(&state, id.clone(), Some(removal))?;
        bake_and_reply(
            session,
            ctx,
            &id,
            response,
            self.no_bake,
            "object removed".into(),
        )
    }
}

/// Make a video clip's picture larger and cleaner ("Enhance quality"):
/// Real-ESRGAN remakes every frame at 2x or 4x (at most 3840 px on the
/// long side; for footage up to 1920 px), taking out blur, noise and
/// compression blocks. Used by the preview and the export. The ML worker
/// runs it; a GPU bundle makes it 10–50 times faster than the CPU, and the
/// answer says what the bake will take. --scale off switches it off;
/// without --scale, says what the clip has. Bakes before it returns unless
/// --no-bake. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EnhanceQualityArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// 2, 4 or off.
    #[arg(long)]
    pub scale: Option<String>,
    /// Set it and return without baking (an export bakes what is missing).
    #[arg(long)]
    #[serde(default)]
    pub no_bake: bool,
}

impl Operation for EnhanceQualityArgs {
    const NAME: &'static str = "enhance_quality";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let Some(scale) = &self.scale else {
            let value = report(session, &id);
            let message = match value["settings"]["upscale"]["scale"].as_u64() {
                Some(s) => format!("the clip is enhanced {s}x"),
                None => "the clip is not enhanced".into(),
            };
            return Ok(Outcome::read(message, value));
        };
        let scale = match scale.trim().trim_end_matches(['x', 'X']) {
            "off" | "none" | "0" | "1" => None,
            "2" => Some(2),
            "4" => Some(4),
            other => {
                return Err(CliError::usage(format!(
                    "{other:?} is not a scale; choose 2, 4 or off"
                )))
            }
        };
        let response = enhance::enhance_set_upscale(&session.state, id.clone(), scale)?;
        let message = match scale {
            Some(s) => format!("enhanced {s}x"),
            None => "enhance quality off".into(),
        };
        bake_and_reply(
            session,
            ctx,
            &id,
            response,
            self.no_bake || scale.is_none(),
            message,
        )
    }
}
