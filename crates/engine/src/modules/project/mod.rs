//! Project document model and persistence.
//!
//! Owns the on-disk format: what a `.chukcut` project *is*. The shape is
//! informed by CapCut's `draft_content.json` (a mature, battle-tested layout
//! for exactly this problem) but it is our own format — we do not import or
//! export theirs.
//!
//! The central idea borrowed from that format: **segments reference materials
//! by id, they do not embed them.** A track holds segments; a segment holds a
//! time range plus a `material_id`; materials live in one flat pool grouped by
//! kind. That indirection is what makes "replace this clip everywhere",
//! "relink missing media", and undo/redo cheap.
//!
//! See `docs/architecture/project-format.md`.

pub mod autosave;
pub mod commands;
pub mod configure;
pub mod document;
pub mod grade;
pub mod migrate;
pub mod recovery;

pub use configure::{ConfigureCommand, ProjectConfig};
pub use document::*;
