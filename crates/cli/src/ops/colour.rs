//! Colour tools that read the picture: auto adjust, colour match, and grade
//! presets. Each edit is one engine command (`grading::commands`) and one
//! undo step.

use chukcut_engine::modules::grading::commands::{self as grading, AutoAdjust, ColourMatch};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::Time;
use crate::On;

fn amount(value: Option<f32>) -> CliResult<f32> {
    let amount = value.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&amount) {
        return Err(CliError::usage("--amount is 0..1"));
    }
    Ok(amount)
}

fn grade_of(session: &Session, id: &str) -> serde_json::Value {
    session.with(|p| {
        let edit = chukcut_engine::modules::inspector::edit::GradeEdit::of_segment(p, id)
            .unwrap_or_default();
        json!(summary::grade_values(&edit))
    })
}

/// Balance a clip automatically: exposure, white balance and contrast from
/// its own pixels, written into its grade controls (exposure, temperature,
/// tint, whites, blacks, vibrance), which stay editable. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AutoAdjustArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// How far towards the full correction, 0..1. Default 1.
    #[arg(long)]
    pub amount: Option<f32>,
}

impl Operation for AutoAdjustArgs {
    const NAME: &'static str = "auto_adjust";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        ctx.progress("measuring the clip", None);
        let done = grading::grading_auto_adjust(
            &session.state,
            AutoAdjust {
                segment_id: id.clone(),
                amount: amount(self.amount)?,
            },
        )?;
        let c = done.controls;
        Ok(Outcome::changed(
            format!(
                "auto adjust: exposure {:+.2}, temperature {:+.2}, tint {:+.2}, whites {:+.2}, blacks {:+.2}",
                c.exposure, c.temperature, c.tint, c.whites, c.blacks
            ),
            json!({
                "clip": id,
                "controls": c,
                "before": done.before,
                "after": done.after,
                "grade": grade_of(session, &id),
            }),
        ))
    }
}

/// Grade a clip so its colours match another clip (or one frame of it): the
/// L*a*b* means and spreads through exposure, contrast, saturation,
/// temperature and tint, the rest of the histogram through the red, green
/// and blue curves. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ColourMatchArgs {
    /// The clip to grade: id, id prefix or `lane:index`.
    pub clip: String,
    /// The clip to match.
    #[arg(long)]
    pub to: String,
    /// Match the reference's frame at this timeline time instead of the
    /// whole reference clip.
    #[arg(long)]
    pub at: Option<Time>,
    /// How far towards the full match, 0..1. Default 1.
    #[arg(long)]
    pub amount: Option<f32>,
}

impl Operation for ColourMatchArgs {
    const NAME: &'static str = "colour_match";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (id, reference) = session.with(|p| -> CliResult<_> {
            Ok((select::clip(p, &self.clip)?, select::clip(p, &self.to)?))
        })?;
        let at = self.at.map(|t| t.resolve(session.fps()));
        ctx.progress("measuring both clips", None);
        let done = grading::grading_match(
            &session.state,
            ColourMatch {
                segment_id: id.clone(),
                reference_id: reference.clone(),
                at,
                amount: amount(self.amount)?,
            },
        )?;
        Ok(Outcome::changed(
            format!(
                "matched to {reference}: L*a*b* distance {:.1} -> {:.1}{}",
                done.distance_before,
                done.distance_after,
                if done.curves {
                    " (sliders and curves)"
                } else {
                    ""
                }
            ),
            json!({
                "clip": id,
                "reference": reference,
                "controls": done.controls,
                "curves": done.curves,
                "distance_before": done.distance_before,
                "distance_after": done.distance_after,
                "lab_reference": done.reference,
                "lab_before": done.before,
                "lab_after": done.after,
                "grade": grade_of(session, &id),
            }),
        ))
    }
}

/// Save a clip's whole grade as a named preset (Filters › My presets in the
/// app). A preset of that name is replaced only with --replace.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct GradePresetSaveArgs {
    /// The clip whose grade is saved.
    pub clip: String,
    /// The preset's name. Default: the next free "Preset N".
    #[arg(long)]
    pub name: Option<String>,
    /// Replace a preset of the same name.
    #[arg(long)]
    #[serde(default)]
    pub replace: bool,
}

impl Operation for GradePresetSaveArgs {
    const NAME: &'static str = "grade_preset_save";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let name = self
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(grading::grading_preset_name);
        let entry = grading::grading_save_preset(&session.state, id, name, self.replace)?;
        Ok(Outcome::read(
            format!("saved preset {:?}", entry.name),
            json!({"preset": entry}),
        ))
    }
}

/// Give a clip a saved grade preset, replacing its grade. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct GradePresetApplyArgs {
    /// The clip.
    pub clip: String,
    /// The preset's name, as `grade-preset list` shows it.
    pub name: String,
}

impl Operation for GradePresetApplyArgs {
    const NAME: &'static str = "grade_preset_apply";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        grading::grading_apply_preset(&session.state, id.clone(), self.name.clone())?;
        Ok(Outcome::changed(
            format!("applied preset {:?}", self.name),
            json!({"clip": id, "grade": grade_of(session, &id)}),
        ))
    }
}

/// List the saved grade presets, or delete one with --remove.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct GradePresetsArgs {
    /// Delete the preset of this name.
    #[arg(long)]
    pub remove: Option<String>,
}

impl Operation for GradePresetsArgs {
    const NAME: &'static str = "grade_presets";
    fn run(self, _: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if let Some(name) = &self.remove {
            grading::grading_delete_preset(name.clone())?;
        }
        let presets = grading::grading_presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        Ok(Outcome::read(
            match (&self.remove, names.is_empty()) {
                (Some(name), _) => format!("removed preset {name:?}"),
                (None, true) => "no grade presets saved".into(),
                (None, false) => format!("grade presets: {}", names.join(", ")),
            },
            json!({"presets": presets}),
        ))
    }
}

/// Top-level subcommands for the colour tools.
#[derive(Subcommand)]
pub enum ColourCommand {
    /// Balance a clip's exposure, white balance and contrast automatically.
    AutoAdjust(On<AutoAdjustArgs>),
    /// Grade a clip so its colours match another clip.
    ColourMatch(On<ColourMatchArgs>),
    /// Save a clip's grade as a preset.
    GradePresetSave(On<GradePresetSaveArgs>),
    /// Give a clip a saved grade preset.
    GradePresetApply(On<GradePresetApplyArgs>),
    /// List the grade presets, or delete one.
    GradePresets(On<GradePresetsArgs>),
}

impl ColourCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::AutoAdjust(o) => crate::on(o, dry, ctx),
            Self::ColourMatch(o) => crate::on(o, dry, ctx),
            Self::GradePresetSave(o) => crate::on(o, dry, ctx),
            Self::GradePresetApply(o) => crate::on(o, dry, ctx),
            Self::GradePresets(o) => crate::on(o, dry, ctx),
        }
    }
}
