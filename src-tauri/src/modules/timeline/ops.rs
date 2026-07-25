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

use crate::modules::project::{Micros, Project, Segment, TimeRange, Track, Transform};

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
    /// Several commands that undo as one unit, applied in order.
    Composite {
        label: String,
        commands: Vec<EditCommand>,
    },
}

pub type EditResult = Result<(), String>;

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
            EditCommand::Composite { label, .. } => label.clone(),
        }
    }

    pub fn apply(&self, project: &mut Project) -> EditResult {
        match self {
            EditCommand::AddTrack { track, index } => {
                let index = (*index).min(project.tracks.len());
                project.tracks.insert(index, track.clone());
                reindex_render_order(project);
                Ok(())
            }

            EditCommand::RemoveTrack { index, .. } => {
                if *index >= project.tracks.len() {
                    return Err("track index out of range".into());
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
                let track = project
                    .track_mut(track_id)
                    .ok_or_else(|| format!("unknown track {track_id}"))?;
                if !track.is_range_free(&segment.target_range, None) {
                    return Err("target range is occupied".into());
                }
                let index = (*index).min(track.segments.len());
                track.segments.insert(index, segment.clone());
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
                if after_target.duration <= 0 {
                    return Err("trim would leave an empty clip".into());
                }
                let (track_id, _) = project
                    .segment(segment_id)
                    .map(|(t, s)| (t.id.clone(), s.id.clone()))
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;

                let track = project.track_mut(&track_id).expect("track existed");
                if !track.is_range_free(after_target, Some(segment_id)) {
                    return Err("trim would overlap a neighbouring clip".into());
                }

                let segment = project.segment_mut(segment_id).expect("segment existed");
                segment.target_range = *after_target;
                segment.source_range = *after_source;
                sort_track(project, &track_id);
                Ok(())
            }

            EditCommand::SetTransform {
                segment_id, after, ..
            } => {
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.transform = *after;
                Ok(())
            }

            EditCommand::SetSpeed {
                segment_id, after, ..
            } => {
                if *after <= 0.0 {
                    return Err("speed must be positive".into());
                }
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.speed = *after;
                Ok(())
            }

            EditCommand::SetVolume {
                segment_id, after, ..
            } => {
                let segment = project
                    .segment_mut(segment_id)
                    .ok_or_else(|| format!("unknown segment {segment_id}"))?;
                segment.volume = after.clamp(0.0, 4.0);
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
    let source_split = (left_duration as f64 * segment.speed as f64) as Micros;

    let left_target = TimeRange::new(segment.target_range.start, left_duration);
    let left_source = TimeRange::new(segment.source_range.start, source_split);

    let mut right = segment.clone();
    right.id = crate::modules::project::new_id();
    right.target_range = TimeRange::new(at, right_duration);
    right.source_range = TimeRange::new(
        segment.source_range.start + source_split,
        segment.source_range.duration - source_split,
    );

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
