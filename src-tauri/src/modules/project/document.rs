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
use std::collections::{BTreeMap, BTreeSet, HashSet};
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
    ///
    /// Defaulted like every other float in the document so that a file which
    /// lost the value — see `migrate::repair_non_finite`, which drops a key
    /// `serde_json` wrote as `null` because the number was NaN — still opens
    /// with something sane rather than not at all.
    #[serde(default = "default_fps")]
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

    /// The link group `segment_id` belongs to, if any.
    pub fn link_group_of(&self, segment_id: &str) -> Option<&Id> {
        let (_, segment) = self.segment(segment_id)?;
        self.materials.link_of(segment)
    }

    /// Every segment in `group`, with the track it sits on and its index there.
    ///
    /// In document order, which is what makes a mirrored edit deterministic:
    /// the composite a linked gesture expands into must be the same composite
    /// every time or undo stops being exact.
    pub fn link_members(&self, group: &str) -> Vec<(&Track, usize, &Segment)> {
        self.tracks
            .iter()
            .flat_map(|track| {
                track
                    .segments
                    .iter()
                    .enumerate()
                    .filter(move |(_, segment)| segment.extras.iter().any(|extra| extra == group))
                    .map(move |(index, segment)| (track, index, segment))
            })
            .collect()
    }

    /// Whether this clip's sound is played by a *linked* clip on an audio lane
    /// rather than by the clip itself.
    ///
    /// Importing a file with both streams puts the picture on a video lane and
    /// the sound on an audio lane, as two linked segments of the same material.
    /// Both would otherwise be heard — the mixer's rule is "a video material
    /// whose container carries audio contributes sound, wherever it sits" — and
    /// the same waveform summed with itself is 6 dB louder and phases with
    /// every microsecond the two are out by.
    ///
    /// So the segment that is *not* on the audio lane defers. The check is
    /// deliberately about where the partner sits rather than about a stored
    /// role: dragging the audio clip onto a video lane, or the picture onto an
    /// audio one, then does the obvious thing instead of silencing both.
    pub fn sound_is_on_a_linked_lane(&self, track: &Track, segment: &Segment) -> bool {
        if track.kind == TrackKind::Audio {
            return false;
        }
        let Some(group) = self.materials.link_of(segment) else {
            return false;
        };
        self.link_members(group).into_iter().any(|(lane, _, other)| {
            lane.kind == TrackKind::Audio
                && other.id != segment.id
                && other.material_id == segment.material_id
        })
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
    #[serde(default = "opaque_black")]
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
    /// Transitions, referenced from the `extras` list of the segment they are
    /// the entrance to. See [`TransitionMaterial`].
    ///
    /// A typed category rather than a JSON blob in `extras` below, because both
    /// `validate()` and the renderer read these fields on every frame and
    /// neither should be parsing `serde_json::Value` to do it.
    #[serde(default)]
    pub transitions: Vec<TransitionMaterial>,
    /// Per-clip colour adjustments, referenced from the `extras` list of the
    /// segment they grade. See [`ColorAdjustMaterial`].
    ///
    /// A typed category for the same two reasons `transitions` above is one:
    /// the compositor reads these fields on every frame and must not parse
    /// `serde_json::Value` to do it, and a pool category referenced through
    /// `extras` changes no existing segment, constructor or test — the exact
    /// payoff [`TransitionMaterial`] records for the same choice.
    #[serde(default)]
    pub color_adjusts: Vec<ColorAdjustMaterial>,
    /// Every link group id that some segment currently belongs to.
    ///
    /// ## Why linkage is on the segment and this is only a type tag
    ///
    /// A clip and the sound it was imported with are **two segments** on two
    /// lanes that move, trim, split and delete as one (see
    /// `docs/decisions/0005-linked-audio-and-video.md`). Where that
    /// relationship is stored was a real choice, and the alternative — a table
    /// on `Project` naming the pairs — was rejected for the reason
    /// [`TransitionMaterial`] gives for rejecting the same idea: a side table
    /// that names segment ids is a **second place segment ids appear**, so
    /// every structural edit has to remember to fix it up, and the one that
    /// forgets leaves a row pointing at a segment that no longer exists.
    ///
    /// So membership hangs on the segment: a link group id sits in
    /// [`Segment::extras`], and two segments are linked when they carry the
    /// same one. That single decision is what makes the rest free —
    /// `RemoveSegment` already snapshots the whole `Segment`, so undoing a
    /// delete restores the linkage without knowing links exist; `MoveSegment`
    /// and `TrimSegment` never touch `extras`, so a linked clip carries its
    /// group through every edit; and `split_at` clones the segment, so the
    /// only work is one line that gives the two new halves a group of their
    /// own.
    ///
    /// What is left over is the one thing membership-on-the-segment cannot
    /// answer: `extras` is a bare list of ids with **no type tag** — the kind
    /// of an id is whichever pool category it resolves in — so nothing in an
    /// id says whether it names a link group or an effect instance. This set
    /// is that category, and holds nothing else, because a link group has no
    /// parameters. It is maintained by exactly one command,
    /// `EditCommand::SetLinkGroup`, which is also what keeps it exactly
    /// invertible.
    ///
    /// A `BTreeSet` rather than a `Vec` for the reason `extras` below is a
    /// `BTreeMap`: a save of an unchanged project must produce an unchanged
    /// file, and an insertion-ordered list of ids does not.
    ///
    /// Deleting every member of a group leaves its id here with nothing
    /// pointing at it. That is deliberate: pruning on delete would mean the
    /// undo of that delete had to put the id back, and `RemoveSegment` does
    /// not carry it. An unreferenced group is inert — it is 38 bytes of JSON
    /// and no code path looks at it.
    #[serde(default)]
    pub links: BTreeSet<Id>,
    /// Non-media parameter blocks referenced by segments (speed curves,
    /// transitions, effect instances). Kept as one map so adding a new kind
    /// does not change the schema.
    ///
    /// Ordered rather than hashed, because the whole point of saving a project
    /// as indented JSON is that it can be read and diffed. A `HashMap` iterates
    /// in an order that changes between processes, so every save of an
    /// unchanged project rewrote this block into a different order and showed
    /// up as a diff. Sorted keys cost nothing at this size and make a save
    /// deterministic.
    #[serde(default)]
    pub extras: BTreeMap<Id, serde_json::Value>,
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

    pub fn transition(&self, id: &str) -> Option<&TransitionMaterial> {
        self.transitions.iter().find(|m| m.id == id)
    }

    pub fn transition_mut(&mut self, id: &str) -> Option<&mut TransitionMaterial> {
        self.transitions.iter_mut().find(|m| m.id == id)
    }

    pub fn color_adjust(&self, id: &str) -> Option<&ColorAdjustMaterial> {
        self.color_adjusts.iter().find(|m| m.id == id)
    }

    /// The colour adjustment applied to `segment`, if it has one.
    ///
    /// The resolution step for the colour category, exactly like
    /// [`Self::transition_of`] is for transitions: `Segment::extras` carries no
    /// type tag, so an id is a colour adjustment when this pool category
    /// resolves it. A segment carrying more than one is malformed; this returns
    /// the first, because rendering *a* grade beats rendering none.
    pub fn color_adjust_of(&self, segment: &Segment) -> Option<&ColorAdjustMaterial> {
        segment.extras.iter().find_map(|id| self.color_adjust(id))
    }

    /// The transition `segment` is entered through, if it has one.
    ///
    /// `Segment::extras` carries no type tag — the kind of an id is whichever
    /// pool category it resolves in — so this is the resolution step for the
    /// transition category. A segment with more than one transition id is
    /// malformed and `validate()` reports it; this returns the first, because
    /// rendering *a* transition beats rendering none.
    pub fn transition_of(&self, segment: &Segment) -> Option<&TransitionMaterial> {
        segment.extras.iter().find_map(|id| self.transition(id))
    }

    /// The link group `segment` belongs to, if any.
    ///
    /// The resolution step for the link category, exactly like
    /// [`Self::transition_of`] is for transitions. A segment carrying more than
    /// one is malformed and `validate()` reports it; this returns the first,
    /// because moving a clip with *a* partner beats moving it with none.
    pub fn link_of<'a>(&self, segment: &'a Segment) -> Option<&'a Id> {
        segment.extras.iter().find(|id| self.links.contains(*id))
    }

    /// Which kind a material id belongs to, without the caller guessing.
    ///
    /// Transitions are absent on purpose: a [`MaterialKind`] is something that
    /// can be *placed* on a track, and a transition cannot be. A caller that
    /// wants one asks [`Self::transition`].
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
    #[serde(default = "default_fps")]
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
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "opaque_white")]
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
    #[serde(default = "opaque_black")]
    pub color: [f32; 4],
    #[serde(default = "origin")]
    pub offset: [f32; 2],
    #[serde(default = "zero")]
    pub blur: f32,
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

/// What a transition does between two clips.
///
/// One variant per shader entry point in
/// `modules/transitions/shaders/transition.wgsl`. Direction is deliberately
/// *not* baked into the variant — a left wipe and a right wipe are the same
/// shader with a different uniform, and four variants apiece would quadruple
/// this enum, the pipeline cache and the UI list for no gain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// Straight crossfade.
    #[default]
    Dissolve,
    /// Out to a colour, then in from it. The colour covers the whole canvas.
    DipToColor,
    /// A hard, optionally softened, edge sweeping across the frame.
    Wipe,
    /// Both clips travel together; the incoming one pushes the outgoing one off.
    Slide,
    /// The outgoing clip pushes towards the viewer as the incoming one settles
    /// back, crossfaded.
    Zoom,
}

/// Which way a directional transition travels across the frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDirection {
    Left,
    #[default]
    Right,
    Up,
    Down,
}

impl TransitionDirection {
    /// The value the shader's `switch` matches on. Kept next to the enum so the
    /// two orderings cannot drift apart unnoticed.
    pub fn shader_index(self) -> u32 {
        match self {
            TransitionDirection::Left => 0,
            TransitionDirection::Right => 1,
            TransitionDirection::Up => 2,
            TransitionDirection::Down => 3,
        }
    }
}

/// The length a transition gets when the user has not said otherwise.
pub const DEFAULT_TRANSITION_DURATION: Micros = 500_000;

/// The parameters of one transition, living in the material pool.
///
/// ## Why a material and not an entity of its own
///
/// The obvious alternative is a `Vec<Transition>` on `Track`, each row naming
/// the two segments it joins. It was rejected: a side table that names segments
/// is a *second* place segment ids appear, so every structural edit — remove,
/// move, split, ripple — has to remember to fix it up, and the one that forgets
/// leaves a row pointing at a segment that no longer exists. Hanging the
/// reference on the segment means a segment carries its transition with it
/// through every edit for free, and `RemoveSegment` — which already snapshots
/// the whole `Segment` — undoes the removal of both without knowing that
/// transitions exist at all.
///
/// So this is a material, referenced by id from a segment, exactly like every
/// other parameter block in the pool. The *parameters* live here rather than
/// inline on the segment for the same reason a video's do: the pool is where
/// things that can be enumerated and shared live, and it keeps the segment
/// schema fixed.
///
/// ## Why the incoming clip owns it, not the outgoing one
///
/// CapCut hangs its transition off the **left** (outgoing) segment. We hang it
/// off the **right** (incoming) one, and the reason is splitting. `split_at`
/// trims the original in place and inserts a fresh clone for the remainder.
/// With left-ownership, splitting the outgoing clip leaves the transition on
/// the half that no longer touches the cut, so the split composite grows an
/// extra command to move it. With right-ownership the clone is by construction
/// a clip whose left edge is a brand new cut with nothing on it, so the whole
/// fix is one unconditional line in `split_at`, and splitting the *outgoing*
/// clip needs nothing at all because the transition sits on a segment the split
/// never touched.
///
/// Read the ownership as a preposition: a transition describes how its segment
/// is *entered*.
///
/// ## Where the id is stored
///
/// In `Segment::extras`, which is a list of material ids with no type tag; the
/// kind is whichever pool category the id resolves in. That trick is CapCut's
/// (see `docs/research/draft-format.md`) and its payoff is visible here —
/// adding transitions to this format changed no existing segment, no existing
/// constructor and no existing test. The cost is that a corrupted id silently
/// drops the transition, which is exactly what `validate()` is made to catch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionMaterial {
    pub id: Id,
    pub kind: TransitionKind,
    /// Total length of the effect.
    ///
    /// A transition is **centred on the cut**: it runs from `cut - duration/2`
    /// to `cut + duration/2` and neither clip moves. The alternative — the two
    /// clips genuinely overlapping in time, CapCut's `is_overlap` — is not
    /// available to us, because "segments within a track never overlap" is an
    /// invariant the whole editing model rests on, and honouring the overlap
    /// would mean shortening the timeline and shifting everything downstream
    /// every time a transition's length changed.
    ///
    /// Centring instead means each clip contributes frames from *beyond* its
    /// trimmed boundary for half the duration: the outgoing clip is read past
    /// its out point and the incoming one before its in point, out of the
    /// handles the trim left behind. Where there is no handle the borrowed
    /// frame freezes at the boundary, which is what an editor does when it
    /// says "insufficient media".
    ///
    /// The start is deliberately *not* stored. It is derived from the cut,
    /// which is `incoming.target_range.start`, so trimming or moving either
    /// clip carries the transition along and there is no second copy of the
    /// truth to fall out of date.
    pub duration: Micros,
    /// Applied to the progress before the shader sees it. `Easing::Hold` is a
    /// legitimate choice: it holds the outgoing clip and cuts at the far edge.
    #[serde(default)]
    pub easing: Easing,
    /// Wipe and slide only.
    #[serde(default)]
    pub direction: TransitionDirection,
    /// Dip only. Linear RGBA.
    #[serde(default = "opaque_black")]
    pub color: [f32; 4],
    /// Wipe only: width of the softened edge as a fraction of the frame.
    #[serde(default = "default_softness")]
    pub softness: f32,
    /// Zoom only: extra scale the push adds. `0.35` reaches 1.35x.
    #[serde(default = "default_zoom")]
    pub zoom: f32,
}

fn default_softness() -> f32 {
    0.04
}

fn default_zoom() -> f32 {
    0.35
}

impl TransitionMaterial {
    /// A transition of `kind` with every other parameter at its default.
    pub fn new(kind: TransitionKind, duration: Micros) -> Self {
        Self {
            id: new_id(),
            kind,
            duration,
            easing: Easing::EaseInOut,
            direction: TransitionDirection::default(),
            color: opaque_black(),
            softness: default_softness(),
            zoom: default_zoom(),
        }
    }
}

// ---------------------------------------------------------------------------
// Colour adjustments
// ---------------------------------------------------------------------------

/// One clip's colour adjustments, living in the material pool.
///
/// ## Where these live, and why
///
/// Three homes were considered. Fields on `Segment` were rejected because a
/// struct-literal field breaks every existing `Segment { .. }` constructor in
/// the tree, including ones owned by other modules, for a feature most clips
/// never use. A `serde_json::Value` in `MaterialPool::extras` was rejected
/// because the compositor resolves this on **every frame** of every graded
/// clip, and that map exists for parameter blocks nothing hot reads. So it is
/// a typed pool category referenced from `Segment::extras`, exactly like
/// [`TransitionMaterial`] and for the reasons written there: the segment
/// carries its grade through every move, trim and split for free, and
/// `RemoveSegment` — which snapshots the whole segment — undoes the loss of
/// the reference without knowing colour exists.
///
/// ## Identity is the default, exactly
///
/// Every default is the arithmetic identity of its operation (add 0, multiply
/// by 1, mix by 1, shift by 0), and the compositor additionally skips the
/// colour pass entirely for an identity material, so a document that never
/// touches colour renders byte-identical to one built before the feature
/// existed. There is a pixel test pinning that.
///
/// The fields are what the shader applies, in the order the shader applies
/// them (temperature, saturation, contrast, brightness — see `quad.wgsl`):
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorAdjustMaterial {
    pub id: Id,
    /// Added to each channel after contrast. `-1..1`, `0` is identity.
    #[serde(default = "zero")]
    pub brightness: f32,
    /// Scales the distance from mid grey. `0..2`, `1` is identity.
    #[serde(default = "one")]
    pub contrast: f32,
    /// Mixes between the luma-grey image and the original. `0..2`, `1` is
    /// identity; above 1 oversaturates.
    #[serde(default = "one")]
    pub saturation: f32,
    /// Warm/cool shift: positive pushes red up and blue down. `-1..1`, `0` is
    /// identity.
    #[serde(default = "zero")]
    pub temperature: f32,
    /// A .cube look, applied *after* the scalar adjustments — grade first,
    /// look second, the order `quad.wgsl` implements.
    ///
    /// On this material rather than a category of its own because the two are
    /// one gesture in the panel and one uniform block in the shader, and a
    /// second pool category would force the grade/look ordering question to
    /// be answered across two materials on every segment. `None` is "no
    /// look", the identity-is-absence rule the scalars follow.
    #[serde(default)]
    pub lut: Option<LutRef>,
}

/// A reference to a .cube LUT file on disk.
///
/// The **path** is what is stored; the parsed cube is runtime state, cached
/// by `render::lut::LutCache` keyed on path + mtime. A project whose LUT file
/// has gone must still open: `validate()` reports the missing file as a
/// warning — the same rule as `VideoMaterial::path` — and the renderer
/// renders the clip unadjusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LutRef {
    /// Absolute path to the .cube file.
    pub path: String,
    /// Blend between the (graded) input and the LUT's output, `0..1`.
    /// `1` is the LUT as authored; `0` contributes nothing, but the path is
    /// kept so the slider can come back up without re-picking the file.
    #[serde(default = "one")]
    pub intensity: f32,
}

impl ColorAdjustMaterial {
    /// A fresh identity adjustment with its own id.
    pub fn identity() -> Self {
        Self {
            id: new_id(),
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: None,
        }
    }

    /// Whether the four scalar sliders are all at their identity.
    ///
    /// Distinct from [`Self::is_identity`] because the renderer gates the two
    /// halves independently: a clip with only a LUT must not take the scalar
    /// grade's encode/adjust path, and — the case that matters for the
    /// "render unadjusted" contract — a clip whose LUT *file is missing* and
    /// whose scalars are identity must take no colour path at all.
    pub fn scalars_are_identity(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 1.0
            && self.saturation == 1.0
            && self.temperature == 0.0
    }

    /// Whether applying this would change nothing.
    ///
    /// The compositor uses this to skip the colour pass outright rather than
    /// trusting floating-point identities to hold through the shader — a
    /// grade that shifts an untouched clip by one code value is a regression
    /// for every existing project.
    pub fn is_identity(&self) -> bool {
        self.scalars_are_identity() && self.lut.is_none()
    }

    /// The first field that is not a finite number, by name. Same contract as
    /// [`Transform::non_finite_field`] and for the same reason: a NaN saves as
    /// `null` and the project never opens again.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        for (name, value) in [
            ("brightness", self.brightness),
            ("contrast", self.contrast),
            ("saturation", self.saturation),
            ("temperature", self.temperature),
        ] {
            if !value.is_finite() {
                return Some(name);
            }
        }
        if self.lut.as_ref().is_some_and(|l| !l.intensity.is_finite()) {
            return Some("LUT intensity");
        }
        None
    }
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

// Every float in the document has a `serde` default, including the ones that
// are not optional in the schema. That is not an invitation to write partial
// files: it is what makes a *damaged* file recoverable. `serde_json` cannot
// write a NaN or an infinity — it emits `null` — so a project that once held
// one comes back with a null where a number belongs, and without a default the
// whole file refuses to open. `migrate::repair_non_finite` drops those keys and
// these functions decide what the document reads instead.

fn zero() -> f32 {
    0.0
}

fn default_fps() -> f64 {
    30.0
}

fn default_font_size() -> f32 {
    48.0
}

fn origin() -> [f32; 2] {
    [0.0, 0.0]
}

fn unit_scale() -> [f32; 2] {
    [1.0, 1.0]
}

fn opaque_black() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn opaque_white() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
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

/// The widest a segment's stored source duration may sit from the one its
/// speed implies, in microseconds.
///
/// The two cannot be related exactly. Durations are whole microseconds and the
/// factor between them is an `f32`, so every operation that derives one from
/// the other rounds; splitting compounds it, because both halves are derived
/// from the same whole. Two microseconds plus one per unit of speed covers
/// that and is still five orders of magnitude tighter than the error this
/// invariant exists to catch — a speed change that forgot to move a range is
/// wrong by a *factor* of the speed, which on a one-second clip is hundreds of
/// thousands of microseconds.
pub fn speed_slack(speed: f32) -> Micros {
    if !speed.is_finite() {
        return 0;
    }
    2 + speed.abs().ceil().min(1e6) as Micros
}

/// How much source a stretch of timeline consumes at `speed`.
///
/// `docs/architecture/project-format.md`:
/// `source_range.duration = target_range.duration × speed`. This is the one
/// function allowed to compute it. Every command that writes a source duration
/// writes *this* number, which is what makes undo exact: two commands that
/// rounded differently would leave the document a microsecond away from where
/// it started.
pub fn source_duration_for(target_duration: Micros, speed: f32) -> Micros {
    if !speed.is_finite() {
        return target_duration;
    }
    (target_duration as f64 * speed as f64).round() as Micros
}

impl Segment {
    /// The source duration this segment's timeline duration and speed imply.
    pub fn implied_source_duration(&self) -> Micros {
        if !self.speed.is_finite() {
            return self.source_range.duration;
        }
        source_duration_for(self.target_range.duration, self.speed)
    }

    /// Whether the two ranges agree about the speed between them.
    pub fn speed_invariant_holds(&self) -> bool {
        let implied = self.implied_source_duration();
        (self.source_range.duration - implied).abs() <= speed_slack(self.speed)
    }

    /// The first float on this segment that is not a finite number, by name.
    ///
    /// Non-finite values are refused at the edit boundary because they cannot
    /// be written back: `serde_json` has no NaN or infinity and emits `null`,
    /// which is a save that reports success and a file that never reopens.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        if !self.speed.is_finite() {
            return Some("speed");
        }
        if !self.volume.is_finite() {
            return Some("volume");
        }
        if let Some(field) = self.transform.non_finite_field() {
            return Some(field);
        }
        if let Some(field) = self.crop.as_ref().and_then(Crop::non_finite_field) {
            return Some(field);
        }
        if self
            .keyframes
            .iter()
            .any(|t| t.keyframes.iter().any(|k| !k.value.is_finite()))
        {
            return Some("keyframe value");
        }
        None
    }

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
    #[serde(default = "origin")]
    pub position: [f32; 2],
    #[serde(default = "unit_scale")]
    pub scale: [f32; 2],
    /// Degrees, clockwise.
    #[serde(default = "zero")]
    pub rotation: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
}

impl Transform {
    /// The first field that is not a finite number, by the name a user would
    /// recognise. `None` means every value is usable.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        if !self.position.iter().all(|v| v.is_finite()) {
            return Some("position");
        }
        if !self.scale.iter().all(|v| v.is_finite()) {
            return Some("scale");
        }
        if !self.rotation.is_finite() {
            return Some("rotation");
        }
        if !self.opacity.is_finite() {
            return Some("opacity");
        }
        None
    }
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
    #[serde(default = "zero")]
    pub left: f32,
    #[serde(default = "zero")]
    pub top: f32,
    #[serde(default = "one")]
    pub right: f32,
    #[serde(default = "one")]
    pub bottom: f32,
}

impl Crop {
    /// The first field that is not a finite number, by name.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        for (name, value) in [
            ("crop left", self.left),
            ("crop top", self.top),
            ("crop right", self.right),
            ("crop bottom", self.bottom),
        ] {
            if !value.is_finite() {
                return Some(name);
            }
        }
        None
    }
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
///
/// Ordered, and the order is the declaration order below. Nothing renders
/// differently for it — it is what lets a segment keep its `KeyframeTrack`s in
/// a canonical order, so that removing the last keyframe of a property and
/// undoing it puts the track back exactly where it was rather than at the end.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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
    #[serde(default = "zero")]
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
        // Warnings raised inside the walk below, kept apart only because the
        // error sink borrows `issues` for the length of the loop.
        let mut outside: Vec<ValidationIssue> = Vec::new();
        let mut error = |message: String, subject_id: Option<Id>| {
            issues.push(ValidationIssue {
                severity: Severity::Error,
                message,
                subject_id,
            });
        };

        // A non-finite number cannot be written to JSON — `serde_json` emits
        // `null` and the file never opens again — so one in the document means
        // an edit let it through and the next save is silent data loss.
        if !self.fps.is_finite() || self.fps <= 0.0 {
            error(
                format!("project frame rate is not a positive number: {}", self.fps),
                None,
            );
        }
        if !self.canvas.background.iter().all(|c| c.is_finite()) {
            error("canvas background is not a finite colour".into(), None);
        }

        // Ids are the document's only cross-references — `segment_mut` and
        // `track_mut` both return the *first* match — so a duplicate makes one
        // of the two unreachable and every edit aimed at it silently hits the
        // other.
        let mut seen_tracks: HashSet<&str> = HashSet::new();
        let mut seen_segments: HashSet<&str> = HashSet::new();

        for track in &self.tracks {
            if !seen_tracks.insert(track.id.as_str()) {
                error(
                    format!("two tracks share the id {}", track.id),
                    Some(track.id.clone()),
                );
            }
            if !track.volume.is_finite() {
                error(
                    "track volume is not a finite number".into(),
                    Some(track.id.clone()),
                );
            }

            let mut prev_end = Micros::MIN;
            for seg in &track.segments {
                if !seen_segments.insert(seg.id.as_str()) {
                    error(
                        format!("two segments share the id {}", seg.id),
                        Some(seg.id.clone()),
                    );
                }
                if seg.target_range.duration <= 0 {
                    error(
                        "segment has non-positive duration".into(),
                        Some(seg.id.clone()),
                    );
                }
                if seg.target_range.start < 0 {
                    error(
                        format!(
                            "segment starts before the timeline, at {} µs",
                            seg.target_range.start
                        ),
                        Some(seg.id.clone()),
                    );
                }
                if seg.target_range.start < prev_end {
                    error(
                        "segments overlap or are unsorted".into(),
                        Some(seg.id.clone()),
                    );
                }
                prev_end = seg.target_range.end();

                if seg.source_range.start < 0 {
                    error(
                        format!(
                            "segment reads from before the start of its material, at {} µs",
                            seg.source_range.start
                        ),
                        Some(seg.id.clone()),
                    );
                }
                if seg.source_range.duration <= 0 {
                    error(
                        "segment has a non-positive source duration".into(),
                        Some(seg.id.clone()),
                    );
                }

                // Two link groups on one clip means every mirrored edit picks
                // whichever `link_of` returned first, so half the partners move
                // and half do not. `SetLinkGroup` cannot produce it; a
                // hand-edited file can.
                let links = seg
                    .extras
                    .iter()
                    .filter(|id| self.materials.links.contains(*id))
                    .count();
                if links > 1 {
                    error(
                        format!("clip belongs to {links} link groups; it may belong to one"),
                        Some(seg.id.clone()),
                    );
                }

                if self.materials.kind_of(&seg.material_id).is_none() {
                    error(
                        format!("segment references unknown material {}", seg.material_id),
                        Some(seg.id.clone()),
                    );
                }

                if let Some(field) = seg.non_finite_field() {
                    error(
                        format!("segment {field} is not a finite number"),
                        Some(seg.id.clone()),
                    );
                } else if seg.speed <= 0.0 {
                    error(
                        format!("segment speed is not positive: {}", seg.speed),
                        Some(seg.id.clone()),
                    );
                } else if !seg.speed_invariant_holds() {
                    // The two ranges are the document's own definition of what
                    // a speed change means; when they disagree, `split_at`
                    // cuts the source in the wrong place and the exporter and
                    // the mixer read a different piece of the file than the
                    // preview showed.
                    error(
                        format!(
                            "segment source duration {} µs does not match its timeline duration \
                             {} µs at {}x speed (expected {} µs)",
                            seg.source_range.duration,
                            seg.target_range.duration,
                            seg.speed,
                            seg.implied_source_duration()
                        ),
                        Some(seg.id.clone()),
                    );
                }

                // Keyframes. The sampler walks a track with `partition_point`,
                // so unsorted or duplicated times do not merely look wrong,
                // they read the wrong pair of neighbours; and "this property
                // is animated" is "a track exists for it" on both sides of the
                // IPC boundary, so an emptied track left behind shows a
                // property as animated when it is not.
                let mut animated: HashSet<AnimatableProperty> = HashSet::new();
                for keys in &seg.keyframes {
                    if !animated.insert(keys.property) {
                        error(
                            format!("two keyframe tracks animate {:?}", keys.property),
                            Some(seg.id.clone()),
                        );
                    }
                    if keys.keyframes.is_empty() {
                        error(
                            format!("{:?} is marked as animated with no keyframes", keys.property),
                            Some(seg.id.clone()),
                        );
                    }
                    let mut previous: Option<Micros> = None;
                    for key in &keys.keyframes {
                        match previous {
                            Some(p) if key.time < p => error(
                                format!("{:?} keyframes are out of order", keys.property),
                                Some(seg.id.clone()),
                            ),
                            Some(p) if key.time == p => error(
                                format!(
                                    "two {:?} keyframes share the time {} µs",
                                    keys.property, key.time
                                ),
                                Some(seg.id.clone()),
                            ),
                            _ => {}
                        }
                        previous = Some(key.time);

                        // Not an error. Trimming a clip's tail legitimately
                        // leaves keyframes beyond the new end, and the document
                        // keeps them on purpose so that undoing the trim brings
                        // the animation back — `project-format.md`: "trim its
                        // head and the animation stays attached to the frames
                        // it was authored against". The user is told because
                        // part of what they authored is no longer playing.
                        if key.time < 0 || key.time > seg.target_range.duration {
                            outside.push(ValidationIssue {
                                severity: Severity::Warning,
                                message: format!(
                                    "a {:?} keyframe at {} µs is outside the clip and will not play",
                                    keys.property, key.time
                                ),
                                subject_id: Some(seg.id.clone()),
                            });
                        }
                    }
                }
            }
        }

        issues.extend(outside);

        // A colour adjustment is applied on every rendered frame of the clip
        // that references it, so a non-finite value here is a NaN handed to the
        // GPU — and, worse, a save that emits `null` and never opens again.
        // Pushed directly rather than through the `error` closure above, whose
        // borrow of `issues` has ended by this point.
        for m in &self.materials.color_adjusts {
            if let Some(field) = m.non_finite_field() {
                issues.push(ValidationIssue {
                    severity: Severity::Error,
                    message: format!("colour adjustment {field} is not a finite number"),
                    subject_id: Some(m.id.clone()),
                });
            }
            // A warning, not an error, for the reason a missing video file is
            // one: the project must still open and play, with the clip
            // rendered unadjusted. The renderer makes the same check per
            // frame through `render::lut::LutCache`.
            if let Some(lut) = &m.lut {
                if !std::path::Path::new(&lut.path).exists() {
                    issues.push(ValidationIssue {
                        severity: Severity::Warning,
                        message: format!("LUT file is missing: {}", lut.path),
                        subject_id: Some(m.id.clone()),
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

        // A transition is the one thing in this document whose correctness
        // depends on two segments at once, so its checks live with the module
        // that knows the rule rather than being restated here.
        issues.extend(crate::modules::transitions::validate::issues(self));

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

    /// A graded project must come back from disk exactly as it went in. The
    /// grade is a pool material referenced by id from `Segment::extras`, so
    /// both halves of that — the material's values and the reference — have to
    /// survive the trip.
    #[test]
    fn colour_adjustments_round_trip_through_json() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut adjust = ColorAdjustMaterial::identity();
        adjust.brightness = 0.25;
        adjust.contrast = 1.4;
        adjust.saturation = 0.5;
        adjust.temperature = -0.3;
        let adjust_id = adjust.id.clone();
        project.materials.color_adjusts.push(adjust);

        let mut track = Track::new(TrackKind::Video, "V1");
        let mut segment = Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        segment.extras.push(adjust_id.clone());
        track.segments.push(segment);
        project.tracks.push(track);

        let json = serde_json::to_string_pretty(&project).expect("serialize");
        let reloaded: Project = serde_json::from_str(&json).expect("deserialize");

        let segment = &reloaded.tracks[0].segments[0];
        let resolved = reloaded
            .materials
            .color_adjust_of(segment)
            .expect("the reference survived");
        assert_eq!(resolved.id, adjust_id);
        assert_eq!(resolved.brightness, 0.25);
        assert_eq!(resolved.contrast, 1.4);
        assert_eq!(resolved.saturation, 0.5);
        assert_eq!(resolved.temperature, -0.3);

        // And a second save is byte-identical, which is what makes the file
        // diffable — the property `MaterialPool::extras`' docs demand of every
        // category.
        let again = serde_json::to_string_pretty(&reloaded).expect("serialize again");
        assert_eq!(json, again);
    }

    /// A file written before the colour category existed has no
    /// `color_adjusts` key at all, and it must open with the pool empty rather
    /// than refuse to load.
    #[test]
    fn a_project_saved_before_colour_existed_still_opens() {
        let project = Project::new("old", CanvasConfig::default(), 30.0);
        let mut json: serde_json::Value = serde_json::to_value(&project).expect("serialize");
        let materials = json["materials"].as_object_mut().expect("materials object");
        materials.remove("color_adjusts");

        let reloaded: Project = serde_json::from_value(json).expect("an old file still opens");
        assert!(reloaded.materials.color_adjusts.is_empty());
    }

    /// The defaults are the identity, field by field. If one of these drifts,
    /// every untouched clip in every old project changes appearance.
    #[test]
    fn a_default_colour_adjustment_is_the_identity() {
        let adjust: ColorAdjustMaterial =
            serde_json::from_str(r#"{ "id": "c1" }"#).expect("defaults fill in");
        assert!(adjust.is_identity());
        assert!(ColorAdjustMaterial::identity().is_identity());

        let mut warmed = ColorAdjustMaterial::identity();
        warmed.temperature = 0.2;
        assert!(!warmed.is_identity());

        // A LUT alone makes the material non-identity even with every scalar
        // at rest — the two halves are gated independently.
        let mut looked = ColorAdjustMaterial::identity();
        looked.lut = Some(LutRef {
            path: "/looks/warm.cube".into(),
            intensity: 1.0,
        });
        assert!(!looked.is_identity());
        assert!(looked.scalars_are_identity());
    }

    /// A LUT reference round-trips — path and intensity — and a pre-LUT file
    /// (no `lut` key on the material) still opens with the field `None`.
    #[test]
    fn lut_references_round_trip_and_old_materials_open() {
        let mut adjust = ColorAdjustMaterial::identity();
        adjust.lut = Some(LutRef {
            path: "/looks/warm.cube".into(),
            intensity: 0.7,
        });
        let json = serde_json::to_string(&adjust).expect("serialize");
        let back: ColorAdjustMaterial = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, adjust);

        let old: ColorAdjustMaterial =
            serde_json::from_str(r#"{ "id": "c1", "brightness": 0.1 }"#).expect("old file opens");
        assert!(old.lut.is_none());

        // Intensity defaults to 1 when a hand-edited file leaves it off.
        let bare: ColorAdjustMaterial =
            serde_json::from_str(r#"{ "id": "c1", "lut": { "path": "/l.cube" } }"#).expect("opens");
        assert_eq!(bare.lut.as_ref().map(|l| l.intensity), Some(1.0));
    }

    /// A project referencing a LUT file that no longer exists must open —
    /// missing look, playable project — and say so as a warning, exactly the
    /// missing-media rule.
    #[test]
    fn a_missing_lut_file_is_a_warning_not_an_error() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut adjust = ColorAdjustMaterial::identity();
        adjust.lut = Some(LutRef {
            path: "/nonexistent/look.cube".into(),
            intensity: 1.0,
        });
        project.materials.color_adjusts.push(adjust);

        let issues = project.validate();
        let issue = issues
            .iter()
            .find(|i| i.message.contains("LUT file is missing"))
            .expect("the missing file is reported");
        assert_eq!(issue.severity, Severity::Warning);
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
