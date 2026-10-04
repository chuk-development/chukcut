//! "Remove background": a person matte per source frame, made by a model in
//! the ML worker, baked into the cache, and applied by the compositor as
//! alpha like a mask.
//!
//! - **The setting** is a field of the clip's compositing material
//!   ([`BackgroundRemoval`](crate::modules::project::compositing::BackgroundRemoval)):
//!   the model and its version, nothing else. Turning it on or off is one
//!   undoable edit.
//! - **The mattes** are pixels, so they are cache, not document
//!   (`docs/research/ml-features.md` §5.6): one greyscale PNG per source
//!   frame under `~/.cache/chukcut/mattes/` ([`cache`]), keyed by the media
//!   file's content key, the model and the version.
//! - **Baking** ([`bake`]) runs the clip's source range through Robust Video
//!   Matting (`modules/ml/matte.rs`) on its own thread; the preview shows
//!   each frame's matte as soon as it is written. An export bakes whatever
//!   is missing before it renders (`commands::matting_ensure`).
//! - **Rendering**: `render::matte_frames` finds the matte of the frame being
//!   drawn and `quad.wgsl` multiplies it into the clip's alpha, after the
//!   chroma key and with the masks, in every path a clip is drawn through.
//!
//! The model, RVM (GPL-3.0), and why: `docs/research/ml-features.md` §3.4
//! and `docs/decisions/0025-ml-worker-process.md`.

pub mod bake;
pub mod cache;
pub mod commands;
