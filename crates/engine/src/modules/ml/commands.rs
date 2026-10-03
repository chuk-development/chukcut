//! The ML commands: what is installed, what can be, and what runs where.
//!
//! `ml_status`, `ml_models`, `ml_runtimes`, `ml_install`, `ml_remove` and
//! `ml_benchmark`. The settings page lists models and runtime packs with
//! their size and licence *before* anything is downloaded; features that use
//! a model fetch it themselves on first use (see `faces::FaceDetector`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, Probe, RequestBody};
use chukcut_ml_worker::registry::{self, Task};
use serde::{Deserialize, Serialize};

use super::{download, worker, MlError};

/// The machine's ML state, for Settings → Hardware.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MlStatus {
    /// The worker binary, when one was found.
    pub worker: Option<String>,
    /// The runtime pack the worker will load, when one is installed.
    pub runtime: Option<String>,
    /// What the worker found when it loaded the runtime; only filled when
    /// the status was asked for with `probe`.
    pub probe: Option<Probe>,
    /// Why ML is not available, as a sentence.
    pub problem: Option<String>,
}

/// What is installed. With `probe`, the worker is started and loads the
/// runtime to report which providers work — slower (up to a second, longer
/// with CUDA), so the settings page asks for it once, not per frame.
pub fn ml_status(probe: bool) -> MlStatus {
    let root = super::root();
    let mut status = MlStatus {
        worker: worker::binary().map(|p| p.display().to_string()),
        runtime: std::env::var("CHUKCUT_ORT_DYLIB")
            .ok()
            .or_else(|| registry::preferred_runtime(&root).map(|p| p.name.to_string())),
        ..MlStatus::default()
    };
    if status.worker.is_none() {
        status.problem = Some(format!("{} is not installed", worker::BINARY));
    } else if status.runtime.is_none() {
        status.problem = Some("no ONNX Runtime pack is installed yet".into());
    } else if probe {
        match worker::request(
            RequestBody::Probe,
            &[],
            &|_, _| {},
            None,
            Duration::from_secs(60),
        ) {
            Ok(Outcome::Probe(p)) => status.probe = Some(p),
            Ok(other) => status.problem = Some(format!("unexpected answer {other:?}")),
            Err(e) => status.problem = Some(e.to_string()),
        }
    }
    status
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub task: Task,
    pub licence: &'static str,
    pub commercial_ok: bool,
    pub bytes: u64,
    pub downloaded: bool,
    pub providers_tested: &'static [&'static str],
}

pub fn ml_models() -> Vec<ModelInfo> {
    let root = super::root();
    registry::MODELS
        .iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            version: m.version,
            task: m.task,
            licence: m.licence,
            commercial_ok: m.commercial_ok,
            bytes: m.bytes,
            downloaded: registry::model_present(&root, m),
            providers_tested: m.providers_tested,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    /// The download.
    pub bytes: u64,
    pub installed: bool,
    /// What it takes on disk once unpacked.
    pub installed_bytes: u64,
    pub needs: &'static str,
}

pub fn ml_runtimes() -> Vec<RuntimeInfo> {
    let root = super::root();
    registry::RUNTIME_PACKS
        .iter()
        .map(|p| RuntimeInfo {
            id: p.id,
            name: p.name,
            version: p.version,
            bytes: p.bytes,
            installed: registry::runtime_present(&root, p),
            installed_bytes: download::size_of(&registry::runtime_dir(&root, p)),
            needs: p.needs,
        })
        .collect()
}

/// Something to install or remove.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MlItem {
    Model {
        id: String,
        /// Needed for a model whose licence forbids commercial use.
        #[serde(default)]
        allow_noncommercial: bool,
    },
    Runtime {
        id: String,
    },
}

/// Progress of an install, for a progress bar.
#[derive(Debug, Clone, Serialize)]
pub struct MlProgress {
    pub done: u64,
    pub total: u64,
}

/// Download and install `item`. Blocking; run it on a worker thread.
pub fn ml_install(
    item: MlItem,
    progress: &(dyn Fn(MlProgress) + Sync),
    cancel: &AtomicBool,
) -> Result<String, String> {
    let root = super::root();
    let report = |done, total| progress(MlProgress { done, total });
    match item {
        MlItem::Model {
            id,
            allow_noncommercial,
        } => {
            let spec = registry::model(&id).ok_or(format!("there is no model {id}"))?;
            download::ensure_model(&root, spec, allow_noncommercial, &report, cancel)?;
            Ok(format!("{} is installed", spec.name))
        }
        MlItem::Runtime { id } => {
            let pack =
                registry::runtime_pack(&id).ok_or(format!("there is no runtime pack {id}"))?;
            download::ensure_runtime(&root, pack, &report, cancel)?;
            // A running worker loaded the old choice of runtime; the next
            // request starts one that picks the new pack.
            worker::shutdown();
            Ok(format!("{} is installed", pack.name))
        }
    }
}

/// Delete `item` from the cache. The worker is stopped first, so no
/// process has the files open.
pub fn ml_remove(item: MlItem) -> Result<String, String> {
    let root = super::root();
    worker::shutdown();
    let (path, name) = match &item {
        MlItem::Model { id, .. } => {
            let spec = registry::model(id).ok_or(format!("there is no model {id}"))?;
            (
                registry::model_path(&root, spec)
                    .parent()
                    .expect("a model path has a directory")
                    .to_path_buf(),
                spec.name,
            )
        }
        MlItem::Runtime { id } => {
            let pack =
                registry::runtime_pack(id).ok_or(format!("there is no runtime pack {id}"))?;
            (registry::runtime_dir(&root, pack), pack.name)
        }
    };
    if path.exists() {
        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
    }
    Ok(format!("{name} is removed"))
}

/// What [`ml_benchmark`] measured.
#[derive(Debug, Clone, Serialize)]
pub struct Benchmark {
    pub model: String,
    pub provider: String,
    pub width: u32,
    pub height: u32,
    pub iterations: u32,
    pub mean_millis: f32,
    pub min_millis: f32,
}

/// Time `model` on a synthetic `width × height` frame in the worker. The
/// model must be installed. This is how the speeds in the docs are measured.
pub fn ml_benchmark(
    model: &str,
    width: u32,
    height: u32,
    iterations: u32,
    cancel: &AtomicBool,
) -> Result<Benchmark, String> {
    let outcome = worker::request(
        RequestBody::Benchmark {
            model: model.into(),
            width,
            height,
            iterations,
        },
        &[],
        &|_, _| {},
        Some(cancel),
        Duration::from_secs(600),
    )
    .map_err(String::from)?;
    match outcome {
        Outcome::Benchmark {
            provider,
            iterations,
            mean_millis,
            min_millis,
        } => Ok(Benchmark {
            model: model.into(),
            provider,
            width,
            height,
            iterations,
            mean_millis,
            min_millis,
        }),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}")).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_name_licence_and_size_before_any_download() {
        let models = ml_models();
        let yunet = models.iter().find(|m| m.id == "yunet").unwrap();
        assert_eq!(yunet.licence, "MIT");
        assert!(yunet.commercial_ok && yunet.bytes > 100_000);
        let runtimes = ml_runtimes();
        assert!(runtimes
            .iter()
            .any(|r| r.id == "cpu" && r.bytes > 1_000_000));
    }

    #[test]
    fn unknown_items_are_refused_in_words() {
        let error = ml_install(
            MlItem::Model {
                id: "nope".into(),
                allow_noncommercial: false,
            },
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("nope"));
    }
}
