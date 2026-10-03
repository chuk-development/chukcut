//! Several pictures on one canvas: picture in picture and split screens.

use chukcut_engine::modules::fx::commands as fx_commands;
use chukcut_engine::modules::fx::edit::{Corner, SplitLayout};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;

pub const CORNERS: &[&str] = &["top_left", "top_right", "bottom_left", "bottom_right"];
pub const LAYOUTS: &[&str] = &[
    "two_rows",
    "two_columns",
    "three_rows",
    "three_columns",
    "grid",
];

/// What `catalog layouts` lists.
pub fn layouts_catalog() -> Value {
    let cells = |layout: &str| match layout {
        "two_rows" | "two_columns" => 2,
        "three_rows" | "three_columns" => 3,
        _ => 4,
    };
    let mut out: Vec<Value> = LAYOUTS
        .iter()
        .map(|l| json!({"id": l, "kind": "split", "clips": cells(l), "text": format!("split screen, {} clips", cells(l))}))
        .collect();
    out.extend(
        CORNERS
            .iter()
            .map(|c| json!({"id": format!("pip:{c}"), "kind": "pip", "corner": c, "clips": 1, "text": "picture in picture"})),
    );
    json!(out)
}

/// Make a clip a picture in picture: a third of the canvas wide, in a
/// corner with a margin, with a frame effect (rounded corners, a thin border,
/// a soft shadow). The clip should be on a lane above the main picture. One
/// undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct LayoutPipArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// top_left, top_right, bottom_left or bottom_right (the default).
    #[arg(long, default_value = "bottom_right")]
    #[serde(default = "bottom_right")]
    pub corner: String,
}

fn bottom_right() -> String {
    "bottom_right".into()
}

impl Operation for LayoutPipArgs {
    const NAME: &'static str = "layout_pip";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let corner: Corner = enum_named("corner", &self.corner, CORNERS)?;
        fx_commands::fx_layout_pip(&session.state, id.clone(), corner)?;
        let clip = session.with(|p| summary::clip_by_id(p, &id));
        Ok(Outcome::changed(
            format!("picture in picture, {}", self.corner.to_ascii_lowercase()),
            json!({"clip": clip}),
        ))
    }
}

/// Arrange clips into a split screen. Each clip fills one cell, in the order
/// given: two_rows and two_columns take 2 clips, three_rows and
/// three_columns 3, grid 4. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct LayoutSplitArgs {
    /// The clips, one per cell: ids, id prefixes or `lane:index`.
    #[arg(required = true)]
    pub clips: Vec<String>,
    /// two_rows (the default), two_columns, three_rows, three_columns or grid.
    #[arg(long, default_value = "two_rows")]
    #[serde(default = "two_rows")]
    pub layout: String,
}

fn two_rows() -> String {
    "two_rows".into()
}

impl Operation for LayoutSplitArgs {
    const NAME: &'static str = "layout_split";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.clips.is_empty() {
            return Err(CliError::usage("a split screen needs clips"));
        }
        let layout: SplitLayout = enum_named("split layout", &self.layout, LAYOUTS)?;
        let ids: Vec<String> = session.with(|p| {
            self.clips
                .iter()
                .map(|r| select::clip(p, r))
                .collect::<CliResult<_>>()
        })?;
        fx_commands::fx_layout_split(&session.state, ids.clone(), layout)?;
        let clips: Vec<Value> =
            session.with(|p| ids.iter().map(|id| summary::clip_by_id(p, id)).collect());
        Ok(Outcome::changed(
            format!(
                "split screen {} with {} clip(s)",
                self.layout.to_ascii_lowercase(),
                ids.len()
            ),
            json!({"clips": clips}),
        ))
    }
}
