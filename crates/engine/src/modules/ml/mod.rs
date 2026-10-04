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
//! - **Landmarks** ([`landmarks`]): MediaPipe's face mesh, 478 points per
//!   face, for retouch and face-follow (`modules::landmarks`).
//! - **Tracker** ([`tracker`]): VitTrack, tracker T2 for fast motion, with a
//!   whole-frame scan that finds the object again after it was hidden.
//! - **Matte** ([`matte`]): Robust Video Matting (people) and BiRefNet
//!   (objects), for "Remove background" (`modules/matting`).
//! - **Segment** ([`segment`]): MobileSAM, the object under a click, for
//!   "Select object".
//! - **Interpolate** ([`interpolate`]): RIFE, frames between frames, for
//!   "Optical flow (AI)" slow motion (`speed::flow`).
//! - **Separate** ([`separate`]): HTDemucs, the voice out of a recording,
//!   for "Isolate voice" (`voice::isolate`).
//! - **Inpaint** ([`inpaint`]): LaMa, a part of a picture painted over, for
//!   "Remove object" (`enhance`).
//! - **Upscale** ([`upscale`]): Real-ESRGAN, a larger, cleaner picture, for
//!   "Enhance quality" (`enhance`).
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
pub mod inpaint;
pub mod interpolate;
pub mod landmarks;
pub mod matte;
pub mod segment;
pub mod separate;
pub mod tracker;
pub mod upscale;
pub mod worker;

pub use worker::MlError;

/// The ML directory: `models/` and `runtime/` under the cache. It sits in the
/// cache because a model can always be fetched again, but "clear cache" and
/// the cache limit leave it alone (`workspace::trim::EXEMPT_DIRS`): the
/// downloads are large and the user chose them. Models and runtime packs are
/// removed one by one in Settings › AI acceleration.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("ml")
}

/// The "AI runtime" setting (`workspace::Settings::ml_runtime`): a pack id,
/// or `None` for the automatic choice.
static RUNTIME_SETTING: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// Put the "AI runtime" setting into effect. A change stops a running worker:
/// it loaded its runtime when it started, and the next request starts a new
/// one on the chosen pack.
pub fn set_runtime_setting(pack: Option<String>) {
    let pack = pack.filter(|p| !p.trim().is_empty() && p != "auto");
    let changed = {
        let mut current = RUNTIME_SETTING.lock();
        let changed = *current != pack;
        *current = pack;
        changed
    };
    if changed && std::env::var_os("CHUKCUT_ML_RUNTIME").is_none() {
        worker::shutdown();
    }
}

/// The runtime pack asked for by name: `CHUKCUT_ML_RUNTIME` when set, else
/// the "AI runtime" setting, else `None` (automatic). The worker reads the
/// same variable; `worker::spawn` hands it the setting through it.
pub fn runtime_choice() -> Option<String> {
    std::env::var("CHUKCUT_ML_RUNTIME")
        .ok()
        .or_else(|| RUNTIME_SETTING.lock().clone())
}

/// The installed ONNX Runtime pack the worker will load: the one asked for
/// ([`runtime_choice`]) when it is installed, else the automatic choice
/// (`registry::choose_runtime`).
pub fn preferred_runtime(root: &std::path::Path) -> Option<&'static registry::RuntimePack> {
    registry::choose_runtime(
        root,
        registry::nvidia_driver()
            .as_deref()
            .and_then(registry::driver_major),
        runtime_choice().as_deref(),
    )
}

/// The ONNX Runtime packs the "AI runtime" setting can name, in registry
/// order: `(id, name, installed)`.
pub fn runtime_packs() -> Vec<(&'static str, &'static str, bool)> {
    let root = root();
    registry::RUNTIME_PACKS
        .iter()
        .filter(|p| p.kind == registry::PackKind::OnnxRuntime)
        .map(|p| (p.id, p.name, registry::runtime_present(&root, p)))
        .collect()
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
    if std::env::var_os("CHUKCUT_ORT_DYLIB").is_none() && preferred_runtime(&root).is_none() {
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
    // A model too slow for the CPU must not be downloaded (224 MB for
    // BiRefNet) or loaded on a machine where only the CPU works: ask the
    // worker what runs first.
    if !spec.cpu_ok {
        let providers = match worker::request(
            RequestBody::Probe,
            &[],
            &|_, _| {},
            Some(cancel),
            Duration::from_secs(60),
        )? {
            Outcome::Probe(probe) => probe.providers,
            other => return Err(MlError::Failed(format!("unexpected answer {other:?}"))),
        };
        if providers.iter().all(|p| p == "CPU") {
            return Err(MlError::Failed(format!(
                "{} needs a GPU, and models run on the CPU here (one frame would take \
                 12–25 s and 6–11 GB of memory). Install the GPU bundle in Settings › AI \
                 acceleration, or use People or Select object",
                spec.name
            )));
        }
    }
    for spec in std::iter::once(spec).chain(spec.companion.and_then(registry::model)) {
        download::ensure_model(
            &root,
            spec,
            false,
            &|done, total| {
                progress(
                    &format!("Downloading {what} ({} of {} MB)", done >> 20, total >> 20),
                    (total > 0).then(|| done as f32 / total as f32),
                )
            },
            cancel,
        )?;
    }
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
