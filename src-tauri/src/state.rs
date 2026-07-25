//! Shared application state.
//!
//! One `AppState` is managed by Tauri and handed to every command. It holds the
//! open project behind a lock; commands take the lock, mutate, and drop it
//! before doing anything slow. Long-running work (decode, render, export) must
//! never hold the project lock — it clones the slice of the document it needs.

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

use crate::modules::project::Project;
use crate::modules::timeline::History;

pub struct AppState {
    /// The open document. `None` before the first project is created or opened.
    pub project: RwLock<Option<Project>>,
    /// Where the open project lives on disk, if it has been saved.
    pub project_path: RwLock<Option<PathBuf>>,
    /// Undo/redo stacks for the open project.
    pub history: RwLock<History>,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            project: RwLock::new(None),
            project_path: RwLock::new(None),
            history: RwLock::new(History::new()),
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
            history: RwLock::new(History::new()),
        }
    }
}
