//! A larger, cleaner picture, for "Enhance quality" (`enhance`):
//! Real-ESRGAN in the ML worker (`chukcut_ml_worker::esrgan`).
//!
//! Stateless: one request is one frame and the size it is wanted at. The
//! worker runs the 4x model in tiles and reduces the result to that size.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::esrgan::SCALE;
use chukcut_ml_worker::protocol::{Outcome, RequestBody};

use super::{worker, MlError};

/// The registry id of the super-resolution model.
pub const MODEL: &str = "realesr-general-x4v3";

/// The model's own scale: the largest a picture can be made.
pub const MODEL_SCALE: u32 = SCALE as u32;

/// The version of [`MODEL`] this build runs.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the super-resolution model")
}

/// Make the model ready; returns the provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the enhance-quality model", progress, cancel)
}

/// A larger picture and where it was made.
#[derive(Debug, Clone)]
pub struct Upscaled {
    /// RGBA8 at the size asked for.
    pub rgba: Vec<u8>,
    pub provider: String,
    /// The worker's own time, without the transfer.
    pub millis: f32,
}

/// `rgba` (RGBA8, `width × height`) made by the model and resized to
/// `out_width × out_height` (at most [`MODEL_SCALE`] times each side).
pub fn upscale(
    rgba: &[u8],
    width: usize,
    height: usize,
    out_width: usize,
    out_height: usize,
    cancel: Option<&AtomicBool>,
) -> Result<Upscaled, MlError> {
    if rgba.len() != width * height * 4 {
        return Err(MlError::Failed(format!(
            "a {width}x{height} picture was expected, got {} bytes",
            rgba.len()
        )));
    }
    let body = RequestBody::Upscale {
        model: MODEL.into(),
        width: width as u32,
        height: height as u32,
        out_width: out_width as u32,
        out_height: out_height as u32,
    };
    // A 1080p frame is ~20 s on four CPU threads; tiles report progress, so
    // the deadline is per frame, scaled by its size.
    let megapixels = (width * height) as u64 / 1_000_000 + 1;
    let timeout = Duration::from_secs(90 + 60 * megapixels);
    let (outcome, big) = worker::request_payload(body, rgba, &|_, _| {}, cancel, timeout)?;
    match outcome {
        Outcome::Upscaled {
            provider, millis, ..
        } if big.len() == out_width * out_height * 4 => Ok(Upscaled {
            rgba: big,
            provider,
            millis,
        }),
        Outcome::Upscaled { .. } => Err(MlError::Failed(format!(
            "the worker sent {} bytes for a {out_width}x{out_height} picture",
            big.len()
        ))),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
