//! Person mattes (Robust Video Matting) through the ML worker, for
//! "Remove background" (`modules/matting`).
//!
//! RVM carries state from frame to frame, so a [`Matter`] is one run of
//! consecutive frames: feed it frames in order, start a new one at a seek.
//! The state lives in the worker. If the worker dies mid-run, the next frame
//! goes to a new worker and starts from a clean state: a few frames of a
//! softer matte while it settles, instead of a failed job.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};
use chukcut_ml_worker::registry;

use super::{worker, MlError};

/// The registry id of the matting model.
pub const MODEL: &str = "rvm";

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// The version of [`MODEL`] this build runs, which a project records with
/// the setting so a matte is never silently made by another one.
pub fn model_version() -> &'static str {
    registry::model(MODEL)
        .map(|m| m.version)
        .expect("the registry lists the matting model")
}

/// One run of consecutive frames through the matting model.
#[derive(Debug)]
pub struct Matter {
    session: u64,
    /// Where the model runs, e.g. `CUDA` or `CPU`; filled by the first frame.
    pub provider: Option<String>,
}

impl Matter {
    /// Make the matting model ready (see [`super::prepare`]); returns the
    /// provider it runs on.
    pub fn prepare(
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<String, MlError> {
        super::prepare(MODEL, "the background remover", progress, cancel)
    }

    pub fn new() -> Self {
        Matter {
            session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            provider: None,
        }
    }

    /// The matte of the next frame (RGBA8, `width × height`): one byte of
    /// alpha per pixel, 255 where the person is.
    pub fn next(&mut self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, MlError> {
        let body = RequestBody::Matte {
            model: MODEL.into(),
            session: self.session,
            width: width as u32,
            height: height as u32,
        };
        // The first frame on CUDA can include kernel selection; later ones
        // take tens of milliseconds.
        let (outcome, alpha) =
            worker::request_payload(body, rgba, &|_, _| {}, None, Duration::from_secs(60))?;
        match outcome {
            Outcome::Matte { provider, .. } if alpha.len() == width * height => {
                self.provider.get_or_insert(provider);
                Ok(alpha)
            }
            Outcome::Matte { .. } => Err(MlError::Failed(format!(
                "the worker sent a {}-byte matte for a {width}x{height} frame",
                alpha.len()
            ))),
            other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}

impl Default for Matter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Matter {
    fn drop(&mut self) {
        // Only tell a worker that is running; never start one to forget.
        if let Some(client) = worker::running() {
            let _ = client.call_with(
                RequestBody::MatteEnd {
                    session: self.session,
                },
                &[],
                &|_, _| {},
                None,
                Duration::from_secs(5),
            );
        }
    }
}
