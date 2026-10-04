//! The colour tools' shell-facing API: auto adjust, colour match and grade
//! presets.
//!
//! Auto adjust and colour match decode a few frames, so they block for a
//! fraction of a second: the app calls them from a background task, the CLI
//! and MCP directly. Neither holds the project lock across the decode
//! (`sources::plan` / `sources::read`), and both write their answer onto the
//! clip's grade *as it is when the answer is ready*: only the controls the
//! tool owns are replaced, so a slider the user moved meanwhile stays.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::auto::{self, AutoControls, Measure};
use super::matching::{self, MatchControls};
use super::pixels::LabStats;
use super::presets::{self, PresetEntry};
use super::sources;
use crate::modules::inspector::commands::commit_grade;
use crate::modules::inspector::edit::{self, GradeEdit};
use crate::modules::project::document::Micros;
use crate::modules::timeline::commands::EditResponse;
use crate::modules::workspace::paths;
use crate::state::AppState;

fn full() -> f32 {
    1.0
}

/// What auto adjust is asked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoAdjust {
    pub segment_id: String,
    /// `0..1`: how far towards the full correction. CapCut's "intensity".
    #[serde(default = "full")]
    pub amount: f32,
}

/// What auto adjust did.
#[derive(Debug, Clone, Serialize)]
pub struct AutoAdjusted {
    pub controls: AutoControls,
    pub before: Measure,
    pub after: Measure,
}

/// Balance a clip's exposure, white balance and contrast from its own
/// pixels, as one undo step. See `grading::auto`.
pub fn grading_auto_adjust(
    state: &Arc<AppState>,
    request: AutoAdjust,
) -> Result<AutoAdjusted, String> {
    let plan = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        sources::plan(project, &request.segment_id, None)?
    };
    let picture = sources::read(&plan)?;
    let result = auto::auto_adjust(
        &picture.samples,
        &picture.grade,
        picture.lut.as_ref(),
        request.amount,
    );
    let controls = result.controls;
    commit_grade(state, |project| {
        let now = GradeEdit::of_segment(project, &request.segment_id)?;
        edit::set_grade_command(
            project,
            &request.segment_id,
            Some(controls.applied_to(&now)),
        )
    })?;
    Ok(AutoAdjusted {
        controls,
        before: result.before,
        after: result.after,
    })
}

/// What colour match is asked: grade `segment_id` like `reference_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColourMatch {
    pub segment_id: String,
    /// The clip to match: any video or photo clip on the timeline.
    pub reference_id: String,
    /// A timeline instant inside the reference: match the frame shown then
    /// instead of the reference clip as a whole.
    #[serde(default)]
    pub at: Option<Micros>,
    #[serde(default = "full")]
    pub amount: f32,
}

/// What colour match did and how close it got, in L*a*b* units.
#[derive(Debug, Clone, Serialize)]
pub struct ColourMatched {
    pub controls: MatchControls,
    /// Whether the red, green and blue curves carry part of the match.
    pub curves: bool,
    pub reference: LabStats,
    pub before: LabStats,
    pub after: LabStats,
    pub distance_before: f32,
    pub distance_after: f32,
}

/// Grade a clip so its colours match another clip's (or one frame of it),
/// as one undo step. See `grading::matching`.
pub fn grading_match(state: &Arc<AppState>, request: ColourMatch) -> Result<ColourMatched, String> {
    if request.segment_id == request.reference_id && request.at.is_none() {
        return Err("pick another clip to match to".into());
    }
    let (target, reference) = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        (
            sources::plan(project, &request.segment_id, None)?,
            sources::plan(project, &request.reference_id, request.at).map_err(|e| {
                if e.contains("no longer") {
                    "the reference clip is no longer on the timeline".to_string()
                } else {
                    format!("the reference: {e}")
                }
            })?,
        )
    };
    let target = sources::read(&target)?;
    let reference = sources::read(&reference)?;
    // The reference as the viewer sees it: its own grade applied.
    let seen = reference
        .samples
        .graded(&reference.grade, reference.lut.as_ref());
    let result = matching::colour_match(
        &target.samples,
        &target.grade,
        target.lut.as_ref(),
        &seen,
        request.amount,
    );
    let (controls, curves) = (result.controls, result.edit.grade.curves.clone());
    let matched_curves = result.curves;
    commit_grade(state, |project| {
        let now = GradeEdit::of_segment(project, &request.segment_id)?;
        let mut edit = controls.applied_to(&now);
        if matched_curves {
            edit.grade.curves.red = curves.red;
            edit.grade.curves.green = curves.green;
            edit.grade.curves.blue = curves.blue;
        }
        edit::set_grade_command(project, &request.segment_id, Some(edit))
    })?;
    Ok(ColourMatched {
        controls,
        curves: matched_curves,
        reference: result.reference,
        before: result.before,
        after: result.after,
        distance_before: result.distance_before,
        distance_after: result.distance_after,
    })
}

// --- presets -------------------------------------------------------------------

/// The saved grade presets, by name.
pub fn grading_presets() -> Vec<PresetEntry> {
    presets::list(&paths::grade_presets_dir())
}

/// The name "Save as preset" suggests: the next free "Preset N".
pub fn grading_preset_name() -> String {
    presets::next_name(&paths::grade_presets_dir())
}

/// Save a clip's whole grade as a preset called `name`. A preset of that
/// name is replaced only with `replace`.
pub fn grading_save_preset(
    state: &Arc<AppState>,
    segment_id: String,
    name: String,
    replace: bool,
) -> Result<PresetEntry, String> {
    let grade = {
        let guard = state.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        GradeEdit::of_segment(project, &segment_id)?
    };
    presets::save(&paths::grade_presets_dir(), &name, &grade, replace)
}

/// Give a clip the preset's grade, replacing its own, as one undo step.
pub fn grading_apply_preset(
    state: &Arc<AppState>,
    segment_id: String,
    name: String,
) -> Result<EditResponse, String> {
    let preset = presets::load(&paths::grade_presets_dir(), &name)?;
    commit_grade(state, |project| {
        edit::set_grade_command(project, &segment_id, Some(preset.grade))
    })
}

pub fn grading_delete_preset(name: String) -> Result<(), String> {
    presets::delete(&paths::grade_presets_dir(), &name)
}

/// A preset's tile for the Filters tab: the sample picture with the preset's
/// grade, drawn by the compositor and cached.
pub fn grading_preset_tile(name: String, size: (u32, u32)) -> Result<PathBuf, String> {
    let preset = presets::load(&paths::grade_presets_dir(), &name)?;
    crate::modules::fx::tiles::grade_tile(&preset.grade, size)
}
