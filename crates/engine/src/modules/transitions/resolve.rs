//! Where a transition sits in time, and what the renderer must show inside it.
//!
//! Everything here is pure arithmetic on the document — no wgpu, no mutation,
//! no IO — for the same reason `render/layout.rs` is: this is where a
//! transition is most likely to be quietly wrong, and a bug you can only see by
//! looking at a rendered frame is a bug you will not find. Progress at the
//! boundaries, a one-frame transition, a transition longer than the clips it
//! joins: all of it is testable on a machine with no GPU.

use crate::modules::project::document::{
    MaterialKind, MaterialPool, Micros, Project, Segment, TimeRange, Track, TransitionMaterial,
};

/// One transition, resolved against the two segments it joins.
///
/// Holding borrows rather than ids is deliberate: everything a caller wants to
/// do next — build a `SourceRequest`, place a quad, draw a handle on the
/// timeline — needs the segments themselves, and looking them up again by id is
/// how two callers end up disagreeing about which pair a transition joins.
#[derive(Debug, Clone, Copy)]
pub struct TransitionSpan<'a> {
    pub material: &'a TransitionMaterial,
    /// The clip being left. Ends exactly at [`Self::cut`].
    pub from: &'a Segment,
    /// The clip being entered, and the one carrying the transition id.
    pub to: &'a Segment,
    /// Timeline instant the two clips meet at.
    pub cut: Micros,
    /// The stretch of timeline the effect covers, already clamped so that it
    /// cannot run past the far edge of either clip.
    pub window: TimeRange,
    materials: &'a MaterialPool,
}

/// One side of a transition at one instant.
#[derive(Debug, Clone, Copy)]
pub struct TransitionLayer<'a> {
    pub segment: &'a Segment,
    pub kind: MaterialKind,
    /// Instant inside the material. Outside the segment's own `source_range`
    /// for whichever half of the window the segment is not live in — that
    /// borrowing is the whole point — and clamped into the material's real
    /// extent so a provider is never asked for a negative timestamp.
    pub source_time: Micros,
}

/// A transition at one instant, ready to be rendered.
#[derive(Debug, Clone, Copy)]
pub struct TransitionInstant<'a> {
    pub material: &'a TransitionMaterial,
    pub window: TimeRange,
    /// Position in the window before easing, `0..1`. Kept because it is what a
    /// UI scrubbing the transition wants to show, and because a test that
    /// asserts on eased progress cannot tell an easing bug from an arithmetic
    /// one.
    pub linear: f32,
    /// What the shader gets: `linear` through the material's easing.
    pub progress: f32,
    pub from: TransitionLayer<'a>,
    pub to: TransitionLayer<'a>,
}

/// Map a timeline instant into a segment's source **without** requiring the
/// instant to be inside the segment.
///
/// `Segment::source_time_at` refuses anything outside `target_range`, which is
/// correct for compositing and useless here: borrowing frames from beyond a
/// trimmed boundary is exactly what a transition does. Same arithmetic, no
/// containment check.
pub fn extended_source_time(segment: &Segment, time: Micros) -> Micros {
    let offset = time - segment.target_range.start;
    segment.source_range.start + (offset as f64 * segment.speed as f64) as Micros
}

/// [`extended_source_time`] through the clip's speed curve when it has one;
/// past either end the curve's edge speed carries on.
pub fn extended_source_time_in(
    materials: &MaterialPool,
    segment: &Segment,
    time: Micros,
) -> Micros {
    materials
        .time_map(segment)
        .source_at(time - segment.target_range.start)
}

/// How long a material actually is, when that is knowable.
///
/// `None` for images and text, which have no extent — asking for one at any
/// instant is legal.
fn material_extent(materials: &MaterialPool, id: &str) -> Option<Micros> {
    if let Some(video) = materials.video(id) {
        return Some(video.duration);
    }
    if let Some(audio) = materials.audio(id) {
        return Some(audio.duration);
    }
    None
}

/// The window a transition of `duration` occupies between two clips.
///
/// Centred on the cut, then clamped so it cannot leave either clip. The clamp
/// is applied to each half independently, which makes an over-long transition
/// asymmetric rather than making it refuse to render — a trim can shorten a
/// clip under a transition that was legal when it was placed, and the frame
/// still has to come out.
///
/// `None` when the duration is not positive, which is not a window at all.
pub fn window_for(
    cut: Micros,
    duration: Micros,
    from: &Segment,
    to: &Segment,
) -> Option<TimeRange> {
    if duration <= 0 {
        return None;
    }
    let before = (duration / 2).min(from.target_range.duration.max(0));
    let after = (duration - duration / 2).min(to.target_range.duration.max(0));
    let total = before + after;
    (total > 0).then(|| TimeRange::new(cut - before, total))
}

/// The longest transition the two clips either side of `cut` can carry.
///
/// Half the effect eats into each clip, so the limit is twice the shorter of
/// them. Exposed because the UI wants to clamp a drag handle to it rather than
/// discovering the limit by having the edit rejected.
pub fn max_duration(from: &Segment, to: &Segment) -> Micros {
    2 * from
        .target_range
        .duration
        .min(to.target_range.duration)
        .max(0)
}

/// Position in `window`, `0` at its start and approaching `1` at its end.
///
/// `None` outside the window. The window is half-open like every other range in
/// this document, so an instant exactly at the end belongs to the next thing,
/// and progress is therefore never exactly `1` — `1` is the limit, not a value
/// any frame is rendered at.
pub fn linear_progress_at(window: TimeRange, time: Micros) -> Option<f32> {
    if window.duration <= 0 || !window.contains(time) {
        return None;
    }
    Some(((time - window.start) as f64 / window.duration as f64) as f32)
}

impl<'a> TransitionSpan<'a> {
    /// Progress before easing at `time`, or `None` outside the window.
    pub fn linear_progress(&self, time: Micros) -> Option<f32> {
        linear_progress_at(self.window, time)
    }

    /// Progress the shader sees at `time`.
    pub fn progress(&self, time: Micros) -> Option<f32> {
        self.linear_progress(time)
            .map(|t| self.material.easing.apply(t))
    }

    /// Whether `segment_id` is one of the two clips this transition joins.
    pub fn joins(&self, segment_id: &str) -> bool {
        self.from.id == segment_id || self.to.id == segment_id
    }

    /// Everything the renderer needs at `time`, or `None` outside the window.
    pub fn instant(&self, time: Micros) -> Option<TransitionInstant<'a>> {
        let linear = self.linear_progress(time)?;
        Some(TransitionInstant {
            material: self.material,
            window: self.window,
            linear,
            progress: self.material.easing.apply(linear),
            from: self.layer(self.from, time)?,
            to: self.layer(self.to, time)?,
        })
    }

    fn layer(&self, segment: &'a Segment, time: Micros) -> Option<TransitionLayer<'a>> {
        let kind = self.materials.kind_of(&segment.material_id)?;
        let mut source_time = extended_source_time_in(self.materials, segment, time).max(0);
        if let Some(extent) = material_extent(self.materials, &segment.material_id) {
            // The last microsecond, not the first one past the end: a decoder
            // asked for a timestamp at or beyond the duration has nothing to
            // hand back, and a frozen final frame is the intended fallback.
            source_time = source_time.min((extent - 1).max(0));
        }
        Some(TransitionLayer {
            segment,
            kind,
            source_time,
        })
    }
}

/// Every transition on `track`, in timeline order.
///
/// A transition id on a segment whose predecessor is missing or does not touch
/// it is skipped rather than returned: it describes a join that no longer
/// exists, so there is nothing to render. `validate::issues` reports it, which
/// is the right division — this function's job is to say what the frame looks
/// like, not to complain.
pub fn spans<'a>(track: &'a Track, materials: &'a MaterialPool) -> Vec<TransitionSpan<'a>> {
    let mut out = Vec::new();
    for pair in track.segments.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        let Some(material) = materials.transition_of(to) else {
            continue;
        };
        let cut = to.target_range.start;
        if from.target_range.end() != cut {
            continue;
        }
        let Some(window) = window_for(cut, material.duration, from, to) else {
            continue;
        };
        out.push(TransitionSpan {
            material,
            from,
            to,
            cut,
            window,
            materials,
        });
    }
    out
}

/// The transition covering `time` on `track`, if any.
///
/// At most one can, because a transition never reaches past the far edge of
/// either clip it joins and two transitions on the same track are separated by
/// at least one whole clip.
pub fn span_at<'a>(
    track: &'a Track,
    materials: &'a MaterialPool,
    time: Micros,
) -> Option<TransitionSpan<'a>> {
    spans(track, materials)
        .into_iter()
        .find(|span| span.window.contains(time))
}

/// The transition `segment` takes part in at `time`, if any.
///
/// This is the compositor's entry point. It is asked once per segment it was
/// already going to draw, and answering `Some` means "draw this pair through
/// the transition instead". Exactly one of the two clips contains any instant
/// of the window, so no frame ever gets the same transition twice.
pub fn instant_for<'a>(
    track: &'a Track,
    materials: &'a MaterialPool,
    segment: &Segment,
    time: Micros,
) -> Option<TransitionInstant<'a>> {
    let span = span_at(track, materials, time)?;
    span.joins(&segment.id)
        .then(|| span.instant(time))
        .flatten()
}

/// Every transition live at `time`, with the track it is on.
///
/// For the preview's overlay and for tests; the compositor uses
/// [`instant_for`], which does not allocate a vector per segment.
pub fn instants_at(project: &Project, time: Micros) -> Vec<(&Track, TransitionInstant<'_>)> {
    project
        .tracks
        .iter()
        .filter_map(|track| {
            let span = span_at(track, &project.materials, time)?;
            Some((track, span.instant(time)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Easing, TrackKind, TransitionKind, VideoMaterial,
    };
    use crate::modules::project::{new_id, Transform};

    /// Two abutting four-second clips off one ten-second file, cut at 4 s.
    fn cut_project() -> (Project, String, String, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "m".into(),
            path: "/nonexistent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });

        let mut track = Track::new(TrackKind::Video, "V1");
        let make = |target: TimeRange, source: TimeRange| Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: target,
            source_range: source,
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        // The second clip starts two seconds into the file, so it has a two
        // second head handle for a transition to borrow from.
        let left = make(TimeRange::new(0, 4_000_000), TimeRange::new(0, 4_000_000));
        let right = make(
            TimeRange::new(4_000_000, 4_000_000),
            TimeRange::new(2_000_000, 4_000_000),
        );
        let (left_id, right_id) = (left.id.clone(), right.id.clone());
        let track_id = track.id.clone();
        track.segments.push(left);
        track.segments.push(right);
        project.tracks.push(track);
        (project, track_id, left_id, right_id)
    }

    fn attach(project: &mut Project, segment_id: &str, material: TransitionMaterial) -> String {
        let id = material.id.clone();
        project.materials.transitions.push(material);
        project
            .segment_mut(segment_id)
            .unwrap()
            .extras
            .push(id.clone());
        id
    }

    #[test]
    fn window_is_centred_on_the_cut() {
        let (mut project, track_id, _, right_id) = cut_project();
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let spans = spans(track, &project.materials);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].cut, 4_000_000);
        assert_eq!(spans[0].window, TimeRange::new(3_500_000, 1_000_000));
    }

    #[test]
    fn progress_is_zero_at_the_start_and_never_reaches_one() {
        let window = TimeRange::new(3_500_000, 1_000_000);
        assert_eq!(linear_progress_at(window, 3_500_000), Some(0.0));
        assert_eq!(linear_progress_at(window, 4_000_000), Some(0.5));
        // The last instant inside a half-open window is one microsecond short
        // of the end, so progress approaches 1 without arriving.
        let last = linear_progress_at(window, 4_499_999).unwrap();
        assert!(last < 1.0 && last > 0.999_998, "{last}");
        // And the end itself belongs to whatever comes next.
        assert_eq!(linear_progress_at(window, 4_500_000), None);
        assert_eq!(linear_progress_at(window, 3_499_999), None);
    }

    #[test]
    fn a_one_frame_transition_still_has_a_window() {
        // 30 fps, one frame. The window is 33_333 µs, so the halves are 16_666
        // before the cut and 16_667 after: odd durations put the extra
        // microsecond on the incoming side rather than rounding the window away.
        let (mut project, track_id, _, right_id) = cut_project();
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 33_333);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 4_000_000).unwrap();
        assert_eq!(span.window, TimeRange::new(3_983_334, 33_333));
        assert_eq!(span.linear_progress(3_983_334), Some(0.0));
        assert!(span.linear_progress(4_016_666).unwrap() < 1.0);
        assert_eq!(span.linear_progress(4_016_667), None);
    }

    #[test]
    fn a_one_microsecond_transition_covers_exactly_the_cut() {
        // The degenerate case: half of it is nothing, so the whole window sits
        // on the incoming side and the outgoing clip is read one microsecond
        // past its own end.
        let (mut project, track_id, _, right_id) = cut_project();
        attach(
            &mut project,
            &right_id,
            TransitionMaterial::new(TransitionKind::Dissolve, 1),
        );

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 4_000_000).unwrap();
        assert_eq!(span.window, TimeRange::new(4_000_000, 1));
        assert_eq!(span.linear_progress(4_000_000), Some(0.0));
        let instant = span.instant(4_000_000).unwrap();
        assert_eq!(instant.from.source_time, 4_000_000);
        assert_eq!(instant.to.source_time, 2_000_000);
    }

    #[test]
    fn each_side_borrows_frames_from_beyond_its_own_boundary() {
        let (mut project, track_id, _, right_id) = cut_project();
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 3_500_000).unwrap();

        // Half a second before the cut: the outgoing clip is inside its own
        // range, the incoming one is half a second before its in point.
        let early = span.instant(3_500_000).unwrap();
        assert_eq!(early.from.source_time, 3_500_000);
        assert_eq!(early.to.source_time, 1_500_000);

        // Quarter of a second after: the outgoing clip is now past its own out
        // point, reading the tail handle.
        let late = span.instant(4_250_000).unwrap();
        assert_eq!(late.from.source_time, 4_250_000);
        assert_eq!(late.to.source_time, 2_250_000);
    }

    #[test]
    fn a_borrowed_frame_never_leaves_the_material() {
        // A clip that starts at the very beginning of its file has no head
        // handle, so the incoming side freezes on frame zero instead of asking
        // for a negative timestamp.
        let (mut project, track_id, _, right_id) = cut_project();
        {
            let segment = project.segment_mut(&right_id).unwrap();
            segment.source_range = TimeRange::new(0, 4_000_000);
        }
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 3_500_000).unwrap();
        assert_eq!(span.instant(3_500_000).unwrap().to.source_time, 0);
    }

    #[test]
    fn easing_shapes_the_progress_but_not_the_window() {
        let (mut project, track_id, _, right_id) = cut_project();
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::EaseIn;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 4_000_000).unwrap();
        assert_eq!(span.linear_progress(4_000_000), Some(0.5));
        // EaseIn is t², so the midpoint of the window is a quarter of the way
        // through the effect.
        assert_eq!(span.progress(4_000_000), Some(0.25));
        assert_eq!(span.progress(3_500_000), Some(0.0));
    }

    #[test]
    fn an_over_long_transition_is_clamped_into_the_clips_it_joins() {
        // Ten seconds of transition between two four-second clips: the window
        // fills both of them and no more.
        let (mut project, track_id, _, right_id) = cut_project();
        attach(
            &mut project,
            &right_id,
            TransitionMaterial::new(TransitionKind::Dissolve, 10_000_000),
        );

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 4_000_000).unwrap();
        assert_eq!(span.window, TimeRange::new(0, 8_000_000));
        assert_eq!(max_duration(span.from, span.to), 8_000_000);
    }

    #[test]
    fn a_transition_whose_neighbours_no_longer_touch_resolves_to_nothing() {
        let (mut project, track_id, left_id, right_id) = cut_project();
        attach(
            &mut project,
            &right_id,
            TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000),
        );
        // Trim the outgoing clip's tail so the two no longer meet.
        project.segment_mut(&left_id).unwrap().target_range.duration = 3_000_000;

        let track = project.track(&track_id).unwrap();
        assert!(spans(track, &project.materials).is_empty());
        assert!(span_at(track, &project.materials, 4_000_000).is_none());
    }

    #[test]
    fn only_the_clip_that_contains_the_instant_claims_the_transition() {
        let (mut project, track_id, _left_id, right_id) = cut_project();
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let left = track.segments[0].clone();
        let right = track.segments[1].clone();

        // Before the cut the outgoing clip is the one being drawn.
        assert!(instant_for(track, &project.materials, &left, 3_600_000).is_some());
        assert!(instant_for(track, &project.materials, &right, 3_600_000).is_some());
        // …but the compositor only ever asks about the segment that contains
        // the instant, and that is exactly one of them.
        assert!(left.target_range.contains(3_600_000));
        assert!(!right.target_range.contains(3_600_000));
        assert!(!left.target_range.contains(4_100_000));
        assert!(right.target_range.contains(4_100_000));

        assert_eq!(instants_at(&project, 3_600_000).len(), 1);
        assert_eq!(instants_at(&project, 2_000_000).len(), 0);
    }

    #[test]
    fn speed_scales_the_borrowed_source_instant() {
        let (mut project, track_id, left_id, right_id) = cut_project();
        project.segment_mut(&left_id).unwrap().speed = 2.0;
        let mut material = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        material.easing = Easing::Linear;
        attach(&mut project, &right_id, material);

        let track = project.track(&track_id).unwrap();
        let span = span_at(track, &project.materials, 4_250_000).unwrap();
        // A quarter second past the clip's own end, at 2x, is half a second of
        // source past where the clip stopped reading.
        assert_eq!(span.instant(4_250_000).unwrap().from.source_time, 8_500_000);
    }
}
