//! Commands for captions: what the UI, a CLI and an MCP server call.
//!
//! Every mutation is an `EditCommand` applied through the history, so each of
//! these is one Ctrl+Z. New materials are pushed into the pool just before the
//! command that references them, exactly as `text_add` does and for its reason.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use super::edit::{self, CaptionClip, PlaceOptions};
use super::srt::{self, SubtitleFormat};
use super::style::CaptionStyle;
use super::{group, CaptionMode, Cue, Transcript};
use crate::modules::project::{Micros, Project, TextMaterial};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// The reply to an edit that made captions: the document, and what to select.
#[derive(Serialize)]
pub struct CaptionsAdded {
    #[serde(flatten)]
    pub edit: EditResponse,
    pub track_id: String,
    pub segment_ids: Vec<String>,
}

/// Every caption in the project, in time order.
pub fn captions_list(state: &Arc<AppState>) -> Result<Vec<CaptionClip>, String> {
    state.with_project(edit::clips)
}

/// The style of caption `segment_id`, or of the first caption, or the default
/// for this canvas when there are none — what the style panel shows.
pub fn captions_style_of(
    state: &Arc<AppState>,
    segment_id: Option<&str>,
) -> Result<CaptionStyle, String> {
    state.with_project(|project| style_of(project, segment_id))
}

fn style_of(project: &Project, segment_id: Option<&str>) -> CaptionStyle {
    let clip = match segment_id {
        Some(id) => edit::clips(project)
            .into_iter()
            .find(|c| c.segment_id == id),
        None => edit::clips(project).into_iter().next(),
    };
    clip.and_then(|clip| {
        let (_, segment) = project.segment(&clip.segment_id)?;
        let material = project.materials.text(&clip.material_id)?;
        Some(CaptionStyle::of(material, segment))
    })
    .unwrap_or_else(|| CaptionStyle::default_for(&project.canvas))
}

/// Put `cues` on the caption lane.
pub fn captions_add(
    state: &Arc<AppState>,
    cues: &[Cue],
    style: Option<CaptionStyle>,
    options: PlaceOptions,
) -> Result<CaptionsAdded, String> {
    let (track_id, segment_ids) = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let style = style.unwrap_or_else(|| style_of(project, None));
        let placed = edit::place(project, cues, &style, options)?;
        apply_with(state, project, placed.materials, placed.command)?;
        (placed.track_id, placed.segment_ids)
    };
    Ok(CaptionsAdded {
        edit: respond(state)?,
        track_id,
        segment_ids,
    })
}

/// Group a transcript into captions and put them on the lane.
pub fn captions_from_transcript(
    state: &Arc<AppState>,
    transcript: &Transcript,
    mode: CaptionMode,
    style: Option<CaptionStyle>,
    options: PlaceOptions,
) -> Result<CaptionsAdded, String> {
    let cues = group::group(&transcript.timed_words(), mode);
    if cues.is_empty() {
        return Err("no speech was found in the audio".to_string());
    }
    captions_add(state, &cues, style, options)
}

/// Read an `.srt` or `.vtt` file onto the caption lane.
pub fn captions_import(
    state: &Arc<AppState>,
    path: &Path,
    style: Option<CaptionStyle>,
    options: PlaceOptions,
) -> Result<CaptionsAdded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // Subtitle files from Windows tools are often Latin-1. Reading them lossily
    // turns the odd "é" into a replacement mark instead of refusing the file.
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
    let cues = srt::parse(&text)?;
    captions_add(state, &cues, style, options)
}

/// Write the project's captions as a sidecar subtitle file. Returns how many
/// were written.
pub fn captions_export(
    state: &Arc<AppState>,
    path: &Path,
    format: Option<SubtitleFormat>,
) -> Result<usize, String> {
    let cues = state.with_project(edit::cues)?;
    if cues.is_empty() {
        return Err("there are no captions to export".to_string());
    }
    let format = format
        .or_else(|| SubtitleFormat::from_path(path))
        .unwrap_or(SubtitleFormat::Srt);
    std::fs::write(path, srt::format(&cues, format))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(cues.len())
}

/// The sidecar path for an export: the video's name with the subtitle
/// extension, which is what players look for next to a file.
pub fn sidecar_path(video: &Path, format: SubtitleFormat) -> std::path::PathBuf {
    video.with_extension(format.extension())
}

pub fn captions_set_text(
    state: &Arc<AppState>,
    segment_id: &str,
    text: &str,
) -> Result<EditResponse, String> {
    run(state, |project| {
        edit::set_text(project, segment_id, text).map(|c| (Vec::new(), c))
    })
}

/// Restyle one caption, or all of them when `segment_id` is `None`.
pub fn captions_set_style(
    state: &Arc<AppState>,
    segment_id: Option<&str>,
    style: &CaptionStyle,
) -> Result<EditResponse, String> {
    run(state, |project| {
        edit::restyle(project, segment_id, style).map(|c| (Vec::new(), c))
    })
}

/// Split a caption at timeline instant `at`. Returns the two new segment ids.
pub fn captions_split(
    state: &Arc<AppState>,
    segment_id: &str,
    at: Micros,
) -> Result<(EditResponse, [String; 2]), String> {
    let mut ids = None;
    let response = run(state, |project| {
        let (materials, command, new_ids) = edit::split(project, segment_id, at)?;
        ids = Some(new_ids);
        Ok((materials, command))
    })?;
    Ok((response, ids.expect("set on success")))
}

/// Merge two neighbouring captions. Returns the merged segment's id.
pub fn captions_merge(
    state: &Arc<AppState>,
    first: &str,
    second: &str,
) -> Result<(EditResponse, String), String> {
    let mut id = None;
    let response = run(state, |project| {
        let (material, command, new_id) = edit::merge(project, first, second)?;
        id = Some(new_id);
        Ok((vec![material], command))
    })?;
    Ok((response, id.expect("set on success")))
}

/// Delete every caption.
pub fn captions_clear(state: &Arc<AppState>) -> Result<EditResponse, String> {
    run(state, |project| {
        edit::clear(project)
            .map(|c| (Vec::new(), c))
            .ok_or_else(|| "there are no captions to delete".to_string())
    })
}

/// Re-group the existing captions as word or sentence captions.
pub fn captions_regroup(state: &Arc<AppState>, mode: CaptionMode) -> Result<EditResponse, String> {
    run(state, |project| edit::regroup(project, mode))
}

/// Plan an edit against the document, then apply it.
fn run(
    state: &Arc<AppState>,
    plan: impl FnOnce(&Project) -> Result<(Vec<TextMaterial>, EditCommand), String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (materials, command) = plan(project)?;
        apply_with(state, project, materials, command)?;
    }
    respond(state)
}

/// Push `materials`, then apply `command`. A failed command takes the
/// materials out again, so a refused edit leaves the pool as it found it.
fn apply_with(
    state: &AppState,
    project: &mut Project,
    materials: Vec<TextMaterial>,
    command: EditCommand,
) -> Result<(), String> {
    let added: Vec<String> = materials.iter().map(|m| m.id.clone()).collect();
    project.materials.texts.extend(materials);
    let result = state.history.write().apply(project, command);
    if result.is_err() {
        project.materials.texts.retain(|m| !added.contains(&m.id));
    }
    result
}

/// The reply to an edit, with the working copy written on the way out — the
/// same as `timeline::commands::respond`, which is private to that module.
fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);

    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::CanvasConfig;

    fn state() -> Arc<AppState> {
        let state = AppState::new();
        *state.project.write() = Some(Project::new("t", CanvasConfig::default(), 30.0));
        state
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        // Inside the workspace's `target/`, which is ignored and per-checkout.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/captions");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn import_then_export_round_trips_through_the_document() {
        let state = state();
        let source = scratch("in.srt");
        let original = "1\n00:00:01,000 --> 00:00:02,000\nfirst line\n\n2\n00:00:02,500 --> 00:00:04,000\nsecond\nand third\n\n";
        std::fs::write(&source, original).unwrap();
        let added = captions_import(&state, &source, None, PlaceOptions::default()).unwrap();
        assert_eq!(added.segment_ids.len(), 2);
        assert!(added.edit.can_undo);

        let out = scratch("out.srt");
        assert_eq!(captions_export(&state, &out, None).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), original);

        let vtt = scratch("out.vtt");
        captions_export(&state, &vtt, None).unwrap();
        assert!(std::fs::read_to_string(&vtt).unwrap().starts_with("WEBVTT"));
    }

    #[test]
    fn every_caption_edit_is_one_undo_step() {
        let state = state();
        let cues = vec![
            Cue::new(0, 1_000_000, "one two"),
            Cue::new(1_000_000, 2_000_000, "three"),
        ];
        captions_add(&state, &cues, None, PlaceOptions::default()).unwrap();
        let first = captions_list(&state).unwrap()[0].segment_id.clone();

        captions_set_text(&state, &first, "uno dos").unwrap();
        assert_eq!(captions_list(&state).unwrap()[0].text, "uno dos");
        crate::modules::timeline::commands::timeline_undo(&state).unwrap();
        assert_eq!(captions_list(&state).unwrap()[0].text, "one two");

        captions_clear(&state).unwrap();
        assert!(captions_list(&state).unwrap().is_empty());
        crate::modules::timeline::commands::timeline_undo(&state).unwrap();
        assert_eq!(captions_list(&state).unwrap().len(), 2);
    }

    #[test]
    fn a_refused_edit_leaves_the_pool_alone() {
        let state = state();
        let before = state.with_project(|p| p.materials.texts.len()).unwrap();
        assert!(captions_add(&state, &[], None, PlaceOptions::default()).is_err());
        assert!(captions_export(&state, &scratch("none.srt"), None).is_err());
        assert_eq!(
            state.with_project(|p| p.materials.texts.len()).unwrap(),
            before
        );
    }
}
