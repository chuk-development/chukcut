//! Project templates: list them, make a project from one, save a project as
//! one, and put media into a slot. `docs/cli.md`, "Templates".

use std::path::{Path, PathBuf};

use chukcut_engine::modules::template::commands as template_commands;
use chukcut_engine::modules::template::commands::{ApplyAs, SaveRequest};
use chukcut_engine::state::AppState;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::{absolute, Session};
use crate::values::{seconds, Time};

fn slot_json(slot: &template_commands::SlotInfo) -> Value {
    json!({
        "index": slot.index,
        "label": slot.label,
        "duration": seconds(slot.duration),
        "aspect": format!("{}:{}", slot.aspect[0], slot.aspect[1]),
        "accepts": slot.accepts,
    })
}

fn info_json(info: &template_commands::TemplateInfo) -> Value {
    json!({
        "id": info.id,
        "name": info.name,
        "description": info.description,
        "category": info.category,
        "builtin": info.builtin,
        "canvas": format!("{}x{}", info.width, info.height),
        "duration": seconds(info.duration),
        "slots": info.slots.iter().map(slot_json).collect::<Vec<_>>(),
        "path": info.path,
    })
}

/// List the templates: the built-ins, then your own.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateListArgs {}

impl TemplateListArgs {
    pub fn run(self) -> CliResult<Outcome> {
        let list = template_commands::template_list();
        let lines: Vec<String> = list
            .iter()
            .map(|t| {
                format!(
                    "{:<20} {} slot{}, {:.1} s, {}x{}  {}",
                    t.id,
                    t.slots.len(),
                    if t.slots.len() == 1 { "" } else { "s" },
                    seconds(t.duration),
                    t.width,
                    t.height,
                    t.name
                )
            })
            .collect();
        Ok(Outcome::read(
            lines.join("\n"),
            json!(list.iter().map(info_json).collect::<Vec<_>>()),
        ))
    }
}

/// Make a new project file from a template, your files filling its slots in
/// order: a longer clip is trimmed to its slot, a shorter one slowed to fill
/// it, and a picture of another shape is cropped to the slot's shape. Fewer
/// files than slots leave the rest showing their placeholder. With `into`,
/// the template goes into the existing project instead, as a new timeline
/// or as a compound clip (`as`).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateApplyArgs {
    /// The template id (`template list`).
    pub template: String,
    /// Video and photo files, one per slot, in slot order.
    #[serde(default)]
    pub media: Vec<PathBuf>,
    /// The project's name (with --into: the timeline's or compound clip's).
    /// Defaults to the template's.
    #[arg(long)]
    pub name: Option<String>,
    /// Replace a file that already exists.
    #[arg(long)]
    #[serde(default)]
    pub force: bool,
    /// Put the template into the existing project instead of writing a new
    /// one.
    #[arg(long)]
    #[serde(default)]
    pub into: bool,
    /// With --into: `timeline` (a new timeline tab, the default) or
    /// `compound` (a compound clip at --at).
    #[arg(long = "as", value_name = "timeline|compound")]
    #[serde(default, rename = "as")]
    pub mode: Option<String>,
    /// With --into --as compound: where the compound clip starts. Default 0.
    #[arg(long)]
    #[serde(default)]
    pub at: Option<Time>,
}

impl TemplateApplyArgs {
    /// The `--into` form, as the operation on an open project.
    pub fn into_args(self) -> TemplateApplyIntoArgs {
        TemplateApplyIntoArgs {
            template: self.template,
            media: self.media,
            mode: self.mode,
            at: self.at,
            name: self.name,
        }
    }

    /// Build the project and a session that will save it to `path`.
    pub fn create(self, path: &Path) -> CliResult<(Session, Outcome)> {
        if self.mode.is_some() || self.at.is_some() {
            return Err(CliError::usage(
                "--as and --at put a template into an existing project; add --into",
            ));
        }
        let path = absolute(path);
        if path.exists() && !self.force {
            return Err(CliError::project(format!(
                "{} already exists; pass --force to replace it",
                path.display()
            )));
        }
        let media: Vec<String> = self
            .media
            .iter()
            .map(|m| {
                let m = absolute(m);
                m.canonicalize().unwrap_or(m).to_string_lossy().into_owned()
            })
            .collect();
        let state = AppState::new();
        let applied =
            template_commands::template_new_project(&state, &self.template, &media, self.name)?;
        let session = Session {
            state,
            path: path.clone(),
            dirty: true,
            disk_mtime: None,
        };
        let empty = applied.empty.len();
        let message = format!(
            "created {} from {} ({} slot{} filled{})",
            path.display(),
            self.template,
            applied.filled.len(),
            if applied.filled.len() == 1 { "" } else { "s" },
            if empty > 0 {
                format!(", {empty} still empty")
            } else {
                String::new()
            }
        );
        let data = json!({
            "path": path,
            "filled": applied.filled.iter().map(|f| json!({
                "slot": f.index,
                "clip": f.segment_id,
                "file": f.path,
                "slowed_to": f.slowed_to,
                "cropped": f.cropped,
            })).collect::<Vec<_>>(),
            "empty": applied.empty,
        });
        Ok((session, Outcome::changed(message, data)))
    }
}

/// Put a template into this project, your files filling its slots in order:
/// as a new timeline (opened) or as a compound clip at a time, on the first
/// video lane with room. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateApplyIntoArgs {
    /// The template id (`template list`).
    pub template: String,
    /// Video and photo files, one per slot, in slot order.
    #[serde(default)]
    pub media: Vec<PathBuf>,
    /// `timeline` (a new timeline tab, the default) or `compound` (a
    /// compound clip at `at`).
    #[arg(long = "as", value_name = "timeline|compound")]
    #[serde(default, rename = "as")]
    pub mode: Option<String>,
    /// Where the compound clip starts. Default 0.
    #[arg(long)]
    #[serde(default)]
    pub at: Option<Time>,
    /// The timeline's or compound clip's name. Defaults to the template's.
    #[arg(long)]
    #[serde(default)]
    pub name: Option<String>,
}

impl Operation for TemplateApplyIntoArgs {
    const NAME: &'static str = "template_apply_into";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let mode: ApplyAs = match self.mode.as_deref() {
            None => ApplyAs::Timeline,
            Some(name) => enum_named("way to apply a template", name, &["timeline", "compound"])?,
        };
        if self.at.is_some() && mode == ApplyAs::Timeline {
            return Err(CliError::usage(
                "--at places a compound clip; add --as compound",
            ));
        }
        let media: Vec<String> = self
            .media
            .iter()
            .map(|m| {
                let m = absolute(m);
                m.canonicalize().unwrap_or(m).to_string_lossy().into_owned()
            })
            .collect();
        let at = self.at.map(|t| t.resolve(session.fps()));
        let applied = template_commands::template_apply_into(
            &session.state,
            &self.template,
            &media,
            mode,
            at,
            self.name,
        )?;
        let empty = applied.empty.len();
        let mut message = format!(
            "put {} into the project as {} ({} slot{} filled{})",
            self.template,
            mode.label(),
            applied.filled.len(),
            if applied.filled.len() == 1 { "" } else { "s" },
            if empty > 0 {
                format!(", {empty} still empty")
            } else {
                String::new()
            }
        );
        if let Some(note) = &applied.note {
            message.push_str(&format!("; {}", note.to_lowercase()));
        }
        let clip = applied
            .segment_id
            .as_ref()
            .map(|id| session.with(|p| summary::clip_by_id(p, id)));
        Ok(Outcome::changed(
            message,
            json!({
                "as": mode,
                "sequence": applied.sequence_id,
                "clip": clip,
                "filled": applied.filled.iter().map(|f| json!({
                    "slot": f.index,
                    "clip": f.segment_id,
                    "file": f.path,
                    "slowed_to": f.slowed_to,
                    "cropped": f.cropped,
                })).collect::<Vec<_>>(),
                "empty": applied.empty,
                "note": applied.note,
            }),
        ))
    }
}

/// Save this project as a template of your own. The clips you name become
/// its slots, in that order; without any, the slots it already has are
/// kept. The media it still uses is copied into the template.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateSaveArgs {
    /// The template's name.
    #[arg(long)]
    pub name: String,
    /// Clips that become slots, in fill order (id, id prefix or `lane:index`).
    #[arg(long = "slot")]
    #[serde(default)]
    pub slots: Vec<String>,
    /// A label per slot, in the same order ("Opening shot").
    #[arg(long = "label")]
    #[serde(default)]
    pub labels: Vec<String>,
    /// One line about what it is for.
    #[arg(long)]
    #[serde(default)]
    pub description: Option<String>,
    /// Where the Templates tab files it. Defaults to "My templates".
    #[arg(long)]
    #[serde(default)]
    pub category: Option<String>,
}

impl Operation for TemplateSaveArgs {
    const NAME: &'static str = "template_save";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let slots = self
            .slots
            .iter()
            .map(|r| session.with(|p| select::clip(p, r)))
            .collect::<CliResult<Vec<_>>>()?;
        let request = SaveRequest {
            name: self.name,
            description: self.description.unwrap_or_default(),
            category: self.category.unwrap_or_default(),
            slots,
            labels: self.labels,
        };
        let info = template_commands::template_save_project(&session.project(), &request)?;
        Ok(Outcome::read(
            format!(
                "saved template {} with {} slot{}",
                info.id,
                info.slots.len(),
                if info.slots.len() == 1 { "" } else { "s" }
            ),
            info_json(&info),
        ))
    }
}

/// Put a file into a slot — or any video or photo clip — keeping the clip's
/// place, length, animation and look. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateReplaceArgs {
    /// The clip (id, id prefix or `lane:index`), or `slot:N` for slot N.
    #[arg(long)]
    pub clip: String,
    /// The video or photo to put there.
    #[arg(long)]
    pub media: PathBuf,
    /// Where in a longer clip the slot starts reading.
    #[arg(long)]
    pub from: Option<Time>,
}

impl Operation for TemplateReplaceArgs {
    const NAME: &'static str = "template_replace";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let slots = template_commands::template_slots(&session.state)?;
        let id = match self.clip.trim().strip_prefix("slot:") {
            Some(n) => {
                let n: u32 = n
                    .parse()
                    .map_err(|_| CliError::usage(format!("{} is not a slot number", self.clip)))?;
                // Two timelines made from templates both have a slot 1: the
                // open timeline's is meant.
                let root =
                    session.with(|p| chukcut_engine::modules::sequence::root_id(p).to_string());
                slots
                    .iter()
                    .filter(|s| s.index == n)
                    .min_by_key(|s| s.timeline_id != root)
                    .map(|s| s.segment_id.clone())
                    .ok_or_else(|| CliError::refused(format!("the project has no slot {n}")))?
            }
            // A slot inside a compound clip is not on the open timeline's
            // lanes; its id (or a prefix) still names it.
            None => session
                .with(|p| select::clip(p, &self.clip))
                .or_else(|error| {
                    let wanted = self.clip.trim();
                    let mut found = slots
                        .iter()
                        .filter(|s| s.segment_id.starts_with(wanted) && !wanted.is_empty());
                    match (found.next(), found.next()) {
                        (Some(slot), None) => Ok(slot.segment_id.clone()),
                        _ => Err(error),
                    }
                })?,
        };
        let media = absolute(&self.media);
        let media = media.canonicalize().unwrap_or(media);
        let from = self.from.map(|t| t.resolve(session.fps()));
        let replaced = template_commands::template_replace_media(
            &session.state,
            &id,
            &media.to_string_lossy(),
            from,
        )?;
        let clip = session.with(|p| {
            if p.segment(&id).is_some() {
                summary::clip_by_id(p, &id)
            } else {
                // Inside a compound clip: not on the open timeline's lanes.
                json!({ "id": id })
            }
        });
        let mut message = format!("put {} into the clip", media.display());
        if let Some(speed) = replaced.slowed_to {
            message.push_str(&format!(", slowed to {speed:.2}x to fill it"));
        }
        Ok(Outcome::changed(
            message,
            json!({"clip": clip, "slowed_to": replaced.slowed_to, "cropped": replaced.cropped}),
        ))
    }
}

/// List this project's slots, filled or not.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateSlotsArgs {}

impl Operation for TemplateSlotsArgs {
    const NAME: &'static str = "template_slots";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let slots = template_commands::template_slots(&session.state)?;
        let lines: Vec<String> = slots
            .iter()
            .map(|s| {
                format!(
                    "slot {}: {:.2} s, {}:{}{}{}{}",
                    s.index,
                    seconds(s.duration),
                    s.aspect[0],
                    s.aspect[1],
                    if s.in_compound {
                        format!(" in compound clip \u{201c}{}\u{201d}", s.sequence_name)
                    } else {
                        String::new()
                    },
                    s.label
                        .as_ref()
                        .map(|l| format!(" \u{201c}{l}\u{201d}"))
                        .unwrap_or_default(),
                    s.media_path
                        .as_ref()
                        .map(|p| format!(" \u{2190} {p}"))
                        .unwrap_or_else(|| " (empty)".into())
                )
            })
            .collect();
        Ok(Outcome::read(
            if lines.is_empty() {
                "the project has no slots".to_string()
            } else {
                lines.join("\n")
            },
            json!(slots
                .iter()
                .map(|s| json!({
                    "index": s.index,
                    "clip": s.segment_id,
                    "label": s.label,
                    "start": seconds(s.start),
                    "duration": seconds(s.duration),
                    "aspect": format!("{}:{}", s.aspect[0], s.aspect[1]),
                    "accepts": s.accepts,
                    "filled": s.filled,
                    "media": s.media_path,
                    "sequence": s.sequence_id,
                    "sequence_name": s.sequence_name,
                    "in_compound": s.in_compound,
                    "timeline": s.timeline_id,
                }))
                .collect::<Vec<_>>()),
        ))
    }
}

/// Delete one of your own templates.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateDeleteArgs {
    /// The template id (`template list`).
    pub template: String,
}

impl TemplateDeleteArgs {
    pub fn run(self) -> CliResult<Outcome> {
        template_commands::template_delete(&self.template)?;
        Ok(Outcome::read(
            format!("deleted template {}", self.template),
            json!({"id": self.template}),
        ))
    }
}
