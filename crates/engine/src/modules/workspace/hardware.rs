//! What this machine can actually do, reported to the user.
//!
//! This exists to answer one question the owner has asked in several forms:
//! *is it really using my graphics card?* Every other surface answers that
//! implicitly and unreliably — a fast export could be a fast CPU, and a slow
//! one could be a driver silently refusing.
//!
//! The rule the whole module follows: **a capability is only reported as usable
//! if it was used.** Both probes underneath establish their answers by opening a
//! device and pushing a real frame through it, not by reading a capability list.
//! That distinction is not pedantry — it is what this hardware actually
//! required. `av1_vaapi` is present in every FFmpeg build and encodes on
//! nothing; `avcodec_find_decoder(AV1)` returns `libdav1d`, which has no
//! hardware support at all and decodes on the CPU while reporting success. Both
//! of those cost an afternoon before the probes were made to try rather than
//! ask.
//!
//! So the report has three states per item, and the third is the interesting
//! one: usable, absent, and **present but refused, with the driver's reason**.

use serde::Serialize;

use crate::modules::export::hwaccel;
use crate::modules::gpu;
use crate::modules::media::hwdecode;

/// One encoder or decoder, as the settings panel shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareCodec {
    /// Stable id, e.g. `h264_vaapi` for an encoder or `h264` for a decoder.
    pub id: String,
    /// What to show: "H.264", "HEVC".
    pub label: String,
    /// The accelerator behind it: `vaapi`, `qsv`, `nvenc`, `videotoolbox`.
    pub accel: String,
    /// Present in the build at all.
    pub available: bool,
    /// A real frame went through it. The only field worth believing.
    pub usable: bool,
    /// Why not, in prose, when `usable` is false and `available` is true.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareReport {
    /// Adapter name as the driver reports it, e.g. "Intel(R) Graphics (RPL-U)".
    pub gpu: Option<String>,
    /// `Vulkan`, `Gl`, `Metal`, `Dx12`.
    pub backend: Option<String>,
    /// `IntegratedGpu`, `DiscreteGpu`, `Cpu` — a software rasterizer answering
    /// as a GPU is a thing that happens, and it explains a great deal.
    pub device_type: Option<String>,
    /// Whether a decoded video frame can become a texture without a copy
    /// through system memory. This is the single flag that decides whether
    /// hardware decode is worth using at all.
    pub can_import_dmabuf: bool,
    pub encoders: Vec<HardwareCodec>,
    pub decoders: Vec<HardwareCodec>,
    /// True when the GPU could not be opened, so the codec lists are all this
    /// report contains. The panel says so rather than showing an empty section
    /// that reads as "unsupported".
    pub partial: bool,
}

/// Build the report.
///
/// Both probes are cached for the process — a GPU is not hot-plugged mid-session
/// and each probe opens devices — so this is cheap after the first call. The
/// first call is not: expect on the order of a hundred milliseconds per encoder
/// on a cold start, which is why the panel that shows it loads asynchronously.
pub fn report() -> HardwareReport {
    let encoders = hwaccel::detect()
        .into_iter()
        .map(|encoder| HardwareCodec {
            id: encoder.id,
            label: encoder.label,
            accel: format!("{:?}", encoder.accel).to_lowercase(),
            available: encoder.available,
            usable: encoder.usable,
            note: encoder.note,
        })
        .collect();

    let decoders = hwdecode::capabilities()
        .iter()
        .map(|support| HardwareCodec {
            id: support.decoder_name.clone(),
            label: support.codec.label().to_string(),
            accel: "vaapi".to_string(),
            available: support.in_build,
            usable: support.usable,
            note: support.note.clone(),
        })
        .collect();

    // The process's device, not one of this report's own: creating a second
    // device is the bug `modules::gpu` exists to prevent. Asking for it here
    // does open it if nothing has yet, which is the right trade — the user asked
    // what their GPU is, and an answer needs an adapter.
    match gpu::render_context() {
        Some(ctx) => {
            let info = ctx.adapter_info();
            HardwareReport {
                gpu: Some(info.name.clone()),
                backend: Some(format!("{:?}", info.backend)),
                device_type: Some(format!("{:?}", info.device_type)),
                can_import_dmabuf: ctx.can_import_dmabuf(),
                encoders,
                decoders,
                partial: false,
            }
        }
        None => HardwareReport {
            gpu: None,
            backend: None,
            device_type: None,
            can_import_dmabuf: false,
            encoders,
            decoders,
            partial: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_always_answerable() {
        // Never `Result`: a machine with no GPU still gets a report, because
        // "we could not open a device" is precisely the answer the user is
        // looking for when they open this panel.
        let report = report();
        if report.partial {
            assert!(report.gpu.is_none());
            assert!(!report.can_import_dmabuf);
        } else {
            assert!(report.gpu.is_some());
        }
    }

    #[test]
    fn nothing_is_usable_without_being_available() {
        let report = report();
        for codec in report.encoders.iter().chain(report.decoders.iter()) {
            assert!(
                !codec.usable || codec.available,
                "{} is usable but not available, which cannot be true",
                codec.id
            );
        }
    }
}
