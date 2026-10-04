//! Copy, cut, paste and duplicate.
//!
//! The clipboard lives in the timeline, not in the system clipboard: what it
//! holds is segments of *this* document, with their material ids, and a paste
//! into another project would point at media that is not there.
//!
//! Paste puts the clips at the playhead, each on the lane it was copied from,
//! keeping their distances to each other. A lane that has no room for them
//! gives way to the next lane of the same kind that does, and failing that to
//! a new lane — a paste never overlaps anything and never fails for lack of
//! room. On the main lane with the magnet on, the clips are inserted at the
//! nearest cut instead and everything after them moves right, as CapCut does.

use std::collections::BTreeSet;

use chukcut_engine::modules::project::{
    new_id, Micros, Project, Segment, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::timeline::ops::EditCommand;

use super::batch::{arrange, Place};

/// One copied clip.
#[derive(Debug, Clone)]
pub(crate) struct Copied {
    /// The lane it was copied from, and that lane's kind.
    pub lane: String,
    pub kind: TrackKind,
    /// Its start relative to the earliest copied clip.
    pub offset: Micros,
    pub segment: Segment,
    /// The link group it was in, so a pasted pair is linked again.
    pub group: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Clipboard {
    pub clips: Vec<Copied>,
}

impl Clipboard {
    /// The materials of the copied titles. Each pasted title needs a material
    /// of its own, or editing one title's words would edit the other's.
    pub(crate) fn text_materials(&self, project: &Project) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for clip in &self.clips {
            let id = &clip.segment.material_id;
            if project.materials.texts.iter().any(|m| &m.id == id) && !out.contains(id) {
                out.push(id.clone());
            }
        }
        out
    }
}

/// Copy the clips `ids` and everything linked to them: a clip and its sound
/// are one thing to copy, as they are one thing to move.
pub(crate) fn copy(project: &Project, ids: &[String]) -> Option<Clipboard> {
    let mut wanted: Vec<String> = Vec::new();
    for id in ids {
        if !wanted.contains(id) {
            wanted.push(id.clone());
        }
        if let Some(group) = project.link_group_of(id) {
            for (_, _, partner) in project.link_members(group) {
                if !wanted.contains(&partner.id) {
                    wanted.push(partner.id.clone());
                }
            }
        }
    }
    let mut clips: Vec<Copied> = wanted
        .iter()
        .filter_map(|id| project.segment(id))
        .map(|(track, segment)| Copied {
            lane: track.id.clone(),
            kind: track.kind,
            offset: segment.target_range.start,
            segment: segment.clone(),
            group: project.materials.link_of(segment).cloned(),
        })
        .collect();
    let earliest = clips.iter().map(|c| c.offset).min()?;
    for clip in &mut clips {
        clip.offset -= earliest;
    }
    clips.sort_by_key(|c| c.offset);
    Some(Clipboard { clips })
}

/// What a paste does, and the clips it made (for the selection).
#[derive(Debug, Default)]
pub(crate) struct Pasted {
    pub commands: Vec<EditCommand>,
    pub ids: Vec<String>,
}

/// Where on a gapless lane a paste at `at` goes: the end of the clip `at` is
/// inside, the end of the lane when `at` is past it, otherwise `at`, which
/// is then a cut.
pub(crate) fn insertion_point(track: &Track, at: Micros) -> Micros {
    let end = track
        .segments
        .iter()
        .map(|s| s.target_range.end())
        .max()
        .unwrap_or(0);
    if at >= end {
        return end;
    }
    track
        .segments
        .iter()
        .find(|s| s.target_range.start < at && at < s.target_range.end())
        .map(|s| s.target_range.end())
        .unwrap_or(at)
}

/// The commands that paste `clipboard` at `at`. `material` maps a copied
/// clip's material id to the one the pasted clip should use (fresh title
/// materials); `magnet` names the main lane when the magnet is on.
///
/// Built against a scratch copy of the document, applying each command as it
/// is made, so every lane search sees what the paste has already put there.
pub(crate) fn paste(
    project: &Project,
    clipboard: &Clipboard,
    at: Micros,
    magnet: Option<&str>,
    material: &dyn Fn(&str) -> String,
) -> Result<Pasted, String> {
    if clipboard.clips.is_empty() {
        return Err("the clipboard is empty".into());
    }
    let at = at.max(0);
    let mut scratch = project.clone();
    let mut out = Pasted::default();
    let run = |scratch: &mut Project, command: EditCommand, out: &mut Pasted| {
        command.apply(scratch)?;
        out.commands.push(command);
        Ok::<(), String>(())
    };

    // Copied clips grouped by the lane they came from, main lane first so a
    // pasted picture's place is known before its sound looks for one.
    let main = project
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Video);
    let lane_rank = |lane: &str| -> (u8, usize) {
        match project.tracks.iter().position(|t| t.id == lane) {
            Some(i) if Some(i) == main => (0, i),
            Some(i) => (1, i),
            None => (2, 0),
        }
    };
    let mut lanes: Vec<String> = Vec::new();
    for clip in &clipboard.clips {
        if !lanes.contains(&clip.lane) {
            lanes.push(clip.lane.clone());
        }
    }
    lanes.sort_by_key(|lane| lane_rank(lane));

    // Where each copied clip landed: (copied segment id, pasted start).
    let mut landed: Vec<(String, Micros)> = Vec::new();
    // (copied group, pasted segment id), to link the pasted clips again.
    let mut groups: Vec<(String, String)> = Vec::new();

    for lane in lanes {
        let clips: Vec<&Copied> = clipboard.clips.iter().filter(|c| c.lane == lane).collect();
        let kind = clips[0].kind;
        let main_id = main.map(|i| project.tracks[i].id.clone());
        let onto_main = magnet.is_some()
            && magnet == main_id.as_deref()
            && Some(lane.as_str()) == magnet
            && scratch.track(&lane).is_some_and(|t| !t.locked);

        let starts: Vec<Micros> = if onto_main {
            // Ripple insert: the clips go back to back at the cut, and the
            // rest of the lane moves right by as much.
            let track = scratch.track(&lane).expect("checked above");
            let point = insertion_point(track, at);
            let total: Micros = clips.iter().map(|c| c.segment.target_range.duration).sum();
            let places: Vec<Place> = track
                .segments
                .iter()
                .filter(|s| s.target_range.start >= point)
                .map(|s| Place::new(&s.id, &lane, s.target_range.start + total))
                .collect();
            for command in arrange(&scratch, &places, &BTreeSet::new(), &[])? {
                run(&mut scratch, command, &mut out)?;
            }
            let mut next = point;
            clips
                .iter()
                .map(|c| {
                    let start = next;
                    next += c.segment.target_range.duration;
                    start
                })
                .collect()
        } else {
            clips
                .iter()
                .map(|c| {
                    // A clip linked to one already pasted keeps its distance
                    // to it, wherever that one went.
                    let partner = c.group.as_ref().and_then(|group| {
                        clipboard
                            .clips
                            .iter()
                            .filter(|o| o.group.as_ref() == Some(group))
                            .find_map(|o| {
                                landed
                                    .iter()
                                    .find(|(id, _)| *id == o.segment.id)
                                    .map(|(_, start)| start - o.offset)
                            })
                    });
                    partner.unwrap_or(at) + c.offset
                })
                .collect()
        };

        let target_lane = if onto_main {
            lane.clone()
        } else {
            let fits = |track: &Track| {
                track.kind == kind
                    && !track.locked
                    && clips.iter().zip(&starts).all(|(c, start)| {
                        track.is_range_free(
                            &TimeRange::new(*start, c.segment.target_range.duration),
                            None,
                        )
                    })
            };
            let own = scratch
                .track(&lane)
                .filter(|t| fits(t))
                .map(|t| t.id.clone());
            let other = || {
                scratch
                    .tracks
                    .iter()
                    .filter(|t| t.id != lane && t.id != main_id.clone().unwrap_or_default())
                    .find(|t| fits(t))
                    .map(|t| t.id.clone())
            };
            match own.or_else(other) {
                Some(id) => id,
                None => {
                    let (track, index) = new_lane(&scratch, kind);
                    let id = track.id.clone();
                    run(
                        &mut scratch,
                        EditCommand::AddTrack { track, index },
                        &mut out,
                    )?;
                    id
                }
            }
        };

        for (clip, start) in clips.iter().zip(starts) {
            let mut segment = clip.segment.clone();
            segment.id = new_id();
            segment.material_id = material(&segment.material_id);
            segment.target_range = TimeRange::new(start, segment.target_range.duration);
            // A transition belongs to a cut, and a link to a pair; neither
            // comes along. The pair is linked again below, as a new pair. A
            // template slot is one place to fill: the copy is a plain clip.
            segment.extras.retain(|id| {
                project.materials.transition(id).is_none()
                    && !project.materials.links.contains(id)
                    && !chukcut_engine::modules::template::slot::is_marker(project, id)
            });
            let index = scratch
                .track(&target_lane)
                .map(|t| {
                    t.segments
                        .iter()
                        .filter(|s| s.target_range.start < start)
                        .count()
                })
                .unwrap_or(0);
            landed.push((clip.segment.id.clone(), start));
            if let Some(group) = &clip.group {
                groups.push((group.clone(), segment.id.clone()));
            }
            out.ids.push(segment.id.clone());
            run(
                &mut scratch,
                EditCommand::InsertSegment {
                    track_id: target_lane.clone(),
                    segment,
                    index,
                },
                &mut out,
            )?;
        }
    }

    let mut seen: Vec<&str> = Vec::new();
    for (group, _) in &groups {
        if seen.contains(&group.as_str()) {
            continue;
        }
        seen.push(group);
        let members: Vec<&String> = groups
            .iter()
            .filter(|(g, _)| g == group)
            .map(|(_, id)| id)
            .collect();
        if members.len() < 2 {
            continue;
        }
        let fresh = new_id();
        for id in members {
            run(
                &mut scratch,
                EditCommand::SetLinkGroup {
                    segment_id: id.clone(),
                    before: None,
                    after: Some(fresh.clone()),
                },
                &mut out,
            )?;
        }
    }
    Ok(out)
}

/// A new lane of `kind` and the index it goes in at: video lanes on top of
/// the other video lanes, everything else last.
pub(crate) fn new_lane(project: &Project, kind: TrackKind) -> (Track, usize) {
    let count = project.tracks.iter().filter(|t| t.kind == kind).count();
    let name = match kind {
        TrackKind::Video => format!("Video {}", count + 1),
        TrackKind::Audio => format!("Audio {}", count + 1),
        TrackKind::Text => format!("Text {}", count + 1),
        TrackKind::Sticker => format!("Sticker {}", count + 1),
        TrackKind::Effect => format!("Effect {}", count + 1),
    };
    let index = match kind {
        TrackKind::Video => project
            .tracks
            .iter()
            .rposition(|t| t.kind == TrackKind::Video)
            .map(|i| i + 1)
            .unwrap_or(0),
        _ => project.tracks.len(),
    };
    (Track::new(kind, name), index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::timeline::batch::tests::{apply, clip, project};
    use chukcut_engine::modules::timeline::ops::link;
    use chukcut_engine::modules::timeline::History;

    fn same(id: &str) -> String {
        id.to_string()
    }

    #[test]
    fn a_paste_goes_on_its_own_lane_when_there_is_room() {
        let (mut project, _) = project(&[4_000_000]);
        let overlay = clip(0, 1_000_000);
        let overlay_id = overlay.id.clone();
        project.tracks[1].segments.push(overlay);
        let board = copy(&project, &[overlay_id]).unwrap();
        let pasted = paste(&project, &board, 2_000_000, None, &same).unwrap();
        apply(&mut project, pasted.commands).unwrap();
        let lane = &project.tracks[1];
        assert_eq!(lane.segments.len(), 2);
        assert_eq!(lane.segments[1].target_range.start, 2_000_000);
        assert_eq!(lane.segments[1].id, pasted.ids[0]);
    }

    #[test]
    fn a_paste_with_no_room_takes_the_next_free_lane_or_a_new_one() {
        let (mut project, _) = project(&[4_000_000]);
        let overlay = clip(0, 3_000_000);
        let overlay_id = overlay.id.clone();
        project.tracks[1].segments.push(overlay);
        let board = copy(&project, &[overlay_id]).unwrap();
        // At one second the overlay lane is taken, and the main lane is not
        // a place for overlays: a new lane.
        let pasted = paste(&project, &board, 1_000_000, None, &same).unwrap();
        apply(&mut project, pasted.commands).unwrap();
        assert_eq!(project.tracks.len(), 4);
        let (track, segment) = project.segment(&pasted.ids[0]).unwrap();
        assert_eq!(track.kind, TrackKind::Video);
        assert_eq!(segment.target_range.start, 1_000_000);
    }

    #[test]
    fn a_paste_on_the_magnetic_main_lane_inserts_at_the_cut() {
        let (mut project, ids) = project(&[2_000_000, 2_000_000]);
        let main = project.tracks[0].id.clone();
        let board = copy(&project, &[ids[0].clone()]).unwrap();
        // The playhead inside the first clip: the copy goes after it.
        let pasted = paste(&project, &board, 1_000_000, Some(&main), &same).unwrap();
        apply(&mut project, pasted.commands).unwrap();
        let starts: Vec<(String, Micros)> = project.tracks[0]
            .segments
            .iter()
            .map(|s| (s.id.clone(), s.target_range.start))
            .collect();
        assert_eq!(
            starts,
            vec![
                (ids[0].clone(), 0),
                (pasted.ids[0].clone(), 2_000_000),
                (ids[1].clone(), 4_000_000)
            ]
        );
    }

    #[test]
    fn a_pasted_pair_is_linked_again_and_lines_up() {
        let (mut project, ids) = project(&[2_000_000]);
        let mut sound = clip(0, 2_000_000);
        sound.material_id = "m".into();
        let sound_id = sound.id.clone();
        project.tracks[2].segments.push(sound);
        let command = link(&project, &[ids[0].clone(), sound_id.clone()]).unwrap();
        History::default().apply(&mut project, command).unwrap();
        let main = project.tracks[0].id.clone();

        // Copying the picture copies its sound too.
        let board = copy(&project, &[ids[0].clone()]).unwrap();
        assert_eq!(board.clips.len(), 2);
        let pasted = paste(&project, &board, 500_000, Some(&main), &same).unwrap();
        apply(&mut project, pasted.commands).unwrap();
        assert_eq!(pasted.ids.len(), 2);
        let group = project
            .link_group_of(&pasted.ids[0])
            .cloned()
            .expect("linked");
        assert_eq!(project.link_group_of(&pasted.ids[1]), Some(&group));
        assert_ne!(Some(&group), project.link_group_of(&ids[0]));
        let starts: Vec<Micros> = pasted
            .ids
            .iter()
            .map(|id| project.segment(id).unwrap().1.target_range.start)
            .collect();
        assert_eq!(starts, vec![2_000_000, 2_000_000]);
    }

    #[test]
    fn titles_get_their_own_material() {
        let (project, ids) = project(&[1_000_000]);
        let board = copy(&project, &ids).unwrap();
        let fresh = |_: &str| "fresh".to_string();
        let pasted = paste(&project, &board, 5_000_000, None, &fresh).unwrap();
        match &pasted.commands.last() {
            Some(EditCommand::InsertSegment { segment, .. }) => {
                assert_eq!(segment.material_id, "fresh")
            }
            other => panic!("expected an insert, got {other:?}"),
        }
    }

    #[test]
    fn the_insertion_point_is_a_cut() {
        let (project, _) = project(&[2_000_000, 2_000_000]);
        let lane = &project.tracks[0];
        assert_eq!(insertion_point(lane, 1_000_000), 2_000_000);
        assert_eq!(insertion_point(lane, 2_000_000), 2_000_000);
        assert_eq!(insertion_point(lane, 9_000_000), 4_000_000);
    }
}
