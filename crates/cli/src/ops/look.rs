//! How clips look and move: grade, effects, animation, keyframes, titles,
//! transitions and motion tracking.

use chukcut_engine::modules::fx::{self, commands as fx_commands, ParamKind};
use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::inspector::edit::{GradeEdit, GradeSection};
use chukcut_engine::modules::motion::commands as motion_commands;
use chukcut_engine::modules::project::animation::{
    AnimationPreset, AnimationSlot, ClipAnimation, Ease, PunchZoom, StaggerOrder, TextAnimator,
    TextPreset, TextSlot, TextUnit,
};
use chukcut_engine::modules::project::effects::EffectValue;
use chukcut_engine::modules::project::{
    AnimatableProperty, Easing, Keyframe, LutRef, TextAlign, TextMaterial, TextShadow,
    TransitionKind,
};
use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use chukcut_engine::modules::tracking::commands::{self as tracking_commands, StartTracking};
use chukcut_engine::modules::tracking::job::Direction;
use chukcut_engine::modules::tracking::FollowMode;
use chukcut_engine::modules::transitions::commands as transition_commands;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{assignments, enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::{absolute, Session};
use crate::values::{parse_assignment, parse_color, seconds, srgb_to_linear, Time};

fn clip_outcome(session: &Session, id: &str, message: String) -> Outcome {
    let clip = session.with(|p| summary::clip_by_id(p, id));
    Outcome::changed(message, json!({"clip": clip}))
}

// ---------------------------------------------------------------------------
// Grade
// ---------------------------------------------------------------------------

/// Colour-grade a clip: set any of the grading panel's controls, attach or
/// remove a .cube LUT, or reset a section. All changes are one undo step.
/// `catalog grade` lists the controls and their resting values; values are
/// in document units (exposure in stops; saturation and contrast rest at 1,
/// so contrast=0.1 flattens the picture).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct GradeArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Controls to set, as name=value: exposure=0.5, saturation=1.2,
    /// hsl_hue:orange=-10, wheel_x:gain=0.1.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
    /// A .cube LUT file to attach, or "none" to remove the LUT.
    #[arg(long)]
    pub lut: Option<String>,
    /// How strongly the LUT applies, 0..1.
    #[arg(long)]
    pub lut_intensity: Option<f32>,
    /// Put a section back at rest first: basic, lut, hsl, curves, wheels or all.
    #[arg(long)]
    pub reset: Option<String>,
}

impl Operation for GradeArgs {
    const NAME: &'static str = "grade";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let mut steps = 0;
        if let Some(section) = &self.reset {
            let section: GradeSection = enum_named(
                "grade section",
                section,
                &["basic", "lut", "hsl", "curves", "wheels", "all"],
            )?;
            let has = session.with(|p| {
                p.segment(&id)
                    .and_then(|(_, s)| p.materials.color_adjust_of(s))
                    .is_some()
            });
            if has {
                inspector_commands::inspector_reset_grade(&session.state, id.clone(), section)?;
                steps += 1;
            }
        }

        let current = session.with(|p| {
            p.segment(&id)
                .map(|(_, s)| GradeEdit::of(p.materials.color_adjust_of(s)))
                .unwrap_or_default()
        });
        let mut edit = current.clone();
        if let Some(lut) = &self.lut {
            if lut.eq_ignore_ascii_case("none") {
                edit.lut = None;
            } else {
                let path = absolute(std::path::Path::new(lut));
                let path = path.to_string_lossy().into_owned();
                inspector_commands::inspector_lut_probe(path.clone())?;
                let intensity = edit.lut.as_ref().map_or(1.0, |l| l.intensity);
                edit.lut = Some(LutRef { path, intensity });
            }
        }
        if let Some(intensity) = self.lut_intensity {
            match edit.lut.as_mut() {
                Some(lut) => lut.intensity = intensity,
                None => return Err(CliError::usage("--lut-intensity needs a LUT on the clip")),
            }
        }
        let controls = summary::grade_controls();
        for (name, value) in &self.set {
            let key = name.trim().to_ascii_lowercase();
            let control = controls
                .iter()
                .find(|(n, _)| *n == key)
                .map(|(_, c)| *c)
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "{name} is not a grade control; `chukcut-cli catalog grade` lists them"
                    ))
                })?;
            let value: f32 = value
                .parse()
                .ok()
                .filter(|v: &f32| v.is_finite())
                .ok_or_else(|| CliError::usage(format!("{name}={value} is not a number")))?;
            control.set(&mut edit, value);
        }
        if edit != current {
            inspector_commands::inspector_set_grade(&session.state, id.clone(), Some(edit))?;
            steps += 1;
        }
        let graded = session.with(|p| {
            p.segment(&id)
                .map(|(_, s)| GradeEdit::of(p.materials.color_adjust_of(s)))
                .unwrap_or_default()
        });
        let mut values = summary::grade_values(&graded);
        if let Some(lut) = &graded.lut {
            values.insert("lut".into(), json!(lut.path));
        }
        Ok(Outcome {
            message: if steps > 0 {
                format!("graded clip: {} control(s) away from rest", values.len())
            } else {
                "the grade is already that".into()
            },
            data: json!({"clip": id, "grade": values}),
            mutated: steps > 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

/// Parse `value` for `param` of effect `kind` by the catalog's description of
/// it: a number in range, a colour, or one of a choice's options by name or
/// index.
fn effect_value(kind: &str, param: &str, value: &str) -> CliResult<EffectValue> {
    let desc = fx::descriptor(kind)
        .ok_or_else(|| CliError::usage(format!("there is no effect called {kind}")))?;
    let spec = desc.param(param).ok_or_else(|| {
        let names: Vec<&str> = desc.params.iter().map(|p| p.id).collect();
        CliError::usage(format!(
            "{kind} has no parameter {param}; it has: {}",
            names.join(", ")
        ))
    })?;
    match spec.kind {
        ParamKind::Number { min, max, .. } => {
            let v: f32 = value
                .parse()
                .ok()
                .filter(|v: &f32| v.is_finite())
                .ok_or_else(|| CliError::usage(format!("{param}={value} is not a number")))?;
            if v < min || v > max {
                return Err(CliError::usage(format!(
                    "{param} is between {min} and {max}, not {v}"
                )));
            }
            Ok(EffectValue::Number(v))
        }
        ParamKind::Color { .. } => Ok(EffectValue::Color(srgb_to_linear(parse_color(value)?))),
        ParamKind::Choice { options, .. } => {
            if let Ok(i) = value.parse::<usize>() {
                if i < options.len() {
                    return Ok(EffectValue::Number(i as f32));
                }
            }
            options
                .iter()
                .position(|o| o.eq_ignore_ascii_case(value))
                .map(|i| EffectValue::Number(i as f32))
                .ok_or_else(|| {
                    CliError::usage(format!("{param} is one of: {}", options.join(", ")))
                })
        }
    }
}

/// Add an effect (`catalog effects` lists them). With `clip`, it goes on the
/// end of that clip's stack. Without, it becomes an effect clip on an effect
/// lane at `at`, applying to everything beneath it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectAddArgs {
    /// The effect's id, e.g. gaussian_blur, glow, film_grain (`catalog effects`).
    pub kind: String,
    /// Put it on this clip: id, id prefix or `lane:index`.
    #[arg(long)]
    pub clip: Option<String>,
    /// For an effect clip: where it starts. Defaults to 0.
    #[arg(long, conflicts_with = "clip")]
    pub at: Option<Time>,
    /// For an effect clip: how long it lasts. Defaults to 3 s.
    #[arg(long, conflicts_with = "clip")]
    pub duration: Option<Time>,
    /// For an effect clip: the effect lane to use (index, name or id).
    #[arg(long, conflicts_with = "clip")]
    pub track: Option<String>,
    /// Parameters to set, as name=value.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
}

impl Operation for EffectAddArgs {
    const NAME: &'static str = "effect_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let kind = self.kind.trim().to_string();
        if fx::descriptor(&kind).is_none() {
            return Err(CliError::usage(format!(
                "there is no effect called {kind}; `chukcut-cli catalog effects` lists them"
            )));
        }
        // Check every value before anything is applied.
        let params: Vec<(String, EffectValue)> = self
            .set
            .iter()
            .map(|(k, v)| Ok((k.clone(), effect_value(&kind, k, v)?)))
            .collect::<CliResult<_>>()?;
        let fps = session.fps();

        let (segment_id, effect_id) = match &self.clip {
            Some(reference) => {
                let id = session.with(|p| select::clip(p, reference))?;
                let before: Vec<String> = stack(session, &id);
                fx_commands::fx_add(&session.state, id.clone(), kind.clone())?;
                let effect = stack(session, &id)
                    .into_iter()
                    .find(|e| !before.contains(e))
                    .ok_or("the effect was not added")?;
                (id, effect)
            }
            None => {
                let lane = self
                    .track
                    .as_deref()
                    .map(|t| session.with(|p| select::track(p, t)))
                    .transpose()?;
                let added = fx_commands::fx_add_clip(
                    &session.state,
                    kind.clone(),
                    self.at.map_or(0, |t| t.resolve(fps)),
                    self.duration.map(|d| d.resolve(fps)),
                    lane,
                )?;
                let effect = session
                    .with(|p| {
                        p.segment(&added.segment_id)
                            .map(|(_, s)| s.material_id.clone())
                    })
                    .ok_or("the effect clip was not added")?;
                (added.segment_id, effect)
            }
        };
        let effect_id = set_params(session, &segment_id, &effect_id, params, None)?;
        let clip = session.with(|p| summary::clip_by_id(p, &segment_id));
        Ok(Outcome::changed(
            format!("added {kind}"),
            json!({"clip": clip, "effect_id": effect_id}),
        ))
    }
}

/// The ids of a clip's effect stack, in order.
fn stack(session: &Session, segment_id: &str) -> Vec<String> {
    session.with(|p| {
        p.segment(segment_id)
            .map(|(_, s)| {
                p.materials
                    .effects_of(s)
                    .into_iter()
                    .map(|e| e.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Set several parameters of one effect as one undo step, or, with `at`, set
/// each at that timeline time (animated parameters get a keyframe there).
/// Returns the effect's id afterwards: materials are never edited in place,
/// so an edited effect has a new id.
fn set_params(
    session: &Session,
    segment_id: &str,
    effect_id: &str,
    params: Vec<(String, EffectValue)>,
    at: Option<i64>,
) -> CliResult<String> {
    if params.is_empty() {
        return Ok(effect_id.to_string());
    }
    let index = stack(session, segment_id)
        .iter()
        .position(|e| e == effect_id)
        .ok_or("the effect is not on the clip")?;
    match at {
        Some(at) => {
            for (name, value) in params {
                let current = stack(session, segment_id)[index].clone();
                fx_commands::fx_set_param(
                    &session.state,
                    segment_id.to_string(),
                    current,
                    name,
                    value,
                    Some(at),
                )?;
            }
        }
        None => {
            let mut material = session
                .with(|p| p.materials.effect(effect_id).cloned())
                .ok_or("the effect is not in the project")?;
            for (name, value) in params {
                // A parameter set without a time is a static value; any
                // animation it had is replaced, as setting a slider would be
                // if the parameter were not keyframed.
                material.keyframes.remove(&name);
                material.params.insert(name, value);
            }
            fx_commands::fx_set(
                &session.state,
                segment_id.to_string(),
                effect_id.to_string(),
                material,
            )?;
        }
    }
    Ok(stack(session, segment_id)[index].clone())
}

/// Change an effect on a clip: set parameters, switch it on or off. With
/// `at`, each parameter is set at that timeline time, which keyframes it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectSetArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's stack, its kind, or its id.
    pub effect: String,
    /// Parameters to set, as name=value.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
    /// Switch the effect on (true) or off (false).
    #[arg(long)]
    pub enabled: Option<bool>,
    /// Set the parameters at this timeline time, as keyframes.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for EffectSetArgs {
    const NAME: &'static str = "effect_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (segment_id, effect_id) = session.with(|p| -> CliResult<_> {
            let s = select::clip(p, &self.clip)?;
            let e = select::effect(p, &s, &self.effect)?;
            Ok((s, e))
        })?;
        let kind = session
            .with(|p| p.materials.effect(&effect_id).map(|e| e.kind.clone()))
            .ok_or("the effect is not in the project")?;
        let params: Vec<(String, EffectValue)> = self
            .set
            .iter()
            .map(|(k, v)| Ok((k.clone(), effect_value(&kind, k, v)?)))
            .collect::<CliResult<_>>()?;
        if params.is_empty() && self.enabled.is_none() {
            return Err(CliError::usage("effect set needs --set or --enabled"));
        }
        let fps = session.fps();
        let mut effect_id = set_params(
            session,
            &segment_id,
            &effect_id,
            params,
            self.at.map(|t| t.resolve(fps)),
        )?;
        if let Some(enabled) = self.enabled {
            let index = stack(session, &segment_id)
                .iter()
                .position(|e| *e == effect_id)
                .ok_or("the effect is not on the clip")?;
            let is = session
                .with(|p| p.materials.effect(&effect_id).map(|e| e.enabled))
                .unwrap_or(enabled);
            if is != enabled {
                fx_commands::fx_set_enabled(
                    &session.state,
                    segment_id.clone(),
                    effect_id,
                    enabled,
                )?;
            }
            effect_id = stack(session, &segment_id)[index].clone();
        }
        let clip = session.with(|p| summary::clip_by_id(p, &segment_id));
        Ok(Outcome::changed(
            format!("changed {kind}"),
            json!({"clip": clip, "effect_id": effect_id}),
        ))
    }
}

/// Take an effect off a clip. An effect clip's own effect goes by deleting
/// the clip.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectRemoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's stack, its kind, or its id.
    pub effect: String,
}

impl Operation for EffectRemoveArgs {
    const NAME: &'static str = "effect_remove";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (segment_id, effect_id) = session.with(|p| -> CliResult<_> {
            let s = select::clip(p, &self.clip)?;
            let e = select::effect(p, &s, &self.effect)?;
            Ok((s, e))
        })?;
        fx_commands::fx_remove(&session.state, segment_id.clone(), effect_id)?;
        Ok(clip_outcome(
            session,
            &segment_id,
            "removed the effect".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

fn ease(name: Option<&str>) -> CliResult<Option<Ease>> {
    name.map(|n| {
        enum_named(
            "easing",
            n,
            &[
                "linear",
                "ease_in",
                "ease_out",
                "ease_in_out",
                "smooth",
                "snap",
                "back_in",
                "back",
                "back_in_out",
                "elastic",
                "bounce",
            ],
        )
    })
    .transpose()
}

fn names_of<T: serde::Serialize>(items: impl IntoIterator<Item = T>) -> Vec<String> {
    items
        .into_iter()
        .filter_map(|i| serde_json::to_value(i).ok())
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// Give a clip an In, Out or Combo (looping) animation preset, or clear one
/// with preset "none". `catalog animations` lists the presets.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AnimateArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The preset (fade, slide_left, zoom_in, pop, pulse, ...) or "none".
    #[arg(long)]
    pub preset: String,
    /// in, out or combo.
    #[arg(long, default_value = "in")]
    #[serde(default = "slot_in")]
    pub slot: String,
    /// In and Out: how long it takes. Combo: one period of the loop.
    #[arg(long)]
    pub duration: Option<Time>,
    /// The easing curve (ease_out, linear, back, bounce, ...).
    #[arg(long)]
    pub easing: Option<String>,
    /// Scales how far the preset moves; 1 is as designed.
    #[arg(long)]
    pub strength: Option<f32>,
}

fn slot_in() -> String {
    "in".into()
}

impl Operation for AnimateArgs {
    const NAME: &'static str = "animate";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let slot: AnimationSlot = enum_named("slot", &self.slot, &["in", "out", "combo"])?;
        let animation = if self.preset.eq_ignore_ascii_case("none") {
            None
        } else {
            let catalog = motion_commands::motion_catalog();
            let valid = names_of(
                catalog
                    .clip
                    .iter()
                    .filter(|p| p.fits(slot))
                    .map(|p| p.preset),
            );
            let valid: Vec<&str> = valid.iter().map(String::as_str).collect();
            let preset: AnimationPreset = enum_named("animation preset", &self.preset, &valid)?;
            let descriptor = catalog
                .clip
                .iter()
                .find(|d| d.preset == preset)
                .ok_or("unknown preset")?;
            if !descriptor.fits(slot) {
                return Err(CliError::usage(format!(
                    "{} does not go in the {} slot; it goes in: {}",
                    self.preset,
                    self.slot,
                    valid.join(", ")
                )));
            }
            Some(ClipAnimation {
                preset,
                duration: self
                    .duration
                    .map_or(descriptor.duration, |d| d.resolve(session.fps())),
                easing: ease(self.easing.as_deref())?.unwrap_or(descriptor.easing),
                strength: self.strength.unwrap_or(1.0),
            })
        };
        motion_commands::motion_set_animation(&session.state, id.clone(), slot, animation)?;
        Ok(clip_outcome(
            session,
            &id,
            format!("animation {} set", self.slot),
        ))
    }
}

/// Animate a title's letters, words or lines in or out (typewriter, fade,
/// fade_up, pop, slide_up, drop, zoom, spin), or clear with preset "none".
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AnimateTextArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The preset or "none".
    #[arg(long)]
    pub preset: String,
    /// in or out.
    #[arg(long, default_value = "in")]
    #[serde(default = "slot_in")]
    pub slot: String,
    /// letter, word or line.
    #[arg(long)]
    pub unit: Option<String>,
    /// forward, backward, centre or random.
    #[arg(long)]
    pub order: Option<String>,
    /// The whole window, first unit's start to last unit's end.
    #[arg(long)]
    pub duration: Option<Time>,
    /// How much neighbouring units overlap, 0..1.
    #[arg(long)]
    pub overlap: Option<f32>,
    /// The easing curve (ease_out, linear, back, bounce, ...).
    #[arg(long)]
    pub easing: Option<String>,
    /// Scales the distance, size and angle a unit travels; 1 is as designed.
    #[arg(long)]
    pub strength: Option<f32>,
}

impl Operation for AnimateTextArgs {
    const NAME: &'static str = "animate_text";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let slot: TextSlot = enum_named("slot", &self.slot, &["in", "out"])?;
        let animator = if self.preset.eq_ignore_ascii_case("none") {
            None
        } else {
            let catalog = motion_commands::motion_catalog();
            let valid = names_of(catalog.text.iter().map(|p| p.preset));
            let valid: Vec<&str> = valid.iter().map(String::as_str).collect();
            let preset: TextPreset = enum_named("text animation", &self.preset, &valid)?;
            let mut animator: TextAnimator = catalog
                .text
                .iter()
                .find(|d| d.preset == preset)
                .map(|d| d.animator)
                .ok_or("unknown preset")?;
            if let Some(unit) = &self.unit {
                animator.unit = enum_named::<TextUnit>("unit", unit, &["letter", "word", "line"])?;
            }
            if let Some(order) = &self.order {
                animator.order = enum_named::<StaggerOrder>(
                    "order",
                    order,
                    &["forward", "backward", "centre", "random"],
                )?;
            }
            if let Some(d) = self.duration {
                animator.duration = d.resolve(session.fps());
            }
            if let Some(o) = self.overlap {
                animator.overlap = o;
            }
            if let Some(e) = ease(self.easing.as_deref())? {
                animator.easing = e;
            }
            if let Some(s) = self.strength {
                animator.strength = s;
            }
            Some(animator)
        };
        motion_commands::motion_set_text_animation(&session.state, id.clone(), slot, animator)?;
        Ok(clip_outcome(
            session,
            &id,
            format!("text animation {} set", self.slot),
        ))
    }
}

/// A punch-in zoom on a clip, or with `auto`, alternate 100% and `amount`
/// across the jump cuts around it, which hides cuts in a talking head.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ZoomArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Scale reached; 1.1 is 110%.
    #[arg(long)]
    pub amount: Option<f32>,
    /// The point that stays still, x in canvas units (0 is the centre).
    #[arg(long, allow_hyphen_values = true)]
    pub pivot_x: Option<f32>,
    /// The point that stays still, y in canvas units (+1 is the top).
    #[arg(long, allow_hyphen_values = true)]
    pub pivot_y: Option<f32>,
    /// How long the push takes from the clip's start; 0 is a hard punch.
    #[arg(long)]
    pub duration: Option<Time>,
    /// The easing curve of the push (ease_out, linear, back, ...).
    #[arg(long)]
    pub easing: Option<String>,
    /// Remove the zoom.
    #[arg(long)]
    #[serde(default)]
    pub clear: bool,
    /// Alternate the zoom across the clip's neighbouring jump cuts.
    #[arg(long)]
    #[serde(default)]
    pub auto: bool,
}

impl Operation for ZoomArgs {
    const NAME: &'static str = "zoom";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        if self.auto {
            motion_commands::motion_auto_zoom(
                &session.state,
                id.clone(),
                self.amount.unwrap_or(1.1),
            )?;
            return Ok(clip_outcome(session, &id, "auto zoom set".into()));
        }
        let zoom = if self.clear {
            None
        } else {
            let mut zoom = PunchZoom::default();
            if let Some(a) = self.amount {
                zoom.amount = a;
            }
            if let Some(x) = self.pivot_x {
                zoom.pivot[0] = x;
            }
            if let Some(y) = self.pivot_y {
                zoom.pivot[1] = y;
            }
            if let Some(d) = self.duration {
                zoom.duration = d.resolve(session.fps());
            }
            if let Some(e) = ease(self.easing.as_deref())? {
                zoom.easing = e;
            }
            Some(zoom)
        };
        motion_commands::motion_set_zoom(&session.state, id.clone(), zoom)?;
        Ok(clip_outcome(
            session,
            &id,
            if self.clear {
                "zoom removed".into()
            } else {
                "zoom set".into()
            },
        ))
    }
}

/// Put a keyframe on a clip property at a timeline time, change the one that
/// is there, or remove it. Properties: position_x, position_y, scale_x,
/// scale_y, rotation, opacity, volume.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct KeyframeArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The property to animate.
    #[arg(long)]
    pub property: String,
    /// The timeline time of the keyframe.
    #[arg(long)]
    pub at: Time,
    /// The value there (canvas units for position, 1 = 100% for scale,
    /// degrees for rotation, 0..1 for opacity, gain for volume).
    #[arg(long, allow_hyphen_values = true, required_unless_present = "remove")]
    pub value: Option<f32>,
    /// hold, linear, ease_in, ease_out or ease_in_out.
    #[arg(long)]
    pub easing: Option<String>,
    /// Remove the keyframe at that time.
    #[arg(long)]
    #[serde(default)]
    pub remove: bool,
}

impl Operation for KeyframeArgs {
    const NAME: &'static str = "keyframe";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let property: AnimatableProperty = enum_named(
            "property",
            &self.property,
            &[
                "position_x",
                "position_y",
                "scale_x",
                "scale_y",
                "rotation",
                "opacity",
                "volume",
            ],
        )?;
        let easing: Option<Easing> = self
            .easing
            .as_deref()
            .map(|e| {
                enum_named(
                    "keyframe easing",
                    e,
                    &["hold", "linear", "ease_in", "ease_out", "ease_in_out"],
                )
            })
            .transpose()?;
        let (start, existing) = session.with(|p| {
            let (_, s) = p.segment(&id).expect("selected");
            (s.target_range.start, s.keyframes.clone())
        });
        let time = self.at.resolve(session.fps()) - start;
        let current = existing
            .iter()
            .find(|t| t.property == property)
            .and_then(|t| t.keyframes.iter().find(|k| k.time == time))
            .copied();
        let mut commands = Vec::new();
        match (self.remove, current) {
            (true, Some(keyframe)) => commands.push(EditCommand::RemoveKeyframe {
                segment_id: id.clone(),
                property,
                keyframe,
            }),
            (true, None) => {
                return Err(CliError::refused(format!(
                    "there is no {} keyframe at that time",
                    self.property
                )))
            }
            (false, Some(keyframe)) => {
                let value = self
                    .value
                    .ok_or_else(|| CliError::usage("a keyframe needs --value"))?;
                if value != keyframe.value {
                    commands.push(EditCommand::MoveKeyframe {
                        segment_id: id.clone(),
                        property,
                        from_time: time,
                        to_time: time,
                        before_value: keyframe.value,
                        after_value: value,
                    });
                }
                if let Some(easing) = easing.filter(|e| *e != keyframe.easing) {
                    commands.push(EditCommand::SetKeyframeEasing {
                        segment_id: id.clone(),
                        property,
                        time,
                        before: keyframe.easing,
                        after: easing,
                    });
                }
            }
            (false, None) => {
                let value = self
                    .value
                    .ok_or_else(|| CliError::usage("a keyframe needs --value"))?;
                commands.push(EditCommand::AddKeyframe {
                    segment_id: id.clone(),
                    property,
                    keyframe: Keyframe {
                        time,
                        value,
                        easing: easing.unwrap_or_default(),
                    },
                });
            }
        }
        if commands.is_empty() {
            return Ok(Outcome::read(
                "the keyframe is already that",
                json!({"clip": id}),
            ));
        }
        timeline_commands::timeline_apply_many(&session.state, commands, "Keyframe".into())?;
        Ok(clip_outcome(
            session,
            &id,
            format!("keyframe at {:.3} s", seconds(time + start)),
        ))
    }
}

// ---------------------------------------------------------------------------
// Titles
// ---------------------------------------------------------------------------

/// The look of a title. Every field is optional; what is left out keeps its
/// current value (or the default for a new title).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleStyle {
    /// A font family installed on this machine (`catalog fonts`).
    #[arg(long)]
    pub font: Option<String>,
    /// Size in canvas pixels.
    #[arg(long)]
    pub size: Option<f32>,
    /// Text colour: #rrggbb, #rrggbbaa or a name.
    #[arg(long)]
    pub color: Option<String>,
    /// Bold on or off.
    #[arg(long)]
    pub bold: Option<bool>,
    /// Italic on or off.
    #[arg(long)]
    pub italic: Option<bool>,
    /// left, center or right.
    #[arg(long)]
    pub align: Option<String>,
    /// Outline width in pixels; 0 for none.
    #[arg(long)]
    pub stroke_width: Option<f32>,
    /// Outline colour.
    #[arg(long)]
    pub stroke_color: Option<String>,
    /// A box behind the text: a colour, or "none".
    #[arg(long)]
    pub background: Option<String>,
    /// Drop shadow on (true) or off (false).
    #[arg(long)]
    pub shadow: Option<bool>,
    /// Horizontal position in canvas units: 0 is the centre, 1 the right edge.
    #[arg(long, allow_hyphen_values = true)]
    pub x: Option<f32>,
    /// Vertical position in canvas units: 0 is the centre, 1 the top edge.
    #[arg(long, allow_hyphen_values = true)]
    pub y: Option<f32>,
}

impl TitleStyle {
    fn apply(&self, m: &mut TextMaterial) -> CliResult<()> {
        if let Some(font) = &self.font {
            m.font_family = font.clone();
        }
        if let Some(size) = self.size {
            m.font_size = size;
        }
        if let Some(c) = &self.color {
            m.color = parse_color(c)?;
        }
        if let Some(b) = self.bold {
            m.bold = b;
        }
        if let Some(i) = self.italic {
            m.italic = i;
        }
        if let Some(a) = &self.align {
            m.align = enum_named::<TextAlign>("alignment", a, &["left", "center", "right"])?;
        }
        if let Some(w) = self.stroke_width {
            m.stroke_width = w;
        }
        if let Some(c) = &self.stroke_color {
            m.stroke_color = parse_color(c)?;
        }
        if let Some(bg) = &self.background {
            m.background = if bg.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(parse_color(bg)?)
            };
        }
        match self.shadow {
            Some(false) => m.shadow = None,
            Some(true) if m.shadow.is_none() => {
                let o = (m.font_size / 12.0).round();
                m.shadow = Some(TextShadow {
                    color: [0.0, 0.0, 0.0, 0.55],
                    offset: [o, o],
                    blur: (m.font_size / 8.0).round(),
                })
            }
            _ => {}
        }
        Ok(())
    }

    /// The commands that bring title `segment_id` to this style.
    fn commands(&self, session: &Session, segment_id: &str) -> CliResult<Vec<EditCommand>> {
        let (material, transform) = session.with(|p| {
            let (_, s) = p.segment(segment_id).expect("selected");
            (p.materials.text(&s.material_id).cloned(), s.transform)
        });
        let before = material.ok_or("the clip is not a title")?;
        let mut after = before.clone();
        self.apply(&mut after)?;
        let mut commands = Vec::new();
        if after != before {
            text_check(&after)?;
            commands.push(EditCommand::SetTextMaterial { before, after });
        }
        if self.x.is_some() || self.y.is_some() {
            let mut moved = transform;
            if let Some(x) = self.x {
                moved.position[0] = x;
            }
            if let Some(y) = self.y {
                moved.position[1] = y;
            }
            commands.push(EditCommand::SetTransform {
                segment_id: segment_id.to_string(),
                before: transform,
                after: moved,
            });
        }
        Ok(commands)
    }
}

fn text_check(material: &TextMaterial) -> CliResult<()> {
    chukcut_engine::modules::text::edit::check_material(material).map_err(CliError::usage)
}

/// Put a title on the timeline. When `at` is taken on the title lane, the
/// title slides to the first gap after it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleAddArgs {
    /// The words.
    pub text: String,
    /// Where it starts. Defaults to 0.
    #[arg(long)]
    pub at: Option<Time>,
    /// How long it shows. Defaults to 3 s.
    #[arg(long)]
    pub duration: Option<Time>,
    /// The title lane: index, name or id. Without it the first title lane,
    /// where a title that overlaps another moves to the next gap; with it two
    /// titles can show at once (`lane-add --kind text` makes a lane).
    #[arg(long)]
    pub track: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub style: TitleStyle,
}

impl Operation for TitleAddArgs {
    const NAME: &'static str = "title_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        // Check the style before the title exists, so a bad colour does not
        // leave a half-made title behind.
        let mut probe = session
            .with(|p| chukcut_engine::modules::text::default_material(p, Some(self.text.clone())));
        self.style.apply(&mut probe)?;
        text_check(&probe)?;

        let lane = super::text::title_lane(session, self.track.as_deref())?;
        let added = text_commands::text_add_on(
            &session.state,
            self.at.map_or(0, |t| t.resolve(fps)),
            Some(self.text.clone()),
            self.duration.map(|d| d.resolve(fps)),
            lane,
        )?;
        let commands = self.style.commands(session, &added.segment_id)?;
        if !commands.is_empty() {
            timeline_commands::timeline_apply_many(&session.state, commands, "Style title".into())?;
        }
        Ok(clip_outcome(
            session,
            &added.segment_id,
            format!(
                "added title {:?} at {:.3} s",
                self.text,
                seconds(added.start)
            ),
        ))
    }
}

/// Change a title's words or look, as one undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleSetArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// New words.
    #[arg(long)]
    pub text: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub style: TitleStyle,
}

impl Operation for TitleSetArgs {
    const NAME: &'static str = "title_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let mut commands = self.style.commands(session, &id)?;
        if let Some(text) = &self.text {
            // Fold the new words into the material edit, so words and style
            // are one step.
            let base = session.with(|p| {
                p.segment(&id)
                    .and_then(|(_, s)| p.materials.text(&s.material_id).cloned())
            });
            let base = base.ok_or("the clip is not a title")?;
            match commands.iter_mut().find_map(|c| match c {
                EditCommand::SetTextMaterial { after, .. } => Some(after),
                _ => None,
            }) {
                Some(after) => after.content = text.clone(),
                None => {
                    let mut after = base.clone();
                    after.content = text.clone();
                    text_check(&after)?;
                    commands.insert(
                        0,
                        EditCommand::SetTextMaterial {
                            before: base,
                            after,
                        },
                    );
                }
            }
        }
        if commands.is_empty() {
            return Ok(Outcome::read("nothing to change", json!({"clip": id})));
        }
        timeline_commands::timeline_apply_many(&session.state, commands, "Edit title".into())?;
        Ok(clip_outcome(session, &id, "title changed".into()))
    }
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

/// Put a transition on the cut at the start of a clip (the incoming clip).
/// Built in: dissolve, dip_to_color, wipe, slide, zoom, blur; or any library
/// transition by its preset id (`catalog transitions`). Re-adding replaces.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TransitionAddArgs {
    /// The incoming clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The transition kind or library preset.
    #[arg(long, default_value = "dissolve")]
    #[serde(default = "dissolve")]
    pub kind: String,
    /// How long it takes. Defaults to the transition's own default, and is
    /// shortened to what the two clips allow.
    #[arg(long)]
    pub duration: Option<Time>,
}

fn dissolve() -> String {
    "dissolve".into()
}

impl Operation for TransitionAddArgs {
    const NAME: &'static str = "transition_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let duration = self.duration.map(|d| d.resolve(session.fps()));
        let had = session.with(|p| {
            p.segment(&id)
                .and_then(|(_, s)| p.materials.transition_of(s))
                .is_some()
        });
        if had {
            transition_commands::transitions_remove(&session.state, id.clone())?;
        }
        let name = self.kind.trim().to_ascii_lowercase().replace('-', "_");
        let builtin: Option<TransitionKind> = serde_json::from_value(Value::String(name.clone()))
            .ok()
            .filter(|k| *k != TransitionKind::Library);
        let result = match builtin {
            Some(kind) => {
                transition_commands::transitions_add(&session.state, id.clone(), kind, duration)
            }
            None => {
                let catalog = transition_commands::transitions_catalog();
                let presets: Vec<&str> = catalog.iter().filter_map(|d| d.preset).collect();
                let matches: Vec<&str> = presets
                    .iter()
                    .copied()
                    .filter(|p| {
                        p.eq_ignore_ascii_case(&self.kind)
                            || p.rsplit(':')
                                .next()
                                .is_some_and(|tail| tail.eq_ignore_ascii_case(&self.kind))
                    })
                    .collect();
                match matches.as_slice() {
                    [one] => transition_commands::transitions_add_preset(
                        &session.state,
                        id.clone(),
                        one.to_string(),
                        duration,
                    ),
                    [] => Err(format!(
                        "there is no transition called {}; `chukcut-cli catalog transitions` lists them",
                        self.kind
                    )),
                    many => Err(format!("{} could be {}", self.kind, many.join(" or "))),
                }
            }
        };
        if let Err(error) = result {
            if had {
                // Put back what was there: the replace is all or nothing.
                let _ = timeline_commands::timeline_undo(&session.state);
            }
            return Err(error.into());
        }
        Ok(clip_outcome(
            session,
            &id,
            format!("added {} transition", self.kind),
        ))
    }
}

/// Remove the transition at the start of a clip.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TransitionRemoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for TransitionRemoveArgs {
    const NAME: &'static str = "transition_remove";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        transition_commands::transitions_remove(&session.state, id.clone())?;
        Ok(clip_outcome(session, &id, "removed the transition".into()))
    }
}

// ---------------------------------------------------------------------------
// Motion tracking
// ---------------------------------------------------------------------------

/// Track a region of a video clip through its frames, and optionally make an
/// overlay (a title, a sticker, another clip) follow it. Blocks until the
/// analysis finishes; progress goes to stderr.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TrackArgs {
    /// The video clip to track in: id, id prefix or `lane:index`.
    pub clip: String,
    /// The timeline time the box is drawn at.
    #[arg(long)]
    pub at: Time,
    /// The box in the clip's own frame: centre x, centre y, width, height,
    /// as fractions of the frame (e.g. 0.5,0.4,0.2,0.2).
    #[arg(long)]
    pub rect: String,
    /// forward, backward or both.
    #[arg(long)]
    pub direction: Option<String>,
    /// Make this clip follow the track: id, id prefix or `lane:index`.
    #[arg(long)]
    pub overlay: Option<String>,
    /// position, position_scale or position_scale_rotation.
    #[arg(long)]
    pub mode: Option<String>,
}

impl Operation for TrackArgs {
    const NAME: &'static str = "track";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (target, overlay) = session.with(|p| -> CliResult<_> {
            Ok((
                select::clip(p, &self.clip)?,
                self.overlay
                    .as_deref()
                    .map(|o| select::clip(p, o))
                    .transpose()?,
            ))
        })?;
        let rect: Vec<f32> = self
            .rect
            .split(',')
            .map(|v| v.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|_| CliError::usage("--rect is cx,cy,w,h as fractions"))?;
        let [cx, cy, w, h] = rect[..] else {
            return Err(CliError::usage("--rect is four numbers: cx,cy,w,h"));
        };
        let request = StartTracking {
            target_segment_id: target.clone(),
            at: self.at.resolve(session.fps()),
            rect: [cx, cy, w, h],
            direction: self
                .direction
                .as_deref()
                .map(|d| enum_named::<Direction>("direction", d, &["forward", "backward", "both"]))
                .transpose()?
                .unwrap_or_default(),
            overlay_id: overlay.clone(),
            mode: self
                .mode
                .as_deref()
                .map(|m| {
                    enum_named::<FollowMode>(
                        "follow mode",
                        m,
                        &["position", "position_scale", "position_scale_rotation"],
                    )
                })
                .transpose()?
                .unwrap_or_default(),
            retrack: None,
        };
        let job = tracking_commands::tracking_start(&session.state, request, None)?;
        let outcome = loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let Some(status) = tracking_commands::tracking_status(job) else {
                return Err(CliError::refused("the tracking job vanished"));
            };
            if let Some(done) = status.finished {
                tracking_commands::tracking_forget(job);
                break done?;
            }
            if status.total > 0 {
                ctx.progress(
                    &format!("Tracking frame {} of {}", status.done, status.total),
                    Some(status.done as f32 / status.total as f32),
                );
            }
        };
        let mut data = json!({
            "track_id": outcome.track_id,
            "frames": outcome.frames,
            "seconds": outcome.seconds,
        });
        if let Some(o) = &overlay {
            data["overlay"] = session.with(|p| summary::clip_by_id(p, o));
        }
        Ok(Outcome::changed(
            format!("tracked {} frames", outcome.frames),
            data,
        ))
    }
}
