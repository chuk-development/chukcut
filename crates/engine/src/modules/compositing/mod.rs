//! Masks, chroma key and blend modes: how a clip is cut out and laid onto
//! what is beneath it.
//!
//! ## The pieces
//!
//! - The document side is
//!   [`CompositingMaterial`](crate::modules::project::compositing::CompositingMaterial)
//!   in `project/compositing.rs`: one pool material per clip, referenced from
//!   its `extras`, never edited in place. Decision 0020.
//! - [`edit`] — the edits, as `EditCommand`s.
//! - [`commands`] — the shell-facing API, `compositing_<verb>`, and the
//!   eyedropper that picks a key colour from the footage.
//! - The renderer's side is in `render`: `render::matte` packs masks and the
//!   key for `quad.wgsl` (per-pixel alpha in the clip's own quad) and is
//!   their CPU reference; `render::blend` is the reference for the blend pass
//!   (`fs_blend` in `fx.wgsl`), which lays a clip's layer onto the frame
//!   composited beneath it.

pub mod commands;
pub mod edit;

#[cfg(test)]
mod render_tests;
