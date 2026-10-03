//! Keyframe-free motion: In, Out and Combo animation presets, the text
//! animator, and the punch-in zoom.
//!
//! The document stores parameters (`project::animation`); this module turns
//! them into a frame. Nothing here writes a keyframe, which is the point:
//! a preset is evaluated against the clip's *current* start and end, so it
//! survives every trim, and a split hands the entrance to the left half and
//! the exit to the right (`docs/research/resolve-plugins.md` §6.3–6.5).
//!
//! - [`pose`] — presets and easing curves to a relative transform, pure.
//! - [`text`] — per-letter, word or line stagger, and the animated title
//!   frame the compositor asks for while a text animator runs.
//! - [`catalog`] — the presets a UI offers, with their defaults.
//! - [`edit`] — the body of `EditCommand::SetAnimation` and the commands the
//!   UI builds: set a slot, set the zoom, auto zoom across jump cuts, split.
//! - [`commands`] — the shell-facing surface.
//!
//! ## How it reaches the picture
//!
//! The compositor calls [`pose::clip_motion`] once per clip after sampling
//! keyframes: offsets, scales, angle and opacity are applied on top of the
//! clip's own transform; a wipe becomes a narrowed quad
//! (`render::layout::reveal`); a blur is drawn through the transition
//! pipeline's blur pass with one side empty, so no quad shader changes. A
//! text clip whose animator is running is drawn from
//! [`text::animated_text_frame`] instead of the source provider.

pub mod catalog;
pub mod commands;
pub mod edit;
pub mod pose;
pub mod text;

pub use catalog::{clip_presets, text_presets, PresetDescriptor, TextPresetDescriptor};
pub use pose::{clip_motion, ClipMotion, Pose};
