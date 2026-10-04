//! How a clip is cut out and laid on what is beneath it: shape masks, the
//! chroma key and blend modes. Each change is one undo step in the session.

use chukcut_engine::modules::compositing::commands as compositing;
use chukcut_engine::modules::compositing::edit::ResetPart;
use chukcut_engine::modules::matting::commands as matting;
use chukcut_engine::modules::project::compositing::{
    BlendMode, ChromaKey, CompositingMaterial, MaskOp, MaskShape, MASK_PARAMS,
};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{assignments, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{hex, parse_assignment, parse_color, Time};

fn material(session: &Session, segment_id: &str) -> CompositingMaterial {
    compositing::compositing_get(&session.state, segment_id.to_string()).unwrap_or_default()
}

/// The masks, key and blend mode of a clip, as a script reads them back.
pub fn describe(m: &CompositingMaterial) -> Value {
    json!({
        "masks": m.masks.iter().enumerate().map(|(i, mask)| json!({
            "index": i,
            "id": mask.id,
            "shape": mask.shape.name(),
            "op": mask.op.name(),
            "x": mask.x, "y": mask.y,
            "width": mask.width, "height": mask.height,
            "rotation": mask.rotation, "feather": mask.feather,
            "roundness": mask.roundness,
            "invert": mask.invert, "enabled": mask.enabled,
            "animated": mask.keyframes.keys().collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "key": m.key.as_ref().map(|k| json!({
            "color": hex([k.color[0], k.color[1], k.color[2], 1.0]),
            "tolerance": k.tolerance, "softness": k.softness,
            "spill": k.spill, "shrink": k.shrink, "enabled": k.enabled,
        })),
        "blend": m.blend.name(),
        "background": m.background.as_ref().map(|b| json!({
            "model": b.model, "version": b.version,
        })),
    })
}

fn outcome(session: &Session, segment_id: &str, message: String) -> Outcome {
    let clip = session.with(|p| summary::clip_by_id(p, segment_id));
    let m = material(session, segment_id);
    Outcome::changed(message, json!({"clip": clip, "compositing": describe(&m)}))
}

fn shape_named(name: &str) -> CliResult<MaskShape> {
    let normalized = name.trim().to_ascii_lowercase();
    let normalized = match normalized.as_str() {
        "circle" => "ellipse".to_string(),
        "rect" => "rectangle".to_string(),
        other => other.to_string(),
    };
    let shape = MaskShape::parse(&normalized);
    if shape.code().is_none() {
        return Err(CliError::usage(format!(
            "{name:?} is not a mask shape; choose one of: {}",
            MaskShape::NAMES.join(", ")
        )));
    }
    Ok(shape)
}

fn op_named(name: &str) -> CliResult<MaskOp> {
    let op = MaskOp::parse(&name.trim().to_ascii_lowercase());
    if op.code().is_none() {
        return Err(CliError::usage(format!(
            "{name:?} is not a mask operation; choose one of: {}",
            MaskOp::NAMES.join(", ")
        )));
    }
    Ok(op)
}

/// A mask by index (0 is the first) or id.
pub(crate) fn mask_ref(m: &CompositingMaterial, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    if let Some(found) = m.masks.iter().find(|mask| mask.id == reference) {
        return Ok(found.id.clone());
    }
    if let Ok(index) = reference.parse::<usize>() {
        return m
            .masks
            .get(index)
            .map(|mask| mask.id.clone())
            .ok_or_else(|| {
                CliError::usage(format!(
                    "the clip has {} mask(s), so there is no mask {index}",
                    m.masks.len()
                ))
            });
    }
    Err(CliError::usage(format!(
        "{reference:?} is neither a mask index nor a mask id"
    )))
}

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

/// Shape masks on a clip. Add one (`add`: linear, mirror, ellipse/circle,
/// rectangle, star, heart), then set its values: x and y are the centre as a
/// fraction of the clip from its middle (+y up), width, height and feather
/// are fractions of the clip's shorter side, rotation is degrees clockwise,
/// roundness rounds a rectangle's corners (0..1). Several masks combine in
/// order by their `op`: add, subtract or intersect. With `at`, values are
/// keyframed at that timeline time. Each change is one undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MaskArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Add a mask of this shape; the other options then apply to it.
    #[arg(long)]
    pub add: Option<String>,
    /// The mask to change: its index (0 is the first) or id. Defaults to the
    /// one just added, or the only one.
    #[arg(long)]
    pub mask: Option<String>,
    /// Values to set, as name=value: x, y, width, height, rotation, feather,
    /// roundness.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
    /// Set the values at this timeline time as keyframes.
    #[arg(long)]
    pub at: Option<Time>,
    /// Change the mask's shape.
    #[arg(long)]
    pub shape: Option<String>,
    /// How it combines with the masks before it: add, subtract or intersect.
    #[arg(long)]
    pub op: Option<String>,
    /// Invert the mask (true) or not (false).
    #[arg(long)]
    pub invert: Option<bool>,
    /// Switch the mask on (true) or off (false).
    #[arg(long)]
    pub enabled: Option<bool>,
    /// Take the mask off the clip.
    #[arg(long)]
    #[serde(default)]
    pub remove: bool,
    /// Take every mask off the clip first.
    #[arg(long)]
    #[serde(default)]
    pub clear: bool,
}

impl Operation for MaskArgs {
    const NAME: &'static str = "mask";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let segment_id = session.with(|p| select::clip(p, &self.clip))?;
        let state = session.state.clone();
        let mut done = Vec::new();
        if self.clear {
            compositing::compositing_reset(&state, segment_id.clone(), ResetPart::Masks)?;
            done.push("cleared the masks".to_string());
        }
        let mut target: Option<String> = None;
        if let Some(shape) = &self.add {
            let shape = shape_named(shape)?;
            let (_, id) =
                compositing::compositing_add_mask(&state, segment_id.clone(), shape.clone())?;
            done.push(format!("added a {} mask", shape.label().to_lowercase()));
            target = Some(id);
        }
        let wants_mask = !self.set.is_empty()
            || self.shape.is_some()
            || self.op.is_some()
            || self.invert.is_some()
            || self.enabled.is_some()
            || self.remove;
        if wants_mask {
            let current = material(session, &segment_id);
            let mask_id = match (&self.mask, target) {
                (Some(reference), _) => mask_ref(&current, reference)?,
                (None, Some(id)) => id,
                (None, None) => match current.masks.as_slice() {
                    [one] => one.id.clone(),
                    [] => return Err(CliError::usage("the clip has no masks; add one with --add")),
                    _ => {
                        return Err(CliError::usage(
                            "the clip has several masks; name one with --mask",
                        ))
                    }
                },
            };
            if self.remove {
                compositing::compositing_remove_mask(&state, segment_id.clone(), mask_id)?;
                done.push("removed the mask".into());
            } else {
                let shape = self.shape.as_deref().map(shape_named).transpose()?;
                let op = self.op.as_deref().map(op_named).transpose()?;
                if shape.is_some()
                    || op.is_some()
                    || self.invert.is_some()
                    || self.enabled.is_some()
                {
                    compositing::compositing_set_mask_options(
                        &state,
                        segment_id.clone(),
                        mask_id.clone(),
                        shape,
                        op,
                        self.invert,
                        self.enabled,
                    )?;
                    done.push("changed the mask".into());
                }
                let at = self.at.map(|t| t.resolve(session.fps()));
                for (name, raw) in &self.set {
                    if !MASK_PARAMS.contains(&name.as_str()) {
                        return Err(CliError::usage(format!(
                            "a mask has no {name:?}; it has: {}",
                            MASK_PARAMS.join(", ")
                        )));
                    }
                    let value: f32 = raw
                        .trim()
                        .parse()
                        .ok()
                        .filter(|v: &f32| v.is_finite())
                        .ok_or_else(|| CliError::usage(format!("{name}={raw} is not a number")))?;
                    if let Some(at) = at {
                        // Keyframe at `at`: make sure the parameter is
                        // animated there, then set the value there.
                        let animated = material(session, &segment_id)
                            .mask(&mask_id)
                            .is_some_and(|m| m.is_animated(name));
                        if !animated {
                            compositing::compositing_toggle_mask_keyframe(
                                &state,
                                segment_id.clone(),
                                mask_id.clone(),
                                name.clone(),
                                at,
                            )?;
                        }
                    }
                    match compositing::compositing_set_mask_value(
                        &state,
                        segment_id.clone(),
                        mask_id.clone(),
                        name.clone(),
                        value,
                        at,
                    ) {
                        Ok(_) => {}
                        // The keyframe just added already holds the value.
                        Err(e) if e == "nothing changed" => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                if !self.set.is_empty() {
                    done.push(format!(
                        "set {}",
                        self.set
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        if done.is_empty() {
            return Err(CliError::usage(
                "mask needs --add, --set, --shape, --op, --invert, --enabled, --remove or --clear",
            ));
        }
        Ok(outcome(session, &segment_id, done.join("; ")))
    }
}

// ---------------------------------------------------------------------------
// Chroma key
// ---------------------------------------------------------------------------

/// Key a colour out of a clip (green screen). Give the colour (`color`,
/// #rrggbb) or pick it from the footage (`pick`: x,y on the canvas, 0..1
/// from the top left, at `at` or the clip's start). tolerance, softness,
/// spill and shrink are 0..1. `off` takes the key away.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ChromaKeyArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The colour to key out, e.g. #00ff00.
    #[arg(long)]
    pub color: Option<String>,
    /// Pick the colour from the footage at this canvas point, "x,y" in 0..1.
    #[arg(long)]
    pub pick: Option<String>,
    /// When to pick, on the timeline. Defaults to the clip's start.
    #[arg(long)]
    pub at: Option<Time>,
    /// How close to the colour still keys out fully, 0..1.
    #[arg(long)]
    pub tolerance: Option<f32>,
    /// How soft the edge between keyed and kept is, 0..1.
    #[arg(long)]
    pub softness: Option<f32>,
    /// How much of the colour's tint is taken out of what stays, 0..1.
    #[arg(long)]
    pub spill: Option<f32>,
    /// How far the edge is pulled in, 0..1.
    #[arg(long)]
    pub shrink: Option<f32>,
    /// Switch the key on (true) or off (false), keeping its values.
    #[arg(long)]
    pub enabled: Option<bool>,
    /// Take the key off the clip.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for ChromaKeyArgs {
    const NAME: &'static str = "chroma_key";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let segment_id = session.with(|p| select::clip(p, &self.clip))?;
        let state = session.state.clone();
        if self.off {
            compositing::compositing_set_key(&state, segment_id.clone(), None)?;
            return Ok(outcome(session, &segment_id, "took the key off".into()));
        }
        let mut color = self
            .color
            .as_deref()
            .map(parse_color)
            .transpose()
            .map_err(CliError::usage)?
            .map(|c| [c[0], c[1], c[2]]);
        if let Some(point) = &self.pick {
            let parts: Vec<f32> = point
                .split(',')
                .map(|v| v.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .map_err(|_| CliError::usage(format!("{point:?} is not x,y")))?;
            let [x, y] = parts[..] else {
                return Err(CliError::usage(format!("{point:?} is not x,y")));
            };
            let start = session.with(|p| p.segment(&segment_id).map(|(_, s)| s.target_range.start));
            let at = self
                .at
                .map(|t| t.resolve(session.fps()))
                .or(start)
                .unwrap_or(0);
            let picked = pollster::block_on(compositing::compositing_pick_key_color(
                &state,
                segment_id.clone(),
                at,
                [x, y],
            ))?;
            color = Some(picked);
        }
        let existing = material(session, &segment_id).key;
        let mut key = match (existing, color) {
            (Some(mut key), Some(color)) => {
                key.color = color;
                key
            }
            (Some(key), None) => key,
            (None, Some(color)) => ChromaKey::new(color),
            (None, None) => {
                return Err(CliError::usage(
                    "the clip has no key yet; give --color or --pick",
                ))
            }
        };
        for (value, slot) in [
            (self.tolerance, &mut key.tolerance),
            (self.softness, &mut key.softness),
            (self.spill, &mut key.spill),
            (self.shrink, &mut key.shrink),
        ] {
            if let Some(v) = value {
                if !v.is_finite() {
                    return Err(CliError::usage("key values must be numbers"));
                }
                *slot = v;
            }
        }
        if let Some(enabled) = self.enabled {
            key.enabled = enabled;
        }
        let shown = hex([key.color[0], key.color[1], key.color[2], 1.0]);
        compositing::compositing_set_key(&state, segment_id.clone(), Some(key))?;
        Ok(outcome(session, &segment_id, format!("keyed out {shown}")))
    }
}

// ---------------------------------------------------------------------------
// Remove background
// ---------------------------------------------------------------------------

/// Remove the background behind the people in a video clip (Robust Video
/// Matting in the ML worker; the model and ONNX Runtime download on first
/// use). Bakes the clip's matte into the cache before it returns, so a
/// following export or render shows it. `off` keeps the background again.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RemoveBackgroundArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Keep the background again.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for RemoveBackgroundArgs {
    const NAME: &'static str = "remove_background";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let segment_id = session.with(|p| select::clip(p, &self.clip))?;
        let state = session.state.clone();
        let answer = matting::matting_remove_background(&state, segment_id.clone(), !self.off)?;
        if self.off {
            return Ok(outcome(session, &segment_id, "kept the background".into()));
        }
        let Some(job) = answer.job else {
            return Ok(outcome(
                session,
                &segment_id,
                "removed the background (already baked)".into(),
            ));
        };
        let finished = loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let Some(status) = matting::matting_status(job) else {
                return Err(CliError::refused("the background removal job vanished"));
            };
            if let Some(finished) = status.finished {
                matting::matting_forget(job);
                break finished;
            }
            let p = &status.progress;
            if p.total > 0 {
                ctx.progress(
                    &format!("Removing the background: frame {} of {}", p.done, p.total),
                    Some(p.done as f32 / p.total as f32),
                );
            }
        };
        match finished {
            Ok(done) => Ok(outcome(
                session,
                &segment_id,
                format!(
                    "removed the background: {} frames in {:.1} s on {}",
                    done.written,
                    done.seconds,
                    done.provider.as_deref().unwrap_or("the CPU")
                ),
            )),
            // The setting stays on (an undo takes it off); the export bakes
            // what is missing or says why it cannot.
            Err(error) => Err(CliError::refused(format!(
                "Remove background is on, but its matte could not be made: {error}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Blend
// ---------------------------------------------------------------------------

/// How a clip blends with the lanes beneath it: normal, multiply, screen,
/// overlay, soft_light, hard_light, darken, lighten, color_dodge,
/// color_burn, difference, exclusion, add or subtract; and its opacity (0..1).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct BlendArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The blend mode.
    pub mode: Option<String>,
    /// The clip's opacity, 0..1.
    #[arg(long)]
    pub opacity: Option<f32>,
}

impl Operation for BlendArgs {
    const NAME: &'static str = "blend";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let segment_id = session.with(|p| select::clip(p, &self.clip))?;
        let state = session.state.clone();
        let mut done = Vec::new();
        if let Some(mode) = &self.mode {
            let normalized = mode.trim().to_ascii_lowercase().replace(['-', ' '], "_");
            let normalized = normalized.replace("colour", "color");
            let blend = BlendMode::parse(&normalized);
            if blend.code().is_none() && !blend.is_normal() {
                return Err(CliError::usage(format!(
                    "{mode:?} is not a blend mode; choose one of: {}",
                    BlendMode::NAMES.join(", ")
                )));
            }
            let current = material(session, &segment_id).blend;
            if current != blend {
                compositing::compositing_set_blend(&state, segment_id.clone(), blend.clone())?;
            }
            done.push(format!("blend {}", blend.name()));
        }
        if let Some(opacity) = self.opacity {
            if !opacity.is_finite() {
                return Err(CliError::usage("opacity must be a number"));
            }
            let before = session
                .with(|p| p.segment(&segment_id).map(|(_, s)| s.transform))
                .ok_or("the clip is gone")?;
            let mut after = before;
            after.opacity = opacity.clamp(0.0, 1.0);
            if after.opacity != before.opacity {
                timeline_commands::timeline_apply_many(
                    &state,
                    vec![EditCommand::SetTransform {
                        segment_id: segment_id.clone(),
                        before,
                        after,
                    }],
                    "Change opacity".into(),
                )?;
            }
            done.push(format!("opacity {}", after.opacity));
        }
        if done.is_empty() {
            return Err(CliError::usage("blend needs a mode or --opacity"));
        }
        Ok(outcome(session, &segment_id, done.join(", ")))
    }
}
