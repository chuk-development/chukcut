//! "Select object": the mask of the object under the user's clicks, by
//! MobileSAM in the ML worker (`chukcut_ml_worker::sam`).
//!
//! Stateless per request; the worker keeps the last frame's embedding, so
//! several clicks on one frame pay for one encode (~0.9 s on four CPU
//! threads, a few milliseconds on CUDA) and a decode each (~50 ms on the
//! CPU).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody, SegmentPoint};

use super::{worker, MlError};

/// The registry id of the segmentation model (its encoder; the decoder is
/// its companion).
pub const MODEL: &str = "mobilesam";

/// The version of [`MODEL`] this build runs.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the segmentation model")
}

/// One mask: one byte per pixel, and SAM's estimate of its quality.
#[derive(Debug, Clone)]
pub struct Mask {
    pub alpha: Vec<u8>,
    pub score: f32,
    pub provider: String,
}

/// Make the model ready; returns the provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the object selector", progress, cancel)
}

/// The mask of the object under `points` (pixels of the frame), inside
/// `bbox` (x, y, w, h) when given, in this RGBA8 frame.
pub fn segment(
    rgba: &[u8],
    width: usize,
    height: usize,
    points: &[SegmentPoint],
    bbox: Option<[f32; 4]>,
) -> Result<Mask, MlError> {
    let body = RequestBody::Segment {
        model: MODEL.into(),
        width: width as u32,
        height: height as u32,
        points: points.to_vec(),
        bbox,
    };
    let (outcome, alpha) =
        worker::request_payload(body, rgba, &|_, _| {}, None, Duration::from_secs(60))?;
    match outcome {
        Outcome::Segment {
            score, provider, ..
        } if alpha.len() == width * height => Ok(Mask {
            alpha,
            score,
            provider,
        }),
        Outcome::Segment { .. } => Err(MlError::Failed(format!(
            "the worker sent a {}-byte mask for a {width}x{height} frame",
            alpha.len()
        ))),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
