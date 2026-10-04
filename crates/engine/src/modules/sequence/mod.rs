//! Sequences: several timelines in one project, and compound clips.
//!
//! A project holds any number of **sequences**. Each is a stack of lanes and a
//! list of markers — exactly what `Project::tracks` and `Project::markers`
//! are. One sequence is *active*: its lanes are the ones in `Project::tracks`,
//! so every edit command, the preview, the timeline UI and every analysis
//! module work on it without knowing sequences exist. The others are *parked*
//! in `MaterialPool::sequences`.
//!
//! A sequence is a material. A segment whose `material_id` names a sequence is
//! a **compound clip**: its picture is that sequence rendered at the segment's
//! source time, and its sound is that sequence's mix. Two kinds share the one
//! type — a `Timeline` is a top-level tab (CapCut's "Timeline 01"), a
//! `Compound` is what "Create compound clip" makes and is shown only through
//! the clips that use it.
//!
//! Switching the active sequence is an edit (`SequenceEdit::Activate`) and is
//! on the undo stack. That is not decoration: every command on the stack
//! names segments and lanes of the sequence that was active when it was made,
//! so undoing an edit made inside a compound clip only works while that
//! compound is active again. Putting navigation on the same stack keeps the
//! two in step. Decision 0024.
//!
//! The order of all sequences — the tab order — is the pool list with the
//! active sequence inserted at `ActiveSequence::slot`. Switching takes the
//! target out of that list and parks the old one in its place, so a switch and
//! its inverse leave the list exactly as it was.

pub mod audio;
pub mod build;
pub mod commands;
pub mod digest;
pub mod edit;
pub mod retime;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::modules::project::{Id, Marker, Micros, Project, Track};

pub use edit::SequenceEdit;

/// The id the main timeline has in a project that never had a second one.
/// A file written before sequences existed has no `sequence` key and reads as
/// this.
pub const MAIN_SEQUENCE_ID: &str = "main";

/// The name of that timeline, as CapCut names its first one.
pub const MAIN_SEQUENCE_NAME: &str = "Timeline 01";

/// How deep compound clips may nest. A deeper document is refused at the edit
/// boundary, and a hand-edited one renders the levels below as nothing. Eight
/// is far beyond what anyone builds by hand, and every level costs a full
/// canvas render per frame.
pub const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SequenceKind {
    /// A top-level timeline, shown as a tab.
    #[default]
    Timeline,
    /// The contents of a compound clip.
    Compound,
}

/// A sequence that is not the one being edited.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sequence {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub kind: SequenceKind,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
}

impl Sequence {
    /// End of the last segment on any lane.
    pub fn duration(&self) -> Micros {
        tracks_duration(&self.tracks)
    }
}

/// Which sequence `Project::tracks` holds, and how the user got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSequence {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub kind: SequenceKind,
    /// Where this sequence sits in the order of all sequences. See the module
    /// docs.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slot: usize,
    /// The sequences entered on the way here, outermost first: empty on a
    /// timeline, the parent chain inside a compound clip. The breadcrumbs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<Id>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl Default for ActiveSequence {
    fn default() -> Self {
        Self {
            id: MAIN_SEQUENCE_ID.into(),
            name: MAIN_SEQUENCE_NAME.into(),
            kind: SequenceKind::Timeline,
            slot: 0,
            path: Vec::new(),
        }
    }
}

impl ActiveSequence {
    /// Whether this is what a file without the key reads as, so it is not
    /// written and old projects round-trip byte for byte.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

pub(crate) fn tracks_duration(tracks: &[Track]) -> Micros {
    tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .map(|s| s.target_range.end())
        .max()
        .unwrap_or(0)
}

/// One sequence as a list entry: for tabs, the CLI's `timeline list` and MCP.
#[derive(Debug, Clone, Serialize)]
pub struct SequenceInfo {
    pub id: Id,
    pub name: String,
    pub kind: SequenceKind,
    pub active: bool,
    pub duration: Micros,
    pub tracks: usize,
    pub clips: usize,
    /// How many compound clips, anywhere in the project, show this sequence.
    pub uses: usize,
}

/// Every sequence in tab order, the active one included.
pub fn list(project: &Project) -> Vec<SequenceInfo> {
    let mut out: Vec<SequenceInfo> = project
        .materials
        .sequences
        .iter()
        .map(|s| SequenceInfo {
            id: s.id.clone(),
            name: s.name.clone(),
            kind: s.kind,
            active: false,
            duration: s.duration(),
            tracks: s.tracks.len(),
            clips: s.tracks.iter().map(|t| t.segments.len()).sum(),
            uses: uses_of(project, &s.id),
        })
        .collect();
    let active = &project.sequence;
    let at = active.slot.min(out.len());
    out.insert(
        at,
        SequenceInfo {
            id: active.id.clone(),
            name: active.name.clone(),
            kind: active.kind,
            active: true,
            duration: project.duration(),
            tracks: project.tracks.len(),
            clips: project.tracks.iter().map(|t| t.segments.len()).sum(),
            uses: uses_of(project, &active.id),
        },
    );
    out
}

/// The top-level timelines, in tab order. Inside a compound clip the timeline
/// it was entered from counts as the active one.
pub fn timelines(project: &Project) -> Vec<SequenceInfo> {
    let root = root_id(project).to_string();
    list(project)
        .into_iter()
        .filter(|s| s.kind == SequenceKind::Timeline)
        .map(|mut s| {
            s.active = s.id == root;
            s
        })
        .collect()
}

/// The timeline the user is in: the active sequence, or the outermost one on
/// the way into a compound clip.
pub fn root_id(project: &Project) -> &str {
    project
        .sequence
        .path
        .first()
        .unwrap_or(&project.sequence.id)
}

/// `(id, name)` for every level from the root timeline to the active
/// sequence. One entry on a timeline.
pub fn breadcrumbs(project: &Project) -> Vec<(Id, String)> {
    let mut out: Vec<(Id, String)> = project
        .sequence
        .path
        .iter()
        .map(|id| (id.clone(), name_of(project, id).unwrap_or_default()))
        .collect();
    out.push((project.sequence.id.clone(), project.sequence.name.clone()));
    out
}

pub fn name_of(project: &Project, id: &str) -> Option<String> {
    if project.sequence.id == id {
        return Some(project.sequence.name.clone());
    }
    project.materials.sequence(id).map(|s| s.name.clone())
}

/// Whether `id` names a sequence of this project, active or parked.
pub fn exists(project: &Project, id: &str) -> bool {
    project.sequence.id == id || project.materials.sequence(id).is_some()
}

/// The lanes of sequence `id`, wherever it is.
pub fn tracks_of<'a>(project: &'a Project, id: &str) -> Option<&'a [Track]> {
    if project.sequence.id == id {
        return Some(&project.tracks);
    }
    project.materials.sequence(id).map(|s| s.tracks.as_slice())
}

/// The length of sequence `id`: how far into it a compound clip can read.
pub fn duration_of(project: &Project, id: &str) -> Option<Micros> {
    tracks_of(project, id).map(tracks_duration)
}

/// How many segments, in any sequence, use `id` as their material.
pub fn uses_of(project: &Project, id: &str) -> usize {
    all_tracks(project)
        .flat_map(|t| t.segments.iter())
        .filter(|s| s.material_id == id)
        .count()
}

/// Every lane of every sequence.
pub fn all_tracks(project: &Project) -> impl Iterator<Item = &Track> {
    project.tracks.iter().chain(
        project
            .materials
            .sequences
            .iter()
            .flat_map(|s| s.tracks.iter()),
    )
}

/// The sequences directly used by clips on `tracks`.
fn children(project: &Project, tracks: &[Track]) -> BTreeSet<Id> {
    tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter(|s| exists(project, &s.material_id))
        .map(|s| s.material_id.clone())
        .collect()
}

/// Whether sequence `outer` shows sequence `inner`, directly or through
/// further compound clips. `None` when the walk finds a cycle or goes deeper
/// than [`MAX_DEPTH`]: the document is broken and the answer is "assume yes".
fn contains(project: &Project, outer: &str, inner: &str, depth: usize) -> Option<bool> {
    if depth > MAX_DEPTH {
        return None;
    }
    let tracks = tracks_of(project, outer)?;
    for child in children(project, tracks) {
        if child == inner {
            return Some(true);
        }
        if contains(project, &child, inner, depth + 1)? {
            return Some(true);
        }
    }
    Some(false)
}

/// How many levels of compound clips sit inside sequence `id`: 0 for a
/// sequence without any. `None` for a cycle or a nest past [`MAX_DEPTH`].
pub fn depth_of(project: &Project, id: &str) -> Option<usize> {
    fn walk(project: &Project, id: &str, seen: &mut Vec<Id>) -> Option<usize> {
        if seen.iter().any(|s| s == id) || seen.len() > MAX_DEPTH {
            return None;
        }
        let tracks = tracks_of(project, id)?;
        seen.push(id.to_string());
        let mut deepest = 0;
        for child in children(project, tracks) {
            deepest = deepest.max(walk(project, &child, seen)? + 1);
        }
        seen.pop();
        Some(deepest)
    }
    walk(project, id, &mut Vec::new())
}

/// Refuse a clip of `material_id` on the active sequence when it would make
/// a sequence contain itself, or nest deeper than [`MAX_DEPTH`].
///
/// Called by `EditCommand::InsertSegment`, which is how every clip enters a
/// lane — a paste of a compound clip into its own contents is the ordinary way
/// to try this, and it has to fail there rather than hang the renderer.
pub fn check_insert(project: &Project, material_id: &str) -> Result<(), String> {
    if !exists(project, material_id) {
        return Ok(());
    }
    let here = &project.sequence.id;
    if material_id == here
        || project.sequence.path.iter().any(|p| p == material_id)
        || contains(project, material_id, here, 0) != Some(false)
    {
        return Err("a compound clip cannot contain itself".into());
    }
    let inner = depth_of(project, material_id).ok_or("that compound clip is nested in a loop")?;
    // The levels above the active sequence count too: the outermost timeline
    // renders all of them.
    if project.sequence.path.len() + inner + 1 > MAX_DEPTH {
        return Err(format!(
            "compound clips can nest at most {MAX_DEPTH} levels deep"
        ));
    }
    Ok(())
}

/// The project as it would be with `id` active: what renders, mixes and
/// exports that sequence. `None` when there is no such sequence.
///
/// A clone. Rendering a compound clip uses [`nested`] instead, which only
/// swaps the lanes.
pub fn view(project: &Project, id: &str) -> Option<Project> {
    let mut view = project.clone();
    if view.sequence.id != id {
        edit::activate(&mut view, id, Vec::new()).ok()?;
    } else {
        view.sequence.path.clear();
    }
    Some(view)
}

/// The project as the export sees it: the root timeline, even while a
/// compound clip is open in the editor. Borrowed when nothing is open.
pub fn export_root(project: Project) -> Project {
    if project.sequence.path.is_empty() {
        return project;
    }
    let root = root_id(&project).to_string();
    view(&project, &root).unwrap_or(project)
}

/// The document a compound clip of sequence `id` renders: the same pool and
/// canvas, that sequence's lanes. `None` for an unknown sequence.
pub fn nested(project: &Project, id: &str) -> Option<Project> {
    let tracks = tracks_of(project, id)?.to_vec();
    Some(Project {
        id: project.id.clone(),
        schema_version: project.schema_version,
        name: project.name.clone(),
        created_at: project.created_at,
        updated_at: project.updated_at,
        canvas: project.canvas,
        fps: project.fps,
        canvas_chosen: project.canvas_chosen,
        materials: project.materials.clone(),
        tracks,
        markers: Vec::new(),
        sequence: ActiveSequence {
            id: id.to_string(),
            name: name_of(project, id).unwrap_or_default(),
            kind: SequenceKind::Compound,
            slot: 0,
            path: Vec::new(),
        },
    })
}

/// Structural problems with the sequences themselves: duplicate ids, a
/// sequence that contains itself, a nest past [`MAX_DEPTH`], and the ordinary
/// lane checks on every parked sequence. Called from `Project::validate`.
pub fn validate(project: &Project) -> Vec<crate::modules::project::ValidationIssue> {
    use crate::modules::project::{Severity, ValidationIssue};
    let mut issues = Vec::new();
    let mut error = |message: String, subject: Option<Id>| {
        issues.push(ValidationIssue {
            severity: Severity::Error,
            message,
            subject_id: subject,
        })
    };
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    ids.insert(project.sequence.id.as_str());
    for s in &project.materials.sequences {
        if !ids.insert(s.id.as_str()) {
            error(
                format!("two sequences share the id {}", s.id),
                Some(s.id.clone()),
            );
        }
        if project
            .materials
            .kind_of(&s.id)
            .is_some_and(|k| k != crate::modules::project::MaterialKind::Sequence)
        {
            error(
                format!("the sequence {} shares its id with a material", s.id),
                Some(s.id.clone()),
            );
        }
    }
    for id in project.sequence.path.iter() {
        if project.materials.sequence(id).is_none() {
            error(
                format!("the way into this compound clip passes through {id}, which is gone"),
                None,
            );
        }
    }
    for s in std::iter::once(project.sequence.id.as_str())
        .chain(project.materials.sequences.iter().map(|s| s.id.as_str()))
    {
        if depth_of(project, s).is_none() {
            error(
                format!(
                    "the sequence \"{}\" contains itself or nests deeper than {MAX_DEPTH} levels",
                    name_of(project, s).unwrap_or_default()
                ),
                Some(s.to_string()),
            );
        }
    }
    // Every parked sequence's lanes get the checks the active one gets. Only
    // the issues about its own lanes and clips are kept: the pool-level ones
    // are the active document's and were reported already.
    for s in &project.materials.sequences {
        let Some(mut view) = nested(project, &s.id) else {
            continue;
        };
        // The pool's own sequences are checked once, here, not per view.
        view.materials.sequences.clear();
        let own: BTreeSet<&str> = s
            .tracks
            .iter()
            .flat_map(|t| {
                std::iter::once(t.id.as_str()).chain(t.segments.iter().map(|g| g.id.as_str()))
            })
            .collect();
        for mut issue in view.validate() {
            if issue
                .subject_id
                .as_deref()
                .is_some_and(|id| own.contains(id))
            {
                // A sequence clip inside a parked sequence names a material
                // the stripped view no longer has; that warning is noise.
                if issue
                    .message
                    .starts_with("segment references unknown material")
                    && issue
                        .message
                        .rsplit(' ')
                        .next()
                        .is_some_and(|m| exists(project, m))
                {
                    continue;
                }
                issue.message = format!("in \"{}\": {}", s.name, issue.message);
                issues.push(issue);
            }
        }
    }
    issues
}

/// [`nested`] for the compositor, which has the pool and the canvas size but
/// not the project. Only parked sequences can be reached this way, which is
/// all a compound clip on the active sequence can show. The canvas is clear
/// rather than the project's background, so the compound clip is transparent
/// wherever its own lanes are empty.
pub fn nested_in(
    materials: &crate::modules::project::MaterialPool,
    canvas: (u32, u32),
    id: &str,
) -> Option<Project> {
    let sequence = materials.sequence(id)?;
    let mut view = Project::new(
        sequence.name.clone(),
        crate::modules::project::CanvasConfig {
            width: canvas.0,
            height: canvas.1,
            background: [0.0, 0.0, 0.0, 0.0],
        },
        30.0,
    );
    view.materials = materials.clone();
    view.tracks = sequence.tracks.clone();
    view.sequence = ActiveSequence {
        id: sequence.id.clone(),
        name: sequence.name.clone(),
        kind: sequence.kind,
        slot: 0,
        path: Vec::new(),
    };
    Some(view)
}

/// What a compound clip's picture shows at nested time `time` of sequence
/// `id`, as a file and a time in it: the topmost visible video or still at
/// that instant, followed into compound clips inside. What the timeline draws
/// a compound clip's thumbnails from, so they come out of the same caches as
/// every other clip's. `None` where nothing is there to show.
pub fn picture_at(project: &Project, id: &str, time: Micros) -> Option<(Id, Micros)> {
    fn walk(project: &Project, id: &str, time: Micros, depth: usize) -> Option<(Id, Micros)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let pool = &project.materials;
        for track in tracks_of(project, id)?.iter().rev() {
            if track.hidden {
                continue;
            }
            let Some(segment) = track.segment_at(time) else {
                continue;
            };
            let Some(source) = pool.time_map(segment).source_time_at(time) else {
                continue;
            };
            if pool.video(&segment.material_id).is_some()
                || pool.image(&segment.material_id).is_some()
            {
                return Some((segment.material_id.clone(), source));
            }
            if exists(project, &segment.material_id) {
                if let Some(found) = walk(project, &segment.material_id, source, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(project, id, time, 0)
}
