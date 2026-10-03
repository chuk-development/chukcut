//! Edit commands for the timeline gestures that touch more than one clip.
//!
//! CapCut keeps its main track (the first video lane) gapless while the
//! "main track magnet" is on: deleting or shortening a clip there pulls
//! everything after it to the left, and dragging a clip along it reorders
//! the lane instead of leaving a hole. Each function here reads the document
//! and returns the primitive commands that do that; the caller applies them
//! as one undo step through `timeline_apply_many`, which adds link partners
//! and puts plain move batches into an order no intermediate state rejects.

use chukcut_engine::modules::project::{
    source_duration_for, Micros, Project, Segment, TimeRange, Track,
};
use chukcut_engine::modules::timeline::ops::EditCommand;

/// The shortest clip a trim may leave: one frame at 30 fps, rounded up.
pub(crate) const MIN_CLIP: Micros = 34_000;

/// How far into its material a segment may read, if the material has an end.
/// Stills and generated materials can be stretched without limit.
pub(crate) fn source_limit(project: &Project, material_id: &str) -> Option<Micros> {
    let pool = &project.materials;
    if let Some(video) = pool.videos.iter().find(|m| m.id == material_id) {
        return Some(video.duration);
    }
    pool.audios
        .iter()
        .find(|m| m.id == material_id)
        .map(|audio| audio.duration)
}

/// Which edge of a clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Head,
    Tail,
}

/// The ranges a segment has after its `edge` is moved to the timeline time
/// `to`, clamped so the clip keeps at least [`MIN_CLIP`], reads nothing before
/// the start of its material and nothing past `limit`.
///
/// With `anchored`, a head trim keeps the clip's start where it is and only
/// shortens it — what a rippled head trim on the main track looks like.
pub(crate) fn trimmed(
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

/// A trim, and with `ripple` every later clip on the lane shifted by however
/// much the clip's end moved, so the lane keeps its spacing.
pub(crate) fn trim(
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

/// Take a clip off the timeline; with `ripple`, pull everything after it on
/// the lane left into the hole it leaves.
pub(crate) fn remove(
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
    let mut commands = vec![crate::edits::remove(project, segment_id)?];
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

/// Where in a gapless lane a clip dropped with its centre at `centre` lands:
/// the number of the lane's other clips whose centre is before it.
pub(crate) fn insertion_index(track: &Track, moving: &str, centre: Micros) -> usize {
    track
        .segments
        .iter()
        .filter(|s| s.id != moving)
        .filter(|s| s.target_range.start + s.target_range.duration / 2 < centre)
        .count()
}

/// Drop `segment_id` into the gapless `track` at `index` among the lane's
/// other clips, and pack the lane from zero.
///
/// `batch::arrange` does the moving: it parks the clips first so none passes
/// through another, and carries every moved clip's link partners by the same
/// distance — which a plain batch cannot do for a clip that moves twice.
pub(crate) fn drop_into_gapless(
    project: &Project,
    track_id: &str,
    segment_id: &str,
    index: usize,
) -> Result<Vec<EditCommand>, String> {
    let track = project
        .track(track_id)
        .ok_or_else(|| format!("unknown track {track_id}"))?;
    let (from, moving) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    if from.kind != track.kind {
        return Err("a clip can only move to a lane of its own kind".into());
    }
    let mut order: Vec<(String, Micros)> = track
        .segments
        .iter()
        .filter(|s| s.id != segment_id)
        .map(|s| (s.id.clone(), s.target_range.duration))
        .collect();
    order.insert(
        index.min(order.len()),
        (segment_id.to_string(), moving.target_range.duration),
    );
    let places = super::batch::packed(track_id, &order);
    super::batch::arrange(project, &places, &Default::default(), &[])
}

/// Move a clip out of a gapless lane to `to_start` on another lane, and close
/// the hole it leaves behind.
pub(crate) fn lift_out_of_gapless(
    project: &Project,
    segment_id: &str,
    to_track: &str,
    to_start: Micros,
) -> Result<Vec<EditCommand>, String> {
    let (from, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let mut commands = vec![EditCommand::MoveSegment {
        segment_id: segment_id.to_string(),
        from_track: from.id.clone(),
        to_track: to_track.to_string(),
        from_start: segment.target_range.start,
        to_start: to_start.max(0),
    }];
    let index = from
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("the segment is on its track");
    if let Some(next) = from.segments.get(index + 1) {
        let shift = segment.target_range.start - next.target_range.start;
        commands.extend(shifted(
            from,
            segment.target_range.start,
            Some(segment_id),
            shift,
        ));
    }
    Ok(commands)
}

/// The command that removes the lane `segment_id` sits on, when taking that
/// clip away leaves it empty — as CapCut tidies up overlay lanes. The main
/// lane and the last lane of a kind stay. `inserted_at` is the index of a lane
/// the same batch adds first, which moves everything from there down by one.
pub(crate) fn drop_emptied_lane(
    project: &Project,
    segment_id: &str,
    inserted_at: Option<usize>,
) -> Option<EditCommand> {
    let index = project
        .tracks
        .iter()
        .position(|t| t.segments.iter().any(|s| s.id == segment_id))?;
    let track = &project.tracks[index];
    let main = project
        .tracks
        .iter()
        .position(|t| t.kind == chukcut_engine::modules::project::TrackKind::Video);
    let of_kind = project
        .tracks
        .iter()
        .filter(|t| t.kind == track.kind)
        .count();
    if track.segments.len() != 1 || Some(index) == main || of_kind < 2 {
        return None;
    }
    let mut emptied = track.clone();
    emptied.segments.clear();
    let index = match inserted_at {
        Some(at) if at <= index => index + 1,
        _ => index,
    };
    Some(EditCommand::RemoveTrack {
        track: emptied,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::project::{new_id, CanvasConfig, TrackKind, Transform};
    use chukcut_engine::modules::timeline::ops::compose_edits;
    use chukcut_engine::modules::timeline::History;

    fn clip(start: Micros, duration: Micros) -> Segment {
        Segment {
            id: new_id(),
            material_id: "m".into(),
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

    /// A main lane holding clips of the given lengths back to back, and an
    /// empty second video lane.
    fn lane(lengths: &[Micros]) -> (Project, Vec<String>) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut main = Track::new(TrackKind::Video, "V1");
        let mut at = 0;
        let mut ids = Vec::new();
        for length in lengths {
            let segment = clip(at, *length);
            ids.push(segment.id.clone());
            main.segments.push(segment);
            at += length;
        }
        project.tracks.push(main);
        project.tracks.push(Track::new(TrackKind::Video, "V2"));
        (project, ids)
    }

    fn apply(project: &mut Project, commands: Vec<EditCommand>) -> Result<(), String> {
        let command = compose_edits(project, "test", commands)?;
        History::default().apply(project, command)
    }

    fn starts(project: &Project) -> Vec<(String, Micros)> {
        project.tracks[0]
            .segments
            .iter()
            .map(|s| (s.id.clone(), s.target_range.start))
            .collect()
    }

    #[test]
    fn reordering_a_gapless_lane_swaps_without_overlap() {
        let (mut project, ids) = lane(&[5_000_000, 3_000_000, 2_000_000]);
        // The first clip dropped after the second.
        let commands = drop_into_gapless(&project, &project.tracks[0].id.clone(), &ids[0], 1)
            .expect("commands");
        apply(&mut project, commands).expect("the reorder applies");
        assert_eq!(
            starts(&project),
            vec![
                (ids[1].clone(), 0),
                (ids[0].clone(), 3_000_000),
                (ids[2].clone(), 8_000_000),
            ]
        );
    }

    #[test]
    fn dropping_at_its_own_place_changes_nothing() {
        let (project, ids) = lane(&[1_000_000, 1_000_000]);
        let commands =
            drop_into_gapless(&project, &project.tracks[0].id, &ids[1], 1).expect("commands");
        assert!(commands.is_empty());
    }

    #[test]
    fn a_clip_from_another_lane_is_inserted_and_the_lane_opens() {
        let (mut project, ids) = lane(&[2_000_000, 2_000_000]);
        let incoming = clip(10_000_000, 1_000_000);
        let incoming_id = incoming.id.clone();
        project.tracks[1].segments.push(incoming);
        let main = project.tracks[0].id.clone();
        let commands = drop_into_gapless(&project, &main, &incoming_id, 1).expect("commands");
        apply(&mut project, commands).expect("the insert applies");
        assert_eq!(
            starts(&project),
            vec![
                (ids[0].clone(), 0),
                (incoming_id, 2_000_000),
                (ids[1].clone(), 3_000_000),
            ]
        );
        assert!(project.tracks[1].segments.is_empty());
    }

    #[test]
    fn an_overlay_lane_left_empty_goes_and_comes_back_on_undo() {
        let (mut project, _) = lane(&[1_000_000]);
        let lone = clip(0, 1_000_000);
        let lone_id = lone.id.clone();
        project.tracks[1].segments.push(lone);
        let main = project.tracks[0].id.clone();

        let mut commands = drop_into_gapless(&project, &main, &lone_id, 0).expect("commands");
        commands.extend(drop_emptied_lane(&project, &lone_id, None));
        let command = compose_edits(&project, "test", commands).expect("composed");
        let mut history = History::default();
        history.apply(&mut project, command).expect("applies");
        assert_eq!(project.tracks.len(), 1);

        history.undo(&mut project).expect("undoes");
        assert_eq!(project.tracks.len(), 2);
        assert_eq!(project.tracks[1].segments[0].id, lone_id);
    }

    #[test]
    fn the_main_lane_and_the_last_of_a_kind_stay() {
        let (project, ids) = lane(&[1_000_000]);
        assert!(drop_emptied_lane(&project, &ids[0], None).is_none());
    }

    #[test]
    fn ripple_delete_closes_the_hole() {
        let (mut project, ids) = lane(&[1_000_000, 2_000_000, 3_000_000]);
        let commands = remove(&project, &ids[0], true).expect("commands");
        apply(&mut project, commands).expect("the delete applies");
        assert_eq!(
            starts(&project),
            vec![(ids[1].clone(), 0), (ids[2].clone(), 2_000_000)]
        );
    }

    #[test]
    fn lifting_a_clip_out_closes_the_hole() {
        let (mut project, ids) = lane(&[1_000_000, 2_000_000, 3_000_000]);
        let overlay = project.tracks[1].id.clone();
        let commands = lift_out_of_gapless(&project, &ids[1], &overlay, 500_000).expect("commands");
        apply(&mut project, commands).expect("the lift applies");
        assert_eq!(
            starts(&project),
            vec![(ids[0].clone(), 0), (ids[2].clone(), 1_000_000)]
        );
        assert_eq!(project.tracks[1].segments[0].target_range.start, 500_000);
    }

    #[test]
    fn rippled_trims_shift_the_rest_of_the_lane_both_ways() {
        let (mut project, ids) = lane(&[4_000_000, 1_000_000]);
        let segment = project.tracks[0].segments[0].clone();

        // Shorter by a second: the second clip follows it left.
        let (target, source) = trimmed(&segment, Edge::Tail, 3_000_000, Some(10_000_000), false);
        let commands = trim(&project, &ids[0], target, source, true).expect("commands");
        apply(&mut project, commands).expect("the trim applies");
        assert_eq!(starts(&project)[1].1, 3_000_000);

        // Longer again by two: the second clip makes room first.
        let segment = project.tracks[0].segments[0].clone();
        let (target, source) = trimmed(&segment, Edge::Tail, 5_000_000, Some(10_000_000), false);
        let commands = trim(&project, &ids[0], target, source, true).expect("commands");
        apply(&mut project, commands).expect("the trim applies");
        assert_eq!(starts(&project)[1].1, 5_000_000);
    }

    #[test]
    fn an_anchored_head_trim_keeps_the_start_and_reads_later() {
        let (project, _) = lane(&[4_000_000]);
        let segment = &project.tracks[0].segments[0];
        let (target, source) = trimmed(segment, Edge::Head, 1_000_000, None, true);
        assert_eq!(target, TimeRange::new(0, 3_000_000));
        assert_eq!(source, TimeRange::new(1_000_000, 3_000_000));
    }

    #[test]
    fn trims_stop_at_the_material_and_at_one_frame() {
        let (project, _) = lane(&[4_000_000]);
        let segment = &project.tracks[0].segments[0];
        // The material is four seconds long: the tail cannot grow past it.
        let (target, _) = trimmed(segment, Edge::Tail, 9_000_000, Some(4_000_000), false);
        assert_eq!(target.end(), 4_000_000);
        // The head cannot read before the material's start.
        let (target, source) = trimmed(segment, Edge::Head, -2_000_000, None, false);
        assert_eq!(target.start, 0);
        assert_eq!(source.start, 0);
        // Nor cross the tail.
        let (target, _) = trimmed(segment, Edge::Head, 9_000_000, None, false);
        assert_eq!(target.duration, MIN_CLIP);
    }

    #[test]
    fn trims_scale_the_source_with_the_speed() {
        let (project, _) = lane(&[4_000_000]);
        let mut segment = project.tracks[0].segments[0].clone();
        segment.speed = 2.0;
        segment.source_range = TimeRange::new(0, 8_000_000);
        let (target, source) = trimmed(&segment, Edge::Head, 1_000_000, None, false);
        assert_eq!(target, TimeRange::new(1_000_000, 3_000_000));
        assert_eq!(source, TimeRange::new(2_000_000, 6_000_000));
    }
}
