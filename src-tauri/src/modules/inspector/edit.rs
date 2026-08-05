//! Crop and colour edits, built out of existing command primitives.
//!
//! ## Why these are a `Composite` of remove + insert
//!
//! `EditCommand` has no `SetCrop` and no colour variant, and the enum lives in
//! `timeline/ops.rs`, which this module does not reach into. Rather than the
//! documented deviation `text_set` took — mutate directly, lose undo — both
//! edits here are spelled as a `Composite` of `RemoveSegment` followed by
//! `InsertSegment` of the same segment with one field changed. That is not a
//! trick; it is the command model used as designed:
//!
//! - Both primitives snapshot the **whole** segment, so the composite is
//!   exactly invertible without any new code, and undo restores crop, colour
//!   reference, keyframes and transform together, byte for byte.
//! - `InsertSegment` re-runs every entry check (`check_segment`, overlap,
//!   non-finite floats), so a crop with a NaN in it is refused at the boundary
//!   exactly like a transform with one.
//! - `History::apply` deliberately does not link-mirror or recurse into a
//!   `Composite`, which is the behaviour these edits want anyway: cropping a
//!   video clip must not do anything to the linked audio clip.
//!
//! When `timeline/ops.rs` grows `SetCrop`/`SetColorAdjust` variants, each
//! builder here shrinks to constructing that variant and nothing else changes.
//!
//! ## Where a colour edit's material comes from
//!
//! A colour adjustment is a pool material referenced from `Segment::extras`
//! (see [`ColorAdjustMaterial`] for the placement argument). Materials are
//! **never edited in place**: every committed change mints a fresh material
//! and the undoable edit is the segment's reference swapping over to it. The
//! superseded material stays in the pool unreferenced — inert, like an emptied
//! link group, a few dozen bytes that undo can swing the reference back to.
//! That is what makes colour undo exact without a command that mutates the
//! pool.

use crate::modules::project::document::{
    new_id, source_duration_for, ColorAdjustMaterial, Crop, LutRef, Project, Segment, TimeRange,
    Transform,
};
use crate::modules::timeline::ops::EditCommand;

/// The values of a colour edit as the panel sends them: no id, because the
/// material identity is minted here, not chosen by the webview.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ColorEdit {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    /// The .cube look, if any. Defaulted so a payload from before LUTs
    /// existed still deserialises.
    #[serde(default)]
    pub lut: Option<LutRef>,
}

impl ColorEdit {
    /// Identity means "nothing at all": scalars at rest *and* no look. A
    /// grade with only a LUT is a real grade — the identity-is-absence rule
    /// applies to the material, not to each half separately.
    fn is_identity(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 1.0
            && self.saturation == 1.0
            && self.temperature == 0.0
            && self.lut.is_none()
    }
}

/// The remove + insert pair described in the module docs, for a segment with
/// `mutate` applied to it.
fn replace_segment(
    project: &Project,
    segment_id: &str,
    label: &str,
    mutate: impl FnOnce(&mut Segment),
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
    mutate(&mut after);

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

/// The edit that sets a segment's crop, or clears it.
///
/// `None` and the full-frame rectangle both store as `None`: "not cropped" has
/// exactly one spelling in the document, so resetting a crop returns the file
/// to the bytes it had before the crop existed.
pub fn set_crop_command(
    project: &Project,
    segment_id: &str,
    crop: Option<Crop>,
) -> Result<EditCommand, String> {
    let crop = normalize_crop(crop)?;

    let (_, current) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if crop_key(&current.crop) == crop_key(&crop) {
        return Err("the clip is already cropped exactly like that".into());
    }

    replace_segment(project, segment_id, "Crop clip", |segment| {
        segment.crop = crop;
    })
}

/// Refuse the crops that cannot mean anything, and give "uncropped" its one
/// canonical spelling.
fn normalize_crop(crop: Option<Crop>) -> Result<Option<Crop>, String> {
    let Some(crop) = crop else {
        return Ok(None);
    };
    if let Some(field) = crop.non_finite_field() {
        return Err(format!("{field} must be a finite number"));
    }
    if !(0.0..=1.0).contains(&crop.left)
        || !(0.0..=1.0).contains(&crop.top)
        || !(0.0..=1.0).contains(&crop.right)
        || !(0.0..=1.0).contains(&crop.bottom)
    {
        return Err("a crop is given as fractions of the picture, between 0 and 1".into());
    }
    // The renderer would draw nothing for an empty rectangle (`layout::crop_uv`
    // returns `None`), which from the panel would read as the clip having been
    // deleted. Refusing is kinder than a mystery.
    if crop.right <= crop.left || crop.bottom <= crop.top {
        return Err("the crop would keep none of the picture".into());
    }
    if crop.left == 0.0 && crop.top == 0.0 && crop.right == 1.0 && crop.bottom == 1.0 {
        return Ok(None);
    }
    Ok(Some(crop))
}

/// A crop as comparable bits. `f32` is not `Eq` and two NaNs never compare
/// equal, but NaNs were rejected before this is called.
fn crop_key(crop: &Option<Crop>) -> Option<[u32; 4]> {
    crop.map(|c| {
        [
            c.left.to_bits(),
            c.top.to_bits(),
            c.right.to_bits(),
            c.bottom.to_bits(),
        ]
    })
}

/// The edit that sets a segment's colour adjustment, or clears it.
///
/// Returns the material to add to the pool — `None` when the edit only
/// detaches — alongside the command. The caller pushes the material *before*
/// applying the command, exactly the ordering `text_add` documents: a segment
/// must never, even between two writes, reference a material the pool does not
/// hold.
///
/// An identity `color` is treated as clearing: a grade that changes nothing
/// and no grade at all must be the same document, or "reset" leaves residue
/// behind in every saved file.
pub fn set_color_command(
    project: &Project,
    segment_id: &str,
    color: Option<ColorEdit>,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let (_, current) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;

    let color = color.filter(|c| !c.is_identity());
    if let Some(c) = &color {
        for (name, value) in [
            ("brightness", c.brightness),
            ("contrast", c.contrast),
            ("saturation", c.saturation),
            ("temperature", c.temperature),
        ] {
            if !value.is_finite() {
                return Err(format!("{name} must be a finite number"));
            }
        }
        if let Some(lut) = &c.lut {
            if !lut.intensity.is_finite() {
                return Err("LUT intensity must be a finite number".into());
            }
            if lut.path.trim().is_empty() {
                return Err("a LUT needs a file".into());
            }
        }
    }

    let had_adjust = project.materials.color_adjust_of(current).is_some();
    if color.is_none() && !had_adjust {
        return Err("the clip has no colour adjustment to remove".into());
    }

    let material = color.map(|c| {
        let mut material = ColorAdjustMaterial::identity();
        material.brightness = c.brightness;
        material.contrast = c.contrast;
        material.saturation = c.saturation;
        material.temperature = c.temperature;
        // Intensity is clamped rather than refused: the document's contract
        // is 0..1 and a slider cannot exceed it, so anything outside is a
        // caller rounding artefact, not an intent.
        material.lut = c.lut.map(|lut| LutRef {
            path: lut.path,
            intensity: lut.intensity.clamp(0.0, 1.0),
        });
        material
    });
    let new_id = material.as_ref().map(|m| m.id.clone());

    let materials = &project.materials;
    let command = replace_segment(project, segment_id, "Adjust colour", move |segment| {
        // Every id that resolves as a colour adjustment goes; at most one
        // comes back. This also repairs a malformed segment carrying two.
        segment
            .extras
            .retain(|id| materials.color_adjust(id).is_none());
        if let Some(id) = new_id {
            segment.extras.push(id);
        }
    })?;

    Ok((material, command))
}

// ---------------------------------------------------------------------------
// Paste attributes
// ---------------------------------------------------------------------------

/// Everything "Paste attributes" carries from the copied clip to the selected
/// ones: the look of a clip, none of its timing or identity.
///
/// The grade travels as *values* rather than as a material id, because the
/// clipboard outlives the document it copied from — an id from another project
/// resolves to nothing here. One material is minted per paste and **shared by
/// every target**, which is safe because colour materials are immutable: every
/// committed change mints a fresh material and swaps the reference (see the
/// module docs above), so nothing can later edit one target's grade through the
/// shared block.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClipAttributes {
    pub transform: Transform,
    pub speed: f32,
    pub volume: f32,
    #[serde(default)]
    pub crop: Option<Crop>,
    #[serde(default)]
    pub color: Option<ColorEdit>,
}

/// The edit that applies `attributes` to every clip in `targets`.
///
/// One `replace_segment` per target, all in one `Composite` — one undo step,
/// and undoing it restores every clip byte for byte because both primitives
/// snapshot the whole segment. Link partners are deliberately **not** touched:
/// `History::apply` does not mirror into a composite, so a selection holding
/// both halves of a linked pair applies exactly once to each, and a selection
/// holding one half leaves the other alone — pasting a transform onto a sound
/// clip nobody selected would be the double-application this avoids.
///
/// Returns the minted colour material alongside, `None` when the source had no
/// grade; the caller pushes it into the pool before applying, exactly the
/// `set_color_command` contract.
pub fn paste_attributes_command(
    project: &Project,
    attributes: &ClipAttributes,
    targets: &[String],
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    if let Some(field) = attributes.transform.non_finite_field() {
        return Err(format!("{field} must be a finite number"));
    }
    if !attributes.speed.is_finite() || attributes.speed <= 0.0 {
        return Err("the copied clip's speed is not usable".into());
    }
    if !attributes.volume.is_finite() {
        return Err("volume must be a finite number".into());
    }
    let crop = normalize_crop(attributes.crop)?;

    // The same validation and identity-is-absence rule as `set_color_command`:
    // an identity grade on the source means the targets end up ungraded.
    let color = attributes.color.clone().filter(|c| !c.is_identity());
    if let Some(c) = &color {
        for (name, value) in [
            ("brightness", c.brightness),
            ("contrast", c.contrast),
            ("saturation", c.saturation),
            ("temperature", c.temperature),
        ] {
            if !value.is_finite() {
                return Err(format!("{name} must be a finite number"));
            }
        }
        if let Some(lut) = &c.lut {
            if !lut.intensity.is_finite() {
                return Err("LUT intensity must be a finite number".into());
            }
            if lut.path.trim().is_empty() {
                return Err("a LUT needs a file".into());
            }
        }
    }
    let material = color.map(|c| {
        let mut material = ColorAdjustMaterial::identity();
        material.brightness = c.brightness;
        material.contrast = c.contrast;
        material.saturation = c.saturation;
        material.temperature = c.temperature;
        material.lut = c.lut.map(|lut| LutRef {
            path: lut.path,
            intensity: lut.intensity.clamp(0.0, 1.0),
        });
        material
    });
    let color_id = material.as_ref().map(|m| m.id.clone());

    // Targets in document order, each once, so the composite a given paste
    // expands into is always the same one — a stale selection entry is skipped
    // like `removalCommands` skips it, because the rest of the selection is
    // still there to paste onto.
    let wanted: std::collections::BTreeSet<&str> = targets.iter().map(String::as_str).collect();
    let ordered: Vec<&Segment> = project
        .tracks
        .iter()
        .flat_map(|track| track.segments.iter())
        .filter(|segment| wanted.contains(segment.id.as_str()))
        .collect();
    if ordered.is_empty() {
        return Err("none of those clips are on the timeline any more".into());
    }

    let materials = &project.materials;
    let mut commands = Vec::with_capacity(ordered.len());
    for target in &ordered {
        let color_id = color_id.clone();
        commands.push(replace_segment(
            project,
            &target.id,
            "Paste attributes",
            move |segment| {
                segment.transform = attributes.transform;
                // The speed is the factor between the two ranges, so the source
                // range follows — the clip keeps its place and length on the
                // timeline, exactly what `SetSpeed` does.
                segment.speed = attributes.speed;
                segment.source_range = TimeRange::new(
                    segment.source_range.start,
                    source_duration_for(segment.target_range.duration, attributes.speed),
                );
                segment.volume = attributes.volume.clamp(0.0, 4.0);
                segment.crop = crop;
                segment
                    .extras
                    .retain(|id| materials.color_adjust(id).is_none());
                if let Some(id) = color_id {
                    segment.extras.push(id);
                }
            },
        )?);
    }

    let command = if commands.len() == 1 {
        commands.into_iter().next().expect("one command")
    } else {
        EditCommand::Composite {
            label: "Paste attributes".into(),
            commands,
        }
    };
    Ok((material, command))
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/// The edit that names a clip, or clears its name.
///
/// A segment has no name field, and growing one would break every
/// `Segment { .. }` literal in the tree — including fixtures owned by other
/// modules — for something only the timeline's label reads. So the name lives
/// where every other segment-scoped parameter block lives: an entry in
/// `MaterialPool::extras` shaped `{"clip_name": …}`, referenced from
/// `Segment::extras`, resolved by the webview's `segmentLabel`. Rust never
/// reads it back — the label is presentation.
///
/// Same mint-and-swap contract as colour: the returned `(id, value)` goes into
/// `MaterialPool::extras` *before* the command applies, the undoable edit is
/// the reference swinging over, and a superseded entry stays in the pool inert.
pub fn rename_clip_command(
    project: &Project,
    segment_id: &str,
    name: Option<String>,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (_, current) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;

    // An all-whitespace name is a request to go back to the derived label,
    // which keeps "unnamed" at one spelling in the document.
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    let existing: Vec<String> = current
        .extras
        .iter()
        .filter(|id| clip_name_entry(project, id).is_some())
        .cloned()
        .collect();
    if name.is_none() && existing.is_empty() {
        return Err("the clip has no name to clear".into());
    }
    if let Some(name) = &name {
        // Renaming to the name it already has would be an undo step that does
        // nothing.
        if existing
            .iter()
            .filter_map(|id| clip_name_entry(project, id))
            .any(|current| current == *name)
        {
            return Err("the clip is already called that".into());
        }
    }

    let entry = name.map(|n| (new_id(), serde_json::json!({ "clip_name": n })));
    let entry_id = entry.as_ref().map(|(id, _)| id.clone());

    let command = replace_segment(project, segment_id, "Rename clip", move |segment| {
        segment.extras.retain(|id| !existing.contains(id));
        if let Some(id) = entry_id {
            segment.extras.push(id);
        }
    })?;
    Ok((entry, command))
}

/// The clip name an extras-pool id resolves to, if it is a name entry at all.
fn clip_name_entry<'a>(project: &'a Project, id: &str) -> Option<&'a str> {
    project
        .materials
        .extras
        .get(id)
        .and_then(|value| value.get("clip_name"))
        .and_then(|name| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, TrackKind};
    use crate::modules::timeline::History;

    fn project_with_clip() -> (Project, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let segment = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        let id = segment.id.clone();
        track.segments.push(segment);
        project.tracks.push(track);
        project
            .materials
            .videos
            .push(crate::modules::project::document::VideoMaterial {
                id: "m".into(),
                path: "/nonexistent/m.mp4".into(),
                width: 640,
                height: 480,
                duration: 10_000_000,
                fps: 30.0,
                has_audio: false,
                rotation: 0,
            });
        (project, id)
    }

    fn crop(left: f32, top: f32, right: f32, bottom: f32) -> Crop {
        Crop {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The whole point of the composite: a crop is one undo step, and undoing
    /// it restores the document byte for byte.
    #[test]
    fn a_crop_applies_and_undoes_exactly() {
        let (mut project, segment_id) = project_with_clip();
        let pristine = serde_json::to_string(&project).unwrap();
        let mut history = History::new();

        let command =
            set_crop_command(&project, &segment_id, Some(crop(0.25, 0.0, 0.75, 1.0))).unwrap();
        history.apply(&mut project, command).unwrap();

        let (_, segment) = project.segment(&segment_id).unwrap();
        let applied = segment.crop.expect("crop landed");
        assert_eq!(applied.left, 0.25);
        assert_eq!(applied.right, 0.75);
        assert_eq!(history.undo_label().as_deref(), Some("Crop clip"));

        history.undo(&mut project).unwrap();
        assert_eq!(serde_json::to_string(&project).unwrap(), pristine);

        history.redo(&mut project).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(segment.crop.is_some());
    }

    /// "Uncropped" has one spelling. Sending the full frame stores `None`, so
    /// reset really does return the document to its pre-crop bytes.
    #[test]
    fn a_full_frame_crop_stores_as_none() {
        let (mut project, segment_id) = project_with_clip();
        let mut history = History::new();
        let command =
            set_crop_command(&project, &segment_id, Some(crop(0.1, 0.1, 0.9, 0.9))).unwrap();
        history.apply(&mut project, command).unwrap();

        let command =
            set_crop_command(&project, &segment_id, Some(crop(0.0, 0.0, 1.0, 1.0))).unwrap();
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(segment.crop.is_none());
    }

    #[test]
    fn empty_and_out_of_range_crops_are_refused() {
        let (project, segment_id) = project_with_clip();
        assert!(set_crop_command(&project, &segment_id, Some(crop(0.8, 0.0, 0.2, 1.0))).is_err());
        assert!(set_crop_command(&project, &segment_id, Some(crop(0.5, 0.0, 0.5, 1.0))).is_err());
        assert!(set_crop_command(&project, &segment_id, Some(crop(-0.5, 0.0, 1.0, 1.0))).is_err());
        assert!(
            set_crop_command(&project, &segment_id, Some(crop(f32::NAN, 0.0, 1.0, 1.0))).is_err()
        );
        // And a no-op is refused rather than becoming an undo step that does
        // nothing.
        assert!(set_crop_command(&project, &segment_id, None).is_err());
    }

    /// Crop must not spread to link partners. `History::apply` mirrors bare
    /// commands onto linked clips but deliberately not composites; this pins
    /// that the composite path really is taken.
    #[test]
    fn cropping_a_linked_clip_leaves_its_partner_alone() {
        let (mut project, segment_id) = project_with_clip();
        // A linked partner on an audio lane, the shape importing produces.
        let mut audio_lane = Track::new(TrackKind::Audio, "A1");
        let partner = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["group-1".into()],
            keyframes: Vec::new(),
        };
        let partner_id = partner.id.clone();
        audio_lane.segments.push(partner);
        project.tracks.push(audio_lane);
        project.materials.links.insert("group-1".into());
        project
            .segment_mut(&segment_id)
            .unwrap()
            .extras
            .push("group-1".into());

        let mut history = History::new();
        let command =
            set_crop_command(&project, &segment_id, Some(crop(0.25, 0.0, 0.75, 1.0))).unwrap();
        history.apply(&mut project, command).unwrap();

        let (_, partner) = project.segment(&partner_id).unwrap();
        assert!(partner.crop.is_none(), "the partner was not cropped");
        let (_, cropped) = project.segment(&segment_id).unwrap();
        assert!(cropped.crop.is_some());
        assert!(
            cropped.extras.contains(&"group-1".to_string()),
            "the link survived the crop"
        );
    }

    #[test]
    fn a_colour_edit_attaches_swaps_and_detaches_with_exact_undo() {
        let (mut project, segment_id) = project_with_clip();
        let pristine_segments = serde_json::to_string(&project.tracks).unwrap();
        let mut history = History::new();

        let warm = ColorEdit {
            brightness: 0.1,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.4,
            lut: None,
        };
        let (material, command) =
            set_color_command(&project, &segment_id, Some(warm.clone())).unwrap();
        let first_id = material.as_ref().unwrap().id.clone();
        project.materials.color_adjusts.push(material.unwrap());
        history.apply(&mut project, command).unwrap();
        assert_eq!(history.undo_label().as_deref(), Some("Adjust colour"));

        let (_, segment) = project.segment(&segment_id).unwrap();
        let resolved = project.materials.color_adjust_of(segment).unwrap();
        assert_eq!(resolved.id, first_id);
        assert_eq!(resolved.temperature, 0.4);

        // A second commit swaps the reference to a fresh material.
        let cool = ColorEdit {
            temperature: -0.4,
            ..warm
        };
        let (material, command) = set_color_command(&project, &segment_id, Some(cool)).unwrap();
        project.materials.color_adjusts.push(material.unwrap());
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        let resolved = project.materials.color_adjust_of(segment).unwrap();
        assert_ne!(resolved.id, first_id);
        assert_eq!(resolved.temperature, -0.4);
        assert_eq!(
            segment
                .extras
                .iter()
                .filter(|id| project.materials.color_adjust(id).is_some())
                .count(),
            1,
            "one grade at a time"
        );

        // Undo swings back to the first material, which is still in the pool.
        history.undo(&mut project).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert_eq!(
            project.materials.color_adjust_of(segment).unwrap().id,
            first_id
        );

        // Detach, then undo everything: the segments are back to pristine.
        // The pool keeps the superseded materials — inert on purpose, like an
        // emptied link group.
        history.redo(&mut project).unwrap();
        let (material, command) = set_color_command(&project, &segment_id, None).unwrap();
        assert!(material.is_none(), "a detach mints nothing");
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(project.materials.color_adjust_of(segment).is_none());

        while history.can_undo() {
            history.undo(&mut project).unwrap();
        }
        assert_eq!(
            serde_json::to_string(&project.tracks).unwrap(),
            pristine_segments
        );
    }

    /// An identity grade is a request to remove the grade: "no adjustment" has
    /// one spelling in the document, mirroring the full-frame crop above.
    #[test]
    fn an_identity_colour_edit_clears_rather_than_attaches() {
        let (mut project, segment_id) = project_with_clip();
        let identity = ColorEdit {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: None,
        };
        // With nothing attached, "set to identity" is a no-op and refused.
        assert!(set_color_command(&project, &segment_id, Some(identity.clone())).is_err());

        let (material, command) = set_color_command(
            &project,
            &segment_id,
            Some(ColorEdit {
                brightness: 0.3,
                ..identity.clone()
            }),
        )
        .unwrap();
        project.materials.color_adjusts.push(material.unwrap());
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        let (material, command) = set_color_command(&project, &segment_id, Some(identity)).unwrap();
        assert!(material.is_none());
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(project.materials.color_adjust_of(segment).is_none());
    }

    /// A LUT with identity scalars is a real grade: it attaches, it swaps,
    /// and stripping the LUT with the scalars still at rest clears the whole
    /// material — the identity-is-absence rule applied to the material, not
    /// to each half.
    #[test]
    fn a_lut_alone_attaches_and_identity_without_it_clears() {
        let (mut project, segment_id) = project_with_clip();
        let mut history = History::new();
        let looked = ColorEdit {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: Some(LutRef {
                path: "/looks/warm.cube".into(),
                intensity: 0.8,
            }),
        };

        let (material, command) =
            set_color_command(&project, &segment_id, Some(looked.clone())).unwrap();
        let material = material.expect("a LUT-only grade is a grade");
        assert_eq!(
            material.lut.as_ref().map(|l| l.path.as_str()),
            Some("/looks/warm.cube")
        );
        assert!(material.scalars_are_identity());
        project.materials.color_adjusts.push(material);
        history.apply(&mut project, command).unwrap();

        // Dropping the LUT with everything else at rest clears the material
        // entirely rather than leaving an identity husk attached.
        let (material, command) = set_color_command(
            &project,
            &segment_id,
            Some(ColorEdit {
                lut: None,
                ..looked
            }),
        )
        .unwrap();
        assert!(material.is_none());
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(project.materials.color_adjust_of(segment).is_none());
    }

    /// Intensity is clamped into the document's 0..1 contract, and a
    /// non-finite intensity or an empty path is refused outright.
    #[test]
    fn lut_intensity_is_clamped_and_nonsense_luts_are_refused() {
        let (project, segment_id) = project_with_clip();
        let base = ColorEdit {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: Some(LutRef {
                path: "/looks/warm.cube".into(),
                intensity: 7.0,
            }),
        };

        let (material, _) = set_color_command(&project, &segment_id, Some(base.clone())).unwrap();
        assert_eq!(material.unwrap().lut.unwrap().intensity, 1.0);

        let mut nan = base.clone();
        nan.lut.as_mut().unwrap().intensity = f32::NAN;
        assert!(set_color_command(&project, &segment_id, Some(nan)).is_err());

        let mut pathless = base;
        pathless.lut.as_mut().unwrap().path = "   ".into();
        assert!(set_color_command(&project, &segment_id, Some(pathless)).is_err());
    }

    #[test]
    fn non_finite_colour_values_are_refused() {
        let (project, segment_id) = project_with_clip();
        let bad = ColorEdit {
            brightness: f32::NAN,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: None,
        };
        assert!(set_color_command(&project, &segment_id, Some(bad)).is_err());
    }

    // -----------------------------------------------------------------------
    // Paste attributes
    // -----------------------------------------------------------------------

    fn attributes() -> ClipAttributes {
        ClipAttributes {
            transform: Transform {
                position: [0.2, -0.1],
                scale: [0.5, 0.5],
                rotation: 15.0,
                opacity: 0.8,
                flip_h: true,
                flip_v: false,
            },
            speed: 2.0,
            volume: 0.4,
            crop: Some(Crop {
                left: 0.1,
                top: 0.0,
                right: 0.9,
                bottom: 1.0,
            }),
            color: Some(ColorEdit {
                brightness: 0.1,
                contrast: 1.2,
                saturation: 0.9,
                temperature: 0.3,
                lut: None,
            }),
        }
    }

    /// The linked pair the importer produces, with both halves selected: each
    /// half takes the attributes exactly once, one undo restores both exactly,
    /// and the two share one immutable grade material.
    #[test]
    fn pasting_attributes_onto_a_linked_pair_applies_once_per_clip() {
        let (mut project, video_id) = project_with_clip();
        let mut audio_lane = Track::new(TrackKind::Audio, "A1");
        let partner = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index: 1,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["group-1".into()],
            keyframes: Vec::new(),
        };
        let audio_id = partner.id.clone();
        audio_lane.segments.push(partner);
        project.tracks.push(audio_lane);
        project.materials.links.insert("group-1".into());
        project
            .segment_mut(&video_id)
            .unwrap()
            .extras
            .push("group-1".into());
        let pristine = serde_json::to_string(&project).unwrap();

        let (material, command) = paste_attributes_command(
            &project,
            &attributes(),
            &[video_id.clone(), audio_id.clone()],
        )
        .unwrap();
        let grade_id = material.as_ref().unwrap().id.clone();
        project.materials.color_adjusts.push(material.unwrap());
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();
        assert_eq!(history.undo_label().as_deref(), Some("Paste attributes"));

        for id in [&video_id, &audio_id] {
            let (_, segment) = project.segment(id).unwrap();
            assert_eq!(segment.speed, 2.0, "applied once, not compounded");
            assert_eq!(segment.volume, 0.4);
            assert_eq!(segment.transform.rotation, 15.0);
            assert_eq!(segment.crop.unwrap().left, 0.1);
            // The clip keeps its place and length; the source range follows the
            // speed, exactly the `SetSpeed` rule.
            assert_eq!(segment.target_range, TimeRange::new(0, 4_000_000));
            assert_eq!(segment.source_range, TimeRange::new(0, 8_000_000));
            assert!(
                segment.extras.contains(&"group-1".to_string()),
                "the link survived the paste"
            );
            assert_eq!(
                project.materials.color_adjust_of(segment).unwrap().id,
                grade_id,
                "both halves reference the one shared grade material"
            );
        }

        // One undo puts both clips back byte for byte; the minted material
        // stays in the pool, inert like every superseded grade.
        history.undo(&mut project).unwrap();
        assert!(!history.can_undo(), "one gesture, one entry");
        project.materials.color_adjusts.clear();
        assert_eq!(serde_json::to_string(&project).unwrap(), pristine);
    }

    /// Pasting onto one half of a pair must leave the other half alone: the
    /// composite is deliberately outside the link mirroring, and pasting a
    /// transform onto a sound clip nobody selected would be a double
    /// application by another name.
    #[test]
    fn pasting_attributes_onto_one_half_leaves_the_partner_alone() {
        let (mut project, video_id) = project_with_clip();
        let mut audio_lane = Track::new(TrackKind::Audio, "A1");
        let partner = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index: 1,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["group-1".into()],
            keyframes: Vec::new(),
        };
        let audio_id = partner.id.clone();
        audio_lane.segments.push(partner);
        project.tracks.push(audio_lane);
        project.materials.links.insert("group-1".into());
        project
            .segment_mut(&video_id)
            .unwrap()
            .extras
            .push("group-1".into());

        let (material, command) =
            paste_attributes_command(&project, &attributes(), &[video_id.clone()]).unwrap();
        project.materials.color_adjusts.push(material.unwrap());
        History::new().apply(&mut project, command).unwrap();

        let (_, partner) = project.segment(&audio_id).unwrap();
        assert_eq!(partner.speed, 1.0);
        assert_eq!(partner.volume, 1.0);
        assert!(partner.crop.is_none());
        let (_, pasted) = project.segment(&video_id).unwrap();
        assert_eq!(pasted.speed, 2.0);
    }

    #[test]
    fn pasting_attributes_with_no_grade_clears_the_targets_grade() {
        let (mut project, segment_id) = project_with_clip();
        let (material, command) = set_color_command(
            &project,
            &segment_id,
            Some(ColorEdit {
                brightness: 0.3,
                contrast: 1.0,
                saturation: 1.0,
                temperature: 0.0,
                lut: None,
            }),
        )
        .unwrap();
        project.materials.color_adjusts.push(material.unwrap());
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        let mut plain = attributes();
        plain.color = None;
        let (material, command) =
            paste_attributes_command(&project, &plain, &[segment_id.clone()]).unwrap();
        assert!(material.is_none(), "no grade on the source mints nothing");
        history.apply(&mut project, command).unwrap();

        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(
            project.materials.color_adjust_of(segment).is_none(),
            "the target looks like the source: ungraded"
        );
    }

    #[test]
    fn pasting_attributes_refuses_nonsense_and_stale_targets() {
        let (project, segment_id) = project_with_clip();

        let mut bad = attributes();
        bad.speed = f32::NAN;
        assert!(paste_attributes_command(&project, &bad, &[segment_id.clone()]).is_err());

        let mut bad = attributes();
        bad.volume = f32::INFINITY;
        assert!(paste_attributes_command(&project, &bad, &[segment_id.clone()]).is_err());

        assert!(
            paste_attributes_command(&project, &attributes(), &["gone".into()]).is_err(),
            "a selection of clips that no longer exist has nothing to paste onto"
        );
    }

    // -----------------------------------------------------------------------
    // Rename
    // -----------------------------------------------------------------------

    #[test]
    fn renaming_a_clip_attaches_swaps_and_clears_with_exact_undo() {
        let (mut project, segment_id) = project_with_clip();
        let pristine_tracks = serde_json::to_string(&project.tracks).unwrap();
        let mut history = History::new();

        let (entry, command) =
            rename_clip_command(&project, &segment_id, Some("Opening shot".into())).unwrap();
        let (first_id, value) = entry.expect("a name mints an entry");
        assert_eq!(value["clip_name"], "Opening shot");
        project.materials.extras.insert(first_id.clone(), value);
        history.apply(&mut project, command).unwrap();
        assert_eq!(history.undo_label().as_deref(), Some("Rename clip"));

        let (_, segment) = project.segment(&segment_id).unwrap();
        assert_eq!(
            clip_name_entry(&project, &segment.extras[0]),
            Some("Opening shot")
        );

        // Renaming again swaps to a fresh entry; only one name at a time.
        let (entry, command) =
            rename_clip_command(&project, &segment_id, Some("Retake".into())).unwrap();
        let (second_id, value) = entry.unwrap();
        assert_ne!(second_id, first_id);
        project.materials.extras.insert(second_id, value);
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        let names: Vec<&str> = segment
            .extras
            .iter()
            .filter_map(|id| clip_name_entry(&project, id))
            .collect();
        assert_eq!(names, vec!["Retake"]);

        // The same name again is a no-op and refused; whitespace clears.
        assert!(rename_clip_command(&project, &segment_id, Some("Retake".into())).is_err());
        let (entry, command) =
            rename_clip_command(&project, &segment_id, Some("   ".into())).unwrap();
        assert!(entry.is_none(), "clearing mints nothing");
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&segment_id).unwrap();
        assert!(segment
            .extras
            .iter()
            .all(|id| clip_name_entry(&project, id).is_none()));

        // With no name there is nothing to clear.
        assert!(rename_clip_command(&project, &segment_id, None).is_err());

        // Undo everything: the segments are back to pristine; the pool keeps
        // the superseded entries, inert on purpose.
        while history.can_undo() {
            history.undo(&mut project).unwrap();
        }
        assert_eq!(
            serde_json::to_string(&project.tracks).unwrap(),
            pristine_tracks
        );
    }
}
