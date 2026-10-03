//! Turning a reviewed list of cuts into one undoable edit.
//!
//! The edit is built by *running the existing edits on a copy*: split the clip
//! at every cut boundary with [`split_at`], remove the pieces that fall inside
//! a cut, and close every gap by moving what follows to the left. Each step is
//! applied to the copy as it is recorded, so the next step is computed against
//! the document exactly as it will be when the real one gets there, and the
//! recorded steps become one `Composite`: one entry on the undo stack, one
//! Ctrl+Z to get the whole take back.
//!
//! Reusing `split_at` rather than computing the pieces here is the point.
//! Splitting already knows how to cut a linked picture-and-sound pair at the
//! same instant and give each pair of halves its own link group, how to rebase
//! keyframes so a fade crossing a cut stays one fade, and that a transition
//! belongs to the left half. A second implementation of any of that would
//! drift from the first.
//!
//! ## Which lanes ripple
//!
//! The clip's lane and the lane of everything linked to it, closed over links:
//! if a clip later on one of those lanes is linked to a clip on a third lane,
//! the third lane moves too, or that later pair would come apart. Every other
//! lane — music, overlays, titles — stays where it is, which is CapCut's main
//! track behaviour and the only rule that does not silently move a music cue
//! the user placed by hand.

use std::collections::BTreeSet;

use crate::modules::project::document::{Micros, Project, TimeRange};
use crate::modules::timeline::ops::{split_at, EditCommand};

/// The shortest piece of a clip a cut may leave between two cuts or at an
/// edge: one frame at 30 fps. Anything shorter is a flash frame, never what
/// someone reviewing a list of pauses meant to keep, so it goes with its cut.
pub const MIN_KEEP: Micros = 34_000;

/// What applying a cut list did.
#[derive(Debug, Clone)]
pub struct CutPlan {
    pub command: EditCommand,
    /// Timeline time removed from the clip, in microseconds.
    pub removed: Micros,
    /// How many separate stretches were removed after merging.
    pub cuts: usize,
}

/// The edit that removes `cuts` — ranges in the clip's **source** time — from
/// segment `segment_id` and its linked partners, and closes the gaps.
pub fn remove_ranges(
    project: &Project,
    segment_id: &str,
    cuts: &[TimeRange],
    label: &str,
) -> Result<CutPlan, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if track.locked {
        return Err("the clip's track is locked".into());
    }
    let clip = segment.target_range;
    let timeline = timeline_cuts(segment, cuts);
    let timeline = absorb_slivers(clip, timeline);
    if timeline.is_empty() {
        return Err("there is nothing to remove".into());
    }
    let removed: Micros = timeline.iter().map(|c| c.duration).sum();
    if removed >= clip.duration {
        return Err("that would remove the whole clip — delete it instead".into());
    }

    let mut sim = project.clone();
    let mut commands = Vec::new();
    let mut record = |sim: &mut Project, command: EditCommand| -> Result<(), String> {
        command.apply(sim)?;
        commands.push(command);
        Ok(())
    };

    // 1. Split at every boundary strictly inside the clip.
    let lane = track.id.clone();
    let points: BTreeSet<Micros> = timeline
        .iter()
        .flat_map(|c| [c.start, c.end()])
        .filter(|t| *t > clip.start && *t < clip.end())
        .collect();
    for at in points {
        let piece = sim
            .track(&lane)
            .and_then(|t| {
                t.segments
                    .iter()
                    .find(|s| s.target_range.start < at && at < s.target_range.end())
            })
            .map(|s| s.id.clone())
            .ok_or("the clip changed while the cuts were being made")?;
        let split = split_at(&sim, &piece, at)?;
        record(&mut sim, split)?;
    }

    // 2. Remove every piece of the clip that lies inside a cut, with its
    //    linked partners when they lie inside it too.
    let doomed: Vec<String> = sim
        .track(&lane)
        .map(|t| {
            t.segments
                .iter()
                .filter(|s| {
                    timeline
                        .iter()
                        .any(|c| s.target_range.start >= c.start && s.target_range.end() <= c.end())
                })
                .map(|s| s.id.clone())
                .collect()
        })
        .unwrap_or_default();
    for id in doomed {
        let mut members = vec![id.clone()];
        if let Some(group) = sim.link_group_of(&id).cloned() {
            for (_, _, partner) in sim.link_members(&group) {
                let inside = timeline.iter().any(|c| {
                    partner.target_range.start >= c.start && partner.target_range.end() <= c.end()
                });
                if partner.id != id && inside {
                    members.push(partner.id.clone());
                }
            }
        }
        for member in members {
            let Some((track, index)) = sim.tracks.iter().find_map(|t| {
                t.segments
                    .iter()
                    .position(|s| s.id == member)
                    .map(|i| (t.id.clone(), i))
            }) else {
                continue;
            };
            let segment = sim.track(&track).expect("found above").segments[index].clone();
            record(
                &mut sim,
                EditCommand::RemoveSegment {
                    track_id: track,
                    segment,
                    index,
                },
            )?;
        }
    }

    // 3. Close the gaps on every lane that has to stay in sync with the clip.
    let lanes = rippled_lanes(project, segment_id, clip.start);
    let shift = |t: Micros| -> Micros {
        timeline
            .iter()
            .map(|c| (t.min(c.end()) - c.start).max(0))
            .sum()
    };
    let mut moves: Vec<(Micros, String, String)> = Vec::new();
    for track in sim.tracks.iter().filter(|t| lanes.contains(&t.id)) {
        for s in &track.segments {
            if s.target_range.start >= clip.start && shift(s.target_range.start) > 0 {
                moves.push((s.target_range.start, track.id.clone(), s.id.clone()));
            }
        }
    }
    // Everything moves left, so left to right never runs into a neighbour
    // that has not moved yet.
    moves.sort();
    for (from_start, track_id, segment_id) in moves {
        record(
            &mut sim,
            EditCommand::MoveSegment {
                segment_id,
                from_track: track_id.clone(),
                to_track: track_id,
                from_start,
                to_start: from_start - shift(from_start),
            },
        )
        .map_err(|error| format!("the cuts would make clips overlap on a linked lane ({error})"))?;
    }

    Ok(CutPlan {
        command: EditCommand::Composite {
            label: label.to_string(),
            commands,
        },
        removed,
        cuts: timeline.len(),
    })
}

/// Source-time cuts as merged, sorted timeline ranges inside the clip.
fn timeline_cuts(
    segment: &crate::modules::project::document::Segment,
    cuts: &[TimeRange],
) -> Vec<TimeRange> {
    let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
        segment.speed as f64
    } else {
        1.0
    };
    let source = segment.source_range;
    let clip = segment.target_range;
    let to_timeline = |t: Micros| -> Micros {
        let offset = ((t - source.start) as f64 / speed).round() as Micros;
        (clip.start + offset).clamp(clip.start, clip.end())
    };
    let mut ranges: Vec<(Micros, Micros)> = cuts
        .iter()
        .filter(|c| c.duration > 0)
        .map(|c| (to_timeline(c.start), to_timeline(c.end())))
        .filter(|(a, b)| b > a)
        .collect();
    ranges.sort();
    merge(ranges)
}

fn merge(ranges: Vec<(Micros, Micros)>) -> Vec<TimeRange> {
    let mut merged: Vec<(Micros, Micros)> = Vec::new();
    for (a, b) in ranges {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
        .into_iter()
        .map(|(a, b)| TimeRange::new(a, b - a))
        .collect()
}

/// Fold every kept piece shorter than [`MIN_KEEP`] into the cuts around it.
fn absorb_slivers(clip: TimeRange, cuts: Vec<TimeRange>) -> Vec<TimeRange> {
    let mut ranges: Vec<(Micros, Micros)> = cuts.iter().map(|c| (c.start, c.end())).collect();
    let mut kept_from = clip.start;
    let mut extra = Vec::new();
    for c in &cuts {
        if c.start > kept_from && c.start - kept_from < MIN_KEEP {
            extra.push((kept_from, c.start));
        }
        kept_from = c.end();
    }
    if clip.end() > kept_from && clip.end() - kept_from < MIN_KEEP && !cuts.is_empty() {
        extra.push((kept_from, clip.end()));
    }
    ranges.extend(extra);
    ranges.sort();
    merge(ranges)
}

/// The lanes whose clips after `from` must move with the cut clip.
fn rippled_lanes(project: &Project, segment_id: &str, from: Micros) -> BTreeSet<String> {
    let mut lanes = BTreeSet::new();
    let mut queue = vec![segment_id.to_string()];
    let mut seen = BTreeSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some((track, _)) = project.segment(&id) else {
            continue;
        };
        if lanes.insert(track.id.clone()) {
            // A newly rippled lane: everything on it from `from` on moves, so
            // whatever those clips are linked to has to move as well.
            for s in &track.segments {
                if s.target_range.start >= from {
                    queue.push(s.id.clone());
                }
            }
        }
        if let Some(group) = project.link_group_of(&id) {
            for (_, _, partner) in project.link_members(group) {
                queue.push(partner.id.clone());
            }
        }
    }
    lanes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AnimatableProperty, AudioMaterial, CanvasConfig, Easing, Keyframe, KeyframeTrack, Segment,
        Track, TrackKind, Transform, VideoMaterial, MICROS_PER_SECOND,
    };
    use crate::modules::timeline::History;

    const S: Micros = MICROS_PER_SECOND;

    fn seg(id: &str, material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: id.into(),
            material_id: material.into(),
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

    /// A talking-head take imported with its sound: picture on V1, sound on
    /// A1, linked; a second clip after it on V1 linked to its own sound on A1;
    /// music on A2 that must not move.
    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "take".into(),
            path: "/tmp/take.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 60 * S,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        p.materials.audios.push(AudioMaterial {
            id: "music".into(),
            path: "/tmp/music.mp3".into(),
            duration: 60 * S,
            sample_rate: 48_000,
            channels: 2,
        });
        p.materials.links.insert("g1".into());
        p.materials.links.insert("g2".into());

        let mut v = Track::new(TrackKind::Video, "V1");
        let mut a = Track::new(TrackKind::Audio, "A1");
        let mut take = seg("take-v", "take", 0, 10 * S);
        take.extras.push("g1".into());
        let mut take_a = seg("take-a", "take", 0, 10 * S);
        take_a.extras.push("g1".into());
        let mut next = seg("next-v", "take", 10 * S, 5 * S);
        next.source_range = TimeRange::new(20 * S, 5 * S);
        next.extras.push("g2".into());
        let mut next_a = next.clone();
        next_a.id = "next-a".into();
        v.segments = vec![take, next];
        a.segments = vec![take_a, next_a];
        let mut m = Track::new(TrackKind::Audio, "A2");
        m.segments = vec![seg("music", "music", 0, 20 * S)];
        p.tracks = vec![v, a, m];
        p
    }

    fn lane(p: &Project, index: usize) -> Vec<(Micros, Micros, Micros)> {
        p.tracks[index]
            .segments
            .iter()
            .map(|s| {
                (
                    s.target_range.start,
                    s.target_range.duration,
                    s.source_range.start,
                )
            })
            .collect()
    }

    #[test]
    fn two_pauses_come_out_of_the_picture_and_the_sound_together() {
        let mut p = project();
        let before = p.clone();
        let plan = remove_ranges(
            &p,
            "take-v",
            &[TimeRange::new(2 * S, S), TimeRange::new(6 * S, 2 * S)],
            "Remove silences",
        )
        .unwrap();
        assert_eq!(plan.removed, 3 * S);
        assert_eq!(plan.cuts, 2);

        let mut history = History::new();
        history.apply(&mut p, plan.command).unwrap();

        // Picture: [0,2) [3,6) [8,10) of the source, closed up, then the next
        // clip pulled left by three seconds.
        let want = vec![
            (0, 2 * S, 0),
            (2 * S, 3 * S, 3 * S),
            (5 * S, 2 * S, 8 * S),
            (7 * S, 5 * S, 20 * S),
        ];
        assert_eq!(lane(&p, 0), want);
        // Sound: identical, so the pair stays in sync piece for piece.
        assert_eq!(lane(&p, 1), want);
        // Music on the other lane did not move.
        assert_eq!(lane(&p, 2), vec![(0, 20 * S, 0)]);
        // Every piece of picture is linked to the piece of sound beside it.
        for (v, a) in p.tracks[0].segments.iter().zip(&p.tracks[1].segments) {
            let gv = p.materials.link_of(v).cloned();
            assert!(gv.is_some());
            assert_eq!(gv, p.materials.link_of(a).cloned());
        }
        assert!(p
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::document::Severity::Error));

        // One undo step gives the take back exactly.
        history.undo(&mut p).unwrap();
        assert_eq!(lane(&p, 0), lane(&before, 0));
        assert_eq!(lane(&p, 1), lane(&before, 1));
    }

    #[test]
    fn a_cut_at_the_head_keeps_the_clip_where_it_starts() {
        let mut p = project();
        let plan = remove_ranges(&p, "take-a", &[TimeRange::new(0, S)], "Cut").unwrap();
        History::new().apply(&mut p, plan.command).unwrap();
        assert_eq!(lane(&p, 0)[0], (0, 9 * S, S));
        assert_eq!(lane(&p, 1)[0], (0, 9 * S, S));
        assert_eq!(lane(&p, 0)[1], (9 * S, 5 * S, 20 * S));
    }

    #[test]
    fn source_time_maps_through_speed() {
        let mut p = project();
        // Make the take play at 2x: ten seconds of timeline read twenty of source.
        for t in &mut p.tracks[..2] {
            t.segments[0].speed = 2.0;
            t.segments[0].source_range = TimeRange::new(0, 20 * S);
        }
        // Two seconds of *source* at 2x is one second of timeline.
        let plan = remove_ranges(&p, "take-v", &[TimeRange::new(4 * S, 2 * S)], "Cut").unwrap();
        assert_eq!(plan.removed, S);
        History::new().apply(&mut p, plan.command).unwrap();
        assert_eq!(lane(&p, 0)[0], (0, 2 * S, 0));
        assert_eq!(lane(&p, 0)[1], (2 * S, 7 * S, 6 * S));
    }

    #[test]
    fn slivers_between_cuts_are_removed_with_them() {
        let p = project();
        let plan = remove_ranges(
            &p,
            "take-v",
            &[TimeRange::new(S, S), TimeRange::new(2 * S + 10_000, S)],
            "Cut",
        )
        .unwrap();
        assert_eq!(plan.cuts, 1);
        assert_eq!(plan.removed, 2 * S + 10_000);
    }

    #[test]
    fn removing_everything_is_refused() {
        let p = project();
        let error = remove_ranges(&p, "take-v", &[TimeRange::new(0, 10 * S)], "Cut").unwrap_err();
        assert!(error.contains("whole clip"));
        assert!(remove_ranges(&p, "take-v", &[], "Cut").is_err());
    }

    #[test]
    fn a_fade_out_on_the_tail_stays_on_the_tail() {
        let mut p = project();
        p.tracks[1].segments[0].keyframes.push(KeyframeTrack {
            property: AnimatableProperty::Volume,
            keyframes: vec![
                Keyframe {
                    time: 9 * S,
                    value: 1.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 10 * S,
                    value: 0.0,
                    easing: Easing::Linear,
                },
            ],
        });
        let plan = remove_ranges(&p, "take-v", &[TimeRange::new(5 * S, S)], "Cut").unwrap();
        History::new().apply(&mut p, plan.command).unwrap();
        let tail = &p.tracks[1].segments[1];
        assert_eq!(tail.target_range, TimeRange::new(5 * S, 4 * S));
        let kf = &tail.keyframes[0].keyframes;
        // Rebased onto the tail piece, which starts six seconds into the take.
        assert_eq!(kf.last().unwrap().time, 4 * S);
    }
}
