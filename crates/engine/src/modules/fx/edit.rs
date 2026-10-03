//! Effect edits, as `EditCommand`s.
//!
//! The same command model the colour panel uses (`inspector/edit.rs`):
//!
//! - **Materials are never edited in place.** Every change mints a fresh
//!   [`EffectMaterial`] and the undoable edit is the segment's reference
//!   swapping over to it. The superseded material stays in the pool,
//!   unreferenced and inert, which is what lets undo swing the reference back
//!   exactly. It also makes sharing safe: the two halves of a split clip name
//!   the same material, and editing one half mints a new one for that half
//!   only.
//! - **The swap is a `Composite` of `RemoveSegment` and `InsertSegment`** of
//!   the same segment with its reference changed. Both primitives snapshot the
//!   whole segment, so the composite inverts exactly without a new variant in
//!   `timeline/ops.rs`, and `InsertSegment` re-runs every entry check.
//!
//! A builder returns the material it minted next to the command. The caller
//! puts the material into the pool *before* applying the command (the
//! ordering `text_add` documents: no instant may hold a segment naming a
//! material the pool lacks) and takes it back out if the command is refused.
//! `fx/commands.rs` does exactly that.

use crate::modules::project::document::{
    new_id, Easing, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
};
use crate::modules::project::effects::{put_keyframe, EffectMaterial, EffectValue};
use crate::modules::timeline::ops::EditCommand;

use super::catalog::{self, descriptor, ParamKind};

/// How long a new effect clip is when the caller does not say.
pub const DEFAULT_CLIP_DURATION: Micros = 3_000_000;

/// The remove + insert pair for `segment_id` with `mutate` applied.
pub(crate) fn replace_segment(
    project: &Project,
    segment_id: &str,
    label: &str,
    mutate: impl FnOnce(&mut Segment) -> Result<(), String>,
) -> Result<EditCommand, String> {
    let (track, before) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment found on this track");
    let mut after = before.clone();
    mutate(&mut after)?;
    Ok(EditCommand::Composite {
        label: label.to_string(),
        commands: vec![
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: before.clone(),
                index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: after,
                index,
            },
        ],
    })
}

/// The effect `effect_id` as `segment_id` applies it, or a refusal naming
/// what is wrong.
fn applied<'a>(
    project: &'a Project,
    segment_id: &str,
    effect_id: &str,
) -> Result<&'a EffectMaterial, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if segment.material_id != effect_id && !segment.extras.iter().any(|id| id == effect_id) {
        return Err("that effect is not on this clip".into());
    }
    project
        .materials
        .effect(effect_id)
        .ok_or_else(|| format!("unknown effect {effect_id}"))
}

/// Check every value of `material` against the catalog, clamping numbers into
/// range. Refuses non-finite values and parameters the effect does not have.
pub fn normalize(mut material: EffectMaterial) -> Result<EffectMaterial, String> {
    if let Some(field) = material.non_finite_field() {
        return Err(format!("{field} must be a finite number"));
    }
    let Some(desc) = descriptor(&material.kind) else {
        // An effect from a later build: keep its values exactly as they are.
        return Ok(material);
    };
    for (name, value) in material.params.iter_mut() {
        let spec = desc
            .param(name)
            .ok_or_else(|| format!("{} has no parameter {name}", desc.label))?;
        *value = match (spec.kind, *value) {
            (ParamKind::Color { .. }, EffectValue::Color(c)) => {
                EffectValue::Color(c.map(|v| v.clamp(0.0, 1.0)))
            }
            (ParamKind::Color { .. }, EffectValue::Number(_)) => {
                return Err(format!("{} takes a colour", spec.label))
            }
            (_, EffectValue::Number(v)) => EffectValue::Number(spec.clamp(v)),
            (_, EffectValue::Color(_)) => return Err(format!("{} takes a number", spec.label)),
        };
    }
    // A value equal to the default is not stored: "at rest" has one spelling.
    material
        .params
        .retain(|name, value| match desc.param(name) {
            Some(spec) => match (spec.kind, *value) {
                (ParamKind::Color { default }, EffectValue::Color(c)) => c != default,
                (_, EffectValue::Number(v)) => v != spec.default_number(),
                _ => true,
            },
            None => true,
        });
    for (name, keys) in material.keyframes.iter_mut() {
        let spec = desc
            .param(name)
            .ok_or_else(|| format!("{} has no parameter {name}", desc.label))?;
        if !spec.keyframable() {
            return Err(format!("{} cannot be animated", spec.label));
        }
        keys.sort_by_key(|k| k.time);
        keys.dedup_by_key(|k| k.time);
        for key in keys.iter_mut() {
            key.value = spec.clamp(key.value);
        }
    }
    material.keyframes.retain(|_, keys| !keys.is_empty());
    Ok(material)
}

/// Add a new effect of `kind`, at its defaults, to the end of `segment_id`'s
/// stack.
pub fn add_command(
    project: &Project,
    segment_id: &str,
    kind: &str,
) -> Result<(EffectMaterial, EditCommand), String> {
    let desc = descriptor(kind).ok_or_else(|| format!("there is no effect called {kind}"))?;
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if track.kind == TrackKind::Audio {
        return Err("effects apply to pictures, not to sound".into());
    }
    let _ = segment;
    let material = EffectMaterial::new(desc.id);
    let id = material.id.clone();
    let command = replace_segment(project, segment_id, &format!("Add {}", desc.label), |s| {
        s.extras.push(id);
        Ok(())
    })?;
    Ok((material, command))
}

/// Take `effect_id` off `segment_id`. An effect clip's own effect cannot be
/// removed this way — deleting the clip is how that goes.
pub fn remove_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
) -> Result<EditCommand, String> {
    let effect = applied(project, segment_id, effect_id)?;
    let label = descriptor(&effect.kind).map_or("effect", |d| d.label);
    replace_segment(project, segment_id, &format!("Remove {label}"), |s| {
        if s.material_id == effect_id {
            return Err(
                "this is the effect clip's own effect; delete the clip to remove it".into(),
            );
        }
        s.extras.retain(|id| id != effect_id);
        Ok(())
    })
}

/// Swap `effect_id` on `segment_id` for `updated`, minted under a new id and
/// normalised, at the same place in the stack.
pub fn replace_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
    updated: EffectMaterial,
    label: &str,
) -> Result<(EffectMaterial, EditCommand), String> {
    let current = applied(project, segment_id, effect_id)?;
    if updated.kind != current.kind {
        return Err("an edit cannot change which effect it is".into());
    }
    let mut material = normalize(updated)?;
    material.id = new_id();
    let new = material.id.clone();
    let command = replace_segment(project, segment_id, label, |s| {
        if s.material_id == effect_id {
            s.material_id = new;
        } else if let Some(slot) = s.extras.iter_mut().find(|id| *id == effect_id) {
            *slot = new;
        }
        Ok(())
    })?;
    Ok((material, command))
}

/// Set one parameter. A numeric parameter that is animated is set *at*
/// `source_time` — the keyframe there gets the value, or a new one is added —
/// because its static value is not what is shown.
pub fn set_param_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
    param: &str,
    value: EffectValue,
    source_time: Option<Micros>,
) -> Result<(EffectMaterial, EditCommand), String> {
    let current = applied(project, segment_id, effect_id)?;
    let desc = descriptor(&current.kind).ok_or("this effect is from a newer version")?;
    let spec = desc
        .param(param)
        .ok_or_else(|| format!("{} has no parameter {param}", desc.label))?;
    let mut updated = current.clone();
    match (value, updated.keyframes.get_mut(param), source_time) {
        (EffectValue::Number(v), Some(keys), Some(at)) if !keys.is_empty() => {
            put_keyframe(keys, at, v, Easing::Linear);
        }
        _ => {
            updated.params.insert(param.to_string(), value);
        }
    }
    replace_command(
        project,
        segment_id,
        effect_id,
        updated,
        &format!("Change {}", spec.label.to_lowercase()),
    )
}

/// The diamond: add a keyframe for `param` at `source_time` holding the value
/// shown there, or remove the one that is within `tolerance` of it.
pub fn toggle_keyframe_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
    param: &str,
    source_time: Micros,
    tolerance: Micros,
) -> Result<(EffectMaterial, EditCommand), String> {
    let current = applied(project, segment_id, effect_id)?;
    let desc = descriptor(&current.kind).ok_or("this effect is from a newer version")?;
    let spec = desc
        .param(param)
        .ok_or_else(|| format!("{} has no parameter {param}", desc.label))?;
    if !spec.keyframable() {
        return Err(format!("{} cannot be animated", spec.label));
    }
    let mut updated = current.clone();
    let shown = current.number_at(param, source_time, spec.default_number());
    let keys = updated.keyframes.entry(param.to_string()).or_default();
    let label = match keys
        .iter()
        .position(|k| (k.time - source_time).abs() <= tolerance)
    {
        Some(i) => {
            let removed = keys.remove(i);
            // The last keyframe going leaves the value it held, so taking the
            // animation away does not make the picture jump.
            if keys.is_empty() {
                updated
                    .params
                    .insert(param.to_string(), EffectValue::Number(removed.value));
            }
            "Delete keyframe"
        }
        None => {
            put_keyframe(keys, source_time, shown, Easing::Linear);
            "Add keyframe"
        }
    };
    replace_command(project, segment_id, effect_id, updated, label)
}

/// Switch an effect off or on, keeping its values.
pub fn set_enabled_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
    enabled: bool,
) -> Result<(EffectMaterial, EditCommand), String> {
    let current = applied(project, segment_id, effect_id)?;
    let mut updated = current.clone();
    updated.enabled = enabled;
    let label = if enabled {
        "Show effect"
    } else {
        "Hide effect"
    };
    replace_command(project, segment_id, effect_id, updated, label)
}

/// Every parameter of an effect back to its default, keyframes gone.
pub fn reset_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
) -> Result<(EffectMaterial, EditCommand), String> {
    let current = applied(project, segment_id, effect_id)?;
    let mut updated = current.clone();
    updated.params.clear();
    updated.keyframes.clear();
    replace_command(project, segment_id, effect_id, updated, "Reset effect")
}

/// Move `effect_id` to position `to` among the clip's `extras` effects, which
/// is the order they apply in.
pub fn move_command(
    project: &Project,
    segment_id: &str,
    effect_id: &str,
    to: usize,
) -> Result<EditCommand, String> {
    applied(project, segment_id, effect_id)?;
    let materials = &project.materials;
    replace_segment(project, segment_id, "Reorder effects", |s| {
        let order: Vec<String> = s
            .extras
            .iter()
            .filter(|id| materials.effect(id).is_some())
            .cloned()
            .collect();
        let from = order
            .iter()
            .position(|id| id == effect_id)
            .ok_or("an effect clip's own effect always comes first")?;
        let mut reordered = order.clone();
        let moved = reordered.remove(from);
        reordered.insert(to.min(reordered.len()), moved);
        if reordered == order {
            return Err("the effect is already there".into());
        }
        // Rewrite only the effect slots, in place, so the other ids in
        // `extras` (colour, transition, link) keep their positions.
        let mut next = reordered.into_iter();
        for id in s.extras.iter_mut() {
            if materials.effect(id).is_some() {
                *id = next.next().expect("same number of effect slots");
            }
        }
        Ok(())
    })
}

/// Where an effect clip was put.
#[derive(Debug, Clone)]
pub struct ClipPlacement {
    pub command: EditCommand,
    pub material: EffectMaterial,
    pub segment_id: String,
    pub track_id: String,
    pub start: Micros,
}

/// An effect clip of `kind` at `at` on an effect lane. It goes on the
/// topmost unlocked effect lane with room for it — `lane` first, when given —
/// or on a new lane on top of everything, because an effect clip applies to
/// what is beneath it and a new effect is meant to see the whole picture.
pub fn clip_command(
    project: &Project,
    kind: &str,
    at: Micros,
    duration: Micros,
    lane: Option<&str>,
) -> Result<ClipPlacement, String> {
    let desc = descriptor(kind).ok_or_else(|| format!("there is no effect called {kind}"))?;
    if duration <= 0 {
        return Err(format!("an effect clip cannot be {duration} µs long"));
    }
    let start = at.max(0);
    let range = TimeRange::new(start, duration);
    let fits =
        |t: &Track| t.kind == TrackKind::Effect && !t.locked && t.is_range_free(&range, None);
    let existing = lane
        .and_then(|id| project.track(id))
        .filter(|t| fits(t))
        .or_else(|| project.tracks.iter().rev().find(|t| fits(t)));
    let lane = match existing {
        Some(track) => track.clone(),
        None => {
            let count = project
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Effect)
                .count();
            Track::new(TrackKind::Effect, format!("Effects {}", count + 1))
        }
    };

    let material = EffectMaterial::new(desc.id);
    let segment = Segment {
        id: new_id(),
        material_id: material.id.clone(),
        target_range: range,
        source_range: TimeRange::new(0, duration),
        render_index: project.tracks.len() as i32,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    let segment_id = segment.id.clone();
    let track_id = lane.id.clone();
    let index = lane
        .segments
        .iter()
        .filter(|s| s.target_range.start < start)
        .count();
    let mut commands = Vec::new();
    if existing.is_none() {
        commands.push(EditCommand::AddTrack {
            track: lane,
            index: project.tracks.len(),
        });
    }
    commands.push(EditCommand::InsertSegment {
        track_id: track_id.clone(),
        segment,
        index,
    });
    Ok(ClipPlacement {
        command: EditCommand::Composite {
            label: format!("Add {}", desc.label),
            commands,
        },
        material,
        segment_id,
        track_id,
        start,
    })
}

/// The instant of `segment`'s source clock at timeline `time`, clamped into
/// the clip: the clock effect keyframes are stored in.
pub fn source_time(segment: &Segment, time: Micros) -> Micros {
    let start = segment.target_range.start;
    let end = segment.target_range.end() - 1;
    let t = time.clamp(start, end.max(start));
    segment
        .source_time_at(t)
        .unwrap_or(segment.source_range.start)
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// A split-screen arrangement: each selected clip fills one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitLayout {
    /// Two cells side by side.
    TwoColumns,
    /// Two cells stacked: the 9:16 reaction layout.
    TwoRows,
    /// Three cells stacked.
    ThreeRows,
    /// Three cells side by side.
    ThreeColumns,
    /// Four cells, two by two.
    Grid,
}

impl SplitLayout {
    pub const ALL: [SplitLayout; 5] = [
        SplitLayout::TwoRows,
        SplitLayout::TwoColumns,
        SplitLayout::ThreeRows,
        SplitLayout::ThreeColumns,
        SplitLayout::Grid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SplitLayout::TwoColumns => "Side by side",
            SplitLayout::TwoRows => "Top and bottom",
            SplitLayout::ThreeRows => "Three rows",
            SplitLayout::ThreeColumns => "Three columns",
            SplitLayout::Grid => "Grid",
        }
    }

    /// The cells as `[left, top, right, bottom]` fractions of the canvas, in
    /// reading order.
    pub fn cells(self) -> Vec<[f32; 4]> {
        let (cols, rows) = match self {
            SplitLayout::TwoColumns => (2, 1),
            SplitLayout::TwoRows => (1, 2),
            SplitLayout::ThreeRows => (1, 3),
            SplitLayout::ThreeColumns => (3, 1),
            SplitLayout::Grid => (2, 2),
        };
        let mut cells = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                cells.push([
                    c as f32 / cols as f32,
                    r as f32 / rows as f32,
                    (c + 1) as f32 / cols as f32,
                    (r + 1) as f32 / rows as f32,
                ]);
            }
        }
        cells
    }
}

/// The picture size of a segment's material, as displayed (rotation applied),
/// or `None` for a material with no fixed size (a title).
fn picture_size(project: &Project, segment: &Segment) -> Option<(f32, f32)> {
    let pool = &project.materials;
    if let Some(video) = pool.video(&segment.material_id) {
        let (w, h) = (video.width as f32, video.height as f32);
        return Some(if video.rotation.rem_euclid(180) == 90 {
            (h, w)
        } else {
            (w, h)
        });
    }
    pool.image(&segment.material_id)
        .map(|image| (image.width as f32, image.height as f32))
}

/// The transform and crop that make a picture of `source` size fill `cell`
/// (canvas fractions) exactly: a centred crop to the cell's shape, then the
/// scale that makes the fitted crop as wide as the cell.
pub fn fill_cell(
    canvas: (u32, u32),
    source: Option<(f32, f32)>,
    cell: [f32; 4],
) -> (Transform, Option<crate::modules::project::document::Crop>) {
    use crate::modules::project::document::Crop;
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let cell_w = (cell[2] - cell[0]) * cw;
    let cell_h = (cell[3] - cell[1]) * ch;
    let centre = ((cell[0] + cell[2]) * 0.5, (cell[1] + cell[3]) * 0.5);
    let mut transform = Transform {
        position: [(centre.0 - 0.5) * 2.0, (0.5 - centre.1) * 2.0],
        ..Transform::default()
    };
    let Some((sw, sh)) = source.filter(|(w, h)| *w > 0.0 && *h > 0.0) else {
        // No intrinsic size: centre it in the cell at the cell's height.
        transform.scale = [cell_h / ch, cell_h / ch];
        return (transform, None);
    };
    let cell_aspect = cell_w / cell_h;
    let source_aspect = sw / sh;
    let crop = if (source_aspect - cell_aspect).abs() < 1e-4 {
        None
    } else if source_aspect > cell_aspect {
        let keep = cell_aspect / source_aspect;
        Some(Crop {
            left: (1.0 - keep) * 0.5,
            top: 0.0,
            right: 1.0 - (1.0 - keep) * 0.5,
            bottom: 1.0,
        })
    } else {
        let keep = source_aspect / cell_aspect;
        Some(Crop {
            left: 0.0,
            top: (1.0 - keep) * 0.5,
            right: 1.0,
            bottom: 1.0 - (1.0 - keep) * 0.5,
        })
    };
    // The cropped picture has the cell's aspect; the compositor first fits it
    // to the canvas, then applies the scale.
    let (fit_w, _) = crate::modules::render::layout::fit_size(
        canvas,
        ((cell_aspect * 1000.0).round().max(1.0) as u32, 1000),
    );
    let scale = cell_w / fit_w;
    transform.scale = [scale, scale];
    (transform, crop)
}

/// Arrange `segment_ids`, in order, into the cells of `layout`, as one undo
/// step. Each clip keeps its opacity and flips; position, scale, rotation and
/// crop are set. Clips beyond the number of cells are left alone.
pub fn split_command(
    project: &Project,
    segment_ids: &[String],
    layout: SplitLayout,
) -> Result<EditCommand, String> {
    if segment_ids.len() < 2 {
        return Err("select two or more clips to arrange them".into());
    }
    let canvas = (project.canvas.width, project.canvas.height);
    let mut commands = Vec::new();
    for (id, cell) in segment_ids.iter().zip(layout.cells()) {
        let (track, segment) = project
            .segment(id)
            .ok_or_else(|| format!("unknown segment {id}"))?;
        if track.kind == TrackKind::Audio || project.materials.is_effect_clip(segment) {
            return Err("only pictures can be arranged".into());
        }
        let (mut transform, crop) = fill_cell(canvas, picture_size(project, segment), cell);
        transform.opacity = segment.transform.opacity;
        transform.flip_h = segment.transform.flip_h;
        transform.flip_v = segment.transform.flip_v;
        let composite = replace_segment(project, id, "Arrange", |s| {
            s.transform = transform;
            s.crop = crop;
            Ok(())
        })?;
        if let EditCommand::Composite {
            commands: inner, ..
        } = composite
        {
            commands.extend(inner);
        }
    }
    Ok(EditCommand::Composite {
        label: format!("Arrange: {}", layout.label().to_lowercase()),
        commands,
    })
}

/// Which corner a picture-in-picture goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Make `segment_id` a picture in picture: a third of the canvas wide, in
/// `corner` with a margin, and a frame effect with rounded corners, a thin
/// border and a soft shadow. One undo step.
pub fn pip_command(
    project: &Project,
    segment_id: &str,
    corner: Corner,
) -> Result<(EffectMaterial, EditCommand), String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if track.kind == TrackKind::Audio || project.materials.is_effect_clip(segment) {
        return Err("only a picture can be a picture in picture".into());
    }
    let canvas = (project.canvas.width, project.canvas.height);
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let source = picture_size(project, segment).unwrap_or((cw, ch));
    let (fit_w, fit_h) = crate::modules::render::layout::fit_size(
        canvas,
        (source.0.max(1.0) as u32, source.1.max(1.0) as u32),
    );
    let scale = (cw * 0.38) / fit_w;
    let (w, h) = (fit_w * scale, fit_h * scale);
    let margin = 0.05 * cw.min(ch);
    let x = (cw - w) * 0.5 - margin;
    let y = (ch - h) * 0.5 - margin;
    let (sx, sy) = match corner {
        Corner::TopLeft => (-1.0, 1.0),
        Corner::TopRight => (1.0, 1.0),
        Corner::BottomLeft => (-1.0, -1.0),
        Corner::BottomRight => (1.0, -1.0),
    };
    let transform = Transform {
        position: [sx * x / (cw * 0.5), sy * y / (ch * 0.5)],
        scale: [scale, scale],
        ..segment.transform
    };

    let materials = &project.materials;
    let mut frame = segment
        .extras
        .iter()
        .filter_map(|id| materials.effect(id))
        .find(|e| e.kind == catalog::FRAME)
        .cloned()
        .unwrap_or_else(|| EffectMaterial::new(catalog::FRAME));
    frame.id = new_id();
    for (name, value) in [
        ("radius", 14.0),
        ("border", 6.0),
        ("shadow", 55.0),
        ("shadow_blur", 35.0),
        ("shadow_distance", 15.0),
    ] {
        frame
            .params
            .insert(name.to_string(), EffectValue::Number(value));
    }
    let frame = normalize(frame)?;
    let frame_id = frame.id.clone();
    let command = replace_segment(project, segment_id, "Picture in picture", |s| {
        s.transform = transform;
        s.extras.retain(|id| {
            materials
                .effect(id)
                .is_none_or(|e| e.kind != catalog::FRAME)
        });
        s.extras.push(frame_id);
        Ok(())
    })?;
    Ok((frame, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, VideoMaterial};
    use crate::modules::timeline::history::History;

    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/x.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "Main");
        for (i, id) in ["a", "b"].iter().enumerate() {
            track.segments.push(Segment {
                id: (*id).into(),
                material_id: "v".into(),
                target_range: TimeRange::new(i as Micros * 2_000_000, 2_000_000),
                source_range: TimeRange::new(0, 2_000_000),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                transform: Transform::default(),
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            });
        }
        p.tracks.push(track);
        p
    }

    /// Put the minted material in the pool and apply, as `commands.rs` does.
    fn commit(
        p: &mut Project,
        h: &mut History,
        build: impl FnOnce(&Project) -> (EffectMaterial, EditCommand),
    ) -> String {
        let built = build(p);
        let id = built.0.id.clone();
        p.materials.effects.push(built.0);
        h.apply(p, built.1).unwrap();
        id
    }

    fn stack(p: &Project, segment: &str) -> Vec<EffectMaterial> {
        let (_, s) = p.segment(segment).unwrap();
        p.materials.effects_of(s).into_iter().cloned().collect()
    }

    #[test]
    fn adding_setting_and_undoing_an_effect_round_trips() {
        let mut p = project();
        let mut h = History::new();
        let blur = commit(&mut p, &mut h, |p| {
            add_command(p, "a", "gaussian_blur").unwrap()
        });
        assert_eq!(stack(&p, "a").len(), 1);

        let set =
            set_param_command(&p, "a", &blur, "radius", EffectValue::Number(75.0), None).unwrap();
        let blur2 = commit(&mut p, &mut h, |_| set);
        let effects = stack(&p, "a");
        assert_eq!(effects[0].id, blur2);
        assert_eq!(effects[0].number_at("radius", 0, 0.0), 75.0);

        h.undo(&mut p).unwrap();
        assert_eq!(stack(&p, "a")[0].id, blur);
        h.undo(&mut p).unwrap();
        assert!(stack(&p, "a").is_empty());
        h.redo(&mut p).unwrap();
        h.redo(&mut p).unwrap();
        assert_eq!(stack(&p, "a")[0].number_at("radius", 0, 0.0), 75.0);
        assert!(p
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::Severity::Error));
    }

    #[test]
    fn values_are_clamped_and_a_default_value_is_not_stored() {
        let mut p = project();
        let mut h = History::new();
        let glow = commit(&mut p, &mut h, |p| add_command(p, "a", "glow").unwrap());
        let over = set_param_command(
            &p,
            "a",
            &glow,
            "intensity",
            EffectValue::Number(500.0),
            None,
        )
        .unwrap();
        assert_eq!(over.0.number_at("intensity", 0, 0.0), 100.0);
        let rest = set_param_command(&p, "a", &glow, "intensity", EffectValue::Number(60.0), None)
            .unwrap();
        assert!(rest.0.params.is_empty(), "a default must not be stored");
        assert!(set_param_command(&p, "a", &glow, "nope", EffectValue::Number(1.0), None).is_err());
        assert!(
            set_param_command(&p, "a", &glow, "tint", EffectValue::Number(1.0), None).is_err(),
            "a colour parameter refuses a number"
        );
        assert!(set_param_command(
            &p,
            "a",
            &glow,
            "intensity",
            EffectValue::Number(f32::NAN),
            None
        )
        .is_err());
    }

    #[test]
    fn a_keyframed_parameter_is_set_at_the_playhead() {
        let mut p = project();
        let mut h = History::new();
        let id = commit(&mut p, &mut h, |p| {
            add_command(p, "a", "gaussian_blur").unwrap()
        });
        let id = commit(&mut p, &mut h, |p| {
            toggle_keyframe_command(p, "a", &id, "radius", 0, 1000).unwrap()
        });
        let id = commit(&mut p, &mut h, |p| {
            set_param_command(
                p,
                "a",
                &id,
                "radius",
                EffectValue::Number(80.0),
                Some(1_000_000),
            )
            .unwrap()
        });
        let effect = stack(&p, "a")[0].clone();
        assert_eq!(effect.keyframes["radius"].len(), 2);
        assert_eq!(effect.number_at("radius", 0, 0.0), 20.0);
        assert_eq!(effect.number_at("radius", 500_000, 0.0), 50.0);
        // Removing both keyframes leaves the last value as the static one.
        let id = commit(&mut p, &mut h, |p| {
            toggle_keyframe_command(p, "a", &id, "radius", 0, 1000).unwrap()
        });
        commit(&mut p, &mut h, |p| {
            toggle_keyframe_command(p, "a", &id, "radius", 1_000_000, 1000).unwrap()
        });
        let effect = stack(&p, "a")[0].clone();
        assert!(effect.keyframes.is_empty());
        assert_eq!(effect.number_at("radius", 0, 0.0), 80.0);
    }

    #[test]
    fn reordering_moves_only_effect_slots() {
        let mut p = project();
        let mut h = History::new();
        let a = commit(&mut p, &mut h, |p| {
            add_command(p, "a", "gaussian_blur").unwrap()
        });
        p.materials.links.insert("link".into());
        p.segment_mut("a").unwrap().extras.push("link".into());
        let b = commit(&mut p, &mut h, |p| add_command(p, "a", "glow").unwrap());
        let command = move_command(&p, "a", &b, 0).unwrap();
        h.apply(&mut p, command).unwrap();
        let extras = p.segment("a").unwrap().1.extras.clone();
        assert_eq!(extras, vec![b.clone(), "link".to_string(), a.clone()]);
        h.undo(&mut p).unwrap();
        assert_eq!(
            p.segment("a").unwrap().1.extras,
            vec![a, "link".to_string(), b]
        );
    }

    #[test]
    fn removing_an_effect_undoes_exactly() {
        let mut p = project();
        let mut h = History::new();
        let id = commit(&mut p, &mut h, |p| add_command(p, "a", "vhs").unwrap());
        let command = remove_command(&p, "a", &id).unwrap();
        h.apply(&mut p, command).unwrap();
        assert!(stack(&p, "a").is_empty());
        h.undo(&mut p).unwrap();
        assert_eq!(stack(&p, "a")[0].id, id);
        // The other clip never saw any of it.
        assert!(stack(&p, "b").is_empty());
    }

    #[test]
    fn an_effect_clip_goes_on_a_new_lane_on_top_and_validates() {
        let mut p = project();
        let mut h = History::new();
        let placed = clip_command(&p, "shake", 500_000, 1_000_000, None).unwrap();
        p.materials.effects.push(placed.material.clone());
        h.apply(&mut p, placed.command.clone()).unwrap();
        let (track, segment) = p.segment(&placed.segment_id).unwrap();
        assert_eq!(track.kind, TrackKind::Effect);
        assert!(p.materials.is_effect_clip(segment));
        assert_eq!(p.tracks.last().unwrap().id, track.id);
        // Only the fixture's missing file is reported: an effect clip is not a
        // clip with missing media.
        let issues = p.validate();
        assert!(
            issues
                .iter()
                .all(|i| i.message.starts_with("media file is missing")),
            "{issues:?}"
        );

        // A second one at the same time needs a second lane; one later fits
        // on the first.
        let overlapping = clip_command(&p, "glow", 1_000_000, 1_000_000, None).unwrap();
        assert!(
            matches!(&overlapping.command, EditCommand::Composite { commands, .. }
            if matches!(commands[0], EditCommand::AddTrack { .. }))
        );
        let later = clip_command(&p, "glow", 5_000_000, 1_000_000, None).unwrap();
        assert_eq!(later.track_id, placed.track_id);

        // Its own effect is edited by swapping the clip's material.
        let set = set_param_command(
            &p,
            &placed.segment_id,
            &placed.material.id,
            "amplitude",
            EffectValue::Number(90.0),
            None,
        )
        .unwrap();
        let new_id = commit(&mut p, &mut h, |_| set);
        assert_eq!(p.segment(&placed.segment_id).unwrap().1.material_id, new_id);
        assert!(remove_command(&p, &placed.segment_id, &new_id).is_err());
    }

    #[test]
    fn a_split_screen_fills_each_cell_exactly() {
        let canvas = (1080, 1920);
        let (t, crop) = fill_cell(canvas, Some((1920.0, 1080.0)), [0.0, 0.0, 1.0, 0.5]);
        // The cell is 1080x960; the 16:9 picture is cropped to 9:8.
        let crop = crop.unwrap();
        let kept = crop.right - crop.left;
        assert!((kept * 1920.0 / 1080.0 - 1080.0 / 960.0).abs() < 1e-4);
        // Placed with the real placement maths, its corners are the cell's.
        let placement =
            crate::modules::render::layout::place_quad(canvas, (1920, 1080), &t, Some(crop))
                .unwrap();
        let m = glam::Mat4::from_cols_array(&placement.mvp);
        let corner = |x: f32, y: f32| {
            let v = m * glam::Vec4::new(x, y, 0.0, 1.0);
            ((v.x + 1.0) * 0.5 * 1080.0, (1.0 - v.y) * 0.5 * 1920.0)
        };
        let (l, top) = corner(-0.5, 0.5);
        let (r, bottom) = corner(0.5, -0.5);
        assert!((l - 0.0).abs() < 1.0 && (r - 1080.0).abs() < 1.0, "{l} {r}");
        assert!(
            (top - 0.0).abs() < 1.0 && (bottom - 960.0).abs() < 1.0,
            "{top} {bottom}"
        );
    }

    #[test]
    fn arranging_two_clips_is_one_undo_step() {
        let mut p = project();
        // Put "b" on a lane of its own over "a" so they play together.
        let b = p.tracks[0].segments.remove(1);
        let mut upper = Track::new(TrackKind::Video, "Upper");
        upper.segments.push(Segment {
            target_range: TimeRange::new(0, 2_000_000),
            ..b
        });
        p.tracks.push(upper);
        let mut h = History::new();
        let command = split_command(&p, &["a".into(), "b".into()], SplitLayout::TwoRows).unwrap();
        h.apply(&mut p, command).unwrap();
        let a = p.segment("a").unwrap().1.clone();
        let b = p.segment("b").unwrap().1.clone();
        assert!(a.transform.position[1] > 0.0, "the first clip goes on top");
        assert!(b.transform.position[1] < 0.0);
        assert!(a.crop.is_some() && b.crop.is_some());
        h.undo(&mut p).unwrap();
        assert!(p.segment("a").unwrap().1.crop.is_none());
        assert_eq!(p.segment("b").unwrap().1.transform.position, [0.0, 0.0]);
        assert!(split_command(&p, &["a".into()], SplitLayout::Grid).is_err());
    }

    #[test]
    fn picture_in_picture_scales_into_a_corner_and_frames_the_clip() {
        let mut p = project();
        let mut h = History::new();
        let built = pip_command(&p, "a", Corner::TopRight).unwrap();
        commit(&mut p, &mut h, |_| built);
        let a = p.segment("a").unwrap().1.clone();
        assert!(a.transform.position[0] > 0.0 && a.transform.position[1] > 0.0);
        assert!(a.transform.scale[0] < 1.0);
        let effects = stack(&p, "a");
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].kind, catalog::FRAME);
        // Doing it again replaces the frame rather than stacking a second.
        let built = pip_command(&p, "a", Corner::BottomLeft).unwrap();
        commit(&mut p, &mut h, |_| built);
        assert_eq!(stack(&p, "a").len(), 1);
    }
}
