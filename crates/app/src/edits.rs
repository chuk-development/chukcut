//! Building edit commands from UI gestures.
//!
//! Each function reads the document and returns the one [`EditCommand`] that
//! does what the gesture means. Applying it goes through the engine's command
//! layer, so undo, autosave and validation behave exactly as for any other
//! edit.

use chukcut_engine::modules::project::{
    new_id, Micros, Project, Segment, TimeRange, TrackKind, Transform,
};
use chukcut_engine::modules::timeline::ops::EditCommand;

/// How long a still lands on the timeline.
pub const STILL_DURATION: Micros = 3_000_000;

/// The kind of lane a material belongs on, and how long it runs.
fn placement(project: &Project, material_id: &str) -> Option<(TrackKind, Micros)> {
    let pool = &project.materials;
    if let Some(video) = pool.videos.iter().find(|m| m.id == material_id) {
        return Some((TrackKind::Video, video.duration));
    }
    if pool.images.iter().any(|m| m.id == material_id) {
        return Some((TrackKind::Video, STILL_DURATION));
    }
    if let Some(audio) = pool.audios.iter().find(|m| m.id == material_id) {
        return Some((TrackKind::Audio, audio.duration));
    }
    None
}

/// Put a material at the end of the first lane of its kind.
pub fn append(project: &Project, material_id: &str) -> Result<EditCommand, String> {
    let (kind, duration) =
        placement(project, material_id).ok_or_else(|| format!("unknown material {material_id}"))?;
    if duration <= 0 {
        return Err("the material has no duration".into());
    }
    let track = project
        .tracks
        .iter()
        .find(|t| t.kind == kind && !t.locked)
        .ok_or("there is no unlocked lane for this kind of media")?;
    let start = track
        .segments
        .iter()
        .map(|s| s.target_range.start + s.target_range.duration)
        .max()
        .unwrap_or(0);

    Ok(EditCommand::InsertSegment {
        track_id: track.id.clone(),
        index: track.segments.len(),
        segment: Segment {
            id: new_id(),
            material_id: material_id.to_string(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        },
    })
}

/// Take a clip off the timeline.
pub fn remove(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    for track in &project.tracks {
        if let Some(index) = track.segments.iter().position(|s| s.id == segment_id) {
            return Ok(EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: track.segments[index].clone(),
                index,
            });
        }
    }
    Err(format!("unknown segment {segment_id}"))
}

/// Move a clip to `to_start` on `to_track`, refusing a place where it would
/// overlap another clip.
pub fn move_to(
    project: &Project,
    segment_id: &str,
    to_track: &str,
    to_start: Micros,
) -> Result<EditCommand, String> {
    let (from, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let target = project
        .track(to_track)
        .ok_or_else(|| format!("unknown track {to_track}"))?;
    if target.kind != from.kind {
        return Err("a clip can only move to a lane of its own kind".into());
    }
    let to_start = to_start.max(0);
    let end = to_start + segment.target_range.duration;
    let collides = target.segments.iter().any(|other| {
        other.id != segment.id
            && to_start < other.target_range.start + other.target_range.duration
            && other.target_range.start < end
    });
    if collides {
        return Err("another clip is in the way".into());
    }
    Ok(EditCommand::MoveSegment {
        segment_id: segment.id.clone(),
        from_track: from.id.clone(),
        to_track: target.id.clone(),
        from_start: segment.target_range.start,
        to_start,
    })
}
