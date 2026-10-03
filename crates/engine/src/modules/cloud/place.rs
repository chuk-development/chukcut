//! Putting a generated or downloaded file on the timeline.
//!
//! Pure functions from the document to one `EditCommand`, like
//! `captions::edit`: the shell applies the command through the history, so
//! every placement is one undo step. Three gestures:
//!
//! - [`at_playhead`]: "Add to timeline" for a voiceover, a sound, a stock
//!   clip. The first lane of the right kind that is free there, or a new one.
//! - [`beside`]: a fal result on a new lane just above the clip it was made
//!   from, at the same time — a cut-out over its original.
//! - [`replace`]: a fal result in place of the clip, keeping its timing,
//!   transform, keyframes and links.

use crate::modules::project::{
    new_id, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
};
use crate::modules::timeline::ops::EditCommand;

/// How long a still lands on the timeline (as `edits::STILL_DURATION` in the
/// app).
pub const STILL_DURATION: Micros = 3_000_000;

/// The lane kind a material goes on and how long it runs.
pub fn placement(project: &Project, material_id: &str) -> Option<(TrackKind, Micros)> {
    let pool = &project.materials;
    if let Some(video) = pool.video(material_id) {
        return Some((TrackKind::Video, video.duration));
    }
    if pool.image(material_id).is_some() {
        return Some((TrackKind::Video, STILL_DURATION));
    }
    pool.audio(material_id)
        .map(|audio| (TrackKind::Audio, audio.duration))
}

fn segment(material_id: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
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
    }
}

fn index_in(track: &Track, start: Micros) -> usize {
    track
        .segments
        .iter()
        .filter(|s| s.target_range.start < start)
        .count()
}

/// `material_id` at `at`, on the first unlocked lane of its kind with room,
/// or on a new lane on top when none has.
pub fn at_playhead(
    project: &Project,
    material_id: &str,
    at: Micros,
) -> Result<EditCommand, String> {
    let (kind, duration) =
        placement(project, material_id).ok_or("that file is not in the project")?;
    if duration <= 0 {
        return Err("the file has no duration".to_string());
    }
    let start = at.max(0);
    let range = TimeRange::new(start, duration);
    let free = project
        .tracks
        .iter()
        .find(|t| t.kind == kind && !t.locked && t.is_range_free(&range, None));
    let new_segment = segment(material_id, start, duration);
    Ok(match free {
        Some(track) => EditCommand::InsertSegment {
            track_id: track.id.clone(),
            index: index_in(track, start),
            segment: new_segment,
        },
        None => {
            let name = match kind {
                TrackKind::Audio => "Audio",
                _ => "Video",
            };
            let track = Track::new(kind, name);
            let track_id = track.id.clone();
            EditCommand::Composite {
                label: "Add to timeline".to_string(),
                commands: vec![
                    EditCommand::AddTrack {
                        track,
                        index: project.tracks.len(),
                    },
                    EditCommand::InsertSegment {
                        track_id,
                        index: 0,
                        segment: new_segment,
                    },
                ],
            }
        }
    })
}

/// `material_id` on a new lane right above the lane of `segment_id`, at the
/// clip's start, trimmed to the clip's length.
pub fn beside(
    project: &Project,
    segment_id: &str,
    material_id: &str,
) -> Result<EditCommand, String> {
    let (track, original) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let (kind, duration) =
        placement(project, material_id).ok_or("that file is not in the project")?;
    let index = project
        .tracks
        .iter()
        .position(|t| t.id == track.id)
        .unwrap_or(project.tracks.len().saturating_sub(1));
    let length = duration.min(original.target_range.duration).max(1);
    let mut new_segment = segment(material_id, original.target_range.start, length);
    new_segment.source_range = TimeRange::new(0, length);
    new_segment.transform = original.transform;
    let lane = Track::new(kind, format!("{} (processed)", track.name));
    let lane_id = lane.id.clone();
    Ok(EditCommand::Composite {
        label: "Add processed clip".to_string(),
        commands: vec![
            EditCommand::AddTrack {
                track: lane,
                index: index + 1,
            },
            EditCommand::InsertSegment {
                track_id: lane_id,
                index: 0,
                segment: new_segment,
            },
        ],
    })
}

/// `material_id` in place of the clip `segment_id`: same lane, same time,
/// same transform, keyframes and links. A result shorter than the source
/// range the clip used is cut to fit.
pub fn replace(
    project: &Project,
    segment_id: &str,
    material_id: &str,
) -> Result<EditCommand, String> {
    let (track, original) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let (kind, duration) =
        placement(project, material_id).ok_or("that file is not in the project")?;
    if kind != track.kind {
        return Err("the result is not the same kind of media as the clip".to_string());
    }
    let index = track
        .segments
        .iter()
        .position(|s| s.id == original.id)
        .ok_or("the clip is no longer on the timeline")?;
    let mut new_segment = original.clone();
    new_segment.id = new_id();
    new_segment.material_id = material_id.to_string();
    if original.source_range.start + original.source_range.duration > duration {
        // The result is shorter (or the clip was trimmed into a tail the
        // result does not have): keep the start, cut the end, at the clip's
        // speed.
        let start = original
            .source_range
            .start
            .min(duration.saturating_sub(1).max(0));
        let source = (duration - start).max(1);
        let target = match project.materials.speed_curve_of(original) {
            Some(curve) => crate::modules::project::speed::curve_target_duration(
                &curve.points,
                TimeRange::new(start, source),
            ),
            None => ((source as f64) / f64::from(original.speed.max(0.01))).round() as Micros,
        };
        new_segment.source_range = TimeRange::new(start, source);
        new_segment.target_range = TimeRange::new(original.target_range.start, target.max(1));
    }
    Ok(EditCommand::Composite {
        label: "Replace with processed clip".to_string(),
        commands: vec![
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: original.clone(),
                index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: new_segment,
                index,
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{AudioMaterial, CanvasConfig, VideoMaterial};
    use crate::modules::timeline::history::History;

    fn project() -> Project {
        let mut project = Project::new("p", CanvasConfig::default(), 30.0);
        for (id, duration) in [("v1", 10_000_000), ("v2", 10_000_000), ("short", 4_000_000)] {
            project.materials.videos.push(VideoMaterial {
                id: id.into(),
                path: format!("/x/{id}.mp4"),
                width: 1920,
                height: 1080,
                duration,
                fps: 30.0,
                has_audio: false,
                rotation: 0,
            });
        }
        project.materials.audios.push(AudioMaterial {
            id: "a1".into(),
            path: "/x/a1.mp3".into(),
            duration: 2_000_000,
            sample_rate: 44_100,
            channels: 1,
        });
        let mut video = Track::new(TrackKind::Video, "Video");
        video.segments.push(segment("v1", 0, 10_000_000));
        project.tracks.push(video);
        project.tracks.push(Track::new(TrackKind::Audio, "Audio"));
        project
    }

    fn apply(project: &mut Project, command: EditCommand) {
        History::default().apply(project, command).unwrap();
        assert!(
            project
                .validate()
                .iter()
                .all(|i| i.severity != crate::modules::project::Severity::Error),
            "{:?}",
            project.validate()
        );
    }

    #[test]
    fn audio_goes_on_the_free_audio_lane_at_the_playhead() {
        let mut project = project();
        let command = at_playhead(&project, "a1", 3_000_000).unwrap();
        assert!(matches!(command, EditCommand::InsertSegment { .. }));
        apply(&mut project, command);
        assert_eq!(project.tracks[1].segments[0].target_range.start, 3_000_000);
    }

    #[test]
    fn a_busy_lane_gets_a_new_one_on_top() {
        let mut project = project();
        let command = at_playhead(&project, "v2", 2_000_000).unwrap();
        apply(&mut project, command);
        assert_eq!(project.tracks.len(), 3);
        assert_eq!(project.tracks[2].kind, TrackKind::Video);
        assert_eq!(project.tracks[2].segments[0].material_id, "v2");
    }

    #[test]
    fn a_processed_clip_sits_above_or_replaces_its_original() {
        let mut project = project();
        let clip = project.tracks[0].segments[0].id.clone();
        let command = beside(&project, &clip, "v2").unwrap();
        apply(&mut project, command);
        assert_eq!(project.tracks[1].name, "Video (processed)");
        assert_eq!(project.tracks[1].segments[0].target_range.start, 0);

        let mut project = self::project();
        let clip = project.tracks[0].segments[0].id.clone();
        let command = replace(&project, &clip, "short").unwrap();
        apply(&mut project, command);
        let replaced = &project.tracks[0].segments[0];
        assert_eq!(replaced.material_id, "short");
        assert_eq!(
            replaced.target_range.duration, 4_000_000,
            "cut to the shorter result"
        );
    }
}
