//! The inspector's clip adjustments: crop and per-clip colour.
//!
//! The timeline module owns the edit primitives; this module owns the two
//! panel gestures that have no primitive of their own — setting a segment's
//! crop rectangle and its colour adjustment — and expresses each as a
//! composition of existing [`crate::modules::timeline::ops::EditCommand`]
//! variants, so both land on the ordinary undo stack. `edit.rs` builds the
//! commands and is pure; `commands.rs` is the `#[tauri::command]` surface.

pub mod commands;
pub mod edit;
