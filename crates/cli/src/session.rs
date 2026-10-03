//! One open project: the engine's `AppState`, the file it came from, and
//! whether anything changed.
//!
//! A session is what one CLI invocation, one `batch` or one MCP project
//! holds. It opens and saves through the engine's own project commands, so a
//! file the CLI wrote is a file the app wrote — same migration on the way in,
//! same temp-file-and-rename on the way out. Before saving it validates, and a
//! document with errors is never written: the caller's file keeps its last
//! good state.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chukcut_engine::modules::project::{commands as project_commands, Project, Severity};
use chukcut_engine::state::AppState;

use crate::error::{CliError, CliResult, ErrorKind};

pub struct Session {
    pub state: Arc<AppState>,
    pub path: PathBuf,
    /// Something changed since the last save.
    pub dirty: bool,
    /// The file's modification time when this session last read or wrote it,
    /// so a long-lived session (the MCP server) notices an edit made by
    /// someone else and reloads instead of overwriting it.
    pub disk_mtime: Option<SystemTime>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// An absolute path without requiring the file to exist.
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

impl Session {
    /// Open an existing project file.
    pub fn open(path: &Path) -> CliResult<Self> {
        let path = absolute(path);
        if !path.is_file() {
            return Err(CliError::project(format!(
                "there is no project at {} (create one with `chukcut-cli new`)",
                path.display()
            )));
        }
        let state = AppState::new();
        project_commands::project_open(&state, path.to_string_lossy().into_owned())
            .map_err(CliError::project)?;
        Ok(Self {
            state,
            disk_mtime: mtime(&path),
            path,
            dirty: false,
        })
    }

    /// Start a new, unsaved project that will be written to `path`.
    pub fn create(path: &Path, name: String, width: u32, height: u32, fps: f64) -> CliResult<Self> {
        let path = absolute(path);
        let state = AppState::new();
        project_commands::project_new(&state, name, width, height, fps, false)
            .map_err(CliError::refused)?;
        Ok(Self {
            state,
            path,
            dirty: true,
            disk_mtime: None,
        })
    }

    /// A copy of the open document.
    pub fn project(&self) -> Project {
        self.state
            .project
            .read()
            .clone()
            .expect("a session always has a project")
    }

    /// Run `f` against the open document without cloning it.
    pub fn with<T>(&self, f: impl FnOnce(&Project) -> T) -> T {
        let guard = self.state.project.read();
        f(guard.as_ref().expect("a session always has a project"))
    }

    pub fn fps(&self) -> f64 {
        self.with(|p| p.fps)
    }

    /// Whether the file changed on disk since this session read or wrote it.
    pub fn changed_on_disk(&self) -> bool {
        self.disk_mtime.is_some() && mtime(&self.path) != self.disk_mtime
    }

    /// The validation errors (not warnings) of the open document, as prose.
    pub fn errors(&self) -> Vec<String> {
        self.with(|p| p.validate())
            .into_iter()
            .filter(|issue| issue.severity == Severity::Error)
            .map(|issue| issue.message)
            .collect()
    }

    /// Validate, then write the project to its file atomically.
    pub fn save(&mut self) -> CliResult<()> {
        let errors = self.errors();
        if !errors.is_empty() {
            return Err(CliError::new(
                ErrorKind::Invalid,
                format!(
                    "the edit left the project inconsistent, so {} was not written: {}",
                    self.path.display(),
                    errors.join("; ")
                ),
            ));
        }
        project_commands::project_save(&self.state, Some(self.path.to_string_lossy().into_owned()))
            .map_err(|e| CliError::project(format!("cannot write {}: {e}", self.path.display())))?;
        self.dirty = false;
        self.disk_mtime = mtime(&self.path);
        Ok(())
    }
}
