//! Edit commands: the only sanctioned way to mutate a `Project`.
//!
//! Each command stores both the new state and the state it replaces, so
//! [`EditCommand::invert`] can produce an exact reverse without re-deriving
//! anything. That is why, for example, `RemoveSegment` carries the whole
//! `Segment` and its index — undo has to put it back exactly where it was,
//! including its position in the segment order.
//!
//! Commands compose: a "split" is a `Composite` of one trim and one insert, so
//! it undoes in a single step even though it touches two things.

use serde::{Deserialize, Serialize};

use crate::modules::project::{
    source_duration_for, speed_slack, AnimatableProperty, Easing, Keyframe, KeyframeTrack, Marker,
    Micros, Project, Segment, TimeRange, Track, Transform, TransitionMaterial,
};
use crate::modules::transitions;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EditCommand {
    AddTrack {
        track: Track,
        index: usize,
    },
    RemoveTrack {
        track: Track,
        index: usize,
    },
    /// Move a lane to a new place in the track order.
    ///
    /// `to_index` is the track's position in the *resulting* list, which makes
    /// the inverse the same command with the indices swapped. Render order is
    /// derived from track order by [`reindex_render_order`], so reordering
    /// lanes is the whole of restacking the composite — no segment is touched.
    MoveTrack {
        track_id: String,
        from_index: usize,
        to_index: usize,
    },
    InsertSegment {
        track_id: String,
        segment: Segment,
        index: usize,
    },
    RemoveSegment {
        track_id: String,
        segment: Segment,
        index: usize,
    },
    /// Move a segment in time and/or to another track.
    MoveSegment {
        segment_id: String,
        from_track: String,
        to_track: String,
        from_start: Micros,
        to_start: Micros,
    },
    /// Change which part of the source a segment shows and how long it is.
    /// Used by both edge-drag trimming and slip edits.
    TrimSegment {
        segment_id: String,
        before_target: TimeRange,
        before_source: TimeRange,
        after_target: TimeRange,
        after_source: TimeRange,
    },
    SetTransform {
        segment_id: String,
        before: Transform,
        after: Transform,
    },
    SetSpeed {
        segment_id: String,
        before: f32,
        after: f32,
    },
    SetVolume {
        segment_id: String,
        before: f32,
        after: f32,
    },
    /// Mute, lock, hide and track volume in one variant.
    ///
    /// These four travel together because the UI toggles them from the same
    /// row of controls, and because a variant per flag would quadruple the
    /// enum for no gain — the payload is four bytes either way.
    SetTrackFlags {
        track_id: String,
        before: TrackFlags,
        after: TrackFlags,
    },
    /// Put a transition at the head of a segment.
    ///
    /// The payload is the whole `TransitionMaterial`, not its id, for the
    /// reason `RemoveSegment` carries the whole segment: undo has to put back
    /// exactly what was there, and an id cannot rebuild a colour, an easing and
    /// a direction. `segment_id` is the **incoming** clip — see
    /// `TransitionMaterial` for why that side owns it.
    AddTransition {
        segment_id: String,
        transition: TransitionMaterial,
    },
    RemoveTransition {
        segment_id: String,
        transition: TransitionMaterial,
    },
    /// Retime or restyle a transition in place. The id may not change.
    SetTransition {
        segment_id: String,
        before: TransitionMaterial,
        after: TransitionMaterial,
    },
    /// Put a keyframe on a property of a segment.
    ///
    /// Times are **relative to the segment start**, like every keyframe time in
    /// the document — move the clip and its animation moves with it. See
    /// `docs/architecture/project-format.md`.
    ///
    /// The property's `KeyframeTrack` is created if this is the first keyframe
    /// on it, and removed again when the last one goes, because "this property
    /// is animated" is "a track exists for it" on both sides of the IPC
    /// boundary. An empty track left behind shows the user a property as
    /// animated when it is not.
    AddKeyframe {
        segment_id: String,
        property: AnimatableProperty,
        keyframe: Keyframe,
    },
    RemoveKeyframe {
        segment_id: String,
        property: AnimatableProperty,
        keyframe: Keyframe,
    },
    /// Drag a keyframe: a new time, a new value, the same easing.
    ///
    /// `from_time == to_time` is the ordinary case of editing a value in place
    /// and is not special.
    MoveKeyframe {
        segment_id: String,
        property: AnimatableProperty,
        from_time: Micros,
        to_time: Micros,
        before_value: f32,
        after_value: f32,
    },
    SetKeyframeEasing {
        segment_id: String,
        property: AnimatableProperty,
        time: Micros,
        before: Easing,
        after: Easing,
    },
    /// Put a segment into a link group, or take it out of one.
    ///
    /// Linked segments move, trim, split and delete together — see
    /// [`mirror_linked_edits`] — and this is the only command that changes who
    /// is linked to whom. It is also the only thing that writes
    /// `MaterialPool::links`, which is what keeps that set exactly derivable
    /// from the segments: `after: Some(g)` registers `g` if it is new, and
    /// `after: None` unregisters it once the segment leaving was the last
    /// member. Both directions are therefore byte-exact inverses.
    ///
    /// One segment per command on purpose. Linking a pair is a `Composite` of
    /// two, which is one undo step and needs no new variant; a variant that
    /// took a list would have to decide what a partial failure means.
    SetLinkGroup {
        segment_id: String,
        before: Option<String>,
        after: Option<String>,
    },
    /// Put a marker on the ruler.
    ///
    /// The whole [`Marker`] travels, for the reason `RemoveSegment` carries the
    /// whole segment: undo has to put back exactly what was there — time, label
    /// and colour — and an id cannot rebuild any of them.
    AddMarker {
        marker: Marker,
    },
    RemoveMarker {
        marker: Marker,
    },
    /// Move, rename or recolour a marker in place. The id may not change.
    SetMarker {
        before: Marker,
        after: Marker,
    },
    /// Several commands that undo as one unit, applied in order.
    Composite {
        label: String,
        commands: Vec<EditCommand>,
    },
}

/// The per-track switches, snapshotted together.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackFlags {
    pub muted: bool,
    pub locked: bool,
    pub hidden: bool,
    pub volume: f32,
}

impl TrackFlags {
    pub fn of(track: &Track) -> Self {
        Self {
            muted: track.muted,
            locked: track.locked,
            hidden: track.hidden,
            volume: track.volume,
        }
    }

    fn apply_to(&self, track: &mut Track) {
        track.muted = self.muted;
        track.locked = self.locked;
        track.hidden = self.hidden;
        track.volume = self.volume.clamp(0.0, 4.0);
    }
}

pub type EditResult = Result<(), String>;

/// The range of speeds a command may set.
///
/// Not a taste judgement: `source_range.duration` is derived from the timeline
/// duration times the speed, and an unbounded factor overflows the `i64` a
/// duration is stored in — after which `TimeRange::end()` panics in a debug
/// build and wraps in a release one. A hundred times is far past anything an
/// editor offers, and a hundredth is a two-hour clip from a one-minute one.
const MIN_SPEED: f32 = 0.01;
const MAX_SPEED: f32 = 100.0;

/// Every value a command is about to write into the document passes one of
/// these. They exist because of what the alternative costs: a NaN reaches disk
/// as `null` — `serde_json` has no other way to write it — and the project
/// never opens again, having reported a successful save. Rejecting the edit
/// costs the user one gesture; accepting it costs them the session.
fn non_finite(field: &str) -> String {
    format!(
        "{field} must be a finite number — a project containing a NaN or an infinity saves as \
         null and never opens again"
    )
}

fn check_finite(field: &str, value: f32) -> EditResult {
    if value.is_finite() {
        Ok(())
    } else {
        Err(non_finite(field))
    }
}

fn check_transform(transform: &Transform) -> EditResult {
    match transform.non_finite_field() {
        Some(field) => Err(non_finite(field)),
        None => Ok(()),
    }
}

fn check_speed(speed: f32) -> EditResult {
    check_finite("speed", speed)?;
    if speed <= 0.0 {
        return Err("speed must be positive".into());
    }
    if !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
        return Err(format!(
            "speed must be between {MIN_SPEED}x and {MAX_SPEED}x"
        ));
    }
    Ok(())
}

/// The range checks `validate()` would otherwise only discover after the fact.
fn check_ranges(target: TimeRange, source: TimeRange) -> EditResult {
    if target.duration <= 0 {
        return Err("a clip must be longer than nothing".into());
    }
    if target.start < 0 {
        return Err("a clip cannot start before the beginning of the timeline".into());
    }
    if source.duration <= 0 {
        return Err("a clip must show some of its material".into());
    }
    if source.start < 0 {
        return Err("a clip cannot read from before the start of its material".into());
    }
    Ok(())
}

/// Whether `source` is the range `target` at `speed` actually reads.
///
/// The two are allowed to disagree by [`speed_slack`], which is the rounding
/// every derivation of one from the other costs, and by nothing more. A caller
/// that is out by more than that has done the arithmetic wrong, and the
/// document would go on to render, mix and export a different piece of the
/// file than the one the user trimmed.
fn check_speed_invariant(target: TimeRange, source: TimeRange, speed: f32) -> EditResult {
    let implied = source_duration_for(target.duration, speed);
    if (source.duration - implied).abs() <= speed_slack(speed) {
        Ok(())
    } else {
        Err(format!(
            "this edit would leave the clip showing {} µs of material for {} µs of timeline, \
             but at {speed}x speed that is {} µs of material",
            source.duration, target.duration, implied
        ))
    }
}

/// Snap a source duration onto the exact value its timeline duration and speed
/// imply, once [`check_speed_invariant`] has agreed it is already within a
/// rounding step. Without this the document accumulates the microsecond each
/// caller rounded differently, and undo stops being byte-exact.
fn exact_source(target: TimeRange, source: TimeRange, speed: f32) -> TimeRange {
    TimeRange::new(source.start, source_duration_for(target.duration, speed))
}

/// Where a keyframe may sit.
///
/// Times are relative to the segment start, so the only hard rule is that they
/// are not negative — there is no such thing as a moment before the clip
/// begins.
///
/// Notably **not** a rule: that the time is inside the clip's duration. Trimming
/// a tail legitimately leaves keyframes beyond the new end, and the document
/// keeps them on purpose so that undoing the trim brings the animation back
/// (`project-format.md`: "trim its head and the animation stays attached to the
/// frames it was authored against"). Refusing them here would mean the undo of a
/// perfectly ordinary delete could fail. `validate()` reports them as a warning,
/// which is what the UI shows.
fn check_keyframe_time(time: Micros) -> EditResult {
    if time < 0 {
        return Err("a keyframe cannot sit before the start of its clip".into());
    }
    Ok(())
}

/// Everything that must be true of a segment before it enters the document.
fn check_segment(segment: &Segment) -> EditResult {
    if let Some(field) = segment.non_finite_field() {
        return Err(non_finite(field));
    }
    check_speed(segment.speed)?;
    check_finite("volume", segment.volume)?;
    check_ranges(segment.target_range, segment.source_range)?;
    check_speed_invariant(segment.target_range, segment.source_range, segment.speed)
}

impl EditCommand {
    /// Human-readable label for the undo menu.
    pub fn label(&self) -> String {
        match self {
            EditCommand::AddTrack { .. } => "Add track".into(),
            EditCommand::RemoveTrack { .. } => "Delete track".into(),
            EditCommand::MoveTrack { .. } => "Reorder tracks".into(),
            EditCommand::InsertSegment { .. } => "Add clip".into(),
            EditCommand::RemoveSegment { .. } => "Delete clip".into(),
            EditCommand::MoveSegment { .. } => "Move clip".into(),
            EditCommand::TrimSegment { .. } => "Trim clip".into(),
            EditCommand::SetTransform { .. } => "Transform clip".into(),
            EditCommand::SetSpeed { .. } => "Change speed".into(),
            EditCommand::SetVolume { .. } => "Change volume".into(),
            EditCommand::SetTrackFlags { .. } => "Change track".into(),
            EditCommand::AddTransition { .. } => "Add transition".into(),
            EditCommand::RemoveTransition { .. } => "Remove transition".into(),
            EditCommand::SetTransition { before, after, .. } => {
                // Retiming is the common gesture and deserves its own label in
                // the undo menu; anything else is a parameter change.
                if before.duration != after.duration && before.kind == after.kind {
                    "Retime transition".into()
                } else {
                    "Change transition".into()
                }
            }
            EditCommand::AddKeyframe { .. } => "Add keyframe".into(),
            EditCommand::RemoveKeyframe { .. } => "Delete keyframe".into(),
            EditCommand::MoveKeyframe { .. } => "Move keyframe".into(),
            EditCommand::SetKeyframeEasing { .. } => "Change easing".into(),
            EditCommand::SetLinkGroup { after, .. } => {
                if after.is_some() {
                    "Link clips".into()
                } else {
                    "Unlink clips".into()
                }
            }
            EditCommand::AddMarker { .. } => "Add marker".into(),
            EditCommand::RemoveMarker { .. } => "Delete marker".into(),
            EditCommand::SetMarker { before, after } => {
                // Dragging is the common gesture and deserves its own label;
                // anything else changed the name or the colour.
                if before.time != after.time
                    && before.label == after.label
                    && before.color == after.color
                {
                    "Move marker".into()
                } else {
                    "Edit marker".into()
                }
            }
            EditCommand::Composite { label, .. } => label.clone(),
        }
    }

    pub fn apply(&self, project: &mut Project) -> EditResult {
        match self {
            EditCommand::AddTrack { track, index } => {
                // Two lanes with one id makes `track_mut` return whichever
                // comes first, so every later edit aimed at the other one
                // silently lands on this.
                if project.track(&track.id).is_some() {
                    return Err(format!("a track with the id {} is already open", track.id));
                }
                if !track.volume.is_finite() {
                    return Err(non_finite("track volume"));
                }
                // Refused rather than clamped. `RemoveTrack` now checks that
                // the track it names is at the index it names, so an add that
                // quietly landed somewhere other than where it was asked to go
                // would produce a delete that cannot undo it.
                if *index > project.tracks.len() {
                    return Err("there is no such place on the timeline for a track".into());
                }
                project.tracks.insert(*index, track.clone());
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::RemoveTrack { track, index } => {
                // The index is where the track *was* when the gesture started.
                // Deleting whatever is there now is how a stale toolbar, a
                // slow round trip, or an undo in another panel deletes the
                // wrong lane — and the command carries the track it meant, so
                // there is no reason to guess.
                let found = project
                    .tracks
                    .get(*index)
                    .ok_or("that track is no longer on the timeline")?;
                if found.id != track.id {
                    return Err(format!(
                        "the track \"{}\" is no longer where this edit expected it; the timeline \
                         changed underneath it",
                        track.name
                    ));
                }
                project.tracks.remove(*index);
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::MoveTrack {
                track_id,
                from_index,
                to_index,
            } => {
                // Same guard as `RemoveTrack`: the index is where the track was
                // when the gesture started, and moving whatever is there *now*
                // would reorder someone else's lane and invert incorrectly.
                let found = project
                    .tracks
                    .get(*from_index)
                    .ok_or("that track is no longer on the timeline")?;
                if found.id != *track_id {
                    return Err(format!(
                        "the track \"{}\" is no longer where this edit expected it; the timeline \
                         changed underneath it",
                        found.name
                    ));
                }
                if *to_index >= project.tracks.len() {
                    return Err("there is no such place on the timeline for a track".into());
                }
                let track = project.tracks.remove(*from_index);
                project.tracks.insert(*to_index, track);
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::InsertSegment {
                track_id,
                segment,
                index,
            } => {
                check_segment(segment)?;
                if project.segment(&segment.id).is_some() {
                    return Err(format!(
                        "a clip with the id {} is already on the timeline",
                        segment.id
                    ));
                }
                let track = project
                    .track_mut(track_id)
                    .ok_or_else(|| format!("unknown track {track_id}"))?;
                if !track.is_range_free(&segment.target_range, None) {
                    return Err("target range is occupied".into());
                }
                let mut segment = segment.clone();
                segment.source_range =
                    exact_source(segment.target_range, segment.source_range, segment.speed);
                let index = (*index).min(track.segments.len());
                track.segments.insert(index, segment);
                sort_track(project, track_id);
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::RemoveSegment {
                track_id, segment, ..
            } => {
                let track = project
                    .track_mut(track_id)
                    .ok_or_else(|| format!("unknown track {track_id}"))?;
                let pos = track
                    .segments
                    .iter()
                    .position(|s| s.id == segment.id)
                    .ok_or_else(|| format!("unknown segment {}", segment.id))?;
                track.segments.remove(pos);
                Ok(())
            }

            EditCommand::MoveSegment {
                segment_id,
                from_track,
                to_track,
                to_start,
                ..
            } => {
                if *to_start < 0 {
                    return Err("a clip cannot start before the beginning of the timeline".into());
                }
                let source = project
                    .track_mut(from_track)
                    .ok_or_else(|| format!("unknown track {from_track}"))?;
                let pos = source
                    .segments
                    .iter()
                    .position(|s| s.id == *segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                let mut segment = source.segments.remove(pos);
                let new_range = TimeRange::new(*to_start, segment.target_range.duration);

                let dest = match project.track_mut(to_track) {
                    Some(t) => t,
                    None => {
                        // Put it back before failing so a bad move cannot eat a clip.
                        let source = project.track_mut(from_track).expect("track existed");
                        source.segments.insert(pos, segment);
                        return Err(format!("unknown track {to_track}"));
                    }
                };

                if !dest.is_range_free(&new_range, Some(segment_id)) {
                    let source = project.track_mut(from_track).expect("track existed");
                    source.segments.insert(pos, segment);
                    return Err("target range is occupied".into());
                }

                segment.target_range = new_range;
                dest.segments.push(segment);
                sort_track(project, to_track);
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::TrimSegment {
                segment_id,
                after_target,
                after_source,
                ..
            } => {
                check_ranges(*after_target, *after_source)?;
                let (track_id, speed) = project
                    .segment(segment_id)
                    .map(|(t, s)| (t.id.clone(), s.speed))
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;

                // A trim moves both ranges, and how far each moves depends on
                // the speed. A caller that trimmed the timeline range without
                // scaling the source range is asking for a clip that shows the
                // wrong part of the file, so it is refused rather than repaired.
                check_speed_invariant(*after_target, *after_source, speed)?;

                let track = project.track_mut(&track_id).expect("track existed");
                if !track.is_range_free(after_target, Some(segment_id)) {
                    return Err("trim would overlap a neighbouring clip".into());
                }

                let segment = project.segment_mut(segment_id).expect("segment existed");
                segment.target_range = *after_target;
                segment.source_range = exact_source(*after_target, *after_source, speed);
                sort_track(project, &track_id);
                Ok(())
            }

            EditCommand::SetTransform {
                segment_id, after, ..
            } => {
                check_transform(after)?;
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.transform = *after;
                Ok(())
            }

            EditCommand::SetSpeed {
                segment_id, after, ..
            } => {
                check_speed(*after)?;
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.speed = *after;
                // The speed is the factor *between* the two ranges, so setting
                // it alone leaves the document contradicting itself: the clip
                // keeps its place and length on the timeline, so the source
                // range is what has to change. Without this, `split_at` cuts a
                // sped-up clip in the wrong place, and the mixer and the
                // exporter — both of which derive the source span from the
                // speed — read a different part of the file than the preview
                // showed.
                segment.source_range = TimeRange::new(
                    segment.source_range.start,
                    source_duration_for(segment.target_range.duration, *after),
                );
                Ok(())
            }

            EditCommand::SetVolume {
                segment_id, after, ..
            } => {
                check_finite("volume", *after)?;
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.volume = after.clamp(0.0, 4.0);
                Ok(())
            }

            EditCommand::SetTrackFlags {
                track_id, after, ..
            } => {
                check_finite("track volume", after.volume)?;
                let track = project
                    .track_mut(track_id)
                    .ok_or_else(|| format!("unknown track {track_id}"))?;
                after.apply_to(track);
                Ok(())
            }

            // The bodies live in `transitions::edit` because the rules about
            // what makes a transition placeable belong next to the rules about
            // what makes it render. This table stays a table of contents.
            EditCommand::AddTransition {
                segment_id,
                transition,
            } => transitions::edit::add(project, segment_id, transition),

            EditCommand::RemoveTransition {
                segment_id,
                transition,
            } => transitions::edit::remove(project, segment_id, transition),

            EditCommand::SetTransition {
                segment_id,
                before,
                after,
            } => transitions::edit::set(project, segment_id, before, after),

            EditCommand::AddKeyframe {
                segment_id,
                property,
                keyframe,
            } => {
                check_finite("keyframe value", keyframe.value)?;
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                check_keyframe_time(keyframe.time)?;

                let index = match segment.keyframes.iter().position(|t| t.property == *property) {
                    Some(index) => index,
                    None => {
                        // Tracks are kept in a canonical order rather than in
                        // the order the user happened to animate things. The
                        // order carries no meaning, and pinning it is what
                        // makes removing the last keyframe of a property and
                        // undoing it put the track back exactly where it was —
                        // otherwise undo is one keystroke away from rewriting
                        // the file.
                        let at = segment
                            .keyframes
                            .partition_point(|t| t.property < *property);
                        segment.keyframes.insert(
                            at,
                            KeyframeTrack {
                                property: *property,
                                keyframes: Vec::new(),
                            },
                        );
                        at
                    }
                };

                let track = &mut segment.keyframes[index];
                if track.keyframes.iter().any(|k| k.time == keyframe.time) {
                    return Err(
                        "there is already a keyframe at that point; drag it instead".into()
                    );
                }
                let at = track.keyframes.partition_point(|k| k.time < keyframe.time);
                track.keyframes.insert(at, *keyframe);
                Ok(())
            }

            EditCommand::RemoveKeyframe {
                segment_id,
                property,
                keyframe,
            } => {
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                let index = segment
                    .keyframes
                    .iter()
                    .position(|t| t.property == *property)
                    .ok_or("that property is not animated")?;
                let track = &mut segment.keyframes[index];
                let at = track
                    .keyframes
                    .iter()
                    .position(|k| k.time == keyframe.time)
                    .ok_or("there is no keyframe there")?;
                track.keyframes.remove(at);
                if track.keyframes.is_empty() {
                    segment.keyframes.remove(index);
                }
                Ok(())
            }

            EditCommand::MoveKeyframe {
                segment_id,
                property,
                from_time,
                to_time,
                after_value,
                ..
            } => {
                check_finite("keyframe value", *after_value)?;
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                check_keyframe_time(*to_time)?;

                let track = segment
                    .keyframes
                    .iter_mut()
                    .find(|t| t.property == *property)
                    .ok_or("that property is not animated")?;
                if to_time != from_time && track.keyframes.iter().any(|k| k.time == *to_time) {
                    return Err("there is already a keyframe at that point".into());
                }
                let at = track
                    .keyframes
                    .iter()
                    .position(|k| k.time == *from_time)
                    .ok_or("there is no keyframe there")?;
                track.keyframes[at].time = *to_time;
                track.keyframes[at].value = *after_value;
                // Times are unique within a track, so this is a total order and
                // the result does not depend on which sort ran.
                track.keyframes.sort_by_key(|k| k.time);
                Ok(())
            }

            EditCommand::SetKeyframeEasing {
                segment_id,
                property,
                time,
                after,
                ..
            } => {
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                let track = segment
                    .keyframes
                    .iter_mut()
                    .find(|t| t.property == *property)
                    .ok_or("that property is not animated")?;
                let keyframe = track
                    .keyframes
                    .iter_mut()
                    .find(|k| k.time == *time)
                    .ok_or("there is no keyframe there")?;
                keyframe.easing = *after;
                Ok(())
            }

            EditCommand::SetLinkGroup {
                segment_id,
                before,
                after,
            } => {
                // The `before` is checked rather than trusted. A stale unlink —
                // the panel's copy of the document is one edit behind, the user
                // clicks Unlink — would otherwise strip a group the clip joined
                // in the meantime, and its inverse would put the *old* group
                // back, which is a link between two clips that were never
                // linked.
                let known: Vec<String> = project
                    .segment(segment_id)
                    .map(|(_, segment)| {
                        segment
                            .extras
                            .iter()
                            .filter(|id| project.materials.links.contains(*id))
                            .cloned()
                            .collect()
                    })
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                if known.first() != before.as_ref() {
                    return Err(
                        "this clip's links changed underneath the edit; try it again".into()
                    );
                }

                if let Some(group) = after {
                    project.materials.links.insert(group.clone());
                }
                let group = after.clone();
                let segment = project.segment_mut(segment_id).expect("segment existed");
                segment.extras.retain(|id| !known.contains(id));
                if let Some(group) = group {
                    segment.extras.push(group);
                }

                // A group nothing points at any more goes with the last member
                // that left, so that this command and its inverse are exact
                // opposites down to the bytes on disk. Note the asymmetry with
                // `RemoveSegment`, which deliberately does *not* prune: the
                // segment it took away is coming back on undo still carrying
                // the id.
                if let Some(left) = before {
                    if project.link_members(left).is_empty() {
                        project.materials.links.remove(left);
                    }
                }
                Ok(())
            }

            EditCommand::AddMarker { marker } => {
                if marker.time < 0 {
                    return Err("a marker cannot sit before the beginning of the timeline".into());
                }
                if project.markers.iter().any(|m| m.id == marker.id) {
                    return Err(format!("a marker with the id {} already exists", marker.id));
                }
                project.markers.push(marker.clone());
                sort_markers(project);
                Ok(())
            }

            EditCommand::RemoveMarker { marker } => {
                // The command carries what the panel believed it was deleting;
                // if the marker moved or was renamed underneath a stale menu,
                // deleting it anyway would make the inverse restore the *old*
                // marker — a silent revert of an edit the user made.
                let position = project
                    .markers
                    .iter()
                    .position(|m| m.id == marker.id)
                    .ok_or("that marker is no longer on the timeline")?;
                if project.markers[position] != *marker {
                    return Err("this marker changed underneath the edit; try it again".into());
                }
                project.markers.remove(position);
                Ok(())
            }

            EditCommand::SetMarker { before, after } => {
                if before.id != after.id {
                    return Err("a marker keeps its identity through an edit".into());
                }
                if after.time < 0 {
                    return Err("a marker cannot sit before the beginning of the timeline".into());
                }
                let current = project
                    .markers
                    .iter_mut()
                    .find(|m| m.id == before.id)
                    .ok_or("that marker is no longer on the timeline")?;
                // Same stale check as the remove, same reason: the inverse
                // writes `before` back, and that is only an undo if `before`
                // is what was actually there.
                if *current != *before {
                    return Err("this marker changed underneath the edit; try it again".into());
                }
                *current = after.clone();
                sort_markers(project);
                Ok(())
            }

            EditCommand::Composite { commands, .. } => {
                for (i, cmd) in commands.iter().enumerate() {
                    if let Err(e) = cmd.apply(project) {
                        // Roll back what already applied so a failed composite
                        // leaves the document untouched.
                        for done in commands[..i].iter().rev() {
                            let _ = done.invert().apply(project);
                        }
                        return Err(e);
                    }
                }
                Ok(())
            }
        }
    }

    /// The command that exactly undoes this one.
    pub fn invert(&self) -> EditCommand {
        match self {
            EditCommand::AddTrack { track, index } => EditCommand::RemoveTrack {
                track: track.clone(),
                index: *index,
            },
            EditCommand::RemoveTrack { track, index } => EditCommand::AddTrack {
                track: track.clone(),
                index: *index,
            },
            EditCommand::MoveTrack {
                track_id,
                from_index,
                to_index,
            } => EditCommand::MoveTrack {
                track_id: track_id.clone(),
                from_index: *to_index,
                to_index: *from_index,
            },
            EditCommand::InsertSegment {
                track_id,
                segment,
                index,
            } => EditCommand::RemoveSegment {
                track_id: track_id.clone(),
                segment: segment.clone(),
                index: *index,
            },
            EditCommand::RemoveSegment {
                track_id,
                segment,
                index,
            } => EditCommand::InsertSegment {
                track_id: track_id.clone(),
                segment: segment.clone(),
                index: *index,
            },
            EditCommand::MoveSegment {
                segment_id,
                from_track,
                to_track,
                from_start,
                to_start,
            } => EditCommand::MoveSegment {
                segment_id: segment_id.clone(),
                from_track: to_track.clone(),
                to_track: from_track.clone(),
                from_start: *to_start,
                to_start: *from_start,
            },
            EditCommand::TrimSegment {
                segment_id,
                before_target,
                before_source,
                after_target,
                after_source,
            } => EditCommand::TrimSegment {
                segment_id: segment_id.clone(),
                before_target: *after_target,
                before_source: *after_source,
                after_target: *before_target,
                after_source: *before_source,
            },
            EditCommand::SetTransform {
                segment_id,
                before,
                after,
            } => EditCommand::SetTransform {
                segment_id: segment_id.clone(),
                before: *after,
                after: *before,
            },
            EditCommand::SetSpeed {
                segment_id,
                before,
                after,
            } => EditCommand::SetSpeed {
                segment_id: segment_id.clone(),
                before: *after,
                after: *before,
            },
            EditCommand::SetVolume {
                segment_id,
                before,
                after,
            } => EditCommand::SetVolume {
                segment_id: segment_id.clone(),
                before: *after,
                after: *before,
            },
            EditCommand::SetTrackFlags {
                track_id,
                before,
                after,
            } => EditCommand::SetTrackFlags {
                track_id: track_id.clone(),
                before: *after,
                after: *before,
            },
            EditCommand::AddTransition {
                segment_id,
                transition,
            } => EditCommand::RemoveTransition {
                segment_id: segment_id.clone(),
                transition: transition.clone(),
            },
            EditCommand::RemoveTransition {
                segment_id,
                transition,
            } => EditCommand::AddTransition {
                segment_id: segment_id.clone(),
                transition: transition.clone(),
            },
            EditCommand::SetTransition {
                segment_id,
                before,
                after,
            } => EditCommand::SetTransition {
                segment_id: segment_id.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            EditCommand::AddKeyframe {
                segment_id,
                property,
                keyframe,
            } => EditCommand::RemoveKeyframe {
                segment_id: segment_id.clone(),
                property: *property,
                keyframe: *keyframe,
            },
            EditCommand::RemoveKeyframe {
                segment_id,
                property,
                keyframe,
            } => EditCommand::AddKeyframe {
                segment_id: segment_id.clone(),
                property: *property,
                keyframe: *keyframe,
            },
            EditCommand::MoveKeyframe {
                segment_id,
                property,
                from_time,
                to_time,
                before_value,
                after_value,
            } => EditCommand::MoveKeyframe {
                segment_id: segment_id.clone(),
                property: *property,
                from_time: *to_time,
                to_time: *from_time,
                before_value: *after_value,
                after_value: *before_value,
            },
            EditCommand::SetKeyframeEasing {
                segment_id,
                property,
                time,
                before,
                after,
            } => EditCommand::SetKeyframeEasing {
                segment_id: segment_id.clone(),
                property: *property,
                time: *time,
                before: *after,
                after: *before,
            },
            EditCommand::SetLinkGroup {
                segment_id,
                before,
                after,
            } => EditCommand::SetLinkGroup {
                segment_id: segment_id.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            EditCommand::AddMarker { marker } => EditCommand::RemoveMarker {
                marker: marker.clone(),
            },
            EditCommand::RemoveMarker { marker } => EditCommand::AddMarker {
                marker: marker.clone(),
            },
            EditCommand::SetMarker { before, after } => EditCommand::SetMarker {
                before: after.clone(),
                after: before.clone(),
            },
            EditCommand::Composite { label, commands } => EditCommand::Composite {
                label: label.clone(),
                // Undoing a composite means undoing its parts in reverse.
                commands: commands.iter().rev().map(EditCommand::invert).collect(),
            },
        }
    }
}

/// Build the composite that splits `segment_id` at timeline position `at`.
///
/// Split is not a primitive: it trims the original to end at the cut and
/// inserts a new segment covering the remainder, pointing at the matching
/// slice of the same material.
///
/// A **linked** clip splits with its partners, and the two new right-hand
/// halves become a link group of their own — otherwise the four segments would
/// all carry the original group and dragging the second half of the picture
/// would drag the first half of the sound with it. The whole thing is one
/// `Composite`, so it is one undo step.
pub fn split_at(project: &Project, segment_id: &str, at: Micros) -> Result<EditCommand, String> {
    let group = project.link_group_of(segment_id).cloned();

    // The named clip first, then its partners in document order, so the
    // composite a given cut expands into is always the same one.
    let mut targets = vec![segment_id.to_string()];
    if let Some(group) = &group {
        targets.extend(
            project
                .link_members(group)
                .into_iter()
                .map(|(_, _, segment)| segment.id.clone())
                .filter(|id| id != segment_id),
        );
    }

    let mut commands = Vec::new();
    let mut right_halves = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let split = match split_one(project, target, at) {
            Ok(split) => split,
            // The clip the user aimed at has to cut. A partner the cut misses —
            // only reachable from a hand-edited file, since a linked pair is
            // trimmed and moved as one — is left whole rather than taking the
            // gesture down with it.
            Err(error) if index == 0 => return Err(error),
            Err(_) => continue,
        };
        right_halves.push(split.right_id);
        commands.push(split.trim);
        commands.push(split.insert);
    }

    // Both halves of a cut pair need a group, and it has to be a new one: the
    // original stays on the two left halves, which are still each other's
    // partner.
    if right_halves.len() > 1 {
        let fresh = crate::modules::project::new_id();
        for right in right_halves {
            commands.push(EditCommand::SetLinkGroup {
                segment_id: right,
                before: None,
                after: Some(fresh.clone()),
            });
        }
    }

    Ok(EditCommand::Composite {
        label: "Split clip".into(),
        commands,
    })
}

/// One clip's half of a split: the trim that shortens it and the insert that
/// puts the remainder back.
struct SplitHalves {
    trim: EditCommand,
    insert: EditCommand,
    right_id: String,
}

fn split_one(project: &Project, segment_id: &str, at: Micros) -> Result<SplitHalves, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;

    if !segment.target_range.contains(at) || at == segment.target_range.start {
        return Err("split point is not inside the clip".into());
    }

    let left_duration = at - segment.target_range.start;
    let right_duration = segment.target_range.duration - left_duration;

    // How far into the *source* the cut lands is the timeline distance times
    // the speed — which is only the same as the timeline distance at 1x. Both
    // halves derive their source length from their own timeline length rather
    // than from what is left over, so each one satisfies the document's speed
    // invariant on its own terms.
    let source_split = source_duration_for(left_duration, segment.speed);

    let left_target = TimeRange::new(segment.target_range.start, left_duration);
    let left_source = TimeRange::new(segment.source_range.start, source_split);

    let mut right = segment.clone();
    right.id = crate::modules::project::new_id();
    right.target_range = TimeRange::new(at, right_duration);
    right.source_range = TimeRange::new(
        segment.source_range.start + source_split,
        source_duration_for(right_duration, segment.speed),
    );
    // The right half's left edge is a cut that did not exist a moment ago, so
    // whatever transition described how the original clip was *entered* belongs
    // to the left half, which kept that edge. Unconditional, because a freshly
    // cut edge never has one. This one line is the entire cost of splitting a
    // clip that carries a transition — see `TransitionMaterial` for why the
    // incoming clip owns it rather than the outgoing one.
    // The same argument applies to the link group, for a different reason: the
    // right half is a new clip, and if it kept the group it would be a third
    // member of a pair. The caller gives the right halves a group of their own
    // once it knows how many there are.
    right.extras.retain(|id| {
        project.materials.transition(id).is_none() && !project.materials.links.contains(id)
    });

    // The right half keeps only the animation that describes *its* frames.
    // Keyframe times are relative to the segment start, and until 2026-07 the
    // clone above copied them verbatim — so the right half replayed the whole
    // clip's animation from the cut onward: a fade-out authored for the tail
    // landed `left_duration` too late, and the head's keyframes existed twice.
    // Times at or after the cut shift left by the cut; times before it belong
    // to the left half, which keeps the original list untouched on purpose —
    // sampling inside the left half interpolates toward keyframes beyond its
    // new end exactly as the unsplit clip did, so it plays identically, and
    // `validate()` reports those keyframes as the same warning a tail trim
    // leaves. Where the cut lands mid-ramp, the right half gets an anchor at
    // its own start carrying the value the unsplit clip had at that instant,
    // so neither half's playback moves.
    rebase_keyframes_for_split(&mut right.keyframes, left_duration);

    let right_id = right.id.clone();
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment is on this track")
        + 1;

    Ok(SplitHalves {
        trim: EditCommand::TrimSegment {
            segment_id: segment_id.to_string(),
            before_target: segment.target_range,
            before_source: segment.source_range,
            after_target: left_target,
            after_source: left_source,
        },
        insert: EditCommand::InsertSegment {
            track_id: track.id.clone(),
            segment: right,
            index,
        },
        right_id,
    })
}

/// Rebase a freshly split right half's keyframes onto its own clock.
///
/// `cut` is the split point as an offset from the original segment's start —
/// the left half's duration. Everything at or after it moves left by it;
/// everything before it is dropped, because those instants now belong to the
/// left half. Two boundary cases keep the sound and picture at the cut exactly
/// what they were:
///
/// - A cut inside a ramp gets an **anchor** keyframe at time 0 holding the
///   interpolated value, with the easing of the interval it interrupted, so a
///   fade crossing the cut stays one continuous fade across the two clips.
/// - A track whose keyframes all sit before the cut collapses to a single
///   anchor: the unsplit clip held its last keyframe's value over that region
///   (the sampler clamps), and the right half must not snap back to 1.0.
fn rebase_keyframes_for_split(tracks: &mut Vec<KeyframeTrack>, cut: Micros) {
    for track in tracks.iter_mut() {
        let boundary = track.sample(cut);
        let governing = track
            .keyframes
            .iter()
            .rev()
            .find(|k| k.time < cut)
            .map(|k| k.easing);
        track.keyframes.retain(|k| k.time >= cut);
        for keyframe in track.keyframes.iter_mut() {
            keyframe.time -= cut;
        }
        let anchored = track.keyframes.first().is_some_and(|k| k.time == 0);
        if let (Some(value), Some(easing)) = (boundary, governing) {
            if !anchored {
                track.keyframes.insert(
                    0,
                    Keyframe {
                        time: 0,
                        value,
                        easing,
                    },
                );
            }
        }
    }
    // A property whose animation lived entirely on the left half is not
    // animated here at all, and an empty track shows it as animated.
    tracks.retain(|track| !track.keyframes.is_empty());
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// Wrap `command` so that everything linked to what it touches moves with it.
///
/// A clip imported with sound is two segments on two lanes, and the promise
/// they make to the user is that they behave like one: drag either and both
/// move, trim either and both trim, delete either and both go. The primitives
/// cannot do that themselves — `MoveSegment` names one segment and has nowhere
/// to put a second — so the *caller* adds the partners' commands and the whole
/// thing is one `Composite`. That is one undo step, and undoing it puts both
/// clips back.
///
/// This is the same shape as [`detach_broken_transitions`] and sits next to it
/// in `History::apply` for the same reason: it is the one place every edit
/// passes through, so no caller can forget.
///
/// Deliberately **not** recursive into a `Composite`. The two composites the
/// app builds — a split and an import — already know about links and place
/// their own; mirroring their parts as well would double every command in them.
pub fn mirror_linked_edits(project: &Project, command: EditCommand) -> EditCommand {
    let mirrored = mirrored_partners(project, &command);
    if mirrored.is_empty() {
        return command;
    }
    let label = command.label();
    let mut commands = vec![command];
    commands.extend(mirrored);
    EditCommand::Composite { label, commands }
}

/// The commands that carry `command` over to everything linked to what it
/// touches. Empty when nothing is linked, which is the common case.
///
/// Split out of [`mirror_linked_edits`] because [`compose_edits`] needs the
/// same expansion with different bookkeeping: a multi-clip edit has to mirror
/// each of its parts and then *drop* the mirrors that name a clip the edit
/// already moves itself.
fn mirrored_partners(project: &Project, command: &EditCommand) -> Vec<EditCommand> {
    if project.materials.links.is_empty() {
        return Vec::new();
    }
    match command {
        EditCommand::MoveSegment {
            segment_id,
            to_start,
            ..
        } => mirror_move(project, segment_id, *to_start),
        EditCommand::TrimSegment {
            segment_id,
            before_target,
            after_target,
            ..
        } => mirror_trim(project, segment_id, *before_target, *after_target),
        EditCommand::RemoveSegment { segment, .. } => mirror_remove(project, &segment.id),
        _ => Vec::new(),
    }
}

/// The clip a command is aimed at, for the commands that are aimed at one.
fn primary_segment(command: &EditCommand) -> Option<&str> {
    match command {
        EditCommand::MoveSegment { segment_id, .. }
        | EditCommand::TrimSegment { segment_id, .. }
        | EditCommand::SetTransform { segment_id, .. }
        | EditCommand::SetSpeed { segment_id, .. }
        | EditCommand::SetVolume { segment_id, .. }
        | EditCommand::SetLinkGroup { segment_id, .. } => Some(segment_id),
        EditCommand::RemoveSegment { segment, .. } | EditCommand::InsertSegment { segment, .. } => {
            Some(&segment.id)
        }
        _ => None,
    }
}

/// Fold one edit per clip into a single undo step.
///
/// This is what a multi-selection produces: the user drags four clips, or
/// deletes them, and that is *one* thing they did. The parts are ordinary
/// primitives, so nothing about how an edit applies or inverts changes — what
/// this adds is the two pieces of bookkeeping a batch needs and a single
/// command does not.
///
/// **Links are expanded here, exactly once.** `History::apply` mirrors a bare
/// command onto its link partners but deliberately does not recurse into a
/// `Composite` (see [`mirror_linked_edits`]), so a batch has to bring its own
/// partners — and it has to leave out any partner the batch already names. A
/// selection holding both halves of a linked pair would otherwise move the
/// sound twice: once because the user selected it, once because the picture
/// dragged it along.
///
/// **The parts are ordered so they do not trip over each other.** Segments on a
/// track may not overlap even for the instant between two commands of the same
/// composite, so a block of clips moving one second later has to be applied
/// right-to-left; the same block moving earlier, left-to-right. A composite
/// whose first part is refused rolls the whole batch back, which the user sees
/// as the drag having done nothing at all.
///
/// A batch of one is returned untouched rather than wrapped: it is not a batch,
/// and letting `History::apply` mirror it keeps single-clip editing on exactly
/// the path it has always taken.
pub fn compose_edits(
    project: &Project,
    label: &str,
    commands: Vec<EditCommand>,
) -> Result<EditCommand, String> {
    if commands.is_empty() {
        return Err("there is nothing to edit".into());
    }
    if commands.len() == 1 {
        return Ok(commands.into_iter().next().expect("one command"));
    }

    let ordered = order_for_apply(commands);

    // Every clip the batch names itself. A mirror that would land on one of
    // these is dropped: the batch is already moving it.
    let mut covered: std::collections::BTreeSet<String> = ordered
        .iter()
        .filter_map(primary_segment)
        .map(str::to_string)
        .collect();

    let mut out: Vec<EditCommand> = Vec::with_capacity(ordered.len());
    for command in ordered {
        let mirrored = mirrored_partners(project, &command);
        out.push(command);
        for partner in mirrored {
            let Some(id) = primary_segment(&partner).map(str::to_string) else {
                continue;
            };
            // `insert` answers whether this is the first time: a clip linked to
            // two of the batch's own clips is still only moved once.
            if covered.insert(id) {
                out.push(partner);
            }
        }
    }

    Ok(EditCommand::Composite {
        label: label.to_string(),
        commands: out,
    })
}

/// Put a batch in an order that no intermediate state rejects.
///
/// Only two shapes need it, and both for the same reason — a clip may not pass
/// through a neighbour, even momentarily:
///
/// - **Moves.** Whatever travels forward is applied from the right, whatever
///   travels backward from the left, so each clip's destination has been
///   vacated by the time it gets there.
/// - **Trims.** Every clip that gives space back goes before every clip that
///   takes space, so a block closing up does not have to grow into a gap that
///   is about to appear.
///
/// A mixed batch is left in the order the caller gave, because there is no
/// ordering that is right for one. The one mixed batch the app builds — a
/// ripple delete, one removal followed by leftward moves sorted left to right —
/// arrives already in the order that never overlaps, and relies on being left
/// alone here.
fn order_for_apply(mut commands: Vec<EditCommand>) -> Vec<EditCommand> {
    if commands
        .iter()
        .all(|c| matches!(c, EditCommand::MoveSegment { .. }))
    {
        commands.sort_by_key(|command| match command {
            EditCommand::MoveSegment {
                from_start,
                to_start,
                ..
            } if to_start >= from_start => (0i8, -*to_start),
            EditCommand::MoveSegment { to_start, .. } => (1i8, *to_start),
            _ => (2i8, 0),
        });
        return commands;
    }

    if commands
        .iter()
        .all(|c| matches!(c, EditCommand::TrimSegment { .. }))
    {
        commands.sort_by_key(|command| match command {
            EditCommand::TrimSegment {
                before_target,
                after_target,
                ..
            } => i8::from(after_target.duration > before_target.duration),
            _ => 2i8,
        });
    }

    commands
}

/// Everything linked to `segment_id` except `segment_id` itself.
fn link_partners<'a>(
    project: &'a Project,
    segment_id: &str,
) -> Vec<(&'a Track, usize, &'a Segment)> {
    let Some(group) = project.link_group_of(segment_id) else {
        return Vec::new();
    };
    project
        .link_members(group)
        .into_iter()
        .filter(|(_, _, segment)| segment.id != segment_id)
        .collect()
}

/// Partners travel by the same distance, and stay on their own lane.
///
/// The lane matters: dragging a clip from one video track to another must not
/// drag its sound onto a video track too. Only the *time* is shared.
///
/// The distance is measured from where the segment is now rather than from the
/// command's `from_start`, which is where the UI last saw it. Those differ
/// exactly when the document moved under a stale panel, and moving the partner
/// by a distance nobody dragged is worse than the move being refused.
fn mirror_move(project: &Project, segment_id: &str, to_start: Micros) -> Vec<EditCommand> {
    let Some((_, moved)) = project.segment(segment_id) else {
        return Vec::new();
    };
    let delta = to_start - moved.target_range.start;
    if delta == 0 {
        // A move between lanes at the same instant. The partner has nowhere to
        // go, and emitting a no-op move for it would only be one more command
        // that can fail.
        return Vec::new();
    }
    link_partners(project, segment_id)
        .into_iter()
        .map(|(track, _, partner)| EditCommand::MoveSegment {
            segment_id: partner.id.clone(),
            from_track: track.id.clone(),
            to_track: track.id.clone(),
            from_start: partner.target_range.start,
            to_start: partner.target_range.start + delta,
        })
        .collect()
}

/// Partners take the same movement at each edge.
///
/// Expressed as two deltas — how far the head moved and how far the tail moved
/// — rather than as the same absolute range, because linking is not only for a
/// clip and its own sound: two clips a user linked by hand may sit at different
/// places, and dragging one's tail must not teleport the other.
///
/// Each partner's *source* range follows at its own speed. A head trim of
/// 100 ms on a 2x clip consumes 200 ms of file, and a partner at 1x consumes
/// 100 ms; deriving each from its own speed is what keeps both sides of
/// `check_speed_invariant` happy.
fn mirror_trim(
    project: &Project,
    segment_id: &str,
    before_target: TimeRange,
    after_target: TimeRange,
) -> Vec<EditCommand> {
    let head = after_target.start - before_target.start;
    let tail = after_target.end() - before_target.end();
    if head == 0 && tail == 0 {
        return Vec::new();
    }
    link_partners(project, segment_id)
        .into_iter()
        .map(|(_, _, partner)| {
            let target = TimeRange::new(
                partner.target_range.start + head,
                partner.target_range.duration + tail - head,
            );
            let source = TimeRange::new(
                partner.source_range.start + source_duration_for(head, partner.speed),
                source_duration_for(target.duration, partner.speed),
            );
            EditCommand::TrimSegment {
                segment_id: partner.id.clone(),
                before_target: partner.target_range,
                before_source: partner.source_range,
                after_target: target,
                after_source: source,
            }
        })
        .collect()
}

/// Deleting one member deletes the group.
///
/// The whole `Segment` goes in the command, exactly as the primitive does it,
/// so undo puts the partner back where it was — link group and all — without
/// anything having to remember that it was linked.
fn mirror_remove(project: &Project, segment_id: &str) -> Vec<EditCommand> {
    link_partners(project, segment_id)
        .into_iter()
        .map(|(track, index, partner)| EditCommand::RemoveSegment {
            track_id: track.id.clone(),
            segment: partner.clone(),
            index,
        })
        .collect()
}

/// The composite that links `segment_ids` into one group.
///
/// Any segment already in a group leaves it first, which is the same command
/// in the other direction; the whole thing is one undo step.
pub fn link(project: &Project, segment_ids: &[String]) -> Result<EditCommand, String> {
    if segment_ids.len() < 2 {
        return Err("linking needs two clips".into());
    }
    let group = crate::modules::project::new_id();
    let mut commands = Vec::new();
    for segment_id in segment_ids {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        let before = project.materials.link_of(segment).cloned();
        if before.is_some() {
            commands.push(EditCommand::SetLinkGroup {
                segment_id: segment_id.clone(),
                before: before.clone(),
                after: None,
            });
        }
        commands.push(EditCommand::SetLinkGroup {
            segment_id: segment_id.clone(),
            before: None,
            after: Some(group.clone()),
        });
    }
    Ok(EditCommand::Composite {
        label: "Link clips".into(),
        commands,
    })
}

/// The composite that breaks the group `segment_id` belongs to.
///
/// Every member is released, not only the two the user can see: a group is one
/// thing, and leaving a clip linked to nothing would leave the timeline drawing
/// a link badge on a clip that has no partner.
pub fn unlink(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    let group = project
        .link_group_of(segment_id)
        .cloned()
        .ok_or("this clip is not linked to anything")?;
    let commands = project
        .link_members(&group)
        .into_iter()
        .map(|(_, _, segment)| EditCommand::SetLinkGroup {
            segment_id: segment.id.clone(),
            before: Some(group.clone()),
            after: None,
        })
        .collect();
    Ok(EditCommand::Composite {
        label: "Unlink clips".into(),
        commands,
    })
}

/// Wrap `command` so that it also removes any transition it is about to
/// orphan.
///
/// A transition is the one thing in the document that depends on two segments:
/// it is centred on a cut, and the cut is "the clip before this one ends
/// exactly where it starts". Move that clip, trim it, or delete it, and the
/// cut stops existing — `validate()` then calls the document inconsistent,
/// which it is, and the user has done nothing wrong. The fuzzer reaches it in
/// under two hundred random edits
/// (`tests/edit_fuzz.rs::transitions_survive_the_edits_that_move_the_clips_they_join`).
///
/// The primitive cannot fix itself: `MoveSegment` does not carry the
/// transition, so it could not put one back on undo. So the *caller* prepends
/// the removals, which is what `transitions::edit::detach_around` was written
/// for, and the whole thing is one `Composite` — one undo step, and undoing it
/// brings the transition back.
///
/// Which transitions those are is decided by applying the command to a copy
/// and looking, rather than by guessing from the command's shape: nudging a
/// clip while the cut survives must not delete anything, and that is the common
/// case.
pub fn detach_broken_transitions(project: &Project, command: EditCommand) -> EditCommand {
    // Only these can break a join. Checking first avoids cloning the document
    // for a volume change.
    let structural = matches!(
        command,
        EditCommand::MoveSegment { .. }
            | EditCommand::RemoveSegment { .. }
            | EditCommand::TrimSegment { .. }
            | EditCommand::RemoveTrack { .. }
            | EditCommand::Composite { .. }
    );
    if !structural || project.materials.transitions.is_empty() {
        return command;
    }

    let mut probe = project.clone();
    if command.apply(&mut probe).is_err() {
        // It will fail for the real document too, and a failed edit changes
        // nothing.
        return command;
    }

    // Only a transition that is *still attached* to a segment and has lost its
    // cut needs detaching. One whose segment went with the edit — a deleted
    // clip, a deleted lane — is already gone from the timeline, and the command
    // that removed it carries the segment, so undo puts both back together. A
    // detachment there would be applied to a document the removal command has
    // already snapshotted, and undo would restore the transition twice.
    let orphaned: Vec<String> = attached_transitions(project)
        .into_iter()
        .filter(|(_, id)| join_state(&probe, id) == Some(false))
        .map(|(segment_id, _)| segment_id)
        .collect();
    if orphaned.is_empty() {
        return command;
    }

    let label = command.label();
    let mut commands: Vec<EditCommand> = orphaned
        .iter()
        .filter_map(|segment_id| transitions::edit::remove_command(project, segment_id).ok())
        .collect();
    if commands.is_empty() {
        return command;
    }
    commands.push(command);
    EditCommand::Composite { label, commands }
}

/// `(segment id, transition id)` for every transition attached to a segment.
fn attached_transitions(project: &Project) -> Vec<(String, String)> {
    project
        .tracks
        .iter()
        .flat_map(|track| track.segments.iter())
        .flat_map(|segment| {
            segment.extras.iter().filter_map(move |extra| {
                project
                    .materials
                    .transition(extra)
                    .map(|t| (segment.id.clone(), t.id.clone()))
            })
        })
        .collect()
}

/// Whether `transition_id` is attached to a segment, and if so whether that
/// segment still has a clip ending exactly where it starts.
///
/// `None` means no segment carries it any more — which is not a broken document,
/// only a material in the pool that nothing points at.
fn join_state(project: &Project, transition_id: &str) -> Option<bool> {
    let mut attached = None;
    for track in &project.tracks {
        for (index, segment) in track.segments.iter().enumerate() {
            if !segment.extras.iter().any(|extra| extra == transition_id) {
                continue;
            }
            let joined = index
                .checked_sub(1)
                .and_then(|previous| track.segments.get(previous))
                .is_some_and(|previous| previous.target_range.end() == segment.target_range.start);
            // Any join is enough: a duplicated id is a different problem and
            // detaching it here would not be the fix.
            attached = Some(attached.unwrap_or(false) || joined);
        }
    }
    attached
}

/// Keep the marker list sorted by time, ties by id.
///
/// The id tie-break is what makes the order total: two markers at the same
/// instant would otherwise keep whichever order the edits happened to leave,
/// and a save of an unchanged project must be an unchanged file.
fn sort_markers(project: &mut Project) {
    project
        .markers
        .sort_by(|a, b| a.time.cmp(&b.time).then_with(|| a.id.cmp(&b.id)));
}

/// The composite that splits every unlocked clip under `at`, across tracks.
///
/// Built out of [`split_at`], once per *cluster*: a linked pair under the
/// playhead is one cluster — `split_at` already cuts the partners and gives the
/// right halves a fresh group — so its members are marked covered and not cut
/// a second time when their own lane comes up. The whole thing is one
/// `Composite`, one undo step.
///
/// A clip whose edge sits exactly at `at` has nothing to cut there and is
/// skipped rather than refused; locked lanes are skipped because a lock means
/// "this lane does not take edits". Only when *nothing* is cut does the whole
/// gesture refuse, so the user is told rather than shown an empty undo entry.
pub fn split_all_at(project: &Project, at: Micros) -> Result<EditCommand, String> {
    let mut covered: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut commands = Vec::new();

    for track in &project.tracks {
        if track.locked {
            continue;
        }
        let Some(segment) = track
            .segments
            .iter()
            .find(|s| s.target_range.contains(at) && at != s.target_range.start)
        else {
            continue;
        };
        if covered.contains(&segment.id) {
            continue;
        }
        covered.insert(segment.id.clone());
        if let Some(group) = project.link_group_of(&segment.id) {
            for (_, _, member) in project.link_members(group) {
                covered.insert(member.id.clone());
            }
        }

        // Every cluster is built against the same unchanged document, which is
        // sound because the clusters touch disjoint segments: at most one clip
        // per lane contains `at`, and link partners are claimed above.
        match split_at(project, &segment.id, at)? {
            EditCommand::Composite { commands: parts, .. } => commands.extend(parts),
            other => commands.push(other),
        }
    }

    if commands.is_empty() {
        return Err("nothing to split: no clip crosses the playhead".into());
    }
    Ok(EditCommand::Composite {
        label: "Split all tracks".into(),
        commands,
    })
}

/// Keep a track's segments sorted by start time.
fn sort_track(project: &mut Project, track_id: &str) {
    if let Some(track) = project.track_mut(track_id) {
        track.segments.sort_by_key(|s| s.target_range.start);
    }
}

/// Recompute `render_index` from track order: lower tracks paint first.
///
/// Doing this centrally means edit commands never have to think about z-order,
/// and reordering tracks is enough to restack the composite.
fn reindex_render_order(project: &mut Project) {
    for (track_idx, track) in project.tracks.iter_mut().enumerate() {
        for segment in track.segments.iter_mut() {
            segment.render_index = track_idx as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CanvasConfig, TrackKind, VideoMaterial};
    use crate::modules::timeline::History;

    fn project_with_clip() -> (Project, String, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let segment = Segment {
            id: crate::modules::project::new_id(),
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
        let segment_id = segment.id.clone();
        let track_id = track.id.clone();
        track.segments.push(segment);
        project.tracks.push(track);
        (project, track_id, segment_id)
    }

    #[test]
    fn split_produces_two_adjacent_clips() {
        let (mut project, track_id, segment_id) = project_with_clip();
        let cmd = split_at(&project, &segment_id, 1_000_000).unwrap();
        cmd.apply(&mut project).unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 2);
        assert_eq!(track.segments[0].target_range, TimeRange::new(0, 1_000_000));
        assert_eq!(
            track.segments[1].target_range,
            TimeRange::new(1_000_000, 3_000_000)
        );
        // The fixture's material is not in the pool, so validate() legitimately
        // complains about that. What matters here is that the split left the
        // track structurally sound.
        assert!(!project
            .validate()
            .iter()
            .any(|i| i.message.contains("overlap")));
    }

    #[test]
    fn undoing_a_split_restores_the_original() {
        let (mut project, track_id, segment_id) = project_with_clip();
        let cmd = split_at(&project, &segment_id, 1_000_000).unwrap();
        cmd.apply(&mut project).unwrap();
        cmd.invert().apply(&mut project).unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 1);
        assert_eq!(track.segments[0].target_range, TimeRange::new(0, 4_000_000));
        assert_eq!(track.segments[0].source_range, TimeRange::new(0, 4_000_000));
    }

    /// Structural errors, which is what an applied command must never leave
    /// behind. The fixture's material is not in the pool, so that one warning
    /// of a message is filtered out rather than the whole check dropped.
    fn errors(project: &Project) -> Vec<String> {
        project
            .validate()
            .into_iter()
            .filter(|i| i.severity == crate::modules::project::Severity::Error)
            .map(|i| i.message)
            .filter(|m| !m.contains("unknown material"))
            .collect()
    }

    #[test]
    fn changing_the_speed_moves_the_source_range_with_it() {
        // `project-format.md`: source_range.duration = target_range.duration ×
        // speed. Setting the speed alone left the document contradicting
        // itself, and nothing noticed until an export played the wrong frames.
        let (mut project, _, segment_id) = project_with_clip();

        EditCommand::SetSpeed {
            segment_id: segment_id.clone(),
            before: 1.0,
            after: 2.0,
        }
        .apply(&mut project)
        .unwrap();

        let (_, segment) = project.segment(&segment_id).unwrap();
        assert_eq!(
            segment.target_range,
            TimeRange::new(0, 4_000_000),
            "the clip keeps its place and length on the timeline"
        );
        assert_eq!(
            segment.source_range,
            TimeRange::new(0, 8_000_000),
            "and reads twice as much material to fill it"
        );
        assert!(segment.speed_invariant_holds());
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    #[test]
    fn undoing_a_speed_change_restores_both_ranges() {
        let (mut project, _, segment_id) = project_with_clip();
        let command = EditCommand::SetSpeed {
            segment_id: segment_id.clone(),
            before: 1.0,
            after: 2.5,
        };
        command.apply(&mut project).unwrap();
        command.invert().apply(&mut project).unwrap();

        let (_, segment) = project.segment(&segment_id).unwrap();
        assert_eq!(segment.speed, 1.0);
        assert_eq!(segment.source_range, TimeRange::new(0, 4_000_000));
    }

    #[test]
    fn splitting_a_sped_up_clip_cuts_the_source_at_the_right_place() {
        // The bug this pins: with the speed and the ranges out of step,
        // `split_at` computed the cut from a source range that described a
        // different clip, and the two halves showed overlapping material.
        let (mut project, track_id, segment_id) = project_with_clip();
        EditCommand::SetSpeed {
            segment_id: segment_id.clone(),
            before: 1.0,
            after: 2.0,
        }
        .apply(&mut project)
        .unwrap();

        split_at(&project, &segment_id, 1_000_000)
            .unwrap()
            .apply(&mut project)
            .unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 2);
        // One second of timeline at 2x is two seconds of source, so the second
        // half starts two seconds into the file and runs for six.
        assert_eq!(track.segments[0].target_range, TimeRange::new(0, 1_000_000));
        assert_eq!(track.segments[0].source_range, TimeRange::new(0, 2_000_000));
        assert_eq!(
            track.segments[1].target_range,
            TimeRange::new(1_000_000, 3_000_000)
        );
        assert_eq!(
            track.segments[1].source_range,
            TimeRange::new(2_000_000, 6_000_000)
        );

        // And the frame each half shows at the cut is the same frame the
        // unsplit clip showed there, which is the property a user would notice.
        assert_eq!(
            track.segments[1].source_time_at(1_000_000),
            Some(2_000_000)
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    #[test]
    fn a_trim_that_ignores_the_speed_is_refused() {
        let (mut project, _, segment_id) = project_with_clip();
        EditCommand::SetSpeed {
            segment_id: segment_id.clone(),
            before: 1.0,
            after: 2.0,
        }
        .apply(&mut project)
        .unwrap();
        let before = serde_json::to_string(&project).unwrap();

        // Halving the timeline range at 2x has to halve an *eight* second
        // source range, not a four second one.
        let error = EditCommand::TrimSegment {
            segment_id: segment_id.clone(),
            before_target: TimeRange::new(0, 4_000_000),
            before_source: TimeRange::new(0, 8_000_000),
            after_target: TimeRange::new(0, 2_000_000),
            after_source: TimeRange::new(0, 2_000_000),
        }
        .apply(&mut project)
        .expect_err("a trim that contradicts the speed must be refused");
        assert!(error.contains("2x speed"), "{error}");
        assert_eq!(serde_json::to_string(&project).unwrap(), before);

        // The same trim done properly is accepted.
        EditCommand::TrimSegment {
            segment_id,
            before_target: TimeRange::new(0, 4_000_000),
            before_source: TimeRange::new(0, 8_000_000),
            after_target: TimeRange::new(0, 2_000_000),
            after_source: TimeRange::new(0, 4_000_000),
        }
        .apply(&mut project)
        .expect("a trim that respects it is not");
    }

    #[test]
    fn splitting_a_clip_rebases_the_right_halfs_keyframes() {
        // The bug this pins: `split_one` used to clone the keyframes verbatim.
        // Times are relative to the segment start, so the right half replayed
        // the whole clip's animation from the cut onward — a fade-out authored
        // for the tail played `left_duration` too late, and the head's
        // keyframes existed twice.
        let (mut project, track_id, segment_id) = project_with_clip();

        // A fade-in over the first second, entirely on what becomes the left
        // half, and a fade-out over the last second, crossing the cut at 3.5s.
        for (time, value) in [(0, 0.0), (1_000_000, 1.0)] {
            add(&segment_id, AnimatableProperty::Volume, time, value)
                .apply(&mut project)
                .unwrap();
        }
        for (time, value) in [(3_000_000, 1.0), (4_000_000, 0.0)] {
            add(&segment_id, AnimatableProperty::Volume, time, value)
                .apply(&mut project)
                .unwrap();
        }
        let before = serde_json::to_string(&project).unwrap();

        let command = split_at(&project, &segment_id, 3_500_000).unwrap();
        command.apply(&mut project).unwrap();

        // The left half keeps its keyframes untouched — including the one now
        // past its end, which is what makes it play exactly as before: the
        // sampler interpolates toward it, so the fade-out still reaches 0.5 at
        // the cut.
        let left = project.segment(&segment_id).unwrap().1;
        let left_track = &left.keyframes[0];
        assert_eq!(left_track.property, AnimatableProperty::Volume);
        let times: Vec<Micros> = left_track.keyframes.iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0, 1_000_000, 3_000_000, 4_000_000]);

        // The right half's clock starts at the cut. The fade-out's remainder is
        // shifted onto it, anchored at its own start with the value the unsplit
        // clip had at that instant — so the fade is continuous across the cut.
        let track = project.track(&track_id).unwrap();
        let right = &track.segments[1];
        assert_eq!(right.keyframes.len(), 1, "one property is animated here");
        let right_track = &right.keyframes[0];
        let pairs: Vec<(Micros, f32)> = right_track
            .keyframes
            .iter()
            .map(|k| (k.time, k.value))
            .collect();
        assert_eq!(pairs, vec![(0, 0.5), (500_000, 0.0)]);

        // What a listener would check: the value at the cut and at the end are
        // the ones the unsplit clip had at those instants.
        assert_eq!(right_track.sample(0), Some(0.5));
        assert_eq!(right_track.sample(500_000), Some(0.0));

        // And the whole thing comes back on one undo.
        command.invert().apply(&mut project).unwrap();
        assert_eq!(serde_json::to_string(&project).unwrap(), before);
    }

    #[test]
    fn splitting_after_the_animation_holds_the_last_value_on_the_right_half() {
        // A fade-in that finished before the cut: the unsplit clip held 1.0
        // over the right region because the sampler clamps to the last
        // keyframe. Dropping the track entirely would be right by accident for
        // a value of 1.0 — so the fixture fades to 0.6, where the difference
        // between "anchored" and "forgotten" is audible.
        let (mut project, track_id, segment_id) = project_with_clip();
        for (time, value) in [(0, 0.0), (1_000_000, 0.6)] {
            add(&segment_id, AnimatableProperty::Volume, time, value)
                .apply(&mut project)
                .unwrap();
        }

        split_at(&project, &segment_id, 2_000_000)
            .unwrap()
            .apply(&mut project)
            .unwrap();

        let track = project.track(&track_id).unwrap();
        let right = &track.segments[1];
        let pairs: Vec<(Micros, f32)> = right.keyframes[0]
            .keyframes
            .iter()
            .map(|k| (k.time, k.value))
            .collect();
        assert_eq!(pairs, vec![(0, 0.6)], "a single anchor holds the value");
    }

    #[test]
    fn splitting_before_the_animation_leaves_the_right_half_clean() {
        // The mirror image: everything animated sits after the cut, so the
        // right half takes all of it, shifted, and needs no anchor.
        let (mut project, track_id, segment_id) = project_with_clip();
        for (time, value) in [(3_000_000, 1.0), (4_000_000, 0.0)] {
            add(&segment_id, AnimatableProperty::Volume, time, value)
                .apply(&mut project)
                .unwrap();
        }

        split_at(&project, &segment_id, 1_000_000)
            .unwrap()
            .apply(&mut project)
            .unwrap();

        let track = project.track(&track_id).unwrap();
        let right = &track.segments[1];
        let pairs: Vec<(Micros, f32)> = right.keyframes[0]
            .keyframes
            .iter()
            .map(|k| (k.time, k.value))
            .collect();
        assert_eq!(pairs, vec![(2_000_000, 1.0), (3_000_000, 0.0)]);
    }

    #[test]
    fn a_value_that_cannot_be_saved_is_refused_at_the_boundary() {
        // Each of these produced a project that saved successfully and then
        // failed to load forever after with "invalid type: null, expected f32".
        let (mut project, track_id, segment_id) = project_with_clip();

        let mut transform = Transform::default();
        transform.opacity = f32::NAN;
        let error = EditCommand::SetTransform {
            segment_id: segment_id.clone(),
            before: Transform::default(),
            after: transform,
        }
        .apply(&mut project)
        .expect_err("a NaN opacity must be refused");
        assert!(error.contains("opacity"), "{error}");

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(EditCommand::SetSpeed {
                segment_id: segment_id.clone(),
                before: 1.0,
                after: bad,
            }
            .apply(&mut project)
            .is_err());
            assert!(EditCommand::SetVolume {
                segment_id: segment_id.clone(),
                before: 1.0,
                after: bad,
            }
            .apply(&mut project)
            .is_err());
            assert!(EditCommand::SetTrackFlags {
                track_id: track_id.clone(),
                before: TrackFlags {
                    muted: false,
                    locked: false,
                    hidden: false,
                    volume: 1.0
                },
                after: TrackFlags {
                    muted: false,
                    locked: false,
                    hidden: false,
                    volume: bad
                },
            }
            .apply(&mut project)
            .is_err());
        }

        let mut poisoned = project.segment(&segment_id).unwrap().1.clone();
        poisoned.id = "poisoned".into();
        poisoned.target_range = TimeRange::new(10_000_000, 1_000_000);
        poisoned.source_range = TimeRange::new(0, 1_000_000);
        poisoned.transform.scale = [1.0, f32::INFINITY];
        assert!(EditCommand::InsertSegment {
            track_id,
            segment: poisoned,
            index: 1,
        }
        .apply(&mut project)
        .is_err());

        // Nothing that cannot be written back reached the document, so it
        // still survives a save and a load — which is the whole point. (The
        // file does contain `null`s: `crop` and `shadow` are genuinely
        // optional. What matters is that none of them sits where a number
        // belongs, and only a real parse can tell the difference.)
        let json = serde_json::to_string(&project).unwrap();
        serde_json::from_str::<Project>(&json).expect("the document still reopens");
    }

    #[test]
    fn a_speed_outside_the_supported_range_is_refused() {
        let (mut project, _, segment_id) = project_with_clip();
        for bad in [0.0, -1.0, 1e30, 0.0001] {
            assert!(
                EditCommand::SetSpeed {
                    segment_id: segment_id.clone(),
                    before: 1.0,
                    after: bad,
                }
                .apply(&mut project)
                .is_err(),
                "{bad}x was accepted"
            );
        }
    }

    #[test]
    fn removing_a_track_by_a_stale_index_does_not_delete_the_wrong_lane() {
        // The gesture said "delete the lane I am looking at". If the timeline
        // moved underneath it, deleting whatever is now at that index deletes
        // someone else's work and calls it a success.
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.tracks.push(Track::new(TrackKind::Video, "V1"));
        project.tracks.push(Track::new(TrackKind::Audio, "A1"));
        let audio = project.tracks[1].clone();

        let stale = EditCommand::RemoveTrack {
            track: audio.clone(),
            // The index it had before a lane was inserted above it.
            index: 0,
        };
        let error = stale.apply(&mut project).expect_err("a stale index refuses");
        assert!(error.contains("A1"), "the message names the lane: {error}");
        assert_eq!(project.tracks.len(), 2, "and nothing was deleted");
        assert_eq!(project.tracks[0].name, "V1");

        // With the right index it removes the lane it named.
        EditCommand::RemoveTrack {
            track: audio,
            index: 1,
        }
        .apply(&mut project)
        .unwrap();
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].name, "V1");
    }

    #[test]
    fn an_edit_that_would_break_a_document_invariant_is_refused() {
        let (mut project, track_id, segment_id) = project_with_clip();

        // A clip before the beginning of the timeline.
        assert!(EditCommand::MoveSegment {
            segment_id: segment_id.clone(),
            from_track: track_id.clone(),
            to_track: track_id.clone(),
            from_start: 0,
            to_start: -1_000_000,
        }
        .apply(&mut project)
        .is_err());

        // A clip reading from before the start of its material.
        assert!(EditCommand::TrimSegment {
            segment_id: segment_id.clone(),
            before_target: TimeRange::new(0, 4_000_000),
            before_source: TimeRange::new(0, 4_000_000),
            after_target: TimeRange::new(0, 4_000_000),
            after_source: TimeRange::new(-500_000, 4_000_000),
        }
        .apply(&mut project)
        .is_err());

        // A second clip with an id the document already uses: the two become
        // indistinguishable and every later edit hits whichever is first.
        let mut twin = project.segment(&segment_id).unwrap().1.clone();
        twin.target_range = TimeRange::new(10_000_000, 1_000_000);
        twin.source_range = TimeRange::new(0, 1_000_000);
        let error = EditCommand::InsertSegment {
            track_id: track_id.clone(),
            segment: twin,
            index: 1,
        }
        .apply(&mut project)
        .expect_err("a duplicate clip id must be refused");
        assert!(error.contains("already on the timeline"), "{error}");

        // And a second lane with an id the document already uses.
        let twin_track = project.track(&track_id).unwrap().clone();
        assert!(EditCommand::AddTrack {
            track: twin_track,
            index: 0,
        }
        .apply(&mut project)
        .is_err());

        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    fn keyframe(time: Micros, value: f32) -> Keyframe {
        Keyframe {
            time,
            value,
            easing: Easing::Linear,
        }
    }

    fn add(segment_id: &str, property: AnimatableProperty, time: Micros, value: f32) -> EditCommand {
        EditCommand::AddKeyframe {
            segment_id: segment_id.to_string(),
            property,
            keyframe: keyframe(time, value),
        }
    }

    #[test]
    fn keyframes_land_in_order_and_the_track_appears_and_disappears_with_them() {
        let (mut project, _, id) = project_with_clip();

        // Out of order on purpose: the sampler uses `partition_point`, so an
        // unsorted track does not look wrong, it interpolates the wrong pair.
        for (time, value) in [(2_000_000, 1.0), (0, 0.0), (1_000_000, 0.5)] {
            add(&id, AnimatableProperty::Opacity, time, value)
                .apply(&mut project)
                .unwrap();
        }

        let (_, segment) = project.segment(&id).unwrap();
        assert_eq!(segment.keyframes.len(), 1, "one track, for one property");
        let times: Vec<Micros> = segment.keyframes[0]
            .keyframes
            .iter()
            .map(|k| k.time)
            .collect();
        assert_eq!(times, vec![0, 1_000_000, 2_000_000]);
        assert_eq!(segment.keyframes[0].sample(500_000), Some(0.25));

        // A second keyframe at a time already taken is refused: the UI sends
        // MoveKeyframe for that.
        assert!(add(&id, AnimatableProperty::Opacity, 1_000_000, 0.9)
            .apply(&mut project)
            .is_err());

        // There is no moment before a clip starts.
        assert!(add(&id, AnimatableProperty::Opacity, -1, 1.0)
            .apply(&mut project)
            .is_err());

        // But one past the end is allowed, because trimming a tail produces
        // exactly that and the undo of a delete has to be able to put it back.
        // It is a warning, not an error: the user authored something that is no
        // longer playing.
        add(&id, AnimatableProperty::Opacity, 9_000_000, 1.0)
            .apply(&mut project)
            .expect("a keyframe past the end is kept, not refused");
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
        assert!(project
            .validate()
            .iter()
            .any(|i| i.severity == crate::modules::project::Severity::Warning
                && i.message.contains("outside the clip")));
        EditCommand::RemoveKeyframe {
            segment_id: id.clone(),
            property: AnimatableProperty::Opacity,
            keyframe: keyframe(9_000_000, 1.0),
        }
        .apply(&mut project)
        .unwrap();

        // Removing the last keyframe of a property takes the track with it,
        // because "is animated" is "has a track" on both sides.
        for time in [0, 1_000_000, 2_000_000] {
            EditCommand::RemoveKeyframe {
                segment_id: id.clone(),
                property: AnimatableProperty::Opacity,
                keyframe: keyframe(time, 0.0),
            }
            .apply(&mut project)
            .unwrap();
        }
        assert!(
            project.segment(&id).unwrap().1.keyframes.is_empty(),
            "an emptied track shows the property as animated when it is not"
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    #[test]
    fn undoing_a_keyframe_edit_restores_the_document_exactly() {
        let (mut project, _, id) = project_with_clip();
        // Two properties, so that a track put back in the wrong place shows up.
        for property in [
            AnimatableProperty::Opacity,
            AnimatableProperty::PositionX,
            AnimatableProperty::Rotation,
        ] {
            add(&id, property, 0, 0.0).apply(&mut project).unwrap();
            add(&id, property, 1_000_000, 1.0)
                .apply(&mut project)
                .unwrap();
        }
        // One property animated by a single keyframe, sitting in the middle of
        // the canonical order: removing it drops the whole track, and undo has
        // to put the track back where it was rather than at the end.
        add(&id, AnimatableProperty::ScaleX, 0, 1.0)
            .apply(&mut project)
            .unwrap();
        assert_eq!(project.segment(&id).unwrap().1.keyframes.len(), 4);
        let before = serde_json::to_string(&project).unwrap();

        for command in [
            add(&id, AnimatableProperty::Opacity, 500_000, 0.5),
            EditCommand::RemoveKeyframe {
                segment_id: id.clone(),
                property: AnimatableProperty::ScaleX,
                keyframe: keyframe(0, 1.0),
            },
            EditCommand::MoveKeyframe {
                segment_id: id.clone(),
                property: AnimatableProperty::Rotation,
                from_time: 1_000_000,
                to_time: 250_000,
                before_value: 1.0,
                after_value: 90.0,
            },
            // The ordinary "edit the value where it is" case.
            EditCommand::MoveKeyframe {
                segment_id: id.clone(),
                property: AnimatableProperty::Rotation,
                from_time: 0,
                to_time: 0,
                before_value: 0.0,
                after_value: 45.0,
            },
            EditCommand::SetKeyframeEasing {
                segment_id: id.clone(),
                property: AnimatableProperty::Opacity,
                time: 0,
                before: Easing::Linear,
                after: Easing::Hold,
            },
        ] {
            command
                .apply(&mut project)
                .unwrap_or_else(|e| panic!("{} was refused: {e}", command.label()));
            let after = serde_json::to_string(&project).unwrap();
            assert_ne!(after, before, "{} changed nothing", command.label());

            command.invert().apply(&mut project).unwrap();
            assert_eq!(
                serde_json::to_string(&project).unwrap(),
                before,
                "undoing {} did not restore the document",
                command.label()
            );

            command.apply(&mut project).unwrap();
            assert_eq!(
                serde_json::to_string(&project).unwrap(),
                after,
                "redoing {} produced a different document",
                command.label()
            );
            command.invert().apply(&mut project).unwrap();
        }
    }

    #[test]
    fn a_keyframe_edit_that_cannot_be_saved_or_has_nowhere_to_go_is_refused() {
        let (mut project, _, id) = project_with_clip();
        add(&id, AnimatableProperty::Opacity, 0, 0.0)
            .apply(&mut project)
            .unwrap();
        add(&id, AnimatableProperty::Opacity, 1_000_000, 1.0)
            .apply(&mut project)
            .unwrap();

        // A NaN keyframe value is the same data loss as a NaN opacity.
        assert!(add(&id, AnimatableProperty::Opacity, 500_000, f32::NAN)
            .apply(&mut project)
            .is_err());
        assert!(EditCommand::MoveKeyframe {
            segment_id: id.clone(),
            property: AnimatableProperty::Opacity,
            from_time: 0,
            to_time: 0,
            before_value: 0.0,
            after_value: f32::INFINITY,
        }
        .apply(&mut project)
        .is_err());

        // Moving onto an occupied time, and moving one that is not there.
        assert!(EditCommand::MoveKeyframe {
            segment_id: id.clone(),
            property: AnimatableProperty::Opacity,
            from_time: 0,
            to_time: 1_000_000,
            before_value: 0.0,
            after_value: 0.0,
        }
        .apply(&mut project)
        .is_err());
        assert!(EditCommand::MoveKeyframe {
            segment_id: id.clone(),
            property: AnimatableProperty::Opacity,
            from_time: 777,
            to_time: 500_000,
            before_value: 0.0,
            after_value: 0.0,
        }
        .apply(&mut project)
        .is_err());

        // And touching a property nothing has animated.
        assert!(EditCommand::SetKeyframeEasing {
            segment_id: id.clone(),
            property: AnimatableProperty::Volume,
            time: 0,
            before: Easing::Linear,
            after: Easing::Hold,
        }
        .apply(&mut project)
        .is_err());

        let json = serde_json::to_string(&project).unwrap();
        serde_json::from_str::<Project>(&json).expect("the document still reopens");
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    // -----------------------------------------------------------------------
    // Linked clips
    // -----------------------------------------------------------------------

    /// What an import of a file with both streams produces: the picture on a
    /// video lane, the sound on an audio lane, the same material under both,
    /// the same `target_range`, and one link group holding them together.
    struct Pair {
        project: Project,
        video_track: String,
        audio_track: String,
        video: String,
        audio: String,
        group: String,
    }

    fn linked_pair() -> Pair {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "m".into(),
            path: "/media/clip.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });

        let mut video_track = Track::new(TrackKind::Video, "V1");
        let mut audio_track = Track::new(TrackKind::Audio, "A1");
        // `render_index` is derived from track order and normalised by
        // `reindex_render_order` on the first structural edit — which nothing
        // inverts, because it is a function of the document rather than of the
        // command. A fixture that got it wrong would therefore fail every
        // byte-exact undo assertion below for a reason that has nothing to do
        // with links.
        let make = |id: &str, render_index: i32| Segment {
            id: id.to_string(),
            material_id: "m".into(),
            target_range: TimeRange::new(1_000_000, 4_000_000),
            source_range: TimeRange::new(0, 4_000_000),
            render_index,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        video_track.segments.push(make("v", 0));
        audio_track.segments.push(make("a", 1));
        let video_track_id = video_track.id.clone();
        let audio_track_id = audio_track.id.clone();
        project.tracks.push(video_track);
        project.tracks.push(audio_track);

        link(&project, &["v".into(), "a".into()])
            .expect("two clips can be linked")
            .apply(&mut project)
            .expect("linking is accepted");
        let group = project
            .link_group_of("v")
            .expect("the pair is linked")
            .clone();

        Pair {
            project,
            video_track: video_track_id,
            audio_track: audio_track_id,
            video: "v".into(),
            audio: "a".into(),
            group,
        }
    }

    fn range_of(project: &Project, segment_id: &str) -> TimeRange {
        project
            .segment(segment_id)
            .unwrap_or_else(|| panic!("{segment_id} is on the timeline"))
            .1
            .target_range
    }

    #[test]
    fn linking_registers_one_group_and_puts_it_on_both_clips() {
        let pair = linked_pair();
        assert_eq!(
            pair.project.materials.links.len(),
            1,
            "one group, not one per clip"
        );
        assert_eq!(pair.project.link_group_of("a"), Some(&pair.group));
        assert_eq!(
            pair.project.link_members(&pair.group).len(),
            2,
            "and both clips are in it"
        );
        assert!(errors(&pair.project).is_empty(), "{:?}", errors(&pair.project));
    }

    #[test]
    fn moving_a_linked_clip_moves_its_partner_as_one_undo_step() {
        let Pair {
            mut project,
            video_track,
            ..
        } = linked_pair();
        let before = serde_json::to_string(&project).unwrap();
        let mut history = History::new();

        history
            .apply(
                &mut project,
                EditCommand::MoveSegment {
                    segment_id: "v".into(),
                    from_track: video_track.clone(),
                    to_track: video_track,
                    from_start: 1_000_000,
                    to_start: 3_000_000,
                },
            )
            .expect("the move is accepted");

        assert_eq!(range_of(&project, "v"), TimeRange::new(3_000_000, 4_000_000));
        assert_eq!(
            range_of(&project, "a"),
            TimeRange::new(3_000_000, 4_000_000),
            "the sound travelled the same distance"
        );
        assert_eq!(
            project.segment("a").unwrap().0.kind,
            TrackKind::Audio,
            "and stayed on its own lane"
        );
        let after = serde_json::to_string(&project).unwrap();

        // One step, not two: the whole thing came back on a single undo.
        history.undo(&mut project).expect("undo");
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "one undo put both clips back"
        );
        assert!(!history.can_undo(), "and there was only one step to undo");

        history.redo(&mut project).expect("redo");
        assert_eq!(serde_json::to_string(&project).unwrap(), after);
    }

    #[test]
    fn dragging_a_linked_clip_to_another_video_lane_leaves_its_sound_where_it_is() {
        // The lane is not shared, only the time is. Moving the picture up a
        // lane must not drag the sound onto a video track.
        let Pair {
            mut project,
            video_track,
            ..
        } = linked_pair();
        let second = Track::new(TrackKind::Video, "V2");
        let second_id = second.id.clone();
        project.tracks.insert(1, second);

        let mut history = History::new();
        history
            .apply(
                &mut project,
                EditCommand::MoveSegment {
                    segment_id: "v".into(),
                    from_track: video_track,
                    to_track: second_id.clone(),
                    from_start: 1_000_000,
                    to_start: 1_000_000,
                },
            )
            .expect("the move is accepted");

        assert_eq!(project.segment("v").unwrap().0.id, second_id);
        assert_eq!(
            project.segment("a").unwrap().0.kind,
            TrackKind::Audio,
            "the sound is still on the audio lane"
        );
        assert_eq!(range_of(&project, "a"), TimeRange::new(1_000_000, 4_000_000));
    }

    #[test]
    fn trimming_a_linked_clip_trims_its_partner_at_the_same_edge() {
        let Pair { mut project, .. } = linked_pair();
        let mut history = History::new();

        // Pull the head in by half a second.
        history
            .apply(
                &mut project,
                EditCommand::TrimSegment {
                    segment_id: "v".into(),
                    before_target: TimeRange::new(1_000_000, 4_000_000),
                    before_source: TimeRange::new(0, 4_000_000),
                    after_target: TimeRange::new(1_500_000, 3_500_000),
                    after_source: TimeRange::new(500_000, 3_500_000),
                },
            )
            .expect("the trim is accepted");

        let (_, audio) = project.segment("a").unwrap();
        assert_eq!(audio.target_range, TimeRange::new(1_500_000, 3_500_000));
        assert_eq!(
            audio.source_range,
            TimeRange::new(500_000, 3_500_000),
            "the sound reads from the same place in the file as the picture"
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));

        // And the tail, which must leave the head alone.
        history
            .apply(
                &mut project,
                EditCommand::TrimSegment {
                    segment_id: "a".into(),
                    before_target: TimeRange::new(1_500_000, 3_500_000),
                    before_source: TimeRange::new(500_000, 3_500_000),
                    after_target: TimeRange::new(1_500_000, 2_000_000),
                    after_source: TimeRange::new(500_000, 2_000_000),
                },
            )
            .expect("trimming from the audio side works the same way");

        assert_eq!(range_of(&project, "v"), TimeRange::new(1_500_000, 2_000_000));
        assert_eq!(range_of(&project, "a"), TimeRange::new(1_500_000, 2_000_000));
    }

    #[test]
    fn splitting_a_linked_pair_produces_two_pairs_that_move_apart() {
        let Pair {
            mut project,
            video_track,
            audio_track,
            group,
            ..
        } = linked_pair();
        let before = serde_json::to_string(&project).unwrap();
        let mut history = History::new();

        let command = split_at(&project, "v", 3_000_000).expect("the cut is inside the clip");
        history.apply(&mut project, command).expect("split");

        assert_eq!(project.track(&video_track).unwrap().segments.len(), 2);
        assert_eq!(
            project.track(&audio_track).unwrap().segments.len(),
            2,
            "the sound was cut at the same instant"
        );

        let right_video = project.track(&video_track).unwrap().segments[1].id.clone();
        let right_audio = project.track(&audio_track).unwrap().segments[1].id.clone();
        assert_eq!(
            range_of(&project, &right_video),
            TimeRange::new(3_000_000, 2_000_000)
        );
        assert_eq!(
            range_of(&project, &right_audio),
            TimeRange::new(3_000_000, 2_000_000)
        );

        // Two pairs, not one group of four: the second half of the picture must
        // drag the second half of the sound and nothing else.
        let right_group = project
            .link_group_of(&right_video)
            .expect("the right halves are linked")
            .clone();
        assert_ne!(right_group, group, "and it is a group of their own");
        assert_eq!(project.link_group_of(&right_audio), Some(&right_group));
        assert_eq!(project.link_group_of("v"), Some(&group));
        assert_eq!(project.link_members(&group).len(), 2);
        assert_eq!(project.link_members(&right_group).len(), 2);
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));

        // Dragging the right pair moves exactly two clips.
        history
            .apply(
                &mut project,
                EditCommand::MoveSegment {
                    segment_id: right_video.clone(),
                    from_track: video_track.clone(),
                    to_track: video_track,
                    from_start: 3_000_000,
                    to_start: 6_000_000,
                },
            )
            .expect("the right pair moves");
        assert_eq!(
            range_of(&project, &right_audio),
            TimeRange::new(6_000_000, 2_000_000)
        );
        assert_eq!(
            range_of(&project, "v"),
            TimeRange::new(1_000_000, 2_000_000),
            "the left half stayed put"
        );
        assert_eq!(range_of(&project, "a"), TimeRange::new(1_000_000, 2_000_000));

        // And the whole session undoes back to where it started, including the
        // group that the split invented.
        history.undo(&mut project).expect("undo the move");
        history.undo(&mut project).expect("undo the split");
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "undoing a split of a linked pair restores the document exactly"
        );
    }

    #[test]
    fn deleting_one_of_a_linked_pair_deletes_both_and_one_undo_restores_both() {
        let Pair {
            mut project,
            audio_track,
            ..
        } = linked_pair();
        let before = serde_json::to_string(&project).unwrap();
        let audio = project.segment("a").unwrap().1.clone();
        let mut history = History::new();

        history
            .apply(
                &mut project,
                EditCommand::RemoveSegment {
                    track_id: audio_track,
                    segment: audio,
                    index: 0,
                },
            )
            .expect("the delete is accepted");

        assert!(project.segment("a").is_none());
        assert!(
            project.segment("v").is_none(),
            "deleting the sound deleted the picture with it"
        );

        history.undo(&mut project).expect("undo");
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "one undo brought both clips back, still linked"
        );
        assert_eq!(project.link_group_of("v"), project.link_group_of("a"));
    }

    #[test]
    fn unlinking_lets_the_two_move_independently() {
        let Pair {
            mut project,
            video_track,
            ..
        } = linked_pair();
        let mut history = History::new();

        let command = unlink(&project, "v").expect("the pair is linked");
        history.apply(&mut project, command).expect("unlink");

        assert_eq!(project.link_group_of("v"), None);
        assert_eq!(project.link_group_of("a"), None);
        assert!(
            project.materials.links.is_empty(),
            "the group went with its last member"
        );

        history
            .apply(
                &mut project,
                EditCommand::MoveSegment {
                    segment_id: "v".into(),
                    from_track: video_track.clone(),
                    to_track: video_track,
                    from_start: 1_000_000,
                    to_start: 5_000_000,
                },
            )
            .expect("the move is accepted");

        assert_eq!(range_of(&project, "v"), TimeRange::new(5_000_000, 4_000_000));
        assert_eq!(
            range_of(&project, "a"),
            TimeRange::new(1_000_000, 4_000_000),
            "the sound stayed exactly where it was"
        );

        // And undoing the unlink links them again, so the next move mirrors.
        history.undo(&mut project).expect("undo the move");
        history.undo(&mut project).expect("undo the unlink");
        assert_eq!(project.link_group_of("v"), project.link_group_of("a"));
        assert!(project.link_group_of("v").is_some());
    }

    #[test]
    fn an_unlink_built_against_a_stale_document_is_refused() {
        // The panel's copy of the document is one edit behind and the user
        // clicks Unlink. Applying it anyway would strip whichever group the
        // clip is in now, and its inverse would restore a link that never
        // existed.
        let Pair { mut project, .. } = linked_pair();
        let stale = EditCommand::SetLinkGroup {
            segment_id: "v".into(),
            before: Some("a-group-from-a-previous-life".into()),
            after: None,
        };
        let error = stale
            .apply(&mut project)
            .expect_err("a stale link edit must be refused");
        assert!(error.contains("changed underneath"), "{error}");
        assert!(project.link_group_of("v").is_some(), "and nothing changed");
    }

    #[test]
    fn a_linked_edit_that_cannot_be_mirrored_changes_nothing() {
        // The audio lane is occupied where the sound would have to land. The
        // pair moves as one thing or not at all — half a move is a pair that no
        // longer lines up, which is worse than a refusal.
        let Pair {
            mut project,
            video_track,
            audio_track,
            ..
        } = linked_pair();
        let blocker = Segment {
            id: "blocker".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(6_000_000, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            // The audio lane is the second track; see `linked_pair`.
            render_index: 1,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        project
            .track_mut(&audio_track)
            .unwrap()
            .segments
            .push(blocker);
        let before = serde_json::to_string(&project).unwrap();

        let mut history = History::new();
        let error = history
            .apply(
                &mut project,
                EditCommand::MoveSegment {
                    segment_id: "v".into(),
                    from_track: video_track.clone(),
                    to_track: video_track,
                    from_start: 1_000_000,
                    to_start: 6_000_000,
                },
            )
            .expect_err("the sound has nowhere to go");
        assert!(error.contains("occupied"), "{error}");
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "and the picture did not move either"
        );
        assert!(!history.can_undo(), "a refused edit is not in the history");
    }

    #[test]
    fn move_into_occupied_range_is_rejected_and_leaves_document_intact() {
        let (mut project, track_id, segment_id) = project_with_clip();
        let blocker = Segment {
            id: crate::modules::project::new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(5_000_000, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        project.track_mut(&track_id).unwrap().segments.push(blocker);

        let cmd = EditCommand::MoveSegment {
            segment_id: segment_id.clone(),
            from_track: track_id.clone(),
            to_track: track_id.clone(),
            from_start: 0,
            to_start: 5_000_000,
        };
        assert!(cmd.apply(&mut project).is_err());

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 2);
        assert!(track.segments.iter().any(|s| s.id == segment_id));
    }

    // -----------------------------------------------------------------------
    // Multi-clip edits
    //
    // A selection is one gesture and has to be one undo step. What these guard
    // is the two things a batch has that a single command does not: the link
    // partners it must expand exactly once, and the order its parts must be
    // applied in so that no intermediate state overlaps.
    // -----------------------------------------------------------------------

    /// Three one-second clips on one lane, back to back from zero.
    fn row_of_three() -> (Project, String, Vec<String>) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let ids: Vec<String> = ["one", "two", "three"].iter().map(|s| s.to_string()).collect();
        for (index, id) in ids.iter().enumerate() {
            track.segments.push(Segment {
                id: id.clone(),
                material_id: "m".into(),
                target_range: TimeRange::new(index as Micros * 1_000_000, 1_000_000),
                source_range: TimeRange::new(0, 1_000_000),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                transform: Transform::default(),
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            });
        }
        let track_id = track.id.clone();
        project.tracks.push(track);
        (project, track_id, ids)
    }

    fn remove_command(project: &Project, segment_id: &str) -> EditCommand {
        let (track, segment) = project.segment(segment_id).expect("on the timeline");
        let index = track
            .segments
            .iter()
            .position(|s| s.id == segment_id)
            .expect("in its lane");
        EditCommand::RemoveSegment {
            track_id: track.id.clone(),
            segment: segment.clone(),
            index,
        }
    }

    fn move_command(project: &Project, segment_id: &str, delta: Micros) -> EditCommand {
        let (track, segment) = project.segment(segment_id).expect("on the timeline");
        EditCommand::MoveSegment {
            segment_id: segment_id.to_string(),
            from_track: track.id.clone(),
            to_track: track.id.clone(),
            from_start: segment.target_range.start,
            to_start: segment.target_range.start + delta,
        }
    }

    #[test]
    fn deleting_a_selection_is_one_undo_step() {
        let (mut project, track_id, ids) = row_of_three();
        let batch = vec![
            remove_command(&project, &ids[0]),
            remove_command(&project, &ids[2]),
        ];
        let command = compose_edits(&project, "Delete clips", batch).unwrap();

        let mut history = History::new();
        history.apply(&mut project, command).unwrap();
        assert_eq!(project.track(&track_id).unwrap().segments.len(), 1);

        history.undo(&mut project).unwrap();
        let track = project.track(&track_id).unwrap();
        assert_eq!(
            track.segments.len(),
            3,
            "one undo has to bring back everything one gesture took away"
        );
        // And it puts them back where they were, in order.
        assert_eq!(
            track
                .segments
                .iter()
                .map(|s| s.target_range.start)
                .collect::<Vec<_>>(),
            vec![0, 1_000_000, 2_000_000]
        );
        assert!(!history.can_undo(), "one gesture, one entry");
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    #[test]
    fn a_selection_holding_both_halves_of_a_linked_pair_moves_it_once() {
        let Pair {
            mut project,
            video,
            audio,
            ..
        } = linked_pair();

        let batch = vec![
            move_command(&project, &video, 2_000_000),
            move_command(&project, &audio, 2_000_000),
        ];
        let command = compose_edits(&project, "Move clips", batch).unwrap();

        let EditCommand::Composite { commands, .. } = &command else {
            panic!("a batch is a composite");
        };
        assert_eq!(
            commands.len(),
            2,
            "the sound is in the selection, so the picture must not drag it as well: {commands:#?}"
        );

        let mut history = History::new();
        history.apply(&mut project, command).unwrap();
        // Moved once, by the distance the user dragged — not twice, which would
        // have landed it at 7s.
        assert_eq!(range_of(&project, &video), TimeRange::new(3_000_000, 4_000_000));
        assert_eq!(range_of(&project, &audio), TimeRange::new(3_000_000, 4_000_000));

        history.undo(&mut project).unwrap();
        assert_eq!(range_of(&project, &video), TimeRange::new(1_000_000, 4_000_000));
        assert_eq!(range_of(&project, &audio), TimeRange::new(1_000_000, 4_000_000));
    }

    #[test]
    fn a_selection_holding_one_half_of_a_linked_pair_still_drags_the_other() {
        let Pair {
            mut project,
            video_track,
            video,
            audio,
            ..
        } = linked_pair();

        // A second, unlinked clip on the picture lane, selected along with the
        // half of the pair. The sound is *not* selected, so the batch has to
        // bring it along itself.
        project.track_mut(&video_track).unwrap().segments.push(Segment {
            id: "loose".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(6_000_000, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });

        let batch = vec![
            move_command(&project, &video, 2_000_000),
            move_command(&project, "loose", 2_000_000),
        ];
        let command = compose_edits(&project, "Move clips", batch).unwrap();
        let EditCommand::Composite { commands, .. } = &command else {
            panic!("a batch is a composite");
        };
        assert_eq!(commands.len(), 3, "two clips asked for, one partner added");

        History::new().apply(&mut project, command).unwrap();
        assert_eq!(range_of(&project, &audio).start, 3_000_000);
        assert_eq!(range_of(&project, "loose").start, 8_000_000);
    }

    #[test]
    fn a_block_of_clips_moving_together_does_not_trip_over_itself() {
        // Every one of these destinations is occupied by the clip in front of
        // it at the moment the batch starts. Applied left to right the first
        // command is refused and the whole gesture is rolled back.
        let (mut project, track_id, ids) = row_of_three();
        let forward: Vec<EditCommand> = ids
            .iter()
            .map(|id| move_command(&project, id, 3_000_000))
            .collect();
        let command = compose_edits(&project, "Move clips", forward).unwrap();
        History::new().apply(&mut project, command).unwrap();
        assert_eq!(
            project
                .track(&track_id)
                .unwrap()
                .segments
                .iter()
                .map(|s| s.target_range.start)
                .collect::<Vec<_>>(),
            vec![3_000_000, 4_000_000, 5_000_000]
        );

        // And the same block coming back, which needs the opposite order.
        let backward: Vec<EditCommand> = ids
            .iter()
            .map(|id| move_command(&project, id, -3_000_000))
            .collect();
        let command = compose_edits(&project, "Move clips", backward).unwrap();
        History::new().apply(&mut project, command).unwrap();
        assert_eq!(
            project
                .track(&track_id)
                .unwrap()
                .segments
                .iter()
                .map(|s| s.target_range.start)
                .collect::<Vec<_>>(),
            vec![0, 1_000_000, 2_000_000]
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    // -----------------------------------------------------------------------
    // Ripple edits
    //
    // A ripple is not a primitive: the frontend sends one removal plus one move
    // per later clip on the lane, in an order that never overlaps — the removal
    // first, then the moves left to right, so each destination is vacated
    // before anything arrives. `compose_edits` is what carries the link
    // partners and makes the whole thing one undo step.
    // -----------------------------------------------------------------------

    #[test]
    fn a_ripple_delete_batch_closes_the_gap_and_undoes_in_one_step() {
        let (mut project, track_id, ids) = row_of_three();
        let before = serde_json::to_string(&project).unwrap();

        // What `rippleDeleteCommands` in the webview builds: remove "one",
        // pull "two" and "three" left by its duration.
        let batch = vec![
            remove_command(&project, &ids[0]),
            move_command(&project, &ids[1], -1_000_000),
            move_command(&project, &ids[2], -1_000_000),
        ];
        let command = compose_edits(&project, "Ripple delete", batch).unwrap();

        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(
            track
                .segments
                .iter()
                .map(|s| (s.id.clone(), s.target_range.start))
                .collect::<Vec<_>>(),
            vec![("two".to_string(), 0), ("three".to_string(), 1_000_000)],
            "everything later moved left by the deleted clip's duration"
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));

        history.undo(&mut project).unwrap();
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "one undo puts the clip back and the gap with it"
        );
        assert!(!history.can_undo(), "one gesture, one entry");
    }

    /// A second linked pair after the first, for ripples across linked clips.
    fn two_linked_pairs() -> Pair {
        let mut pair = linked_pair();
        let make = |id: &str, render_index: i32| Segment {
            id: id.to_string(),
            material_id: "m".into(),
            target_range: TimeRange::new(6_000_000, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        pair.project
            .track_mut(&pair.video_track)
            .unwrap()
            .segments
            .push(make("v2", 0));
        pair.project
            .track_mut(&pair.audio_track)
            .unwrap()
            .segments
            .push(make("a2", 1));
        link(&pair.project, &["v2".into(), "a2".into()])
            .unwrap()
            .apply(&mut pair.project)
            .unwrap();
        pair
    }

    #[test]
    fn a_ripple_across_linked_clips_moves_both_halves_of_every_pair() {
        // The batch names only the video lane — remove v, move v2 — and the
        // audio has to come along by itself: a delete, and every partner of
        // every moved clip, expanded exactly once by `compose_edits`.
        let Pair { mut project, .. } = two_linked_pairs();
        let before = serde_json::to_string(&project).unwrap();

        let batch = vec![
            remove_command(&project, "v"),
            move_command(&project, "v2", -4_000_000),
        ];
        let command = compose_edits(&project, "Ripple delete", batch).unwrap();
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        assert!(project.segment("v").is_none());
        assert!(
            project.segment("a").is_none(),
            "deleting the picture deleted its linked sound"
        );
        assert_eq!(range_of(&project, "v2").start, 2_000_000);
        assert_eq!(
            range_of(&project, "a2").start,
            2_000_000,
            "the moved clip's sound rippled with it"
        );
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));

        history.undo(&mut project).unwrap();
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "one undo restores both pairs, still linked"
        );
    }

    #[test]
    fn a_ripple_whose_mirror_has_nowhere_to_go_changes_nothing() {
        // A music bed sits where the moved clip's sound would land, so the
        // mirrored move is refused — and with it the whole ripple, because a
        // ripple that deletes the clip but leaves the gap is worse than one
        // that visibly did nothing.
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "m".into(),
            path: "/media/clip.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        let make = |id: &str, start: Micros, duration: Micros, render_index: i32| Segment {
            id: id.to_string(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        let mut video_track = Track::new(TrackKind::Video, "V1");
        video_track.segments.push(make("head", 0, 1_000_000, 0));
        video_track.segments.push(make("v2", 6_000_000, 1_000_000, 0));
        let mut audio_track = Track::new(TrackKind::Audio, "A1");
        audio_track.segments.push(make("bed", 5_200_000, 500_000, 1));
        audio_track.segments.push(make("a2", 6_000_000, 1_000_000, 1));
        project.tracks.push(video_track);
        project.tracks.push(audio_track);
        link(&project, &["v2".into(), "a2".into()])
            .unwrap()
            .apply(&mut project)
            .unwrap();
        let before = serde_json::to_string(&project).unwrap();

        // Ripple-deleting "head" pulls v2 back by a second; its mirrored sound
        // would land on the bed at 5.2s.
        let batch = vec![
            remove_command(&project, "head"),
            move_command(&project, "v2", -1_000_000),
        ];
        let command = compose_edits(&project, "Ripple delete", batch).unwrap();
        let mut history = History::new();
        let error = history
            .apply(&mut project, command)
            .expect_err("the sound has nowhere to go");
        assert!(error.contains("occupied"), "{error}");
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "and nothing was deleted or moved"
        );
        assert!(!history.can_undo());
    }

    // -----------------------------------------------------------------------
    // Track order
    // -----------------------------------------------------------------------

    /// Three lanes, one clip each, render order already normalised.
    fn stacked_tracks() -> (Project, Vec<String>) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut ids = Vec::new();
        for name in ["V1", "V2", "V3"] {
            let mut track = Track::new(TrackKind::Video, name);
            track.segments.push(Segment {
                id: format!("clip-{name}"),
                material_id: "m".into(),
                target_range: TimeRange::new(0, 1_000_000),
                source_range: TimeRange::new(0, 1_000_000),
                render_index: ids.len() as i32,
                speed: 1.0,
                volume: 1.0,
                transform: Transform::default(),
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            });
            ids.push(track.id.clone());
            project.tracks.push(track);
        }
        (project, ids)
    }

    #[test]
    fn reordering_tracks_restacks_the_render_order_and_undoes_exactly() {
        let (mut project, ids) = stacked_tracks();
        let before = serde_json::to_string(&project).unwrap();

        let command = EditCommand::MoveTrack {
            track_id: ids[0].clone(),
            from_index: 0,
            to_index: 2,
        };
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        let order: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(order, vec!["V2", "V3", "V1"]);
        // Render order follows track order: the moved lane now paints on top.
        for (index, track) in project.tracks.iter().enumerate() {
            assert_eq!(
                track.segments[0].render_index, index as i32,
                "{} carries the render index of its new position",
                track.name
            );
        }

        history.undo(&mut project).unwrap();
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "undoing the reorder restores order and render indices alike"
        );
    }

    #[test]
    fn a_track_move_against_a_stale_index_is_refused() {
        let (mut project, ids) = stacked_tracks();
        let before = serde_json::to_string(&project).unwrap();

        // The gesture thought V2 was still at index 0.
        let error = EditCommand::MoveTrack {
            track_id: ids[1].clone(),
            from_index: 0,
            to_index: 2,
        }
        .apply(&mut project)
        .expect_err("a stale index refuses");
        assert!(error.contains("V1"), "the message names the lane: {error}");
        assert_eq!(serde_json::to_string(&project).unwrap(), before);

        // And a destination past the end is a place that does not exist.
        assert!(EditCommand::MoveTrack {
            track_id: ids[0].clone(),
            from_index: 0,
            to_index: 3,
        }
        .apply(&mut project)
        .is_err());
    }

    #[test]
    fn a_batch_of_one_is_left_alone_so_the_history_still_mirrors_it() {
        // Selecting one clip and dragging it has to stay on exactly the path it
        // has always taken: a bare command, mirrored onto its partner by
        // `History::apply`. Wrapping it would take that mirroring away, because
        // a composite is deliberately not expanded there.
        let Pair {
            mut project,
            video,
            audio,
            ..
        } = linked_pair();

        let batch = vec![move_command(&project, &video, 2_000_000)];
        let command = compose_edits(&project, "Move clips", batch).unwrap();
        assert!(
            matches!(command, EditCommand::MoveSegment { .. }),
            "one clip is not a batch"
        );

        History::new().apply(&mut project, command).unwrap();
        assert_eq!(range_of(&project, &audio).start, 3_000_000);
    }

    #[test]
    fn an_empty_batch_is_refused_rather_than_recorded_as_an_edit() {
        let (project, _, _) = row_of_three();
        assert!(compose_edits(&project, "Move clips", Vec::new()).is_err());
    }

    // -----------------------------------------------------------------------
    // Markers
    // -----------------------------------------------------------------------

    fn marker(id: &str, time: Micros) -> Marker {
        Marker {
            id: id.into(),
            time,
            label: String::new(),
            color: crate::modules::project::MarkerColor::default(),
        }
    }

    #[test]
    fn marker_edits_apply_and_undo_exactly_and_stay_sorted() {
        let (mut project, _, _) = project_with_clip();
        let pristine = serde_json::to_string(&project).unwrap();
        let mut history = History::new();

        // Added out of time order on purpose: the list has to come out sorted
        // whatever order the user pressed M in.
        history
            .apply(&mut project, EditCommand::AddMarker { marker: marker("m2", 2_000_000) })
            .unwrap();
        history
            .apply(&mut project, EditCommand::AddMarker { marker: marker("m1", 1_000_000) })
            .unwrap();
        assert_eq!(
            project.markers.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["m1", "m2"]
        );

        // Rename, recolour and move in one edit; undo restores all three.
        let mut renamed = marker("m1", 3_500_000);
        renamed.label = "chorus".into();
        renamed.color = crate::modules::project::MarkerColor::Red;
        history
            .apply(
                &mut project,
                EditCommand::SetMarker {
                    before: marker("m1", 1_000_000),
                    after: renamed.clone(),
                },
            )
            .unwrap();
        // Moving past m2 re-sorted the list.
        assert_eq!(
            project.markers.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["m2", "m1"]
        );
        assert_eq!(project.markers[1].label, "chorus");

        history
            .apply(&mut project, EditCommand::RemoveMarker { marker: renamed })
            .unwrap();
        assert_eq!(project.markers.len(), 1);

        while history.can_undo() {
            history.undo(&mut project).unwrap();
        }
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            pristine,
            "undoing every marker edit restores the document byte for byte"
        );
    }

    #[test]
    fn nonsense_and_stale_marker_edits_are_refused() {
        let (mut project, _, _) = project_with_clip();
        EditCommand::AddMarker { marker: marker("m1", 1_000_000) }
            .apply(&mut project)
            .unwrap();

        // No second marker under one id, and no instant before the timeline.
        assert!(EditCommand::AddMarker { marker: marker("m1", 2_000_000) }
            .apply(&mut project)
            .is_err());
        assert!(EditCommand::AddMarker { marker: marker("m2", -1) }
            .apply(&mut project)
            .is_err());
        assert!(EditCommand::SetMarker {
            before: marker("m1", 1_000_000),
            after: marker("m1", -5),
        }
        .apply(&mut project)
        .is_err());

        // A stale panel: the marker moved since the menu was opened. Deleting
        // or editing it anyway would make undo restore the wrong marker.
        assert!(EditCommand::RemoveMarker { marker: marker("m1", 999) }
            .apply(&mut project)
            .is_err());
        assert!(EditCommand::SetMarker {
            before: marker("m1", 999),
            after: marker("m1", 2_000_000),
        }
        .apply(&mut project)
        .is_err());

        // And the id is an identity, not a field.
        assert!(EditCommand::SetMarker {
            before: marker("m1", 1_000_000),
            after: marker("m2", 1_000_000),
        }
        .apply(&mut project)
        .is_err());

        assert_eq!(project.markers.len(), 1);
        assert_eq!(project.markers[0].time, 1_000_000);
    }

    // -----------------------------------------------------------------------
    // Mute
    // -----------------------------------------------------------------------

    #[test]
    fn undoing_a_mute_restores_the_old_volume() {
        // Mute is `SetVolume { after: 0 }` with the previous value in `before`,
        // which is what makes one undo bring the level back rather than
        // resetting it to 1.
        let (mut project, _, segment_id) = project_with_clip();
        EditCommand::SetVolume {
            segment_id: segment_id.clone(),
            before: 1.0,
            after: 0.7,
        }
        .apply(&mut project)
        .unwrap();

        let mut history = History::new();
        history
            .apply(
                &mut project,
                EditCommand::SetVolume {
                    segment_id: segment_id.clone(),
                    before: 0.7,
                    after: 0.0,
                },
            )
            .unwrap();
        assert_eq!(project.segment(&segment_id).unwrap().1.volume, 0.0);

        history.undo(&mut project).unwrap();
        assert_eq!(
            project.segment(&segment_id).unwrap().1.volume,
            0.7,
            "undo restores the level the clip had, not a default"
        );
    }

    // -----------------------------------------------------------------------
    // Split all tracks
    // -----------------------------------------------------------------------

    #[test]
    fn split_all_cuts_every_lane_under_the_playhead_in_one_undo_step() {
        // A linked pair on two lanes plus an unlinked clip on a third: the pair
        // is one cluster (split_at cuts both halves), the loose clip another,
        // and the whole gesture is a single entry on the undo stack.
        let Pair {
            mut project,
            video_track,
            audio_track,
            ..
        } = linked_pair();
        let mut third = Track::new(TrackKind::Video, "V2");
        third.segments.push(Segment {
            id: "loose".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 2,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        let third_id = third.id.clone();
        project.tracks.push(third);
        let before = serde_json::to_string(&project).unwrap();

        let command = split_all_at(&project, 3_000_000).expect("there is plenty to cut");
        let mut history = History::new();
        history.apply(&mut project, command).unwrap();

        assert_eq!(project.track(&video_track).unwrap().segments.len(), 2);
        assert_eq!(
            project.track(&audio_track).unwrap().segments.len(),
            2,
            "the linked sound was cut once, by its partner's cluster"
        );
        assert_eq!(project.track(&third_id).unwrap().segments.len(), 2);
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));

        history.undo(&mut project).unwrap();
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            before,
            "one undo puts every lane back"
        );
        assert!(!history.can_undo(), "one gesture, one entry");
    }

    #[test]
    fn split_all_skips_locked_lanes_and_refuses_when_nothing_crosses() {
        let (mut project, track_id, _) = project_with_clip();
        let mut locked = Track::new(TrackKind::Video, "V2");
        locked.locked = true;
        locked.segments.push(Segment {
            id: "immovable".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 1,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        let locked_id = locked.id.clone();
        project.tracks.push(locked);

        split_all_at(&project, 2_000_000)
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(project.track(&track_id).unwrap().segments.len(), 2);
        assert_eq!(
            project.track(&locked_id).unwrap().segments.len(),
            1,
            "a locked lane takes no cut"
        );

        // Past the end of everything there is nothing to cut, and a clip edge
        // is not an inside either.
        assert!(split_all_at(&project, 50_000_000).is_err());
        assert!(
            split_all_at(&project, 0).is_err(),
            "a cut on the very first edge has nothing to its left"
        );
    }
}
