//! Edits of a whole selection: move, trim and delete several clips as one
//! gesture, and so as one undo step.
//!
//! Every function here returns the primitive commands of the gesture in an
//! order that no intermediate state rejects. The caller sends them through
//! `timeline_apply_many`, which folds them into one `Composite`. For a batch of
//! moves `compose_edits` sorts the parts itself; a mixed batch (trims and
//! moves, removals and moves) is kept in the order given, so this module puts
//! it in a safe order itself.
//!
//! The central tool is [`arrange`]: "these clips end up here". It parks every
//! moving clip past the end of the timeline first and then puts each one in
//! its place, so clips can swap places, jump over each other and change lanes
//! without ever passing through a neighbour. It also moves the link partners
//! of every moving clip by the same distance, which is what `compose_edits`
//! cannot do for a clip that moves twice (park, then place): it mirrors only
//! the first part and drops the second.

use std::collections::BTreeSet;

use chukcut_engine::modules::project::{
    source_duration_for, Micros, Project, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::timeline::ops::EditCommand;

/// Where a clip should end up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    pub segment_id: String,
    pub track_id: String,
    pub start: Micros,
}

impl Place {
    pub(crate) fn new(segment_id: &str, track_id: &str, start: Micros) -> Self {
        Self {
            segment_id: segment_id.to_string(),
            track_id: track_id.to_string(),
            start,
        }
    }
}

struct Moving {
    segment_id: String,
    from_track: String,
    from_start: Micros,
    to_track: String,
    to_start: Micros,
    /// The room the clip needs while it is parked: the larger of its length
    /// now and its length after the trims of the same batch.
    room: Micros,
}

/// The moves that put every clip of `places` where it says, with the link
/// partners of each moving clip carried by the same distance on their own
/// lanes. Clips in `fixed` are never carried along (they are deleted by the
/// same batch, or moved by it explicitly).
///
/// `lengths` gives a clip's length after the trims of the same batch, when
/// that differs from its length now, so a parking slot is never too short.
///
/// A single clip without partners moves with one plain `MoveSegment`, the
/// shape `History::apply` mirrors itself.
pub(crate) fn arrange(
    project: &Project,
    places: &[Place],
    fixed: &BTreeSet<String>,
    lengths: &[(String, Micros)],
) -> Result<Vec<EditCommand>, String> {
    let length_of = |id: &str, now: Micros| {
        lengths
            .iter()
            .find(|(other, _)| other == id)
            .map(|(_, length)| *length)
            .unwrap_or(now)
    };
    let mut named: BTreeSet<String> = fixed.clone();
    named.extend(places.iter().map(|p| p.segment_id.clone()));

    let mut moving: Vec<Moving> = Vec::new();
    for place in places {
        let (track, segment) = project
            .segment(&place.segment_id)
            .ok_or_else(|| format!("unknown segment {}", place.segment_id))?;
        let target = project
            .track(&place.track_id)
            .ok_or_else(|| format!("unknown track {}", place.track_id))?;
        if target.kind != track.kind {
            return Err("a clip can only move to a lane of its own kind".into());
        }
        if track.id == place.track_id && segment.target_range.start == place.start {
            continue;
        }
        moving.push(Moving {
            segment_id: segment.id.clone(),
            from_track: track.id.clone(),
            from_start: segment.target_range.start,
            to_track: place.track_id.clone(),
            to_start: place.start.max(0),
            room: segment
                .target_range
                .duration
                .max(length_of(&segment.id, segment.target_range.duration)),
        });
    }

    // Partners travel by the same distance and stay on their own lane, the
    // rule `mirror_move` applies to a single move.
    let mut partners = Vec::new();
    for m in &moving {
        let delta = m.to_start - m.from_start;
        let Some(group) = project.link_group_of(&m.segment_id) else {
            continue;
        };
        for (track, _, partner) in project.link_members(group) {
            if delta == 0 || !named.insert(partner.id.clone()) {
                continue;
            }
            partners.push(Moving {
                segment_id: partner.id.clone(),
                from_track: track.id.clone(),
                from_start: partner.target_range.start,
                to_track: track.id.clone(),
                to_start: partner.target_range.start + delta,
                room: partner.target_range.duration,
            });
        }
    }
    moving.extend(partners);

    if moving.iter().any(|m| m.to_start < 0) {
        return Err("a clip cannot start before the beginning of the timeline".into());
    }
    match moving.len() {
        0 => return Ok(Vec::new()),
        1 => {
            let m = &moving[0];
            return Ok(vec![EditCommand::MoveSegment {
                segment_id: m.segment_id.clone(),
                from_track: m.from_track.clone(),
                to_track: m.to_track.clone(),
                from_start: m.from_start,
                to_start: m.to_start,
            }]);
        }
        _ => {}
    }

    let mut slot = moving
        .iter()
        .map(|m| m.to_start + m.room)
        .chain(
            project
                .tracks
                .iter()
                .flat_map(|t| t.segments.iter())
                .map(|s| s.target_range.end()),
        )
        .max()
        .unwrap_or(0)
        + 1_000_000;

    let mut parks = Vec::with_capacity(moving.len());
    let mut finals = Vec::with_capacity(moving.len());
    for m in &moving {
        parks.push(EditCommand::MoveSegment {
            segment_id: m.segment_id.clone(),
            from_track: m.from_track.clone(),
            to_track: m.to_track.clone(),
            from_start: m.from_start,
            to_start: slot,
        });
        finals.push((
            m.to_start,
            EditCommand::MoveSegment {
                segment_id: m.segment_id.clone(),
                from_track: m.to_track.clone(),
                to_track: m.to_track.clone(),
                from_start: slot,
                to_start: m.to_start,
            },
        ));
        slot += m.room;
    }
    finals.sort_by_key(|(start, _)| *start);
    parks.extend(finals.into_iter().map(|(_, command)| command));
    Ok(parks)
}

/// A gapless lane laid out from zero in the given order.
pub(crate) fn packed(track_id: &str, order: &[(String, Micros)]) -> Vec<Place> {
    let mut at = 0;
    order
        .iter()
        .map(|(id, length)| {
            let place = Place::new(id, track_id, at);
            at += length;
            place
        })
        .collect()
}

/// Move the clips `ids` by `delta` in time; with `to_lane`, every one of them
/// onto that lane (the caller only offers it when they share one). With
/// `magnet` naming the main lane, that lane is packed again afterwards in
/// the order the clips now stand in, so it stays gapless.
pub(crate) fn group_move(
    project: &Project,
    ids: &[String],
    delta: Micros,
    to_lane: Option<&str>,
    magnet: Option<&str>,
) -> Result<Vec<EditCommand>, String> {
    let places = group_places(project, ids, delta, to_lane, magnet)?;
    arrange(project, &places, &BTreeSet::new(), &[])
}

/// Where [`group_move`] puts every clip it moves itself — what the timeline
/// draws while the selection is dragged. Link partners it leaves to
/// `arrange` are not in it; [`with_partners`] adds them.
pub(crate) fn group_places(
    project: &Project,
    ids: &[String],
    delta: Micros,
    to_lane: Option<&str>,
    magnet: Option<&str>,
) -> Result<Vec<Place>, String> {
    let earliest = ids
        .iter()
        .filter_map(|id| project.segment(id))
        .map(|(_, s)| s.target_range.start)
        .min()
        .ok_or("nothing is selected")?;
    let delta = delta.max(-earliest);
    let mut places: Vec<Place> = Vec::new();
    for id in ids {
        let (track, segment) = project
            .segment(id)
            .ok_or_else(|| format!("unknown segment {id}"))?;
        let lane = to_lane.unwrap_or(&track.id);
        places.push(Place::new(id, lane, segment.target_range.start + delta));
    }
    if let Some(main) = magnet {
        // The main lane is packed rather than shifted, so its clips do not
        // move by `delta`. A selected clip linked to one of them follows that
        // clip instead — `arrange` carries partners it is not told about — or
        // a picture and its sound would drift apart.
        let main_groups: Vec<&String> = places
            .iter()
            .filter(|p| p.track_id == main)
            .filter_map(|p| project.link_group_of(&p.segment_id))
            .collect();
        places.retain(|p| {
            p.track_id == main
                || !project
                    .link_group_of(&p.segment_id)
                    .is_some_and(|g| main_groups.contains(&g))
        });
        places = repack_main(project, main, places, &[], &[]);
    }
    Ok(places)
}

/// `places` and the link partners `arrange` would carry along with them,
/// each by its clip's distance, on its own lane.
pub(crate) fn with_partners(project: &Project, places: &[Place]) -> Vec<Place> {
    let mut out = places.to_vec();
    for place in places {
        let Some((_, segment)) = project.segment(&place.segment_id) else {
            continue;
        };
        let delta = place.start - segment.target_range.start;
        let Some(group) = project.link_group_of(&place.segment_id) else {
            continue;
        };
        for (track, _, partner) in project.link_members(group) {
            if delta != 0 && !out.iter().any(|p| p.segment_id == partner.id) {
                out.push(Place::new(
                    &partner.id,
                    &track.id,
                    partner.target_range.start + delta,
                ));
            }
        }
    }
    out
}

/// Put the main lane back together after a batch: every clip that ends up on
/// it (the ones already there and not leaving, and the ones `places` brings
/// to it), ordered by where they now stand, packed from zero. `gone` are
/// clips the batch deletes; `lengths` lengths the batch's trims change.
fn repack_main(
    project: &Project,
    main: &str,
    places: Vec<Place>,
    gone: &[String],
    lengths: &[(String, Micros)],
) -> Vec<Place> {
    let Some(track) = project.track(main) else {
        return places;
    };
    let length = |id: &str, now: Micros| {
        lengths
            .iter()
            .find(|(other, _)| other == id)
            .map(|(_, l)| *l)
            .unwrap_or(now)
    };
    // (start, rank, id, length): at the same start a clip that moved there
    // goes before the one that was there, which is where it was dropped.
    let mut lane: Vec<(Micros, u8, String, Micros)> = Vec::new();
    for segment in &track.segments {
        if gone.contains(&segment.id) || places.iter().any(|p| p.segment_id == segment.id) {
            continue;
        }
        lane.push((
            segment.target_range.start,
            1,
            segment.id.clone(),
            length(&segment.id, segment.target_range.duration),
        ));
    }
    let mut rest = Vec::new();
    for place in places {
        if place.track_id == main {
            let now = project
                .segment(&place.segment_id)
                .map(|(_, s)| s.target_range.duration)
                .unwrap_or(0);
            lane.push((
                place.start,
                0,
                place.segment_id.clone(),
                length(&place.segment_id, now),
            ));
        } else {
            rest.push(place);
        }
    }
    lane.sort();
    let order: Vec<(String, Micros)> = lane.into_iter().map(|(_, _, id, l)| (id, l)).collect();
    rest.extend(packed(main, &order));
    rest
}

/// Delete the clips `ids`. Overlay lanes they leave empty go too, and with
/// `magnet` the main lane closes up. One batch, in the order: removals,
/// lane removals (last lane first), then the moves that close the gaps.
pub(crate) fn group_delete(
    project: &Project,
    ids: &[String],
    magnet: Option<&str>,
) -> Result<Vec<EditCommand>, String> {
    let mut commands = Vec::new();
    for id in ids {
        commands.push(crate::edits::remove(project, id)?);
    }
    commands.extend(emptied_lanes(project, ids));
    if let Some(main) = magnet {
        let places = repack_main(project, main, Vec::new(), ids, &[]);
        let gone: BTreeSet<String> = ids.iter().cloned().collect();
        commands.extend(arrange(project, &places, &gone, &[])?);
    }
    Ok(commands)
}

/// The lane removals for every lane that deleting `ids` leaves empty, last
/// lane first so each index is still right when it applies. The main lane
/// and the last lane of each kind stay, as CapCut keeps them.
pub(crate) fn emptied_lanes(project: &Project, ids: &[String]) -> Vec<EditCommand> {
    let main = project
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Video);
    let mut left: Vec<(TrackKind, usize)> = Vec::new();
    for kind in [
        TrackKind::Video,
        TrackKind::Audio,
        TrackKind::Text,
        TrackKind::Sticker,
        TrackKind::Effect,
    ] {
        left.push((
            kind,
            project.tracks.iter().filter(|t| t.kind == kind).count(),
        ));
    }
    let mut out = Vec::new();
    for (index, track) in project.tracks.iter().enumerate().rev() {
        let emptied = !track.segments.is_empty()
            && track.segments.iter().all(|s| ids.contains(&s.id))
            && Some(index) != main;
        let Some(count) = left.iter_mut().find(|(k, _)| *k == track.kind) else {
            continue;
        };
        if !emptied || count.1 < 2 {
            continue;
        }
        count.1 -= 1;
        let mut empty = track.clone();
        empty.segments.clear();
        out.push(EditCommand::RemoveTrack {
            track: empty,
            index,
        });
    }
    out
}

/// One clip's new ranges in a selection trim.
#[derive(Debug, Clone)]
pub(crate) struct Trimmed {
    pub segment_id: String,
    pub target: TimeRange,
    pub source: TimeRange,
}

/// Trim several clips at once. With `magnet`, the main lane is packed again
/// with the new lengths. The order: trims that shorten, the moves, then the
/// trims that lengthen, so a clip grows only into room already made.
pub(crate) fn group_trim(
    project: &Project,
    trims: &[Trimmed],
    magnet: Option<&str>,
) -> Result<Vec<EditCommand>, String> {
    let mut shrink = Vec::new();
    let mut grow = Vec::new();
    let mut lengths = Vec::new();
    // Partners are trimmed here rather than left to `compose_edits`: a
    // partner the same batch also moves counts as already handled there, and
    // its trim would be dropped.
    let mut all: Vec<Trimmed> = trims.to_vec();
    let mut named: BTreeSet<String> = trims.iter().map(|t| t.segment_id.clone()).collect();
    for trim in trims {
        all.extend(partner_trims(project, trim, &mut named));
    }
    for trim in &all {
        let (_, segment) = project
            .segment(&trim.segment_id)
            .ok_or_else(|| format!("unknown segment {}", trim.segment_id))?;
        if trim.target == segment.target_range && trim.source == segment.source_range {
            continue;
        }
        let command = EditCommand::TrimSegment {
            segment_id: segment.id.clone(),
            before_target: segment.target_range,
            before_source: segment.source_range,
            after_target: trim.target,
            after_source: trim.source,
        };
        lengths.push((segment.id.clone(), trim.target.duration));
        if trim.target.duration > segment.target_range.duration {
            grow.push(command);
        } else {
            shrink.push(command);
        }
    }
    let mut commands = shrink;
    if let Some(main) = magnet {
        let places = repack_main(project, main, Vec::new(), &[], &lengths);
        // Nothing is held back from `arrange`: a clip whose picture the
        // packing moves goes with it, trimmed or not.
        commands.extend(arrange(project, &places, &BTreeSet::new(), &lengths)?);
    }
    commands.extend(grow);
    Ok(commands)
}

/// The trims that carry `trim` over to the clips linked to its clip: the same
/// movement at each edge, each partner's source at its own speed — the rule
/// `ops::mirror_trim` applies to a single trim.
fn partner_trims(project: &Project, trim: &Trimmed, named: &mut BTreeSet<String>) -> Vec<Trimmed> {
    let Some((_, segment)) = project.segment(&trim.segment_id) else {
        return Vec::new();
    };
    let head = trim.target.start - segment.target_range.start;
    let tail = trim.target.end() - segment.target_range.end();
    let Some(group) = project.link_group_of(&segment.id) else {
        return Vec::new();
    };
    if head == 0 && tail == 0 {
        return Vec::new();
    }
    project
        .link_members(group)
        .into_iter()
        .filter(|(_, _, partner)| named.insert(partner.id.clone()))
        .map(|(_, _, partner)| {
            let target = TimeRange::new(
                partner.target_range.start + head,
                partner.target_range.duration + tail - head,
            );
            Trimmed {
                segment_id: partner.id.clone(),
                target,
                source: TimeRange::new(
                    partner.source_range.start + source_duration_for(head, partner.speed),
                    source_duration_for(target.duration, partner.speed),
                ),
            }
        })
        .collect()
}

/// Every clip on unlocked lanes, in lane order: what Ctrl+A selects.
pub(crate) fn all_clips(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .filter(|t: &&Track| !t.locked)
        .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chukcut_engine::modules::project::{new_id, CanvasConfig, Segment, Transform};
    use chukcut_engine::modules::timeline::ops::{compose_edits, link};
    use chukcut_engine::modules::timeline::History;

    pub(crate) fn clip(start: Micros, duration: Micros) -> Segment {
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

    /// A main lane with clips of the given lengths back to back, an empty
    /// overlay lane and an empty audio lane.
    pub(crate) fn project(lengths: &[Micros]) -> (Project, Vec<String>) {
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
        project.tracks.push(Track::new(TrackKind::Audio, "A1"));
        (project, ids)
    }

    pub(crate) fn apply(project: &mut Project, commands: Vec<EditCommand>) -> Result<(), String> {
        if commands.is_empty() {
            return Ok(());
        }
        let command = compose_edits(project, "test", commands)?;
        History::default().apply(project, command)
    }

    fn starts(project: &Project, track: usize) -> Vec<(String, Micros)> {
        project.tracks[track]
            .segments
            .iter()
            .map(|s| (s.id.clone(), s.target_range.start))
            .collect()
    }

    /// Put a copy of the clip `id` on the audio lane, linked to it.
    fn with_sound(project: &mut Project, id: &str) -> String {
        let (_, picture) = project.segment(id).unwrap();
        let mut sound = picture.clone();
        sound.id = new_id();
        let sound_id = sound.id.clone();
        project.tracks[2].segments.push(sound);
        project.tracks[2]
            .segments
            .sort_by_key(|s| s.target_range.start);
        let command = link(project, &[id.to_string(), sound_id.clone()]).unwrap();
        History::default().apply(project, command).unwrap();
        sound_id
    }

    #[test]
    fn clips_swap_places_through_the_parking_lot() {
        let (mut project, ids) = project(&[1_000_000, 1_000_000]);
        let main = project.tracks[0].id.clone();
        let places = vec![
            Place::new(&ids[0], &main, 1_000_000),
            Place::new(&ids[1], &main, 0),
        ];
        let commands = arrange(&project, &places, &BTreeSet::new(), &[]).unwrap();
        apply(&mut project, commands).expect("the swap applies");
        assert_eq!(
            starts(&project, 0),
            vec![(ids[1].clone(), 0), (ids[0].clone(), 1_000_000)]
        );
    }

    #[test]
    fn a_single_unlinked_move_is_one_plain_move() {
        let (project, ids) = project(&[1_000_000]);
        let overlay = project.tracks[1].id.clone();
        let commands = arrange(
            &project,
            &[Place::new(&ids[0], &overlay, 500_000)],
            &BTreeSet::new(),
            &[],
        )
        .unwrap();
        assert_eq!(commands.len(), 1);
    }

    #[test]
    fn a_linked_partner_follows_a_reorder_of_the_main_lane() {
        let (mut project, ids) = project(&[1_000_000, 2_000_000]);
        let sound = with_sound(&mut project, &ids[0]);
        let main = project.tracks[0].id.clone();
        // The first clip goes after the second: it ends up at two seconds,
        // and its sound with it.
        let commands = group_move(&project, &[ids[0].clone()], 2_500_000, None, Some(&main))
            .expect("commands");
        apply(&mut project, commands).expect("applies");
        assert_eq!(
            starts(&project, 0),
            vec![(ids[1].clone(), 0), (ids[0].clone(), 2_000_000)]
        );
        assert_eq!(
            project.segment(&sound).unwrap().1.target_range.start,
            2_000_000
        );
    }

    #[test]
    fn a_selected_sound_stays_with_its_packed_picture() {
        let (mut project, ids) = project(&[1_000_000, 1_000_000]);
        let sound = with_sound(&mut project, &ids[0]);
        let main = project.tracks[0].id.clone();
        // Everything selected and dragged right: the main lane packs back to
        // zero, and the sound keeps lining up with its picture.
        let all = vec![ids[0].clone(), ids[1].clone(), sound.clone()];
        let commands = group_move(&project, &all, 1_000_000, None, Some(&main)).unwrap();
        apply(&mut project, commands).unwrap();
        let picture = project.segment(&ids[0]).unwrap().1.target_range.start;
        assert_eq!(
            project.segment(&sound).unwrap().1.target_range.start,
            picture
        );
    }

    #[test]
    fn the_ghost_of_a_drag_carries_the_partners() {
        let (mut project, ids) = project(&[1_000_000, 1_000_000]);
        let sound = with_sound(&mut project, &ids[0]);
        let main = project.tracks[0].id.clone();
        let places = vec![Place::new(&ids[0], &main, 1_500_000)];
        let ghosts = with_partners(&project, &places);
        let audio = project.tracks[2].id.clone();
        assert!(ghosts.contains(&Place::new(&sound, &audio, 1_500_000)));
        assert_eq!(ghosts.len(), 2);
    }

    #[test]
    fn a_selection_moves_together_and_undoes_in_one_step() {
        let (mut project, _) = project(&[1_000_000]);
        let a = clip(0, 1_000_000);
        let b = clip(3_000_000, 1_000_000);
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        project.tracks[1].segments.extend([a, b]);
        let before = starts(&project, 1);
        let commands =
            group_move(&project, &[a_id.clone(), b_id.clone()], 500_000, None, None).unwrap();
        let command = compose_edits(&project, "Move clips", commands).unwrap();
        let mut history = History::default();
        history.apply(&mut project, command).unwrap();
        assert_eq!(
            starts(&project, 1),
            vec![(a_id, 500_000), (b_id, 3_500_000)]
        );
        history.undo(&mut project).unwrap();
        assert_eq!(starts(&project, 1), before);
    }

    #[test]
    fn a_group_move_stops_at_time_zero() {
        let (mut project, _) = project(&[1_000_000]);
        let a = clip(1_000_000, 1_000_000);
        let b = clip(3_000_000, 1_000_000);
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        project.tracks[1].segments.extend([a, b]);
        let commands = group_move(
            &project,
            &[a_id.clone(), b_id.clone()],
            -5_000_000,
            None,
            None,
        )
        .unwrap();
        apply(&mut project, commands).unwrap();
        assert_eq!(starts(&project, 1), vec![(a_id, 0), (b_id, 2_000_000)]);
    }

    #[test]
    fn deleting_from_the_main_lane_closes_every_gap() {
        let (mut project, ids) = project(&[1_000_000, 2_000_000, 3_000_000, 4_000_000]);
        let main = project.tracks[0].id.clone();
        let commands =
            group_delete(&project, &[ids[0].clone(), ids[2].clone()], Some(&main)).unwrap();
        apply(&mut project, commands).expect("applies");
        assert_eq!(
            starts(&project, 0),
            vec![(ids[1].clone(), 0), (ids[3].clone(), 2_000_000)]
        );
    }

    #[test]
    fn deleting_takes_the_linked_sound_and_the_emptied_overlay_lane() {
        let (mut project, ids) = project(&[1_000_000, 1_000_000]);
        let sound = with_sound(&mut project, &ids[1]);
        let lone = clip(0, 500_000);
        let lone_id = lone.id.clone();
        project.tracks[1].segments.push(lone);
        let main = project.tracks[0].id.clone();
        let commands = group_delete(&project, &[ids[1].clone(), lone_id], Some(&main)).unwrap();
        apply(&mut project, commands).expect("applies");
        assert!(project.segment(&sound).is_none());
        assert_eq!(project.tracks.len(), 2, "the overlay lane went");
    }

    #[test]
    fn trimming_a_selection_on_the_main_lane_keeps_it_gapless() {
        let (mut project, ids) = project(&[2_000_000, 2_000_000, 2_000_000]);
        let main = project.tracks[0].id.clone();
        let trims: Vec<Trimmed> = [0, 1]
            .iter()
            .map(|&i| {
                let (_, s) = project.segment(&ids[i]).unwrap();
                Trimmed {
                    segment_id: s.id.clone(),
                    target: TimeRange::new(s.target_range.start, 1_000_000),
                    source: TimeRange::new(0, 1_000_000),
                }
            })
            .collect();
        let commands = group_trim(&project, &trims, Some(&main)).unwrap();
        apply(&mut project, commands).expect("applies");
        assert_eq!(
            starts(&project, 0),
            vec![
                (ids[0].clone(), 0),
                (ids[1].clone(), 1_000_000),
                (ids[2].clone(), 2_000_000)
            ]
        );
    }

    #[test]
    fn trimming_linked_pictures_keeps_their_sounds_in_line() {
        let (mut project, ids) = project(&[2_000_000, 2_000_000]);
        let first = with_sound(&mut project, &ids[0]);
        let second = with_sound(&mut project, &ids[1]);
        let main = project.tracks[0].id.clone();
        let trims: Vec<Trimmed> = ids
            .iter()
            .map(|id| {
                let (_, s) = project.segment(id).unwrap();
                Trimmed {
                    segment_id: s.id.clone(),
                    target: TimeRange::new(s.target_range.start, 1_000_000),
                    source: TimeRange::new(0, 1_000_000),
                }
            })
            .collect();
        let commands = group_trim(&project, &trims, Some(&main)).unwrap();
        apply(&mut project, commands).expect("applies");
        for (picture, sound) in [(&ids[0], &first), (&ids[1], &second)] {
            assert_eq!(
                project.segment(picture).unwrap().1.target_range,
                project.segment(sound).unwrap().1.target_range
            );
        }
        assert_eq!(
            project.segment(&ids[1]).unwrap().1.target_range.start,
            1_000_000
        );
    }

    #[test]
    fn the_last_lane_of_a_kind_stays() {
        let (project, ids) = project(&[1_000_000]);
        assert!(emptied_lanes(&project, &ids).is_empty());
    }
}
