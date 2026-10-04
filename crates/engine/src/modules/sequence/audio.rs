//! A compound clip's sound.
//!
//! The picture of a compound clip has to be rendered as a whole — its opacity,
//! transform and effects apply to the composite, not to each clip inside — but
//! sound is a sum, and a sum can be taken apart. So both mixers (the preview's
//! `audio::mixer::plan` and the export's `export::audio::mix_timeline`) are
//! handed a flattened copy of the project in which every compound clip's
//! sound-bearing clips sit on lanes of their own, already moved, trimmed and
//! retimed into the outer timeline. The mixers stay what they were.
//!
//! What maps exactly: position, the window the compound clip shows, constant
//! speed (multiplied through), clip volume and lane volume (multiplied
//! through), mute (a muted lane, inside or out, contributes nothing), volume
//! keyframes of the clips inside. What does not: a compound clip's *own*
//! volume keyframes and a speed curve on the compound clip itself — those play
//! at its plain volume and its constant speed. Decision 0024.

use std::borrow::Cow;

use crate::modules::project::{
    source_duration_for, AnimatableProperty, Micros, Project, Segment, TimeRange, Track, TrackKind,
};

/// `project` with every compound clip's sound laid out on lanes of its own.
/// Borrowed, untouched, when the active timeline has no compound clip.
pub fn flatten_audio(project: &Project) -> Cow<'_, Project> {
    flatten_at(project, 0)
}

fn flatten_at(project: &Project, depth: usize) -> Cow<'_, Project> {
    let has_compound = project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .any(|s| project.materials.sequence(&s.material_id).is_some());
    if !has_compound {
        return Cow::Borrowed(project);
    }
    let mut extra: Vec<Track> = Vec::new();
    for track in &project.tracks {
        if track.muted || !matches!(track.kind, TrackKind::Audio | TrackKind::Video) {
            continue;
        }
        for segment in &track.segments {
            if project.materials.sequence(&segment.material_id).is_none() {
                continue;
            }
            if depth >= super::MAX_DEPTH {
                tracing::warn!(segment = %segment.id, "compound clips nest too deep; silent");
                continue;
            }
            let Some(inner) = super::nested(project, &segment.material_id) else {
                continue;
            };
            let inner = flatten_at(&inner, depth + 1);
            extra.extend(expand(&inner, track, segment));
        }
    }
    let mut flat = project.clone();
    flat.tracks.extend(extra);
    Cow::Owned(flat)
}

fn sane(speed: f32) -> f64 {
    if speed.is_finite() && speed > 0.0 {
        speed as f64
    } else {
        1.0
    }
}

/// The sound-bearing clips of `inner` (already flattened) as heard through
/// compound clip `outer` on `lane`: one lane per inner lane that has any.
fn expand(inner: &Project, lane: &Track, outer: &Segment) -> Vec<Track> {
    let k = sane(outer.speed);
    let window = outer.source_range;
    let mut out = Vec::new();
    for track in &inner.tracks {
        if track.muted || !matches!(track.kind, TrackKind::Audio | TrackKind::Video) {
            continue;
        }
        let mut segments = Vec::new();
        for s in &track.segments {
            // Inside the compound only the clip that is not deferring to a
            // linked partner is heard, exactly as on a timeline. The rule is
            // applied here, against the inner document, because the copy
            // below leaves the link behind.
            if inner.sound_is_on_a_linked_lane(track, s) {
                continue;
            }
            // A compound clip inside: already expanded onto lanes of its own.
            if inner.materials.sequence(&s.material_id).is_some() {
                continue;
            }
            let a = s.target_range.start.max(window.start);
            let b = s.target_range.end().min(window.end());
            if b <= a {
                continue;
            }
            let into_clip = a - s.target_range.start;
            let start =
                outer.target_range.start + ((a - window.start) as f64 / k).round() as Micros;
            let end = outer.target_range.start + ((b - window.start) as f64 / k).round() as Micros;
            if end <= start {
                continue;
            }
            let speed = (sane(s.speed) * k) as f32;
            let target = TimeRange::new(start, end - start);
            let source = TimeRange::new(
                s.source_range.start + source_duration_for(into_clip, s.speed),
                source_duration_for(target.duration, speed),
            );
            let mut c = s.clone();
            c.id = format!("{}/{}", outer.id, s.id);
            c.target_range = target;
            c.source_range = source;
            c.speed = speed;
            c.volume = s.volume * outer.volume;
            c.extras.retain(|e| !inner.materials.links.contains(e));
            // Volume keyframes are relative to the clip's start: shift them by
            // the part the window cut off, and squeeze them by the speed.
            c.keyframes = s
                .keyframes
                .iter()
                .filter(|k| k.property == AnimatableProperty::Volume)
                .cloned()
                .map(|mut t| {
                    for key in &mut t.keyframes {
                        key.time = ((key.time - into_clip) as f64 / k).round() as Micros;
                    }
                    t
                })
                .collect();
            segments.push(c);
        }
        if segments.is_empty() {
            continue;
        }
        out.push(Track {
            id: format!("{}/{}", outer.id, track.id),
            kind: track.kind,
            name: track.name.clone(),
            segments,
            muted: false,
            locked: true,
            hidden: true,
            volume: track.volume * lane.volume,
        });
    }
    out
}
