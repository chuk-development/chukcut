//! Machine learning, run in a separate process.
//!
//! The editor never loads ONNX Runtime. Models run in `chukcut-ml-worker`
//! (`crates/ml-worker`), which this module starts, talks to over a framed
//! protocol on its stdin and stdout, and restarts when it dies. The research
//! is `docs/research/ml-features.md` §5; the decision, with what it costs, is
//! `docs/decisions/0025-ml-worker-process.md`.
//!
//! - **Worker** ([`worker`]): finding the binary, starting it, one request at
//!   a time per caller with progress and cancel, restart on crash with a
//!   budget so a worker that dies at start-up does not loop.
//! - **Downloads** ([`download`]): models and ONNX Runtime packs, pinned by
//!   URL and SHA-256 in the registry (`chukcut_ml_worker::registry`), into
//!   the cache (`~/.cache/chukcut/ml`), on first use, with progress.
//! - **Faces** ([`faces`]): YuNet, for auto reframe.
//! - **Tracker** ([`tracker`]): VitTrack, tracker T2 for fast motion, with a
//!   whole-frame scan that finds the object again after it was hidden.
//! - **Matte** ([`matte`]): Robust Video Matting, for "Remove background"
//!   (`modules/matting`).
//! - **Commands** ([`commands`]): what the UI, the CLI and MCP call.
//!
//! **Degrading.** Every caller treats ML as optional. No worker binary, no
//! runtime, no network for the first download, a crash: each ends in an
//! [`MlError`] that says what is missing, and the feature that asked falls
//! back to its model-free path (auto reframe to saliency, tracking to KLT).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};
use chukcut_ml_worker::registry;

pub mod commands;
pub mod download;
pub mod faces;
pub mod matte;
pub mod tracker;
pub mod worker;

pub use worker::MlError;

/// The ML directory: `models/` and `runtime/` under the cache. A model is
/// derived data that can always be fetched again, so "clear cache" may take
/// it; the next use downloads it again.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("ml")
}

/// Make `model` ready to run and return the provider it runs on: find the
/// worker, fetch the CPU runtime pack (9 MB) when no runtime is installed,
/// fetch the model, and create its session in the worker. Fails fast, before
/// any download, when the worker binary is not installed. `what` names the
/// model in progress sentences ("the face detector").
pub fn prepare(
    model: &str,
    what: &str,
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    if worker::binary().is_none() {
        return Err(MlError::Unavailable(format!(
            "{} is not installed",
            worker::BINARY
        )));
    }
    let spec = registry::model(model)
        .ok_or_else(|| MlError::Failed(format!("there is no model {model}")))?;
    let root = root();
    if std::env::var_os("CHUKCUT_ORT_DYLIB").is_none()
        && registry::preferred_runtime(&root).is_none()
    {
        let pack = registry::runtime_pack("cpu").expect("the registry has a CPU pack");
        download::ensure_runtime(
            &root,
            pack,
            &|done, total| {
                progress(
                    &format!(
                        "Downloading ONNX Runtime ({} of {} MB)",
                        done >> 20,
                        total.max(done) >> 20
                    ),
                    (total > 0).then(|| done as f32 / total as f32),
                )
            },
            cancel,
        )?;
    }
    download::ensure_model(
        &root,
        spec,
        false,
        &|done, total| {
            progress(
                &format!("Downloading {what}"),
                (total > 0).then(|| done as f32 / total as f32),
            )
        },
        cancel,
    )?;
    progress(&format!("Starting {what}"), None);
    match worker::request(
        RequestBody::Load {
            model: model.into(),
        },
        &[],
        &|_, _| {},
        Some(cancel),
        // Session creation on CUDA loads cuDNN and picks kernels; on a cold
        // disk that has taken tens of seconds.
        Duration::from_secs(120),
    )? {
        Outcome::Loaded { provider, .. } => Ok(provider),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}
