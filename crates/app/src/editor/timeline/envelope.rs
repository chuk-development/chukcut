//! What the timeline draws and edits inside a clip: keyframe diamonds and
//! the fade handles of sound clips.
//!
//! Keyframe times are relative to the clip's start (see
//! `docs/architecture/timeline-editing.md`, "Keyframes"), so a diamond sits at
//! `start + time` on the timeline and a drag changes the time by the
//! pointer's distance, never by anything scaled by speed.
//!
//! The `Volume` keyframe track is not keyframes to the user: it is the fade
//! envelope, owned by the inspector's fade rows and by the handles here. Both
//! read and write it by the same pattern — a fade-in is a first keyframe at 0
//! with value 0 followed by a 1, a fade-out the mirror at the end — and the
//! two copies of that pattern (`inspector::fades` / `fade_command`, and
//! [`fades`] / [`fade_command`] below) must stay the same. The tests here pin
//! the shape both sides write.

use chukcut_engine::modules::project::{AnimatableProperty, Easing, Keyframe, Micros, Segment};
use chukcut_engine::modules::timeline::ops::EditCommand;

// --- keyframes --------------------------------------------------------------

/// The instants with a keyframe on any animated property except the volume
/// envelope, sorted and distinct. One diamond stands for all the properties
/// keyed at that instant, as in CapCut.
pub(crate) fn instants(segment: &Segment) -> Vec<Micros> {
    let mut out: Vec<Micros> = segment
        .keyframes
        .iter()
        .filter(|t| t.property != AnimatableProperty::Volume)
        .flat_map(|t| t.keyframes.iter().map(|k| k.time))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Move every keyframe at `from` to `to` (clip-relative), as one step. The
/// time is kept inside the clip; an instant another keyframe of the same
/// property already holds is refused, because keyframe times are distinct.
pub(crate) fn retime(segment: &Segment, from: Micros, to: Micros) -> Result<EditCommand, String> {
    let to = to.clamp(0, segment.target_range.duration);
    if to == from {
        return Err("the keyframe did not move".into());
    }
    let mut commands = Vec::new();
    for track in segment
        .keyframes
        .iter()
        .filter(|t| t.property != AnimatableProperty::Volume)
    {
        let Some(keyframe) = track.keyframes.iter().find(|k| k.time == from) else {
            continue;
        };
        if track.keyframes.iter().any(|k| k.time == to) {
            return Err("another keyframe is already at that instant".into());
        }
        commands.push(EditCommand::MoveKeyframe {
            segment_id: segment.id.clone(),
            property: track.property,
            from_time: from,
            to_time: to,
            before_value: keyframe.value,
            after_value: keyframe.value,
        });
    }
    if commands.is_empty() {
        return Err("there is no keyframe there".into());
    }
    Ok(EditCommand::Composite {
        label: "Move keyframe".into(),
        commands,
    })
}

/// Delete every keyframe at `at` (clip-relative), as one step.
pub(crate) fn remove(segment: &Segment, at: Micros) -> Result<EditCommand, String> {
    let commands: Vec<EditCommand> = segment
        .keyframes
        .iter()
        .filter(|t| t.property != AnimatableProperty::Volume)
        .filter_map(|t| {
            t.keyframes
                .iter()
                .find(|k| k.time == at)
                .map(|&keyframe| EditCommand::RemoveKeyframe {
                    segment_id: segment.id.clone(),
                    property: t.property,
                    keyframe,
                })
        })
        .collect();
    if commands.is_empty() {
        return Err("there is no keyframe there".into());
    }
    Ok(EditCommand::Composite {
        label: "Delete keyframe".into(),
        commands,
    })
}

// --- fades -------------------------------------------------------------------

/// The fade-in and fade-out lengths the clip's `Volume` envelope describes.
/// The same reading as the inspector's; see the module docs.
pub(crate) fn fades(segment: &Segment) -> (Micros, Micros) {
    let Some(track) = segment
        .keyframes
        .iter()
        .find(|t| t.property == AnimatableProperty::Volume)
    else {
        return (0, 0);
    };
    let k = track.keyframes.as_slice();
    let len = segment.target_range.duration;
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
    let fade_in = match k {
        [first, second, ..]
            if first.time == 0 && near(first.value, 0.0) && near(second.value, 1.0) =>
        {
            second.time
        }
        _ => 0,
    };
    let fade_out = match k {
        [.., before, last]
            if last.time == len && near(last.value, 0.0) && near(before.value, 1.0) =>
        {
            len - before.time
        }
        _ => 0,
    };
    (fade_in, fade_out)
}

/// Rewrite the `Volume` envelope as the two fades, as one step. The same
/// writing as the inspector's; see the module docs.
pub(crate) fn fade_command(
    segment: &Segment,
    fade_in: Micros,
    fade_out: Micros,
) -> Result<EditCommand, String> {
    use AnimatableProperty as A;
    let len = segment.target_range.duration;
    let fade_in = fade_in.clamp(0, len);
    let fade_out = fade_out.clamp(0, len - fade_in);
    if (fade_in, fade_out) == fades(segment) {
        return Err("the fades did not change".into());
    }
    let mut commands: Vec<EditCommand> = segment
        .keyframes
        .iter()
        .filter(|t| t.property == A::Volume)
        .flat_map(|t| t.keyframes.iter())
        .map(|&keyframe| EditCommand::RemoveKeyframe {
            segment_id: segment.id.clone(),
            property: A::Volume,
            keyframe,
        })
        .collect();
    let mut points: Vec<(Micros, f32)> = Vec::new();
    if fade_in > 0 {
        points.push((0, 0.0));
        points.push((fade_in, 1.0));
    }
    if fade_out > 0 {
        let apex = len - fade_out;
        if points.last().map(|p| p.0) != Some(apex) {
            points.push((apex, 1.0));
        }
        points.push((len, 0.0));
    }
    commands.extend(
        points
            .into_iter()
            .map(|(time, value)| EditCommand::AddKeyframe {
                segment_id: segment.id.clone(),
                property: A::Volume,
                keyframe: Keyframe {
                    time,
                    value,
                    easing: Easing::Linear,
                },
            }),
    );
    Ok(EditCommand::Composite {
        label: "Change fade".into(),
        commands,
    })
}

/// Which fade a handle sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    In,
    Out,
}

/// The fade length a handle dragged to `pointer` (clip-relative time) asks
/// for: the distance from its own edge, kept inside what the other fade
/// leaves.
pub(crate) fn dragged_fade(side: Side, pointer: Micros, len: Micros, other: Micros) -> Micros {
    let room = (len - other).max(0);
    match side {
        Side::In => pointer.clamp(0, room),
        Side::Out => (len - pointer).clamp(0, room),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::timeline::batch::tests::clip;
    use chukcut_engine::modules::project::KeyframeTrack;
    use chukcut_engine::modules::project::{CanvasConfig, Project, Track, TrackKind};
    use chukcut_engine::modules::timeline::History;

    fn key(time: Micros, value: f32) -> Keyframe {
        Keyframe {
            time,
            value,
            easing: Easing::Linear,
        }
    }

    fn animated() -> Segment {
        let mut segment = clip(0, 4_000_000);
        segment.keyframes = vec![
            KeyframeTrack {
                property: AnimatableProperty::PositionX,
                keyframes: vec![key(0, 0.0), key(1_000_000, 10.0)],
            },
            KeyframeTrack {
                property: AnimatableProperty::Opacity,
                keyframes: vec![key(1_000_000, 0.5)],
            },
            KeyframeTrack {
                property: AnimatableProperty::Volume,
                keyframes: vec![key(0, 0.0), key(500_000, 1.0)],
            },
        ];
        segment
    }

    fn on_a_lane(segment: Segment) -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut lane = Track::new(TrackKind::Video, "V1");
        lane.segments.push(segment);
        project.tracks.push(lane);
        project
    }

    #[test]
    fn one_diamond_per_instant_and_none_for_the_fades() {
        assert_eq!(instants(&animated()), vec![0, 1_000_000]);
    }

    #[test]
    fn retiming_moves_every_property_keyed_there() {
        let segment = animated();
        let id = segment.id.clone();
        let mut project = on_a_lane(segment.clone());
        let command = retime(&segment, 1_000_000, 2_000_000).unwrap();
        let mut history = History::default();
        history.apply(&mut project, command).unwrap();
        let moved = &project.segment(&id).unwrap().1;
        assert_eq!(instants(moved), vec![0, 2_000_000]);
        history.undo(&mut project).unwrap();
        assert_eq!(
            instants(project.segment(&id).unwrap().1),
            vec![0, 1_000_000]
        );
    }

    #[test]
    fn retiming_onto_another_keyframe_is_refused() {
        assert!(retime(&animated(), 1_000_000, 0).is_err());
    }

    #[test]
    fn deleting_takes_the_whole_instant() {
        let segment = animated();
        let id = segment.id.clone();
        let mut project = on_a_lane(segment.clone());
        History::default()
            .apply(&mut project, remove(&segment, 1_000_000).unwrap())
            .unwrap();
        assert_eq!(instants(project.segment(&id).unwrap().1), vec![0]);
    }

    #[test]
    fn fades_round_trip_through_the_envelope() {
        let segment = animated();
        let id = segment.id.clone();
        assert_eq!(fades(&segment), (500_000, 0));
        let mut project = on_a_lane(segment.clone());
        let command = fade_command(&segment, 1_000_000, 2_000_000).unwrap();
        History::default().apply(&mut project, command).unwrap();
        assert_eq!(
            fades(project.segment(&id).unwrap().1),
            (1_000_000, 2_000_000)
        );
    }

    #[test]
    fn a_fade_handle_stops_at_the_other_fade() {
        assert_eq!(
            dragged_fade(Side::In, 3_000_000, 4_000_000, 2_000_000),
            2_000_000
        );
        assert_eq!(
            dragged_fade(Side::Out, 500_000, 4_000_000, 1_000_000),
            3_000_000
        );
        assert_eq!(dragged_fade(Side::Out, 5_000_000, 4_000_000, 0), 0);
        assert_eq!(dragged_fade(Side::In, -10, 4_000_000, 0), 0);
    }
}
