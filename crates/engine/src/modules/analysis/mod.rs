//! Picture and sound analysis that drives edits: scene detection,
//! stabilisation, beat detection and auto reframe.
//!
//! The plan is `docs/research/ml-features.md` — §3.5 scenes, §3.10
//! stabilisation, §3.6 beats, §3.7 auto reframe — in its "Phase 0" form: no
//! ML runtime, no model download, everything in the engine in plain Rust on
//! frames and samples we decode ourselves. Each piece is written so a model
//! can replace its detector later (TransNetV2 for scenes, Beat This! for
//! beats, YuNet / RT-DETR for reframe, all in the separate ML worker that
//! §5.1 asks for) without touching how the result reaches the document.
//!
//! - **Jobs** ([`jobs`]): one registry for every analysis, with progress and
//!   cancel; one undo step per finished job.
//! - **Document** ([`store`]): results that drive edits are tagged entries in
//!   `MaterialPool::extras`, referenced from the clip, in source time.
//! - **Cache** ([`cache`]): raw per-frame results, for speed only.
//! - **Scenes** ([`scenes`]): histogram + block difference against a local
//!   median; "Split at scene changes".
//! - **Stabilisation** ([`stabilise`]): KLT camera path, Gaussian smoothing,
//!   applied by the compositor as a moving crop window and a counter-turn,
//!   so the preview and the export match.
//! - **Beats** ([`beats`]): spectral-flux onsets, autocorrelation tempo,
//!   dynamic-programming beat tracking; auto-cut and snap ([`edits`]).
//! - **Reframe** ([`reframe`]): motion, contrast and skin saliency, best
//!   window per frame, per-shot path smoothing, written as position
//!   keyframes on a clip filled to the new canvas.
//! - **Commands** ([`commands`]): what the UI, a CLI and MCP call.

pub mod beats;
pub mod cache;
pub mod commands;
pub mod edits;
pub mod frames;
pub mod jobs;
pub mod reframe;
pub mod scenes;
pub mod stabilise;
pub mod store;
