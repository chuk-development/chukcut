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
/// loader path: `CHUKCUT_CUDA_LIB_DIRS` (colon-separated) and the runtime
/// pack's own `lib/` (where a future cuDNN pack would unpack to).
fn cuda_dirs(lib: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("CHUKCUT_CUDA_LIB_DIRS")
        .map(|v| std::env::split_paths(&v).collect())
        .unwrap_or_default();
    if let Some(parent) = lib.parent() {
        dirs.push(parent.to_path_buf());
    }
    dirs
}

/// Load each CUDA and cuDNN library the CUDA provider will ask for from the
/// first of `dirs` that has it. A library loaded this way is found by its
/// soname when the provider opens it later, so the user's loader path does
/// not have to change. Libraries found nowhere are left to the system loader.
fn preload_cuda(dirs: &[PathBuf]) {
    for name in ep::cuda::CUDA_DYLIBS.iter().chain(ep::cuda::CUDNN_DYLIBS) {
        if let Some(path) = dirs.iter().map(|d| d.join(name)).find(|p| p.is_file()) {
            if let Err(e) = ort::util::preload_dylib(&path) {
                eprintln!(
                    "chukcut-ml-worker: could not preload {}: {e}",
                    path.display()
                );
            }
        }
    }
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
        preload_cuda(&cuda_dirs(&lib));
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
        // "ORT Build Info: git-branch=rel-1.28.3, git-commit-id=…, …"
        let runtime_version = info
            .split(", ")
            .find_map(|part| part.split("git-branch=rel-").nth(1))
            .unwrap_or("unknown")
            .to_string();
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
                Ok(_) => {
                    probe.providers.push(name.to_string());
                    order.push(name);
                }
                Err(e) => probe.unavailable.push((name.to_string(), e.to_string())),
            }
        }
        probe.providers.push("CPU".to_string());
        order.push("CPU");
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
