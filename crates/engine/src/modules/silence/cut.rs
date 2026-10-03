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
/// segment `segment_id` and its linked partners, and closes the gaps on their
/// lanes only. See [`remove_ranges_in_sync`] for the variant that keeps every
/// other lane in step too.
pub fn remove_ranges(
    project: &Project,
    segment_id: &str,
    cuts: &[TimeRange],
    label: &str,
) -> Result<CutPlan, String> {
    remove_ranges_in_sync(project, segment_id, cuts, label, false)
}

/// [`remove_ranges`], and with `everything` set, every unlocked lane ripples
/// across each removed stretch as well — "Keep everything in sync".
///
/// What that means per clip on another lane, with `f(t)` the timeline instant
/// `t` lands on once the cuts are closed:
///
/// - after the cuts, or between them: moved to `f(start)`;
/// - a **text** clip (title or caption) a cut runs through: shortened to
///   `f(start)..f(end)`, and a caption's words are re-timed through `f`, so
///   each word stays on the sound that says it and one caption stays one
///   caption;
/// - any **other** clip a cut runs through (music, an overlay): cut the same
///   way the clip itself is — split at the cut's edges, the inside removed —
///   because "in sync" means everything after the cut plays against the same
///   frame of the take it did before;
/// - a clip entirely inside a cut goes, like a sliver.
///
/// Locked lanes stay where they are: a lock means the lane takes no edits.
pub fn remove_ranges_in_sync(
    project: &Project,
    segment_id: &str,
    cuts: &[TimeRange],
    label: &str,
    everything: bool,
) -> Result<CutPlan, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if track.locked {
        return Err("the clip's track is locked".into());
    }
    let clip = segment.target_range;
    let timeline = timeline_cuts(project, segment, cuts);
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

    let shift = |t: Micros| -> Micros {
        timeline
            .iter()
            .map(|c| (t.min(c.end()) - c.start).max(0))
            .sum()
    };

    // 2b. Keep everything in sync: every other unlocked lane loses the same
    //     stretches, before anything moves.
    let mut lanes = rippled_lanes(project, segment_id, clip.start);
    if everything {
        for command in sync_other_lanes(&sim, &timeline, &shift)? {
            record(&mut sim, command)?;
        }
        lanes.extend(
            sim.tracks
                .iter()
                .filter(|t| !t.locked)
                .map(|t| t.id.clone()),
        );
    }

    // 3. Close the gaps on every lane that has to stay in sync with the clip.
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
        .map_err(|error| {
            format!("the cuts would make clips overlap on a rippled lane ({error})")
        })?;
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

/// The edits that take the timeline stretches `cuts` out of every unlocked
/// clip that overlaps one — splitting, removing, or (text) shortening and
/// re-timing — leaving the moves to the caller. See [`remove_ranges_in_sync`].
fn sync_other_lanes(
    project: &Project,
    cuts: &[TimeRange],
    shift: &dyn Fn(Micros) -> Micros,
) -> Result<Vec<EditCommand>, String> {
    let landing = |t: Micros| t - shift(t);
    let overlaps = |s: &crate::modules::project::document::Segment| {
        cuts.iter()
            .any(|c| s.target_range.start < c.end() && c.start < s.target_range.end())
    };
    let mut sim = project.clone();
    let mut commands = Vec::new();
    let mut record = |sim: &mut Project, command: EditCommand| -> Result<(), String> {
        command.apply(sim)?;
        commands.push(command);
        Ok(())
    };

    // Text first: shortened in place, never split.
    let texts: Vec<String> = sim
        .tracks
        .iter()
        .filter(|t| !t.locked)
        .flat_map(|t| &t.segments)
        .filter(|s| overlaps(s) && sim.materials.text(&s.material_id).is_some())
        .map(|s| s.id.clone())
        .collect();
    for id in texts {
        let Some((track, segment)) = sim.segment(&id) else {
            continue;
        };
        let start = landing(segment.target_range.start);
        let duration = landing(segment.target_range.end()) - start;
        if duration < MIN_KEEP {
            let index = track.segments.iter().position(|s| s.id == id).unwrap_or(0);
            let command = EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: segment.clone(),
                index,
            };
            record(&mut sim, command)?;
            continue;
        }
        let segment = segment.clone();
        // Text runs at speed 1, so the source shortens with the timeline.
        let after_source = TimeRange::new(segment.source_range.start, duration);
        let trim = EditCommand::TrimSegment {
            segment_id: id.clone(),
            before_target: segment.target_range,
            before_source: segment.source_range,
            after_target: TimeRange::new(segment.target_range.start, duration),
            after_source,
        };
        record(&mut sim, trim)?;
        if let Some(retimed) = retime_caption(&sim, &segment, start, &landing) {
            record(&mut sim, retimed)?;
        }
    }

    // Everything else is cut like the clip: split at every edge inside it,
    // then remove what lies inside a cut. Splitting a linked clip splits its
    // partners too, so later lanes find theirs already cut. The text clips
    // shortened above are left out: until the moves they still sit over the
    // cuts in the old times, and would be cut a second time.
    let is_text = |sim: &Project, s: &crate::modules::project::document::Segment| {
        sim.materials.text(&s.material_id).is_some()
    };
    let edges: BTreeSet<Micros> = cuts.iter().flat_map(|c| [c.start, c.end()]).collect();
    for at in edges {
        loop {
            let piece = sim
                .tracks
                .iter()
                .filter(|t| !t.locked)
                .flat_map(|t| &t.segments)
                .find(|s| {
                    s.target_range.start < at
                        && at < s.target_range.end()
                        && overlaps(s)
                        && !is_text(&sim, s)
                })
                .map(|s| s.id.clone());
            let Some(piece) = piece else { break };
            let split = split_at(&sim, &piece, at)?;
            record(&mut sim, split)?;
        }
    }
    loop {
        let doomed = sim.tracks.iter().filter(|t| !t.locked).find_map(|t| {
            t.segments
                .iter()
                .position(|s| {
                    !is_text(&sim, s)
                        && cuts.iter().any(|c| {
                            s.target_range.start >= c.start && s.target_range.end() <= c.end()
                        })
                })
                .map(|index| (t.id.clone(), index, t.segments[index].clone()))
        });
        let Some((track_id, index, segment)) = doomed else {
            break;
        };
        record(
            &mut sim,
            EditCommand::RemoveSegment {
                track_id,
                segment,
                index,
            },
        )?;
    }
    Ok(commands)
}

/// A caption's words re-timed for a clip that will start at `start` once the
/// cuts close: every word through `landing`, back into the clip's source time.
fn retime_caption(
    project: &Project,
    segment: &crate::modules::project::document::Segment,
    start: Micros,
    landing: &dyn Fn(Micros) -> Micros,
) -> Option<EditCommand> {
    let material = project.materials.text(&segment.material_id)?;
    let caption = material.caption.as_ref()?;
    let shared = project
        .tracks
        .iter()
        .flat_map(|t| &t.segments)
        .filter(|s| s.material_id == material.id)
        .count()
        > 1;
    if caption.words.is_empty() || shared {
        return None;
    }
    let source = segment.source_range.start;
    let to_timeline = |w: Micros| w - source + segment.target_range.start;
    let to_source = |t: Micros| landing(t) - start + source;
    let mut after = material.clone();
    for word in &mut after.caption.as_mut()?.words {
        word.start = to_source(to_timeline(word.start));
        word.end = to_source(to_timeline(word.end)).max(word.start);
    }
    (after != *material).then(|| EditCommand::SetTextMaterial {
        before: material.clone(),
        after,
    })
}

/// Source-time cuts as merged, sorted timeline ranges inside the clip.
fn timeline_cuts(
    project: &Project,
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
    // Through the speed curve when there is one (`project::speed`).
    let map = project.materials.time_map(segment);
    let to_timeline = |t: Micros| -> Micros {
        let offset = if map.is_curved() {
            map.offset_of(t)
        } else {
            ((t - source.start) as f64 / speed).round() as Micros
        };
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
pub(crate) fn rippled_lanes(project: &Project, segment_id: &str, from: Micros) -> BTreeSet<String> {
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

    fn named(p: &Project, name: &str) -> Vec<(Micros, Micros, Micros)> {
        lane(p, p.tracks.iter().position(|t| t.name == name).unwrap())
    }

    /// The take of [`project`], with captions over it made before the cut.
    fn captioned() -> Project {
        use crate::modules::captions::{edit, CaptionStyle, Cue, TimedWord};
        let w = |text: &str, a: Micros, b: Micros| TimedWord {
            text: text.into(),
            start: a,
            end: b,
        };
        let mut p = project();
        let cues = [
            Cue {
                words: vec![w("one", S / 2, S)],
                ..Cue::new(S / 2, 3 * S / 2, "one")
            },
            Cue {
                words: vec![
                    w("two", 16 * S / 10, 19 * S / 10),
                    w("three", 32 * S / 10, 36 * S / 10),
                ],
                ..Cue::new(16 * S / 10, 4 * S, "two three")
            },
            Cue {
                words: vec![w("gone", 63 * S / 10, 7 * S)],
                ..Cue::new(62 * S / 10, 78 * S / 10, "gone")
            },
            Cue {
                words: vec![w("four", 86 * S / 10, 9 * S)],
                ..Cue::new(85 * S / 10, 95 * S / 10, "four")
            },
        ];
        let style = CaptionStyle::default_for(&p.canvas);
        let placed = edit::place(&p, &cues, &style, edit::PlaceOptions::default()).unwrap();
        p.materials.texts.extend(placed.materials);
        placed.command.apply(&mut p).unwrap();
        p
    }

    #[test]
    fn keeping_everything_in_sync_ripples_captions_and_music_in_one_step() {
        use crate::modules::captions::edit::cues;
        let mut p = captioned();
        let before = serde_json::to_value(&p).unwrap();
        let cuts = [TimeRange::new(2 * S, S), TimeRange::new(6 * S, 2 * S)];
        let plan = remove_ranges_in_sync(&p, "take-v", &cuts, "Remove silences", true).unwrap();
        let mut history = History::new();
        history.apply(&mut p, plan.command).unwrap();
        let errors: Vec<_> = p
            .validate()
            .into_iter()
            .filter(|i| i.severity == crate::modules::project::document::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");

        // The take is cut as without the option.
        assert_eq!(named(&p, "V1")[3], (7 * S, 5 * S, 20 * S));
        // The music lost the same stretches and closed up with the take.
        assert_eq!(
            named(&p, "A2"),
            vec![(0, 2 * S, 0), (2 * S, 3 * S, 3 * S), (5 * S, 12 * S, 8 * S)]
        );
        // Every caption word sits where its sound now is; the one inside a
        // pause went with it.
        let words: Vec<(String, Micros, Micros)> = cues(&p)
            .into_iter()
            .flat_map(|c| c.words)
            .map(|w| (w.text, w.start, w.end))
            .collect();
        assert_eq!(
            words,
            vec![
                ("one".into(), S / 2, S),
                ("two".into(), 16 * S / 10, 19 * S / 10),
                ("three".into(), 22 * S / 10, 26 * S / 10),
                ("four".into(), 56 * S / 10, 6 * S),
            ]
        );
        let texts: Vec<(Micros, Micros)> = cues(&p).iter().map(|c| (c.start, c.end)).collect();
        assert_eq!(
            texts,
            vec![
                (S / 2, 3 * S / 2),
                (16 * S / 10, 3 * S),
                (55 * S / 10, 65 * S / 10)
            ]
        );

        // One undo gives everything back, byte for byte.
        history.undo(&mut p).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), before);
    }

    #[test]
    fn without_sync_other_lanes_stay_put() {
        let mut p = captioned();
        let captions_before = named(&p, crate::modules::captions::edit::LANE_NAME);
        let plan =
            remove_ranges_in_sync(&p, "take-v", &[TimeRange::new(2 * S, S)], "Cut", false).unwrap();
        History::new().apply(&mut p, plan.command).unwrap();
        assert_eq!(
            named(&p, crate::modules::captions::edit::LANE_NAME),
            captions_before
        );
        assert_eq!(named(&p, "A2"), vec![(0, 20 * S, 0)]);
    }

    #[test]
    fn a_locked_lane_does_not_ripple_even_in_sync() {
        let mut p = captioned();
        p.tracks.iter_mut().find(|t| t.name == "A2").unwrap().locked = true;
        let plan =
            remove_ranges_in_sync(&p, "take-v", &[TimeRange::new(2 * S, S)], "Cut", true).unwrap();
        History::new().apply(&mut p, plan.command).unwrap();
        assert_eq!(named(&p, "A2"), vec![(0, 20 * S, 0)]);
    }
}
