//! "Remove background": a matte per source frame, made by a model in the ML
//! worker, baked into the cache, and applied by the compositor as alpha like
//! a mask. Three models: people (Robust Video Matting), the main object
//! (BiRefNet lite) and "Select object", the object under the user's clicks
//! (MobileSAM, propagated over the clip with VitTrack, [`object`]). Any of
//! them can be inverted: the subject cut out, the rest kept.
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
pub mod object;

use serde::{Deserialize, Serialize};

use crate::modules::ml::matte;
use crate::modules::project::compositing::BackgroundRemoval;

/// What "Remove background" keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode {
    /// People, by Robust Video Matting: fast, stable from frame to frame.
    #[default]
    People,
    /// The main object of the picture, by BiRefNet lite: any object, finer
    /// edges, needs a GPU.
    Objects,
}

impl BackgroundMode {
    /// The registry id of the model.
    pub fn model(self) -> &'static str {
        match self {
            BackgroundMode::People => matte::MODEL,
            BackgroundMode::Objects => matte::OBJECTS_MODEL,
        }
    }

    /// The mode a setting was made with; `None` for "Select object".
    pub fn of(setting: &BackgroundRemoval) -> Option<Self> {
        [BackgroundMode::People, BackgroundMode::Objects]
            .into_iter()
            .find(|m| m.model() == setting.model)
    }
}
