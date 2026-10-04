//! Mattes through the ML worker, for "Remove background"
//! (`modules/matting`): people with Robust Video Matting, the main object of
//! the frame with BiRefNet lite.
//!
//! RVM carries state from frame to frame, so a [`Matter`] is one run of
//! consecutive frames: feed it frames in order, start a new one at a seek.
//! The state lives in the worker. If the worker dies mid-run, the next frame
//! goes to a new worker and starts from a clean state: a few frames of a
//! softer matte while it settles, instead of a failed job. BiRefNet has no
//! state; the same [`Matter`] runs it frame by frame.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};
use chukcut_ml_worker::registry::{self, Task};

use super::{worker, MlError};

/// The registry id of the people matting model.
pub const MODEL: &str = "rvm";

/// The registry id of the object matting model.
pub const OBJECTS_MODEL: &str = "birefnet-lite";

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// The version of [`MODEL`] this build runs, which a project records with
/// the setting so a matte is never silently made by another one.
pub fn model_version() -> &'static str {
    version_of(MODEL).expect("the registry lists the matting model")
}

/// The version of `model` this build runs, when it has it.
pub fn version_of(model: &str) -> Option<&'static str> {
    registry::model(model).map(|m| m.version)
}

/// Whether `model` makes mattes frame by frame without state (BiRefNet).
pub fn is_stateless(model: &str) -> bool {
    registry::model(model).is_some_and(|m| m.task == Task::MatteImage)
}

/// One run of consecutive frames through a matting model.
#[derive(Debug)]
pub struct Matter {
    model: String,
    session: u64,
    /// Where the model runs, e.g. `CUDA` or `CPU`; filled by the first frame.
    pub provider: Option<String>,
}

impl Matter {
    /// Make the people matting model ready (see [`super::prepare`]);
    /// returns the provider it runs on.
    pub fn prepare(
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<String, MlError> {
        Self::prepare_model(MODEL, progress, cancel)
    }

    /// Make `model` ready; returns the provider it runs on. A model that
    /// needs a GPU (`cpu_ok` false) fails here, in words, when only the CPU
    /// works (`super::prepare`).
    pub fn prepare_model(
        model: &str,
        progress: &dyn Fn(&str, Option<f32>),
        cancel: &AtomicBool,
    ) -> Result<String, MlError> {
        let what = match model {
            MODEL => "the background remover",
            OBJECTS_MODEL => "the object cut-out model",
            other => other,
        };
        super::prepare(model, what, progress, cancel)
    }

    /// A run of the people model.
    pub fn new() -> Self {
        Self::with_model(MODEL)
    }

    /// A run of `model`.
    pub fn with_model(model: &str) -> Self {
        Matter {
            model: model.to_string(),
            session: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            provider: None,
        }
    }

    /// The matte of the next frame (RGBA8, `width × height`): one byte of
    /// alpha per pixel, 255 where the subject is.
    pub fn next(&mut self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, MlError> {
        let body = RequestBody::Matte {
            model: self.model.clone(),
            session: self.session,
            width: width as u32,
            height: height as u32,
            allow_cpu: false,
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
        if is_stateless(&self.model) {
            return;
        }
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
