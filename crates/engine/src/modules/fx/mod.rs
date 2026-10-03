//! Built-in effects and layouts: the short-form pack, the film look, and
//! picture-in-picture and split-screen arrangements.
//!
//! Not to be confused with `modules/effects`, which loads third-party effect
//! packages at runtime. Everything here is ours: our WGSL, our parameters,
//! rendered by the compositor on the one wgpu device.
//!
//! ## The pieces
//!
//! - The document side is [`EffectMaterial`](crate::modules::project::EffectMaterial)
//!   in `project/effects.rs`: an effect is a parameter block in the pool,
//!   referenced from a clip (it sees that clip) or as the material of a clip
//!   on an effect lane (it sees everything beneath it). Decision 0016.
//! - [`catalog`] — every effect, its parameters, ranges and defaults.
//! - [`edit`] — the edits, as `EditCommand`s, and the layout builders.
//! - [`commands`] — the shell-facing API, `fx_<verb>`.
//! - [`render`] — the pipelines and the pass recorder the compositor drives.
//! - [`tiles`] — preview tiles rendered by the compositor and cached on disk.

pub mod catalog;
pub mod commands;
pub mod edit;
pub mod render;
pub mod tiles;

#[cfg(test)]
mod render_tests;

pub use catalog::{catalog, descriptor, Category, EffectDescriptor, ParamKind, ParamSpec};
pub use render::{chain_for, FxFrame, FxInstance, FxRenderer, OverDraw};
