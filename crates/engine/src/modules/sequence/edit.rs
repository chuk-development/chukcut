//! The invertible edits on sequences, carried by `EditCommand::Sequence`.
//!
//! Four primitives. Everything the user does with sequences — a new timeline,
//! a compound clip, entering one, flattening one — is a `Composite` of these
//! and the ordinary segment and lane commands, built in [`super::build`] and
//! [`super::commands`].

use serde::{Deserialize, Serialize};

use super::{ActiveSequence, Sequence, SequenceKind};
use crate::modules::project::{Id, Project, TransitionMaterial};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SequenceEdit {
    /// Park a new sequence in the pool at `index`.
    ///
    /// `transitions` and `links` are what a duplicated timeline brings with it:
    /// its clips get their own copies of the transitions they carry, because
    /// a transition is edited in place and a shared one would change in both
    /// timelines at once, and their own link groups, which have to be
    /// registered in `MaterialPool::links` to count as links at all. Both are
    /// empty for a compound clip, whose clips move rather than copy.
    Add {
        sequence: Sequence,
        index: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        transitions: Vec<TransitionMaterial>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
    },
    /// The inverse of `Add`. Refused while any clip still shows the sequence.
    Remove {
        sequence: Sequence,
        index: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        transitions: Vec<TransitionMaterial>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
    },
    /// Rename a sequence, active or parked.
    Rename {
        id: Id,
        before: String,
        after: String,
    },
    /// Make `to` the active sequence and park `from`. `from_path` and
    /// `to_path` are the breadcrumbs on each side, so entering a compound clip
    /// and leaving it are the same primitive.
    Activate {
        from: Id,
        from_path: Vec<Id>,
        to: Id,
        to_path: Vec<Id>,
    },
}

impl SequenceEdit {
    pub fn label(&self) -> String {
        match self {
            SequenceEdit::Add { sequence, .. } => match sequence.kind {
                SequenceKind::Timeline => "New timeline".into(),
                SequenceKind::Compound => "Create compound clip".into(),
            },
            SequenceEdit::Remove { sequence, .. } => match sequence.kind {
                SequenceKind::Timeline => "Delete timeline".into(),
                SequenceKind::Compound => "Remove compound clip".into(),
            },
            SequenceEdit::Rename { .. } => "Rename timeline".into(),
            SequenceEdit::Activate {
                to_path, from_path, ..
            } => {
                if to_path.len() > from_path.len() {
                    "Open compound clip".into()
                } else if to_path.len() < from_path.len() {
                    "Close compound clip".into()
                } else {
                    "Switch timeline".into()
                }
            }
        }
    }

    pub fn apply(&self, project: &mut Project) -> Result<(), String> {
        match self {
            SequenceEdit::Add {
                sequence,
                index,
                transitions,
                links,
            } => {
                if super::exists(project, &sequence.id)
                    || project.materials.kind_of(&sequence.id).is_some()
                {
                    return Err(format!(
                        "a sequence with the id {} already exists",
                        sequence.id
                    ));
                }
                if *index > project.materials.sequences.len() {
                    return Err("there is no such place for a timeline".into());
                }
                for t in transitions {
                    if project.materials.transition(&t.id).is_some() {
                        return Err(format!("the transition {} already exists", t.id));
                    }
                }
                if links.iter().any(|l| project.materials.links.contains(l)) {
                    return Err("a link group of the copy already exists".into());
                }
                // A sequence that shows itself would recurse for ever in the
                // renderer. The builders cannot make one; a hand-written
                // command can.
                if sequence
                    .tracks
                    .iter()
                    .flat_map(|t| t.segments.iter())
                    .any(|s| s.material_id == sequence.id)
                {
                    return Err("a compound clip cannot contain itself".into());
                }
                project.materials.sequences.insert(*index, sequence.clone());
                project
                    .materials
                    .transitions
                    .extend(transitions.iter().cloned());
                project.materials.links.extend(links.iter().cloned());
                Ok(())
            }
            SequenceEdit::Remove {
                sequence,
                index,
                transitions,
                links,
            } => {
                let found = project
                    .materials
                    .sequences
                    .get(*index)
                    .ok_or("that timeline is no longer in the project")?;
                if found.id != sequence.id {
                    return Err(format!(
                        "the timeline \"{}\" is no longer where this edit expected it",
                        sequence.name
                    ));
                }
                if super::uses_of(project, &sequence.id) > 0 {
                    return Err(format!(
                        "\"{}\" is still used by a compound clip",
                        sequence.name
                    ));
                }
                if project.sequence.path.contains(&sequence.id) {
                    return Err("a timeline that is open cannot be deleted".into());
                }
                for t in transitions {
                    let at = project
                        .materials
                        .transitions
                        .iter()
                        .rposition(|m| m.id == t.id)
                        .ok_or("a transition of the timeline is gone")?;
                    project.materials.transitions.remove(at);
                }
                for l in links {
                    project.materials.links.remove(l);
                }
                project.materials.sequences.remove(*index);
                Ok(())
            }
            SequenceEdit::Rename { id, before, after } => {
                let after = after.trim();
                if after.is_empty() {
                    return Err("a timeline needs a name".into());
                }
                let current = if project.sequence.id == *id {
                    &mut project.sequence.name
                } else {
                    &mut project
                        .materials
                        .sequences
                        .iter_mut()
                        .find(|s| s.id == *id)
                        .ok_or("that timeline is no longer in the project")?
                        .name
                };
                if current != before {
                    return Err("the timeline was renamed underneath this edit".into());
                }
                *current = after.to_string();
                Ok(())
            }
            SequenceEdit::Activate {
                from,
                from_path,
                to,
                to_path,
            } => {
                if project.sequence.id != *from || project.sequence.path != *from_path {
                    return Err("a different timeline is open than this edit expected".into());
                }
                activate(project, to, to_path.clone())
            }
        }
    }

    pub fn invert(&self) -> SequenceEdit {
        match self {
            SequenceEdit::Add {
                sequence,
                index,
                transitions,
                links,
            } => SequenceEdit::Remove {
                sequence: sequence.clone(),
                index: *index,
                transitions: transitions.clone(),
                links: links.clone(),
            },
            SequenceEdit::Remove {
                sequence,
                index,
                transitions,
                links,
            } => SequenceEdit::Add {
                sequence: sequence.clone(),
                index: *index,
                transitions: transitions.clone(),
                links: links.clone(),
            },
            SequenceEdit::Rename { id, before, after } => SequenceEdit::Rename {
                id: id.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            SequenceEdit::Activate {
                from,
                from_path,
                to,
                to_path,
            } => SequenceEdit::Activate {
                from: to.clone(),
                from_path: to_path.clone(),
                to: from.clone(),
                to_path: from_path.clone(),
            },
        }
    }
}

/// Swap sequence `to` into `Project::tracks` and park the active one.
///
/// The order of all sequences is the pool with the active one inserted at its
/// slot. `to` leaves that list and the old active sequence takes the place it
/// had in it, so the list — and with it the pool — is the same afterwards as
/// before, and switching back restores the pool exactly.
pub(crate) fn activate(project: &mut Project, to: &str, path: Vec<Id>) -> Result<(), String> {
    for p in &path {
        if !super::exists(project, p) {
            return Err(format!("there is no sequence {p} to pass through"));
        }
    }
    if project.sequence.id == to {
        project.sequence.path = path;
        return Ok(());
    }
    let pool = &mut project.materials.sequences;
    let at = pool
        .iter()
        .position(|s| s.id == to)
        .ok_or_else(|| format!("there is no timeline {to}"))?;
    let slot = project.sequence.slot.min(pool.len());
    // `to`'s place in the order of all sequences, and the old active one's
    // place once `to` has left it.
    let virtual_index = if at < slot { at } else { at + 1 };
    let parked_at = if slot < virtual_index { slot } else { slot - 1 };

    let target = pool.remove(at);
    let old = std::mem::replace(
        &mut project.sequence,
        ActiveSequence {
            id: target.id.clone(),
            name: target.name.clone(),
            kind: target.kind,
            slot: virtual_index,
            path,
        },
    );
    let parked = Sequence {
        id: old.id,
        name: old.name,
        kind: old.kind,
        tracks: std::mem::replace(&mut project.tracks, target.tracks),
        markers: std::mem::replace(&mut project.markers, target.markers),
    };
    project.materials.sequences.insert(parked_at, parked);
    Ok(())
}
