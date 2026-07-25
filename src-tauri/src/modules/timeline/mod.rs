//! Timeline editing operations and undo/redo.
//!
//! Every mutation of the document goes through an [`EditCommand`]. The command
//! carries enough information to apply itself *and* to invert itself, which is
//! what makes undo exact rather than approximate. Nothing else is allowed to
//! mutate a `Project` in place — if a feature needs a new kind of edit, it adds
//! a command variant here.
//!
//! See `docs/architecture/timeline-editing.md`.

pub mod commands;
pub mod history;
pub mod ops;

pub use history::History;
