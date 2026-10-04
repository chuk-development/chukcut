//! What the ML worker can run, where each piece comes from, and where it
//! lives on disk.
//!
//! Two kinds of download, both pinned and checked:
//!
//! - **Models**: ONNX files. Each URL names an immutable revision (a Git
//!   commit, never a branch), and the SHA-256 is the Git LFS object id at that
//!   commit, which *is* the SHA-256 of the file. Read from
//!   `raw.githubusercontent.com/opencv/opencv_zoo/<commit>/…` on 2026-10-03.
//!   RVM is a GitHub release asset of tag `v1.0.0` (commit `17d1774`); a
//!   release asset can be replaced by its owner, which the SHA-256 (taken
//!   from the download on 2026-10-04) would catch.
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
    /// Source separation: a stretch of stereo sound in, its voice out
    /// (HTDemucs).
    Separate,
    /// Dense face landmarks: a face crop in, 478 points out (MediaPipe face
    /// mesh), on faces YuNet found.
    FaceLandmarks,
    /// Person matting: an alpha matte per frame, recurrent over a run of
    /// frames.
    Matte,
    /// Matting one frame at a time with no state: the main object of the
    /// picture (BiRefNet).
    MatteImage,
    /// The image half of a promptable segmenter (SAM): a frame in, an
    /// embedding out. Its `companion` is the decoder.
    SegmentEncoder,
    /// The prompt half: an embedding and clicks in, a mask out.
    SegmentDecoder,
    /// Frame interpolation: two frames and a phase in, the frame between
    /// them out (RIFE).
    Interpolate,
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
    /// Whether a feature may run it on the CPU. BiRefNet at 1024² takes
    /// 12–25 s and 6–11 GB of memory per frame on the CPU, which for video
    /// is a hang, not a slow path.
    pub cpu_ok: bool,
    /// A second model that is downloaded and used with this one (SAM's
    /// decoder for its encoder).
    pub companion: Option<&'static str>,
}

/// The sample rate source separation runs at: HTDemucs was trained on
/// 44.1 kHz stereo music.
pub const SEPARATION_RATE: u32 = 44_100;
/// Frames per channel the separation model takes at once: HTDemucs's
/// training segment, 7.8 s. The export's input is fixed at this length.
pub const SEPARATION_SEGMENT: usize = 343_980;

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
        cpu_ok: true,
        companion: None,
    },
    // HTDemucs fine-tuned, the vocals specialist (Défossez, "Hybrid
    // Transformers for Music Source Separation", 2022;
    // github.com/facebookresearch/demucs, MIT, weights released under it),
    // for "Isolate voice". StemSplitio's ONNX export: the STFT and its
    // inverse are inside the graph as convolutions, so it is waveform in,
    // waveform out, `mix` [1, 2, 343980] at 44.1 kHz to `stems`
    // [1, 4, 2, 343980] (drums, bass, other, vocals; only vocals is trained
    // in this specialist). Chosen over the Mel-Band RoFormer (better, but
    // 953 MB and ~10x slower) and over UVR's MDX-Net models (the weights'
    // licence is a README sentence). Hugging Face commit 2ef0d75; SHA-256 =
    // LFS object id, checked against the download on 2026-10-04.
    ModelSpec {
        id: "htdemucs-vocals",
        version: "ft-vocals-2ef0d75",
        name: "HTDemucs (isolate voice)",
        task: Task::Separate,
        licence: "MIT",
        commercial_ok: true,
        url: "https://huggingface.co/StemSplitio/htdemucs-ft-vocals-onnx/resolve/2ef0d757d3e226d0da85fb8c71514f464fcabdd0/htdemucs_ft_vocals.onnx",
        sha256: "8c5d5e2da1f27050240bb80236673307ee3b40d4b064066d9350f4d64bfd544d",
        bytes: 316_446_953,
        file: "htdemucs_ft_vocals.onnx",
        providers_tested: &["CPU", "CUDA"],
        // About half real time on eight CPU threads: slow, bounded.
        cpu_ok: true,
        companion: None,
    },
    // MediaPipe Face Landmarker v2's face mesh (Google, Apache-2.0 per its
    // model card, "Model Card MediaPipe Face Mesh V2"): 478 points from a
    // 256x256 face crop, plus a face-presence logit. The ONNX file is
    // naklitechie/face-landmarks-onnx (Apache-2.0), a tf2onnx conversion of
    // face_landmarks_detector.tflite from face_landmarker.task; input
    // `input_12` [N, 256, 256, 3] NHWC RGB 0..1, outputs `Identity`
    // [N, 1, 1, 1434] (x, y, z in crop pixels) and `Identity_1` (the
    // logit). Three other conversions on Hugging Face give the same numbers.
    // Faces are found by YuNet. Commit 575c338; SHA-256 = LFS object id.
    ModelSpec {
        id: "facemesh",
        version: "v2-478-575c338",
        name: "MediaPipe face mesh (face landmarks)",
        task: Task::FaceLandmarks,
        licence: "Apache-2.0",
        commercial_ok: true,
        url: "https://huggingface.co/naklitechie/face-landmarks-onnx/resolve/575c33816c840c5b156398adb485e4f8d138adc2/face_landmarks.onnx",
        sha256: "f38c3321ceffbc9e95103480ad38cc3f52e7e1bde2bcee7cd9355d0b9138ac0c",
        bytes: 4_920_995,
        file: "face_landmarks.onnx",
        providers_tested: &["CPU", "CUDA"],
        cpu_ok: true,
        companion: Some("yunet"),
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
        cpu_ok: true,
        companion: None,
    },
    ModelSpec {
        id: "rvm",
        version: "1.0.0-mobilenetv3",
        name: "Robust Video Matting (people)",
        task: Task::Matte,
        // The repository's licence, which covers the released weights; GPL
        // allows commercial use, and chukcut is GPL itself (decision 0010).
        licence: "GPL-3.0",
        commercial_ok: true,
        url: "https://github.com/PeterL1n/RobustVideoMatting/releases/download/v1.0.0/rvm_mobilenetv3_fp32.onnx",
        sha256: "88d4531297118f595bf2fd60f6f566aec2e559393802d1f436c380f0cbbd2828",
        bytes: 14_975_696,
        file: "rvm_mobilenetv3_fp32.onnx",
        // CUDA mattes differ from the CPU's by 1.3/255 on average over a
        // 90-frame talking head (recurrent state carries float rounding).
        providers_tested: &["CPU", "CUDA"],
        cpu_ok: true,
        companion: None,
    },
    // BiRefNet lite (Zheng et al., 2024; github.com/ZhengPeng7/BiRefNet,
    // MIT, weights included), the ONNX export onnx-community publishes for
    // transformers.js: fp32, a fixed 1024×1024 input normalised with
    // ImageNet's mean and deviation, one logit map out. Their fp16 file
    // (115 MB) was 10 % faster on an RTX 3060 (385 against 426 ms) but has
    // no CPU kernels for some nodes and risks fp16 overflow in attention;
    // not worth it. Hugging Face commit
    // de15b22; the SHA-256 is the LFS object id there, read 2026-10-04.
    ModelSpec {
        id: "birefnet-lite",
        version: "lite-1024-fp32",
        name: "BiRefNet lite (objects)",
        task: Task::MatteImage,
        licence: "MIT",
        commercial_ok: true,
        url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
        sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
        bytes: 224_005_088,
        file: "model.onnx",
        providers_tested: &["CPU", "CUDA"],
        cpu_ok: false,
        companion: None,
    },
    // MobileSAM (Zhang et al., 2023; github.com/ChaoningZhang/MobileSAM,
    // Apache-2.0, weights included): Segment Anything's prompt decoder on a
    // TinyViT image encoder, 60× smaller than SAM's. The ONNX files are
    // Acly's export (huggingface.co/Acly/MobileSAM, MIT) at commit 0d3b403;
    // the encoder takes the frame as HWC RGB 0..255 with its long side at
    // 1024 and normalises and pads it itself. SHA-256 = LFS object ids.
    ModelSpec {
        id: "mobilesam",
        version: "acly-0d3b403",
        name: "MobileSAM (select an object)",
        task: Task::SegmentEncoder,
        licence: "Apache-2.0",
        commercial_ok: true,
        url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/mobile_sam_image_encoder.onnx",
        sha256: "580f5fb648ea1062c0aabc26217aed56921985f03f0cbbd852bba81d760cc749",
        bytes: 28_157_093,
        file: "mobile_sam_image_encoder.onnx",
        providers_tested: &["CPU", "CUDA"],
        cpu_ok: true,
        companion: Some("mobilesam-decoder"),
    },
    ModelSpec {
        id: "mobilesam-decoder",
        version: "acly-0d3b403",
        name: "Segment Anything mask decoder (for MobileSAM)",
        task: Task::SegmentDecoder,
        licence: "Apache-2.0",
        commercial_ok: true,
        url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/sam_mask_decoder_single.onnx",
        sha256: "93915fc7c993ab9d59ab8c9ccd3bce37f7509c81ab4150a74abd4d2abbd8570d",
        bytes: 16_501_323,
        file: "sam_mask_decoder_single.onnx",
        providers_tested: &["CPU", "CUDA"],
        cpu_ok: true,
        companion: None,
    },
    // RIFE v4 (Huang et al., ECCV 2022; github.com/hzwer/Practical-RIFE,
    // MIT, weights included) for "Optical flow (AI)" frame blending. The
    // ONNX file is walterlow/RIFE_fp32_timestep (MIT), FuryTMP/RIFE_fp32's
    // export with its baked t = 0.5 exposed as a `timestep` input, so one
    // session makes any phase. Chosen over yuvraj108c/rife-onnx's RIFE 4.9
    // export, which works the same (both checked on a moving square, within
    // 1 px of the true position at t = 0.25 and 0.5) but whose repository
    // states no licence. Hugging Face commit ee09066; SHA-256 = LFS object
    // id, read 2026-10-04.
    ModelSpec {
        id: "rife",
        version: "v4-fp32-timestep-ee09066",
        name: "RIFE (optical-flow frame interpolation)",
        task: Task::Interpolate,
        licence: "MIT",
        commercial_ok: true,
        url: "https://huggingface.co/walterlow/RIFE_fp32_timestep/resolve/ee09066f9822f8b28b8477a1b4cc30f19d607590/RIFE_fp32_timestep.onnx",
        sha256: "4da60c1f20d42dba4f21503140940aa48a488585ead977532bf22e95a0319327",
        bytes: 21_604_567,
        file: "RIFE_fp32_timestep.onnx",
        providers_tested: &["CPU", "CUDA"],
        // Slow on the CPU (seconds per 1080p frame) but bounded in memory;
        // the bake says how long it will take instead of refusing.
        cpu_ok: true,
        companion: None,
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

/// How a runtime pack is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Archive {
    /// A `.tgz`, unpacked while it downloads (Microsoft's ONNX Runtime).
    TarGz,
    /// A Python wheel (a zip, read from its end, so it is downloaded to a
    /// file first): NVIDIA's own redistribution of a CUDA library on PyPI.
    Wheel,
}

/// What a runtime pack holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackKind {
    /// An ONNX Runtime build: the library the worker loads.
    OnnxRuntime,
    /// Libraries a GPU provider of an ONNX Runtime build opens: the CUDA
    /// runtime, cuBLAS, cuRAND, NVRTC, cuDNN.
    Libraries,
}

/// One downloadable set of runtime libraries: an ONNX Runtime build, or a
/// library a GPU provider needs that systems often lack (cuDNN, or a CUDA
/// runtime new enough for ONNX Runtime 1.28).
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
    pub archive: Archive,
    /// The file in `lib/` whose presence means the pack is installed.
    pub library: &'static str,
    pub kind: PackKind,
    /// The CUDA major version the pack is built for or belongs to; 0 when
    /// it has nothing to do with CUDA (the CPU build).
    pub cuda: u32,
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
        archive: Archive::TarGz,
        library: "libonnxruntime.so",
        kind: PackKind::OnnxRuntime,
        cuda: 0,
    },
    RuntimePack {
        id: "cuda12",
        name: "ONNX Runtime for NVIDIA (CUDA 12)",
        version: ORT_VERSION,
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-gpu_cuda12-1.28.3.tgz",
        sha256: "4e16d2ec66521a24fe917bd848eb8ae3a9107cb8b4aec978fb2b3ab1a640fcb7",
        bytes: 423_745_424,
        providers: &["CUDA"],
        needs: "CUDA 12.1 or newer (cudart, cuBLAS, cuRAND) and cuDNN 9: the NVIDIA CUDA 12 bundle",
        archive: Archive::TarGz,
        library: "libonnxruntime.so",
        kind: PackKind::OnnxRuntime,
        cuda: 12,
    },
    RuntimePack {
        id: "cuda13",
        name: "ONNX Runtime for NVIDIA (CUDA 13)",
        version: ORT_VERSION,
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-linux-x64-gpu_cuda13-1.28.3.tgz",
        sha256: "33e91f819324449bede2d1e853c8d46bb6253fa8890bac98ae676f041c8f73a6",
        bytes: 240_886_111,
        providers: &["CUDA"],
        needs: "CUDA 13 (cudart, cuBLAS, cuRAND) and cuDNN 9 for CUDA 13: the NVIDIA CUDA 13 bundle",
        archive: Archive::TarGz,
        library: "libonnxruntime.so",
        kind: PackKind::OnnxRuntime,
        cuda: 13,
    },
    // NVIDIA's own wheels on PyPI, which NVIDIA publishes for exactly this
    // use: an application that needs the CUDA runtime without a CUDA install
    // (NVIDIA's licence for these wheels allows using the runtime libraries
    // in an application). The worker preloads them by path, so the system's
    // own (older) CUDA never gets in the way: Ubuntu's CUDA 12.0
    // `libcudart.so.12` lacks `cudaLibraryGetKernel`, which ORT 1.28's CUDA
    // 12 provider calls. Versions and SHA-256 from PyPI's JSON API, read on
    // 2026-10-04. cuBLAS declares NVRTC as a dependency (cuBLASLt compiles
    // some kernels at run time), so it is in the bundle too.
    RuntimePack {
        id: "cudart-cu12",
        name: "CUDA 12 runtime (NVIDIA)",
        version: "12.9.79",
        url: "https://files.pythonhosted.org/packages/bc/46/a92db19b8309581092a3add7e6fceb4c301a3fd233969856a8cbf042cd3c/nvidia_cuda_runtime_cu12-12.9.79-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
        sha256: "25bba2dfb01d48a9b59ca474a1ac43c6ebf7011f1b0b8cc44f54eb6ac48a96c3",
        bytes: 3_493_179,
        providers: &[],
        needs: "an NVIDIA driver 525 or newer",
        archive: Archive::Wheel,
        library: "libcudart.so.12",
        kind: PackKind::Libraries,
        cuda: 12,
    },
    RuntimePack {
        id: "cublas-cu12",
        name: "cuBLAS for CUDA 12 (NVIDIA)",
        version: "12.9.2.10",
        url: "https://files.pythonhosted.org/packages/cb/c0/0a517bfe63ccd3b92eb254d264e28fca3c7cab75d07daea315250fb1bf73/nvidia_cublas_cu12-12.9.2.10-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "e4f53a8ca8c5d6e8c492d0d0a3d565ecb59a751b19cfdaa4f6da0ab2104c1702",
        bytes: 581_240_110,
        providers: &[],
        needs: "the CUDA 12 runtime",
        archive: Archive::Wheel,
        library: "libcublas.so.12",
        kind: PackKind::Libraries,
        cuda: 12,
    },
    RuntimePack {
        id: "curand-cu12",
        name: "cuRAND for CUDA 12 (NVIDIA)",
        version: "10.3.10.19",
        url: "https://files.pythonhosted.org/packages/31/44/193a0e171750ca9f8320626e8a1f2381e4077a65e69e2fb9708bd479e34a/nvidia_curand_cu12-10.3.10.19-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "49b274db4780d421bd2ccd362e1415c13887c53c214f0d4b761752b8f9f6aa1e",
        bytes: 68_295_626,
        providers: &[],
        needs: "the CUDA 12 runtime",
        archive: Archive::Wheel,
        library: "libcurand.so.10",
        kind: PackKind::Libraries,
        cuda: 12,
    },
    RuntimePack {
        id: "nvrtc-cu12",
        name: "NVRTC for CUDA 12 (NVIDIA)",
        version: "12.9.86",
        url: "https://files.pythonhosted.org/packages/b8/85/e4af82cc9202023862090bfca4ea827d533329e925c758f0cde964cb54b7/nvidia_cuda_nvrtc_cu12-12.9.86-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl",
        sha256: "210cf05005a447e29214e9ce50851e83fc5f4358df8b453155d5e1918094dcb4",
        bytes: 89_568_129,
        providers: &[],
        needs: "the CUDA 12 runtime",
        archive: Archive::Wheel,
        library: "libnvrtc.so.12",
        kind: PackKind::Libraries,
        cuda: 12,
    },
    // cuDNN 9 for CUDA 12 (`nvidia-cudnn-cu12`, NVIDIA's cuDNN licence,
    // which allows redistribution of the runtime libraries). SHA-256 from
    // PyPI's JSON API, read on 2026-10-04.
    RuntimePack {
        id: "cudnn9-cu12",
        name: "cuDNN 9 for CUDA 12 (NVIDIA)",
        version: "9.27.0.42",
        url: "https://files.pythonhosted.org/packages/65/e4/c5a205d48ff00ed8b27882bb45d338d8138e976c76385f328a326bbfaeda/nvidia_cudnn_cu12-9.27.0.42-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "0a4aa3a7d2264256506c6857fc41fc0c499982f78d70195b1cfcc9055c1957cd",
        bytes: 766_178_381,
        providers: &[],
        needs: "an NVIDIA driver and the CUDA 12 runtime",
        archive: Archive::Wheel,
        library: "libcudnn.so.9",
        kind: PackKind::Libraries,
        cuda: 12,
    },
    RuntimePack {
        id: "cudart-cu13",
        name: "CUDA 13 runtime (NVIDIA)",
        version: "13.4.92",
        url: "https://files.pythonhosted.org/packages/98/8a/3431271f6344874b8f1ac03f16b3d679c91493f8da63f716160403e6d0a0/nvidia_cuda_runtime-13.4.92-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
        sha256: "9641f797da20ce1dd8e779b6e96d08cf9ba564cec8e8225458811ee26423f3a5",
        bytes: 2_494_438,
        providers: &[],
        needs: "an NVIDIA driver 580 or newer",
        archive: Archive::Wheel,
        library: "libcudart.so.13",
        kind: PackKind::Libraries,
        cuda: 13,
    },
    RuntimePack {
        id: "cublas-cu13",
        name: "cuBLAS for CUDA 13 (NVIDIA)",
        version: "13.8.1.7",
        url: "https://files.pythonhosted.org/packages/eb/1b/adc70acf3fde509b1f0ca29d1c0e8d9b00aaecd871f8cf877257c8b68d9c/nvidia_cublas-13.8.1.7-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "c11a27fd4379510e5b1f84b367a2514d1e52fe5cc13442117a0e0a1addee3cf2",
        bytes: 439_315_821,
        providers: &[],
        needs: "the CUDA 13 runtime",
        archive: Archive::Wheel,
        library: "libcublas.so.13",
        kind: PackKind::Libraries,
        cuda: 13,
    },
    RuntimePack {
        id: "curand-cu13",
        name: "cuRAND for CUDA 13 (NVIDIA)",
        version: "10.4.4.72",
        url: "https://files.pythonhosted.org/packages/07/73/3ee8e5b4cb891401e603ffd3a59b35c6afe785fd2de123afe7c7029603dc/nvidia_curand-10.4.4.72-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "25c3457ae7a224fdd484dab90b0fc5dc0e842fab5db3012afa4a5bd2af4eb7e5",
        bytes: 61_498_332,
        providers: &[],
        needs: "the CUDA 13 runtime",
        archive: Archive::Wheel,
        library: "libcurand.so.10",
        kind: PackKind::Libraries,
        cuda: 13,
    },
    RuntimePack {
        id: "nvrtc-cu13",
        name: "NVRTC for CUDA 13 (NVIDIA)",
        version: "13.4.92",
        url: "https://files.pythonhosted.org/packages/56/9c/1342ebb460ce2afd014ec5a002adc99108e3c53a6b70f399cf64dd1f267d/nvidia_cuda_nvrtc-13.4.92-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl",
        sha256: "5ce8c97b00b232c4f50c8c4b5a3b68cafee08bdb82ea86f2052ff01d03194f4a",
        bytes: 53_301_733,
        providers: &[],
        needs: "the CUDA 13 runtime",
        archive: Archive::Wheel,
        library: "libnvrtc.so.13",
        kind: PackKind::Libraries,
        cuda: 13,
    },
    RuntimePack {
        id: "cudnn9-cu13",
        name: "cuDNN 9 for CUDA 13 (NVIDIA)",
        version: "9.27.0.42",
        url: "https://files.pythonhosted.org/packages/af/75/96ea5c5368eb595c39d629cde08a66227a864e71ccb0e593add2612bb952/nvidia_cudnn_cu13-9.27.0.42-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "9677e76f21862eb5da7ee5ed69d544738b2d8b5c3ce7e5ec125c5592e6cdbdc8",
        bytes: 536_771_498,
        providers: &[],
        needs: "an NVIDIA driver and the CUDA 13 runtime",
        archive: Archive::Wheel,
        library: "libcudnn.so.9",
        kind: PackKind::Libraries,
        cuda: 13,
    },
];

/// One way to light up a GPU: an ONNX Runtime build and every library its
/// provider needs, installed and removed together. What Settings › AI
/// acceleration offers, and `chukcut-cli ml install gpu`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bundle {
    pub id: &'static str,
    pub name: &'static str,
    /// The CUDA major version.
    pub cuda: u32,
    /// The oldest NVIDIA driver (major version) that runs it.
    pub min_driver: u32,
    /// Runtime pack ids, the ONNX Runtime build first.
    pub packs: &'static [&'static str],
}

pub const BUNDLES: &[Bundle] = &[
    Bundle {
        id: "nvidia-cu13",
        name: "NVIDIA GPU (CUDA 13)",
        cuda: 13,
        // CUDA 13.0 shipped with the r580 driver; minor-version
        // compatibility keeps every 13.x runtime working on it.
        min_driver: 580,
        packs: &[
            "cuda13",
            "cudart-cu13",
            "cublas-cu13",
            "curand-cu13",
            "nvrtc-cu13",
            "cudnn9-cu13",
        ],
    },
    Bundle {
        id: "nvidia-cu12",
        name: "NVIDIA GPU (CUDA 12)",
        cuda: 12,
        // CUDA 12.x minor-version compatibility starts at r525.
        min_driver: 525,
        packs: &[
            "cuda12",
            "cudart-cu12",
            "cublas-cu12",
            "curand-cu12",
            "nvrtc-cu12",
            "cudnn9-cu12",
        ],
    },
];

/// The bundle called `id`.
pub fn bundle(id: &str) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| b.id == id)
}

/// The bundle for an NVIDIA driver of major version `driver`: CUDA 13 from
/// r580 on (it is the smaller download and the newer runtime), CUDA 12 from
/// r525. `None` for an older driver, which no ONNX Runtime 1.28 build runs on.
pub fn bundle_for_driver(driver: u32) -> Option<&'static Bundle> {
    BUNDLES.iter().find(|b| driver >= b.min_driver)
}

/// Download size of a bundle's packs that are not installed under `root`.
pub fn bundle_missing_bytes(root: &Path, bundle: &Bundle) -> u64 {
    bundle
        .packs
        .iter()
        .filter_map(|id| runtime_pack(id))
        .filter(|p| !runtime_present(root, p))
        .map(|p| p.bytes)
        .sum()
}

/// Whether every pack of `bundle` is installed under `root`.
pub fn bundle_present(root: &Path, bundle: &Bundle) -> bool {
    bundle
        .packs
        .iter()
        .all(|id| runtime_pack(id).is_some_and(|p| runtime_present(root, p)))
}

/// The NVIDIA kernel driver's version (`610.57.04`), when one is loaded.
pub fn nvidia_driver() -> Option<String> {
    let version = std::fs::read_to_string("/sys/module/nvidia/version")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            // "NVRM version: NVIDIA UNIX x86_64 Kernel Module  610.57.04  …"
            let text = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
            let line = text.lines().next()?;
            line.split_whitespace()
                .find(|w| {
                    w.split('.').count() >= 2 && w.split('.').all(|p| p.parse::<u32>().is_ok())
                })
                .map(str::to_string)
        })?;
    Some(version)
}

/// The major number of a driver version string (`610` of `610.57.04`).
pub fn driver_major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse().ok()
}

/// The pack called `id`.
pub fn runtime_pack(id: &str) -> Option<&'static RuntimePack> {
    RUNTIME_PACKS.iter().find(|p| p.id == id)
}

/// Where `pack` is unpacked: `runtime/<id>-<version>/`, holding `lib/`.
pub fn runtime_dir(root: &Path, pack: &RuntimePack) -> PathBuf {
    root.join("runtime")
        .join(format!("{}-{}", pack.id, pack.version))
}

/// The library that marks an unpacked `pack` (for an ONNX Runtime pack,
/// the one to load).
pub fn runtime_library(root: &Path, pack: &RuntimePack) -> PathBuf {
    runtime_dir(root, pack).join("lib").join(pack.library)
}

pub fn runtime_present(root: &Path, pack: &RuntimePack) -> bool {
    runtime_library(root, pack).is_file()
}

/// Which files of a runtime archive to keep, by their path inside it. Only
/// the libraries: headers, docs and the TensorRT provider (which needs a
/// TensorRT install nobody has by accident, and is 1 GB of the archive's
/// contents) stay behind. In a wheel, only the shared libraries under
/// `nvidia/<package>/lib/`.
pub fn keep_from_runtime_archive<'a>(
    pack: &RuntimePack,
    path_in_archive: &'a str,
) -> Option<&'a str> {
    let name = path_in_archive.rsplit('/').next()?;
    match pack.archive {
        Archive::TarGz => {
            let in_lib = path_in_archive.contains("/lib/");
            (in_lib
                && name.starts_with("libonnxruntime")
                && name.contains(".so")
                && !name.contains("tensorrt"))
            .then_some(name)
        }
        Archive::Wheel => (path_in_archive.starts_with("nvidia/")
            && path_in_archive.contains("/lib/")
            && name.starts_with("lib")
            && name.contains(".so"))
        .then_some(name),
    }
}

/// The installed ONNX Runtime pack the worker should load on this machine.
/// See [`choose_runtime`]; this one reads the driver and
/// `CHUKCUT_ML_RUNTIME` (a pack id: `cpu`, `cuda12`, `cuda13`) itself.
pub fn preferred_runtime(root: &Path) -> Option<&'static RuntimePack> {
    let forced = std::env::var("CHUKCUT_ML_RUNTIME").ok();
    choose_runtime(
        root,
        nvidia_driver().as_deref().and_then(driver_major),
        forced.as_deref(),
    )
}

/// Which installed ONNX Runtime pack to load, best first:
///
/// 1. `forced`, when it names an installed pack;
/// 2. the build of a complete NVIDIA bundle the driver can run (CUDA 13
///    before 12): every library its provider needs is ours, so it works;
/// 3. a GPU build the driver can run whose libraries must come from the
///    system or `CHUKCUT_CUDA_LIB_DIRS` (it may still end on the CPU);
/// 4. the CPU build;
/// 5. any build at all: a GPU build runs the CPU provider too.
pub fn choose_runtime(
    root: &Path,
    driver: Option<u32>,
    forced: Option<&str>,
) -> Option<&'static RuntimePack> {
    let installed = |id: &str| runtime_pack(id).filter(|p| runtime_present(root, p));
    if let Some(pack) = forced.and_then(installed) {
        if pack.kind == PackKind::OnnxRuntime {
            return Some(pack);
        }
    }
    let runnable: Vec<&Bundle> = match driver {
        Some(driver) => BUNDLES.iter().filter(|b| driver >= b.min_driver).collect(),
        None => Vec::new(),
    };
    if let Some(bundle) = runnable.iter().find(|b| bundle_present(root, b)) {
        return installed(bundle.packs[0]);
    }
    if let Some(pack) = runnable.iter().find_map(|b| installed(b.packs[0])) {
        return Some(pack);
    }
    installed("cpu").or_else(|| {
        RUNTIME_PACKS
            .iter()
            .find(|p| p.kind == PackKind::OnnxRuntime && runtime_present(root, p))
    })
}

/// The library packs that go with ONNX Runtime pack `ort`: those of the
/// same CUDA major version, installed under `root`. Empty for the CPU build.
pub fn library_packs_for(root: &Path, ort: &RuntimePack) -> Vec<&'static RuntimePack> {
    if ort.cuda == 0 {
        return Vec::new();
    }
    RUNTIME_PACKS
        .iter()
        .filter(|p| p.kind == PackKind::Libraries && p.cuda == ort.cuda)
        .filter(|p| runtime_present(root, p))
        .collect()
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
            match p.archive {
                Archive::TarGz => {
                    assert!(p.url.contains(&format!("/v{}/", p.version)), "{}", p.url)
                }
                Archive::Wheel => {
                    assert!(p.url.contains(&format!("-{}-", p.version)), "{}", p.url)
                }
            }
        }
    }

    #[test]
    fn every_companion_is_listed_and_decodes_for_its_encoder() {
        for m in MODELS {
            if let Some(id) = m.companion {
                let companion = model(id).unwrap_or_else(|| panic!("{id}"));
                // SAM's encoder brings its decoder; the face mesh brings the
                // detector that finds the faces it reads.
                match m.task {
                    Task::SegmentEncoder => assert_eq!(companion.task, Task::SegmentDecoder),
                    Task::FaceLandmarks => assert_eq!(companion.task, Task::DetectFaces),
                    other => panic!("{} ({other:?}) should not have a companion", m.id),
                }
            }
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
        let ort = runtime_pack("cuda12").unwrap();
        let keep = |p| keep_from_runtime_archive(ort, p);
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
        let wheel = runtime_pack("cudnn9-cu12").unwrap();
        assert_eq!(
            keep_from_runtime_archive(wheel, "nvidia/cudnn/lib/libcudnn_ops.so.9"),
            Some("libcudnn_ops.so.9")
        );
        assert!(keep_from_runtime_archive(wheel, "nvidia/cudnn/include/cudnn.h").is_none());
        // CUDA 13's wheels share one directory.
        let cudart = runtime_pack("cudart-cu13").unwrap();
        assert_eq!(
            keep_from_runtime_archive(cudart, "nvidia/cu13/lib/libcudart.so.13"),
            Some("libcudart.so.13")
        );
        assert!(
            keep_from_runtime_archive(wheel, "nvidia_cudnn_cu12-9.27.0.42.dist-info/RECORD")
                .is_none()
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/ml-registry")
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn install(root: &Path, ids: &[&str]) {
        for id in ids {
            let lib = runtime_library(root, runtime_pack(id).unwrap());
            std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
            std::fs::write(&lib, b"").unwrap();
        }
    }

    #[test]
    fn a_gpu_pack_wins_over_the_cpu_pack() {
        let root = scratch("gpu-wins");
        assert!(choose_runtime(&root, Some(610), None).is_none());
        install(&root, &["cpu", "cuda12"]);
        assert_eq!(choose_runtime(&root, Some(610), None).unwrap().id, "cuda12");
        // Without an NVIDIA driver the CPU build is the honest choice.
        assert_eq!(choose_runtime(&root, None, None).unwrap().id, "cpu");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_driver_picks_the_bundle() {
        assert_eq!(bundle_for_driver(610).unwrap().id, "nvidia-cu13");
        assert_eq!(bundle_for_driver(580).unwrap().id, "nvidia-cu13");
        assert_eq!(bundle_for_driver(570).unwrap().id, "nvidia-cu12");
        assert_eq!(bundle_for_driver(525).unwrap().id, "nvidia-cu12");
        assert!(bundle_for_driver(470).is_none());
        assert_eq!(driver_major("610.57.04"), Some(610));
        assert_eq!(driver_major(""), None);
        for bundle in BUNDLES {
            let ort = runtime_pack(bundle.packs[0]).unwrap();
            assert_eq!(ort.kind, PackKind::OnnxRuntime, "{}", bundle.id);
            for id in bundle.packs {
                let pack = runtime_pack(id).unwrap_or_else(|| panic!("{id}"));
                assert_eq!(pack.cuda, bundle.cuda, "{id}");
            }
        }
    }

    #[test]
    fn a_complete_bundle_wins_and_a_driver_too_old_for_it_does_not() {
        let root = scratch("bundle");
        // Both GPU builds, but only CUDA 12's libraries.
        install(&root, &["cpu", "cuda13"]);
        install(&root, bundle("nvidia-cu12").unwrap().packs);
        assert_eq!(choose_runtime(&root, Some(610), None).unwrap().id, "cuda12");
        install(&root, bundle("nvidia-cu13").unwrap().packs);
        assert_eq!(choose_runtime(&root, Some(610), None).unwrap().id, "cuda13");
        // An r550 driver cannot run CUDA 13.
        assert_eq!(choose_runtime(&root, Some(550), None).unwrap().id, "cuda12");
        // Forced, by pack id; a library pack is not a runtime.
        assert_eq!(
            choose_runtime(&root, Some(610), Some("cpu")).unwrap().id,
            "cpu"
        );
        assert_eq!(
            choose_runtime(&root, Some(610), Some("cudnn9-cu13"))
                .unwrap()
                .id,
            "cuda13"
        );
        let ort13 = runtime_pack("cuda13").unwrap();
        let libs: Vec<&str> = library_packs_for(&root, ort13)
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(libs.len(), 5);
        assert!(libs.iter().all(|id| id.ends_with("-cu13")), "{libs:?}");
        assert!(library_packs_for(&root, runtime_pack("cpu").unwrap()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
