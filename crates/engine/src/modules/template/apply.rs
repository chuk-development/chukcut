//! A template put into a project that is already open: as a new timeline,
//! or as a compound clip at the playhead.
//!
//! The template is built exactly as "New project from template" builds it
//! (`commands::template_build_project`: slots filled, assets drawn), and
//! that whole document is then carried into the open one as a sequence
//! (decision 0024): its lanes become a new timeline tab, or the contents of a
//! compound clip. Everything is one `EditCommand`, so one Ctrl+Z takes the
//! template out again.
//!
//! Two documents meet here, so ids have to be made unique first. Every id
//! the template defines — materials, lanes, clips, markers, link groups,
//! pool entries — is replaced by a fresh one ([`fresh_ids`]), because a
//! user template is read from disk with the same ids every time and the same
//! template applied twice must not collide with itself. A media file the
//! open project already has (a placeholder of the same slot shape, the same
//! music bed, a clip the user imported before) is not added twice: the
//! template's clips are pointed at the project's material for that path.
//!
//! What enters the pool: media and titles through `AddMaterial`, transitions
//! and link groups with the sequence (`SequenceEdit::Add` carries them), and
//! the parameter blocks — grades, effects, animations, masks, speed curves,
//! slot markers and the other `extras` — straight into the pool before the
//! edit, the convention every parameter edit follows ("an unreferenced
//! material is inert", `template_replace_media`, `analysis::commands`).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::modules::project::document::{
    new_id, Id, MaterialPool, Micros, Project, Segment, TimeRange, Track, TrackKind,
};
use crate::modules::project::Transform;
use crate::modules::sequence::{Sequence, SequenceEdit, SequenceKind};
use crate::modules::timeline::ops::{EditCommand, PoolMaterial};

/// How a template goes into an open project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyAs {
    /// A new timeline tab, opened.
    #[default]
    Timeline,
    /// A compound clip at the playhead, on the first video lane with room.
    Compound,
}

impl ApplyAs {
    pub fn label(self) -> &'static str {
        match self {
            ApplyAs::Timeline => "a new timeline",
            ApplyAs::Compound => "a compound clip",
        }
    }
}

/// The edit that brings a built template into `project`, and what has to be
/// put into the pool before it is applied.
pub struct Merge {
    pub command: EditCommand,
    /// Parameter blocks the edit's clips name. Inserted into the pool first
    /// ([`insert_inert`]); inert until the edit references them.
    pub inert: MaterialPool,
    /// The new sequence: the timeline, or the compound clip's contents.
    pub sequence_id: Id,
    /// The compound clip, when the template went in as one.
    pub segment_id: Option<Id>,
    /// Every id the template defined → the id it has in the project.
    pub ids: HashMap<Id, Id>,
    /// A sentence when the template was made for another canvas.
    pub note: Option<String>,
}

/// Every id `project` defines, outside its own id and the active sequence's
/// (which the caller replaces itself).
fn defined_ids(project: &Project) -> BTreeSet<Id> {
    let pool = &project.materials;
    let mut ids: BTreeSet<Id> = BTreeSet::new();
    ids.extend(pool.videos.iter().map(|m| m.id.clone()));
    ids.extend(pool.audios.iter().map(|m| m.id.clone()));
    ids.extend(pool.images.iter().map(|m| m.id.clone()));
    ids.extend(pool.texts.iter().map(|m| m.id.clone()));
    ids.extend(pool.transitions.iter().map(|m| m.id.clone()));
    ids.extend(pool.color_adjusts.iter().map(|m| m.id.clone()));
    ids.extend(pool.effects.iter().map(|m| m.id.clone()));
    ids.extend(pool.animations.iter().map(|m| m.id.clone()));
    ids.extend(pool.trackings.iter().map(|m| m.id.clone()));
    ids.extend(pool.follows.iter().map(|m| m.id.clone()));
    ids.extend(pool.speed_curves.iter().map(|m| m.id.clone()));
    ids.extend(pool.compositing.iter().map(|m| m.id.clone()));
    ids.extend(pool.links.iter().cloned());
    ids.extend(pool.extras.keys().cloned());
    let lanes = |tracks: &[Track], ids: &mut BTreeSet<Id>| {
        for track in tracks {
            ids.insert(track.id.clone());
            ids.extend(track.segments.iter().map(|s| s.id.clone()));
        }
    };
    lanes(&project.tracks, &mut ids);
    ids.extend(project.markers.iter().map(|m| m.id.clone()));
    for sequence in &pool.sequences {
        ids.insert(sequence.id.clone());
        lanes(&sequence.tracks, &mut ids);
        ids.extend(sequence.markers.iter().map(|m| m.id.clone()));
    }
    // The active sequence's id is not a reference anything else carries
    // (nothing shows the template's main timeline); the caller gives the new
    // sequence its own id.
    ids.remove(&project.sequence.id);
    ids.remove(&project.id);
    ids
}

/// Replace every string in `value` that `map` names — as a value or as an
/// object key — by what it maps to. Ids are UUIDs, so a string that equals
/// one is that id.
fn rename(value: &mut Value, map: &HashMap<Id, Id>) {
    match value {
        Value::String(s) => {
            if let Some(to) = map.get(s.as_str()) {
                *s = to.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rename(v, map)),
        Value::Object(object) => {
            let keys: Vec<String> = object
                .keys()
                .filter(|k| map.contains_key(k.as_str()))
                .cloned()
                .collect();
            for key in keys {
                if let Some(v) = object.remove(&key) {
                    object.insert(map[&key].clone(), v);
                }
            }
            object.values_mut().for_each(|v| rename(v, map));
        }
        _ => {}
    }
}

/// `template` with fresh ids for everything it defines, except media files
/// `into` already has, which take `into`'s material id. Answers the renamed
/// template and the map.
pub fn fresh_ids(template: &Project, into: &Project) -> Result<(Project, HashMap<Id, Id>), String> {
    let pool = &into.materials;
    let mut map: HashMap<Id, Id> = defined_ids(template)
        .into_iter()
        .map(|id| (id, new_id()))
        .collect();
    let t = &template.materials;
    for m in &t.videos {
        if let Some(have) = pool.videos.iter().find(|v| v.path == m.path) {
            map.insert(m.id.clone(), have.id.clone());
        }
    }
    for m in &t.audios {
        if let Some(have) = pool.audios.iter().find(|v| v.path == m.path) {
            map.insert(m.id.clone(), have.id.clone());
        }
    }
    for m in &t.images {
        if let Some(have) = pool.images.iter().find(|v| v.path == m.path) {
            map.insert(m.id.clone(), have.id.clone());
        }
    }
    let mut value = serde_json::to_value(template).map_err(|e| e.to_string())?;
    rename(&mut value, &map);
    let renamed: Project = serde_json::from_value(value)
        .map_err(|e| format!("the template does not read after renaming its ids: {e}"))?;
    Ok((renamed, map))
}

/// Put `inert`'s parameter blocks into `pool`. The two sorted categories
/// stay sorted.
pub fn insert_inert(pool: &mut MaterialPool, inert: &MaterialPool) {
    pool.color_adjusts
        .extend(inert.color_adjusts.iter().cloned());
    pool.effects.extend(inert.effects.iter().cloned());
    pool.trackings.extend(inert.trackings.iter().cloned());
    pool.follows.extend(inert.follows.iter().cloned());
    pool.compositing.extend(inert.compositing.iter().cloned());
    for a in &inert.animations {
        let at = pool.animations.partition_point(|m| m.id < a.id);
        pool.animations.insert(at, a.clone());
    }
    for c in &inert.speed_curves {
        let at = pool.speed_curves.partition_point(|m| m.id < c.id);
        pool.speed_curves.insert(at, c.clone());
    }
    for (id, origin) in &inert.origins {
        pool.origins
            .entry(id.clone())
            .or_insert_with(|| origin.clone());
    }
    for (id, value) in &inert.extras {
        pool.extras.insert(id.clone(), value.clone());
    }
}

/// The video lane a compound clip `range` long goes on: the main lane when it
/// has room there, else the first unlocked video lane above it with room;
/// `None` when a new lane is needed.
fn lane_for(project: &Project, range: &TimeRange) -> Option<usize> {
    project
        .tracks
        .iter()
        .enumerate()
        .find(|(_, t)| t.kind == TrackKind::Video && !t.locked && t.is_range_free(range, None))
        .map(|(i, _)| i)
}

/// Build the edit that brings `template` — a document `template_build_project`
/// made — into `project` as `mode`, the compound clip starting at `at`.
/// `name` names the new timeline or compound clip.
pub fn merge(
    project: &Project,
    template: &Project,
    name: &str,
    mode: ApplyAs,
    at: Micros,
) -> Result<Merge, String> {
    if template.duration() <= 0 {
        return Err("the template is empty".into());
    }
    let (t, ids) = fresh_ids(template, project)?;
    let pool = &project.materials;
    let mut commands: Vec<EditCommand> = Vec::new();

    // Media and titles the project does not have yet, through the undoable
    // pool edit.
    let mut add = |material: PoolMaterial, index: usize| {
        commands.push(EditCommand::AddMaterial { material, index });
    };
    let new_videos = t
        .materials
        .videos
        .iter()
        .filter(|m| pool.video(&m.id).is_none());
    for (i, m) in new_videos.enumerate() {
        add(PoolMaterial::Video(m.clone()), pool.videos.len() + i);
    }
    let new_audios = t
        .materials
        .audios
        .iter()
        .filter(|m| pool.audio(&m.id).is_none());
    for (i, m) in new_audios.enumerate() {
        add(PoolMaterial::Audio(m.clone()), pool.audios.len() + i);
    }
    let new_images = t
        .materials
        .images
        .iter()
        .filter(|m| pool.image(&m.id).is_none());
    for (i, m) in new_images.enumerate() {
        add(PoolMaterial::Image(m.clone()), pool.images.len() + i);
    }
    for (i, m) in t.materials.texts.iter().enumerate() {
        add(PoolMaterial::Text(m.clone()), pool.texts.len() + i);
    }

    // The template's own compound clips, then its timeline as a sequence of
    // the project. Only the compound sequences its lanes reach come along.
    let reached = reached_sequences(&t);
    let mut parked = pool.sequences.len();
    for sequence in t
        .materials
        .sequences
        .iter()
        .filter(|s| reached.contains(&s.id))
    {
        let mut sequence = sequence.clone();
        sequence.kind = SequenceKind::Compound;
        commands.push(EditCommand::Sequence {
            edit: SequenceEdit::Add {
                sequence,
                index: parked,
                transitions: Vec::new(),
                links: Vec::new(),
            },
        });
        parked += 1;
    }
    let sequence_id = new_id();
    let kind = match mode {
        ApplyAs::Timeline => SequenceKind::Timeline,
        ApplyAs::Compound => SequenceKind::Compound,
    };
    commands.push(EditCommand::Sequence {
        edit: SequenceEdit::Add {
            sequence: Sequence {
                id: sequence_id.clone(),
                name: name.to_string(),
                kind,
                tracks: t.tracks.clone(),
                markers: t.markers.clone(),
            },
            index: parked,
            transitions: t.materials.transitions.clone(),
            links: t.materials.links.iter().cloned().collect(),
        },
    });

    let mut segment_id = None;
    let label = match mode {
        ApplyAs::Timeline => {
            commands.push(EditCommand::Sequence {
                edit: SequenceEdit::Activate {
                    from: project.sequence.id.clone(),
                    from_path: project.sequence.path.clone(),
                    to: sequence_id.clone(),
                    to_path: Vec::new(),
                },
            });
            "Template as a timeline"
        }
        ApplyAs::Compound => {
            let at = at.max(0);
            let duration = t.duration();
            let range = TimeRange::new(at, duration);
            let segment = Segment {
                id: new_id(),
                material_id: sequence_id.clone(),
                target_range: range,
                source_range: TimeRange::new(0, duration),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                transform: Transform::default(),
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            };
            segment_id = Some(segment.id.clone());
            let (track_id, index) = match lane_for(project, &range) {
                Some(lane) => {
                    let track = &project.tracks[lane];
                    let index = track
                        .segments
                        .iter()
                        .filter(|s| s.target_range.start < at)
                        .count();
                    (track.id.clone(), index)
                }
                None => {
                    let count = project
                        .tracks
                        .iter()
                        .filter(|t| t.kind == TrackKind::Video)
                        .count();
                    let lane = Track::new(TrackKind::Video, format!("Video {}", count + 1));
                    let id = lane.id.clone();
                    commands.push(EditCommand::AddTrack {
                        track: lane,
                        index: project.tracks.len(),
                    });
                    (id, 0)
                }
            };
            commands.push(EditCommand::InsertSegment {
                track_id,
                segment,
                index,
            });
            "Template as a compound clip"
        }
    };

    let inert = MaterialPool {
        color_adjusts: t.materials.color_adjusts.clone(),
        effects: t.materials.effects.clone(),
        animations: t.materials.animations.clone(),
        trackings: t.materials.trackings.clone(),
        follows: t.materials.follows.clone(),
        speed_curves: t.materials.speed_curves.clone(),
        compositing: t.materials.compositing.clone(),
        origins: t
            .materials
            .origins
            .iter()
            .filter(|(id, _)| !pool.origins.contains_key(*id))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        extras: t.materials.extras.clone(),
        ..MaterialPool::default()
    };
    let (tw, th) = (template.canvas.width, template.canvas.height);
    let (pw, ph) = (project.canvas.width, project.canvas.height);
    let note = (tw as u64 * ph as u64 != th as u64 * pw as u64).then(|| {
        format!(
            "The template was made for {tw}\u{d7}{th}; it is laid out on this project's \
             {pw}\u{d7}{ph} canvas"
        )
    });
    Ok(Merge {
        command: EditCommand::Composite {
            label: label.to_string(),
            commands,
        },
        inert,
        sequence_id,
        segment_id,
        ids,
        note,
    })
}

/// The parked sequences `project`'s active lanes show, through compound clips
/// inside compound clips.
fn reached_sequences(project: &Project) -> BTreeSet<Id> {
    let mut found: BTreeSet<Id> = BTreeSet::new();
    let mut queue: Vec<&Track> = project.tracks.iter().collect();
    while let Some(track) = queue.pop() {
        for segment in &track.segments {
            if let Some(inner) = project.materials.sequence(&segment.material_id) {
                if found.insert(inner.id.clone()) {
                    queue.extend(inner.tracks.iter());
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::CanvasConfig;
    use crate::modules::template::commands::tests_support::built;

    fn open_project() -> Project {
        let mut p = Project::new("open", CanvasConfig::default(), 30.0);
        p.tracks.push(Track::new(TrackKind::Video, "Video 1"));
        p.tracks.push(Track::new(TrackKind::Audio, "Audio 1"));
        p
    }

    #[test]
    fn every_id_is_fresh_and_references_follow() {
        let template = built("quick-cuts");
        let into = open_project();
        let (renamed, map) = fresh_ids(&template, &into).unwrap();
        let before = defined_ids(&template);
        let after = defined_ids(&renamed);
        assert_eq!(before.len(), after.len());
        assert!(before.is_disjoint(&after));
        // Every clip still names materials and markers the pool has.
        for track in &renamed.tracks {
            for s in &track.segments {
                assert!(renamed.materials.kind_of(&s.material_id).is_some());
                for e in &s.extras {
                    assert!(after.contains(e), "{e} lost its entry");
                }
            }
        }
        assert_eq!(map.len(), before.len());
    }

    #[test]
    fn media_the_project_has_is_not_added_again() {
        let template = built("quick-cuts");
        let mut into = open_project();
        // The project already uses the template's first placeholder file.
        let shared = template.materials.images[0].clone();
        into.materials.images.push(shared.clone());
        let (renamed, _) = fresh_ids(&template, &into).unwrap();
        let still = renamed
            .materials
            .images
            .iter()
            .find(|m| m.path == shared.path)
            .unwrap();
        assert_eq!(still.id, shared.id);
        let merged = merge(&into, &template, "T", ApplyAs::Timeline, 0).unwrap();
        let EditCommand::Composite { commands, .. } = &merged.command else {
            panic!("a composite")
        };
        let images_added = commands
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    EditCommand::AddMaterial {
                        material: PoolMaterial::Image(m),
                        ..
                    } if m.path == shared.path
                )
            })
            .count();
        assert_eq!(images_added, 0);
    }
}
