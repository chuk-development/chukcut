//! Painting over a part of a picture, for "Remove object" (`enhance`):
//! LaMa in the ML worker (`chukcut_ml_worker::lama`).
//!
//! Stateless: one request is one crop of one frame and its mask. The crop,
//! the mask, the blend back into the frame and the smoothing from frame to
//! frame are the engine's (`enhance::removal`); the worker only runs the
//! network.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

use super::{worker, MlError};

/// The registry id of the inpainting model.
pub const MODEL: &str = "lama";

/// The version of [`MODEL`] this build runs.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the inpainting model")
}

/// Make the model ready; returns the provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the object-removal model", progress, cancel)
}

/// A filled picture and where it was made.
#[derive(Debug, Clone)]
pub struct Inpainted {
    /// RGBA8, the size of the picture sent.
    pub rgba: Vec<u8>,
    pub provider: String,
    /// The worker's own time, without the transfer.
    pub millis: f32,
}

/// `rgba` (RGBA8, `width × height`) with the pixels where `mask` is nonzero
/// painted over.
pub fn inpaint(
    rgba: &[u8],
    mask: &[u8],
    width: usize,
    height: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Inpainted, MlError> {
    let size = width * height;
    if rgba.len() != size * 4 || mask.len() != size {
        return Err(MlError::Failed(format!(
            "a {width}x{height} picture and mask were expected, got {} and {} bytes",
            rgba.len(),
            mask.len()
        )));
    }
    let body = RequestBody::Inpaint {
        model: MODEL.into(),
        width: width as u32,
        height: height as u32,
    };
    let mut payload = Vec::with_capacity(size * 5);
    payload.extend_from_slice(rgba);
    payload.extend_from_slice(mask);
    // ~3 s on four CPU threads, a first CUDA run picks kernels: generous,
    // but a wedged worker still dies.
    let timeout = Duration::from_secs(120);
    let (outcome, filled) = worker::request_payload(body, &payload, &|_, _| {}, cancel, timeout)?;
    match outcome {
        Outcome::Inpainted {
            provider, millis, ..
        } if filled.len() == size * 4 => Ok(Inpainted {
            rgba: filled,
            provider,
            millis,
        }),
        Outcome::Inpainted { .. } => Err(MlError::Failed(format!(
            "the worker sent {} bytes for a {width}x{height} picture",
            filled.len()
        ))),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
