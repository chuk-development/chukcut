//! Markers: named instants on the ruler.
//!
//! The engine has no marker commands of its own; a marker edit is one
//! `EditCommand` (`AddMarker`, `SetMarker`, `RemoveMarker`) applied through
//! `timeline_apply`, which is what the app's ruler does too.

use chukcut_engine::modules::project::{Marker, MarkerColor, Project};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::Session;
use crate::values::{seconds, Time};

const COLORS: &[&str] = &["blue", "green", "yellow", "orange", "red", "purple"];

fn color(name: &str) -> CliResult<MarkerColor> {
    enum_named("marker colour", name, COLORS)
}

fn marker_json(index: usize, marker: &Marker) -> Value {
    json!({
        "index": index,
        "id": marker.id,
        "time": seconds(marker.time),
        "time_us": marker.time,
        "label": marker.label,
        "color": marker.color,
    })
}

/// The markers in time order, as `marker list` numbers them.
fn in_order(project: &Project) -> Vec<&Marker> {
    let mut markers: Vec<&Marker> = project.markers.iter().collect();
    markers.sort_by_key(|m| m.time);
    markers
}

/// The marker `reference` names: its id, a unique id prefix of four or more
/// characters, or its index in time order.
fn find(project: &Project, reference: &str) -> CliResult<Marker> {
    let reference = reference.trim();
    let markers = in_order(project);
    if let Some(m) = markers.iter().find(|m| m.id == reference) {
        return Ok((*m).clone());
    }
    if let Ok(index) = reference.parse::<usize>() {
        return markers.get(index).map(|m| (*m).clone()).ok_or_else(|| {
            CliError::usage(format!(
                "the project has {} marker(s), so there is no marker {index}",
                markers.len()
            ))
        });
    }
    if reference.len() >= 4 {
        let matches: Vec<&&Marker> = markers
            .iter()
            .filter(|m| m.id.starts_with(reference))
            .collect();
        match matches.as_slice() {
            [one] => return Ok((**one).clone()),
            [] => {}
            many => {
                return Err(CliError::usage(format!(
                    "{reference} matches {} markers; give more of the id",
                    many.len()
                )))
            }
        }
    }
    Err(CliError::usage(format!(
        "there is no marker {reference}; `chukcut-cli marker list` lists them"
    )))
}

fn found(session: &Session, id: &str) -> Value {
    session.with(|p| {
        in_order(p)
            .into_iter()
            .enumerate()
            .find(|(_, m)| m.id == id)
            .map(|(i, m)| marker_json(i, m))
            .unwrap_or(Value::Null)
    })
}

/// Put a marker on the ruler at a time, with an optional label and colour.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MarkerAddArgs {
    /// The timeline time of the marker.
    #[arg(long)]
    pub at: Time,
    /// A short label; empty is an unnamed marker.
    #[arg(long)]
    pub label: Option<String>,
    /// blue (the default), green, yellow, orange, red or purple.
    #[arg(long)]
    pub color: Option<String>,
}

impl Operation for MarkerAddArgs {
    const NAME: &'static str = "marker_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let mut marker = Marker::new(self.at.resolve(session.fps()));
        if let Some(label) = self.label {
            marker.label = label;
        }
        if let Some(c) = &self.color {
            marker.color = color(c)?;
        }
        let id = marker.id.clone();
        let time = marker.time;
        timeline_commands::timeline_apply(&session.state, EditCommand::AddMarker { marker })?;
        Ok(Outcome::changed(
            format!("added a marker at {:.3} s", seconds(time)),
            json!({"marker": found(session, &id)}),
        ))
    }
}

/// Move, rename or recolour a marker, as one undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MarkerSetArgs {
    /// The marker: its id, an id prefix, or its index in time order.
    pub marker: String,
    /// Move it to this timeline time.
    #[arg(long)]
    pub at: Option<Time>,
    /// A new label; an empty string removes the label.
    #[arg(long)]
    pub label: Option<String>,
    /// blue, green, yellow, orange, red or purple.
    #[arg(long)]
    pub color: Option<String>,
}

impl Operation for MarkerSetArgs {
    const NAME: &'static str = "marker_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let before = session.with(|p| find(p, &self.marker))?;
        let mut after = before.clone();
        if let Some(at) = self.at {
            after.time = at.resolve(session.fps());
        }
        if let Some(label) = self.label {
            after.label = label;
        }
        if let Some(c) = &self.color {
            after.color = color(c)?;
        }
        let id = before.id.clone();
        if after == before {
            return Ok(Outcome::read(
                "the marker is already that",
                json!({"marker": found(session, &id)}),
            ));
        }
        timeline_commands::timeline_apply(
            &session.state,
            EditCommand::SetMarker { before, after },
        )?;
        Ok(Outcome::changed(
            "marker changed",
            json!({"marker": found(session, &id)}),
        ))
    }
}

/// Remove markers from the ruler, as one undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MarkerRemoveArgs {
    /// The markers: ids, id prefixes, or indexes in time order.
    #[arg(required = true)]
    pub markers: Vec<String>,
}

impl Operation for MarkerRemoveArgs {
    const NAME: &'static str = "marker_remove";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.markers.is_empty() {
            return Err(CliError::usage("marker remove needs at least one marker"));
        }
        // Resolve every reference first: an index means the marker that is
        // there now, not after the first removal.
        let mut markers: Vec<Marker> = session.with(|p| {
            self.markers
                .iter()
                .map(|r| find(p, r))
                .collect::<CliResult<_>>()
        })?;
        markers.dedup_by(|a, b| a.id == b.id);
        let ids: Vec<String> = markers.iter().map(|m| m.id.clone()).collect();
        let commands: Vec<EditCommand> = markers
            .into_iter()
            .map(|marker| EditCommand::RemoveMarker { marker })
            .collect();
        timeline_commands::timeline_apply_many(&session.state, commands, "Delete marker".into())?;
        Ok(Outcome::changed(
            format!("removed {} marker(s)", ids.len()),
            json!({"removed": ids}),
        ))
    }
}

/// List the markers in time order.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MarkerListArgs {}

impl Operation for MarkerListArgs {
    const NAME: &'static str = "marker_list";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (list, lines): (Vec<Value>, Vec<String>) = session.with(|p| {
            in_order(p)
                .into_iter()
                .enumerate()
                .map(|(i, m)| {
                    let color = serde_json::to_value(m.color)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    (
                        marker_json(i, m),
                        format!(
                            "  {i:<3} {}  {:>9.3} s  {color:<7} {}",
                            &m.id[..m.id.len().min(8)],
                            seconds(m.time),
                            m.label
                        ),
                    )
                })
                .unzip()
        });
        let mut message = format!("{} marker(s)", list.len());
        for line in lines {
            message.push('\n');
            message.push_str(&line);
        }
        Ok(Outcome::read(message, json!(list)))
    }
}
