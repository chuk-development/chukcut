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
    new_id, source_duration_for, ColorAdjustMaterial, Crop, LutRef, Micros, Project, Segment,
    TimeRange, TrackKind, Transform,
};
use crate::modules::project::grade::{CurveChannel, Grade, Wheel, WheelKind};
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

/// The edit that sets a segment's four original colour sliders and its LUT,
/// or clears its whole grade.
///
/// The extended grade (tone, HSL, curves, wheels — [`Grade`]) is carried
/// over from the clip's current material untouched: a caller that only knows
/// the four sliders, like the filter presets, must not wipe the curves the
/// user drew. `None` still clears everything, the way "no filter" means no
/// grade. See [`set_grade_command`] for the rest of the contract.
pub fn set_color_command(
    project: &Project,
    segment_id: &str,
    color: Option<ColorEdit>,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let (_, current) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let grade = project
        .materials
        .color_adjust_of(current)
        .map(|m| m.grade.clone())
        .unwrap_or_default();
    let edit = color.map(|c| GradeEdit {
        brightness: c.brightness,
        contrast: c.contrast,
        saturation: c.saturation,
        temperature: c.temperature,
        lut: c.lut,
        grade,
    });
    set_grade_command(project, segment_id, edit)
}

/// A clip's whole grade as the panel sends it: the four original sliders,
/// the look, and the extended [`Grade`]. No id, because the material
/// identity is minted here, not chosen by the caller.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GradeEdit {
    #[serde(default)]
    pub brightness: f32,
    #[serde(default = "one")]
    pub contrast: f32,
    #[serde(default = "one")]
    pub saturation: f32,
    #[serde(default)]
    pub temperature: f32,
    #[serde(default)]
    pub lut: Option<LutRef>,
    #[serde(default)]
    pub grade: Grade,
}

fn one() -> f32 {
    1.0
}

impl Default for GradeEdit {
    fn default() -> Self {
        Self::identity()
    }
}

impl GradeEdit {
    /// No grade at all.
    pub fn identity() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: None,
            grade: Grade::default(),
        }
    }

    /// The values of `material`, or the identity for an ungraded clip.
    pub fn of(material: Option<&ColorAdjustMaterial>) -> Self {
        material.map_or_else(Self::identity, |m| Self {
            brightness: m.brightness,
            contrast: m.contrast,
            saturation: m.saturation,
            temperature: m.temperature,
            lut: m.lut.clone(),
            grade: m.grade.clone(),
        })
    }

    /// The values of the grade on `segment_id` as the document holds it now.
    pub fn of_segment(project: &Project, segment_id: &str) -> Result<Self, String> {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        Ok(Self::of(project.materials.color_adjust_of(segment)))
    }

    /// Identity means "nothing at all": every control at rest *and* no look.
    pub fn is_identity(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 1.0
            && self.saturation == 1.0
            && self.temperature == 0.0
            && self.lut.is_none()
            && self.grade.is_identity()
    }

    /// The edit with `section` back at rest and everything else kept.
    pub fn reset(mut self, section: GradeSection) -> Self {
        let rest = Grade::default();
        match section {
            GradeSection::All => return Self::identity(),
            GradeSection::Basic => {
                let keep = (self.grade.hsl, self.grade.curves.clone(), self.grade.wheels);
                self = Self {
                    lut: self.lut,
                    ..Self::identity()
                };
                (self.grade.hsl, self.grade.curves, self.grade.wheels) = keep;
            }
            GradeSection::Lut => self.lut = None,
            GradeSection::Hsl => self.grade.hsl = rest.hsl,
            GradeSection::Curves => self.grade.curves = rest.curves,
            GradeSection::Wheels => self.grade.wheels = rest.wheels,
        }
        self
    }
}

/// One panel section, for "reset this section".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeSection {
    /// Every slider of the Basic tab; the LUT stays.
    Basic,
    Lut,
    Hsl,
    Curves,
    Wheels,
    All,
}

/// Every single-number control of the grading panel, by name.
///
/// This is how a CLI, an MCP client or a slider addresses one control
/// without knowing the shape of [`GradeEdit`]: read the current values, set
/// one control, send the whole edit. Values are in document units (see
/// `project::grade`), not slider positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeControl {
    Brightness,
    Contrast,
    Saturation,
    Temperature,
    LutIntensity,
    Exposure,
    Tint,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Vibrance,
    Sharpen,
    Clarity,
    VignetteAmount,
    VignetteMidpoint,
    VignetteFeather,
    Grain,
    Fade,
    /// One of the eight HSL bands, `0..8` in `HSL_BANDS` order.
    HslHue(u8),
    HslSaturation(u8),
    HslLuminance(u8),
    WheelX(WheelKind),
    WheelY(WheelKind),
    WheelLuma(WheelKind),
}

impl GradeControl {
    /// The control's current value in `edit`.
    pub fn get(self, edit: &GradeEdit) -> f32 {
        let g = &edit.grade;
        let band = |i: u8| g.hsl.bands[(i as usize).min(7)];
        match self {
            Self::Brightness => edit.brightness,
            Self::Contrast => edit.contrast,
            Self::Saturation => edit.saturation,
            Self::Temperature => edit.temperature,
            Self::LutIntensity => edit.lut.as_ref().map_or(1.0, |l| l.intensity),
            Self::Exposure => g.exposure,
            Self::Tint => g.tint,
            Self::Highlights => g.highlights,
            Self::Shadows => g.shadows,
            Self::Whites => g.whites,
            Self::Blacks => g.blacks,
            Self::Vibrance => g.vibrance,
            Self::Sharpen => g.sharpen,
            Self::Clarity => g.clarity,
            Self::VignetteAmount => g.vignette.amount,
            Self::VignetteMidpoint => g.vignette.midpoint,
            Self::VignetteFeather => g.vignette.feather,
            Self::Grain => g.grain,
            Self::Fade => g.fade,
            Self::HslHue(i) => band(i).hue,
            Self::HslSaturation(i) => band(i).saturation,
            Self::HslLuminance(i) => band(i).luminance,
            Self::WheelX(k) => g.wheels.get(k).x,
            Self::WheelY(k) => g.wheels.get(k).y,
            Self::WheelLuma(k) => g.wheels.get(k).luma,
        }
    }

    /// `edit` with this control set to `value`. Range limits are applied
    /// when the edit is committed, not here, so a caller can read back what
    /// it asked for until then.
    pub fn set(self, edit: &mut GradeEdit, value: f32) {
        let g = &mut edit.grade;
        match self {
            Self::Brightness => edit.brightness = value,
            Self::Contrast => edit.contrast = value,
            Self::Saturation => edit.saturation = value,
            Self::Temperature => edit.temperature = value,
            Self::LutIntensity => {
                if let Some(lut) = edit.lut.as_mut() {
                    lut.intensity = value;
                }
            }
            Self::Exposure => g.exposure = value,
            Self::Tint => g.tint = value,
            Self::Highlights => g.highlights = value,
            Self::Shadows => g.shadows = value,
            Self::Whites => g.whites = value,
            Self::Blacks => g.blacks = value,
            Self::Vibrance => g.vibrance = value,
            Self::Sharpen => g.sharpen = value,
            Self::Clarity => g.clarity = value,
            Self::VignetteAmount => g.vignette.amount = value,
            Self::VignetteMidpoint => g.vignette.midpoint = value,
            Self::VignetteFeather => g.vignette.feather = value,
            Self::Grain => g.grain = value,
            Self::Fade => g.fade = value,
            Self::HslHue(i) => g.hsl.bands[(i as usize).min(7)].hue = value,
            Self::HslSaturation(i) => g.hsl.bands[(i as usize).min(7)].saturation = value,
            Self::HslLuminance(i) => g.hsl.bands[(i as usize).min(7)].luminance = value,
            Self::WheelX(k) => g.wheels.get_mut(k).x = value,
            Self::WheelY(k) => g.wheels.get_mut(k).y = value,
            Self::WheelLuma(k) => g.wheels.get_mut(k).luma = value,
        }
    }

    /// The control's value at rest.
    pub fn rest(self) -> f32 {
        Self::get(self, &GradeEdit::identity())
    }
}

/// The edit that sets a segment's whole grade, or clears it.
///
/// Returns the material to add to the pool — `None` when the edit only
/// detaches — alongside the command. The caller pushes the material *before*
/// applying the command, exactly the ordering `text_add` documents: a segment
/// must never, even between two writes, reference a material the pool does not
/// hold.
///
/// An identity edit is treated as clearing: a grade that changes nothing
/// and no grade at all must be the same document, or "reset" leaves residue
/// behind in every saved file. Every value is brought into its documented
/// range on the way in ([`Grade::normalized`]), so the document never holds
/// a value the renderer would have to second-guess.
pub fn set_grade_command(
    project: &Project,
    segment_id: &str,
    edit: Option<GradeEdit>,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let (_, current) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;

    let material = mint_grade(edit)?;
    let had_adjust = project.materials.color_adjust_of(current).is_some();
    if material.is_none() && !had_adjust {
        return Err("the clip has no colour adjustment to remove".into());
    }
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

/// Validate an edit and mint its material; `None` for an identity edit.
fn mint_grade(edit: Option<GradeEdit>) -> Result<Option<ColorAdjustMaterial>, String> {
    let Some(edit) = edit else {
        return Ok(None);
    };
    for (name, value) in [
        ("brightness", edit.brightness),
        ("contrast", edit.contrast),
        ("saturation", edit.saturation),
        ("temperature", edit.temperature),
    ] {
        if !value.is_finite() {
            return Err(format!("{name} must be a finite number"));
        }
    }
    if let Some(lut) = &edit.lut {
        if !lut.intensity.is_finite() {
            return Err("LUT intensity must be a finite number".into());
        }
        if lut.path.trim().is_empty() {
            return Err("a LUT needs a file".into());
        }
    }
    if let Some(name) = edit.grade.non_finite_field() {
        return Err(format!("{name} must be a finite number"));
    }
    let grade = edit.grade.normalized();
    // Intensity is clamped rather than refused: the document's contract
    // is 0..1 and a slider cannot exceed it, so anything outside is a
    // caller rounding artefact, not an intent.
    let lut = edit.lut.map(|lut| LutRef {
        path: lut.path,
        intensity: lut.intensity.clamp(0.0, 1.0),
    });
    let normalized = GradeEdit { lut, grade, ..edit };
    if normalized.is_identity() {
        return Ok(None);
    }
    let mut material = ColorAdjustMaterial::identity();
    material.brightness = normalized.brightness;
    material.contrast = normalized.contrast;
    material.saturation = normalized.saturation;
    material.temperature = normalized.temperature;
    material.lut = normalized.lut;
    material.grade = normalized.grade;
    Ok(Some(material))
}

/// The edit that sets one control of a segment's grade, keeping the rest.
pub fn grade_control_command(
    project: &Project,
    segment_id: &str,
    control: GradeControl,
    value: f32,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let mut edit = GradeEdit::of_segment(project, segment_id)?;
    if control == GradeControl::LutIntensity && edit.lut.is_none() {
        return Err("the clip has no LUT".into());
    }
    control.set(&mut edit, value);
    set_grade_command(project, segment_id, Some(edit))
}

/// The edit that replaces one tone curve's points, keeping the rest.
pub fn curve_command(
    project: &Project,
    segment_id: &str,
    channel: CurveChannel,
    points: Vec<[f32; 2]>,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let mut edit = GradeEdit::of_segment(project, segment_id)?;
    *edit.grade.curves.get_mut(channel) = points;
    set_grade_command(project, segment_id, Some(edit))
}

/// The edit that sets one colour wheel — puck and luma together, which is
/// what one drag of the puck changes.
pub fn wheel_command(
    project: &Project,
    segment_id: &str,
    kind: WheelKind,
    wheel: Wheel,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let mut edit = GradeEdit::of_segment(project, segment_id)?;
    *edit.grade.wheels.get_mut(kind) = wheel;
    set_grade_command(project, segment_id, Some(edit))
}

/// The edit that attaches, swaps or removes the clip's LUT, keeping the
/// rest of the grade.
pub fn lut_command(
    project: &Project,
    segment_id: &str,
    lut: Option<LutRef>,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let mut edit = GradeEdit::of_segment(project, segment_id)?;
    edit.lut = lut;
    set_grade_command(project, segment_id, Some(edit))
}

/// The edit that puts one panel section back at rest.
pub fn reset_grade_command(
    project: &Project,
    segment_id: &str,
    section: GradeSection,
) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String> {
    let edit = GradeEdit::of_segment(project, segment_id)?.reset(section);
    set_grade_command(project, segment_id, Some(edit))
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
    /// The extended grade of the copied clip. Only read alongside `color`:
    /// a clipboard from before it existed pastes the four sliders alone.
    #[serde(default)]
    pub grade: Option<Grade>,
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

    // The same validation and identity-is-absence rule as `set_grade_command`:
    // an identity grade on the source means the targets end up ungraded.
    let material = mint_grade(attributes.color.clone().map(|c| GradeEdit {
        brightness: c.brightness,
        contrast: c.contrast,
        saturation: c.saturation,
        temperature: c.temperature,
        lut: c.lut,
        grade: attributes.grade.clone().unwrap_or_default(),
    }))?;
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

// ---------------------------------------------------------------------------
// Apply a grade to every clip
// ---------------------------------------------------------------------------

/// The Adjust tab's "Apply to all": every other picture clip takes the
/// selected clip's grade, as one undo step.
///
/// No material is minted. Colour materials are immutable — every change mints
/// a fresh one and swaps the reference (see the module docs) — so the other
/// clips can point at the selected clip's own material, and a later change to
/// any one of them swings only that clip away from it. A selected clip with no
/// grade clears everyone's.
pub fn apply_color_to_all_command(
    project: &Project,
    segment_id: &str,
) -> Result<EditCommand, String> {
    let (_, source) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let grade = project
        .materials
        .color_adjust_of(source)
        .map(|m| m.id.clone());

    let pool = &project.materials;
    let is_picture = |segment: &Segment| {
        pool.videos.iter().any(|m| m.id == segment.material_id)
            || pool.images.iter().any(|m| m.id == segment.material_id)
    };
    let mut commands = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            // Only picture lanes: an imported clip's sound sits on an audio
            // lane as a segment of the same *video* material.
            if track.kind != TrackKind::Video || segment.id == segment_id || !is_picture(segment) {
                continue;
            }
            let current = pool.color_adjust_of(segment).map(|m| &m.id);
            if current == grade.as_ref() {
                continue;
            }
            let grade = grade.clone();
            commands.push(replace_segment(
                project,
                &segment.id,
                "Apply grade to all",
                move |segment| {
                    segment.extras.retain(|id| pool.color_adjust(id).is_none());
                    if let Some(id) = grade {
                        segment.extras.push(id);
                    }
                },
            )?);
        }
    }
    if commands.is_empty() {
        return Err("every clip already has this grade".into());
    }
    Ok(EditCommand::Composite {
        label: "Apply grade to all".into(),
        commands,
    })
}

// ---------------------------------------------------------------------------
// Speed
// ---------------------------------------------------------------------------

/// The speed edit the inspector's Speed tab makes: the clip keeps the part of
/// its file it shows and becomes shorter or longer on the timeline.
///
/// `EditCommand::SetSpeed` alone does the other thing — it keeps the clip's
/// place and length and reads more or less of the file — which is right for
/// "paste attributes" but not for a speed slider, where 2x is expected to halve
/// the clip and 0.5x to double it. So this is a `Composite`:
///
/// - every clip linked to the selected one gets the same speed, because a
///   picture at 2x over its own sound at 1x is out of sync from the first
///   frame. `History::apply` mirrors moves and trims onto link partners but
///   not speed, and it does not look inside a `Composite`, so the partners are
///   named here explicitly;
/// - each of those clips is `SetSpeed` and then a `TrimSegment` back to its
///   original source range, which is the new timeline length;
/// - every later clip on each touched lane moves by the change in length, so a
///   gap or a butt cut after the clip stays what it was. Growing moves the
///   later clips first (right to left) so the longer clip has room; shrinking
///   trims first and then pulls the later clips in (left to right).
///
/// One undo step, exactly invertible, built only from existing primitives.
pub fn set_speed_command(
    project: &Project,
    segment_id: &str,
    speed: f32,
) -> Result<EditCommand, String> {
    if !speed.is_finite() || speed <= 0.0 {
        return Err("speed must be a positive number".into());
    }
    let (_, primary) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;

    let members: Vec<(&crate::modules::project::document::Track, &Segment)> =
        match project.link_group_of(segment_id) {
            Some(group) => project
                .link_members(group)
                .into_iter()
                .map(|(track, _, segment)| (track, segment))
                .collect(),
            None => vec![project.segment(segment_id).expect("found above")],
        };
    if members.iter().all(|(_, segment)| segment.speed == speed) {
        return Err("the clip already plays at that speed".into());
    }

    let new_duration = |segment: &Segment| -> Micros {
        ((segment.source_range.duration as f64 / speed as f64).round() as Micros).max(1)
    };
    let delta = new_duration(primary) - primary.target_range.duration;

    let member_ids: std::collections::BTreeSet<&str> =
        members.iter().map(|(_, s)| s.id.as_str()).collect();
    let mut moved: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut moves: Vec<EditCommand> = Vec::new();
    if delta != 0 {
        let mut push_move = |track: &crate::modules::project::document::Track,
                             segment: &Segment| {
            moves.push(EditCommand::MoveSegment {
                segment_id: segment.id.clone(),
                from_track: track.id.clone(),
                to_track: track.id.clone(),
                from_start: segment.target_range.start,
                to_start: segment.target_range.start + delta,
            });
        };
        let mut later: Vec<(&crate::modules::project::document::Track, &Segment)> = Vec::new();
        for (track, member) in &members {
            let end = member.target_range.end();
            for segment in &track.segments {
                if member_ids.contains(segment.id.as_str()) || segment.target_range.start < end {
                    continue;
                }
                if moved.insert(segment.id.as_str()) {
                    later.push((track, segment));
                }
            }
        }
        // A later clip's own sound may sit on a lane this edit does not
        // otherwise touch; it travels with its picture.
        let mut partners = Vec::new();
        for (_, segment) in &later {
            if let Some(group) = project.link_group_of(&segment.id) {
                for (track, _, partner) in project.link_members(group) {
                    if !member_ids.contains(partner.id.as_str())
                        && moved.insert(partner.id.as_str())
                    {
                        partners.push((track, partner));
                    }
                }
            }
        }
        later.extend(partners);
        for (track, segment) in later {
            push_move(track, segment);
        }
        let start_of = |command: &EditCommand| match command {
            EditCommand::MoveSegment { from_start, .. } => *from_start,
            _ => 0,
        };
        if delta > 0 {
            moves.sort_by_key(|c| std::cmp::Reverse(start_of(c)));
        } else {
            moves.sort_by_key(start_of);
        }
    }

    let mut retime = Vec::with_capacity(members.len() * 2);
    for (_, segment) in &members {
        let after_target = TimeRange::new(segment.target_range.start, new_duration(segment));
        // What `SetSpeed` leaves behind, which is what the trim starts from.
        let mid_source = TimeRange::new(
            segment.source_range.start,
            source_duration_for(segment.target_range.duration, speed),
        );
        retime.push(EditCommand::SetSpeed {
            segment_id: segment.id.clone(),
            before: segment.speed,
            after: speed,
        });
        if after_target != segment.target_range {
            retime.push(EditCommand::TrimSegment {
                segment_id: segment.id.clone(),
                before_target: segment.target_range,
                before_source: mid_source,
                after_target,
                after_source: segment.source_range,
            });
        }
    }

    let commands = if delta > 0 {
        moves.into_iter().chain(retime).collect()
    } else {
        retime.into_iter().chain(moves).collect()
    };
    Ok(EditCommand::Composite {
        label: "Change speed".into(),
        commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, TrackKind};
    use crate::modules::timeline::History;

    pub(super) fn project_with_clip() -> (Project, String) {
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

    pub(super) fn attributes() -> ClipAttributes {
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
            grade: None,
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

#[cfg(test)]
mod speed_tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, TrackKind};
    use crate::modules::timeline::History;

    fn clip(material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: new_id(),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Two clips back to back on one video lane, each with its sound linked
    /// on an audio lane.
    pub(super) fn two_linked_clips() -> (Project, [String; 4]) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut video = Track::new(TrackKind::Video, "V1");
        let mut audio = Track::new(TrackKind::Audio, "A1");
        let mut ids = Vec::new();
        for start in [0, 4_000_000] {
            let group = new_id();
            project.materials.links.insert(group.clone());
            let mut v = clip("m", start, 4_000_000);
            let mut a = clip("m", start, 4_000_000);
            // What the engine derives from track order on every edit.
            a.render_index = 1;
            v.extras.push(group.clone());
            a.extras.push(group);
            ids.push(v.id.clone());
            ids.push(a.id.clone());
            video.segments.push(v);
            audio.segments.push(a);
        }
        project.tracks.push(video);
        project.tracks.push(audio);
        let [v1, a1, v2, a2]: [String; 4] = ids.try_into().expect("four clips");
        (project, [v1, a1, v2, a2])
    }

    fn range(project: &Project, id: &str) -> TimeRange {
        project.segment(id).unwrap().1.target_range
    }

    #[test]
    fn slowing_down_lengthens_the_clip_its_sound_and_pushes_what_follows() {
        let (mut project, [v1, a1, v2, a2]) = two_linked_clips();
        let pristine = serde_json::to_string(&project.tracks).unwrap();
        let mut history = History::new();

        let command = set_speed_command(&project, &v1, 0.5).unwrap();
        history.apply(&mut project, command).unwrap();

        for id in [&v1, &a1] {
            let (_, segment) = project.segment(id).unwrap();
            assert_eq!(segment.speed, 0.5);
            assert_eq!(segment.target_range, TimeRange::new(0, 8_000_000));
            assert_eq!(segment.source_range, TimeRange::new(0, 4_000_000));
        }
        assert_eq!(range(&project, &v2).start, 8_000_000);
        assert_eq!(range(&project, &a2).start, 8_000_000);

        history.undo(&mut project).unwrap();
        assert_eq!(serde_json::to_string(&project.tracks).unwrap(), pristine);
    }

    #[test]
    fn speeding_up_shortens_the_clip_and_pulls_what_follows() {
        let (mut project, [v1, a1, v2, a2]) = two_linked_clips();
        let pristine = serde_json::to_string(&project.tracks).unwrap();
        let mut history = History::new();

        let command = set_speed_command(&project, &a1, 2.0).unwrap();
        history.apply(&mut project, command).unwrap();

        assert_eq!(range(&project, &v1), TimeRange::new(0, 2_000_000));
        assert_eq!(range(&project, &a1), TimeRange::new(0, 2_000_000));
        assert_eq!(range(&project, &v2).start, 2_000_000);
        assert_eq!(range(&project, &a2).start, 2_000_000);
        assert!(project.segment(&v1).unwrap().1.speed_invariant_holds());

        history.undo(&mut project).unwrap();
        assert_eq!(serde_json::to_string(&project.tracks).unwrap(), pristine);
    }

    #[test]
    fn awkward_speeds_keep_the_ranges_consistent_and_undo_exactly() {
        let (mut project, [v1, ..]) = two_linked_clips();
        let pristine = serde_json::to_string(&project.tracks).unwrap();
        let mut history = History::new();
        for speed in [3.7_f32, 0.13, 100.0, 0.1] {
            let command = set_speed_command(&project, &v1, speed).unwrap();
            history.apply(&mut project, command).unwrap();
            assert!(project.segment(&v1).unwrap().1.speed_invariant_holds());
        }
        while history.can_undo() {
            history.undo(&mut project).unwrap();
        }
        assert_eq!(serde_json::to_string(&project.tracks).unwrap(), pristine);
    }

    #[test]
    fn the_same_speed_and_nonsense_are_refused() {
        let (project, [v1, ..]) = two_linked_clips();
        assert!(set_speed_command(&project, &v1, 1.0).is_err());
        assert!(set_speed_command(&project, &v1, 0.0).is_err());
        assert!(set_speed_command(&project, &v1, f32::NAN).is_err());
        assert!(set_speed_command(&project, "nope", 2.0).is_err());
    }

    #[test]
    fn a_grade_applied_to_all_is_shared_and_undoes_exactly() {
        let (mut project, [v1, a1, v2, _]) = two_linked_clips();
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
                has_audio: true,
                rotation: 0,
            });
        let mut history = History::new();
        let (material, command) = set_color_command(
            &project,
            &v1,
            Some(ColorEdit {
                brightness: 0.2,
                contrast: 1.0,
                saturation: 1.0,
                temperature: 0.0,
                lut: None,
            }),
        )
        .unwrap();
        let grade = material.unwrap();
        project.materials.color_adjusts.push(grade.clone());
        history.apply(&mut project, command).unwrap();
        let graded = serde_json::to_string(&project.tracks).unwrap();

        let command = apply_color_to_all_command(&project, &v1).unwrap();
        history.apply(&mut project, command).unwrap();
        let of = |project: &Project, id: &str| {
            let (_, segment) = project.segment(id).unwrap();
            project
                .materials
                .color_adjust_of(segment)
                .map(|m| m.id.clone())
        };
        assert_eq!(of(&project, &v2), Some(grade.id.clone()));
        // The sound of a clip is a segment of the same video material on an
        // audio lane; it has no picture and is left alone.
        assert_eq!(of(&project, &a1), None);
        assert!(apply_color_to_all_command(&project, &v1).is_err());

        history.undo(&mut project).unwrap();
        assert_eq!(serde_json::to_string(&project.tracks).unwrap(), graded);
    }
}

#[cfg(test)]
mod grade_tests {
    use super::speed_tests::two_linked_clips;
    use super::tests::{attributes, project_with_clip};
    use super::*;
    use crate::modules::timeline::History;

    // -----------------------------------------------------------------------
    // The extended grade
    // -----------------------------------------------------------------------

    /// Apply a grade builder's result the way the command layer does.
    fn commit(
        project: &mut Project,
        history: &mut History,
        build: impl FnOnce(&Project) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String>,
    ) {
        let (material, command) = build(project).expect("the edit builds");
        if let Some(material) = material {
            project.materials.color_adjusts.push(material);
        }
        history.apply(project, command).expect("the edit applies");
    }

    fn grade_of(project: &Project, id: &str) -> GradeEdit {
        GradeEdit::of_segment(project, id).unwrap()
    }

    /// Every single-number control sets exactly its own field, lands on the
    /// undo stack, and undoes back to the bytes before it — the contract the
    /// panel, a CLI and an MCP client all rely on.
    #[test]
    fn every_grade_control_sets_its_field_and_undoes_exactly() {
        let mut controls = vec![
            GradeControl::Brightness,
            GradeControl::Contrast,
            GradeControl::Saturation,
            GradeControl::Temperature,
            GradeControl::Exposure,
            GradeControl::Tint,
            GradeControl::Highlights,
            GradeControl::Shadows,
            GradeControl::Whites,
            GradeControl::Blacks,
            GradeControl::Vibrance,
            GradeControl::Sharpen,
            GradeControl::Clarity,
            GradeControl::VignetteAmount,
            GradeControl::VignetteMidpoint,
            GradeControl::VignetteFeather,
            GradeControl::Grain,
            GradeControl::Fade,
        ];
        for band in 0..8 {
            controls.push(GradeControl::HslHue(band));
            controls.push(GradeControl::HslSaturation(band));
            controls.push(GradeControl::HslLuminance(band));
        }
        for kind in WheelKind::ALL {
            controls.push(GradeControl::WheelX(kind));
            controls.push(GradeControl::WheelY(kind));
            controls.push(GradeControl::WheelLuma(kind));
        }

        for control in controls {
            let (mut project, id) = project_with_clip();
            let mut history = History::new();
            // Something else graded first, so "keeps the rest" is tested.
            commit(&mut project, &mut history, |p| {
                grade_control_command(p, &id, GradeControl::Fade, 0.25)
            });
            if control == GradeControl::Fade {
                commit(&mut project, &mut history, |p| {
                    grade_control_command(p, &id, GradeControl::Tint, 0.1)
                });
            }
            let before = serde_json::to_string(&project.tracks).unwrap();
            let value = control.rest() + 0.375;
            commit(&mut project, &mut history, |p| {
                grade_control_command(p, &id, control, value)
            });
            let after = grade_of(&project, &id);
            assert!(
                (control.get(&after) - value).abs() < 1e-6,
                "{control:?} reads back {}",
                control.get(&after)
            );
            let mut expected = GradeEdit::identity();
            GradeControl::Fade.set(&mut expected, 0.25);
            if control == GradeControl::Fade {
                GradeControl::Tint.set(&mut expected, 0.1);
            }
            control.set(&mut expected, value);
            assert_eq!(after, expected, "{control:?} touched another field");

            history.undo(&mut project).unwrap();
            assert_eq!(
                serde_json::to_string(&project.tracks).unwrap(),
                before,
                "{control:?} did not undo exactly"
            );
        }
    }

    #[test]
    fn grade_values_are_clamped_and_nonsense_refused() {
        let (project, id) = project_with_clip();
        let (material, _) = grade_control_command(&project, &id, GradeControl::Tint, 7.0).unwrap();
        assert_eq!(material.unwrap().grade.tint, 1.0);
        assert!(grade_control_command(&project, &id, GradeControl::Clarity, f32::NAN).is_err());
        assert!(
            grade_control_command(&project, &id, GradeControl::LutIntensity, 0.5).is_err(),
            "a LUT intensity without a LUT"
        );
        let (material, _) = wheel_command(
            &project,
            &id,
            WheelKind::Gain,
            Wheel {
                x: 2.0,
                y: 0.0,
                luma: 0.0,
            },
        )
        .unwrap();
        assert_eq!(material.unwrap().grade.wheels.gain.x, 1.0);
    }

    /// Curves are stored canonical — sorted, clamped, and an identity curve
    /// as the empty list — so drawing a curve back to the diagonal removes
    /// the whole grade rather than leaving an inert one behind.
    #[test]
    fn curves_store_canonically_and_a_diagonal_clears() {
        let (mut project, id) = project_with_clip();
        let mut history = History::new();
        commit(&mut project, &mut history, |p| {
            curve_command(
                p,
                &id,
                CurveChannel::Red,
                vec![[1.0, 1.0], [0.5, 0.7], [-1.0, 0.0]],
            )
        });
        assert_eq!(
            grade_of(&project, &id).grade.curves.red,
            vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]
        );
        let (material, command) = curve_command(
            &project,
            &id,
            CurveChannel::Red,
            vec![[0.0, 0.0], [1.0, 1.0]],
        )
        .unwrap();
        assert!(material.is_none(), "the identity curve minted a grade");
        history.apply(&mut project, command).unwrap();
        let (_, segment) = project.segment(&id).unwrap();
        assert!(project.materials.color_adjust_of(segment).is_none());
    }

    /// The four-slider path — the filter presets use it — keeps the rest of
    /// the grade, and `None` still clears all of it.
    #[test]
    fn the_four_slider_edit_keeps_the_extended_grade() {
        let (mut project, id) = project_with_clip();
        let mut history = History::new();
        commit(&mut project, &mut history, |p| {
            grade_control_command(p, &id, GradeControl::HslHue(3), 0.5)
        });
        commit(&mut project, &mut history, |p| {
            set_color_command(
                p,
                &id,
                Some(ColorEdit {
                    brightness: 0.1,
                    contrast: 1.0,
                    saturation: 1.0,
                    temperature: 0.0,
                    lut: None,
                }),
            )
        });
        let grade = grade_of(&project, &id);
        assert_eq!(grade.brightness, 0.1);
        assert_eq!(grade.grade.hsl.bands[3].hue, 0.5);

        commit(&mut project, &mut history, |p| {
            set_color_command(p, &id, None)
        });
        assert!(grade_of(&project, &id).is_identity());
    }

    #[test]
    fn resetting_a_section_keeps_the_others() {
        let (mut project, id) = project_with_clip();
        let mut history = History::new();
        let mut edit = GradeEdit::identity();
        edit.brightness = 0.2;
        edit.lut = Some(LutRef {
            path: "/looks/a.cube".into(),
            intensity: 0.5,
        });
        edit.grade.exposure = 0.5;
        edit.grade.hsl.bands[0].saturation = -0.3;
        edit.grade.curves.master = vec![[0.0, 0.1], [1.0, 1.0]];
        edit.grade.wheels.lift.x = 0.2;
        commit(&mut project, &mut history, |p| {
            set_grade_command(p, &id, Some(edit.clone()))
        });

        for section in [
            GradeSection::Basic,
            GradeSection::Lut,
            GradeSection::Hsl,
            GradeSection::Curves,
            GradeSection::Wheels,
        ] {
            let (material, _) = reset_grade_command(&project, &id, section).unwrap();
            let after = GradeEdit::of(material.as_ref());
            let basic_rest = after.brightness == 0.0 && after.grade.exposure == 0.0;
            assert_eq!(basic_rest, section == GradeSection::Basic, "{section:?}");
            assert_eq!(
                after.lut.is_none(),
                section == GradeSection::Lut,
                "{section:?}"
            );
            assert_eq!(
                after.grade.hsl == Default::default(),
                section == GradeSection::Hsl,
                "{section:?}"
            );
            assert_eq!(
                after.grade.curves.is_identity(),
                section == GradeSection::Curves,
                "{section:?}"
            );
            assert_eq!(
                after.grade.wheels == Default::default(),
                section == GradeSection::Wheels,
                "{section:?}"
            );
        }
        let (material, _) = reset_grade_command(&project, &id, GradeSection::All).unwrap();
        assert!(material.is_none());
    }

    /// "Apply to all" shares the whole material, so the extended grade
    /// travels with it; paste carries it as values.
    #[test]
    fn apply_to_all_and_paste_carry_the_extended_grade() {
        let (mut project, [v1, _, v2, _]) = two_linked_clips();
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
                has_audio: true,
                rotation: 0,
            });
        let mut history = History::new();
        commit(&mut project, &mut history, |p| {
            curve_command(p, &v1, CurveChannel::Blue, vec![[0.0, 0.2], [1.0, 1.0]])
        });
        let command = apply_color_to_all_command(&project, &v1).unwrap();
        history.apply(&mut project, command).unwrap();
        assert_eq!(
            grade_of(&project, &v2).grade.curves.blue,
            vec![[0.0, 0.2], [1.0, 1.0]]
        );

        let mut attributes = attributes();
        let mut grade = Grade::default();
        grade.vibrance = 0.4;
        attributes.grade = Some(grade);
        let (material, _) = paste_attributes_command(&project, &attributes, &[v2]).unwrap();
        let material = material.unwrap();
        assert_eq!(material.grade.vibrance, 0.4);
        assert_eq!(material.brightness, 0.1);
    }
}
