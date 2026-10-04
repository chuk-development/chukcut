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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ort::ep::{self, ExecutionProvider as _};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

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
    sessions: HashMap<&'static str, Loaded>,
    /// Providers to try for a new session, best first.
    order: Vec<&'static str>,
    root: PathBuf,
}

pub struct Loaded {
    pub session: Session,
    pub provider: &'static str,
}

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
    for prefix in CUDA_PRELOAD {
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
    pub fn load(root: &Path) -> Result<Runtime, Failure> {
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
        preload_cuda(&cuda_dirs(root, &lib));
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
        probe.providers.push("CPU".to_string());
        order.push("CPU");
        probe.libraries = loaded_cuda_libraries();
        Ok(Runtime {
            probe,
            sessions: HashMap::new(),
            order,
            root: root.to_path_buf(),
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
                match build(provider, &path) {
                    Ok(session) => {
                        loaded = Some(Loaded { session, provider });
                        break;
                    }
                    Err(e) => {
                        eprintln!("chukcut-ml-worker: {} on {provider} failed: {e}", spec.id);
                        last_error = e;
                    }
                }
            }
            let loaded = loaded.ok_or((ErrorKind::Inference, last_error))?;
            self.sessions.insert(spec.id, loaded);
        }
        Ok(self.sessions.get_mut(spec.id).expect("inserted above"))
    }
}

fn build(provider: &str, model: &Path) -> Result<Session, String> {
    let started = Instant::now();
    let mut builder = Session::builder()
        .map_err(|e| e.to_string())?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| e.to_string())?
        .with_intra_threads(INTRA_THREADS)
        .map_err(|e| e.to_string())?;
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
