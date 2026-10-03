//! Speed ramps: a clip whose speed changes along a curve.
//!
//! The document model and the arithmetic live in `project::speed` (the curve,
//! its integral, and [`crate::modules::project::TimeMap`], which every
//! renderer, decoder and mixer asks for a clip's source time). This module is
//! the editing side:
//!
//! - [`edit`] — the body of `EditCommand::SetSpeedCurve` and the commands the
//!   UI builds: apply a preset, set custom points, remove the curve. Each one
//!   also changes the clip's length and moves what follows it, as one undo
//!   step.
//! - [`commands`] — the shell-facing surface.
//!
//! ## Sound
//!
//! A clip on a speed curve is heard: `modules::audiofx` renders its sound
//! through the curve with Signalsmith Stretch, keeping the pitch (or, with
//! "Change audio pitch" on, resampling it like a tape). The export renders it
//! inline; the preview plays the cached render and stays silent on the clip
//! until that has landed, usually well under a second. Decision 0021.

pub mod commands;
pub mod edit;
