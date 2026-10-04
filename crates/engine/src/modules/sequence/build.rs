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
use super::retime::Outer;
use super::{exists, Sequence, SequenceKind};
use crate::modules::project::speed::curve_target_duration;
use crate::modules::project::{
    new_id, source_duration_for, Id, MaterialKind, Micros, Project, Segment, SpeedCurveMaterial,
    TimeRange, Track, TrackKind, Transform,
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
/// A compound clip at another speed hands its speed down: each clip inside
/// plays at its own speed times the compound clip's, and keeps its keyframes
/// on the frames they were set on. A speed curve on the compound clip becomes
/// a curve on each video, audio and compound clip inside (`super::retime`;
/// exact), and a constant speed on the stills and titles that cover the same
/// stretch. Refused: a clip with a curve of its own inside a compound clip
/// with a curve (no exact form), and a curved clip the window's edge cuts.
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
    let outer = Outer::new(&project.materials, segment);
    let window = outer.window();
    let shared = super::uses_of(project, &sequence.id) > 1;
    let pool = &project.materials;

    let mut texts: Vec<PoolMaterial> = Vec::new();
    let mut lanes: Vec<(&Track, Vec<Piece>)> = Vec::new();
    for lane in &sequence.tracks {
        let mut mapped = Vec::new();
        for s in &lane.segments {
            let a = s.target_range.start.max(window.start);
            let b = s.target_range.end().min(window.end());
            if b <= a {
                continue;
            }
            let own = pool.speed_curve_of(s);
            if own.is_some() && outer.curve().is_some() {
                return Err(
                    "a clip with its own speed curve is inside this compound clip, which has a \
                     speed curve too; remove one of the two curves first"
                        .into(),
                );
            }
            // The clip cut to the window, still in nested time.
            let mut c = s.clone();
            if a > s.target_range.start || b < s.target_range.end() {
                if own.is_some() {
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
            let Some(piece) = retimed(pool, &outer, c, own)? else {
                continue;
            };
            let mut piece = piece;
            if shared {
                let c = &mut piece.segment;
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
            mapped.push(piece);
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
    for ((_, pieces), target) in lanes.into_iter().zip(targets) {
        for piece in pieces {
            let index = b
                .scratch
                .track(&target)
                .map(|t| {
                    t.segments
                        .iter()
                        .filter(|o| o.target_range.start < piece.segment.target_range.start)
                        .count()
                })
                .unwrap_or(0);
            piece.insert(&mut b, &target, index)?;
        }
    }
    Ok(b.finish("Put compound clip back"))
}

/// One clip of a compound clip on its way back to the timeline.
struct Piece {
    /// As it enters the lane: at a constant speed, without any curve.
    segment: Segment,
    /// The curve it then plays through, with the source range and timeline
    /// range that go with it.
    curve: Option<(SpeedCurveMaterial, TimeRange, TimeRange)>,
}

impl Piece {
    /// Insert the clip, then — for a curved one — give it its curve and its
    /// exact ranges. The curve goes on in two steps because a clip only takes
    /// a curve through `SetSpeedCurve`: it enters at a constant speed rounded
    /// down, so its first length is never longer than its final one and never
    /// reaches into the clip after it, and the trim then sets the ranges the
    /// curve was composed for.
    fn insert(self, b: &mut Builder, track_id: &str, index: usize) -> Result<(), String> {
        let id = self.segment.id.clone();
        let entered = self.segment.target_range;
        let source = self.segment.source_range;
        b.push(EditCommand::InsertSegment {
            track_id: track_id.to_string(),
            segment: self.segment,
            index,
        })?;
        let Some((curve, final_source, final_target)) = self.curve else {
            return Ok(());
        };
        // Never past the final end: the implied length of the shorter
        // source can round a microsecond over it.
        let first = TimeRange::new(
            entered.start,
            curve_target_duration(&curve.points, source)
                .min(final_target.duration)
                .max(1),
        );
        b.push(EditCommand::SetSpeedCurve {
            segment_id: id.clone(),
            before: None,
            after: Some(curve),
            before_target: entered,
            after_target: first,
            slot: None,
        })?;
        if first != final_target || source != final_source {
            b.push(EditCommand::TrimSegment {
                segment_id: id,
                before_target: first,
                before_source: source,
                after_target: final_target,
                after_source: final_source,
            })?;
        }
        Ok(())
    }
}

/// Clip `c` (already cut to the window, in nested time) moved and retimed onto
/// the outer timeline through `outer`. `own` is its speed curve. `None` for a
/// piece that rounds to nothing.
fn retimed(
    pool: &crate::modules::project::MaterialPool,
    outer: &Outer<'_>,
    mut c: Segment,
    own: Option<&SpeedCurveMaterial>,
) -> Result<Option<Piece>, String> {
    let (a, b) = (c.target_range.start, c.target_range.end());
    let start = outer.at(a);
    let end = outer.at(b);
    if end <= start {
        return Ok(None);
    }
    let target = TimeRange::new(start, end - start);
    c.keyframes = super::retime::keyframes(outer, &c, start);

    // Media whose picture or sound runs with time takes the compound clip's
    // curve; a still or a title only needs to cover the same stretch.
    let runs = matches!(
        pool.kind_of(&c.material_id),
        Some(MaterialKind::Video | MaterialKind::Audio | MaterialKind::Sequence)
    );
    let points = match (outer.curve(), own) {
        (None, None) => {
            c.speed = (super::retime::sane(c.speed) * outer.speed()) as f32;
            c.target_range = target;
            // The speed invariant is checked against the rounded product; the
            // source stays exactly what the clip showed.
            if (c.source_range.duration - source_duration_for(target.duration, c.speed)).abs()
                > crate::modules::project::speed_slack(c.speed)
            {
                c.source_range.duration = source_duration_for(target.duration, c.speed);
            }
            return Ok(Some(Piece {
                segment: c,
                curve: None,
            }));
        }
        (None, Some(curve)) => super::retime::scaled(curve, outer.speed()),
        (Some(curve), None) if runs => super::retime::under_curve(curve, &c),
        (Some(_), None) => {
            c.target_range = target;
            c.speed = average_speed(c.source_range.duration, target.duration);
            c.source_range.duration = source_duration_for(target.duration, c.speed);
            return Ok(Some(Piece {
                segment: c,
                curve: None,
            }));
        }
        (Some(_), Some(_)) => unreachable!("refused by the caller"),
    };
    if let Some(curve) = own {
        c.extras.retain(|e| *e != curve.id);
    }
    let final_source = c.source_range;
    c.target_range = target;
    c.speed = average_speed(final_source.duration, target.duration);
    c.source_range.duration = source_duration_for(target.duration, c.speed);
    Ok(Some(Piece {
        segment: c,
        curve: Some((
            SpeedCurveMaterial {
                id: new_id(),
                preset: None,
                points,
            },
            final_source,
            target,
        )),
    }))
}

/// `source / target` as a clip speed, rounded down to the next `f32`, so the
/// source it implies is never more than `source`.
fn average_speed(source: Micros, target: Micros) -> f32 {
    let exact = source as f64 / target.max(1) as f64;
    let mut speed = exact as f32;
    if speed as f64 > exact && speed > f32::MIN_POSITIVE {
        speed = f32::from_bits(speed.to_bits() - 1);
    }
    speed
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
/// The compound clips only this timeline showed go with it: each sequence it
/// reached, directly or through other compound clips, that no clip anywhere
/// uses any more is removed in the same step, so undo brings them back with
/// the timeline. A compound sequence that was already unused before — the
/// contents of a compound clip that was cut and waits to be pasted — is left
/// alone.
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
    let reached = reachable(project, &sequence.tracks);
    b.sequence(SequenceEdit::Remove {
        sequence,
        index,
        transitions: Vec::new(),
        links: Vec::new(),
    })?;
    // One at a time, outermost first: a compound clip inside another only
    // becomes unused once the outer one's sequence is gone.
    loop {
        let scratch = &b.scratch;
        let Some(index) = scratch.materials.sequences.iter().position(|s| {
            s.kind == SequenceKind::Compound
                && reached.contains(&s.id)
                && super::uses_of(scratch, &s.id) == 0
                && !scratch.sequence.path.contains(&s.id)
        }) else {
            break;
        };
        let sequence = scratch.materials.sequences[index].clone();
        b.sequence(SequenceEdit::Remove {
            sequence,
            index,
            transitions: Vec::new(),
            links: Vec::new(),
        })?;
    }
    Ok(b.finish("Delete timeline"))
}

/// Every sequence the clips on `tracks` show, directly or through compound
/// clips inside compound clips.
fn reachable(project: &Project, tracks: &[Track]) -> BTreeSet<Id> {
    let mut found: BTreeSet<Id> = BTreeSet::new();
    let mut queue: Vec<Id> = super::children(project, tracks).into_iter().collect();
    while let Some(id) = queue.pop() {
        if !found.insert(id.clone()) {
            continue;
        }
        if let Some(inner) = super::tracks_of(project, &id) {
            queue.extend(super::children(project, inner));
        }
    }
    found
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

/// `commands`, made against the lanes of sequence `id`, as one list of edits
/// on the open document: when `id` is not the active sequence they run
/// between an activation of `id` and one back to where the user is, so an
/// edit can reach a clip inside a compound clip or on another timeline
/// without the user opening it. The two activations cancel out, so the
/// breadcrumbs and the tab order are what they were, and undo walks the
/// same way back.
pub fn inside(project: &Project, id: &str, commands: Vec<EditCommand>) -> Vec<EditCommand> {
    if project.sequence.id == id {
        return commands;
    }
    let here = project.sequence.id.clone();
    let path = project.sequence.path.clone();
    let mut out = Vec::with_capacity(commands.len() + 2);
    out.push(EditCommand::Sequence {
        edit: SequenceEdit::Activate {
            from: here.clone(),
            from_path: path.clone(),
            to: id.to_string(),
            to_path: Vec::new(),
        },
    });
    out.extend(commands);
    out.push(EditCommand::Sequence {
        edit: SequenceEdit::Activate {
            from: id.to_string(),
            from_path: Vec::new(),
            to: here,
            to_path: path,
        },
    });
    out
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
