//! Commands for masks, the chroma key and blend modes: what the UI, the CLI
//! and the MCP server call.
//!
//! Each mutating command builds its edit in `edit.rs` against the document as
//! it is, puts the minted material into the pool first and pushes the edit
//! through the one history — the contract `fx_add` follows — then answers with
//! the whole updated project.

use std::sync::{Arc, OnceLock};

use super::edit::{self, Minted, ResetPart};
use crate::modules::project::compositing::{
    BlendMode, ChromaKey, CompositingMaterial, Mask, MaskOp, MaskShape,
};
use crate::modules::project::document::{AnimatableProperty, Micros, Project};
use crate::modules::render::{Compositor, SourceProvider};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// How close to the playhead an existing keyframe counts as "there", for the
/// keyframe toggle: half a frame at 30 fps.
pub const KEYFRAME_TOLERANCE: Micros = 16_666;

fn commit(
    state: &Arc<AppState>,
    build: impl FnOnce(&Project) -> Result<Minted, String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) = build(project)?;
        match material {
            Some(material) => {
                let id = material.id.clone();
                project.materials.compositing.push(material);
                if let Err(error) = state.history.write().apply(project, command) {
                    project.materials.compositing.retain(|m| m.id != id);
                    return Err(error);
                }
            }
            None => state.history.write().apply(project, command)?,
        }
    }
    respond(state)
}

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

/// The source time of `segment_id` at timeline time `at`.
fn source_at(project: &Project, segment_id: &str, at: Option<Micros>) -> Option<Micros> {
    let (_, segment) = project.segment(segment_id)?;
    at.map(|t| edit::source_time_in(project, segment, t))
}

/// A clip's masks, key and blend mode; empty when it has none.
pub fn compositing_get(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<CompositingMaterial, String> {
    state.with_project(|p| edit::current(p, &segment_id))?
}

/// Add a mask of `shape` at its defaults. Answers the new mask's id with the
/// project.
pub fn compositing_add_mask(
    state: &Arc<AppState>,
    segment_id: String,
    shape: MaskShape,
) -> Result<(EditResponse, String), String> {
    let mut mask_id = String::new();
    let response = commit(state, |p| {
        let (minted, id) = edit::add_mask_command(p, &segment_id, shape)?;
        mask_id = id;
        Ok(minted)
    })?;
    Ok((response, mask_id))
}

pub fn compositing_remove_mask(
    state: &Arc<AppState>,
    segment_id: String,
    mask_id: String,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::remove_mask_command(p, &segment_id, &mask_id)
    })
}

/// Replace one mask wholesale (a drag on the player, committed).
pub fn compositing_set_mask(
    state: &Arc<AppState>,
    segment_id: String,
    mask: Mask,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::replace_mask_command(p, &segment_id, mask, "Move mask")
    })
}

/// Set one parameter (`x`, `y`, `width`, `height`, `rotation`, `feather`,
/// `roundness`). `at` is the playhead on the timeline; an animated parameter
/// is keyed there.
pub fn compositing_set_mask_value(
    state: &Arc<AppState>,
    segment_id: String,
    mask_id: String,
    param: String,
    value: f32,
    at: Option<Micros>,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        let source = source_at(p, &segment_id, at);
        edit::set_mask_value_command(p, &segment_id, &mask_id, &param, value, source)
    })
}

/// Add or remove the keyframe of `param` at the playhead `at`.
pub fn compositing_toggle_mask_keyframe(
    state: &Arc<AppState>,
    segment_id: String,
    mask_id: String,
    param: String,
    at: Micros,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        let source = source_at(p, &segment_id, Some(at)).ok_or("unknown clip")?;
        edit::toggle_mask_keyframe_command(
            p,
            &segment_id,
            &mask_id,
            &param,
            source,
            KEYFRAME_TOLERANCE,
        )
    })
}

/// Shape, combine operation, invert, on/off; `None` keeps each.
pub fn compositing_set_mask_options(
    state: &Arc<AppState>,
    segment_id: String,
    mask_id: String,
    shape: Option<MaskShape>,
    op: Option<MaskOp>,
    invert: Option<bool>,
    enabled: Option<bool>,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::set_mask_options_command(p, &segment_id, &mask_id, shape, op, invert, enabled)
    })
}

pub fn compositing_move_mask(
    state: &Arc<AppState>,
    segment_id: String,
    mask_id: String,
    index: usize,
) -> Result<EditResponse, String> {
    commit(state, |p| {
        edit::move_mask_command(p, &segment_id, &mask_id, index)
    })
}

/// Set the chroma key, or take it off with `None`.
pub fn compositing_set_key(
    state: &Arc<AppState>,
    segment_id: String,
    key: Option<ChromaKey>,
) -> Result<EditResponse, String> {
    commit(state, |p| edit::set_key_command(p, &segment_id, key))
}

pub fn compositing_set_blend(
    state: &Arc<AppState>,
    segment_id: String,
    blend: BlendMode,
) -> Result<EditResponse, String> {
    commit(state, |p| edit::set_blend_command(p, &segment_id, blend))
}

pub fn compositing_reset(
    state: &Arc<AppState>,
    segment_id: String,
    part: ResetPart,
) -> Result<EditResponse, String> {
    commit(state, |p| edit::reset_command(p, &segment_id, part))
}

// ---------------------------------------------------------------------------
// The eyedropper
// ---------------------------------------------------------------------------

/// The longest side the eyedropper renders at. Enough to hit a strand of
/// green between two fingers, small enough to stay well under a frame time.
const PICK_SIZE: u32 = 960;

static PICKER: OnceLock<Option<Arc<Compositor>>> = OnceLock::new();

fn picker() -> Result<Arc<Compositor>, String> {
    PICKER
        .get_or_init(|| {
            let ctx = crate::modules::gpu::render_context()?;
            Some(Arc::new(Compositor::new(ctx)))
        })
        .clone()
        .ok_or_else(|| "this machine has no GPU that can render frames".to_string())
}

/// The document the eyedropper renders: only `segment_id`, opaque, with no
/// key, mask, blend, grade or effect — the footage as shot, where the clip
/// is on the canvas.
pub fn key_probe_project(project: &Project, segment_id: &str) -> Result<Project, String> {
    let mut probe = project.clone();
    let materials = probe.materials.clone();
    let mut found = false;
    for track in &mut probe.tracks {
        track.hidden = false;
        track.segments.retain(|s| s.id == segment_id);
        for segment in &mut track.segments {
            found = true;
            segment.extras.retain(|id| {
                materials.compositing(id).is_none()
                    && materials.color_adjust(id).is_none()
                    && materials.effect(id).is_none()
                    && materials.transition(id).is_none()
            });
            segment.transform.opacity = 1.0;
            segment
                .keyframes
                .retain(|k| k.property != AnimatableProperty::Opacity);
        }
    }
    if !found {
        return Err(format!("unknown segment {segment_id}"));
    }
    Ok(probe)
}

/// The colour of `segment_id`'s footage at timeline time `time`, under the
/// canvas point `point` (`0..1` from the top left), as gamma-encoded sRGB:
/// what the eyedropper on the player sets the key to. A 5×5 average, so one
/// noisy pixel does not decide the key.
pub async fn compositing_pick_key_color(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
    point: [f32; 2],
) -> Result<[f32; 3], String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let probe = key_probe_project(&project, &segment_id)?;
    crate::shell::spawn_blocking(move || {
        let compositor = picker()?;
        let sources = crate::modules::media::MediaSourceProvider::from_project(&probe);
        pick_from(&compositor, &probe, time, point, &sources)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The pick itself, on any source provider (the tests use a synthetic one).
pub fn pick_from(
    compositor: &Compositor,
    probe: &Project,
    time: Micros,
    point: [f32; 2],
    sources: &dyn SourceProvider,
) -> Result<[f32; 3], String> {
    let (cw, ch) = (probe.canvas.width.max(2), probe.canvas.height.max(2));
    let scale = (PICK_SIZE as f32 / cw.max(ch) as f32).min(1.0);
    let size = (
        ((cw as f32 * scale) as u32).max(2) & !1,
        ((ch as f32 * scale) as u32).max(2) & !1,
    );
    let frame = compositor
        .render(
            probe,
            time + crate::modules::project::SAMPLE_SLACK,
            size,
            sources,
        )
        .map_err(|e| e.to_string())?;
    let cx = (point[0].clamp(0.0, 1.0) * (size.0 - 1) as f32).round() as i64;
    let cy = (point[1].clamp(0.0, 1.0) * (size.1 - 1) as f32).round() as i64;
    let mut sum = [0.0f32; 3];
    let mut n = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let x = (cx + dx).clamp(0, size.0 as i64 - 1) as u32;
            let y = (cy + dy).clamp(0, size.1 as i64 - 1) as u32;
            let p = frame.pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c] as f32 / 255.0;
            }
            n += 1.0;
        }
    }
    Ok(sum.map(|v| v / n))
}
