//! Edit commands for the whole-clip gestures a shell offers: put a material
//! on the timeline, take a clip off, move it, trim it.
//!
//! Each function reads the document and returns the primitive commands that do
//! what the gesture means; the caller applies them through the history
//! (`timeline_apply`, or `timeline_apply_many` when there is more than one), so
//! undo, link mirroring and validation behave as for any other edit.
//!
//! The app builds the same gestures in `crates/app/src/edits.rs` and
//! `editor/timeline/ripple.rs`. These are the shell-independent versions, which
//! `chukcut-cli` and its MCP server use; the rules (clip lengths, clamping,
//! ripple order) are the app's, so a script and a mouse produce the same
//! document. Moving the app onto these is a refactor for whoever next owns
//! those files.

use crate::modules::project::{
    new_id, source_duration_for, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
};

use super::ops::EditCommand;

/// How long a still lands on the timeline when no length is given.
pub const STILL_DURATION: Micros = 3_000_000;

/// The shortest clip a trim may leave: one frame at 30 fps, rounded up.
pub const MIN_CLIP: Micros = 34_000;

/// The kind of lane a material belongs on, and how long it naturally runs.
///
/// `None` for an id that is not a placeable material — a title, an effect or a
/// colour adjustment have commands of their own.
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

/// How far into its material a segment may read, if the material has an end.
/// Stills can be stretched without limit.
pub fn source_limit(project: &Project, material_id: &str) -> Option<Micros> {
    let pool = &project.materials;
    if let Some(video) = pool.video(material_id) {
        return Some(video.duration);
    }
    pool.audio(material_id).map(|audio| audio.duration)
}

fn new_segment(material_id: &str, target: TimeRange, source_start: Micros) -> Segment {
    Segment {
        id: new_id(),
        material_id: material_id.to_string(),
        target_range: target,
        source_range: TimeRange::new(source_start, target.duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// Where a segment that starts at `start` goes in a lane's sorted list.
fn insertion_index(track: &Track, start: Micros) -> usize {
    track
        .segments
        .iter()
        .filter(|s| s.target_range.start < start)
        .count()
}

/// Where a clip of `duration` at `start` on `track` can sit without
/// overlapping another clip (ignoring the clips in `skip`).
///
/// - Clear of every clip: `start`, unchanged.
/// - Overlapping a clip by less than `slack`: flush against that clip's
///   edge, when that place is clear.
/// - Otherwise `None`.
///
/// This is what a gesture that rounds an edge onto the frame grid calls with
/// one frame as the slack. A clip whose edge is off the grid (placed before
/// frame snapping, or a video whose length is not a whole number of frames)
/// would otherwise catch a rounded neighbour by a few microseconds, and the
/// move or drop was refused ("another clip is in the way") or, for a drop,
/// fell back to the end of the lane. Touching the off-grid edge wins over
/// the grid: a sub-frame gap or overlap is worse than an edge between frames.
pub fn clear_of_neighbours(
    track: &Track,
    skip: &[String],
    start: Micros,
    duration: Micros,
    slack: Micros,
) -> Option<Micros> {
    let others = || {
        track
            .segments
            .iter()
            .filter(|s| !skip.iter().any(|id| *id == s.id))
    };
    let overlapping = |at: Micros| {
        others()
            .filter(move |o| at < o.target_range.end() && o.target_range.start < at + duration)
            .collect::<Vec<_>>()
    };
    let hits = overlapping(start);
    if hits.is_empty() {
        return Some(start);
    }
    hits.iter()
        .filter_map(|other| {
            let (into_right, into_left) = (
                other.target_range.end() - start,
                start + duration - other.target_range.start,
            );
            if other.target_range.start <= start && into_right < slack {
                Some(other.target_range.end())
            } else if other.target_range.start > start && into_left < slack {
                Some(other.target_range.start - duration)
            } else {
                None
            }
        })
        .find(|&at| at >= 0 && overlapping(at).is_empty())
}

/// Put a material at the end of the first unlocked lane of its kind: the
/// app's "add to timeline" button.
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
        .map(|s| s.target_range.end())
        .max()
        .unwrap_or(0);
    Ok(EditCommand::InsertSegment {
        track_id: track.id.clone(),
        index: track.segments.len(),
        segment: new_segment(material_id, TimeRange::new(start, duration), 0),
    })
}

/// Put a material on the timeline at `at`.
///
/// - `track_id` names the lane; it must be of the material's kind, unlocked,
///   and free for the clip's whole length. Without one, the first lane of the
///   right kind with room is used, and when none has room a new lane is added
///   above the last lane of that kind — what dropping a clip onto an occupied
///   spot does in CapCut.
/// - `source_in` is where in the material the clip starts reading.
/// - `duration` defaults to the rest of the material from `source_in` (three
///   seconds for a still), and is clamped to it.
pub fn place(
    project: &Project,
    material_id: &str,
    track_id: Option<&str>,
    at: Micros,
    duration: Option<Micros>,
    source_in: Micros,
) -> Result<EditCommand, String> {
    let (kind, natural) =
        placement(project, material_id).ok_or_else(|| format!("unknown material {material_id}"))?;
    if at < 0 {
        return Err("a clip cannot start before the beginning of the timeline".into());
    }
    if source_in < 0 {
        return Err("the in point cannot be before the start of the material".into());
    }
    let limit = source_limit(project, material_id);
    let available = match limit {
        Some(limit) => {
            if source_in >= limit {
                return Err("the in point is past the end of the material".into());
            }
            limit - source_in
        }
        None => Micros::MAX,
    };
    let duration = duration.unwrap_or(natural.min(available)).min(available);
    if duration < MIN_CLIP {
        return Err("the clip would be shorter than one frame".into());
    }
    let target = TimeRange::new(at, duration);

    let chosen = match track_id {
        Some(id) => {
            let track = project
                .track(id)
                .ok_or_else(|| format!("unknown track {id}"))?;
            if track.kind != kind {
                return Err(format!(
                    "{} is a {:?} lane and this material belongs on a {:?} lane",
                    track.name, track.kind, kind
                ));
            }
            if track.locked {
                return Err(format!("{} is locked", track.name));
            }
            if !track.is_range_free(&target, None) {
                return Err(format!("another clip is in the way on {}", track.name));
            }
            Some(track)
        }
        None => project
            .tracks
            .iter()
            .find(|t| t.kind == kind && !t.locked && t.is_range_free(&target, None)),
    };

    if let Some(track) = chosen {
        return Ok(EditCommand::InsertSegment {
            track_id: track.id.clone(),
            index: insertion_index(track, at),
            segment: new_segment(material_id, target, source_in),
        });
    }

    // A new lane, directly above the last lane of this kind so a new overlay
    // does not jump over the titles and captions stacked above the pictures.
    let index = project
        .tracks
        .iter()
        .rposition(|t| t.kind == kind)
        .map_or(project.tracks.len(), |i| i + 1);
    let count = project.tracks.iter().filter(|t| t.kind == kind).count();
    let name = match kind {
        TrackKind::Audio => format!("Audio {}", count + 1),
        _ => format!("Video {}", count + 1),
    };
    let track = Track::new(kind, name);
    let track_id = track.id.clone();
    Ok(EditCommand::Composite {
        label: "Add clip".into(),
        commands: vec![
            EditCommand::AddTrack { track, index },
            EditCommand::InsertSegment {
                track_id,
                index: 0,
                segment: new_segment(material_id, target, source_in),
            },
        ],
    })
}

/// Take a clip off the timeline; with `ripple`, pull everything after it on
/// the lane left into the hole it leaves.
pub fn remove(
    project: &Project,
    segment_id: &str,
    ripple: bool,
) -> Result<Vec<EditCommand>, String> {
    let (track, index) = project
        .tracks
        .iter()
        .find_map(|t| {
            t.segments
                .iter()
                .position(|s| s.id == segment_id)
                .map(|i| (t, i))
        })
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let segment = &track.segments[index];
    let mut commands = vec![EditCommand::RemoveSegment {
        track_id: track.id.clone(),
        segment: segment.clone(),
        index,
    }];
    if ripple {
        if let Some(next) = track.segments.get(index + 1) {
            let shift = segment.target_range.start - next.target_range.start;
            commands.extend(shifted(
                track,
                segment.target_range.start,
                Some(segment_id),
                shift,
            ));
        }
    }
    Ok(commands)
}

/// Move a clip to `to_start` on `to_track` (its own lane when `None`),
/// refusing a place where it would overlap another clip.
pub fn move_to(
    project: &Project,
    segment_id: &str,
    to_track: Option<&str>,
    to_start: Micros,
) -> Result<EditCommand, String> {
    let (from, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let target = match to_track {
        Some(id) => project
            .track(id)
            .ok_or_else(|| format!("unknown track {id}"))?,
        None => from,
    };
    if target.kind != from.kind {
        return Err("a clip can only move to a lane of its own kind".into());
    }
    let to_start = to_start.max(0);
    let range = TimeRange::new(to_start, segment.target_range.duration);
    if !target.is_range_free(&range, Some(&segment.id)) {
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

/// Which edge of a clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Head,
    Tail,
}

/// The ranges a segment has after its `edge` is moved to the timeline time
/// `to`, clamped so the clip keeps at least [`MIN_CLIP`], reads nothing before
/// the start of its material and nothing past `limit`.
///
/// With `anchored`, a head trim keeps the clip's start where it is and only
/// shortens it — what a rippled head trim looks like.
pub fn trimmed(
    segment: &Segment,
    edge: Edge,
    to: Micros,
    limit: Option<Micros>,
    anchored: bool,
) -> (TimeRange, TimeRange) {
    let target = segment.target_range;
    let source = segment.source_range;
    let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
        segment.speed as f64
    } else {
        1.0
    };
    match edge {
        Edge::Head => {
            // How far the head may move left: back to the start of the
            // material, and never before the start of the timeline.
            let earliest = target.start - (source.start as f64 / speed).floor() as Micros;
            let earliest = if anchored { earliest } else { earliest.max(0) };
            let to = to.clamp(earliest, target.end() - MIN_CLIP);
            let delta = to - target.start;
            let duration = target.duration - delta;
            let start = if anchored { target.start } else { to };
            let source_start = (source.start + source_duration_for(delta, segment.speed)).max(0);
            (
                TimeRange::new(start, duration),
                TimeRange::new(source_start, source_duration_for(duration, segment.speed)),
            )
        }
        Edge::Tail => {
            let mut latest = Micros::MAX;
            if let Some(limit) = limit {
                latest = target.start + ((limit - source.start).max(0) as f64 / speed) as Micros;
            }
            let to = to.clamp(target.start + MIN_CLIP, latest.max(target.start + MIN_CLIP));
            let duration = to - target.start;
            (
                TimeRange::new(target.start, duration),
                TimeRange::new(source.start, source_duration_for(duration, segment.speed)),
            )
        }
    }
}

/// Move one edge of a clip to timeline time `to`, clamped as [`trimmed`]
/// clamps. With `ripple`, every later clip on the lane shifts by however much
/// the clip's end moved and a head trim keeps the clip's start, so the lane
/// keeps its spacing. Empty when the clamped trim changes nothing.
pub fn trim_edge(
    project: &Project,
    segment_id: &str,
    edge: Edge,
    to: Micros,
    ripple: bool,
) -> Result<Vec<EditCommand>, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let limit = source_limit(project, &segment.material_id);
    let (target, source) = trimmed(segment, edge, to, limit, ripple);
    trim(project, segment_id, target, source, ripple)
}

/// A trim to exact ranges, and with `ripple` every later clip on the lane
/// shifted by however much the clip's end moved.
pub fn trim(
    project: &Project,
    segment_id: &str,
    target: TimeRange,
    source: TimeRange,
    ripple: bool,
) -> Result<Vec<EditCommand>, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if target == segment.target_range && source == segment.source_range {
        return Ok(Vec::new());
    }
    let command = EditCommand::TrimSegment {
        segment_id: segment.id.clone(),
        before_target: segment.target_range,
        before_source: segment.source_range,
        after_target: target,
        after_source: source,
    };
    if !ripple {
        return Ok(vec![command]);
    }
    let shift = target.end() - segment.target_range.end();
    let moves = shifted(track, segment.target_range.start, Some(&segment.id), shift);
    // A lane that closes up trims first and then moves left to right; one
    // that opens up moves right to left first and then grows into the room.
    Ok(if shift <= 0 {
        std::iter::once(command).chain(moves).collect()
    } else {
        moves
            .into_iter()
            .rev()
            .chain(std::iter::once(command))
            .collect()
    })
}

/// Moves for every clip on `track` that starts after `after`, by `shift`,
/// left to right. `skip` is left out.
fn shifted(track: &Track, after: Micros, skip: Option<&str>, shift: Micros) -> Vec<EditCommand> {
    if shift == 0 {
        return Vec::new();
    }
    track
        .segments
        .iter()
        .filter(|s| s.target_range.start > after && Some(s.id.as_str()) != skip)
        .map(|s| EditCommand::MoveSegment {
            segment_id: s.id.clone(),
            from_track: track.id.clone(),
            to_track: track.id.clone(),
            from_start: s.target_range.start,
            to_start: (s.target_range.start + shift).max(0),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CanvasConfig, VideoMaterial};
    use crate::modules::timeline::ops::compose_edits;
    use crate::state::DocumentHistory;

    const S: Micros = 1_000_000;

    fn project() -> (Project, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.tracks.push(Track::new(TrackKind::Video, "Video 1"));
        project.tracks.push(Track::new(TrackKind::Audio, "Audio 1"));
        let id = new_id();
        project.materials.videos.push(VideoMaterial {
            id: id.clone(),
            path: "/nowhere/clip.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 4 * S,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        (project, id)
    }

    #[test]
    fn a_rounded_edge_slides_flush_against_an_off_grid_neighbour() {
        use crate::modules::preview::clock::nearest_frame_time;
        let frame = 33_334;
        let mut track = Track::new(TrackKind::Video, "V");
        // A clip placed before frame snapping: it ends between frames 90
        // and 91 (3.016761 s, the cut QA found).
        track
            .segments
            .push(new_segment("a", TimeRange::new(0, 3_016_761), 0));
        track
            .segments
            .push(new_segment("c", TimeRange::new(8 * S + 5_000, 2 * S), 0));
        let skip: Vec<String> = Vec::new();

        // Dropped just after it, the head rounds back to frame 90 and would
        // overlap by 16 761 µs: it slides to the edge instead.
        let raw = 3_010_000;
        let rounded = nearest_frame_time(raw, 30.0);
        assert_eq!(rounded, 3 * S);
        let at = clear_of_neighbours(&track, &skip, rounded, 2 * S, frame).unwrap();
        assert_eq!(at, 3_016_761);

        // A tail that rounds into the next clip's off-grid head slides back.
        let at = clear_of_neighbours(&track, &skip, 6 * S + 10_000, 2 * S, frame).unwrap();
        assert_eq!(at, 6 * S + 5_000);
        assert!(at + 2 * S <= 8 * S + 5_000);

        // Clear already: unchanged. A real overlap: refused.
        assert_eq!(
            clear_of_neighbours(&track, &skip, 4 * S, S, frame),
            Some(4 * S)
        );
        assert_eq!(clear_of_neighbours(&track, &skip, 2 * S, S, frame), None);

        // A gap one frame too short for the clip: no flush place is clear.
        assert_eq!(
            clear_of_neighbours(&track, &skip, 3_016_761, 5 * S, frame),
            None
        );

        // The clip being moved does not collide with itself.
        let own = vec![track.segments[0].id.clone()];
        assert_eq!(
            clear_of_neighbours(&track, &own, 2 * S, S, frame),
            Some(2 * S)
        );
    }

    #[test]
    fn a_flush_place_is_accepted_by_the_move_it_feeds() {
        let (mut project, id) = project();
        let mut history = DocumentHistory::default();
        let track_id = project.tracks[0].id.clone();
        // Two clips, the first ending off the frame grid.
        let command = place(&project, &id, Some(&track_id), 0, Some(3_016_761), 0).unwrap();
        apply(&mut project, &mut history, vec![command]);
        let command = place(&project, &id, Some(&track_id), 6 * S, Some(S), 0).unwrap();
        apply(&mut project, &mut history, vec![command]);
        let moving = project.tracks[0].segments[1].id.clone();
        let rounded = 3 * S; // the frame boundary nearest to where it was let go
        assert!(move_to(&project, &moving, Some(&track_id), rounded).is_err());
        let at = clear_of_neighbours(
            &project.tracks[0],
            std::slice::from_ref(&moving),
            rounded,
            S,
            33_334,
        )
        .unwrap();
        let command = move_to(&project, &moving, Some(&track_id), at).unwrap();
        apply(&mut project, &mut history, vec![command]);
        assert_eq!(project.tracks[0].segments[1].target_range.start, 3_016_761);
        assert!(project
            .validate()
            .iter()
            .all(|i| !i.message.contains("overlap")));
    }

    fn apply(project: &mut Project, history: &mut DocumentHistory, commands: Vec<EditCommand>) {
        let command = compose_edits(project, "test", commands).expect("composes");
        history.apply(project, command).expect("applies");
    }

    fn starts(project: &Project) -> Vec<Micros> {
        project.tracks[0]
            .segments
            .iter()
            .map(|s| s.target_range.start)
            .collect()
    }

    #[test]
    fn append_lays_clips_end_to_end() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        for _ in 0..2 {
            let c = append(&p, &m).unwrap();
            h.apply(&mut p, c).unwrap();
        }
        assert_eq!(starts(&p), vec![0, 4 * S]);
    }

    #[test]
    fn place_on_an_occupied_spot_opens_a_new_lane_and_undoes_in_one_step() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        let c = place(&p, &m, None, 0, None, 0).unwrap();
        h.apply(&mut p, c).unwrap();
        let c = place(&p, &m, None, S, Some(S), S).unwrap();
        h.apply(&mut p, c).unwrap();

        assert_eq!(p.tracks.len(), 3);
        assert_eq!(
            p.tracks[1].kind,
            TrackKind::Video,
            "above the first video lane"
        );
        let overlay = &p.tracks[1].segments[0];
        assert_eq!(overlay.target_range, TimeRange::new(S, S));
        assert_eq!(overlay.source_range, TimeRange::new(S, S));
        assert!(p
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::Severity::Error));

        h.undo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 2, "the lane goes with the clip");
    }

    #[test]
    fn place_refuses_what_the_material_cannot_supply() {
        let (p, m) = project();
        assert!(
            place(&p, &m, None, 0, None, 4 * S).is_err(),
            "in point at the end"
        );
        assert!(place(&p, &m, None, -1, None, 0).is_err(), "before zero");
        let audio = p.tracks[1].id.clone();
        assert!(
            place(&p, &m, Some(&audio), 0, None, 0).is_err(),
            "wrong lane kind"
        );
        // A duration past the material is clamped, not refused.
        let EditCommand::InsertSegment { segment, .. } =
            place(&p, &m, None, 0, Some(10 * S), 3 * S).unwrap()
        else {
            panic!("one insert");
        };
        assert_eq!(segment.target_range.duration, S);
    }

    #[test]
    fn ripple_delete_closes_the_hole_and_a_plain_one_does_not() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        for _ in 0..3 {
            let c = append(&p, &m).unwrap();
            h.apply(&mut p, c).unwrap();
        }
        let first = p.tracks[0].segments[0].id.clone();
        let c = remove(&p, &first, true).unwrap();
        apply(&mut p, &mut h, c);
        assert_eq!(starts(&p), vec![0, 4 * S]);

        let first = p.tracks[0].segments[0].id.clone();
        let c = remove(&p, &first, false).unwrap();
        apply(&mut p, &mut h, c);
        assert_eq!(starts(&p), vec![4 * S]);
    }

    #[test]
    fn rippled_tail_trims_move_the_rest_of_the_lane_both_ways() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        for _ in 0..2 {
            let c = append(&p, &m).unwrap();
            h.apply(&mut p, c).unwrap();
        }
        let first = p.tracks[0].segments[0].id.clone();
        let c = trim_edge(&p, &first, Edge::Tail, 3 * S, true).unwrap();
        apply(&mut p, &mut h, c);
        assert_eq!(starts(&p), vec![0, 3 * S]);

        // Past the end of the material: clamped to the material's 4 s.
        let c = trim_edge(&p, &first, Edge::Tail, 9 * S, true).unwrap();
        apply(&mut p, &mut h, c);
        assert_eq!(starts(&p), vec![0, 4 * S]);
        assert_eq!(p.tracks[0].segments[0].target_range.duration, 4 * S);
    }

    #[test]
    fn a_head_trim_reads_later_into_the_source() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        let c = append(&p, &m).unwrap();
        h.apply(&mut p, c).unwrap();
        let id = p.tracks[0].segments[0].id.clone();
        let c = trim_edge(&p, &id, Edge::Head, S, false).unwrap();
        apply(&mut p, &mut h, c);
        let s = &p.tracks[0].segments[0];
        assert_eq!(s.target_range, TimeRange::new(S, 3 * S));
        assert_eq!(s.source_range, TimeRange::new(S, 3 * S));
        assert!(trim_edge(&p, &id, Edge::Head, S, false).unwrap().is_empty());
    }

    #[test]
    fn move_refuses_a_collision() {
        let (mut p, m) = project();
        let mut h = DocumentHistory::new();
        for _ in 0..2 {
            let c = append(&p, &m).unwrap();
            h.apply(&mut p, c).unwrap();
        }
        let second = p.tracks[0].segments[1].id.clone();
        assert!(move_to(&p, &second, None, 2 * S).is_err());
        let c = move_to(&p, &second, None, 9 * S).unwrap();
        h.apply(&mut p, c).unwrap();
        assert_eq!(starts(&p), vec![0, 9 * S]);
    }
}
