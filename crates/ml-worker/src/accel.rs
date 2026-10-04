//! How each model is accelerated: the provider and precision it runs at,
//! TensorRT's engine cache, and per-model session settings.
//!
//! Decision 0031. Two modes, chosen in Settings › AI acceleration ("Fast
//! (fp16/TensorRT)") and passed to the worker when it starts:
//!
//! - **Standard**: every model on the best provider that accepts it (CUDA,
//!   OpenVINO, the CPU), in fp32, as before.
//! - **Fast**: models with a [`fast_plan`] run on TensorRT when the TensorRT
//!   add-on is installed (`registry::TENSORRT`), at the precision the plan
//!   names, falling back to CUDA when TensorRT cannot build or run them.
//!   Every plan was measured against fp32 on the CUDA provider for speed
//!   and for quality (`docs/STATUS.md`, "Faster AI on NVIDIA"); a model
//!   whose fp16 output drifted keeps fp32 inside TensorRT, and a model that
//!   gained nothing has no plan.
//!
//! **fp16 files.** No fp16 export with a usable licence exists for RIFE,
//! LaMa or Real-ESRGAN, and converting at install time
//! (`onnxconverter-common`'s algorithm) failed on RIFE and LaMa (Cast nodes
//! typed wrong) and gained 18 % on Real-ESRGAN where TensorRT gained 2–3x;
//! TensorRT's fp16 builder makes fp16 from the fp32 files the app already
//! has, so no second file is downloaded.
//!
//! **Engines.** TensorRT compiles a model for one GPU, driver, TensorRT
//! version, precision and input size; compiling takes 30 s to a few
//! minutes, so engines are cached on disk under
//! `<ml root>/tensorrt/<model>-<version>/<gpu>-<driver>-trt<version>-<precision>/<shape>/`
//! ([`engine_dir`]) with a `build.json` recording how long the build took
//! ([`EngineBuild`]), which `ml status` and the settings page show. A new
//! driver or GPU gets a new directory and builds again; nothing stale is
//! ever loaded.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The worker's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acceleration {
    #[default]
    Standard,
    Fast,
}

impl Acceleration {
    pub fn parse(s: &str) -> Option<Acceleration> {
        match s {
            "standard" => Some(Acceleration::Standard),
            "fast" => Some(Acceleration::Fast),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Acceleration::Standard => "standard",
            Acceleration::Fast => "fast",
        }
    }
}

/// The arithmetic a session runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    Fp32,
    Fp16,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Fp32 => "fp32",
            Precision::Fp16 => "fp16",
        }
    }
}

/// What "Fast" does for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FastPlan {
    /// The precision TensorRT builds at.
    pub precision: Precision,
}

/// The fast plan of model `id`, or `None` when it stays on CUDA in fp32
/// in either mode. Measured on an RTX 3060 (`docs/STATUS.md`, "Faster AI on
/// NVIDIA"); a change here is a change to a measured claim.
pub fn fast_plan(id: &str) -> Option<FastPlan> {
    let precision = match id {
        // RIFE in fp32 (TF32 on Ampere and newer). In fp16 it was 2.5x
        // faster but warped by a grid whose coordinates fp16 cannot hold
        // to a pixel at 1080p: 3 % of the values off by more than 8, PSNR
        // 35.5 dB against fp32. In fp32 TensorRT is 66 dB from the CUDA
        // provider and 1.3–1.9x faster.
        "rife" => Precision::Fp32,
        // Real-ESRGAN: 34 convolutions, the case fp16 was made for: 2.3x,
        // 57.7 dB, no value off by more than 6.
        "realesr-general-x4v3" => Precision::Fp16,
        // LaMa: 2.1x in fp16. Outside the hole the output is identical;
        // inside, the invented fill differs by 2 code values on average
        // (38.6 dB), which side by side is not visible.
        "lama" => Precision::Fp16,
        // BiRefNet lite: 2.65x in fp32 (IoU 0.9998 against the CUDA
        // provider). Its engine takes ~5.5 min and 5 GB of memory to build.
        "birefnet-lite" => Precision::Fp32,
        // RVM: TensorRT's parser refuses its Resize, whose scale comes from
        // the `downsample_ratio` input; it stays on CUDA (15 ms a frame).
        _ => return None,
    };
    Some(FastPlan { precision })
}

/// Session settings a model needs whatever the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tuning {
    /// Turn off ONNX Runtime's constant folding. HTDemucs's export computes
    /// its shapes with ~700 ScatterND/Expand/Range chains on a fixed input
    /// size, which folding turns into gigabytes of constants: the worker
    /// peaked at 6.8 GB (CUDA) and 8.5 GB (CPU) while the session was
    /// created. Without folding it peaks at 1.5 GB and 2.5 GB, runs at the
    /// same speed, and its output differs by at most 8e-6.
    pub no_constant_folding: bool,
}

pub fn tuning(id: &str) -> Tuning {
    Tuning {
        no_constant_folding: id == "htdemucs-vocals",
    }
}

/// A model input's shape as a TensorRT profile entry, `name:1x3x360x640`.
pub fn profile_entry(name: &str, shape: &[i64]) -> String {
    let dims: Vec<String> = shape.iter().map(i64::to_string).collect();
    format!("{name}:{}", dims.join("x"))
}

/// A directory name for a set of input shapes: `input-1x6x360x640`.
pub fn shape_key(shapes: &[(&str, Vec<i64>)]) -> String {
    let parts: Vec<String> = shapes
        .iter()
        .map(|(name, shape)| {
            let dims: Vec<String> = shape.iter().map(i64::to_string).collect();
            format!("{}-{}", slug(name), dims.join("x"))
        })
        .collect();
    parts.join("_")
}

/// Lower-case letters, digits and dashes only.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// What an engine is built for besides the model and its shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineTarget {
    /// The GPU's name, e.g. "NVIDIA GeForce RTX 3060".
    pub gpu: String,
    /// The NVIDIA driver's version.
    pub driver: String,
    pub tensorrt: String,
    pub precision: Precision,
}

impl EngineTarget {
    fn key(&self) -> String {
        format!(
            "{}-{}-trt{}-{}",
            slug(&self.gpu),
            slug(&self.driver),
            slug(&self.tensorrt),
            self.precision.as_str()
        )
    }
}

/// The root of every cached TensorRT engine.
pub fn engines_root(root: &Path) -> PathBuf {
    root.join("tensorrt")
}

/// Where the engine of `model` (id and version) for `target` and one set
/// of input shapes lives.
pub fn engine_dir(
    root: &Path,
    model: &str,
    version: &str,
    target: &EngineTarget,
    shapes: &str,
) -> PathBuf {
    engines_root(root)
        .join(format!("{}-{}", slug(model), slug(version)))
        .join(target.key())
        .join(shapes)
}

/// The NVIDIA GPU's name from the driver's `/proc` files (the first GPU,
/// which is the one ONNX Runtime uses as device 0).
pub fn nvidia_gpu_name() -> Option<String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("/proc/driver/nvidia/gpus")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let text = std::fs::read_to_string(dirs.first()?.join("information")).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("Model:"))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// One TensorRT engine build, as recorded beside the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineBuild {
    pub model: String,
    pub version: String,
    pub precision: Precision,
    /// The input shapes, as [`shape_key`] wrote them.
    pub shapes: String,
    pub gpu: String,
    pub driver: String,
    pub tensorrt: String,
    /// How long building took, the first time.
    pub millis: f32,
}

/// The file in an engine directory that records its build.
pub const BUILD_RECORD: &str = "build.json";

/// Every engine built under `root`, from their records.
pub fn engine_builds(root: &Path) -> Vec<EngineBuild> {
    let mut found = Vec::new();
    let mut stack = vec![engines_root(root)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == BUILD_RECORD) {
                if let Some(build) = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<EngineBuild>(&b).ok())
                {
                    found.push(build);
                }
            }
        }
    }
    found.sort_by(|a, b| (&a.model, &a.shapes).cmp(&(&b.model, &b.shapes)));
    found
}

/// Whether `dir` holds a built engine (TensorRT's provider names its file
/// `…_<precision>_sm<arch>.engine`).
pub fn engine_present(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "engine"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_directories_name_everything_an_engine_depends_on() {
        let target = EngineTarget {
            gpu: "NVIDIA GeForce RTX 3060".into(),
            driver: "610.57.04".into(),
            tensorrt: "10.16.1.11".into(),
            precision: Precision::Fp16,
        };
        let shapes = shape_key(&[("input", vec![1, 6, 360, 640])]);
        assert_eq!(shapes, "input-1x6x360x640");
        let dir = engine_dir(Path::new("/ml"), "rife", "v4-fp32", &target, &shapes);
        assert_eq!(
            dir,
            Path::new(
                "/ml/tensorrt/rife-v4-fp32/nvidia-geforce-rtx-3060-610-57-04-trt10-16-1-11-fp16/input-1x6x360x640"
            )
        );
        // Another driver is another directory: an engine is never loaded
        // on a driver it was not built with.
        let other = EngineTarget {
            driver: "615.10".into(),
            ..target.clone()
        };
        assert_ne!(
            engine_dir(Path::new("/ml"), "rife", "v4-fp32", &other, &shapes),
            dir
        );
        assert_eq!(profile_entry("mask", &[1, 1, 512, 512]), "mask:1x1x512x512");
    }

    #[test]
    fn only_measured_models_have_a_fast_plan() {
        assert_eq!(fast_plan("rife").unwrap().precision, Precision::Fp32);
        assert_eq!(
            fast_plan("realesr-general-x4v3").unwrap().precision,
            Precision::Fp16
        );
        assert!(fast_plan("rvm").is_none());
        assert!(fast_plan("yunet").is_none());
        assert!(tuning("htdemucs-vocals").no_constant_folding);
        assert!(!tuning("rife").no_constant_folding);
        assert_eq!(Acceleration::parse("fast"), Some(Acceleration::Fast));
        assert_eq!(Acceleration::parse("fast").unwrap().as_str(), "fast");
    }

    #[test]
    fn build_records_are_found_wherever_they_lie() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/ml-accel/builds");
        let _ = std::fs::remove_dir_all(&root);
        let target = EngineTarget {
            gpu: "GPU".into(),
            driver: "1".into(),
            tensorrt: "10".into(),
            precision: Precision::Fp32,
        };
        let dir = engine_dir(&root, "lama", "v", &target, "image-1x3x512x512");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!engine_present(&dir));
        std::fs::write(dir.join("x_fp32_sm86.engine"), b"").unwrap();
        assert!(engine_present(&dir));
        let build = EngineBuild {
            model: "lama".into(),
            version: "v".into(),
            precision: Precision::Fp32,
            shapes: "image-1x3x512x512".into(),
            gpu: "GPU".into(),
            driver: "1".into(),
            tensorrt: "10".into(),
            millis: 61_000.0,
        };
        std::fs::write(dir.join(BUILD_RECORD), serde_json::to_vec(&build).unwrap()).unwrap();
        assert_eq!(engine_builds(&root), vec![build]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
