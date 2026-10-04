//! Dense face landmarks (MediaPipe's face mesh, 478 points) through the ML
//! worker, for face retouch and for text and stickers that follow a face
//! (`modules::landmarks`).
//!
//! The worker finds faces with YuNet on the first frame and then follows
//! them the way MediaPipe does: each frame's points give the region the next
//! frame is read in (`FaceMesh::roi`), and the detector runs again only when
//! every face is lost.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

pub use chukcut_ml_worker::protocol::FaceMesh;

use super::{worker, MlError};

/// The registry id of the face mesh model.
pub const MODEL: &str = "facemesh";
/// Faces per frame a clip is analysed for. Two covers an interview; more
/// costs a model run each on every frame.
pub const MAX_FACES: u32 = 2;

/// The version of [`MODEL`] this build runs, which the cache key records.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the face mesh")
}

/// Make the face mesh (and the detector it brings) ready; returns the
/// provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the face landmark model", progress, cancel)
}

/// The faces in one RGBA8 frame, with their landmarks in its pixels.
/// `hints` are the last frame's `roi`s.
pub fn landmarks(
    rgba: &[u8],
    width: usize,
    height: usize,
    hints: &[[f32; 4]],
    cancel: Option<&AtomicBool>,
) -> Result<Vec<FaceMesh>, MlError> {
    match worker::request(
        RequestBody::FaceLandmarks {
            model: MODEL.into(),
            width: width as u32,
            height: height as u32,
            hints: hints.to_vec(),
            max_faces: MAX_FACES,
        },
        rgba,
        &|_, _| {},
        cancel,
        Duration::from_secs(60),
    )? {
        Outcome::FaceLandmarks { faces, .. } => Ok(faces),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
