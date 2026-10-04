//! Face detection (YuNet) through the ML worker.
//!
//! Used by auto reframe as the preferred subject cue. [`FaceDetector::prepare`]
//! does everything the first use needs (see [`super::prepare`]).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

pub use chukcut_ml_worker::protocol::Face;

use super::{worker, MlError};

/// The registry id of the face model.
pub const MODEL: &str = "yunet";
/// YuNet's published default. Lower finds more profile and small faces and
/// more false ones; the reframe path votes over many frames, so the default
/// is right for it.
pub const SCORE_THRESHOLD: f32 = 0.6;
/// The model was trained on faces between about 10 and 300 px. Frames for
/// detection should be sized so the faces that matter fall in that range;
/// 360 px tall does it for talking heads and wide shots alike.
pub const DETECTION_HEIGHT: u32 = 360;

/// A ready face detector. Cheap to keep; the session lives in the worker.
#[derive(Debug, Clone)]
pub struct FaceDetector {
    /// Where the model runs, e.g. `CUDA` or `CPU`.
    pub provider: String,
}

impl FaceDetector {
    /// Make face detection ready, downloading what is missing. `progress`
    /// gets a sentence for the status line and a fraction when one is known.
    pub fn prepare(
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<FaceDetector, MlError> {
        let provider = super::prepare(MODEL, "the face detector", progress, cancel)?;
        Ok(FaceDetector { provider })
    }

    /// The faces in one RGBA8 frame, in its pixels.
    pub fn detect(&self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<Face>, MlError> {
        match worker::request(
            RequestBody::DetectFaces {
                model: MODEL.into(),
                width: width as u32,
                height: height as u32,
                score_threshold: SCORE_THRESHOLD,
            },
            rgba,
            &|_, _| {},
            None,
            Duration::from_secs(30),
        )? {
            Outcome::Faces { faces, .. } => Ok(faces),
            other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}
