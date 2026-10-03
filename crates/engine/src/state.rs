//! Shared application state.
//!
//! One `AppState` is managed by Tauri and handed to every command. It holds the
//! open project behind a lock; commands take the lock, mutate, and drop it
//! before doing anything slow. Long-running work (decode, render, export) must
//! never hold the project lock — it clones the slice of the document it needs.

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

use crate::modules::project::{ConfigureCommand, Project};
use crate::modules::timeline::ops::{self, EditCommand};
use crate::modules::tracking::TrackingCommand;

pub struct AppState {
    /// The open document. `None` before the first project is created or opened.
    pub project: RwLock<Option<Project>>,
    /// Where the open project lives on disk, if it has been saved.
    pub project_path: RwLock<Option<PathBuf>>,
    /// Undo/redo stacks for the open project.
    pub history: RwLock<DocumentHistory>,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        // Filler-word cutting reads the captions' words; see `captions::words`.
        crate::modules::captions::words::register();
        Arc::new(Self {
            project: RwLock::new(None),
            project_path: RwLock::new(None),
            history: RwLock::new(DocumentHistory::new()),
        })
    }

    /// Run `f` against the open project, or return a "no project open" error.
    pub fn with_project<T>(&self, f: impl FnOnce(&Project) -> T) -> Result<T, String> {
        let guard = self.project.read();
        let project = guard.as_ref().ok_or("no project is open")?;
        Ok(f(project))
    }

    pub fn with_project_mut<T>(&self, f: impl FnOnce(&mut Project) -> T) -> Result<T, String> {
        let mut guard = self.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        Ok(f(project))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            project: RwLock::new(None),
            project_path: RwLock::new(None),
            history: RwLock::new(DocumentHistory::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// The one undo stack
// ---------------------------------------------------------------------------

/// How many edits are kept. Beyond this the oldest are dropped. The same depth
/// `timeline::history::History` uses, because a user cannot tell the two kinds
/// of edit apart and the stack must not behave as if there were two.
const MAX_DEPTH: usize = 500;

/// Any invertible change to the open document.
///
/// Two kinds, one stack. Timeline edits are `timeline::ops::EditCommand` and
/// stay entirely that module's business; the project-settings edit
/// (`project::ConfigureCommand`) writes fields — name, canvas, fps — that no
/// `EditCommand` variant can reach, and growing that enum from here would mean
/// editing a module another concern owns. What the user needs is that Ctrl+Z
/// walks *all* of their edits in order, whichever kind each one was, and this
/// enum is exactly that requirement as a type.
///
/// Tracking edits are the third kind for the same reason: they write pool
/// categories no `EditCommand` reaches (`modules::tracking::edit`).
enum DocumentCommand {
    Edit(EditCommand),
    Configure(ConfigureCommand),
    Tracking(TrackingCommand),
    /// Several of the above as one undo step, applied in order — a project
    /// setting and the timeline edit that goes with it, as auto reframe's
    /// "switch to 9:16 and fill it" (`modules::analysis`).
    Sequence {
        label: String,
        commands: Vec<DocumentCommand>,
    },
}

impl DocumentCommand {
    fn apply(&self, project: &mut Project) -> Result<(), String> {
        match self {
            Self::Edit(command) => command.apply(project),
            Self::Configure(command) => command.apply(project),
            Self::Tracking(command) => command.apply(project),
            Self::Sequence { commands, .. } => {
                for (done, command) in commands.iter().enumerate() {
                    if let Err(error) = command.apply(project) {
                        // All or nothing: take back the parts that landed.
                        for applied in commands[..done].iter().rev() {
                            let _ = applied.invert().apply(project);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }

    fn invert(&self) -> Self {
        match self {
            Self::Edit(command) => Self::Edit(command.invert()),
            Self::Configure(command) => Self::Configure(command.invert()),
            Self::Tracking(command) => Self::Tracking(command.invert()),
            Self::Sequence { label, commands } => Self::Sequence {
                label: label.clone(),
                commands: commands.iter().rev().map(Self::invert).collect(),
            },
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Edit(command) => command.label(),
            Self::Configure(command) => command.label(),
            Self::Tracking(command) => command.label(),
            Self::Sequence { label, .. } => label.clone(),
        }
    }
}

/// Undo/redo for the whole document: the timeline history with one more kind
/// of command in the same stack.
///
/// The semantics for timeline edits are `timeline::history::History`'s,
/// reproduced deliberately — including the two pre-apply expansions
/// (`mirror_linked_edits`, then `detach_broken_transitions`) that History runs
/// and documents. If History's `apply` ever grows a third expansion, this one
/// needs it too; that coupling is the price of not touching the timeline
/// module, and it is written down here so the drift is a known place to check
/// rather than a surprise.
#[derive(Default)]
pub struct DocumentHistory {
    undo_stack: Vec<DocumentCommand>,
    redo_stack: Vec<DocumentCommand>,
}

impl DocumentHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a timeline edit and record it. On failure nothing is recorded, so
    /// a rejected edit never shows up in the undo menu.
    pub fn apply(&mut self, project: &mut Project, command: EditCommand) -> Result<(), String> {
        // The same two expansions, in the same order, as `History::apply`, and
        // for the same reasons: link partners come along exactly once, and
        // transitions get to see the whole expanded edit before deciding
        // whether the cut they describe still exists.
        let command = ops::mirror_linked_edits(project, command);
        let command = ops::detach_broken_transitions(project, command);
        command.apply(project)?;
        self.record(DocumentCommand::Edit(command));
        Ok(())
    }

    /// Apply a project-settings edit and record it on the same stack.
    pub fn apply_configure(
        &mut self,
        project: &mut Project,
        command: ConfigureCommand,
    ) -> Result<(), String> {
        command.apply(project)?;
        self.record(DocumentCommand::Configure(command));
        Ok(())
    }

    /// Apply a tracking edit and record it on the same stack. All or nothing:
    /// a refused tracking edit leaves the document as it was.
    pub fn apply_tracking(
        &mut self,
        project: &mut Project,
        command: TrackingCommand,
    ) -> Result<(), String> {
        command.apply(project)?;
        self.record(DocumentCommand::Tracking(command));
        Ok(())
    }

    /// Apply a project-settings edit and then a timeline edit as **one** undo
    /// step labelled `label`. All or nothing.
    pub fn apply_configure_and_edit(
        &mut self,
        project: &mut Project,
        configure: ConfigureCommand,
        edit: EditCommand,
        label: String,
    ) -> Result<(), String> {
        let edit = ops::mirror_linked_edits(project, edit);
        let edit = ops::detach_broken_transitions(project, edit);
        let command = DocumentCommand::Sequence {
            label,
            commands: vec![
                DocumentCommand::Configure(configure),
                DocumentCommand::Edit(edit),
            ],
        };
        command.apply(project)?;
        self.record(command);
        Ok(())
    }

    fn record(&mut self, command: DocumentCommand) {
        self.redo_stack.clear();
        self.undo_stack.push(command);
        if self.undo_stack.len() > MAX_DEPTH {
            self.undo_stack.remove(0);
        }
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
        self.undo_stack.last().map(DocumentCommand::label)
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo_stack.last().map(DocumentCommand::label)
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
    use crate::modules::project::{CanvasConfig, ProjectConfig, Track, TrackKind};

    fn project() -> Project {
        Project::new("t", CanvasConfig::default(), 30.0)
    }

    fn add_track() -> EditCommand {
        EditCommand::AddTrack {
            track: Track::new(TrackKind::Video, "V1"),
            index: 0,
        }
    }

    fn resize(project: &Project, width: u32, height: u32) -> ConfigureCommand {
        ConfigureCommand::new(
            project,
            ProjectConfig {
                width,
                height,
                ..ProjectConfig::of(project)
            },
        )
    }

    #[test]
    fn timeline_edits_still_round_trip() {
        let mut p = project();
        let mut history = DocumentHistory::new();

        history.apply(&mut p, add_track()).unwrap();
        assert_eq!(p.tracks.len(), 1);

        history.undo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 0);

        history.redo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 1);
    }

    #[test]
    fn a_settings_edit_shares_the_stack_with_timeline_edits() {
        // The user's model is one list of things they did. Edit, configure,
        // edit — three Ctrl+Z must peel them off in exactly reverse order.
        let mut p = project();
        let mut history = DocumentHistory::new();

        history.apply(&mut p, add_track()).unwrap();
        let resized = resize(&p, 1920, 1080);
        history.apply_configure(&mut p, resized).unwrap();
        history
            .apply(
                &mut p,
                EditCommand::AddTrack {
                    track: Track::new(TrackKind::Audio, "A1"),
                    index: 1,
                },
            )
            .unwrap();

        assert_eq!(history.undo_label().as_deref(), Some("Add track"));
        history.undo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!((p.canvas.width, p.canvas.height), (1920, 1080));

        assert_eq!(history.undo_label().as_deref(), Some("Project settings"));
        history.undo(&mut p).unwrap();
        assert_eq!((p.canvas.width, p.canvas.height), (1080, 1920));
        assert_eq!(p.tracks.len(), 1);

        history.undo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 0);
        assert!(!history.can_undo());

        // And forward again, in order.
        history.redo(&mut p).unwrap();
        history.redo(&mut p).unwrap();
        assert_eq!((p.canvas.width, p.canvas.height), (1920, 1080));
        history.redo(&mut p).unwrap();
        assert_eq!(p.tracks.len(), 2);
    }

    #[test]
    fn a_new_edit_of_either_kind_clears_the_redo_stack() {
        let mut p = project();
        let mut history = DocumentHistory::new();

        history.apply(&mut p, add_track()).unwrap();
        history.undo(&mut p).unwrap();
        assert!(history.can_redo());

        let resized = resize(&p, 1920, 1080);
        history.apply_configure(&mut p, resized).unwrap();
        assert!(!history.can_redo());
    }

    #[test]
    fn a_refused_settings_edit_is_not_recorded() {
        let mut p = project();
        let mut history = DocumentHistory::new();

        let odd = resize(&p, 1235, 695);
        assert!(history.apply_configure(&mut p, odd).is_err());
        assert!(!history.can_undo());
        assert_eq!((p.canvas.width, p.canvas.height), (1080, 1920));
    }
}
