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
    source_duration_for, speed_slack, AnimatableProperty, Easing, Keyframe, KeyframeTrack, Micros,
    Project, Segment, TimeRange, Track, Transform, TransitionMaterial,
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
pub fn split_at(project: &Project, segment_id: &str, at: Micros) -> Result<EditCommand, String> {
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
    right
        .extras
        .retain(|id| project.materials.transition(id).is_none());

    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment is on this track")
        + 1;

    Ok(EditCommand::Composite {
        label: "Split clip".into(),
        commands: vec![
            EditCommand::TrimSegment {
                segment_id: segment_id.to_string(),
                before_target: segment.target_range,
                before_source: segment.source_range,
                after_target: left_target,
                after_source: left_source,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: right,
                index,
            },
        ],
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
    use crate::modules::project::{CanvasConfig, TrackKind};

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
}
