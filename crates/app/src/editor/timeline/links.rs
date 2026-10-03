//! A video clip's sound, as CapCut shows it: inside the clip until the user
//! detaches it.
//!
//! Decision 0005 describes the engine's model — a clip and its sound as two
//! linked segments of one material, the sound on an audio lane — and the
//! engine already does everything that model needs: linked clips move, trim,
//! split and delete together, and a picture whose sound sits on a linked
//! audio lane stays silent itself (`Project::sound_is_on_a_linked_lane`), in
//! the preview and in the export.
//!
//! The timeline keeps CapCut's default on top of that: an imported video is
//! one clip and plays its own sound. "Detach audio" is what turns it into the
//! 0005 pair. The detached picture's volume goes to zero in the same step,
//! so a later "Unlink" leaves a silent picture and an independent sound —
//! exactly what CapCut's detach leaves — rather than the same sound twice.

use chukcut_engine::modules::project::{
    new_id, AnimatableProperty, Project, Segment, Track, TrackKind,
};
use chukcut_engine::modules::timeline::ops::EditCommand;

use super::clipboard::new_lane;

/// Whether the video clip `segment_id` carries sound that has not been
/// detached yet.
pub(crate) fn can_detach(project: &Project, segment_id: &str) -> bool {
    let Some((track, segment)) = project.segment(segment_id) else {
        return false;
    };
    track.kind == TrackKind::Video
        && project
            .materials
            .videos
            .iter()
            .any(|m| m.id == segment.material_id && m.has_audio)
        && !project.sound_is_on_a_linked_lane(track, segment)
}

/// Whether the clip's sound is played by a linked clip on an audio lane: the
/// timeline then draws the picture without its sound strip.
pub(crate) fn is_detached(project: &Project, track: &Track, segment: &Segment) -> bool {
    project.sound_is_on_a_linked_lane(track, segment)
}

/// Detach a video clip's sound onto an audio lane: a new clip of the same
/// material and the same ranges, on the first audio lane with room (or a new
/// one), linked to the picture, and the picture muted. One undo step.
pub(crate) fn detach_audio(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    if !can_detach(project, segment_id) {
        return Err("this clip has no sound to detach".into());
    }
    let (_, picture) = project.segment(segment_id).ok_or("the clip is gone")?;
    let mut commands = Vec::new();

    let lane = project
        .tracks
        .iter()
        .find(|t| {
            t.kind == TrackKind::Audio && !t.locked && t.is_range_free(&picture.target_range, None)
        })
        .map(|t| t.id.clone());
    let lane = match lane {
        Some(id) => id,
        None => {
            let (track, index) = new_lane(project, TrackKind::Audio);
            let id = track.id.clone();
            commands.push(EditCommand::AddTrack { track, index });
            id
        }
    };

    let mut sound = picture.clone();
    sound.id = new_id();
    // A link and a transition belong to the picture; colour, crop and the
    // transform mean nothing to sound but are harmless and keep the clip a
    // faithful copy. Only the volume envelope (the fades) is animation that
    // sound has.
    sound.extras.retain(|id| {
        project.materials.transition(id).is_none() && !project.materials.links.contains(id)
    });
    sound
        .keyframes
        .retain(|t| t.property == AnimatableProperty::Volume);
    let sound_id = sound.id.clone();
    let index = project
        .track(&lane)
        .map(|t| {
            t.segments
                .iter()
                .filter(|s| s.target_range.start < sound.target_range.start)
                .count()
        })
        .unwrap_or(0);
    commands.push(EditCommand::InsertSegment {
        track_id: lane,
        segment: sound,
        index,
    });

    // The picture keeps a group it already has; otherwise the pair gets one.
    let group = project
        .materials
        .link_of(picture)
        .cloned()
        .unwrap_or_else(new_id);
    if project.materials.link_of(picture).is_none() {
        commands.push(EditCommand::SetLinkGroup {
            segment_id: picture.id.clone(),
            before: None,
            after: Some(group.clone()),
        });
    }
    commands.push(EditCommand::SetLinkGroup {
        segment_id: sound_id,
        before: None,
        after: Some(group),
    });
    if picture.volume != 0.0 {
        commands.push(EditCommand::SetVolume {
            segment_id: picture.id.clone(),
            before: picture.volume,
            after: 0.0,
        });
    }
    Ok(EditCommand::Composite {
        label: "Detach audio".into(),
        commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::timeline::batch::tests::project;
    use chukcut_engine::modules::project::VideoMaterial;
    use chukcut_engine::modules::timeline::History;

    fn with_sound(project: &mut Project) {
        let material = VideoMaterial {
            id: "m".into(),
            path: "/x.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        };
        project.materials.videos.push(material);
    }

    #[test]
    fn detaching_makes_a_linked_silent_pair_and_undoes_in_one_step() {
        let (mut project, ids) = project(&[2_000_000]);
        with_sound(&mut project);
        assert!(can_detach(&project, &ids[0]));
        let command = detach_audio(&project, &ids[0]).unwrap();
        let mut history = History::default();
        history.apply(&mut project, command).unwrap();

        let sound = &project.tracks[2].segments[0];
        assert_eq!(sound.material_id, "m");
        assert_eq!(
            sound.target_range,
            project.tracks[0].segments[0].target_range
        );
        assert_eq!(
            project.link_group_of(&ids[0]),
            project.link_group_of(&sound.id)
        );
        assert_eq!(project.tracks[0].segments[0].volume, 0.0);
        assert!(!can_detach(&project, &ids[0]), "already detached");

        history.undo(&mut project).unwrap();
        assert!(project.tracks[2].segments.is_empty());
        assert_eq!(project.tracks[0].segments[0].volume, 1.0);
        assert!(project.link_group_of(&ids[0]).is_none());
    }

    #[test]
    fn a_busy_audio_lane_makes_room_with_a_new_one() {
        let (mut project, ids) = project(&[2_000_000]);
        with_sound(&mut project);
        project.tracks[2]
            .segments
            .push(crate::editor::timeline::batch::tests::clip(0, 5_000_000));
        let command = detach_audio(&project, &ids[0]).unwrap();
        History::default().apply(&mut project, command).unwrap();
        assert_eq!(project.tracks.len(), 4);
        assert_eq!(project.tracks[3].kind, TrackKind::Audio);
    }

    #[test]
    fn a_silent_video_has_nothing_to_detach() {
        let (project, ids) = project(&[2_000_000]);
        assert!(!can_detach(&project, &ids[0]));
        assert!(detach_audio(&project, &ids[0]).is_err());
    }
}
