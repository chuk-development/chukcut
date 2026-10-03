//! What the sequence gestures mean, as edit commands.
//!
//! Each function reads the document and returns one `EditCommand` — almost
//! always a `Composite` of the ordinary segment and lane primitives plus the
//! [`SequenceEdit`] ones — so undo, autosave and validation treat a compound
//! clip like any other edit. Nothing here mutates the document it is given.
//!
//! The composites are built against a scratch copy: every part is applied to
//! it as it is added, so a later part's index is the index the document will
//! actually have at that point, and a gesture that cannot work is refused
//! here with the reason rather than half-applied.

use std::collections::{BTreeMap, BTreeSet};

use super::edit::SequenceEdit;
use super::{exists, Sequence, SequenceKind};
use crate::modules::project::{
    new_id, source_duration_for, Id, Micros, Project, Segment, TimeRange, Track, TrackKind,
    Transform,
};
use crate::modules::timeline::ops::{EditCommand, PoolMaterial};

/// A composite under construction, checked part by part.
struct Builder {
    scratch: Project,
    commands: Vec<EditCommand>,
}

impl Builder {
    fn new(project: &Project) -> Self {
        Self {
            scratch: project.clone(),
            commands: Vec::new(),
        }
    }

    fn push(&mut self, command: EditCommand) -> Result<(), String> {
        command.apply(&mut self.scratch)?;
        self.commands.push(command);
        Ok(())
    }

    fn sequence(&mut self, edit: SequenceEdit) -> Result<(), String> {
        self.push(EditCommand::Sequence { edit })
    }

    fn finish(self, label: &str) -> EditCommand {
        EditCommand::Composite {
            label: label.to_string(),
            commands: self.commands,
        }
    }
}

/// What "Create compound clip" made, so the caller can select it.
#[derive(Debug, Clone)]
pub struct Compound {
    pub command: EditCommand,
    pub sequence_id: Id,
    pub segment_id: Id,
}

/// Move the clips `segment_ids` into a new compound clip, which takes their
/// place on the timeline. Their link partners come with them: a picture
/// without its linked sound would leave the sound behind on a lane where the
/// clip it belongs to is gone.
///
/// The compound clip goes on the lowest video lane the selection used, when
/// it fits there once the clips have left — which is the main track whenever
/// the selection touched it — and otherwise on the first video lane it fits,
/// or a new one.
pub fn create_compound(
    project: &Project,
    segment_ids: &[String],
    name: Option<String>,
) -> Result<Compound, String> {
    let mut chosen: BTreeSet<&str> = BTreeSet::new();
    for id in segment_ids {
        if project.segment(id).is_none() {
            return Err(format!("there is no clip {id} on this timeline"));
        }
        chosen.insert(id.as_str());
        if let Some(group) = project.link_group_of(id) {
            for (_, _, partner) in project.link_members(group) {
                chosen.insert(partner.id.as_str());
            }
        }
    }
    if chosen.is_empty() {
        return Err("select the clips to put in a compound clip".into());
    }

    // The lanes the selection uses, in stacking order, with their clips.
    let mut lanes: Vec<(usize, Vec<(usize, &Segment)>)> = Vec::new();
    for (lane_index, track) in project.tracks.iter().enumerate() {
        let picked: Vec<(usize, &Segment)> = track
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| chosen.contains(s.id.as_str()))
            .collect();
        if picked.is_empty() {
            continue;
        }
        if track.locked {
            return Err(format!(
                "\"{}\" is locked, so its clips cannot move into a compound clip",
                track.name
            ));
        }
        lanes.push((lane_index, picked));
    }
    let all = || lanes.iter().flat_map(|(_, picked)| picked.iter());
    let start = all()
        .map(|(_, s)| s.target_range.start)
        .min()
        .expect("non-empty");
    let end = all()
        .map(|(_, s)| s.target_range.end())
        .max()
        .expect("non-empty");

    let sequence_id = new_id();
    let compounds = project
        .materials
        .sequences
        .iter()
        .filter(|s| s.kind == SequenceKind::Compound)
        .count();
    let name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Compound clip {}", compounds + 1));
    let tracks: Vec<Track> = lanes
        .iter()
        .enumerate()
        .map(|(inner_index, (lane_index, picked))| {
            let lane = &project.tracks[*lane_index];
            Track {
                id: new_id(),
                kind: lane.kind,
                name: lane.name.clone(),
                segments: picked
                    .iter()
                    .map(|(_, s)| {
                        let mut s = (*s).clone();
                        s.target_range.start -= start;
                        s.render_index = inner_index as i32;
                        s
                    })
                    .collect(),
                muted: lane.muted,
                locked: false,
                hidden: lane.hidden,
                volume: lane.volume,
            }
        })
        .collect();

    let mut b = Builder::new(project);
    // Right to left on every lane, so each index is still where it was.
    for (lane_index, picked) in &lanes {
        let track_id = project.tracks[*lane_index].id.clone();
        for (index, segment) in picked.iter().rev() {
            b.push(EditCommand::RemoveSegment {
                track_id: track_id.clone(),
                segment: (*segment).clone(),
                index: *index,
            })?;
        }
    }
    let at = b.scratch.materials.sequences.len();
    b.sequence(SequenceEdit::Add {
        sequence: Sequence {
            id: sequence_id.clone(),
            name,
            kind: SequenceKind::Compound,
            tracks,
            markers: Vec::new(),
        },
        index: at,
        transitions: Vec::new(),
        links: Vec::new(),
    })?;

    let range = TimeRange::new(start, end - start);
    let selected_video = lanes
        .iter()
        .map(|(i, _)| *i)
        .filter(|i| project.tracks[*i].kind == TrackKind::Video);
    let other_video = (0..project.tracks.len())
        .filter(|i| project.tracks[*i].kind == TrackKind::Video && !project.tracks[*i].locked);
    let lane_id = selected_video
        .chain(other_video)
        .map(|i| b.scratch.tracks[i].id.clone())
        .find(|id| {
            b.scratch
                .track(id)
                .is_some_and(|t| t.is_range_free(&range, None))
        });
    let lane_id = match lane_id {
        Some(id) => id,
        None => {
            let count = b
                .scratch
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .count();
            let lane = Track::new(TrackKind::Video, format!("Video {}", count + 1));
            let id = lane.id.clone();
            let index = lanes[0].0.min(b.scratch.tracks.len());
            b.push(EditCommand::AddTrack { track: lane, index })?;
            id
        }
    };
    let segment = Segment {
        id: new_id(),
        material_id: sequence_id.clone(),
        target_range: range,
        source_range: TimeRange::new(0, range.duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    let segment_id = segment.id.clone();
    let index = b
        .scratch
        .track(&lane_id)
        .map(|t| {
            t.segments
                .iter()
                .filter(|s| s.target_range.start < start)
                .count()
        })
        .unwrap_or(0);
    b.push(EditCommand::InsertSegment {
        track_id: lane_id,
        segment,
        index,
    })?;

    Ok(Compound {
        command: b.finish("Create compound clip"),
        sequence_id,
        segment_id,
    })
}

/// Put a compound clip's contents back on the timeline in its place.
///
/// The part of the compound clip's sequence the clip shows comes back, cut at
/// its edges; the first video lane inside lands on the compound clip's own
/// lane and every other lane on a new one directly above or below it, in the
/// order they were stacked inside. What the compound clip itself carried — a
/// transform, a grade, effects — goes with it.
///
/// When nothing else shows the sequence it is removed and the clips keep their
/// ids, links and transitions. When another compound clip still uses it, the
/// clips are copies: new ids, their own titles, and no links or transitions,
/// as a paste would have them.
pub fn flatten(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("there is no clip {segment_id} on this timeline"))?;
    let sequence = project
        .materials
        .sequence(&segment.material_id)
        .ok_or("that clip is not a compound clip")?;
    if track.locked {
        return Err(format!("\"{}\" is locked", track.name));
    }
    if project.materials.speed_curve_of(segment).is_some() || (segment.speed - 1.0).abs() > 1e-6 {
        return Err(
            "set the compound clip back to normal speed before putting its clips back".into(),
        );
    }
    let window_start = segment.source_range.start;
    let window_end = window_start + segment.target_range.duration;
    let offset = segment.target_range.start - window_start;
    let shared = super::uses_of(project, &sequence.id) > 1;
    let pool = &project.materials;

    let mut texts: Vec<PoolMaterial> = Vec::new();
    let mut lanes: Vec<(&Track, Vec<Segment>)> = Vec::new();
    for lane in &sequence.tracks {
        let mut mapped = Vec::new();
        for s in &lane.segments {
            let a = s.target_range.start.max(window_start);
            let b = s.target_range.end().min(window_end);
            if b <= a {
                continue;
            }
            let mut c = s.clone();
            if a > s.target_range.start || b < s.target_range.end() {
                if pool.speed_curve_of(s).is_some() {
                    return Err(
                        "a clip on a speed curve is cut by the compound clip's edge; \
                         trim the compound clip to whole clips first"
                            .into(),
                    );
                }
                let head = a - s.target_range.start;
                if head > 0 {
                    c.source_range.start += source_duration_for(head, s.speed);
                    crate::modules::timeline::ops::rebase_keyframes_for_split(
                        &mut c.keyframes,
                        head,
                    );
                }
                c.target_range = TimeRange::new(a, b - a);
                c.source_range.duration = source_duration_for(b - a, s.speed);
            }
            c.target_range.start += offset;
            if shared {
                c.id = new_id();
                c.extras
                    .retain(|e| pool.transition(e).is_none() && !pool.links.contains(e));
                if let Some(text) = pool.text(&c.material_id) {
                    let mut copy = text.clone();
                    copy.id = new_id();
                    c.material_id = copy.id.clone();
                    texts.push(PoolMaterial::Text(copy));
                }
            }
            mapped.push(c);
        }
        if !mapped.is_empty() {
            lanes.push((lane, mapped));
        }
    }

    let mut b = Builder::new(project);
    let lane_index = project
        .tracks
        .iter()
        .position(|t| t.id == track.id)
        .expect("found above");
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment.id)
        .expect("found above");
    b.push(EditCommand::RemoveSegment {
        track_id: track.id.clone(),
        segment: segment.clone(),
        index,
    })?;
    if !shared {
        let at = pool
            .sequences
            .iter()
            .position(|s| s.id == sequence.id)
            .expect("found above");
        b.sequence(SequenceEdit::Remove {
            sequence: sequence.clone(),
            index: at,
            transitions: Vec::new(),
            links: Vec::new(),
        })?;
    }
    for text in texts {
        let index = b.scratch.materials.texts.len();
        b.push(EditCommand::AddMaterial {
            material: text,
            index,
        })?;
    }

    // Which inner lane lands on the compound clip's own lane.
    let home = lanes
        .iter()
        .position(|(lane, _)| lane.kind == TrackKind::Video && track.kind == TrackKind::Video);
    let mut targets: Vec<Id> = Vec::with_capacity(lanes.len());
    let mut below = 0usize;
    let mut above = 0usize;
    for (i, (lane, _)) in lanes.iter().enumerate() {
        if Some(i) == home {
            targets.push(track.id.clone());
            continue;
        }
        let mut new_lane = Track::new(lane.kind, lane.name.clone());
        new_lane.muted = lane.muted;
        new_lane.hidden = lane.hidden;
        new_lane.volume = lane.volume;
        // Lanes stacked under the one that takes the compound clip's place go
        // under it, the rest directly above, each in the order it had inside.
        let at = if home.is_some_and(|h| i < h) {
            below += 1;
            lane_index + below - 1
        } else {
            above += 1;
            lane_index + below + above
        };
        targets.push(new_lane.id.clone());
        b.push(EditCommand::AddTrack {
            track: new_lane,
            index: at.min(b.scratch.tracks.len()),
        })?;
    }
    for ((_, segments), target) in lanes.into_iter().zip(targets) {
        for s in segments {
            let index = b
                .scratch
                .track(&target)
                .map(|t| {
                    t.segments
                        .iter()
                        .filter(|o| o.target_range.start < s.target_range.start)
                        .count()
                })
                .unwrap_or(0);
            b.push(EditCommand::InsertSegment {
                track_id: target.clone(),
                segment: s,
                index,
            })?;
        }
    }
    Ok(b.finish("Put compound clip back"))
}

/// Open the compound clip `segment_id`: its sequence becomes the one being
/// edited, with the current one on the breadcrumb trail.
pub fn open(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("there is no clip {segment_id} on this timeline"))?;
    if project.materials.sequence(&segment.material_id).is_none() {
        return Err("that clip is not a compound clip".into());
    }
    let mut to_path = project.sequence.path.clone();
    to_path.push(project.sequence.id.clone());
    Ok(activate_command(project, &segment.material_id, to_path))
}

/// Go back out to breadcrumb `level` (0 is the timeline). Closing one level is
/// `level = path.len() - 1`.
pub fn close_to(project: &Project, level: usize) -> Result<EditCommand, String> {
    let path = &project.sequence.path;
    let to = path
        .get(level)
        .ok_or("no compound clip is open at that level")?
        .clone();
    Ok(activate_command(project, &to, path[..level].to_vec()))
}

/// Close the innermost open compound clip.
pub fn close(project: &Project) -> Result<EditCommand, String> {
    let depth = project.sequence.path.len();
    if depth == 0 {
        return Err("no compound clip is open".into());
    }
    close_to(project, depth - 1)
}

fn activate_command(project: &Project, to: &str, to_path: Vec<Id>) -> EditCommand {
    EditCommand::Sequence {
        edit: SequenceEdit::Activate {
            from: project.sequence.id.clone(),
            from_path: project.sequence.path.clone(),
            to: to.to_string(),
            to_path,
        },
    }
}

fn timeline_kind(project: &Project, id: &str) -> Option<SequenceKind> {
    if project.sequence.id == id {
        Some(project.sequence.kind)
    } else {
        project.materials.sequence(id).map(|s| s.kind)
    }
}

/// Make timeline `id` the one being edited, closing any open compound clip.
pub fn switch(project: &Project, id: &str) -> Result<EditCommand, String> {
    if timeline_kind(project, id) != Some(SequenceKind::Timeline) {
        return Err(format!("there is no timeline {id}"));
    }
    if super::root_id(project) == id && project.sequence.path.is_empty() {
        return Err("that timeline is already open".into());
    }
    Ok(activate_command(project, id, Vec::new()))
}

/// A new, empty timeline at the end of the tabs — a video and an audio lane,
/// as a new project has — opened at once. Answers the command and its id.
pub fn new_timeline(project: &Project, name: Option<String>) -> Result<(EditCommand, Id), String> {
    let count = super::timelines(project).len();
    let name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Timeline {:02}", count + 1));
    let sequence = Sequence {
        id: new_id(),
        name,
        kind: SequenceKind::Timeline,
        tracks: vec![
            Track::new(TrackKind::Video, "Video 1"),
            Track::new(TrackKind::Audio, "Audio 1"),
        ],
        markers: Vec::new(),
    };
    let id = sequence.id.clone();
    let mut b = Builder::new(project);
    b.sequence(SequenceEdit::Add {
        sequence,
        index: project.materials.sequences.len(),
        transitions: Vec::new(),
        links: Vec::new(),
    })?;
    let to = activate_command(&b.scratch, &id, Vec::new());
    b.push(to)?;
    Ok((b.finish("New timeline"), id))
}

/// Rename any sequence: a timeline tab or a compound clip.
pub fn rename(project: &Project, id: &str, name: &str) -> Result<EditCommand, String> {
    let before = super::name_of(project, id).ok_or_else(|| format!("there is no timeline {id}"))?;
    let after = name.trim();
    if after.is_empty() {
        return Err("a timeline needs a name".into());
    }
    if after == before {
        return Err("that is already its name".into());
    }
    Ok(EditCommand::Sequence {
        edit: SequenceEdit::Rename {
            id: id.to_string(),
            before,
            after: after.to_string(),
        },
    })
}

/// Delete timeline `id`. The last timeline cannot go. Deleting the one being
/// edited opens its neighbour first.
///
/// Compound clips its clips used stay in the pool, unused, so undo finds them.
pub fn delete_timeline(project: &Project, id: &str) -> Result<EditCommand, String> {
    if timeline_kind(project, id) != Some(SequenceKind::Timeline) {
        return Err(format!("there is no timeline {id}"));
    }
    let tabs = super::timelines(project);
    if tabs.len() < 2 {
        return Err("a project keeps at least one timeline".into());
    }
    if super::uses_of(project, id) > 0 {
        return Err("that timeline is used as a compound clip".into());
    }
    let mut b = Builder::new(project);
    if super::root_id(project) == id {
        let at = tabs.iter().position(|t| t.id == id).expect("listed");
        let neighbour = tabs
            .get(at + 1)
            .or_else(|| at.checked_sub(1).and_then(|i| tabs.get(i)))
            .expect("two or more")
            .id
            .clone();
        let to = activate_command(&b.scratch, &neighbour, Vec::new());
        b.push(to)?;
    }
    let index = b
        .scratch
        .materials
        .sequences
        .iter()
        .position(|s| s.id == id)
        .expect("parked by now");
    let sequence = b.scratch.materials.sequences[index].clone();
    b.sequence(SequenceEdit::Remove {
        sequence,
        index,
        transitions: Vec::new(),
        links: Vec::new(),
    })?;
    Ok(b.finish("Delete timeline"))
}

/// A copy of timeline `id` as a new tab after the last one. Its clips get new
/// ids, their own link groups, transitions and titles, so editing the copy
/// never reaches into the original. Compound clips in it still show the same
/// compound sequences, as two compound clips of one sequence do anywhere.
pub fn duplicate_timeline(project: &Project, id: &str) -> Result<(EditCommand, Id), String> {
    if timeline_kind(project, id) != Some(SequenceKind::Timeline) {
        return Err(format!("there is no timeline {id}"));
    }
    let (name, tracks, markers) = if project.sequence.id == id {
        (
            project.sequence.name.clone(),
            project.tracks.clone(),
            project.markers.clone(),
        )
    } else {
        let s = project.materials.sequence(id).expect("kind found it");
        (s.name.clone(), s.tracks.clone(), s.markers.clone())
    };
    let pool = &project.materials;
    let mut links: BTreeMap<Id, Id> = BTreeMap::new();
    let mut transitions = Vec::new();
    let mut texts: Vec<PoolMaterial> = Vec::new();
    let mut copied_texts: BTreeMap<Id, Id> = BTreeMap::new();
    let tracks: Vec<Track> = tracks
        .into_iter()
        .map(|mut t| {
            t.id = new_id();
            for s in &mut t.segments {
                s.id = new_id();
                for e in &mut s.extras {
                    if pool.links.contains(e) {
                        *e = links.entry(e.clone()).or_insert_with(new_id).clone();
                    } else if let Some(transition) = pool.transition(e) {
                        let mut copy = transition.clone();
                        copy.id = new_id();
                        *e = copy.id.clone();
                        transitions.push(copy);
                    }
                }
                if let Some(text) = pool.text(&s.material_id) {
                    let copy_id = copied_texts
                        .entry(text.id.clone())
                        .or_insert_with(|| {
                            let mut copy = text.clone();
                            copy.id = new_id();
                            let id = copy.id.clone();
                            texts.push(PoolMaterial::Text(copy));
                            id
                        })
                        .clone();
                    s.material_id = copy_id;
                }
            }
            t
        })
        .collect();
    let markers = markers
        .into_iter()
        .map(|mut m| {
            m.id = new_id();
            m
        })
        .collect();
    let sequence = Sequence {
        id: new_id(),
        name: format!("{name} copy"),
        kind: SequenceKind::Timeline,
        tracks,
        markers,
    };
    let new = sequence.id.clone();
    let mut b = Builder::new(project);
    for text in texts {
        let index = b.scratch.materials.texts.len();
        b.push(EditCommand::AddMaterial {
            material: text,
            index,
        })?;
    }
    b.sequence(SequenceEdit::Add {
        sequence,
        index: project.materials.sequences.len(),
        transitions,
        links: links.into_values().collect(),
    })?;
    Ok((b.finish("Duplicate timeline"), new))
}

/// Whether `material_id` is a sequence — a compound clip's material.
pub fn is_compound(project: &Project, material_id: &str) -> bool {
    exists(project, material_id)
}

/// The timeline time of nested time `inner` of compound clip `segment`, for
/// constant speed. Used to put the playhead where the user was when they
/// open or close a compound clip.
pub fn outer_time(segment: &Segment, inner: Micros) -> Micros {
    let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
        segment.speed as f64
    } else {
        1.0
    };
    segment.target_range.start
        + ((inner - segment.source_range.start) as f64 / speed).round() as Micros
}
