//! Frames between frames, for "Optical flow (AI)" slow motion
//! (`speed::flow`): RIFE in the ML worker (`chukcut_ml_worker::rife`).
//!
//! Stateless: one request is one pair of consecutive source frames and the
//! phases wanted between them, so a 0.25x clip sends each pair once for its
//! three new frames.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};

use super::{worker, MlError};

/// The registry id of the interpolation model.
pub const MODEL: &str = "rife";

/// The version of [`MODEL`] this build runs, which the cache key records so
/// a frame made by another version is never shown.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the interpolation model")
}

/// Make the model ready; returns the provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the slow-motion model", progress, cancel)
}

/// New frames between two RGBA8 frames, and where they were made.
#[derive(Debug, Clone)]
pub struct Interpolated {
    /// One RGBA8 frame per phase asked for, in that order.
    pub frames: Vec<Vec<u8>>,
    pub provider: String,
    /// The worker's own time for all of them, without the transfer.
    pub millis: f32,
}

/// The frames at `phases` (each strictly between 0 and 1) between `first`
/// and `second`, both RGBA8 of `width × height`.
pub fn interpolate(
    first: &[u8],
    second: &[u8],
    width: usize,
    height: usize,
    phases: &[f32],
    cancel: Option<&AtomicBool>,
) -> Result<Interpolated, MlError> {
    let size = width * height * 4;
    if first.len() != size || second.len() != size {
        return Err(MlError::Failed(format!(
            "two {width}x{height} frames were expected, got {} and {} bytes",
            first.len(),
            second.len()
        )));
    }
    let body = RequestBody::Interpolate {
        model: MODEL.into(),
        width: width as u32,
        height: height as u32,
        phases: phases.to_vec(),
    };
    let mut payload = Vec::with_capacity(size * 2);
    payload.extend_from_slice(first);
    payload.extend_from_slice(second);
    // A 1080p frame on four CPU threads takes several seconds; the first one
    // on CUDA picks kernels. Generous, but a wedged worker still dies.
    let timeout = Duration::from_secs(90 + 45 * phases.len() as u64);
    let (outcome, frames) = worker::request_payload(body, &payload, &|_, _| {}, cancel, timeout)?;
    match outcome {
        Outcome::Interpolated {
            count,
            provider,
            millis,
            ..
        } if count as usize == phases.len() && frames.len() == size * phases.len() => {
            Ok(Interpolated {
                frames: frames.chunks_exact(size).map(<[u8]>::to_vec).collect(),
                provider,
                millis,
            })
        }
        Outcome::Interpolated { .. } => Err(MlError::Failed(format!(
            "the worker sent {} bytes for {} frames of {width}x{height}",
            frames.len(),
            phases.len()
        ))),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
