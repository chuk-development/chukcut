//! How a clip is cut out and laid onto what is beneath it, as document data:
//! shape masks, a chroma key and a blend mode, in one [`CompositingMaterial`].
//!
//! The material sits in the pool category `MaterialPool::compositing` and is
//! referenced from the clip's `Segment::extras`, exactly like a colour
//! adjustment (decision 0007), and is never edited in place: an edit mints a
//! new material and swaps the reference. `docs/decisions/0020-masks-keys-and-
//! blend-modes.md` says why the three live on one material and how each one
//! reaches the picture.
//!
//! ## Units
//!
//! Everything a mask stores is relative to the clip as it is drawn, so a mask
//! moves, scales and turns with its clip and means the same thing in a small
//! preview and a 4K export:
//!
//! - `x`, `y`: the mask's centre, from the clip's centre, as a fraction of the
//!   clip's width and height. `0.5` is the right (or top) edge; `+y` is up,
//!   the convention `Transform::position` uses.
//! - `width`, `height`, `feather`: fractions of the clip's **shorter** side, so
//!   a circle with equal width and height is round on any clip.
//! - `rotation`: degrees, clockwise, like `Transform::rotation`.
//!
//! The key colour is gamma-encoded sRGB, the numbers a colour picker shows.
//!
//! ## Schema safety
//!
//! Shapes, combine operations and blend modes are stored as strings and read
//! through enums with an `Other` variant, so a project written by a later build
//! with a shape or mode this one does not know still opens, renders that part
//! as absent (an unknown mask is skipped, an unknown blend mode draws normally)
//! and saves it back unchanged. Every field at its default is skipped on save
//! and the category is skipped when empty, so a project that never uses any
//! of this saves byte-identical to one written before it existed.
//!
//! ## Time
//!
//! Mask keyframes are in the clip's **source time**, the rule effects follow
//! (decision 0016): a split keeps one continuous animation across the cut.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::document::{new_id, Easing, Id, Keyframe, Micros};
use super::effects::{put_keyframe, sample};

/// How many masks the renderer evaluates per clip. More are kept in the
/// document and shown in the inspector, but only the first this many draw.
pub const MAX_MASKS: usize = 8;

/// The mask parameters that can be keyframed, in inspector order.
pub const MASK_PARAMS: [&str; 7] = [
    "x",
    "y",
    "width",
    "height",
    "rotation",
    "feather",
    "roundness",
];

/// Masks, a chroma key and a blend mode for one clip. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositingMaterial {
    pub id: Id,
    /// Applied in order: each one combines with what the ones before it
    /// left, by its [`MaskOp`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<ChromaKey>,
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend: BlendMode,
    /// "Remove background": a person matte made by a model, baked per source
    /// frame into the cache (`modules/matting`) and applied as alpha like a
    /// mask. `None` when off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundRemoval>,
    /// Draw the clip's matte (its alpha as grey) instead of its picture.
    /// Runtime only: the app sets it on the copy it hands the preview while
    /// "Show matte" is held, so it is never saved, never undone and never
    /// reaches an export.
    #[serde(skip)]
    pub view_matte: bool,
}

impl Default for CompositingMaterial {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositingMaterial {
    /// An empty material with a fresh id.
    pub fn new() -> Self {
        Self {
            id: new_id(),
            masks: Vec::new(),
            key: None,
            blend: BlendMode::Normal,
            background: None,
            view_matte: false,
        }
    }

    /// Whether applying this would change nothing: no mask that draws, no
    /// key that keys, normal blending. Such a material is not stored at all.
    pub fn is_identity(&self) -> bool {
        self.masks.is_empty()
            && self.key.is_none()
            && self.blend.is_normal()
            && self.background.is_none()
    }

    pub fn mask(&self, id: &str) -> Option<&Mask> {
        self.masks.iter().find(|m| m.id == id)
    }

    /// The first value on this material that is not a finite number, by
    /// name. A NaN saves as `null` and the project never opens again.
    pub fn non_finite_field(&self) -> Option<String> {
        for mask in &self.masks {
            if let Some(field) = mask.non_finite_field() {
                return Some(format!("mask {field}"));
            }
        }
        if let Some(key) = &self.key {
            if let Some(field) = key.non_finite_field() {
                return Some(format!("chroma key {field}"));
            }
        }
        if let Some(background) = &self.background {
            if let Some(field) = background.invalid_field() {
                return Some(format!("background {field}"));
            }
        }
        None
    }
}

/// "Remove background" on a clip: which model made (or will make) its matte.
///
/// The document records the model and its version, never the matte itself
/// (decision 0019's split: edit data in the document, pixels in the cache).
/// The pixels are a cache keyed by the media file, this model and this
/// version, so a matte made by one version is never shown for another, and an
/// export that finds frames missing rebuilds them with the version recorded
/// here — or refuses, if this build does not have it.
///
/// Three kinds of model fill it (`modules/matting`): people (`rvm`), the
/// main object of the frame (`birefnet-lite`), and an object the user
/// clicked (`mobilesam`, with the clicks in [`prompt`](Self::prompt)).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundRemoval {
    /// The registry id, e.g. `rvm`.
    pub model: String,
    /// The registry version, e.g. `1.0.0-mobilenetv3`.
    pub version: String,
    /// "Select object": where the user clicked, which defines the matte as
    /// much as the model does (it is part of the cache key). `None` for the
    /// automatic models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<ObjectPrompt>,
    /// Remove what the matte keeps and keep the rest: cut an object out of
    /// the picture instead of keeping only it. Applied when drawing, so
    /// flipping it re-bakes nothing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
    /// Whether the matte cuts the clip, which is "Remove background". Off
    /// when the matte is there only to say where the grade or the effects
    /// apply ([`grade`](Self::grade), [`effects`](Self::effects)): the
    /// whole picture shows. Old files, which have no such field, cut.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cut: bool,
    /// Where the clip's colour grade (the Adjust tab: grade, curves, wheels,
    /// HSL, LUT, vignette, grain) applies: the whole clip, only the matte's
    /// subject, or only the rest. The subject is what the matte keeps,
    /// whatever [`invert`](Self::invert) says about the cut.
    #[serde(default, skip_serializing_if = "MatteTarget::is_whole")]
    pub grade: MatteTarget,
    /// Where the clip's effects apply, the same way: "blur only the
    /// background" is effects on [`MatteTarget::Background`].
    #[serde(default, skip_serializing_if = "MatteTarget::is_whole")]
    pub effects: MatteTarget,
}

/// The clicks that select an object, on one frame of the clip's source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectPrompt {
    /// Source time (µs) of the frame the points were placed on. The matte
    /// is propagated from this frame forwards and backwards.
    pub time: i64,
    pub points: Vec<PromptPoint>,
}

/// One click: a point of the source frame (fractions of its width and
/// height, top-left origin, display orientation), on the object to keep or
/// on a part to leave out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromptPoint {
    pub x: f32,
    pub y: f32,
    /// On the object (`true`) or a part to leave out of it.
    pub keep: bool,
}

impl BackgroundRemoval {
    /// Whether the matte does anything a renderer must draw: it cuts, or it
    /// steers the grade or the effects.
    pub fn is_used(&self) -> bool {
        self.cut || !self.grade.is_whole() || !self.effects.is_whole()
    }

    /// The first value that is not a finite fraction, by name.
    pub fn invalid_field(&self) -> Option<String> {
        let prompt = self.prompt.as_ref()?;
        if prompt.points.is_empty() {
            return Some("prompt points (none)".into());
        }
        if !prompt.points.iter().any(|p| p.keep) {
            return Some("prompt points (none on the object)".into());
        }
        prompt
            .points
            .iter()
            .any(|p| {
                !(p.x.is_finite() && p.y.is_finite())
                    || !(0.0..=1.0).contains(&p.x)
                    || !(0.0..=1.0).contains(&p.y)
            })
            .then(|| "prompt point outside the frame".into())
    }
}

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(from = "String", into = "String")]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// A name this build does not know, kept so it saves back as it
            /// was read.
            Other(String),
        }

        impl $name {
            /// Every name this build knows, in menu order.
            pub const NAMES: &'static [&'static str] = &[$($text),*];

            pub fn name(&self) -> &str {
                match self {
                    $($name::$variant => $text,)*
                    $name::Other(name) => name,
                }
            }

            /// Every variant this build knows, in menu order.
            pub fn all() -> Vec<Self> {
                vec![$($name::$variant),*]
            }

            /// The variant called `name`; `Other` when this build has none.
            pub fn parse(name: &str) -> Self {
                match name {
                    $($text => $name::$variant,)*
                    other => $name::Other(other.to_string()),
                }
            }
        }

        impl From<String> for $name {
            fn from(name: String) -> Self {
                Self::parse(&name)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.name().to_string()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

string_enum! {
    /// A mask's shape. Each is drawn as a signed distance in the clip's own
    /// frame; see `render::matte`.
    MaskShape {
        /// One straight edge: the clip shows below it.
        Linear => "linear",
        /// A band between two parallel edges, `height` apart.
        Mirror => "mirror",
        Ellipse => "ellipse",
        /// A rectangle whose corners round off with `roundness`.
        Rectangle => "rectangle",
        /// A five-pointed star.
        Star => "star",
        Heart => "heart",
    }
}

string_enum! {
    /// How a mask combines with the masks before it.
    #[derive(Default)]
    MaskOp {
        /// Union: the clip shows where this mask or an earlier one does.
        #[default]
        Add => "add",
        /// The clip is cut away where this mask covers.
        Subtract => "subtract",
        /// The clip shows only where this mask and the earlier ones overlap.
        Intersect => "intersect",
    }
}

impl MaskOp {
    fn is_add(&self) -> bool {
        *self == MaskOp::Add
    }
}

string_enum! {
    /// Which part of a clip a grade or its effects apply to, by the clip's
    /// matte ([`BackgroundRemoval`]).
    #[derive(Default)]
    MatteTarget {
        /// The whole clip, as without a matte.
        #[default]
        Whole => "whole",
        /// Only what the matte keeps: the person, the selected object.
        Subject => "subject",
        /// Only the rest.
        Background => "background",
    }
}

impl MatteTarget {
    pub fn is_whole(&self) -> bool {
        *self == MatteTarget::Whole
    }

    /// The name a menu shows.
    pub fn label(&self) -> &str {
        match self {
            MatteTarget::Whole => "Whole clip",
            MatteTarget::Subject => "Subject",
            MatteTarget::Background => "Background",
            MatteTarget::Other(name) => name,
        }
    }
}

/// One shape mask on a clip. See the module docs for the units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// Stable within the clip's material, so the inspector and the CLI can
    /// name one mask across edits (an edit mints a new material, not new
    /// masks).
    pub id: Id,
    pub shape: MaskShape,
    #[serde(default, skip_serializing_if = "MaskOp::is_add")]
    pub op: MaskOp,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub x: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub y: f32,
    #[serde(default = "half")]
    pub width: f32,
    #[serde(default = "half")]
    pub height: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f32,
    /// The width of the soft edge, `0..1` of the clip's shorter side times
    /// [`FEATHER_SPAN`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub feather: f32,
    /// Rectangle corner radius, `0..1` of the smaller half-side.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roundness: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
    /// A switched-off mask keeps its values and draws as if absent.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Animated parameters ([`MASK_PARAMS`]): keyframes in the clip's source
    /// time, sorted. A parameter with keyframes ignores its static value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
}

/// How much of the shorter side a feather of 1 softens over.
pub const FEATHER_SPAN: f32 = 0.5;

fn half() -> f32 {
    0.5
}

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// A mask's parameters resolved for one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskPose {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub feather: f32,
    pub roundness: f32,
}

impl Mask {
    /// A mask of `shape` at the centre of the clip, at the size that shape
    /// starts at in CapCut-like editors.
    pub fn new(shape: MaskShape) -> Self {
        let (width, height) = match shape {
            MaskShape::Rectangle => (0.6, 0.4),
            MaskShape::Mirror => (1.0, 0.3),
            _ => (0.5, 0.5),
        };
        Self {
            id: new_id(),
            shape,
            op: MaskOp::Add,
            x: 0.0,
            y: 0.0,
            width,
            height,
            rotation: 0.0,
            feather: 0.0,
            roundness: 0.0,
            invert: false,
            enabled: true,
            keyframes: BTreeMap::new(),
        }
    }

    /// The static value of a parameter, ignoring keyframes.
    pub fn stored(&self, param: &str) -> Option<f32> {
        Some(match param {
            "x" => self.x,
            "y" => self.y,
            "width" => self.width,
            "height" => self.height,
            "rotation" => self.rotation,
            "feather" => self.feather,
            "roundness" => self.roundness,
            _ => return None,
        })
    }

    /// Set the static value of a parameter. `false` for an unknown name.
    pub fn set_stored(&mut self, param: &str, value: f32) -> bool {
        let slot = match param {
            "x" => &mut self.x,
            "y" => &mut self.y,
            "width" => &mut self.width,
            "height" => &mut self.height,
            "rotation" => &mut self.rotation,
            "feather" => &mut self.feather,
            "roundness" => &mut self.roundness,
            _ => return false,
        };
        *slot = clamp_param(param, value);
        true
    }

    /// A parameter at `source_time`: its keyframes when it has any, else its
    /// stored value.
    pub fn value_at(&self, param: &str, source_time: Micros) -> f32 {
        if let Some(keys) = self.keyframes.get(param).filter(|k| !k.is_empty()) {
            return clamp_param(param, sample(keys, source_time));
        }
        self.stored(param).unwrap_or(0.0)
    }

    /// Every parameter at `source_time`.
    pub fn pose_at(&self, source_time: Micros) -> MaskPose {
        MaskPose {
            x: self.value_at("x", source_time),
            y: self.value_at("y", source_time),
            width: self.value_at("width", source_time),
            height: self.value_at("height", source_time),
            rotation: self.value_at("rotation", source_time),
            feather: self.value_at("feather", source_time),
            roundness: self.value_at("roundness", source_time),
        }
    }

    /// Whether `param` is animated.
    pub fn is_animated(&self, param: &str) -> bool {
        self.keyframes.get(param).is_some_and(|k| !k.is_empty())
    }

    /// Set `param` at `source_time` as a keyframe (creating or replacing the
    /// one there).
    pub fn put_keyframe(&mut self, param: &str, source_time: Micros, value: f32) {
        let value = clamp_param(param, value);
        let keys = self.keyframes.entry(param.to_string()).or_default();
        put_keyframe(keys, source_time, value, Easing::Linear);
    }

    /// Bring every value into its documented range.
    pub fn normalized(mut self) -> Self {
        for param in MASK_PARAMS {
            if let Some(v) = self.stored(param) {
                self.set_stored(param, v);
            }
        }
        for (param, keys) in self.keyframes.iter_mut() {
            for key in keys.iter_mut() {
                key.value = clamp_param(param, key.value);
            }
            keys.sort_by_key(|k| k.time);
            keys.dedup_by_key(|k| k.time);
        }
        self.keyframes.retain(|_, keys| !keys.is_empty());
        self
    }

    pub fn non_finite_field(&self) -> Option<String> {
        for param in MASK_PARAMS {
            if !self.stored(param).unwrap_or(0.0).is_finite() {
                return Some(param.to_string());
            }
        }
        for (name, keys) in &self.keyframes {
            if keys.iter().any(|k| !k.value.is_finite()) {
                return Some(format!("{name} keyframe"));
            }
        }
        None
    }
}

/// The documented range of each mask parameter. Clamped rather than refused:
/// a drag past the edge is an intent to go as far as possible.
pub fn clamp_param(param: &str, value: f32) -> f32 {
    if !value.is_finite() {
        return value;
    }
    match param {
        "x" | "y" => value.clamp(-2.0, 2.0),
        "width" | "height" => value.clamp(0.005, 4.0),
        "rotation" => value.clamp(-3600.0, 3600.0),
        "feather" | "roundness" => value.clamp(0.0, 1.0),
        _ => value,
    }
}

// ---------------------------------------------------------------------------
// Chroma key
// ---------------------------------------------------------------------------

/// A colour key ("green screen"). Distances are measured in the CbCr plane of
/// the gamma-encoded picture, before the clip's colour grade: the key sees the
/// footage as shot. See `render::matte` for the arithmetic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromaKey {
    /// The colour taken out, gamma-encoded sRGB `0..1`.
    pub color: [f32; 3],
    /// How far from the key colour still keys out fully, `0..1`.
    #[serde(default = "default_tolerance")]
    pub tolerance: f32,
    /// How wide the ramp from keyed to kept is beyond the tolerance, `0..1`.
    #[serde(default = "default_softness")]
    pub softness: f32,
    /// How much of the key colour's tint is taken out of what is kept (the
    /// green fringe on hair), `0..1`.
    #[serde(default = "default_spill")]
    pub spill: f32,
    /// How far the matte's edge is pulled in, `0..1` of [`SHRINK_SPAN`] of
    /// the clip's shorter side.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shrink: f32,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// How much of the shorter side an edge shrink of 1 pulls in.
pub const SHRINK_SPAN: f32 = 0.01;

fn default_tolerance() -> f32 {
    0.3
}

fn default_softness() -> f32 {
    0.1
}

fn default_spill() -> f32 {
    0.5
}

impl ChromaKey {
    /// A key on `color` with the default tolerance, softness and spill.
    pub fn new(color: [f32; 3]) -> Self {
        Self {
            color,
            tolerance: default_tolerance(),
            softness: default_softness(),
            spill: default_spill(),
            shrink: 0.0,
            enabled: true,
        }
    }

    /// Bring every value into its documented range.
    pub fn normalized(mut self) -> Self {
        for c in &mut self.color {
            *c = c.clamp(0.0, 1.0);
        }
        self.tolerance = self.tolerance.clamp(0.0, 1.0);
        self.softness = self.softness.clamp(0.0, 1.0);
        self.spill = self.spill.clamp(0.0, 1.0);
        self.shrink = self.shrink.clamp(0.0, 1.0);
        self
    }

    pub fn non_finite_field(&self) -> Option<&'static str> {
        if !self.color.iter().all(|c| c.is_finite()) {
            return Some("colour");
        }
        for (name, v) in [
            ("tolerance", self.tolerance),
            ("softness", self.softness),
            ("spill", self.spill),
            ("shrink", self.shrink),
        ] {
            if !v.is_finite() {
                return Some(name);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Blend modes
// ---------------------------------------------------------------------------

string_enum! {
    /// How a clip's colour meets the layers beneath it. The formulas are the
    /// W3C compositing ones, on gamma-encoded colour; see `render::blend`.
    #[derive(Default)]
    BlendMode {
        #[default]
        Normal => "normal",
        Multiply => "multiply",
        Screen => "screen",
        Overlay => "overlay",
        SoftLight => "soft_light",
        HardLight => "hard_light",
        Darken => "darken",
        Lighten => "lighten",
        ColorDodge => "color_dodge",
        ColorBurn => "color_burn",
        Difference => "difference",
        Exclusion => "exclusion",
        Add => "add",
        Subtract => "subtract",
    }
}

impl BlendMode {
    /// Normal, or a mode this build does not know, which draws as normal.
    pub fn is_normal(&self) -> bool {
        matches!(self, BlendMode::Normal)
    }

    /// The shader's number for this mode; `None` for normal and for modes
    /// this build does not know, both of which take the ordinary draw.
    pub fn code(&self) -> Option<u32> {
        let i = Self::NAMES.iter().position(|n| *n == self.name())?;
        (i > 0).then_some(i as u32)
    }

    /// The name the inspector shows.
    pub fn label(&self) -> &str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft light",
            BlendMode::HardLight => "Hard light",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "Colour dodge",
            BlendMode::ColorBurn => "Colour burn",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Add => "Add",
            BlendMode::Subtract => "Subtract",
            BlendMode::Other(name) => name,
        }
    }
}

impl MaskShape {
    /// The name the inspector shows.
    pub fn label(&self) -> &str {
        match self {
            MaskShape::Linear => "Linear",
            MaskShape::Mirror => "Mirror",
            MaskShape::Ellipse => "Circle",
            MaskShape::Rectangle => "Rectangle",
            MaskShape::Star => "Star",
            MaskShape::Heart => "Heart",
            MaskShape::Other(name) => name,
        }
    }

    /// The shader's number for this shape; `None` for one this build does
    /// not know, which is skipped.
    pub fn code(&self) -> Option<u32> {
        Self::NAMES
            .iter()
            .position(|n| *n == self.name())
            .map(|i| i as u32)
    }
}

impl MaskOp {
    pub fn label(&self) -> &str {
        match self {
            MaskOp::Add => "Add",
            MaskOp::Subtract => "Subtract",
            MaskOp::Intersect => "Intersect",
            MaskOp::Other(name) => name,
        }
    }

    pub fn code(&self) -> Option<u32> {
        Self::NAMES
            .iter()
            .position(|n| *n == self.name())
            .map(|i| i as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resting_mask_saves_only_what_it_needs() {
        let mut mask = Mask::new(MaskShape::Ellipse);
        mask.id = "m".into();
        let json = serde_json::to_string(&mask).unwrap();
        assert_eq!(
            json,
            r#"{"id":"m","shape":"ellipse","width":0.5,"height":0.5}"#
        );
        let back: Mask = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mask);
    }

    #[test]
    fn a_material_round_trips_and_an_empty_one_is_tiny() {
        let mut material = CompositingMaterial::new();
        material.id = "c".into();
        assert_eq!(serde_json::to_string(&material).unwrap(), r#"{"id":"c"}"#);

        let mut mask = Mask::new(MaskShape::Rectangle);
        mask.op = MaskOp::Subtract;
        mask.invert = true;
        mask.put_keyframe("x", 1_000_000, 0.25);
        material.masks.push(mask);
        material.key = Some(ChromaKey::new([0.0, 1.0, 0.0]));
        material.blend = BlendMode::SoftLight;
        let json = serde_json::to_value(&material).unwrap();
        assert_eq!(json["blend"], "soft_light");
        assert_eq!(json["masks"][0]["op"], "subtract");
        let back: CompositingMaterial = serde_json::from_value(json).unwrap();
        assert_eq!(back, material);
    }

    #[test]
    fn unknown_shapes_ops_and_modes_load_and_save_back_unchanged() {
        let json = r#"{"id":"x","blend":"vivid_light","brand_new":1,
            "masks":[{"id":"m","shape":"spiral","op":"xor","width":0.2,"height":0.3}]}"#;
        let material: CompositingMaterial = serde_json::from_str(json).unwrap();
        assert_eq!(material.blend, BlendMode::Other("vivid_light".into()));
        assert_eq!(material.blend.code(), None);
        assert_eq!(material.masks[0].shape.code(), None);
        assert_eq!(material.masks[0].op, MaskOp::Other("xor".into()));
        let out = serde_json::to_value(&material).unwrap();
        assert_eq!(out["blend"], "vivid_light");
        assert_eq!(out["masks"][0]["shape"], "spiral");
        assert_eq!(out["masks"][0]["op"], "xor");
    }

    #[test]
    fn view_matte_is_never_saved() {
        let mut material = CompositingMaterial::new();
        material.view_matte = true;
        let back: CompositingMaterial =
            serde_json::from_str(&serde_json::to_string(&material).unwrap()).unwrap();
        assert!(!back.view_matte);
    }

    #[test]
    fn keyframes_win_and_values_are_clamped() {
        let mut mask = Mask::new(MaskShape::Ellipse);
        mask.put_keyframe("width", 0, 0.2);
        mask.put_keyframe("width", 2_000_000, 0.6);
        assert!((mask.value_at("width", 1_000_000) - 0.4).abs() < 1e-6);
        assert_eq!(mask.value_at("height", 1_000_000), 0.5);
        mask.set_stored("feather", 7.0);
        assert_eq!(mask.feather, 1.0);
        assert!(!mask.set_stored("nonsense", 1.0));
    }

    #[test]
    fn blend_codes_follow_the_menu_order() {
        assert_eq!(BlendMode::Normal.code(), None);
        assert_eq!(BlendMode::Multiply.code(), Some(1));
        assert_eq!(BlendMode::Subtract.code(), Some(13));
        assert_eq!(BlendMode::all().len(), BlendMode::NAMES.len());
    }

    #[test]
    fn a_project_without_compositing_saves_no_key_for_it() {
        use crate::modules::project::document::{CanvasConfig, Project};
        let mut project = Project::new("p", CanvasConfig::default(), 30.0);
        let json = serde_json::to_value(&project).unwrap();
        assert!(json["materials"].get("compositing").is_none());

        let mut material = CompositingMaterial::new();
        material.blend = BlendMode::Screen;
        project.materials.compositing.push(material.clone());
        let text = serde_json::to_string(&project).unwrap();
        let back: Project = serde_json::from_str(&text).unwrap();
        assert_eq!(back.materials.compositing, vec![material]);
    }
}
