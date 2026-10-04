//! Timelines and compound clips: several timelines per project, and clips
//! that hold a timeline of their own.
//!
//! Every operation is one of `sequence::commands`, the functions the app's
//! timeline tabs and context menu call. Opening a compound clip is an edit
//! like any other: the saved file remembers it, and every later command works
//! on the compound clip's lanes until `compound close`.

use chukcut_engine::modules::project::Project;
use chukcut_engine::modules::sequence::{self, commands as seq, SequenceInfo};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::seconds;

fn info_json(index: usize, s: &SequenceInfo) -> Value {
    json!({
        "index": index,
        "id": s.id,
        "name": s.name,
        "kind": s.kind,
        "active": s.active,
        "duration": seconds(s.duration),
        "lanes": s.tracks,
        "clips": s.clips,
        "uses": s.uses,
    })
}

/// The timeline `reference` names: its id, a unique id prefix of four or more
/// characters, its exact name, or its index among the timelines.
fn timeline(project: &Project, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    let tabs = sequence::timelines(project);
    if let Some(t) = tabs
        .iter()
        .find(|t| t.id == reference || t.name == reference)
    {
        return Ok(t.id.clone());
    }
    if let Ok(index) = reference.parse::<usize>() {
        return tabs.get(index).map(|t| t.id.clone()).ok_or_else(|| {
            CliError::usage(format!(
                "the project has {} timeline(s), so there is no timeline {index}",
                tabs.len()
            ))
        });
    }
    if reference.len() >= 4 {
        let matches: Vec<&SequenceInfo> = tabs
            .iter()
            .filter(|t| t.id.starts_with(reference))
            .collect();
        match matches.as_slice() {
            [one] => return Ok(one.id.clone()),
            [] => {}
            many => {
                return Err(CliError::usage(format!(
                    "{reference} matches {} timelines; give more of the id",
                    many.len()
                )))
            }
        }
    }
    Err(CliError::usage(format!(
        "there is no timeline {reference}; `chukcut-cli timeline list` lists them"
    )))
}

fn where_now(session: &Session) -> Value {
    let path = seq::sequence_breadcrumbs(&session.state).unwrap_or_default();
    json!({
        "active": session.with(|p| p.sequence.id.clone()),
        "path": path
            .into_iter()
            .map(|(id, name)| json!({"id": id, "name": name}))
            .collect::<Vec<_>>(),
    })
}

/// List the timelines, the compound clips' sequences, and which is open.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineListArgs {}

impl Operation for TimelineListArgs {
    const NAME: &'static str = "timeline_list";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let all = seq::sequence_list(&session.state)?;
        let timelines: Vec<Value> = seq::sequence_timelines(&session.state)?
            .iter()
            .enumerate()
            .map(|(i, s)| info_json(i, s))
            .collect();
        let compounds: Vec<Value> = all
            .iter()
            .filter(|s| s.kind == sequence::SequenceKind::Compound)
            .enumerate()
            .map(|(i, s)| info_json(i, s))
            .collect();
        let mut data = where_now(session);
        data["timelines"] = json!(timelines);
        data["compounds"] = json!(compounds);
        Ok(Outcome::read(
            format!(
                "{} timeline(s), {} compound clip sequence(s)",
                timelines.len(),
                compounds.len()
            ),
            data,
        ))
    }
}

/// Add an empty timeline (a video and an audio lane) and open it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineNewArgs {
    /// Its name; "Timeline NN" when left out.
    #[arg(long)]
    pub name: Option<String>,
}

impl Operation for TimelineNewArgs {
    const NAME: &'static str = "timeline_new";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (_, id) = seq::sequence_timeline_new(&session.state, self.name)?;
        let mut data = where_now(session);
        data["id"] = json!(id);
        Ok(Outcome::changed("added a timeline and opened it", data))
    }
}

/// Rename a timeline.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineRenameArgs {
    /// The timeline: id, id prefix, name or index (see `timeline list`).
    pub timeline: String,
    /// The new name.
    pub name: String,
}

impl Operation for TimelineRenameArgs {
    const NAME: &'static str = "timeline_rename";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| timeline(p, &self.timeline))?;
        seq::sequence_rename(&session.state, id.clone(), self.name.clone())?;
        Ok(Outcome::changed(
            format!("renamed the timeline to \"{}\"", self.name.trim()),
            json!({"id": id, "name": self.name.trim()}),
        ))
    }
}

/// Delete a timeline. The last one cannot be deleted; deleting the open one
/// opens its neighbour.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineDeleteArgs {
    /// The timeline: id, id prefix, name or index.
    pub timeline: String,
}

impl Operation for TimelineDeleteArgs {
    const NAME: &'static str = "timeline_delete";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| timeline(p, &self.timeline))?;
        seq::sequence_timeline_delete(&session.state, id.clone())?;
        let mut data = where_now(session);
        data["deleted"] = json!(id);
        Ok(Outcome::changed("deleted the timeline", data))
    }
}

/// Copy a timeline into a new one after the last, with its own clips.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineDuplicateArgs {
    /// The timeline: id, id prefix, name or index.
    pub timeline: String,
}

impl Operation for TimelineDuplicateArgs {
    const NAME: &'static str = "timeline_duplicate";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| timeline(p, &self.timeline))?;
        let (_, copy) = seq::sequence_timeline_duplicate(&session.state, id)?;
        Ok(Outcome::changed(
            "duplicated the timeline",
            json!({"id": copy}),
        ))
    }
}

/// Open another timeline. Every later command works on its lanes.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TimelineSwitchArgs {
    /// The timeline: id, id prefix, name or index.
    pub timeline: String,
}

impl Operation for TimelineSwitchArgs {
    const NAME: &'static str = "timeline_switch";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| timeline(p, &self.timeline))?;
        seq::sequence_timeline_switch(&session.state, id)?;
        Ok(Outcome::changed("opened the timeline", where_now(session)))
    }
}

/// Move clips into a new compound clip that takes their place. Linked
/// partners come along.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CompoundCreateArgs {
    /// The clips: ids, id prefixes or `lane:index`.
    #[arg(required = true)]
    pub clips: Vec<String>,
    /// The compound clip's name; "Compound clip N" when left out.
    #[arg(long)]
    pub name: Option<String>,
}

impl Operation for CompoundCreateArgs {
    const NAME: &'static str = "compound_create";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.clips.is_empty() {
            return Err(CliError::usage("compound create needs at least one clip"));
        }
        let ids: Vec<String> = session.with(|p| {
            self.clips
                .iter()
                .map(|r| select::clip(p, r))
                .collect::<CliResult<_>>()
        })?;
        let made = seq::sequence_compound_create(&session.state, ids, self.name)?;
        Ok(Outcome::changed(
            "made a compound clip",
            json!({"clip": made.segment_id, "sequence": made.sequence_id}),
        ))
    }
}

/// Open a compound clip: every later command works on its lanes until
/// `compound close`.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CompoundOpenArgs {
    /// The compound clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for CompoundOpenArgs {
    const NAME: &'static str = "compound_open";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        seq::sequence_compound_open(&session.state, id)?;
        Ok(Outcome::changed(
            "opened the compound clip",
            where_now(session),
        ))
    }
}

/// Close the open compound clip, or all of them.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CompoundCloseArgs {
    /// Go all the way out to the timeline.
    #[arg(long)]
    #[serde(default)]
    pub all: bool,
}

impl Operation for CompoundCloseArgs {
    const NAME: &'static str = "compound_close";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.all {
            seq::sequence_compound_close_to(&session.state, 0)?;
        } else {
            seq::sequence_compound_close(&session.state)?;
        }
        Ok(Outcome::changed(
            "closed the compound clip",
            where_now(session),
        ))
    }
}

/// Put a compound clip's contents back on the timeline in its place.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CompoundFlattenArgs {
    /// The compound clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for CompoundFlattenArgs {
    const NAME: &'static str = "compound_flatten";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        seq::sequence_compound_flatten(&session.state, id.clone())?;
        Ok(Outcome::changed(
            "put the compound clip's clips back",
            json!({"flattened": id}),
        ))
    }
}
