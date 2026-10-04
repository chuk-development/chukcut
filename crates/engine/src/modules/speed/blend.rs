//! Frame blending: a slowed-down clip shows a mix of the two source frames
//! either side of the instant it plays, instead of holding one frame until the
//! next is due.
//!
//! At 0.25x a 30 fps file has a new frame every fourth output frame; without
//! blending the picture stutters in steps of four. With it, output frame `k`
//! at source position `p` frames shows `frame(floor p)` and
//! `frame(floor p + 1)` mixed by `fract p`, so the motion is continuous. The
//! same arithmetic smooths a 24 fps clip on a 30 fps timeline, a speed ramp's
//! slow part, and a constant slow motion alike, because it only asks where in
//! the file the clip is (`TimeMap`), never why.
//!
//! ## The document
//!
//! A per-clip setting, stored the way `audiofx` stores its block: one entry
//! in `MaterialPool::extras`, `{ "frame_blend": { "mode": "blend" } }`,
//! referenced from the clip's `extras`. "None" stores nothing, so a clip that
//! never had blending and one that had it switched off are the same bytes.
//! The edit is a remove + insert of the segment (`set_frame_blend_command`),
//! which makes undo exact without a new `EditCommand` variant. A split copies
//! `extras`, so both halves keep the setting.
//!
//! ## The renderer
//!
//! [`blend_for`] answers the two source instants and the weight for one
//! rendered instant; the compositor fetches both frames and accumulates them
//! (`render::compositor`, `Draw::Accumulated`). The provider keeps the last
//! two frames of each file (`media::provider`), so the earlier of the two is
//! almost always a cache hit and the decoder only ever moves forward.
//!
//! ## Optical flow
//!
//! The third mode, "Optical flow (AI)" (`FrameBlend::Flow`), shows a frame
//! that RIFE made between the two neighbours instead of a mix of them, so a
//! moving edge is in one place rather than in two at half strength. Those
//! frames are baked into the cache by the ML worker (`speed::flow`); until a
//! frame is there the clip falls back to the mix, so the preview never waits
//! and never shows a hole. [`blend_for`] answers the same sample for both
//! modes; the compositor decides which picture to draw.

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{new_id, MaterialKind, Micros, Project, Segment};
use crate::modules::project::MaterialPool;
use crate::modules::timeline::ops::EditCommand;

/// The key of the block inside its `MaterialPool::extras` value.
pub const KEY: &str = "frame_blend";

/// How a clip makes a frame that falls between two of its source frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameBlend {
    /// Show the source frame that is current at that instant.
    #[default]
    None,
    /// Mix the two neighbouring source frames by the fractional position.
    Blend,
    /// Show the frame RIFE made between the two neighbours (`speed::flow`),
    /// baked into the cache; the mix of [`FrameBlend::Blend`] until it is.
    Flow,
}

impl FrameBlend {
    pub const ALL: [FrameBlend; 3] = [FrameBlend::None, FrameBlend::Blend, FrameBlend::Flow];

    pub fn label(self) -> &'static str {
        match self {
            FrameBlend::None => "None",
            FrameBlend::Blend => "Frame blend",
            FrameBlend::Flow => "Optical flow (AI)",
        }
    }

    /// The name the CLI and the MCP server take.
    pub fn name(self) -> &'static str {
        match self {
            FrameBlend::None => "none",
            FrameBlend::Blend => "blend",
            FrameBlend::Flow => "flow",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Ok(FrameBlend::None),
            "blend" | "frame_blend" | "frame-blend" | "on" => Ok(FrameBlend::Blend),
            "flow" | "optical_flow" | "optical-flow" | "ai" => Ok(FrameBlend::Flow),
            other => Err(format!(
                "unknown frame blending {other:?}: use none, blend or flow"
            )),
        }
    }

    /// Whether the clip draws something between its source frames.
    pub fn blends(self) -> bool {
        self != FrameBlend::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Block {
    mode: FrameBlend,
}

/// The setting an extras id resolves to, if it is a frame-blend block.
fn entry(materials: &MaterialPool, id: &str) -> Option<FrameBlend> {
    let value = materials.extras.get(id)?.get(KEY)?;
    serde_json::from_value::<Block>(value.clone())
        .ok()
        .map(|b| b.mode)
}

/// The clip's frame blending; `None` when it has no block.
pub fn frame_blend_of(materials: &MaterialPool, segment: &Segment) -> FrameBlend {
    segment
        .extras
        .iter()
        .find_map(|id| entry(materials, id))
        .unwrap_or_default()
}

/// The two source instants a blended frame mixes, and how much of the second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendSample {
    /// The earlier source frame's number in the file (`floor` of the
    /// position in frames), which names a baked optical-flow frame.
    pub index: i64,
    /// Inside the earlier source frame (its middle, so rounding cannot tip
    /// the request into a neighbour).
    pub first: Micros,
    /// Inside the next source frame.
    pub second: Micros,
    /// `0..1`: the share of `second` in the mix.
    pub weight: f32,
}

/// Below this share of the second frame, the first is shown alone. 1/512 is
/// under half a code value of an 8-bit picture, so skipping the second fetch
/// there changes no pixel.
const NEGLIGIBLE: f64 = 1.0 / 512.0;

/// Where `source_time` falls between the frames of a `fps` file that lasts
/// `duration`. `None` when it is on a frame (nothing to blend) or past the
/// last one (no neighbour to blend with).
pub fn blend_sample(fps: f64, duration: Micros, source_time: Micros) -> Option<BlendSample> {
    if !fps.is_finite() || fps <= 0.0 || source_time < 0 {
        return None;
    }
    let position = source_time as f64 * fps / 1_000_000.0;
    let frame = position.floor();
    let weight = position - frame;
    if weight < NEGLIGIBLE {
        return None;
    }
    let middle = |index: f64| ((index + 0.5) * 1_000_000.0 / fps).round() as Micros;
    let second = middle(frame + 1.0);
    if duration > 0 && second >= duration {
        return None;
    }
    Some(BlendSample {
        index: frame as i64,
        first: middle(frame),
        second,
        weight: weight as f32,
    })
}

/// The blend for `segment` at `source_time`: `Some` only for a video clip
/// with frame blending or optical flow on, at an instant between two of its
/// frames.
pub fn blend_for(
    materials: &MaterialPool,
    segment: &Segment,
    source_time: Micros,
) -> Option<BlendSample> {
    if segment.extras.is_empty() || !frame_blend_of(materials, segment).blends() {
        return None;
    }
    let video = materials
        .videos
        .iter()
        .find(|v| v.id == segment.material_id)?;
    blend_sample(video.fps, video.duration, source_time)
}

/// The edit that sets `segment_id`'s frame blending, with the extras entry to
/// put into the pool before applying it (`None` when switching it off).
pub fn set_frame_blend_command(
    project: &Project,
    segment_id: &str,
    mode: FrameBlend,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if project.materials.kind_of(&segment.material_id) != Some(MaterialKind::Video) {
        return Err("only a video clip has frames to blend".into());
    }
    let materials = &project.materials;
    if frame_blend_of(materials, segment) == mode {
        return Err(format!("frame blending is already {}", mode.name()));
    }
    let entry =
        (mode != FrameBlend::None).then(|| (new_id(), serde_json::json!({ KEY: Block { mode } })));
    // The reference keeps its place in `extras`, so a change does not reorder
    // the clip's other references.
    let slot = segment
        .extras
        .iter()
        .position(|id| entry_is_blend(materials, id));
    let new_ref = entry.as_ref().map(|(id, _)| id.clone());
    let label = match mode {
        FrameBlend::None => "Frame blending off",
        FrameBlend::Blend => "Frame blending on",
        FrameBlend::Flow => "Optical flow on",
    };
    let command =
        crate::modules::inspector::edit::replace_segment(project, segment_id, label, |segment| {
            segment.extras.retain(|id| !entry_is_blend(materials, id));
            if let Some(id) = new_ref {
                let at = slot
                    .unwrap_or(segment.extras.len())
                    .min(segment.extras.len());
                segment.extras.insert(at, id);
            }
        })?;
    Ok((entry, command))
}

fn entry_is_blend(materials: &MaterialPool, id: &str) -> bool {
    materials
        .extras
        .get(id)
        .is_some_and(|value| value.get(KEY).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };
    use crate::modules::timeline::History;

    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nowhere.mp4".into(),
            width: 64,
            height: 64,
            duration: 2_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 4_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 0.25,
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

    fn apply(p: &mut Project, history: &mut History, mode: FrameBlend) {
        let (entry, command) = set_frame_blend_command(p, "s", mode).unwrap();
        if let Some((id, value)) = entry {
            p.materials.extras.insert(id, value);
        }
        history.apply(p, command).unwrap();
    }

    #[test]
    fn a_position_between_frames_mixes_the_two_by_its_fraction() {
        // 30 fps: frame 3 covers 100 000..133 333 µs. A quarter of the way in.
        let s = blend_sample(30.0, 2_000_000, 108_333).unwrap();
        assert!((s.weight - 0.25).abs() < 1e-4, "{s:?}");
        assert_eq!(s.index, 3);
        assert_eq!(s.first, 116_667);
        assert_eq!(s.second, 150_000);
    }

    #[test]
    fn on_a_frame_or_past_the_last_there_is_nothing_to_blend() {
        assert_eq!(blend_sample(30.0, 2_000_000, 100_000), None);
        assert_eq!(blend_sample(30.0, 2_000_000, 1_990_000), None);
        assert_eq!(blend_sample(0.0, 2_000_000, 50_000), None);
    }

    #[test]
    fn switching_it_on_and_off_undoes_to_the_byte() {
        let mut p = project();
        let before = serde_json::to_string(&p).unwrap();
        let mut history = History::new();
        apply(&mut p, &mut history, FrameBlend::Blend);
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(frame_blend_of(&p.materials, seg), FrameBlend::Blend);
        assert_eq!(seg.extras[0], "keep", "the other reference keeps its place");
        assert!(blend_for(&p.materials, seg, 108_333).is_some());

        apply(&mut p, &mut history, FrameBlend::None);
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(frame_blend_of(&p.materials, seg), FrameBlend::None);
        assert_eq!(seg.extras, vec!["keep".to_string()]);

        history.undo(&mut p).unwrap();
        history.undo(&mut p).unwrap();
        let mut undone = p.clone();
        crate::modules::project::prune::prune_unreferenced(&mut undone);
        assert_eq!(serde_json::to_string(&undone).unwrap(), before);
    }

    #[test]
    fn setting_what_is_already_there_is_refused() {
        let p = project();
        assert!(set_frame_blend_command(&p, "s", FrameBlend::None).is_err());
    }

    #[test]
    fn names_parse_both_ways() {
        assert_eq!(FrameBlend::parse("Blend"), Ok(FrameBlend::Blend));
        assert_eq!(FrameBlend::parse("off"), Ok(FrameBlend::None));
        assert_eq!(FrameBlend::parse("optical_flow"), Ok(FrameBlend::Flow));
        for mode in FrameBlend::ALL {
            assert_eq!(FrameBlend::parse(mode.name()), Ok(mode));
        }
        assert!(FrameBlend::parse("sideways").is_err());
    }

    #[test]
    fn optical_flow_samples_like_blending_and_switches_from_it_in_one_step() {
        let mut p = project();
        let mut history = History::new();
        apply(&mut p, &mut history, FrameBlend::Blend);
        apply(&mut p, &mut history, FrameBlend::Flow);
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(frame_blend_of(&p.materials, seg), FrameBlend::Flow);
        assert_eq!(seg.extras.len(), 2, "one block replaced the other");
        assert_eq!(
            blend_for(&p.materials, seg, 108_333),
            blend_sample(30.0, 2_000_000, 108_333)
        );
        history.undo(&mut p).unwrap();
        let (_, seg) = p.segment("s").unwrap();
        assert_eq!(frame_blend_of(&p.materials, seg), FrameBlend::Blend);
    }
}
