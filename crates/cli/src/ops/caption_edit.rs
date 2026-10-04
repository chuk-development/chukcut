//! Editing captions one by one: add, retype, split, merge, clear, regroup.
//!
//! The captions module's own commands, so each edit is the same undo step
//! the caption panel makes. Transcribing, importing and styling are in
//! `audio.rs`.

use chukcut_engine::modules::captions::commands as caption_commands;
use chukcut_engine::modules::captions::edit::PlaceOptions;
use chukcut_engine::modules::captions::{with_estimated_words, CaptionMode, Cue};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{seconds, Time};
use crate::On;

/// The caption a reference names, refused when the clip is not a caption.
fn caption(session: &Session, reference: &str) -> CliResult<String> {
    let id = session.with(|p| select::clip(p, reference))?;
    let is_caption = caption_commands::captions_list(&session.state)?
        .iter()
        .any(|c| c.segment_id == id);
    if !is_caption {
        return Err(CliError::usage(format!(
            "{reference} is not a caption; `chukcut-cli captions list` lists them"
        )));
    }
    Ok(id)
}

fn listed(session: &Session) -> CliResult<serde_json::Value> {
    let clips = caption_commands::captions_list(&session.state)?;
    Ok(json!(clips
        .iter()
        .map(|c| json!({
            "id": c.segment_id, "start": seconds(c.start), "end": seconds(c.end),
            "text": c.text,
        }))
        .collect::<Vec<_>>()))
}

/// Put one caption on the caption lane, from `start` to `end`, beside the
/// captions already there, in their style. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsAddArgs {
    /// The words of the caption.
    pub text: String,
    /// When it appears, on the timeline.
    #[arg(long)]
    pub start: Time,
    /// When it goes.
    #[arg(long)]
    pub end: Time,
}

impl Operation for CaptionsAddArgs {
    const NAME: &'static str = "captions_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let (start, end) = (self.start.resolve(fps), self.end.resolve(fps));
        if end <= start {
            return Err(CliError::usage("a caption must end after it starts"));
        }
        if self.text.trim().is_empty() {
            return Err(CliError::usage("a caption needs words"));
        }
        // Words spread over the caption, as for an imported .srt, so the
        // karaoke highlight has something to light.
        let cues = with_estimated_words(vec![Cue::new(start, end, self.text.trim())]);
        let added = caption_commands::captions_add(
            &session.state,
            &cues,
            None,
            PlaceOptions {
                replace: false,
                auto_emoji: false,
            },
        )?;
        Ok(Outcome::changed(
            format!("added a caption at {:.3} s", seconds(start)),
            json!({"track_id": added.track_id, "clips": added.segment_ids}),
        ))
    }
}

/// Change the words of one caption, keeping its time and style.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsTextArgs {
    /// The caption: id, id prefix or `lane:index`.
    pub clip: String,
    /// The new words.
    pub text: String,
}

impl Operation for CaptionsTextArgs {
    const NAME: &'static str = "captions_text";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = caption(session, &self.clip)?;
        caption_commands::captions_set_text(&session.state, &id, &self.text)?;
        Ok(Outcome::changed(
            "changed the caption's words",
            json!({"clip": id, "text": self.text}),
        ))
    }
}

/// Cut one caption in two at a timeline time; its words are shared out by
/// their timing.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsSplitArgs {
    /// The caption: id, id prefix or `lane:index`.
    pub clip: String,
    /// Where to cut, on the timeline.
    #[arg(long)]
    pub at: Time,
}

impl Operation for CaptionsSplitArgs {
    const NAME: &'static str = "captions_split";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = caption(session, &self.clip)?;
        let at = self.at.resolve(session.fps());
        let (_, ids) = caption_commands::captions_split(&session.state, &id, at)?;
        Ok(Outcome::changed(
            format!("split the caption at {:.3} s", seconds(at)),
            json!({"clips": ids}),
        ))
    }
}

/// Join two neighbouring captions into one.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsMergeArgs {
    /// The first caption: id, id prefix or `lane:index`.
    pub first: String,
    /// The caption right after it.
    pub second: String,
}

impl Operation for CaptionsMergeArgs {
    const NAME: &'static str = "captions_merge";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let first = caption(session, &self.first)?;
        let second = caption(session, &self.second)?;
        let (_, id) = caption_commands::captions_merge(&session.state, &first, &second)?;
        Ok(Outcome::changed("merged two captions", json!({"clip": id})))
    }
}

/// Delete every caption.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsClearArgs {}

impl Operation for CaptionsClearArgs {
    const NAME: &'static str = "captions_clear";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        caption_commands::captions_clear(&session.state)?;
        Ok(Outcome::changed("deleted every caption", json!({})))
    }
}

/// Group the captions again from their words: word captions with at most
/// `words` on screen, or sentence captions without it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsRegroupArgs {
    /// Word captions with at most this many words on screen. Without it,
    /// sentence captions.
    #[arg(long)]
    pub words: Option<usize>,
}

impl Operation for CaptionsRegroupArgs {
    const NAME: &'static str = "captions_regroup";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let mode = match self.words {
            Some(n) => CaptionMode::Words {
                max_words: n.max(1),
            },
            None => CaptionMode::default(),
        };
        caption_commands::captions_regroup(&session.state, mode)?;
        let list = listed(session)?;
        Ok(Outcome::changed(
            format!(
                "regrouped into {} caption(s)",
                list.as_array().map_or(0, Vec::len)
            ),
            list,
        ))
    }
}

/// The caption edits, as `captions` subcommands.
#[derive(Subcommand)]
pub enum CaptionsEditCommand {
    /// Put one caption on the caption lane.
    Add(On<CaptionsAddArgs>),
    /// Change one caption's words.
    Text(On<CaptionsTextArgs>),
    /// Cut a caption in two at a time.
    Split(On<CaptionsSplitArgs>),
    /// Join two neighbouring captions.
    Merge(On<CaptionsMergeArgs>),
    /// Delete every caption.
    Clear(On<CaptionsClearArgs>),
    /// Group the captions again as word or sentence captions.
    Regroup(On<CaptionsRegroupArgs>),
}

impl CaptionsEditCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Add(o) => crate::on(o, dry, ctx),
            Self::Text(o) => crate::on(o, dry, ctx),
            Self::Split(o) => crate::on(o, dry, ctx),
            Self::Merge(o) => crate::on(o, dry, ctx),
            Self::Clear(o) => crate::on(o, dry, ctx),
            Self::Regroup(o) => crate::on(o, dry, ctx),
        }
    }
}
