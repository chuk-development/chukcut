//! "Remove object" and "Enhance quality": a clip's frames remade by models
//! in the ML worker, baked into the cache and drawn in place of the decoded
//! frames. Decision 0029.
//!
//! - **Remove object** paints over an object in every frame with LaMa
//!   (`ml::inpaint`). What to remove is a mask per frame: an object the user
//!   clicked (MobileSAM + VitTrack, the "Select object" machinery of
//!   `matting::object`, whose mattes it reuses), strokes painted on the
//!   picture, and boxes, all in source fractions. LaMa runs on a crop around
//!   the mask, the fill is blended back with a soft edge, and the result is
//!   steadied from frame to frame ([`removal`]).
//! - **Enhance quality** makes the frame larger and cleaner with
//!   Real-ESRGAN (`ml::upscale`), 2x or 4x, at most [`MAX_LONG_SIDE`] on the
//!   long side.
//!
//! Both on one clip run in that order: the object is removed, then the
//! picture enhanced. The two together are the clip's [`Chain`].
//!
//! ## The document
//!
//! Each setting is a block in `MaterialPool::extras` referenced from the
//! clip's `extras`, the way frame blending is stored (`speed::blend`):
//! `{ "object_removal": { … } }` and `{ "upscale": { … } }`. The edit is a
//! remove + insert of the segment, so undo is exact without a new
//! `EditCommand`; a split copies `extras`, so both halves keep it. The
//! document records the models and their versions and the mask's
//! definition, never pixels.
//!
//! ## The cache
//!
//! One JPEG per source frame ([`cache`]), keyed by the media file's content
//! digest, the chain's signature (models, versions, the mask, the scale),
//! the provider and the size, exactly as mattes and optical-flow frames are
//! keyed. They count towards the cache limit; the open project's are kept by
//! a trim. The compositor draws a baked frame in place of the decoded one
//! (`render::enhance`); a frame not baked yet is drawn as decoded, so the
//! preview never waits. An export bakes what it needs first and fails in
//! words when it cannot ([`jobs::ensure`]).

pub mod bake;
pub mod cache;
pub mod commands;
pub mod jobs;
pub mod mask;
pub mod removal;

use serde::{Deserialize, Serialize};

use crate::modules::project::compositing::ObjectPrompt;
use crate::modules::project::document::{new_id, MaterialKind, Project, Segment};
use crate::modules::project::MaterialPool;
use crate::modules::timeline::ops::EditCommand;

/// The key of the object-removal block inside its `MaterialPool::extras`
/// value.
pub const REMOVAL_KEY: &str = "object_removal";
/// The key of the enhance-quality block.
pub const UPSCALE_KEY: &str = "upscale";

/// The long side, in pixels, a remade frame has at most. A 4K picture: an
/// enhanced 1080p clip becomes 4K, a 4K clip's removed object is painted at
/// its full size.
pub const MAX_LONG_SIDE: u32 = 3840;

/// The long side a clip's source may have for "Enhance quality". Larger
/// footage is sharp enough already and the model's 4x picture of it would
/// not fit in memory sensibly.
pub const MAX_UPSCALE_SOURCE: u32 = 1920;

/// How much the mask grows by default, as a fraction of the picture's
/// shorter side: an object's edge, its blur and a little of its shadow.
pub const DEFAULT_GROW: f32 = 0.012;

/// Changes when the way frames are made changes (the crop, the blend, the
/// smoothing), so frames made the old way are never shown for the new one.
pub const PIPELINE_REVISION: u32 = 1;

fn default_grow() -> f32 {
    DEFAULT_GROW
}

fn is_default_grow(v: &f32) -> bool {
    (*v - DEFAULT_GROW).abs() < 1e-6
}

/// "Remove object" on a clip: what to paint over, and with which model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectRemoval {
    /// The inpainting model's registry id, `lama`.
    pub model: String,
    pub version: String,
    /// An object the user clicked on one frame, carried over the clip by
    /// "Select object" (MobileSAM + VitTrack). Source fractions, display
    /// orientation, as `BackgroundRemoval::prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<ObjectPrompt>,
    /// Strokes painted on the picture: the same place in every frame (a
    /// logo, a watermark, a sign on a locked-off shot).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strokes: Vec<Stroke>,
    /// Boxes, `[x, y, width, height]` in source fractions (top-left origin),
    /// the same place in every frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boxes: Vec<[f32; 4]>,
    /// How far the mask grows past what it covers, as a fraction of the
    /// shorter side ([`DEFAULT_GROW`]).
    #[serde(default = "default_grow", skip_serializing_if = "is_default_grow")]
    pub grow: f32,
}

/// One painted stroke: a line through `points` (source fractions, top-left
/// origin) with round ends, `radius` wide on each side (a fraction of the
/// shorter side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub radius: f32,
}

/// "Enhance quality" on a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upscale {
    /// The super-resolution model's registry id.
    pub model: String,
    pub version: String,
    /// 2 or 4: the size the picture is made at, before [`MAX_LONG_SIDE`].
    pub scale: u32,
}

impl ObjectRemoval {
    /// A removal with this build's model and nothing to remove yet.
    pub fn new() -> Self {
        Self {
            model: crate::modules::ml::inpaint::MODEL.into(),
            version: crate::modules::ml::inpaint::model_version().into(),
            prompt: None,
            strokes: Vec::new(),
            boxes: Vec::new(),
            grow: DEFAULT_GROW,
        }
    }

    /// Whether anything is marked for removal.
    pub fn has_mask(&self) -> bool {
        self.prompt.is_some() || !self.strokes.is_empty() || !self.boxes.is_empty()
    }

    /// Why this setting cannot be stored, in words; `None` when it can.
    pub fn invalid(&self) -> Option<String> {
        if !self.has_mask() {
            return Some(
                "nothing is marked for removal: select the object, paint over it or draw a box"
                    .into(),
            );
        }
        let fraction = |v: f32| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(self.grow.is_finite() && (0.0..=0.2).contains(&self.grow)) {
            return Some("the mask can grow by 0 to 0.2 of the picture".into());
        }
        for stroke in &self.strokes {
            if stroke.points.is_empty() {
                return Some("a painted stroke has no points".into());
            }
            if !(stroke.radius.is_finite() && stroke.radius > 0.0 && stroke.radius <= 0.5) {
                return Some("a brush is wider than 0 and at most half the picture".into());
            }
            if !stroke
                .points
                .iter()
                .all(|p| fraction(p[0]) && fraction(p[1]))
            {
                return Some("a painted stroke leaves the picture".into());
            }
        }
        for b in &self.boxes {
            if !(b.iter().all(|&v| fraction(v)) && b[2] > 0.0 && b[3] > 0.0) {
                return Some("a box must lie inside the picture and not be empty".into());
            }
        }
        if let Some(prompt) = &self.prompt {
            let setting = removal_matte_setting(prompt);
            if let Some(field) = setting.invalid_field() {
                return Some(format!("the selection's {field}"));
            }
        }
        None
    }
}

impl Default for ObjectRemoval {
    fn default() -> Self {
        Self::new()
    }
}

impl Upscale {
    /// "Enhance quality" at `scale` with this build's model.
    pub fn new(scale: u32) -> Result<Self, String> {
        if !matches!(scale, 2 | 4) {
            return Err(format!("enhance quality makes 2x or 4x, not {scale}x"));
        }
        Ok(Self {
            model: crate::modules::ml::upscale::MODEL.into(),
            version: crate::modules::ml::upscale::model_version().into(),
            scale,
        })
    }
}

/// The matte setting whose cache holds a removal's selected object: the
/// "Select object" model on the clicks, uncut. The mattes are shared with a
/// "Remove background › Select" of the same clicks.
pub fn removal_matte_setting(
    prompt: &ObjectPrompt,
) -> crate::modules::project::compositing::BackgroundRemoval {
    use crate::modules::ml::segment;
    crate::modules::project::compositing::BackgroundRemoval {
        model: segment::MODEL.into(),
        version: segment::model_version().into(),
        prompt: Some(prompt.clone()),
        invert: false,
        cut: true,
        grade: Default::default(),
        effects: Default::default(),
    }
}

fn block<T: for<'de> Deserialize<'de>>(materials: &MaterialPool, id: &str, key: &str) -> Option<T> {
    let value = materials.extras.get(id)?.get(key)?;
    serde_json::from_value(value.clone()).ok()
}

fn entry_is(materials: &MaterialPool, id: &str, key: &str) -> bool {
    materials
        .extras
        .get(id)
        .is_some_and(|value| value.get(key).is_some())
}

/// The clip's "Remove object"; `None` when it has none.
pub fn removal_of(materials: &MaterialPool, segment: &Segment) -> Option<ObjectRemoval> {
    segment
        .extras
        .iter()
        .find_map(|id| block(materials, id, REMOVAL_KEY))
}

/// The clip's "Enhance quality"; `None` when it has none.
pub fn upscale_of(materials: &MaterialPool, segment: &Segment) -> Option<Upscale> {
    segment
        .extras
        .iter()
        .find_map(|id| block(materials, id, UPSCALE_KEY))
}

/// What is done to a clip's frames, in order: the object removed, then the
/// picture enhanced.
#[derive(Debug, Clone, PartialEq)]
pub struct Chain {
    pub removal: Option<ObjectRemoval>,
    pub upscale: Option<Upscale>,
}

impl Chain {
    /// The chain of a video clip; `None` when neither is on (the cheap
    /// answer the compositor asks for on every clip and frame).
    pub fn of(materials: &MaterialPool, segment: &Segment) -> Option<Chain> {
        if segment.extras.is_empty() {
            return None;
        }
        let removal = removal_of(materials, segment);
        let upscale = upscale_of(materials, segment);
        if removal.is_none() && upscale.is_none() {
            return None;
        }
        materials.video(&segment.material_id)?;
        Some(Chain { removal, upscale })
    }

    /// The operations as a directory-name token: `remove`, `x2`,
    /// `remove_x4`.
    pub fn ops(&self) -> String {
        let mut ops = Vec::new();
        if self.removal.is_some() {
            ops.push("remove".to_string());
        }
        if let Some(up) = &self.upscale {
            ops.push(format!("x{}", up.scale));
        }
        ops.join("_")
    }

    /// A short hash of everything that decides the frames: the models and
    /// their versions, the mask, the growth, the scale and
    /// [`PIPELINE_REVISION`]. Fractions are rounded to a ten-thousandth so a
    /// value that went through JSON and back keys the same.
    pub fn signature(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let mut h = Sha256::new();
        let q = |v: f32| ((v * 10_000.0).round() as i32).to_le_bytes();
        h.update(PIPELINE_REVISION.to_le_bytes());
        h.update(MAX_LONG_SIDE.to_le_bytes());
        if let Some(r) = &self.removal {
            h.update(b"remove");
            h.update(r.model.as_bytes());
            h.update([0]);
            h.update(r.version.as_bytes());
            h.update([0]);
            h.update(q(r.grow));
            if let Some(prompt) = &r.prompt {
                let matte = removal_matte_setting(prompt);
                h.update(matte.version.as_bytes());
                h.update(crate::modules::matting::cache::prompt_hash(prompt).as_bytes());
            }
            for stroke in &r.strokes {
                h.update(b"s");
                h.update(q(stroke.radius));
                for p in &stroke.points {
                    h.update(q(p[0]));
                    h.update(q(p[1]));
                }
            }
            for b in &r.boxes {
                h.update(b"b");
                for v in b {
                    h.update(q(*v));
                }
            }
        }
        if let Some(u) = &self.upscale {
            h.update(b"upscale");
            h.update(u.model.as_bytes());
            h.update([0]);
            h.update(u.version.as_bytes());
            h.update(u.scale.to_le_bytes());
        }
        format!("{:x}", h.finalize())[..12].to_string()
    }

    /// The size a frame of a `width × height` source is decoded at for this
    /// chain: the source's own size, at most [`MAX_LONG_SIDE`] on the long
    /// side (an upscaled source is at most [`MAX_UPSCALE_SOURCE`] already).
    pub fn decode_size(&self, width: u32, height: u32) -> (u32, u32) {
        fit(width, height, MAX_LONG_SIDE)
    }

    /// The size a made frame of a `width × height` source has.
    pub fn out_size(&self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = self.decode_size(width, height);
        match &self.upscale {
            Some(up) => {
                let long = (w.max(h) * up.scale).min(MAX_LONG_SIDE);
                fit_to(w, h, long)
            }
            None => (w, h),
        }
    }
}

/// `width × height` scaled down (never up) to at most `long` on the long
/// side, both sides even.
pub fn fit(width: u32, height: u32, long: u32) -> (u32, u32) {
    if width.max(height) <= long {
        return (even(width), even(height));
    }
    fit_to(width, height, long)
}

/// `width × height` scaled to exactly `long` on the long side, both sides
/// even.
fn fit_to(width: u32, height: u32, long: u32) -> (u32, u32) {
    let scale = long as f64 / width.max(height).max(1) as f64;
    (
        even((width as f64 * scale).round() as u32),
        even((height as f64 * scale).round() as u32),
    )
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// Seconds per frame a chain takes on a `width × height` source, on an RTX
/// 3060 (CUDA) and on four CPU threads, from the speeds measured there
/// (`docs/STATUS.md`, "Remove object and enhance quality"): LaMa per crop,
/// Real-ESRGAN per megapixel of input, plus decoding and writing. Rough,
/// for a sentence that says seconds, minutes or hours before a bake starts;
/// a running bake reports the pace it sees.
pub fn seconds_per_frame(chain: &Chain, source: (u32, u32)) -> (f64, f64) {
    const LAMA_GPU: f64 = 0.16;
    const LAMA_CPU: f64 = 1.9;
    const ESRGAN_GPU_PER_MP: f64 = 0.6;
    const ESRGAN_CPU_PER_MP: f64 = 7.1;
    const OVERHEAD_PER_MP: f64 = 0.02;
    let (w, h) = chain.decode_size(source.0, source.1);
    let mp = w as f64 * h as f64 / 1e6;
    let (mut gpu, mut cpu) = (OVERHEAD_PER_MP * mp, OVERHEAD_PER_MP * mp);
    if chain.removal.is_some() {
        gpu += LAMA_GPU;
        cpu += LAMA_CPU;
    }
    if chain.upscale.is_some() {
        gpu += ESRGAN_GPU_PER_MP * mp;
        cpu += ESRGAN_CPU_PER_MP * mp;
    }
    (gpu, cpu)
}

/// A duration in words: "12 s", "4 min", "2.5 h".
pub fn duration_words(seconds: f64) -> String {
    if seconds < 90.0 {
        format!("{} s", seconds.round().max(1.0) as u64)
    } else if seconds < 5400.0 {
        format!("{} min", (seconds / 60.0).round() as u64)
    } else {
        format!("{:.1} h", seconds / 3600.0)
    }
}

/// Whether "Enhance quality" can be used on a `width × height` source, and
/// why not.
pub fn upscale_refusal(width: u32, height: u32) -> Option<String> {
    let long = width.max(height);
    (long > MAX_UPSCALE_SOURCE).then(|| {
        format!(
            "this clip is {long} px on its long side; enhance quality is for footage up to \
             {MAX_UPSCALE_SOURCE} px (it makes at most {MAX_LONG_SIDE} px)"
        )
    })
}

/// The edit that sets one of the two blocks (`key`) on `segment_id` to
/// `value`, or removes it, with the extras entry to put into the pool
/// before applying it.
fn set_block_command(
    project: &Project,
    segment_id: &str,
    key: &str,
    value: Option<serde_json::Value>,
    label: &str,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if project.materials.kind_of(&segment.material_id) != Some(MaterialKind::Video) {
        return Err("only a video clip has frames to remake".into());
    }
    let materials = &project.materials;
    let entry = value.map(|value| (new_id(), serde_json::json!({ key: value })));
    // The reference keeps its place in `extras`, so a change does not reorder
    // the clip's other references.
    let slot = segment
        .extras
        .iter()
        .position(|id| entry_is(materials, id, key));
    let new_ref = entry.as_ref().map(|(id, _)| id.clone());
    let command =
        crate::modules::inspector::edit::replace_segment(project, segment_id, label, |segment| {
            segment.extras.retain(|id| !entry_is(materials, id, key));
            if let Some(id) = new_ref {
                let at = slot
                    .unwrap_or(segment.extras.len())
                    .min(segment.extras.len());
                segment.extras.insert(at, id);
            }
        })?;
    Ok((entry, command))
}

/// The edit that sets `segment_id`'s "Remove object" (`None`: off).
pub fn set_removal_command(
    project: &Project,
    segment_id: &str,
    removal: Option<&ObjectRemoval>,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    if let Some(why) = removal.and_then(ObjectRemoval::invalid) {
        return Err(why);
    }
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if removal_of(&project.materials, segment).as_ref() == removal {
        return Err(match removal {
            Some(_) => "the clip already has this object removed".into(),
            None => "the clip has no object removed".into(),
        });
    }
    let value = removal
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| e.to_string())?;
    let label = if removal.is_some() {
        "Remove object"
    } else {
        "Remove object off"
    };
    set_block_command(project, segment_id, REMOVAL_KEY, value, label)
}

/// The edit that sets `segment_id`'s "Enhance quality" (`None`: off).
pub fn set_upscale_command(
    project: &Project,
    segment_id: &str,
    upscale: Option<&Upscale>,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if let (Some(_), Some(video)) = (upscale, project.materials.video(&segment.material_id)) {
        if let Some(why) = upscale_refusal(video.width, video.height) {
            return Err(why);
        }
    }
    if upscale_of(&project.materials, segment).as_ref() == upscale {
        return Err(match upscale {
            Some(u) => format!("the clip is already enhanced {}x", u.scale),
            None => "the clip is not enhanced".into(),
        });
    }
    let value = upscale
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| e.to_string())?;
    let label = match upscale {
        Some(u) => format!("Enhance quality {}x", u.scale),
        None => "Enhance quality off".into(),
    };
    set_block_command(project, segment_id, UPSCALE_KEY, value, &label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::compositing::PromptPoint;
    use crate::modules::project::document::{
        CanvasConfig, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };
    use crate::modules::timeline::History;

    pub(crate) fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nowhere.mp4".into(),
            width: 640,
            height: 360,
            duration: 2_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["keep".into()],
            keyframes: Vec::new(),
        });
        p.materials
            .extras
            .insert("keep".into(), serde_json::json!({"name": "x"}));
        p.tracks.push(track);
        p
    }

    fn apply(
        p: &mut Project,
        history: &mut History,
        made: (Option<(String, serde_json::Value)>, EditCommand),
    ) {
        let (entry, command) = made;
        if let Some((id, value)) = entry {
            p.materials.extras.insert(id, value);
        }
        history.apply(p, command).unwrap();
    }

    fn boxed() -> ObjectRemoval {
        ObjectRemoval {
            boxes: vec![[0.1, 0.1, 0.2, 0.2]],
            ..ObjectRemoval::new()
        }
    }

    #[test]
    fn both_settings_go_on_and_off_and_undo_to_the_byte() {
        let mut p = project();
        let before = serde_json::to_string(&p).unwrap();
        let mut history = History::new();
        let removal = boxed();
        let made = set_removal_command(&p, "s", Some(&removal)).unwrap();
        apply(&mut p, &mut history, made);
        let up = Upscale::new(2).unwrap();
        let made = set_upscale_command(&p, "s", Some(&up)).unwrap();
        apply(&mut p, &mut history, made);
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(seg.extras[0], "keep", "the other reference keeps its place");
        let chain = Chain::of(&p.materials, seg).unwrap();
        assert_eq!(chain.removal.as_ref(), Some(&removal));
        assert_eq!(chain.upscale.as_ref(), Some(&up));
        assert_eq!(chain.ops(), "remove_x2");
        assert_eq!(chain.out_size(640, 360), (1280, 720));

        let made = set_removal_command(&p, "s", None).unwrap();
        apply(&mut p, &mut history, made);
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(Chain::of(&p.materials, seg).unwrap().ops(), "x2");
        assert!(set_removal_command(&p, "s", None).is_err());

        for _ in 0..3 {
            history.undo(&mut p).unwrap();
        }
        let mut undone = p.clone();
        crate::modules::project::prune::prune_unreferenced(&mut undone);
        assert_eq!(serde_json::to_string(&undone).unwrap(), before);
    }

    #[test]
    fn a_removal_without_a_mask_or_outside_the_picture_is_refused() {
        let p = project();
        let error = set_removal_command(&p, "s", Some(&ObjectRemoval::new())).unwrap_err();
        assert!(error.contains("nothing is marked"), "{error}");
        let outside = ObjectRemoval {
            boxes: vec![[0.9, 0.1, 1.2, 0.2]],
            ..ObjectRemoval::new()
        };
        assert!(set_removal_command(&p, "s", Some(&outside)).is_err());
        let fat = ObjectRemoval {
            strokes: vec![Stroke {
                points: vec![[0.5, 0.5]],
                radius: 0.9,
            }],
            ..ObjectRemoval::new()
        };
        assert!(set_removal_command(&p, "s", Some(&fat)).is_err());
        assert!(Upscale::new(3).is_err());
    }

    #[test]
    fn large_footage_is_not_upscaled_and_the_output_is_capped() {
        assert!(upscale_refusal(3840, 2160).is_some());
        assert!(upscale_refusal(1920, 1080).is_none());
        let x4 = Chain {
            removal: None,
            upscale: Some(Upscale::new(4).unwrap()),
        };
        assert_eq!(x4.out_size(1920, 1080), (3840, 2160));
        assert_eq!(x4.out_size(1280, 720), (3840, 2160));
        assert_eq!(x4.out_size(320, 180), (1280, 720));
        let remove = Chain {
            removal: Some(boxed()),
            upscale: None,
        };
        assert_eq!(remove.out_size(4096, 2160), (3840, 2024));
        assert_eq!(remove.out_size(1080, 1920), (1080, 1920));
    }

    #[test]
    fn the_signature_follows_the_mask_and_survives_json() {
        let a = Chain {
            removal: Some(boxed()),
            upscale: None,
        };
        let json = serde_json::to_string(&a.removal).unwrap();
        let back = Chain {
            removal: serde_json::from_str(&json).unwrap(),
            upscale: None,
        };
        assert_eq!(a.signature(), back.signature());
        let mut moved = boxed();
        moved.boxes[0][0] = 0.11;
        let b = Chain {
            removal: Some(moved),
            upscale: None,
        };
        assert_ne!(a.signature(), b.signature());
        let mut clicked = boxed();
        clicked.prompt = Some(ObjectPrompt {
            time: 0,
            points: vec![PromptPoint {
                x: 0.5,
                y: 0.5,
                keep: true,
            }],
        });
        let c = Chain {
            removal: Some(clicked),
            upscale: None,
        };
        assert_ne!(a.signature(), c.signature());
        // A default growth is not written, so old files and new ones agree.
        assert!(!json.contains("grow"), "{json}");
    }
}
