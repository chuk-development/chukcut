//! ONNX Runtime, loaded at run time, and the sessions the worker keeps.
//!
//! The library is not linked: `ort`'s `load-dynamic` opens it with `dlopen`
//! when the first request needs it. A machine without a runtime pack still
//! runs the worker (it answers `hello` and reports `runtime_missing`), and the
//! engine falls back to its model-free paths.
//!
//! **Provider order** (`docs/research/ml-features.md` §5.3): CUDA, then
//! OpenVINO, then the CPU. A provider is used for a model only if registering
//! it succeeds; the common failure is a GPU pack without cuDNN, which ends in
//! "libcudnn.so.9: cannot open shared object file" and a CPU session, never a
//! crash. Microsoft's Linux builds carry no OpenVINO provider; an
//! OpenVINO-enabled `libonnxruntime.so` (Intel publishes one) can be named in
//! `CHUKCUT_ORT_DYLIB` and is then used on Intel GPUs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use ort::ep::{self, ExecutionProvider as _};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use crate::accel::{self, Acceleration, EngineBuild, EngineTarget, Precision};
use crate::protocol::{ErrorKind, Probe};
use crate::registry::{self, ModelSpec};

/// An error the worker reports as is.
pub type Failure = (ErrorKind, String);

/// Threads per session. The editor decodes and renders beside the worker;
/// four keeps a small model fast without taking the machine.
const INTRA_THREADS: usize = 4;

/// The loaded runtime.
pub struct Runtime {
    pub probe: Probe,
    /// By model id, or `<id>@<shapes>` for a TensorRT session, which is
    /// built for one set of input shapes.
    sessions: HashMap<String, Loaded>,
    /// Providers to try for a new session, best first.
    order: Vec<&'static str>,
    root: PathBuf,
    /// "Fast" mode and TensorRT registered: models with a fast plan get
    /// TensorRT sessions.
    tensorrt: Option<EngineTarget>,
    /// Models TensorRT failed to build or run in this worker; they stay on
    /// the CUDA provider until the worker restarts.
    tensorrt_failed: HashSet<&'static str>,
}

pub struct Loaded {
    pub session: Session,
    pub provider: &'static str,
    pub precision: Precision,
}

/// Told when a session is about to build a TensorRT engine, which blocks
/// for up to minutes without progress. The worker's main loop sets it per
/// request to send a progress message that names the wait and lets the
/// engine extend the request's deadline (`Reply::Progress::hold_secs`).
type Notice = Box<dyn Fn(&str, u32) + Send>;
static BUILD_NOTICE: Mutex<Option<Notice>> = Mutex::new(None);

/// Set (or clear) the build notice; see [`BUILD_NOTICE`].
pub fn set_build_notice(notice: Option<Notice>) {
    *BUILD_NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = notice;
}

fn build_notice(stage: &str, hold_secs: u32) {
    if let Some(notice) = BUILD_NOTICE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        notice(stage, hold_secs);
    }
}

/// How long the engine waits, without progress, for a TensorRT build: the
/// slowest measured was ~6 min (LaMa, fp32) on a loaded RTX 3060.
pub const TENSORRT_BUILD_HOLD_SECS: u32 = 1800;

/// TensorRT's libraries, preloaded from the TensorRT pack in "Fast" mode so
/// the provider finds them by soname. The builder resources
/// (`libnvinfer_builder_resource_sm86.so…`) are not preloaded: libnvinfer
/// opens the one for the GPU from its own directory (its RPATH is
/// `$ORIGIN`), and loading all of them would map 2 GB for nothing.
const TENSORRT_PRELOAD: &[&str] = &[
    "libnvinfer.so.",
    "libnvinfer_plugin.so.",
    "libnvonnxparser.so.",
];

/// The library to load: `CHUKCUT_ORT_DYLIB` if set, else the preferred
/// installed pack under `root`.
pub fn library(root: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHUKCUT_ORT_DYLIB") {
        return Some(PathBuf::from(path));
    }
    registry::preferred_runtime(root).map(|p| registry::runtime_library(root, p))
}

/// Directories to look in for CUDA and cuDNN libraries, beyond the system's
/// loader path, first match wins: `CHUKCUT_CUDA_LIB_DIRS` (colon-separated),
/// then the installed library packs of the same CUDA major version as the
/// runtime being loaded (`registry::library_packs_for`: the CUDA runtime,
/// cuBLAS, cuRAND, NVRTC and cuDNN from NVIDIA's wheels), then the runtime
/// pack's own `lib/`. A CUDA 13 provider never meets a CUDA 12 library this
/// way, and the system's own CUDA (Ubuntu's 12.0 is too old for ORT 1.28)
/// is used only for what no pack has.
fn cuda_dirs(root: &Path, lib: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("CHUKCUT_CUDA_LIB_DIRS")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    let ort = registry::RUNTIME_PACKS.iter().find(|p| {
        p.kind == registry::PackKind::OnnxRuntime && lib.starts_with(registry::runtime_dir(root, p))
    });
    if let Some(ort) = ort {
        for pack in registry::library_packs_for(root, ort) {
            dirs.push(registry::runtime_dir(root, pack).join("lib"));
        }
    }
    if let Some(parent) = lib.parent() {
        dirs.push(parent.to_path_buf());
    }
    dirs
}

/// The CUDA, cuBLAS and cuDNN libraries this process has loaded, by path,
/// from `/proc/self/maps`: what `ml status --probe` shows as the libraries
/// actually in use, which is the one honest answer to "which CUDA runs".
fn loaded_cuda_libraries() -> Vec<String> {
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().nth(5) else {
            continue;
        };
        let name = path.rsplit('/').next().unwrap_or("");
        let wanted = [
            "libcudart.so",
            "libcublas.so",
            "libcudnn.so",
            "libonnxruntime_providers_cuda.so",
        ];
        if wanted.iter().any(|w| name.starts_with(w)) && !found.iter().any(|f| f == path) {
            found.push(path.to_string());
        }
    }
    found
}

/// The CUDA and cuDNN libraries the CUDA provider opens, by file-name
/// prefix, in an order where each one's dependencies come before it. By
/// prefix rather than by `ort`'s list of names because that list is CUDA
/// 12's (`libcudart.so.12`), and the CUDA 13 provider asks for `.so.13`.
const CUDA_PRELOAD: &[&str] = &[
    "libcudart.so.",
    "libnvJitLink.so.",
    "libcublasLt.so.",
    "libcublas.so.",
    "libnvrtc.so.",
    "libnvrtc-builtins.so.",
    "libcurand.so.",
    "libcufft.so.",
    "libcudnn.so.",
    "libcudnn_graph.so.",
    "libcudnn_ops.so.",
    "libcudnn_heuristic.so.",
    "libcudnn_engines_precompiled.so.",
    "libcudnn_engines_runtime_compiled.so.",
    "libcudnn_adv.so.",
    "libcudnn_cnn.so.",
];

/// Load each CUDA and cuDNN library the CUDA provider will ask for from the
/// first of `dirs` that has it. A library loaded this way is found by its
/// soname when the provider opens it later, so the user's loader path does
/// not have to change. Libraries found nowhere are left to the system loader.
fn preload_cuda(dirs: &[PathBuf]) {
    preload(dirs, CUDA_PRELOAD);
}

/// Load each library named by a prefix in `prefixes` from the first of
/// `dirs` that has it.
fn preload(dirs: &[PathBuf], prefixes: &[&str]) {
    for prefix in prefixes {
        let found = dirs.iter().find_map(|dir| {
            let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(prefix))
                })
                .collect();
            // The soname (`libcudart.so.13`) sorts before the full version
            // (`libcudart.so.13.0.96`); either is the same library.
            names.sort();
            names.into_iter().next()
        });
        if let Some(path) = found {
            if let Err(e) = ort::util::preload_dylib(&path) {
                eprintln!(
                    "chukcut-ml-worker: could not preload {}: {e}",
                    path.display()
                );
            }
        }
    }
}

/// Whether cuDNN 9 can be opened: preloaded from a pack (found by its
/// soname then), or on the system's loader path.
fn cudnn_present() -> bool {
    ort::util::preload_dylib("libcudnn.so.9").is_ok()
}

impl Runtime {
    /// Load the library and find out which providers work.
    pub fn load(root: &Path, mode: Acceleration) -> Result<Runtime, Failure> {
        let lib = library(root).ok_or((
            ErrorKind::RuntimeMissing,
            "no ONNX Runtime is installed for the ML worker".to_string(),
        ))?;
        if !lib.is_file() {
            return Err((
                ErrorKind::RuntimeMissing,
                format!("{} does not exist", lib.display()),
            ));
        }
        let dirs = cuda_dirs(root, &lib);
        preload_cuda(&dirs);
        // TensorRT only in "Fast" mode: libnvinfer is 660 MB to map and
        // relocate, which a worker that will not use it should not pay.
        let ort_pack = registry::RUNTIME_PACKS.iter().find(|p| {
            p.kind == registry::PackKind::OnnxRuntime
                && lib.starts_with(registry::runtime_dir(root, p))
        });
        let addon = ort_pack
            .and_then(registry::tensorrt_for)
            .filter(|a| registry::tensorrt_present(root, a));
        if mode == Acceleration::Fast {
            if let Some(addon) = addon {
                if let Err(e) = registry::link_tensorrt_provider(root, addon) {
                    eprintln!("chukcut-ml-worker: cannot link the TensorRT provider: {e}");
                }
                preload(&dirs, TENSORRT_PRELOAD);
            }
        }
        let committed = ort::init_from(&lib)
            .map_err(|e| {
                (
                    ErrorKind::RuntimeMissing,
                    format!("could not load {}: {e}", lib.display()),
                )
            })?
            .with_name("chukcut-ml-worker")
            .with_telemetry(false)
            .commit();
        if !committed {
            eprintln!("chukcut-ml-worker: an ONNX Runtime environment already existed");
        }
        let info = ort::info();
        // The build info string of Microsoft's builds names no version
        // ("git-branch=HEAD"), but the library's real file name does:
        // libonnxruntime.so -> libonnxruntime.so.1 -> libonnxruntime.so.1.28.3.
        let runtime_version = std::fs::canonicalize(&lib)
            .ok()
            .and_then(|p| {
                p.file_name()?
                    .to_str()?
                    .split_once(".so.")
                    .map(|(_, v)| v.to_string())
            })
            .unwrap_or_else(|| info.to_string());
        let mut probe = Probe {
            runtime: lib.display().to_string(),
            runtime_version,
            acceleration: mode,
            ..Probe::default()
        };
        let mut order = Vec::new();
        // Registering on a throwaway builder is the honest test: it is the
        // step that opens the provider's library and its CUDA dependencies.
        let candidates: [(&'static str, bool, ep::ExecutionProviderDispatch); 2] = [
            (
                "CUDA",
                ep::CUDA::default().is_available().unwrap_or(false),
                ep::CUDA::default().build().error_on_failure(),
            ),
            (
                "OpenVINO",
                ep::OpenVINO::default().is_available().unwrap_or(false),
                ep::OpenVINO::default().build().error_on_failure(),
            ),
        ];
        for (name, built_in, provider) in candidates {
            if !built_in {
                probe.unavailable.push((
                    name.to_string(),
                    "this ONNX Runtime build does not include it".into(),
                ));
                continue;
            }
            match Session::builder()
                .and_then(|b| b.with_execution_providers([provider]).map_err(Into::into))
            {
                // The CUDA provider registers without cuDNN and fails on the
                // first convolution ("cuDNN is unavailable"), which a half
                // installed bundle would turn into a failed job instead of
                // a CPU session. Its absence is a reason, here.
                Ok(_) if name == "CUDA" && !cudnn_present() => probe.unavailable.push((
                    name.to_string(),
                    "with error: cuDNN 9 (libcudnn.so.9) not found; install the GPU bundle".into(),
                )),
                Ok(_) => {
                    probe.providers.push(name.to_string());
                    order.push(name);
                }
                Err(e) => probe.unavailable.push((name.to_string(), e.to_string())),
            }
        }
        // TensorRT is not in `order`: it is only for models with a fast
        // plan, and only in "Fast" mode, and always with CUDA behind it.
        let mut tensorrt = None;
        let cuda_works = order.contains(&"CUDA");
        let why_not = if mode != Acceleration::Fast {
            Some("off (Settings › AI acceleration › Fast)".to_string())
        } else if addon.is_none() {
            Some("not installed (`ml install tensorrt`)".to_string())
        } else if !cuda_works {
            Some("needs the CUDA provider, which does not work here".to_string())
        } else if !ep::TensorRT::default().is_available().unwrap_or(false) {
            Some("this ONNX Runtime build does not include it".to_string())
        } else {
            match Session::builder().and_then(|b| {
                b.with_execution_providers([ep::TensorRT::default().build().error_on_failure()])
                    .map_err(Into::into)
            }) {
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            }
        };
        match why_not {
            None => {
                probe.providers.insert(0, "TensorRT".to_string());
                tensorrt = Some(EngineTarget {
                    gpu: accel::nvidia_gpu_name().unwrap_or_else(|| "nvidia".into()),
                    driver: registry::nvidia_driver().unwrap_or_else(|| "unknown".into()),
                    tensorrt: registry::TENSORRT_VERSION.into(),
                    // Per model, from its plan; this is a placeholder.
                    precision: Precision::Fp32,
                });
            }
            Some(why) => probe.unavailable.push(("TensorRT".to_string(), why)),
        }
        probe.providers.push("CPU".to_string());
        order.push("CPU");
        probe.libraries = loaded_cuda_libraries();
        Ok(Runtime {
            probe,
            sessions: HashMap::new(),
            order,
            root: root.to_path_buf(),
            tensorrt,
            tensorrt_failed: HashSet::new(),
        })
    }

    /// The session for `spec`, created on first use on the best provider
    /// that accepts it.
    pub fn session(&mut self, spec: &'static ModelSpec) -> Result<&mut Loaded, Failure> {
        if !self.sessions.contains_key(spec.id) {
            let path = registry::model_path(&self.root, spec);
            if !registry::model_present(&self.root, spec) {
                return Err((
                    ErrorKind::ModelMissing,
                    format!("{} is not downloaded ({})", spec.name, path.display()),
                ));
            }
            let mut last_error = String::new();
            let mut loaded = None;
            for provider in self.order.clone() {
                match build(provider, &path, accel::tuning(spec.id)) {
                    Ok(session) => {
                        loaded = Some(Loaded {
                            session,
                            provider,
                            precision: Precision::Fp32,
                        });
                        break;
                    }
                    Err(e) => {
                        eprintln!("chukcut-ml-worker: {} on {provider} failed: {e}", spec.id);
                        last_error = e;
                    }
                }
            }
            let loaded = loaded.ok_or((ErrorKind::Inference, last_error))?;
            self.sessions.insert(spec.id.to_string(), loaded);
        }
        Ok(self.sessions.get_mut(spec.id).expect("inserted above"))
    }

    /// The session to run `spec` on inputs of `shapes` (every input with
    /// dimensions, by name; scalars left out): a TensorRT session built for
    /// exactly these shapes when the model has a fast plan and TensorRT
    /// works ("Fast" mode, the add-on installed), else [`Self::session`].
    /// A TensorRT engine that cannot be built leaves the model on CUDA for
    /// the rest of the worker's life, with a log line, never a failed job.
    pub fn session_for(
        &mut self,
        spec: &'static ModelSpec,
        shapes: &[(&str, Vec<i64>)],
    ) -> Result<&mut Loaded, Failure> {
        let plan = accel::fast_plan(spec.id);
        let (Some(plan), Some(target)) = (plan, self.tensorrt.clone()) else {
            return self.session(spec);
        };
        if self.tensorrt_failed.contains(spec.id) {
            return self.session(spec);
        }
        let shape_key = accel::shape_key(shapes);
        let key = format!("{}@{shape_key}", spec.id);
        if !self.sessions.contains_key(&key) {
            let path = registry::model_path(&self.root, spec);
            if !registry::model_present(&self.root, spec) {
                return self.session(spec);
            }
            let target = EngineTarget {
                precision: plan.precision,
                ..target
            };
            let dir = accel::engine_dir(&self.root, spec.id, spec.version, &target, &shape_key);
            let cached = accel::engine_present(&dir);
            if !cached {
                build_notice(
                    &format!(
                        "Preparing TensorRT for {} at this size (first time only, up to a few minutes)",
                        spec.name
                    ),
                    TENSORRT_BUILD_HOLD_SECS,
                );
            }
            let started = Instant::now();
            match build_tensorrt(&path, &dir, plan.precision, shapes, accel::tuning(spec.id)) {
                Ok(session) => {
                    let millis = started.elapsed().as_secs_f32() * 1000.0;
                    if !cached && accel::engine_present(&dir) {
                        let record = EngineBuild {
                            model: spec.id.to_string(),
                            version: spec.version.to_string(),
                            precision: plan.precision,
                            shapes: shape_key.clone(),
                            gpu: target.gpu.clone(),
                            driver: target.driver.clone(),
                            tensorrt: target.tensorrt.clone(),
                            millis,
                        };
                        if let Ok(json) = serde_json::to_vec_pretty(&record) {
                            let _ = std::fs::write(dir.join(accel::BUILD_RECORD), json);
                        }
                    }
                    eprintln!(
                        "chukcut-ml-worker: {} on TensorRT ({}, {shape_key}) in {millis:.0} ms{}",
                        spec.id,
                        plan.precision.as_str(),
                        if cached { ", cached engine" } else { "" }
                    );
                    self.sessions.insert(
                        key.clone(),
                        Loaded {
                            session,
                            provider: "TensorRT",
                            precision: plan.precision,
                        },
                    );
                }
                Err(e) => {
                    eprintln!(
                        "chukcut-ml-worker: {} on TensorRT failed, staying on CUDA: {e}",
                        spec.id
                    );
                    self.tensorrt_failed.insert(spec.id);
                    return self.session(spec);
                }
            }
        }
        Ok(self.sessions.get_mut(&key).expect("inserted above"))
    }

    /// Give up on TensorRT for `spec` after a run failed on it: its
    /// TensorRT sessions are dropped and the next [`Self::session_for`]
    /// answers the CUDA session.
    pub fn demote(&mut self, spec: &'static ModelSpec) {
        eprintln!(
            "chukcut-ml-worker: {} failed on TensorRT; running it on CUDA from now on",
            spec.id
        );
        self.tensorrt_failed.insert(spec.id);
        let prefix = format!("{}@", spec.id);
        self.sessions.retain(|k, _| !k.starts_with(&prefix));
    }

    /// The fast plan `spec` runs by in this worker: `Some` when TensorRT
    /// works here, the model has a plan and TensorRT has not failed on it.
    pub fn fast_plan_for(&self, spec: &ModelSpec) -> Option<accel::FastPlan> {
        self.tensorrt.as_ref()?;
        if self.tensorrt_failed.contains(spec.id) {
            return None;
        }
        accel::fast_plan(spec.id)
    }

    /// The sessions loaded so far, for a probe.
    pub fn sessions_info(&self) -> Vec<crate::protocol::SessionInfo> {
        let mut list: Vec<crate::protocol::SessionInfo> = self
            .sessions
            .iter()
            .map(|(key, loaded)| {
                let (model, shapes) = key.split_once('@').unwrap_or((key.as_str(), ""));
                crate::protocol::SessionInfo {
                    model: model.to_string(),
                    provider: loaded.provider.to_string(),
                    precision: loaded.precision.as_str().to_string(),
                    shapes: shapes.to_string(),
                }
            })
            .collect();
        list.sort_by(|a, b| (&a.model, &a.shapes).cmp(&(&b.model, &b.shapes)));
        list
    }
}

/// A TensorRT session for `model` at exactly `shapes`, its engine cached in
/// `dir`, with the CUDA provider behind it for any node TensorRT does not
/// take. The profile's minimum, optimum and maximum are the same shapes:
/// the engine is the fastest for them and is never rebuilt for another
/// size, which gets its own session and directory instead (a clip has one
/// size; a dynamic profile cost speed and rebuilt on every change).
fn build_tensorrt(
    model: &Path,
    dir: &Path,
    precision: Precision,
    shapes: &[(&str, Vec<i64>)],
    tuning: accel::Tuning,
) -> Result<Session, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir_str = dir.to_string_lossy().to_string();
    let profile: Vec<String> = shapes
        .iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(name, s)| accel::profile_entry(name, s))
        .collect();
    let profile = profile.join(",");
    let mut trt = ep::TensorRT::default()
        .with_fp16(precision == Precision::Fp16)
        .with_engine_cache(true)
        .with_engine_cache_path(&dir_str)
        .with_timing_cache(true)
        .with_timing_cache_path(&dir_str);
    if !profile.is_empty() {
        trt = trt
            .with_profile_min_shapes(&profile)
            .with_profile_opt_shapes(&profile)
            .with_profile_max_shapes(&profile);
    }
    let mut builder = Session::builder()
        .map_err(|e| e.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| e.to_string())?
        .with_intra_threads(INTRA_THREADS)
        .map_err(|e| e.to_string())?;
    builder = tune(builder, tuning)?;
    builder
        .with_execution_providers([
            trt.build().error_on_failure(),
            ep::CUDA::default().build().error_on_failure(),
        ])
        .map_err(|e| e.to_string())?
        .commit_from_file(model)
        .map_err(|e| e.to_string())
}

/// Apply a model's [`accel::Tuning`].
fn tune(
    builder: ort::session::builder::SessionBuilder,
    tuning: accel::Tuning,
) -> Result<ort::session::builder::SessionBuilder, String> {
    if tuning.no_constant_folding {
        builder
            .with_config_entry(
                "optimization.disable_specified_optimizers",
                "ConstantFolding",
            )
            .map_err(|e| e.to_string())
    } else {
        Ok(builder)
    }
}

fn build(provider: &str, model: &Path, tuning: accel::Tuning) -> Result<Session, String> {
    let started = Instant::now();
    let mut builder = Session::builder()
        .map_err(|e| e.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| e.to_string())?
        .with_intra_threads(INTRA_THREADS)
        .map_err(|e| e.to_string())?;
    builder = tune(builder, tuning)?;
    builder = match provider {
        "CUDA" => builder
            .with_execution_providers([ep::CUDA::default().build().error_on_failure()])
            .map_err(|e| e.to_string())?,
        "OpenVINO" => builder
            .with_execution_providers([ep::OpenVINO::default().build().error_on_failure()])
            .map_err(|e| e.to_string())?,
        _ => builder,
    };
    let session = builder.commit_from_file(model).map_err(|e| e.to_string())?;
    eprintln!(
        "chukcut-ml-worker: {} on {provider} in {:.0} ms",
        model.display(),
        started.elapsed().as_secs_f32() * 1000.0
    );
    Ok(session)
}
