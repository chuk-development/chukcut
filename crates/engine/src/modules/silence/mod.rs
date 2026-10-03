//! Silence and filler-word cutting for talking-head footage.
//!
//! The most time-consuming part of editing a talking head is taking the
//! pauses out (`docs/research/resolve-plugins.md` §6.2). Every tool that does
//! it — Recut, AutoCut, FireCut, Resolve's IntelliCut — shows the cuts before
//! making them, because a wrong cut halves a word. So does this module:
//!
//! 1. [`analyse`] decodes the clip's sound once into a 10 ms level envelope
//!    (optionally with RNNoise's voice probability).
//! 2. [`detect`] turns the envelope plus a threshold, a minimum pause and a
//!    padding into a list of cuts — fast enough to re-run on every slider
//!    move.
//! 3. The user reviews the list, switches cuts off, and applies.
//!    [`cut::remove_ranges`] turns the chosen cuts into **one** undoable edit
//!    that splits and ripple-deletes the clip and its linked sound together.
//!
//! Filler words ([`filler`]) take their cuts from a transcript's word times
//! instead of from the levels, and feed the same list and the same edit.
//!
//! Every range here is in the clip's **source** time — microseconds into the
//! file — because that is what does not move when the clip is trimmed or slid
//! on the timeline while the panel is open.

pub mod analyse;
pub mod commands;
pub mod cut;
pub mod detect;
pub mod filler;

pub use analyse::{envelope_from_file, envelope_from_pcm, Envelope};
pub use cut::{remove_ranges, CutPlan};
pub use detect::{detect, suggest_threshold, Method, SilenceParams};
pub use filler::{filler_cuts, is_filler, register_word_timings, TimedWord, WordTimings};
