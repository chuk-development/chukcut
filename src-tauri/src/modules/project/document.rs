//! The project document.
//!
//! One `Project` is one editing session's full state: what media is involved,
//! how it is arranged in time, and how each piece is transformed. It is the
//! single source of truth — the renderer, the exporter and the UI are all pure
//! functions of this struct plus a playhead position.
//!
//! ## Time
//!
//! All times are **microseconds** (`i64`), never floats and never frames.
//! Floats accumulate error when you add a thousand clip durations together;
//! frames force a decision about frame rate into the data model, which breaks
//! the moment a 24 fps clip lands on a 30 fps timeline. Microseconds are exact,
//! survive any frame rate, and convert to frames only at the edges (render,
//! export, ruler labels).
//!
//! ## Materials vs segments
//!
//! A `Segment` is a placement in time. A `Material` is the thing being placed.
//! Segments reference materials by id and never embed them. Two segments that
//! show the same file share one material, so relinking moved media, swapping a
//! clip, or reporting "this project needs these 12 files" is a pool operation
//! rather than a tree walk.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Current on-disk schema version. Bump on any breaking change and add a
/// migration in `migrate.rs`.
pub const SCHEMA_VERSION: u32 = 1;

/// Microseconds. The unit of every time value in the document.
pub type Micros = i64;

pub const MICROS_PER_SECOND: Micros = 1_000_000;

/// A half-open time interval `[start, start + duration)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: Micros,
    pub duration: Micros,
}

impl TimeRange {
    pub fn new(start: Micros, duration: Micros) -> Self {
        Self { start, duration }
    }

    pub fn end(&self) -> Micros {
        self.start + self.duration
    }

    pub fn contains(&self, t: Micros) -> bool {
        t >= self.start && t < self.end()
    }

    pub fn overlaps(&self, other: &TimeRange) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    /// Intersection with `other`, or `None` when they do not overlap.
    pub fn intersect(&self, other: &TimeRange) -> Option<TimeRange> {
        let start = self.start.max(other.start);
        let end = self.end().min(other.end());
        (end > start).then(|| TimeRange::new(start, end - start))
    }
}

/// Stable identifier for materials, segments, tracks and keyframes.
pub type Id = String;

pub fn new_id() -> Id {
    Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// Project root
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub schema_version: u32,
    pub name: String,
    /// Unix millis.
    pub created_at: i64,
    pub updated_at: i64,

    pub canvas: CanvasConfig,
    /// Timeline frame rate. Individual clips keep their own rate; this is the
    /// rate the timeline is rendered and exported at.
    pub fps: f64,

    pub materials: MaterialPool,
    pub tracks: Vec<Track>,
}

impl Project {
    pub fn new(name: impl Into<String>, canvas: CanvasConfig, fps: f64) -> Self {
        Self {
            id: new_id(),
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            created_at: 0,
            updated_at: 0,
            canvas,
            fps,
            materials: MaterialPool::default(),
            tracks: Vec::new(),
        }
    }

    /// End of the last segment on any track. This is what the ruler and the
    /// exporter treat as the project length.
    pub fn duration(&self) -> Micros {
        self.tracks
            .iter()
            .flat_map(|t| t.segments.iter())
            .map(|s| s.target_range.end())
            .max()
            .unwrap_or(0)
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn segment(&self, id: &str) -> Option<(&Track, &Segment)> {
        self.tracks
            .iter()
            .find_map(|t| t.segments.iter().find(|s| s.id == id).map(|s| (t, s)))
    }

    pub fn segment_mut(&mut self, id: &str) -> Option<&mut Segment> {
        self.tracks
            .iter_mut()
            .find_map(|t| t.segments.iter_mut().find(|s| s.id == id))
    }

    /// Every segment live at `time`, ordered back-to-front for compositing.
    ///
    /// Compositing order is `render_index` ascending, and `render_index` is
    /// derived from track order — lower tracks paint first. Hidden and muted
    /// lanes are filtered by the caller, not here, because the exporter and the
    /// preview disagree about what "muted" means for audio.
    pub fn segments_at(&self, time: Micros) -> Vec<(&Track, &Segment)> {
        let mut hits: Vec<(&Track, &Segment)> = self
            .tracks
            .iter()
            .flat_map(|t| {
                t.segments
                    .iter()
                    .filter(move |s| s.target_range.contains(time))
                    .map(move |s| (t, s))
            })
            .collect();
        hits.sort_by_key(|(_, s)| s.render_index);
        hits
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CanvasConfig {
    pub width: u32,
    pub height: u32,
    /// Solid background behind everything, as linear RGBA 0..1.
    pub background: [f32; 4],
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            width: 1080,
            height: 1920,
            background: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

// ---------------------------------------------------------------------------
// Materials
// ---------------------------------------------------------------------------

/// All materials in the project, grouped by kind.
///
/// Grouping by kind (rather than one heterogeneous list) is deliberate: it
/// keeps the JSON readable, lets each kind evolve its own fields without a
/// giant tagged enum, and matches how the UI browses them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialPool {
    #[serde(default)]
    pub videos: Vec<VideoMaterial>,
    #[serde(default)]
    pub audios: Vec<AudioMaterial>,
    #[serde(default)]
    pub images: Vec<ImageMaterial>,
    #[serde(default)]
    pub texts: Vec<TextMaterial>,
    /// Non-media parameter blocks referenced by segments (speed curves,
    /// transitions, effect instances). Kept as one map so adding a new kind
    /// does not change the schema.
    #[serde(default)]
    pub extras: HashMap<Id, serde_json::Value>,
}

impl MaterialPool {
    pub fn video(&self, id: &str) -> Option<&VideoMaterial> {
        self.videos.iter().find(|m| m.id == id)
    }

    pub fn audio(&self, id: &str) -> Option<&AudioMaterial> {
        self.audios.iter().find(|m| m.id == id)
    }

    pub fn image(&self, id: &str) -> Option<&ImageMaterial> {
        self.images.iter().find(|m| m.id == id)
    }

    pub fn text(&self, id: &str) -> Option<&TextMaterial> {
        self.texts.iter().find(|m| m.id == id)
    }

    /// Which kind a material id belongs to, without the caller guessing.
    pub fn kind_of(&self, id: &str) -> Option<MaterialKind> {
        if self.video(id).is_some() {
            Some(MaterialKind::Video)
        } else if self.audio(id).is_some() {
            Some(MaterialKind::Audio)
        } else if self.image(id).is_some() {
            Some(MaterialKind::Image)
        } else if self.text(id).is_some() {
            Some(MaterialKind::Text)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    Video,
    Audio,
    Image,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMaterial {
    pub id: Id,
    /// Absolute path on disk. Missing files are a UI concern (relink), not a
    /// load error — a project must still open with dead links.
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Full duration of the source file.
    pub duration: Micros,
    pub fps: f64,
    /// Whether the file carries an audio stream we can pull from.
    #[serde(default)]
    pub has_audio: bool,
    /// Display rotation from container metadata, in degrees (0/90/180/270).
    #[serde(default)]
    pub rotation: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMaterial {
    pub id: Id,
    pub path: String,
    pub duration: Micros,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMaterial {
    pub id: Id,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMaterial {
    pub id: Id,
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub color: [f32; 4],
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub align: TextAlign,
    /// Outline width in pixels; 0 disables the stroke.
    #[serde(default)]
    pub stroke_width: f32,
    #[serde(default)]
    pub stroke_color: [f32; 4],
    #[serde(default)]
    pub shadow: Option<TextShadow>,
    #[serde(default)]
    pub background: Option<[f32; 4]>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TextShadow {
    pub color: [f32; 4],
    pub offset: [f32; 2],
    pub blur: f32,
}

// ---------------------------------------------------------------------------
// Tracks and segments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Audio,
    Text,
    Sticker,
    Effect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    /// Segments are kept sorted by `target_range.start` and must not overlap
    /// within a track. Edit operations are responsible for maintaining both
    /// invariants; `validate()` checks them.
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub hidden: bool,
    /// Per-track volume multiplier for audio-bearing lanes.
    #[serde(default = "one")]
    pub volume: f32,
}

fn one() -> f32 {
    1.0
}

impl Track {
    pub fn new(kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            kind,
            name: name.into(),
            segments: Vec::new(),
            muted: false,
            locked: false,
            hidden: false,
            volume: 1.0,
        }
    }

    /// Segment covering `time`, if any.
    pub fn segment_at(&self, time: Micros) -> Option<&Segment> {
        self.segments.iter().find(|s| s.target_range.contains(time))
    }

    /// Whether `range` is free, ignoring the segment `exclude` (used when
    /// moving a segment within its own track).
    pub fn is_range_free(&self, range: &TimeRange, exclude: Option<&str>) -> bool {
        !self
            .segments
            .iter()
            .filter(|s| Some(s.id.as_str()) != exclude)
            .any(|s| s.target_range.overlaps(range))
    }
}

/// One placement of a material on a track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: Id,
    pub material_id: Id,
    /// Where the segment sits on the timeline.
    pub target_range: TimeRange,
    /// Which part of the source material is used. For generated materials
    /// (text, solid color) this is `[0, target_range.duration)`.
    pub source_range: TimeRange,
    /// Compositing order, ascending = painted later = on top.
    pub render_index: i32,
    #[serde(default = "one")]
    pub speed: f32,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default)]
    pub crop: Option<Crop>,
    /// Ids into `MaterialPool::extras` — effects, filters, animations applied
    /// to this segment, in application order.
    #[serde(default)]
    pub extras: Vec<Id>,
    #[serde(default)]
    pub keyframes: Vec<KeyframeTrack>,
}

impl Segment {
    /// Map a timeline instant to a position inside the source material,
    /// accounting for `speed`. Returns `None` when `time` is outside the
    /// segment.
    pub fn source_time_at(&self, time: Micros) -> Option<Micros> {
        if !self.target_range.contains(time) {
            return None;
        }
        let offset = time - self.target_range.start;
        let scaled = (offset as f64 * self.speed as f64) as Micros;
        Some(self.source_range.start + scaled)
    }
}

/// Affine placement of a segment on the canvas.
///
/// Position is in normalized canvas units (`0,0` = center, `1` = half the
/// canvas dimension) so a transform survives a canvas resize — switching a
/// project from 9:16 to 16:9 keeps clips where the user put them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Transform {
    pub position: [f32; 2],
    pub scale: [f32; 2],
    /// Degrees, clockwise.
    pub rotation: f32,
    pub opacity: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation: 0.0,
            opacity: 1.0,
            flip_h: false,
            flip_v: false,
        }
    }
}

/// Source-space crop, as fractions of the source dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Keyframes
// ---------------------------------------------------------------------------

/// An animatable property. Adding a variant is how a new property becomes
/// keyframable; the renderer matches on this when sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimatableProperty {
    PositionX,
    PositionY,
    ScaleX,
    ScaleY,
    Rotation,
    Opacity,
    Volume,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyframeTrack {
    pub property: AnimatableProperty,
    /// Sorted by `time`.
    pub keyframes: Vec<Keyframe>,
}

impl KeyframeTrack {
    /// Value at `time`, interpolated between neighbours.
    ///
    /// Times are relative to the segment start, not the timeline, so trimming
    /// or moving a segment does not desynchronize its animation.
    pub fn sample(&self, time: Micros) -> Option<f32> {
        if self.keyframes.is_empty() {
            return None;
        }
        let first = self.keyframes.first()?;
        let last = self.keyframes.last()?;
        if time <= first.time {
            return Some(first.value);
        }
        if time >= last.time {
            return Some(last.value);
        }
        let idx = self.keyframes.partition_point(|k| k.time <= time);
        let a = &self.keyframes[idx - 1];
        let b = &self.keyframes[idx];
        let span = (b.time - a.time) as f32;
        let t = if span <= 0.0 {
            0.0
        } else {
            (time - a.time) as f32 / span
        };
        Some(a.value + (b.value - a.value) * a.easing.apply(t))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Keyframe {
    /// Relative to the segment start.
    pub time: Micros,
    pub value: f32,
    #[serde(default)]
    pub easing: Easing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    Hold,
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    /// Remap a normalized 0..1 progress.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Hold => 0.0,
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => t * (2.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub message: String,
    /// Segment or track the issue belongs to, when it is localizable.
    pub subject_id: Option<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

impl Project {
    /// Structural checks. Warnings are things the app can live with (a missing
    /// file); errors mean an edit operation produced an inconsistent document
    /// and is a bug on our side.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();

        for track in &self.tracks {
            let mut prev_end = Micros::MIN;
            for seg in &track.segments {
                if seg.target_range.duration <= 0 {
                    issues.push(ValidationIssue {
                        severity: Severity::Error,
                        message: "segment has non-positive duration".into(),
                        subject_id: Some(seg.id.clone()),
                    });
                }
                if seg.target_range.start < prev_end {
                    issues.push(ValidationIssue {
                        severity: Severity::Error,
                        message: "segments overlap or are unsorted".into(),
                        subject_id: Some(seg.id.clone()),
                    });
                }
                prev_end = seg.target_range.end();

                if self.materials.kind_of(&seg.material_id).is_none() {
                    issues.push(ValidationIssue {
                        severity: Severity::Error,
                        message: format!("segment references unknown material {}", seg.material_id),
                        subject_id: Some(seg.id.clone()),
                    });
                }
            }
        }

        for m in &self.materials.videos {
            if !std::path::Path::new(&m.path).exists() {
                issues.push(ValidationIssue {
                    severity: Severity::Warning,
                    message: format!("media file is missing: {}", m.path),
                    subject_id: Some(m.id.clone()),
                });
            }
        }

        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_range_intersection() {
        let a = TimeRange::new(0, 100);
        let b = TimeRange::new(50, 100);
        assert_eq!(a.intersect(&b), Some(TimeRange::new(50, 50)));
        assert_eq!(a.intersect(&TimeRange::new(200, 10)), None);
    }

    #[test]
    fn keyframe_sampling_interpolates_and_clamps() {
        let track = KeyframeTrack {
            property: AnimatableProperty::Opacity,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 1_000_000,
                    value: 1.0,
                    easing: Easing::Linear,
                },
            ],
        };
        assert_eq!(track.sample(-5), Some(0.0));
        assert_eq!(track.sample(500_000), Some(0.5));
        assert_eq!(track.sample(2_000_000), Some(1.0));
    }

    #[test]
    fn segment_maps_timeline_time_to_source_time() {
        let seg = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(1_000_000, 1_000_000),
            source_range: TimeRange::new(500_000, 1_000_000),
            render_index: 0,
            speed: 2.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        // Half a second into the segment at 2x speed = one second into source.
        assert_eq!(seg.source_time_at(1_500_000), Some(1_500_000));
        assert_eq!(seg.source_time_at(0), None);
    }

    #[test]
    fn overlapping_segments_are_flagged() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let mk = |start: Micros| Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        track.segments.push(mk(0));
        track.segments.push(mk(500_000));
        project.tracks.push(track);

        let issues = project.validate();
        assert!(issues.iter().any(|i| i.message.contains("overlap")));
    }
}
