//! Clip edits the first CLI left out: names, links, pasting attributes, a
//! grade for every clip, looks, the order and keyframes of effects,
//! transition settings, a title's words and typeface, and the order of
//! masks. Each is one engine command and one undo step.

use std::path::Path;

use chukcut_engine::modules::compositing::commands as compositing;
use chukcut_engine::modules::fx::commands as fx_commands;
use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::inspector::edit::{ClipAttributes, ColorEdit, GradeEdit};
use chukcut_engine::modules::library::commands as library_commands;
use chukcut_engine::modules::library::fonts::FontCategory;
use chukcut_engine::modules::project::document::LutRef;
use chukcut_engine::modules::project::{Easing, TransitionDirection};
use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::transitions::commands as transition_commands;
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{assignments, enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::{absolute, Session};
use crate::values::{parse_assignment, parse_color, seconds, srgb_to_linear, Time};
use crate::On;

fn clip_outcome(session: &Session, id: &str, message: impl Into<String>) -> Outcome {
    let clip = session.with(|p| summary::clip_by_id(p, id));
    Outcome::changed(message, json!({"clip": clip}))
}

fn clips(session: &Session, references: &[String]) -> CliResult<Vec<String>> {
    session.with(|p| {
        references
            .iter()
            .map(|r| select::clip(p, r))
            .collect::<CliResult<Vec<_>>>()
    })
}

// ---------------------------------------------------------------------------
// Names and links
// ---------------------------------------------------------------------------

/// Give a clip a name of its own, shown on the timeline instead of the
/// file's. An empty name puts the file's name back.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RenameArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The new name; empty to clear it.
    pub name: String,
}

impl Operation for RenameArgs {
    const NAME: &'static str = "rename";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let name = Some(self.name.trim().to_string()).filter(|n| !n.is_empty());
        inspector_commands::inspector_rename_clip(&session.state, id.clone(), name.clone())?;
        let mut outcome = clip_outcome(
            session,
            &id,
            match &name {
                Some(n) => format!("named the clip {n:?}"),
                None => "cleared the clip's name".into(),
            },
        );
        outcome.data["name"] = json!(name);
        Ok(outcome)
    }
}

/// Link clips so they move, trim and delete together (a picture and its
/// sound, or any group).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct LinkArgs {
    /// Two or more clips: ids, id prefixes or `lane:index`.
    #[arg(required = true, num_args = 2..)]
    pub clips: Vec<String>,
}

impl Operation for LinkArgs {
    const NAME: &'static str = "link";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.clips.len() < 2 {
            return Err(CliError::usage("link needs at least two clips"));
        }
        let ids = clips(session, &self.clips)?;
        timeline_commands::timeline_link(&session.state, ids.clone())?;
        Ok(Outcome::changed(
            format!("linked {} clips", ids.len()),
            json!({"clips": ids}),
        ))
    }
}

/// Break a clip's link, so it moves on its own again.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct UnlinkArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for UnlinkArgs {
    const NAME: &'static str = "unlink";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        timeline_commands::timeline_unlink(&session.state, id.clone())?;
        Ok(clip_outcome(session, &id, "unlinked the clip"))
    }
}

// ---------------------------------------------------------------------------
// Attributes and grades across clips
// ---------------------------------------------------------------------------

/// Copy one clip's position, scale, rotation, opacity, speed, volume, crop
/// and grade onto other clips, as one undo step ("Paste attributes").
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct PasteAttributesArgs {
    /// The clip to copy from: id, id prefix or `lane:index`.
    #[arg(long)]
    pub from: String,
    /// The clips to paste onto.
    #[arg(required = true)]
    pub clips: Vec<String>,
}

impl Operation for PasteAttributesArgs {
    const NAME: &'static str = "paste_attributes";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let source = session.with(|p| select::clip(p, &self.from))?;
        let targets = clips(session, &self.clips)?;
        if targets.is_empty() {
            return Err(CliError::usage(
                "paste attributes needs clips to paste onto",
            ));
        }
        let attributes = session.with(|p| {
            let (_, s) = p.segment(&source)?;
            let graded = p.materials.color_adjust_of(s);
            let edit = GradeEdit::of(graded);
            Some(ClipAttributes {
                transform: s.transform,
                speed: s.speed,
                volume: s.volume,
                crop: s.crop,
                color: graded.map(|_| ColorEdit {
                    brightness: edit.brightness,
                    contrast: edit.contrast,
                    saturation: edit.saturation,
                    temperature: edit.temperature,
                    lut: edit.lut.clone(),
                }),
                grade: graded.map(|_| edit.grade.clone()),
            })
        });
        let attributes = attributes.ok_or("the source clip is not in the project")?;
        inspector_commands::inspector_paste_attributes(
            &session.state,
            attributes,
            targets.clone(),
        )?;
        let data: Vec<Value> = session.with(|p| {
            targets
                .iter()
                .map(|id| summary::clip_by_id(p, id))
                .collect()
        });
        Ok(Outcome::changed(
            format!("pasted attributes onto {} clip(s)", targets.len()),
            json!({"clips": data}),
        ))
    }
}

/// Give every other picture clip this clip's grade, as one undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct GradeToAllArgs {
    /// The graded clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for GradeToAllArgs {
    const NAME: &'static str = "grade_to_all";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        inspector_commands::inspector_apply_color_to_all(&session.state, id.clone())?;
        Ok(clip_outcome(
            session,
            &id,
            "gave every picture clip this grade",
        ))
    }
}

/// Put a look on a clip: one of chukcut's own looks by name (`catalog
/// looks`), a .cube file (copied into the LUT library first), or "none" to
/// take the LUT off. The rest of the grade stays.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct LookArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// A look's name, a .cube file, or "none".
    pub look: String,
    /// How strongly it applies, 0..1. Default 1.
    #[arg(long)]
    pub intensity: Option<f32>,
}

impl Operation for LookArgs {
    const NAME: &'static str = "look";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let intensity = self.intensity.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&intensity) {
            return Err(CliError::usage("--intensity is 0..1"));
        }
        let wanted = self.look.trim();
        let (lut, name) = if wanted.eq_ignore_ascii_case("none") {
            (None, "no look".to_string())
        } else if wanted.to_ascii_lowercase().ends_with(".cube") {
            let path = absolute(Path::new(wanted));
            let entry = inspector_commands::inspector_lut_import(path.to_string_lossy().into())?;
            (
                Some(LutRef {
                    path: entry.path,
                    intensity,
                }),
                entry.name,
            )
        } else {
            // Writes our looks into the LUT library the first time; cheap
            // after that.
            library_commands::library_looks_install()?;
            let looks = library_commands::library_looks();
            let look = looks
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(wanted))
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "there is no look called {wanted:?}; `chukcut-cli catalog looks` lists them"
                    ))
                })?;
            (
                Some(LutRef {
                    path: look.path.clone(),
                    intensity,
                }),
                look.name.clone(),
            )
        };
        inspector_commands::inspector_set_lut(&session.state, id.clone(), lut)?;
        Ok(clip_outcome(session, &id, format!("look: {name}")))
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

fn clip_and_effect(session: &Session, clip: &str, effect: &str) -> CliResult<(String, String)> {
    session.with(|p| -> CliResult<_> {
        let s = select::clip(p, clip)?;
        let e = select::effect(p, &s, effect)?;
        Ok((s, e))
    })
}

/// Move an effect to another place in the clip's stack (0 is applied
/// first).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectMoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's stack, its kind, or its id.
    pub effect: String,
    /// Its new index.
    #[arg(long)]
    pub to: usize,
}

impl Operation for EffectMoveArgs {
    const NAME: &'static str = "effect_move";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (s, e) = clip_and_effect(session, &self.clip, &self.effect)?;
        fx_commands::fx_move(&session.state, s.clone(), e, self.to)?;
        Ok(clip_outcome(
            session,
            &s,
            format!("moved the effect to {}", self.to),
        ))
    }
}

/// Put every parameter of an effect back to its default.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectResetArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's stack, its kind, or its id.
    pub effect: String,
}

impl Operation for EffectResetArgs {
    const NAME: &'static str = "effect_reset";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (s, e) = clip_and_effect(session, &self.clip, &self.effect)?;
        fx_commands::fx_reset(&session.state, s.clone(), e)?;
        Ok(clip_outcome(session, &s, "reset the effect"))
    }
}

/// Add a keyframe for an effect parameter at a time, or remove the one
/// that is there. `effect set --at` sets a keyed value.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EffectKeyframeArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's stack, its kind, or its id.
    pub effect: String,
    /// The parameter (`catalog effects` lists them).
    #[arg(long)]
    pub param: String,
    /// The timeline time.
    #[arg(long)]
    pub at: Time,
}

impl Operation for EffectKeyframeArgs {
    const NAME: &'static str = "effect_keyframe";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (s, e) = clip_and_effect(session, &self.clip, &self.effect)?;
        let at = self.at.resolve(session.fps());
        fx_commands::fx_toggle_keyframe(&session.state, s.clone(), e, self.param.clone(), at)?;
        Ok(clip_outcome(
            session,
            &s,
            format!(
                "toggled the {} keyframe at {:.3} s",
                self.param,
                seconds(at)
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

const EASINGS: &[&str] = &["hold", "linear", "ease_in", "ease_out", "ease_in_out"];
const DIRECTIONS: &[&str] = &["left", "right", "up", "down"];

/// Change the transition at the start of a clip: its length (shortened to
/// what the two clips allow), easing, direction (wipe, slide), colour
/// (dip), softness (wipe), zoom (zoom) or a library transition's
/// parameters. One undo step per kind of change.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TransitionSetArgs {
    /// The incoming clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The new length.
    #[arg(long)]
    pub duration: Option<Time>,
    /// hold, linear, ease_in, ease_out or ease_in_out.
    #[arg(long)]
    pub easing: Option<String>,
    /// left, right, up or down.
    #[arg(long)]
    pub direction: Option<String>,
    /// The dip colour: #rrggbb or a name.
    #[arg(long)]
    pub color: Option<String>,
    /// The wipe's soft edge, as a fraction of the frame.
    #[arg(long)]
    pub softness: Option<f32>,
    /// The zoom's extra scale (0.35 reaches 1.35x).
    #[arg(long)]
    pub zoom: Option<f32>,
    /// A library transition's parameters, as name=value or name=v1,v2.
    #[arg(long = "param", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub params: Vec<(String, String)>,
}

impl Operation for TransitionSetArgs {
    const NAME: &'static str = "transition_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let current = session.with(|p| {
            p.segment(&id)
                .and_then(|(_, s)| p.materials.transition_of(s).cloned())
        });
        let current = current.ok_or_else(|| {
            CliError::usage("the clip has no transition; add one with `transition add`")
        })?;
        let mut steps = 0;
        let mut data = json!({});
        if let Some(duration) = self.duration {
            let wanted = duration.resolve(session.fps());
            let max = transition_commands::transitions_max_duration(&session.state, id.clone())?;
            let duration = wanted.min(max);
            if duration <= 0 {
                return Err(CliError::refused(
                    "the clips around this cut leave no room for a transition",
                ));
            }
            transition_commands::transitions_retime(&session.state, id.clone(), duration)?;
            data["duration"] = json!(seconds(duration));
            data["shortened"] = json!(duration < wanted);
            steps += 1;
        }
        let mut after = session
            .with(|p| {
                p.segment(&id)
                    .and_then(|(_, s)| p.materials.transition_of(s).cloned())
            })
            .unwrap_or(current);
        let before = after.clone();
        if let Some(e) = &self.easing {
            after.easing = enum_named::<Easing>("easing", e, EASINGS)?;
        }
        if let Some(d) = &self.direction {
            after.direction = enum_named::<TransitionDirection>("direction", d, DIRECTIONS)?;
        }
        if let Some(c) = &self.color {
            after.color = srgb_to_linear(parse_color(c)?);
        }
        if let Some(s) = self.softness {
            after.softness = s;
        }
        if let Some(z) = self.zoom {
            after.zoom = z;
        }
        for (name, raw) in &self.params {
            let values = raw
                .split(',')
                .map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
                .collect::<Option<Vec<f32>>>()
                .filter(|v| (1..=4).contains(&v.len()))
                .ok_or_else(|| {
                    CliError::usage(format!("{name}={raw} is not one to four numbers"))
                })?;
            after.params.insert(name.clone(), values);
        }
        if after != before {
            transition_commands::transitions_set(&session.state, id.clone(), after)?;
            steps += 1;
        }
        if steps == 0 {
            return Err(CliError::usage(
                "transition set needs --duration, --easing, --direction, --color, --softness, --zoom or --param",
            ));
        }
        let mut outcome = clip_outcome(session, &id, "changed the transition");
        outcome.data["transition"] = data;
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// Titles
// ---------------------------------------------------------------------------

fn title_material(session: &Session, clip: &str) -> CliResult<(String, String)> {
    let id = session.with(|p| select::clip(p, clip))?;
    let material = session.with(|p| {
        p.segment(&id)
            .and_then(|(_, s)| p.materials.text(&s.material_id).map(|m| m.id.clone()))
    });
    let material = material.ok_or_else(|| CliError::usage(format!("{clip} is not a title")))?;
    Ok((id, material))
}

/// Change only a title's words, keeping its style.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleTextArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The new words. `\n` in JSON, or a real line break, starts a line.
    pub text: String,
}

impl Operation for TitleTextArgs {
    const NAME: &'static str = "title_text";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (id, material) = title_material(session, &self.clip)?;
        text_commands::text_set_content(&session.state, &material, &self.text)?;
        Ok(clip_outcome(session, &id, "changed the title's words"))
    }
}

/// Set a title's typeface. A family this machine lacks is looked up in the
/// Fontsource catalogue, downloaded and installed first (needs the network
/// once).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleFontArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The font family, e.g. "Inter" or "Bebas Neue".
    pub family: String,
}

impl Operation for TitleFontArgs {
    const NAME: &'static str = "title_font";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (id, material) = title_material(session, &self.clip)?;
        let wanted = self.family.trim();
        let known = library_commands::library_system_fonts();
        let mut installed = false;
        let family = match known.iter().find(|f| f.eq_ignore_ascii_case(wanted)) {
            Some(f) => f.clone(),
            None => {
                ctx.progress("Looking the font up in the catalogue", None);
                let hits = library_commands::library_font_search(wanted, FontCategory::All)?;
                let entry = hits
                    .iter()
                    .find(|e| e.family.eq_ignore_ascii_case(wanted))
                    .ok_or_else(|| {
                        CliError::refused(format!(
                            "{wanted} is neither installed nor in the Fontsource catalogue"
                        ))
                    })?;
                ctx.progress(&format!("Installing {}", entry.family), None);
                let done = library_commands::library_font_install(entry)?;
                installed = true;
                done.families
                    .first()
                    .cloned()
                    .unwrap_or_else(|| entry.family.clone())
            }
        };
        let current = session.with(|p| p.materials.text(&material).map(|m| m.font_family.clone()));
        if current.as_deref() == Some(family.as_str()) {
            return Ok(Outcome::read(
                format!("the title is already in {family}"),
                json!({"clip": id, "family": family}),
            ));
        }
        library_commands::library_font_use_for_title(&session.state, &material, &family)?;
        let mut outcome = clip_outcome(session, &id, format!("title font: {family}"));
        outcome.data["family"] = json!(family);
        outcome.data["installed"] = json!(installed);
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

/// Move one of a clip's masks to another place in its order; masks combine
/// in order by their operation.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MaskMoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The mask: its index (0 is the first) or id.
    pub mask: String,
    /// Its new index.
    #[arg(long)]
    pub to: usize,
}

impl Operation for MaskMoveArgs {
    const NAME: &'static str = "mask_move";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let current = compositing::compositing_get(&session.state, id.clone())?;
        let mask = super::mask::mask_ref(&current, &self.mask)?;
        compositing::compositing_move_mask(&session.state, id.clone(), mask, self.to)?;
        let after = compositing::compositing_get(&session.state, id.clone())?;
        let mut outcome = clip_outcome(session, &id, format!("moved the mask to {}", self.to));
        outcome.data["compositing"] = super::mask::describe(&after);
        Ok(outcome)
    }
}

// ---------------------------------------------------------------------------
// The command line
// ---------------------------------------------------------------------------

/// Top-level subcommands for the clip edits above.
#[derive(Subcommand)]
pub enum ClipCommand {
    /// Give a clip a name of its own.
    Rename(On<RenameArgs>),
    /// Link clips so they move, trim and delete together.
    Link(On<LinkArgs>),
    /// Break a clip's link.
    Unlink(On<UnlinkArgs>),
    /// Copy one clip's transform, speed, volume, crop and grade onto others.
    PasteAttributes(On<PasteAttributesArgs>),
    /// Give every picture clip this clip's grade.
    GradeToAll(On<GradeToAllArgs>),
    /// Put a look (or a .cube LUT) on a clip.
    Look(On<LookArgs>),
    /// Move one of a clip's masks in its order.
    MaskMove(On<MaskMoveArgs>),
}

impl ClipCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Rename(o) => crate::on(o, dry, ctx),
            Self::Link(o) => crate::on(o, dry, ctx),
            Self::Unlink(o) => crate::on(o, dry, ctx),
            Self::PasteAttributes(o) => crate::on(o, dry, ctx),
            Self::GradeToAll(o) => crate::on(o, dry, ctx),
            Self::Look(o) => crate::on(o, dry, ctx),
            Self::MaskMove(o) => crate::on(o, dry, ctx),
        }
    }
}

/// `effect` subcommands for the stack and keyframes.
#[derive(Subcommand)]
pub enum EffectMoreCommand {
    /// Move an effect in the clip's stack.
    Move(On<EffectMoveArgs>),
    /// Put an effect's parameters back to their defaults.
    Reset(On<EffectResetArgs>),
    /// Add or remove a parameter's keyframe at a time.
    Keyframe(On<EffectKeyframeArgs>),
}

impl EffectMoreCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Move(o) => crate::on(o, dry, ctx),
            Self::Reset(o) => crate::on(o, dry, ctx),
            Self::Keyframe(o) => crate::on(o, dry, ctx),
        }
    }
}

/// `transition set`.
#[derive(Subcommand)]
pub enum TransitionMoreCommand {
    /// Change a transition's length, easing, direction, colour or parameters.
    Set(On<TransitionSetArgs>),
}

impl TransitionMoreCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Set(o) => crate::on(o, dry, ctx),
        }
    }
}

/// `title text` and `title font`.
#[derive(Subcommand)]
pub enum TitleMoreCommand {
    /// Change only a title's words.
    Text(On<TitleTextArgs>),
    /// Set a title's typeface, installing it from Fontsource when needed.
    Font(On<TitleFontArgs>),
}

impl TitleMoreCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Text(o) => crate::on(o, dry, ctx),
            Self::Font(o) => crate::on(o, dry, ctx),
        }
    }
}
