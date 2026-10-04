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
//! What maps: position, the window the compound clip shows, its speed —
//! constant (multiplied through) or a curve (composed with each inner clip's
//! speed into a curve of the inner clip's own, `super::retime`) — clip and
//! lane volume (multiplied through), mute (a muted lane, inside or out,
//! contributes nothing), and volume keyframes, the inner clips' and the
//! compound clip's own (multiplied). Decision 0024.

use std::borrow::Cow;

use super::retime::{self, Outer};
use crate::modules::project::Micros;
use crate::modules::project::{
    source_duration_for, AnimatableProperty, Project, Segment, SpeedCurveMaterial, TimeRange,
    Track, TrackKind,
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
    let mut curves: Vec<SpeedCurveMaterial> = Vec::new();
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
            let outer = Outer::new(&project.materials, segment);
            extra.extend(expand(&inner, track, &outer, &mut curves));
        }
    }
    let mut flat = project.clone();
    flat.tracks.extend(extra);
    if !curves.is_empty() {
        flat.materials.speed_curves.extend(curves);
        flat.materials.speed_curves.sort_by(|a, b| a.id.cmp(&b.id));
    }
    Cow::Owned(flat)
}

/// The sound-bearing clips of `inner` (already flattened) as heard through
/// compound clip `outer` on `lane`: one lane per inner lane that has any.
/// Curves the pieces play through are added to `curves`.
fn expand(
    inner: &Project,
    lane: &Track,
    outer: &Outer<'_>,
    curves: &mut Vec<SpeedCurveMaterial>,
) -> Vec<Track> {
    let window = outer.window();
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
            if let Some(piece) = piece(inner, outer, s, window, curves) {
                segments.push(piece);
            }
        }
        if segments.is_empty() {
            continue;
        }
        out.push(Track {
            id: format!("{}/{}", outer.segment.id, track.id),
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

/// Inner clip `s`, cut to the compound clip's window and retimed onto the
/// outer timeline. `None` when none of it is in the window.
fn piece(
    inner: &Project,
    outer: &Outer<'_>,
    s: &Segment,
    window: TimeRange,
    curves: &mut Vec<SpeedCurveMaterial>,
) -> Option<Segment> {
    let a = s.target_range.start.max(window.start);
    let b = s.target_range.end().min(window.end());
    if b <= a {
        return None;
    }
    let start = outer.at(a);
    let end = outer.at(b);
    if end <= start {
        return None;
    }
    let target = TimeRange::new(start, end - start);
    let into_clip = a - s.target_range.start;
    let inner_map = inner.materials.time_map(s);
    let own_curve = inner_map.curve;

    let mut c = s.clone();
    c.id = format!("{}/{}", outer.segment.id, s.id);
    c.target_range = target;
    c.volume = s.volume * outer.segment.volume;
    c.extras
        .retain(|e| !inner.materials.links.contains(e) && inner.materials.speed_curve(e).is_none());
    let points = match (outer.curve(), own_curve) {
        (None, None) => {
            c.speed = (retime::sane(s.speed) * outer.speed()) as f32;
            c.source_range = TimeRange::new(
                s.source_range.start + source_duration_for(into_clip, s.speed),
                source_duration_for(target.duration, c.speed),
            );
            None
        }
        (None, Some(curve)) => {
            c.source_range = inner_map.retimed_source(TimeRange::new(a, b - a));
            Some(retime::scaled(curve, outer.speed()))
        }
        (Some(curve), None) => {
            c.source_range = TimeRange::new(
                s.source_range.start + source_duration_for(into_clip, s.speed),
                source_duration_for(b - a, s.speed),
            );
            Some(retime::under_curve(curve, s))
        }
        (Some(curve), Some(_)) => {
            c.source_range = inner_map.retimed_source(TimeRange::new(a, b - a));
            Some(retime::dense(curve, inner_map, c.source_range))
        }
    };
    if let Some(points) = points {
        let id = format!("{}~curve", c.id);
        c.extras.push(id.clone());
        curves.push(SpeedCurveMaterial {
            id,
            preset: None,
            points,
        });
    }
    // Volume keyframes keep the inner instant they were set at, wherever the
    // compound clip's speed puts it, and multiply with the compound clip's
    // own.
    let inner_volume = retime::keyframes(outer, s, start)
        .into_iter()
        .find(|k| k.property == AnimatableProperty::Volume);
    c.keyframes = retime::volume(outer.segment, inner_volume.as_ref(), target)
        .into_iter()
        .collect();
    Some(c)
}

/// The sound of sequence `id` over `range` of its own time, mixed as its
/// timeline would play it (volumes, keyframes, compound clips inside),
/// interleaved stereo at `rate`. What silence cutting and loudness measure
/// when they are pointed at a compound clip: its contents, not a file.
/// Blocking; it decodes.
pub fn mix_of(
    project: &Project,
    id: &str,
    range: TimeRange,
    rate: u32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Vec<f32>, String> {
    use crate::modules::audio::{plan, FileClipFactory, TimelineMixer, MIX_CHANNELS};
    let view = super::nested(project, id).ok_or("that compound clip's contents are gone")?;
    let mut mixer = TimelineMixer::new(rate, std::sync::Arc::new(FileClipFactory));
    mixer.set_plan(plan(&view));
    let frames = |t: Micros| (t.max(0) as i128 * rate as i128 / 1_000_000) as i64;
    let first = frames(range.start);
    let total = (frames(range.end()) - first).max(0) as usize;
    let mut out = vec![0f32; total * MIX_CHANNELS];
    // Ten seconds a block, so a cancel is noticed quickly.
    let block = rate as usize * 10;
    let mut at = 0usize;
    while at < total {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = block.min(total - at);
        mixer.fill(
            first + at as i64,
            &mut out[at * MIX_CHANNELS..(at + n) * MIX_CHANNELS],
        );
        at += n;
    }
    Ok(out)
}
