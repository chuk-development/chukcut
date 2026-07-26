//! Undo/redo stacks.
//!
//! Straight command-pattern history: applied commands go on the undo stack,
//! undoing moves them to the redo stack, and any new edit clears the redo
//! stack. The history stores commands, not document snapshots — a snapshot of
//! a project with a hundred clips is far more expensive than the delta, and
//! snapshots make it impossible to describe *what* an undo will reverse.

use super::ops::EditCommand;
use crate::modules::project::Project;

/// How many edits are kept. Beyond this the oldest are dropped.
const MAX_DEPTH: usize = 500;

#[derive(Default)]
pub struct History {
    undo_stack: Vec<EditCommand>,
    redo_stack: Vec<EditCommand>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a command and record it. On failure nothing is recorded, so a
    /// rejected edit never shows up in the undo menu.
    pub fn apply(&mut self, project: &mut Project, command: EditCommand) -> Result<(), String> {
        // A structural edit can leave a transition describing a cut that no
        // longer exists, and the primitive that broke it cannot put it back on
        // undo. So the removals it implies are folded in here, before the
        // command is recorded — the user gets one undo step and the transition
        // comes back with it. See `ops::detach_broken_transitions`.
        let command = super::ops::detach_broken_transitions(project, command);
        command.apply(project)?;
        self.redo_stack.clear();
        self.undo_stack.push(command);
        if self.undo_stack.len() > MAX_DEPTH {
            self.undo_stack.remove(0);
        }
        Ok(())
    }

    pub fn undo(&mut self, project: &mut Project) -> Result<Option<String>, String> {
        let Some(command) = self.undo_stack.pop() else {
            return Ok(None);
        };
        let label = command.label();
        if let Err(e) = command.invert().apply(project) {
            // Undo failing means the document drifted from what the command
            // expected. Put it back rather than silently losing history.
            self.undo_stack.push(command);
            return Err(e);
        }
        self.redo_stack.push(command);
        Ok(Some(label))
    }

    pub fn redo(&mut self, project: &mut Project) -> Result<Option<String>, String> {
        let Some(command) = self.redo_stack.pop() else {
            return Ok(None);
        };
        let label = command.label();
        if let Err(e) = command.apply(project) {
            self.redo_stack.push(command);
            return Err(e);
        }
        self.undo_stack.push(command);
        Ok(Some(label))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Label of the edit that undo would reverse, for the menu item.
    pub fn undo_label(&self) -> Option<String> {
        self.undo_stack.last().map(EditCommand::label)
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo_stack.last().map(EditCommand::label)
    }

    /// Drop all history. Called when a different project is opened.
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CanvasConfig, Track, TrackKind};

    #[test]
    fn undo_then_redo_round_trips() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut history = History::new();
        let track = Track::new(TrackKind::Video, "V1");

        history
            .apply(
                &mut project,
                EditCommand::AddTrack {
                    track: track.clone(),
                    index: 0,
                },
            )
            .unwrap();
        assert_eq!(project.tracks.len(), 1);

        history.undo(&mut project).unwrap();
        assert_eq!(project.tracks.len(), 0);

        history.redo(&mut project).unwrap();
        assert_eq!(project.tracks.len(), 1);
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut history = History::new();

        history
            .apply(
                &mut project,
                EditCommand::AddTrack {
                    track: Track::new(TrackKind::Video, "V1"),
                    index: 0,
                },
            )
            .unwrap();
        history.undo(&mut project).unwrap();
        assert!(history.can_redo());

        history
            .apply(
                &mut project,
                EditCommand::AddTrack {
                    track: Track::new(TrackKind::Audio, "A1"),
                    index: 0,
                },
            )
            .unwrap();
        assert!(!history.can_redo());
    }
}
