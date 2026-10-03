//! Errors and exit codes.
//!
//! The engine's command layer answers `Result<T, String>` with user-facing
//! prose. The CLI keeps that prose and adds a *kind*, because a script needs
//! to tell "the file is not a project" from "the edit was refused" without
//! parsing English.

use std::fmt;

/// Why an invocation failed. Each kind is one exit code; `docs/cli.md`
/// lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The engine refused the operation: an unknown clip, a collision, a
    /// value out of range. Nothing was saved.
    Refused,
    /// The arguments do not describe an operation (clap reports its own
    /// usage errors with the same code).
    Usage,
    /// The project file could not be read, written or created.
    Project,
    /// The edit would leave the document inconsistent; nothing was saved.
    Invalid,
    /// An export or a frame render failed.
    Render,
}

impl ErrorKind {
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::Refused => 1,
            ErrorKind::Usage => 2,
            ErrorKind::Project => 3,
            ErrorKind::Invalid => 4,
            ErrorKind::Render => 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Refused, message)
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    pub fn project(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Project, message)
    }

    pub fn render(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Render, message)
    }

    /// Prefix the message with where it happened, keeping the kind.
    pub fn context(mut self, prefix: impl fmt::Display) -> Self {
        self.message = format!("{prefix}: {}", self.message);
        self
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// The engine's `String` errors are refusals unless a call site says
/// otherwise.
impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::refused(message)
    }
}

impl From<&str> for CliError {
    fn from(message: &str) -> Self {
        Self::refused(message)
    }
}

pub type CliResult<T> = Result<T, CliError>;
