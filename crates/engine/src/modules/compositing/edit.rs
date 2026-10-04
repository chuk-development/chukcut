//! The edits for masks, the chroma key and blend modes, as `EditCommand`s.
//!
//! Every builder here works the same way, decision 0007's: read the clip's
//! current [`CompositingMaterial`] (or an empty one), change a copy, mint it
//! under a new id, and return it with the edit that swaps the clip's
//! reference over — a `RemoveSegment` + `InsertSegment` composite of the same
//! segment, so undo is exact and no new `EditCommand` variant is needed. The
//! caller pushes the material into the pool before applying the edit.
//!
//! A material that would change nothing (no mask, no key, normal blending)
//! is not minted at all: the edit takes the reference off the clip, so
//! "remove the last mask" and "never had one" are the same document.

use crate::modules::project::compositing::{
    clamp_param, BackgroundRemoval, BlendMode, ChromaKey, CompositingMaterial, Mask, MaskOp,
    MaskShape, MASK_PARAMS,
};
use crate::modules::project::document::{new_id, MaterialKind, Micros, Project, Segment};
use crate::modules::timeline::ops::EditCommand;

/// What an edit produced: the material to put into the pool first (`None`
/// when the clip ends up with nothing), and the edit.
pub type Minted = (Option<CompositingMaterial>, EditCommand);

/// The clip's current material, or an empty one.
pub fn current(project: &Project, segment_id: &str) -> Result<CompositingMaterial, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if project.materials.is_effect_clip(segment) {
        return Err("an effect clip has no picture to mask".into());
    }
    if project.materials.kind_of(&segment.material_id) == Some(MaterialKind::Audio) {
        return Err("a sound clip has no picture to mask".into());
    }
    Ok(project
        .materials
        .compositing_of(segment)
        .cloned()
        .unwrap_or_default())
}

/// The clip's source time at timeline `time`, clamped into the clip: where a
/// keyframe set "at the playhead" lands.
pub fn source_time_in(project: &Project, segment: &Segment, time: Micros) -> Micros {
    project
        .materials
        .time_map(segment)
        .clamped_source_time(time)
}

/// Swap the clip's material for `material` (validated, normalised, minted).
pub fn set_material_command(
    project: &Project,
    segment_id: &str,
    material: CompositingMaterial,
    label: &str,
) -> Result<Minted, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if let Some(field) = material.non_finite_field() {
        return Err(format!("{field} must be a finite number"));
    }
    let had = project.materials.compositing_of(segment);
    let mut material = CompositingMaterial {
        masks: material.masks.into_iter().map(Mask::normalized).collect(),
        key: material.key.map(ChromaKey::normalized),
        view_matte: false,
        ..material
    };
    if let Some(had) = had {
        let mut same = material.clone();
        same.id = had.id.clone();
        if same == *had {
            return Err("nothing changed".into());
        }
    }
    let minted = if material.is_identity() {
        if had.is_none() {
            return Err("the clip has no masks, key or blend mode to remove".into());
        }
        None
    } else {
        material.id = new_id();
        Some(material)
    };
    let new_id = minted.as_ref().map(|m| m.id.clone());
    let materials = &project.materials;
    let command = crate::modules::inspector::edit::replace_segment(
        project,
        segment_id,
        label,
        move |segment| {
            // Every id that resolves in the category goes, at most one comes
            // back; this also repairs a segment carrying two.
            segment
                .extras
                .retain(|id| materials.compositing(id).is_none());
            if let Some(id) = new_id {
                segment.extras.push(id);
            }
        },
    )?;
    Ok((minted, command))
}

/// Change the clip's material with `change`.
fn update(
    project: &Project,
    segment_id: &str,
    label: &str,
    change: impl FnOnce(&mut CompositingMaterial) -> Result<(), String>,
) -> Result<Minted, String> {
    let mut material = current(project, segment_id)?;
    change(&mut material)?;
    set_material_command(project, segment_id, material, label)
}

fn mask_mut<'a>(
    material: &'a mut CompositingMaterial,
    mask_id: &str,
) -> Result<&'a mut Mask, String> {
    material
        .masks
        .iter_mut()
        .find(|m| m.id == mask_id)
        .ok_or_else(|| "the clip has no such mask".to_string())
}

fn known_param(param: &str) -> Result<(), String> {
    if MASK_PARAMS.contains(&param) {
        Ok(())
    } else {
        Err(format!(
            "a mask has no {param:?}; it has {}",
            MASK_PARAMS.join(", ")
        ))
    }
}

/// Add a mask of `shape` at its defaults; answers its id alongside.
pub fn add_mask_command(
    project: &Project,
    segment_id: &str,
    shape: MaskShape,
) -> Result<(Minted, String), String> {
    if shape.code().is_none() {
        return Err(format!(
            "{shape} is not a mask shape; choose one of {}",
            MaskShape::NAMES.join(", ")
        ));
    }
    let mask = Mask::new(shape);
    let id = mask.id.clone();
    let minted = update(project, segment_id, "Add mask", |m| {
        m.masks.push(mask);
        Ok(())
    })?;
    Ok((minted, id))
}

pub fn remove_mask_command(
    project: &Project,
    segment_id: &str,
    mask_id: &str,
) -> Result<Minted, String> {
    update(project, segment_id, "Remove mask", |m| {
        let before = m.masks.len();
        m.masks.retain(|mask| mask.id != mask_id);
        if m.masks.len() == before {
            return Err("the clip has no such mask".into());
        }
        Ok(())
    })
}

/// Replace one mask wholesale, keeping its place: what a drag on the player
/// commits.
pub fn replace_mask_command(
    project: &Project,
    segment_id: &str,
    mask: Mask,
    label: &str,
) -> Result<Minted, String> {
    update(project, segment_id, label, |m| {
        let id = mask.id.clone();
        *mask_mut(m, &id)? = mask;
        Ok(())
    })
}

/// Set one numeric parameter. With `at` (a source time) and the parameter
/// animated, the keyframe there is set instead of the static value.
pub fn set_mask_value_command(
    project: &Project,
    segment_id: &str,
    mask_id: &str,
    param: &str,
    value: f32,
    at: Option<Micros>,
) -> Result<Minted, String> {
    known_param(param)?;
    if !value.is_finite() {
        return Err(format!("{param} must be a finite number"));
    }
    update(project, segment_id, "Change mask", |m| {
        let mask = mask_mut(m, mask_id)?;
        match at {
            Some(time) if mask.is_animated(param) => mask.put_keyframe(param, time, value),
            _ => {
                mask.set_stored(param, value);
            }
        }
        Ok(())
    })
}

/// Add a keyframe of `param` at `at` (source time) holding the value shown
/// there, or take away the one within `tolerance` of it.
pub fn toggle_mask_keyframe_command(
    project: &Project,
    segment_id: &str,
    mask_id: &str,
    param: &str,
    at: Micros,
    tolerance: Micros,
) -> Result<Minted, String> {
    known_param(param)?;
    let mut label = "Add keyframe";
    let minted = {
        let mut material = current(project, segment_id)?;
        let mask = mask_mut(&mut material, mask_id)?;
        let shown = mask.value_at(param, at);
        let keys = mask.keyframes.entry(param.to_string()).or_default();
        match keys.iter().position(|k| (k.time - at).abs() <= tolerance) {
            Some(i) => {
                let removed = keys.remove(i);
                // The last keyframe going leaves its value behind, so taking
                // the animation away does not make the mask jump.
                if keys.is_empty() {
                    mask.keyframes.remove(param);
                    mask.set_stored(param, removed.value);
                }
                label = "Delete keyframe";
            }
            None => mask.put_keyframe(param, at, shown),
        }
        set_material_command(project, segment_id, material, label)?
    };
    Ok(minted)
}

/// Set the flags and the shape of one mask; `None` leaves that one alone.
pub fn set_mask_options_command(
    project: &Project,
    segment_id: &str,
    mask_id: &str,
    shape: Option<MaskShape>,
    op: Option<MaskOp>,
    invert: Option<bool>,
    enabled: Option<bool>,
) -> Result<Minted, String> {
    if let Some(shape) = &shape {
        if shape.code().is_none() {
            return Err(format!("{shape} is not a mask shape"));
        }
    }
    if let Some(op) = &op {
        if op.code().is_none() {
            return Err(format!(
                "{op} is not a mask operation; choose one of {}",
                MaskOp::NAMES.join(", ")
            ));
        }
    }
    update(project, segment_id, "Change mask", |m| {
        let mask = mask_mut(m, mask_id)?;
        if let Some(shape) = shape {
            mask.shape = shape;
        }
        if let Some(op) = op {
            mask.op = op;
        }
        if let Some(invert) = invert {
            mask.invert = invert;
        }
        if let Some(enabled) = enabled {
            mask.enabled = enabled;
        }
        Ok(())
    })
}

/// Move one mask to `index` in the order masks combine in.
pub fn move_mask_command(
    project: &Project,
    segment_id: &str,
    mask_id: &str,
    index: usize,
) -> Result<Minted, String> {
    update(project, segment_id, "Reorder masks", |m| {
        let from = m
            .masks
            .iter()
            .position(|mask| mask.id == mask_id)
            .ok_or("the clip has no such mask")?;
        let mask = m.masks.remove(from);
        let to = index.min(m.masks.len());
        m.masks.insert(to, mask);
        Ok(())
    })
}

/// Set the chroma key, or take it off with `None`.
pub fn set_key_command(
    project: &Project,
    segment_id: &str,
    key: Option<ChromaKey>,
) -> Result<Minted, String> {
    update(project, segment_id, "Chroma key", |m| {
        m.key = key;
        Ok(())
    })
}

/// Turn "Remove background" on (with the model that makes the matte) or off.
pub fn set_background_command(
    project: &Project,
    segment_id: &str,
    background: Option<BackgroundRemoval>,
) -> Result<Minted, String> {
    let label = if background.is_some() {
        "Remove background"
    } else {
        "Keep background"
    };
    update(project, segment_id, label, |m| {
        m.background = background;
        Ok(())
    })
}

/// Set the blend mode.
pub fn set_blend_command(
    project: &Project,
    segment_id: &str,
    blend: BlendMode,
) -> Result<Minted, String> {
    if blend.code().is_none() && !blend.is_normal() {
        return Err(format!(
            "{blend} is not a blend mode; choose one of {}",
            BlendMode::NAMES.join(", ")
        ));
    }
    update(project, segment_id, "Blend mode", |m| {
        m.blend = blend;
        Ok(())
    })
}

/// What a reset takes off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetPart {
    Masks,
    Key,
    Blend,
    All,
}

pub fn reset_command(
    project: &Project,
    segment_id: &str,
    part: ResetPart,
) -> Result<Minted, String> {
    update(project, segment_id, "Reset", |m| {
        match part {
            ResetPart::Masks => m.masks.clear(),
            ResetPart::Key => m.key = None,
            ResetPart::Blend => m.blend = BlendMode::Normal,
            ResetPart::All => {
                m.masks.clear();
                m.key = None;
                m.blend = BlendMode::Normal;
            }
        }
        Ok(())
    })
}

/// A drag on the player, resolved: move by `delta` (fractions of the clip),
/// set a size, a turn or a feather. The value goes where
/// [`set_mask_value_command`] would put it, so a drag on an animated mask
/// keys it at the playhead.
pub fn pose_mask(mask: &mut Mask, param: &str, value: f32, at: Option<Micros>) {
    let value = clamp_param(param, value);
    match at {
        Some(time) if mask.is_animated(param) => mask.put_keyframe(param, time, value),
        _ => {
            mask.set_stored(param, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };
    use crate::modules::timeline::history::History;

    fn project() -> Project {
        let mut project = Project::new("p", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nonexistent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(track);
        project
    }

    fn apply(project: &mut Project, history: &mut History, (material, command): Minted) {
        if let Some(material) = material {
            project.materials.compositing.push(material);
        }
        history.apply(project, command).expect("applies");
    }

    macro_rules! step {
        ($p:ident, $h:ident, $e:expr $(,)?) => {{
            let minted = $e;
            apply(&mut $p, &mut $h, minted);
        }};
    }

    fn of(project: &Project) -> Option<CompositingMaterial> {
        let (_, s) = project.segment("s").unwrap();
        project.materials.compositing_of(s).cloned()
    }

    #[test]
    fn add_change_remove_and_undo_round_trip_exactly() {
        let mut p = project();
        let mut h = History::new();
        let before = serde_json::to_string(&p.tracks).unwrap();

        let (minted, mask_id) = add_mask_command(&p, "s", MaskShape::Ellipse).unwrap();
        step!(p, h, minted);
        assert_eq!(of(&p).unwrap().masks.len(), 1);

        step!(
            p,
            h,
            set_mask_value_command(&p, "s", &mask_id, "feather", 0.3, None).unwrap()
        );
        assert_eq!(of(&p).unwrap().masks[0].feather, 0.3);

        step!(p, h, set_blend_command(&p, "s", BlendMode::Screen).unwrap());
        step!(
            p,
            h,
            set_key_command(&p, "s", Some(ChromaKey::new([0.0, 1.0, 0.0]))).unwrap()
        );
        let full = of(&p).unwrap();
        assert_eq!(full.blend, BlendMode::Screen);
        assert!(full.key.is_some());

        step!(p, h, reset_command(&p, "s", ResetPart::All).unwrap());
        assert!(of(&p).is_none(), "an empty material is no material");
        assert_eq!(serde_json::to_string(&p.tracks).unwrap(), before);

        h.undo(&mut p).unwrap();
        assert_eq!(of(&p).unwrap(), full);
        for _ in 0..4 {
            h.undo(&mut p).unwrap();
        }
        assert_eq!(serde_json::to_string(&p.tracks).unwrap(), before);
    }

    #[test]
    fn a_value_set_on_an_animated_mask_lands_on_a_keyframe() {
        let mut p = project();
        let mut h = History::new();
        let (minted, id) = add_mask_command(&p, "s", MaskShape::Rectangle).unwrap();
        step!(p, h, minted);
        step!(
            p,
            h,
            toggle_mask_keyframe_command(&p, "s", &id, "x", 0, 1000).unwrap()
        );
        step!(
            p,
            h,
            set_mask_value_command(&p, "s", &id, "x", 0.25, Some(2_000_000)).unwrap()
        );
        let mask = of(&p).unwrap().masks[0].clone();
        assert_eq!(mask.keyframes["x"].len(), 2);
        assert!((mask.value_at("x", 1_000_000) - 0.125).abs() < 1e-6);
        // Toggling the first keyframe away and then the last leaves the last
        // value as the static one.
        step!(
            p,
            h,
            toggle_mask_keyframe_command(&p, "s", &id, "x", 0, 1000).unwrap()
        );
        step!(
            p,
            h,
            toggle_mask_keyframe_command(&p, "s", &id, "x", 2_000_000, 1000).unwrap()
        );
        let mask = of(&p).unwrap().masks[0].clone();
        assert!(mask.keyframes.is_empty());
        assert_eq!(mask.x, 0.25);
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        let p = project();
        assert!(add_mask_command(&p, "s", MaskShape::Other("spiral".into())).is_err());
        assert!(set_blend_command(&p, "s", BlendMode::Other("vivid".into())).is_err());
        assert!(set_mask_value_command(&p, "s", "nope", "x", 0.1, None).is_err());
        assert!(add_mask_command(&p, "missing", MaskShape::Ellipse).is_err());
        assert!(reset_command(&p, "s", ResetPart::All).is_err());
    }

    #[test]
    fn masks_reorder_and_options_change() {
        let mut p = project();
        let mut h = History::new();
        let (m, a) = add_mask_command(&p, "s", MaskShape::Ellipse).unwrap();
        step!(p, h, m);
        let (m, b) = add_mask_command(&p, "s", MaskShape::Star).unwrap();
        step!(p, h, m);
        step!(p, h, move_mask_command(&p, "s", &b, 0).unwrap());
        assert_eq!(of(&p).unwrap().masks[0].id, b);
        step!(
            p,
            h,
            set_mask_options_command(&p, "s", &a, None, Some(MaskOp::Subtract), Some(true), None)
                .unwrap(),
        );
        let mask = of(&p).unwrap().mask(&a).cloned().unwrap();
        assert_eq!(mask.op, MaskOp::Subtract);
        assert!(mask.invert);
    }
}
