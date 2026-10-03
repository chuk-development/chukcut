//! What the ML worker can run, where each piece comes from, and where it
//! lives on disk.
//!
//! Two kinds of download, both pinned and checked:
//!
//! - **Models**: ONNX files. Each URL names an immutable revision (a Git
//!   commit, never a branch), and the SHA-256 is the Git LFS object id at that
//!   commit, which *is* the SHA-256 of the file. Read from
//!   `raw.githubusercontent.com/opencv/opencv_zoo/<commit>/…` on 2026-10-03.
//! - **Runtime packs**: ONNX Runtime itself, as Microsoft publishes it on
//!   GitHub. The worker loads it at run time (`ort`'s `load-dynamic`), so
//!   neither the editor nor the worker binary carries a gigabyte of CUDA
//!   kernels, and a machine without the pack simply runs without ML. The
//!   SHA-256 values are the digests GitHub's release API reports for the
//!   assets of `v1.28.3`, read on 2026-10-03.
//!
//! **Licence gate.** Every model names its licence and whether it may be used
//! in a feature someone pays for (`commercial_ok`). The research
//! (`docs/research/ml-features.md` §1, §7.4) rules out non-commercial weights
//! because they would block a paid tier later and add a use restriction next to
//! the GPL. The engine refuses to download a model with `commercial_ok: false`
//! unless a caller asks for it by name with `allow_noncommercial`, so such a
//! model can never arrive as a side effect of an ordinary feature.
//!
//! **Legal boundary.** This file lists third-party URLs; the repository never
//! contains weights or runtime libraries.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What a model is for. The worker uses it to pick the pre- and
/// post-processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// Faces: boxes, five landmarks, a score.
    DetectFaces,
    /// Single-object tracking: a box in, a box per frame out.
    TrackBox,
}

/// One downloadable model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ModelSpec {
    pub id: &'static str,
    pub version: &'static str,
    pub name: &'static str,
    pub task: Task,
    /// SPDX identifier of the licence that covers code **and** weights.
    pub licence: &'static str,
    /// May a paid feature use it? See the module docs.
    pub commercial_ok: bool,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    /// The file name inside the model's directory.
    pub file: &'static str,
    /// Providers this model has been run on and checked against a
    /// reference. A provider not listed is still tried, but a wrong result
    /// there is a bug report, not a surprise.
    pub providers_tested: &'static [&'static str],
}

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "yunet",
        version: "2026may",
        name: "YuNet face detector",
        task: Task::DetectFaces,
        licence: "MIT",
        commercial_ok: true,
        // The 2026 re-export of the 2023mar weights with symbolic height and
        // width, so one session runs at any frame size (the 2023mar file is
        // fixed at 640x640 and would need a resize per frame).
        url: "https://media.githubusercontent.com/media/opencv/opencv_zoo/26cc381e4d2594bb9f47a26eb8fd96c94a13660d/models/face_detection_yunet/face_detection_yunet_2026may.onnx",
        sha256: "ebafce4e3c118d6554634be5c27ab333b4c047a9a8c3faf1d7cf93101c22f0f0",
        bytes: 229_738,
        file: "face_detection_yunet_2026may.onnx",
        providers_tested: &["CPU", "CUDA"],
    },
    ModelSpec {
        id: "vittrack",
        version: "2023sep",
        name: "VitTrack object tracker",
        task: Task::TrackBox,
        licence: "Apache-2.0",
        commercial_ok: true,
        url: "https://media.githubusercontent.com/media/opencv/opencv_zoo/25f423d0e04c31a17254620e58febd7386da523b/models/object_tracking_vittrack/object_tracking_vittrack_2023sep.onnx",
        sha256: "2990f0b7cd44d92afa48cd97db6de7be113fc1d9594fddb74e2725c10478e91d",
        bytes: 714_726,
        file: "object_tracking_vittrack_2023sep.onnx",
        providers_tested: &["CPU", "CUDA"],
    },
];

/// The model called `id`.
pub fn model(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}

/// Where `spec` lives under the ML root (`<cache>/chukcut/ml`):
/// `models/<id>/<version>/<file>`. The version is part of the path so a
/// project analysed with one version is never silently re-run with another.
pub fn model_path(root: &Path, spec: &ModelSpec) -> PathBuf {
    root.join("models")
        .join(spec.id)
        .join(spec.version)
        .join(spec.file)
}

/// Present at its full size. The checksum is verified once, on download.
pub fn model_present(root: &Path, spec: &ModelSpec) -> bool {
    std::fs::metadata(model_path(root, spec)).is_ok_and(|m| m.len() == spec.bytes)
}

/// One ONNX Runtime build.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RuntimePack {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// Size of the archive (the download), not of what is kept.
    pub bytes: u64,
    /// Providers this build can offer beyond the CPU.
    pub providers: &'static [&'static str],
    /// Shared libraries it needs from elsewhere (the system, or a directory
    /// named in `CHUKCUT_CUDA_LIB_DIRS`), for the settings page to explain a
    /// GPU that does not light up.
    pub needs: &'static str,
}

/// The ONNX Runtime version `ort` 2.0.0-rc.13 is written against. A newer
/// 1.x would load too (the C API is versioned), an older one would not.
pub const ORT_VERSION: &str = "1.28.3";

pub const RUNTIME_PACKS: &[RuntimePack] = &[
    RuntimePack {
        id: "cpu",
        name: "ONNX Runtime (CPU)",
        version: ORT_VERSION,
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-1.28.3.tgz",
        sha256: "db14e4863bd37893fc59729d986ab2a0d043d10b7d44da1913c4982b7e3d009c",
        bytes: 9_130_098,
        providers: &[],
        needs: "",
    },
    RuntimePack {
        id: "cuda12",
        name: "ONNX Runtime for NVIDIA (CUDA 12)",
        version: ORT_VERSION,
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-gpu_cuda12-1.28.3.tgz",
        sha256: "4e16d2ec66521a24fe917bd848eb8ae3a9107cb8b4aec978fb2b3ab1a640fcb7",
        bytes: 423_745_424,
        providers: &["CUDA"],
        needs: "CUDA 12 runtime (cudart, cuBLAS, cuFFT, cuRAND, NVRTC) and cuDNN 9",
    },
    RuntimePack {
        id: "cuda13",
        name: "ONNX Runtime for NVIDIA (CUDA 13)",
        version: ORT_VERSION,
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-gpu_cuda13-1.28.3.tgz",
        sha256: "33e91f819324449bede2d1e853c8d46bb6253fa8890bac98ae676f041c8f73a6",
        bytes: 240_886_111,
        providers: &["CUDA"],
        needs: "CUDA 13 runtime (cudart, cuBLAS, cuFFT, cuRAND, NVRTC) and cuDNN 9 for CUDA 13",
    },
];

/// The pack called `id`.
pub fn runtime_pack(id: &str) -> Option<&'static RuntimePack> {
    RUNTIME_PACKS.iter().find(|p| p.id == id)
}

/// Where `pack` is unpacked: `runtime/<id>-<version>/`, holding `lib/`.
pub fn runtime_dir(root: &Path, pack: &RuntimePack) -> PathBuf {
    root.join("runtime")
        .join(format!("{}-{}", pack.id, pack.version))
}

/// The library to load from an unpacked `pack`.
pub fn runtime_library(root: &Path, pack: &RuntimePack) -> PathBuf {
    runtime_dir(root, pack)
        .join("lib")
        .join("libonnxruntime.so")
}

pub fn runtime_present(root: &Path, pack: &RuntimePack) -> bool {
    runtime_library(root, pack).is_file()
}

/// Which files of a runtime archive to keep, by their path inside it. Only
/// the libraries: headers, docs and the TensorRT provider (which needs a
/// TensorRT install nobody has by accident, and is 1 GB of the archive's
/// contents) stay behind.
pub fn keep_from_runtime_archive(path_in_archive: &str) -> Option<&str> {
    let name = path_in_archive.rsplit('/').next()?;
    let in_lib = path_in_archive.contains("/lib/");
    (in_lib
        && name.starts_with("libonnxruntime")
        && name.contains(".so")
        && !name.contains("tensorrt"))
    .then_some(name)
}

/// The installed pack the worker should load: the first GPU pack that is
/// present, else the CPU pack. A GPU pack carries the CPU provider too, so a
/// machine with one never needs the other.
pub fn preferred_runtime(root: &Path) -> Option<&'static RuntimePack> {
    ["cuda13", "cuda12", "cpu"]
        .iter()
        .filter_map(|id| runtime_pack(id))
        .find(|p| runtime_present(root, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_download_is_pinned_and_checked() {
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(m.url.starts_with("https://"));
            // A branch name would let the file change under the checksum.
            assert!(
                !m.url.contains("/main/") && !m.url.contains("/master/"),
                "{}",
                m.url
            );
            assert!(m.url.ends_with(m.file));
            assert!(m.bytes > 0);
        }
        for p in RUNTIME_PACKS {
            assert_eq!(p.sha256.len(), 64, "{}", p.id);
            assert!(p.url.contains(&format!("/v{}/", p.version)), "{}", p.url);
        }
    }

    #[test]
    fn every_shipped_model_passes_the_licence_gate() {
        // Adding a non-commercial model is a decision, not an accident: it
        // needs this test changed and a line in docs/decisions.
        for m in MODELS {
            assert!(m.commercial_ok, "{} is not commercial_ok", m.id);
            assert!(!m.licence.contains("NC"), "{}", m.licence);
        }
    }

    #[test]
    fn the_archive_filter_keeps_only_the_libraries() {
        let keep = |p| keep_from_runtime_archive(p);
        assert_eq!(
            keep("onnxruntime-linux-x64-gpu_cuda12-1.28.3/lib/libonnxruntime.so.1.28.3"),
            Some("libonnxruntime.so.1.28.3")
        );
        assert_eq!(
            keep("onnxruntime-linux-x64-1.28.3/lib/libonnxruntime_providers_shared.so"),
            Some("libonnxruntime_providers_shared.so")
        );
        assert!(keep(
            "onnxruntime-linux-x64-gpu_cuda12-1.28.3/lib/libonnxruntime_providers_tensorrt.so"
        )
        .is_none());
        assert!(keep("onnxruntime-linux-x64-1.28.3/include/onnxruntime_c_api.h").is_none());
        assert!(
            keep("onnxruntime-linux-x64-1.28.3/lib/cmake/onnxruntime/onnxruntimeConfig.cmake")
                .is_none()
        );
        assert!(keep("onnxruntime-linux-x64-1.28.3/lib/pkgconfig/libonnxruntime.pc").is_none());
    }

    #[test]
    fn a_gpu_pack_wins_over_the_cpu_pack() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/ml-registry");
        let _ = std::fs::remove_dir_all(&root);
        assert!(preferred_runtime(&root).is_none());
        for id in ["cpu", "cuda12"] {
            let lib = runtime_library(&root, runtime_pack(id).unwrap());
            std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
            std::fs::write(&lib, b"").unwrap();
        }
        assert_eq!(preferred_runtime(&root).unwrap().id, "cuda12");
        let _ = std::fs::remove_dir_all(&root);
    }
}
