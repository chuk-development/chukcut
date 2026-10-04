//! Project templates, CapCut-style: a project whose picture clips are slots
//! the user's media fills, with the titles, animations, effects,
//! transitions, looks and music already in place.
//!
//! - `slot.rs` — what makes a clip a slot (a marker in the pool), and the
//!   project's slots in fill order.
//! - `apply.rs` — a template put into an open project, as a timeline or a
//!   compound clip.
//! - `fill.rs` — trimming, slowing and cropping media into a slot, and the
//!   edit that does it.
//! - `format.rs` — the template on disk (decision 0022).
//! - `save.rs` — a project turned into a template.
//! - `builtin.rs` — the templates we ship, built from our own parts.
//! - `assets.rs`, `music.rs` — the placeholders and music beds those use,
//!   drawn and synthesised here.
//! - `thumb.rs` — preview tiles.
//! - `commands.rs` — what the shells call.

pub mod apply;
pub mod assets;
pub mod builtin;
pub mod commands;
pub mod fill;
pub mod format;
pub mod music;
pub mod save;
pub mod slot;
pub mod thumb;
