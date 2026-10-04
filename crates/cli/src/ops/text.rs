//! Title styles, text templates, position presets and duplicates: the
//! `title` commands beyond `title add` and `title set`.

use chukcut_engine::modules::project::new_id;
use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::text::edit as text_edit;
use chukcut_engine::modules::text::presets::{self, TextPosition};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};

const POSITIONS: &[&str] = &[
    "top_left",
    "top",
    "top_right",
    "left",
    "centre",
    "right",
    "bottom_left",
    "bottom",
    "bottom_right",
];

/// What `catalog title_styles` lists.
pub fn styles_catalog() -> Value {
    json!(text_commands::text_styles()
        .into_iter()
        .map(|s| json!({
            "id": s.id,
            "name": s.name,
            "category": s.category,
            "sample": s.sample,
            "text": format!("{} ({})", s.name, s.category.label()),
        }))
        .collect::<Vec<_>>())
}

/// What `catalog title_templates` lists.
pub fn templates_catalog() -> Value {
    json!(text_commands::text_templates()
        .into_iter()
        .map(|t| json!({
            "id": t.id,
            "name": t.name,
            "style": t.style,
            "sample": t.sample(),
            "text": format!("{} (style {})", t.name, t.style),
        }))
        .collect::<Vec<_>>())
}

fn title_outcome(session: &Session, id: &str, message: String) -> Outcome {
    let clip = session.with(|p| summary::clip_by_id(p, id));
    Outcome::changed(message, json!({"clip": clip}))
}

/// Where a styled title goes and what it says, for a new one.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct NewTitle {
    /// Restyle this title clip (id, id prefix or `lane:index`) instead of
    /// adding a new one. It keeps its words, size and place.
    #[arg(long, conflicts_with_all = ["at", "text", "track"])]
    pub clip: Option<String>,
    /// A new title starts here. Defaults to 0; when the time is taken on
    /// the title lane, the title moves to the next gap.
    #[arg(long)]
    pub at: Option<Time>,
    /// The words of a new title. Defaults to the style's sample.
    #[arg(long)]
    pub text: Option<String>,
    /// The title lane for a new title: index, name or id (`lane-add --kind
    /// text` makes one).
    #[arg(long)]
    pub track: Option<String>,
    /// How long a new title shows. Defaults to 3 s.
    #[arg(long, conflicts_with = "clip")]
    pub duration: Option<Time>,
}

impl NewTitle {
    fn lane(&self, session: &Session) -> CliResult<Option<String>> {
        title_lane(session, self.track.as_deref())
    }
}

/// The lane `reference` names, refused unless a title may go there: a text
/// lane that is not locked and holds no captions. Without the check the
/// engine would quietly use the first title lane instead.
pub fn title_lane(session: &Session, reference: Option<&str>) -> CliResult<Option<String>> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let id = session.with(|p| select::track(p, reference))?;
    let captions: Vec<String> =
        chukcut_engine::modules::captions::commands::captions_list(&session.state)?
            .into_iter()
            .map(|c| c.track_id)
            .collect();
    let usable = session.with(|p| {
        p.track(&id).is_some_and(|t| {
            t.kind == chukcut_engine::modules::project::TrackKind::Text
                && !t.locked
                && !captions.contains(&t.id)
        })
    });
    if !usable {
        return Err(CliError::usage(format!(
            "lane {reference} is not a title lane (a text lane, unlocked, without captions); \
             `chukcut-cli lane-add PROJECT --kind text` adds one"
        )));
    }
    Ok(Some(id))
}

/// Add a title in a style, or restyle a title. `catalog title_styles` lists
/// the styles. A restyle keeps the words and the size. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleStyleArgs {
    /// The style id (`catalog title_styles`).
    pub style: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub target: NewTitle,
}

impl Operation for TitleStyleArgs {
    const NAME: &'static str = "title_style";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let style = self.style.trim();
        if presets::style(style).is_none() {
            return Err(CliError::usage(format!(
                "there is no title style called {style}; `chukcut-cli catalog title_styles` lists them"
            )));
        }
        if let Some(reference) = &self.target.clip {
            let id = session.with(|p| select::clip(p, reference))?;
            text_commands::text_apply_style(&session.state, &id, style)?;
            return Ok(title_outcome(
                session,
                &id,
                format!("styled title as {style}"),
            ));
        }
        let lane = self.target.lane(session)?;
        let at = self.target.at.map_or(0, |t| t.resolve(session.fps()));
        let duration = self.target.duration.map(|d| d.resolve(session.fps()));
        let added = text_commands::text_add_style_for(
            &session.state,
            at,
            style,
            self.target.text.clone(),
            lane,
            duration,
        )?;
        Ok(title_outcome(
            session,
            &added.segment_id,
            format!("added a {style} title at {:.3} s", seconds(added.start)),
        ))
    }
}

/// Add a title from a template (a style and an animation), or give a title
/// a template's look and motion. `catalog title_templates` lists them. One
/// undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleTemplateArgs {
    /// The template id (`catalog title_templates`).
    pub template: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub target: NewTitle,
}

impl Operation for TitleTemplateArgs {
    const NAME: &'static str = "title_template";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let template = self.template.trim();
        if presets::template(template).is_none() {
            return Err(CliError::usage(format!(
                "there is no text template called {template}; `chukcut-cli catalog title_templates` lists them"
            )));
        }
        if let Some(reference) = &self.target.clip {
            let id = session.with(|p| select::clip(p, reference))?;
            text_commands::text_apply_template(&session.state, &id, template)?;
            return Ok(title_outcome(
                session,
                &id,
                format!("gave the title the {template} template"),
            ));
        }
        let lane = self.target.lane(session)?;
        let at = self.target.at.map_or(0, |t| t.resolve(session.fps()));
        let duration = self.target.duration.map(|d| d.resolve(session.fps()));
        let added = text_commands::text_add_template_for(
            &session.state,
            at,
            template,
            self.target.text.clone(),
            lane,
            duration,
        )?;
        Ok(title_outcome(
            session,
            &added.segment_id,
            format!("added a {template} title at {:.3} s", seconds(added.start)),
        ))
    }
}

/// Move a title to a cell of the 3 x 3 grid and align its text to match:
/// top_left, top, top_right, left, centre, right, bottom_left, bottom or
/// bottom_right. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitlePositionArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The grid cell.
    #[arg(long)]
    pub position: String,
}

impl Operation for TitlePositionArgs {
    const NAME: &'static str = "title_position";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let name = self
            .position
            .trim()
            .to_ascii_lowercase()
            .replace("center", "centre");
        let position: TextPosition = enum_named("title position", &name, POSITIONS)?;
        text_commands::text_set_position(&session.state, &id, position)?;
        Ok(title_outcome(
            session,
            &id,
            format!("moved the title to {name}"),
        ))
    }
}

/// Copy a title, with its own words and style, its transform, keyframes and
/// animation. The copy goes right after the original on the same lane
/// unless `at` says otherwise. Editing one title then does not change the
/// other. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TitleDuplicateArgs {
    /// The title clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Where the copy starts. Defaults to the original's end.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for TitleDuplicateArgs {
    const NAME: &'static str = "title_duplicate";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let (material_id, lane, range, transform, keyframes, crop, animation) =
            session.with(|p| -> CliResult<_> {
                let (track, segment) = p.segment(&id).ok_or("the clip is gone")?;
                let text = p
                    .materials
                    .text(&segment.material_id)
                    .ok_or("the clip is not a title")?;
                if text.caption.is_some() {
                    return Err(CliError::refused(
                        "the clip is a caption; captions are made by `captions`",
                    ));
                }
                let animation = p.materials.animation_of(segment).cloned().map(|mut a| {
                    a.id = new_id();
                    a
                });
                Ok((
                    text.id.clone(),
                    track.id.clone(),
                    segment.target_range,
                    segment.transform,
                    segment.keyframes.clone(),
                    segment.crop,
                    animation,
                ))
            })?;
        let at = self.at.map_or(range.end(), |t| t.resolve(session.fps()));
        let copy = text_commands::text_duplicate(&session.state, material_id)?;
        let mut placement = session
            .with(|p| text_edit::insert_command_on(p, &copy, at, range.duration, Some(&lane)))?;
        if let EditCommand::Composite { label, commands } = &mut placement.command {
            *label = "Duplicate title".into();
            for command in commands.iter_mut() {
                if let EditCommand::InsertSegment { segment, .. } = command {
                    segment.transform = transform;
                    segment.keyframes = keyframes.clone();
                    segment.crop = crop;
                }
            }
            if let Some(animation) = animation.filter(|a| !a.is_empty()) {
                commands.push(EditCommand::SetAnimation {
                    segment_id: placement.segment_id.clone(),
                    before: None,
                    after: Some(animation),
                    slot: None,
                });
            }
        }
        timeline_commands::timeline_apply(&session.state, placement.command)?;
        Ok(title_outcome(
            session,
            &placement.segment_id,
            format!("duplicated the title at {:.3} s", seconds(placement.start)),
        ))
    }
}
