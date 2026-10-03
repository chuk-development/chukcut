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
//! A clip on a speed curve is **muted**, in the preview and in the export.
//! The audio engine resamples at one constant rate per clip and has no
//! pitch-preserving time stretch (`docs/STATUS.md`, "Speed changes shift
//! pitch"); a sound that slides through two octaves inside one ramp is not
//! something anyone keeps. Resolve does the same with a retime curve. The
//! Curve tab says so. When a time stretch lands, `audio::mixer::plan` and
//! `export::audio` are the two places to stop skipping these clips.

pub mod commands;
pub mod edit;
