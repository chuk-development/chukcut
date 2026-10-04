//! Voice cleanup on one clip: noise reduction and loudness normalisation.
//!
//! CapCut's "Enhance voice" / "Reduce noise" / "Normalize loudness" switches,
//! and the one-knob tools short-form creators install into Resolve for the
//! same job (`docs/research/resolve-plugins.md` §6.6). Two parts:
//!
//! - **Denoise** is rendered, not live: RNNoise runs front to back over the
//!   whole file into a cached WAV ([`denoise`]), and the clip reads that file
//!   instead of its original in the preview and in the export alike. Why
//!   RNNoise and not DeepFilterNet or FFmpeg's filters:
//!   `docs/decisions/0015-voice-cleanup-engine.md`.
//! - **Normalize** is a gain, measured once with EBU R128 and stored.
//!
//! - **Isolate voice** separates the speech from music and noise (or keeps
//!   only the music) with a model in the ML worker, also rendered to the
//!   cache ([`isolate`]); the denoise then works on what it kept.
//!
//! All live in one parameter block on the clip ([`cleanup`]), and both
//! mixers resolve it through [`cleanup::effective_source`].

pub mod cleanup;
pub mod commands;
pub mod denoise;
pub mod isolate;

pub use cleanup::{
    audible_segment, cleanup_of, effective_source, Denoise, EffectiveSource, Normalize,
    VoiceCleanup,
};
