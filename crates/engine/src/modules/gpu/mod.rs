//! The devices this process owns: one GPU, one VAAPI display.
//!
//! Nothing else in the codebase is allowed to open either. `render`, `preview`,
//! `export` and `media` all reach a device through the two accessors here, and
//! the constructors they would otherwise call are crate-private with a comment
//! pointing back at this file.
//!
//! ## Why one of each, rather than one per user
//!
//! This is not tidiness. Both devices have been observed misbehaving when a
//! second one exists:
//!
//! - **wgpu.** Tests that each opened their own `RenderContext` had a dozen
//!   Vulkan instances alive at once under `cargo test`'s default parallelism,
//!   and creating and tearing down that many concurrently segfaults inside the
//!   Mesa driver often enough to make the suite unreliable — measured at four
//!   runs in forty before the preview server was made to share the test
//!   binary's device. The shipped application had the same shape one level up:
//!   the preview's render thread opened one device and the export's lazily
//!   built compositor opened another, so any export ran with two live Vulkan
//!   instances in one address space. `docs/STATUS.md` carries both
//!   observations.
//! - **VAAPI.** `vaInitialize` costs tens of milliseconds and a driver has a
//!   finite number of contexts, so a preview JPEG encoder, an export encoder
//!   and a hardware decoder each opening a display is three displays for one
//!   chip. `media::hwdecode` already refcounted its device for exactly this
//!   reason; this module is that pattern applied to all three.
//!
//! ## What "shared" means for each
//!
//! They are shared differently because the two libraries count differently.
//!
//! A [`RenderContext`] is handed out as an `Arc`: wgpu's `Device` and `Queue`
//! are `Sync` and internally reference counted, so every holder points at the
//! same object and the last one to drop closes it.
//!
//! A [`VaapiDevice`] is handed out **by value**, and each hand-out is a fresh
//! `av_buffer_ref` on the same `AVBufferRef` — libavutil's own atomic refcount
//! is the sharing mechanism. That is what lets a caller pass its handle into a
//! codec context, or drop it, without any coordination with the other callers.
//!
//! ## Failing to open one is normal
//!
//! Neither accessor returns an error, because there is no caller whose response
//! to one would be anything other than "use the software path and say so". A
//! machine with no adapter still runs the editor: the export refuses with a
//! sentence, the preview reports an error to the webview, and neither panics.
//! The reason is logged once, when the attempt is made, rather than once per
//! call.

use std::sync::{Arc, OnceLock};

use crate::modules::media::hwdecode::{CudaDevice, VaapiDevice};
use crate::modules::render::RenderContext;

/// The process's GPU, opened on the first call.
///
/// `None` on a machine that produced no usable adapter — a headless box with no
/// software rasterizer is the real case — which every caller is expected to
/// handle rather than unwrap.
pub fn render_context() -> Option<Arc<RenderContext>> {
    static CONTEXT: OnceLock<Option<Arc<RenderContext>>> = OnceLock::new();
    CONTEXT
        .get_or_init(|| match RenderContext::open() {
            Ok(context) => Some(Arc::new(context)),
            Err(error) => {
                // Not a warning per caller: the compositor, the preview and the
                // export will all ask, and the answer is the same each time.
                tracing::warn!(%error, "no render device available; nothing can be composited");
                None
            }
        })
        .clone()
}

/// The process's VAAPI display, opened on the first call.
///
/// Each call returns a new libavutil reference to the same display, so the
/// caller owns its handle outright and may drop it, clone it, or hand it to a
/// codec context without telling anyone.
///
/// `None` on a machine with no working VAAPI — a headless server, a laptop
/// whose GPU is claimed by something else — which is a normal outcome and not
/// an error. Hardware decode, hardware encode and the hardware JPEG path all
/// fall back to software on it.
pub fn vaapi_device() -> Option<VaapiDevice> {
    static DEVICE: OnceLock<Option<VaapiDevice>> = OnceLock::new();
    DEVICE
        .get_or_init(|| match VaapiDevice::open(None) {
            Ok(device) => {
                tracing::info!(node = %device.node(), "VAAPI device ready");
                Some(device)
            }
            Err(error) => {
                tracing::info!(%error, "no VAAPI device; decoding and encoding in software");
                None
            }
        })
        .clone()
}

/// The process's CUDA context, for NVDEC, opened on the first call.
///
/// Only tried when the NVIDIA kernel driver is loaded: on any other machine
/// libavutil would `dlopen` libcuda just to fail, and say so on stderr.
/// `None` is the normal answer on Intel and AMD.
pub fn cuda_device() -> Option<CudaDevice> {
    static DEVICE: OnceLock<Option<CudaDevice>> = OnceLock::new();
    DEVICE
        .get_or_init(|| {
            if !std::path::Path::new("/proc/driver/nvidia/version").exists() {
                return None;
            }
            match CudaDevice::open() {
                Ok(device) => {
                    tracing::info!("CUDA device ready for NVDEC");
                    Some(device)
                }
                Err(error) => {
                    tracing::info!(%error, "NVIDIA driver loaded but no CUDA device");
                    None
                }
            }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the module. If this ever fails, an export runs with
    /// two Vulkan instances again and the driver crash comes back.
    #[test]
    fn the_render_context_is_one_object_and_not_two() {
        let Some(first) = render_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let second = render_context().expect("a second handle to the same device");
        assert!(
            Arc::ptr_eq(&first, &second),
            "gpu::render_context handed out two different devices"
        );
    }

    /// Two handles to one display, not two displays. If this ever starts
    /// opening a second, the symptom is a driver running out of contexts on a
    /// timeline with many clips.
    #[test]
    fn the_vaapi_device_is_one_display_and_not_two() {
        let Some(first) = vaapi_device() else {
            eprintln!("skipping: no VAAPI device");
            return;
        };
        let second = vaapi_device().expect("a second handle");
        assert_eq!(first.node(), second.node());
        // Distinct owned references to the same `AVBufferRef`: dropping one
        // must not disturb the other, which is what makes handing these out by
        // value safe.
        drop(second);
        assert!(!first.node().is_empty());
    }

    /// Asking repeatedly is what the preview does on every session change, so
    /// it has to be cheap and it has to keep answering the same thing.
    #[test]
    fn asking_again_never_opens_anything() {
        for _ in 0..8 {
            let _ = render_context();
            let _ = vaapi_device();
        }
    }
}
