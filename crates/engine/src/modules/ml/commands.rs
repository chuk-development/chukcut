//! The ML commands: what is installed, what can be, and what runs where.
//!
//! `ml_status`, `ml_models`, `ml_runtimes`, `ml_install`, `ml_remove` and
//! `ml_benchmark`. The settings page lists models and runtime packs with
//! their size and licence *before* anything is downloaded; features that use
//! a model fetch it themselves on first use (see `faces::FaceDetector`).

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::accel;
pub use chukcut_ml_worker::accel::{Acceleration, EngineBuild};
pub use chukcut_ml_worker::protocol::Quality;
use chukcut_ml_worker::protocol::{Outcome, Probe, RequestBody};
use chukcut_ml_worker::registry::{self, Task};
use serde::{Deserialize, Serialize};

use super::{download, worker, MlError};

/// The machine's ML state, for Settings › AI acceleration and `ml status`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MlStatus {
    /// The worker binary, when one was found.
    pub worker: Option<String>,
    /// The runtime pack the worker will load, when one is installed.
    pub runtime: Option<String>,
    /// Its id (`cpu`, `cuda12`, `cuda13`); `None` for `CHUKCUT_ORT_DYLIB`.
    pub runtime_id: Option<String>,
    /// What the worker found when it loaded the runtime; only filled when
    /// the status was asked for with `probe`.
    pub probe: Option<Probe>,
    /// Why ML is not available, as a sentence.
    pub problem: Option<String>,
    /// GPU vendors on this machine (`NVIDIA`, `Intel`, `AMD`), from the
    /// kernel's DRM devices.
    pub gpus: Vec<String>,
    /// The NVIDIA driver's version, when one is loaded.
    pub driver: Option<String>,
    /// The NVIDIA bundle for this driver, installed or not.
    pub bundle: Option<BundleInfo>,
    /// What runs models, as one sentence: "CUDA 13 on the GPU, with
    /// chukcut's CUDA libraries", "the CPU (CUDA: libcudnn.so.9 not found)".
    pub active: String,
    /// What would make ML faster here, as a sentence: the bundle to install
    /// for an NVIDIA GPU, the OpenVINO build for an Intel one.
    pub advice: Option<String>,
    /// The chosen mode (Settings › AI acceleration › Fast).
    pub acceleration: Acceleration,
    /// The TensorRT add-on that goes with this machine's GPU bundle, when
    /// there is an NVIDIA bundle for it.
    pub tensorrt: Option<TensorRtInfo>,
    /// Per model: what it runs on and at what precision.
    pub models: Vec<ModelAcceleration>,
}

/// The TensorRT add-on as the settings page lists it.
#[derive(Debug, Clone, Serialize)]
pub struct TensorRtInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub cuda: u32,
    pub version: &'static str,
    /// Runtime pack ids, listed and removed with the add-on.
    pub packs: &'static [&'static str],
    pub installed: bool,
    /// The whole download.
    pub bytes: u64,
    /// What is still to download.
    pub missing_bytes: u64,
    /// Its packs plus the engines built so far, on disk.
    pub installed_bytes: u64,
}

/// One model's acceleration, for `ml status` and Settings › AI acceleration.
#[derive(Debug, Clone, Serialize)]
pub struct ModelAcceleration {
    pub id: &'static str,
    pub name: &'static str,
    /// `TensorRT`, `CUDA`, `OpenVINO` or `CPU`: what its next job runs on.
    pub provider: String,
    /// `fp32` or `fp16`.
    pub precision: String,
    /// Whether "Fast" changes anything for it (it has a measured plan).
    pub has_fast_plan: bool,
    /// Its TensorRT engines built so far, with how long each took.
    pub engines: Vec<EngineBuild>,
}

/// The TensorRT add-on for the CUDA major of the runtime that loads (or of
/// the bundle this machine's driver calls for).
fn tensorrt_addon(root: &std::path::Path) -> Option<&'static registry::TensorRtAddon> {
    let cuda = registry::preferred_runtime(root)
        .map(|p| p.cuda)
        .filter(|c| *c != 0)
        .or_else(|| {
            registry::nvidia_driver()
                .as_deref()
                .and_then(registry::driver_major)
                .and_then(registry::bundle_for_driver)
                .map(|b| b.cuda)
        })?;
    registry::TENSORRT.iter().find(|t| t.cuda == cuda)
}

fn tensorrt_info(root: &std::path::Path, addon: &'static registry::TensorRtAddon) -> TensorRtInfo {
    let packs = || {
        addon
            .packs
            .iter()
            .filter_map(|id| registry::runtime_pack(id))
    };
    TensorRtInfo {
        id: addon.id,
        name: addon.name,
        cuda: addon.cuda,
        version: registry::TENSORRT_VERSION,
        packs: addon.packs,
        installed: registry::tensorrt_present(root, addon),
        bytes: packs().map(|p| p.bytes).sum(),
        missing_bytes: registry::tensorrt_missing_bytes(root, addon),
        installed_bytes: packs()
            .filter(|p| registry::runtime_present(root, p))
            .map(|p| download::size_of(&registry::runtime_dir(root, p)))
            .sum::<u64>()
            + download::size_of(&accel::engines_root(root)),
    }
}

/// What each model runs on, from the mode, what is installed and, when the
/// status was probed, what the worker found and has loaded.
fn model_acceleration(root: &std::path::Path, status: &MlStatus) -> Vec<ModelAcceleration> {
    let builds = accel::engine_builds(root);
    // The provider plain sessions get: the probe's best non-TensorRT one,
    // or a guess from the installed runtime.
    let base = match &status.probe {
        Some(p) => p
            .providers
            .iter()
            .find(|p| *p != "TensorRT")
            .cloned()
            .unwrap_or_else(|| "CPU".into()),
        None => match status.runtime_id.as_deref() {
            Some("cuda12") | Some("cuda13") => "CUDA".into(),
            Some(_) => "CPU".into(),
            None => "—".into(),
        },
    };
    let tensorrt = status.acceleration == Acceleration::Fast
        && match &status.probe {
            Some(p) => p.providers.iter().any(|p| p == "TensorRT"),
            None => base == "CUDA" && status.tensorrt.as_ref().is_some_and(|t| t.installed),
        };
    registry::MODELS
        .iter()
        .map(|m| {
            let plan = accel::fast_plan(m.id);
            let (mut provider, mut precision) = match plan.filter(|_| tensorrt) {
                Some(plan) => ("TensorRT".to_string(), plan.precision.as_str().to_string()),
                None => (base.clone(), "fp32".to_string()),
            };
            // What the worker actually holds wins: a TensorRT build that
            // failed left the model on CUDA.
            if let Some(probe) = &status.probe {
                let held: Vec<_> = probe.sessions.iter().filter(|s| s.model == m.id).collect();
                if let Some(s) = held.iter().find(|s| s.provider == "TensorRT") {
                    provider = s.provider.clone();
                    precision = s.precision.clone();
                }
            }
            ModelAcceleration {
                id: m.id,
                name: m.name,
                provider,
                precision,
                has_fast_plan: plan.is_some(),
                engines: builds.iter().filter(|b| b.model == m.id).cloned().collect(),
            }
        })
        .collect()
}

/// Turn "Fast (fp16/TensorRT)" on or off: saved in the settings, and the
/// worker restarts in the new mode at its next request.
pub fn ml_set_acceleration(fast: bool) -> Result<String, String> {
    let mut settings = crate::modules::workspace::settings::Settings::load();
    settings.ml_fast = fast;
    // Saves, and puts the mode into effect (`workspace_settings_apply`).
    crate::modules::workspace::commands::workspace_settings_set(settings)?;
    Ok(if fast {
        "AI acceleration: Fast (fp16/TensorRT where installed)".into()
    } else {
        "AI acceleration: Standard (fp32)".into()
    })
}

/// An NVIDIA bundle as the settings page lists it.
#[derive(Debug, Clone, Serialize)]
pub struct BundleInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub cuda: u32,
    pub min_driver: u32,
    pub packs: &'static [&'static str],
    pub installed: bool,
    /// The whole download.
    pub bytes: u64,
    /// What is still to download.
    pub missing_bytes: u64,
    /// What its installed packs take on disk.
    pub installed_bytes: u64,
    /// Whether this machine's driver can run it.
    pub runnable: bool,
    /// The one this machine's driver calls for.
    pub recommended: bool,
}

fn bundle_info(root: &std::path::Path, bundle: &'static registry::Bundle) -> BundleInfo {
    let driver = registry::nvidia_driver()
        .as_deref()
        .and_then(registry::driver_major);
    let packs = || {
        bundle
            .packs
            .iter()
            .filter_map(|id| registry::runtime_pack(id))
    };
    BundleInfo {
        id: bundle.id,
        name: bundle.name,
        cuda: bundle.cuda,
        min_driver: bundle.min_driver,
        packs: bundle.packs,
        installed: registry::bundle_present(root, bundle),
        bytes: packs().map(|p| p.bytes).sum(),
        missing_bytes: registry::bundle_missing_bytes(root, bundle),
        installed_bytes: packs()
            .map(|p| download::size_of(&registry::runtime_dir(root, p)))
            .sum(),
        runnable: driver.is_some_and(|d| d >= bundle.min_driver),
        recommended: driver
            .and_then(registry::bundle_for_driver)
            .is_some_and(|b| b.id == bundle.id),
    }
}

/// Every NVIDIA bundle, for the settings page.
pub fn ml_bundles() -> Vec<BundleInfo> {
    let root = super::root();
    registry::BUNDLES
        .iter()
        .map(|b| bundle_info(&root, b))
        .collect()
}

/// GPU vendors present, by PCI vendor id of each DRM card.
pub fn gpu_vendors() -> Vec<String> {
    let mut vendors = Vec::new();
    let Ok(cards) = std::fs::read_dir("/sys/class/drm") else {
        return vendors;
    };
    for card in cards.flatten() {
        let name = card.file_name();
        let name = name.to_string_lossy();
        // card0, card1 — not the connectors (card0-HDMI-A-1).
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let vendor = std::fs::read_to_string(card.path().join("device/vendor")).unwrap_or_default();
        let vendor = match vendor.trim() {
            "0x10de" => "NVIDIA",
            "0x8086" => "Intel",
            "0x1002" => "AMD",
            _ => continue,
        };
        if !vendors.iter().any(|v| v == vendor) {
            vendors.push(vendor.to_string());
        }
    }
    vendors
}

/// What to install to run models on this machine's GPU, if anything.
fn advice(root: &std::path::Path, gpus: &[String], driver: Option<&str>) -> Option<String> {
    if gpus.iter().any(|g| g == "NVIDIA") {
        let Some(version) = driver else {
            return Some(
                "An NVIDIA GPU is present, but its driver is not loaded (nouveau?); \
                 models run on the CPU until the NVIDIA driver is installed"
                    .into(),
            );
        };
        let Some(bundle) = registry::driver_major(version).and_then(registry::bundle_for_driver)
        else {
            return Some(format!(
                "The NVIDIA driver {version} is too old for GPU models (525 or newer runs \
                 CUDA 12); models run on the CPU"
            ));
        };
        if !registry::bundle_present(root, bundle) {
            return Some(format!(
                "An NVIDIA GPU is present; install \"{}\" ({} MB, `ml install gpu`) to run \
                 models on it",
                bundle.name,
                registry::bundle_missing_bytes(root, bundle) >> 20
            ));
        }
    } else if gpus.iter().any(|g| g == "Intel") && std::env::var_os("CHUKCUT_ORT_DYLIB").is_none() {
        return Some(
            "An Intel GPU is present; models run on it through OpenVINO when \
             CHUKCUT_ORT_DYLIB names an OpenVINO-enabled libonnxruntime.so \
             (Microsoft's Linux builds have no OpenVINO); until then they run on the CPU"
                .into(),
        );
    }
    None
}

/// "What runs models" as a sentence, from what is installed and, when
/// there is one, from the worker's probe.
fn active_sentence(root: &std::path::Path, status: &MlStatus) -> String {
    if let Some(problem) = &status.problem {
        return format!("Nothing: {problem}");
    }
    let Some(probe) = &status.probe else {
        return match &status.runtime {
            Some(runtime) => format!("{runtime} (not started yet)"),
            None => "Nothing yet; ONNX Runtime downloads on first use".into(),
        };
    };
    let gpu = probe.providers.iter().find(|p| *p != "CPU");
    match gpu {
        Some(provider) => {
            let cudart = probe.libraries.iter().find(|l| {
                l.rsplit('/')
                    .next()
                    .is_some_and(|n| n.starts_with("libcudart"))
            });
            let from = match cudart {
                Some(path) if std::path::Path::new(path).starts_with(root) => {
                    " with chukcut's CUDA libraries".to_string()
                }
                Some(path) => format!(" with the CUDA runtime at {path}"),
                None => String::new(),
            };
            let runtime = status.runtime_id.as_deref().unwrap_or("");
            let version = match runtime {
                "cuda12" => " 12",
                "cuda13" => " 13",
                _ => "",
            };
            format!("{provider}{version} on the GPU{from}")
        }
        None => {
            let why = probe
                .unavailable
                .iter()
                .find(|(name, _)| name == "CUDA")
                .map(|(_, why)| {
                    // ONNX Runtime's message starts with its source file and
                    // ends in the loader's reason, which is the useful part.
                    let reason = why.rsplit("with error: ").next().unwrap_or(why).trim();
                    let short: String = reason.chars().take(160).collect();
                    format!(" (CUDA: {short})")
                })
                .unwrap_or_default();
            format!("The CPU{why}")
        }
    }
}

/// What is installed. With `probe`, the worker is started and loads the
/// runtime to report which providers work — slower (up to a second, longer
/// with CUDA), so the settings page asks for it once, not per frame.
pub fn ml_status(probe: bool) -> MlStatus {
    let root = super::root();
    let gpus = gpu_vendors();
    let driver = registry::nvidia_driver();
    let preferred = registry::preferred_runtime(&root);
    let mut status = MlStatus {
        advice: advice(&root, &gpus, driver.as_deref()),
        bundle: driver
            .as_deref()
            .and_then(registry::driver_major)
            .and_then(registry::bundle_for_driver)
            .map(|b| bundle_info(&root, b)),
        gpus,
        driver,
        worker: worker::binary().map(|p| p.display().to_string()),
        acceleration: worker::acceleration(),
        tensorrt: tensorrt_addon(&root).map(|a| tensorrt_info(&root, a)),
        runtime: std::env::var("CHUKCUT_ORT_DYLIB")
            .ok()
            .or_else(|| preferred.map(|p| p.name.to_string())),
        runtime_id: if std::env::var_os("CHUKCUT_ORT_DYLIB").is_some() {
            None
        } else {
            preferred.map(|p| p.id.to_string())
        },
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
    status.active = active_sentence(&root, &status);
    status.models = model_acceleration(&root, &status);
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
    /// Whether it may run on the CPU; `false` means it needs a GPU.
    pub cpu_ok: bool,
    /// A second file downloaded with it (SAM's decoder).
    pub companion: Option<&'static str>,
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
            cpu_ok: m.cpu_ok,
            companion: m.companion,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub kind: registry::PackKind,
    /// The CUDA major version it belongs to, 0 for none.
    pub cuda: u32,
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
            kind: p.kind,
            cuda: p.cuda,
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
    /// An NVIDIA bundle (`registry::BUNDLES`): an ONNX Runtime build and
    /// every CUDA library it needs. `None` picks the one for this machine's
    /// driver.
    Gpu {
        #[serde(default)]
        id: Option<String>,
    },
    /// The TensorRT add-on (`registry::TENSORRT`) for "Fast" acceleration.
    /// `None` picks the one for the GPU bundle in use.
    Tensorrt {
        #[serde(default)]
        id: Option<String>,
    },
}

/// The TensorRT add-on `id` names, or the one for this machine.
fn resolve_tensorrt(id: Option<&str>) -> Result<&'static registry::TensorRtAddon, String> {
    match id {
        Some(id) => registry::tensorrt_addon(id).ok_or_else(|| {
            format!(
                "there is no TensorRT add-on {id}; choose {}",
                registry::TENSORRT
                    .iter()
                    .map(|t| t.id)
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        }),
        None => tensorrt_addon(&super::root()).ok_or_else(|| {
            "TensorRT needs an NVIDIA GPU with the NVIDIA bundle; install that first \
             (`ml install gpu`)"
                .to_string()
        }),
    }
}

/// The bundle `id` names, or the one for this machine's driver.
fn resolve_bundle(id: Option<&str>) -> Result<&'static registry::Bundle, String> {
    match id {
        Some(id) => registry::bundle(id).ok_or_else(|| {
            format!(
                "there is no GPU bundle {id}; choose {}",
                registry::BUNDLES
                    .iter()
                    .map(|b| b.id)
                    .collect::<Vec<_>>()
                    .join(" or ")
            )
        }),
        None => {
            let driver = registry::nvidia_driver().ok_or(
                "no NVIDIA driver is loaded; name a bundle (nvidia-cu12, nvidia-cu13) to \
                 install one anyway",
            )?;
            registry::driver_major(&driver)
                .and_then(registry::bundle_for_driver)
                .ok_or_else(|| {
                    format!("the NVIDIA driver {driver} is too old for GPU models (525 or newer)")
                })
        }
    }
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
            if let Some(companion) = spec.companion.and_then(registry::model) {
                download::ensure_model(&root, companion, allow_noncommercial, &report, cancel)?;
            }
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
        MlItem::Gpu { id } => {
            let bundle = resolve_bundle(id.as_deref())?;
            let total = registry::bundle_missing_bytes(&root, bundle);
            let mut before = 0u64;
            for pack in bundle
                .packs
                .iter()
                .filter_map(|id| registry::runtime_pack(id))
            {
                if registry::runtime_present(&root, pack) {
                    continue;
                }
                let base = before;
                download::ensure_runtime(
                    &root,
                    pack,
                    &|done, _| {
                        progress(MlProgress {
                            done: base + done,
                            total,
                        })
                    },
                    cancel,
                )
                .map_err(|e| format!("{} ({}): {e}", bundle.name, pack.name))?;
                before += pack.bytes;
            }
            worker::shutdown();
            Ok(format!("{} is installed", bundle.name))
        }
        MlItem::Tensorrt { id } => {
            let addon = resolve_tensorrt(id.as_deref())?;
            let total = registry::tensorrt_missing_bytes(&root, addon);
            let mut before = 0u64;
            for pack in addon
                .packs
                .iter()
                .filter_map(|id| registry::runtime_pack(id))
            {
                if registry::runtime_present(&root, pack) {
                    continue;
                }
                let base = before;
                download::ensure_runtime(
                    &root,
                    pack,
                    &|done, _| {
                        progress(MlProgress {
                            done: base + done,
                            total,
                        })
                    },
                    cancel,
                )
                .map_err(|e| format!("{} ({}): {e}", addon.name, pack.name))?;
                before += pack.bytes;
            }
            registry::link_tensorrt_provider(&root, addon)
                .map_err(|e| format!("cannot link the TensorRT provider: {e}"))?;
            worker::shutdown();
            Ok(format!(
                "{} is installed; with Fast acceleration on, the first job of each model \
                 and size builds its engine once (30 s to a few minutes)",
                addon.name
            ))
        }
    }
}

/// Delete `item` from the cache. The worker is stopped first, so no
/// process has the files open.
pub fn ml_remove(item: MlItem) -> Result<String, String> {
    let root = super::root();
    worker::shutdown();
    if let MlItem::Tensorrt { id } = &item {
        let addon = resolve_tensorrt(id.as_deref())?;
        registry::unlink_tensorrt_provider(&root, addon);
        for pack in addon
            .packs
            .iter()
            .filter_map(|id| registry::runtime_pack(id))
        {
            let path = registry::runtime_dir(&root, pack);
            if path.exists() {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
            }
        }
        // Engines are only good for TensorRT.
        let engines = accel::engines_root(&root);
        if engines.exists() {
            std::fs::remove_dir_all(&engines)
                .map_err(|e| format!("cannot delete {}: {e}", engines.display()))?;
        }
        return Ok(format!("{} is removed", addon.name));
    }
    if let MlItem::Gpu { id } = &item {
        let bundle = resolve_bundle(id.as_deref())?;
        for pack in bundle
            .packs
            .iter()
            .filter_map(|id| registry::runtime_pack(id))
        {
            let path = registry::runtime_dir(&root, pack);
            if path.exists() {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
            }
        }
        return Ok(format!("{} is removed", bundle.name));
    }
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
        MlItem::Gpu { .. } | MlItem::Tensorrt { .. } => unreachable!("handled above"),
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
    /// `fp32` or `fp16`.
    pub precision: String,
    /// The untimed first run: with TensorRT, the engine build when it was
    /// not cached yet.
    pub first_millis: f32,
    /// With `compare`: this run's output against the standard (fp32)
    /// session's on the same input.
    pub quality: Option<Quality>,
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
    ml_benchmark_with(model, width, height, iterations, None, false, cancel)
}

/// [`ml_benchmark`] in a given mode (`None`: the configured one), and with
/// `compare`, against the standard session's output. Fast mode and
/// `compare` need a worker in Fast mode: one is started for the
/// measurement and the configured mode restored after it.
pub fn ml_benchmark_with(
    model: &str,
    width: u32,
    height: u32,
    iterations: u32,
    acceleration: Option<Acceleration>,
    compare: bool,
    cancel: &AtomicBool,
) -> Result<Benchmark, String> {
    let configured = worker::acceleration();
    let needs_fast = compare || acceleration == Some(Acceleration::Fast);
    if needs_fast && configured != Acceleration::Fast {
        worker::set_acceleration(Acceleration::Fast);
    }
    let outcome = worker::request(
        RequestBody::Benchmark {
            model: model.into(),
            width,
            height,
            iterations,
            acceleration,
            compare,
        },
        &[],
        &|_, _| {},
        Some(cancel),
        Duration::from_secs(600),
    )
    .map_err(String::from);
    if needs_fast && configured != Acceleration::Fast {
        worker::set_acceleration(configured);
    }
    match outcome? {
        Outcome::Benchmark {
            provider,
            iterations,
            mean_millis,
            min_millis,
            precision,
            first_millis,
            quality,
        } => Ok(Benchmark {
            model: model.into(),
            provider,
            width,
            height,
            iterations,
            mean_millis,
            min_millis,
            precision,
            first_millis,
            quality,
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
