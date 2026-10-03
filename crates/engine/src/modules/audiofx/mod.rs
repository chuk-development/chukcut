//! Audio tools on a clip: pitch-preserving time stretch, an effect stack
//! (equalisers, compressor, reverb, echo, pitch, voice changer presets),
//! auto-ducking and voiceover recording.
//!
//! ## The pieces
//!
//! - [`model`] — the document side: one [`AudioFx`] block per clip in
//!   `MaterialPool::extras`, referenced from the clip's `extras`.
//! - [`catalog`] — every effect, its parameters, ranges and defaults.
//! - [`stretch`] — following a clip's time map (constant speed or speed
//!   curve) with the pitch kept (Signalsmith Stretch) or moved (resampling).
//! - [`dsp`] — the effects themselves, offline over a whole buffer.
//! - [`render`] — one clip's sound: source → time map → effects. The export
//!   calls it; the preview reads its cached result ([`cache`]).
//! - [`ducking`] — music down under speech, written as volume keyframes.
//! - [`record`] — voiceover takes from the default input device.
//! - [`commands`] — the shell-facing API, `audiofx_<verb>`.
//!
//! ## Why rendered and not live
//!
//! A phase vocoder, a reverb and a compressor all have state. Run inside the
//! preview mixer they would start cold at every seek, and the preview would
//! sound different from the export — the one thing an editor must not do.
//! Rendering a clip front to back is the same samples every time, which is
//! the argument `voice::denoise` made for RNNoise and the reason the shape is
//! the same. Decision 0020.

pub mod cache;
pub mod catalog;
pub mod commands;
pub mod dsp;
pub mod ducking;
pub mod model;
pub mod record;
pub mod render;
pub mod stretch;

pub use catalog::{catalog, descriptor, EffectDescriptor, Group, ParamSpec};
pub use model::{fx_of, fx_or_default, AudioEffect, AudioFx, DuckParams, Ducking};
pub use render::{render, spec_for, RenderSpec};
