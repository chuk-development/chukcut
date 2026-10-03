//! Project templates: list them, make a project from one, save a project as
//! one, and put media into a slot. `docs/cli.md`, "Templates".

use std::path::{Path, PathBuf};

use chukcut_engine::modules::template::commands as template_commands;
use chukcut_engine::modules::template::commands::SaveRequest;
use chukcut_engine::state::AppState;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{summary, Ctx, Operation, Outcome};
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
/// files than slots leave the rest showing their placeholder.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TemplateApplyArgs {
    /// The template id (`template list`).
    pub template: String,
    /// Video and photo files, one per slot, in slot order.
    #[serde(default)]
    pub media: Vec<PathBuf>,
    /// The project's name. Defaults to the template's.
    #[arg(long)]
    pub name: Option<String>,
    /// Replace a file that already exists.
    #[arg(long)]
    #[serde(default)]
    pub force: bool,
}

impl TemplateApplyArgs {
    /// Build the project and a session that will save it to `path`.
    pub fn create(self, path: &Path) -> CliResult<(Session, Outcome)> {
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
        let id = match self.clip.trim().strip_prefix("slot:") {
            Some(n) => {
                let n: u32 = n
                    .parse()
                    .map_err(|_| CliError::usage(format!("{} is not a slot number", self.clip)))?;
                template_commands::template_slots(&session.state)?
                    .into_iter()
                    .find(|s| s.index == n)
                    .map(|s| s.segment_id)
                    .ok_or_else(|| CliError::refused(format!("the project has no slot {n}")))?
            }
            None => session.with(|p| select::clip(p, &self.clip))?,
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
        let clip = session.with(|p| summary::clip_by_id(p, &id));
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
                    "slot {}: {:.2} s, {}:{}{}{}",
                    s.index,
                    seconds(s.duration),
                    s.aspect[0],
                    s.aspect[1],
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
