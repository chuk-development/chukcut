//! wgpu device ownership.
//!
//! One `RenderContext` per process. It is created headless — there is no
//! window and no surface anywhere in this module, because the preview does not
//! own one (frames leave over `chukcut-frame://`) and the exporter certainly
//! does not. Anything that needs a surface builds it on top of this device;
//! nothing here knows that surfaces exist.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::error::{RenderError, Result};

/// Devices alive in this process. One, unless somebody went around `gpu`.
///
/// Kept because the failure it guards against is intermittent and expensive to
/// diagnose from scratch: two live Vulkan instances in one address space have
/// been observed segfaulting inside the Mesa driver on this machine. A counter
/// costs nothing and turns "the app crashed once in twenty runs" into a line in
/// the log naming the cause.
static LIVE_DEVICES: AtomicUsize = AtomicUsize::new(0);

/// Backends we are willing to run on, in the order we want them.
///
/// Vulkan first because on Linux it is the only backend with a sane headless
/// story and predictable performance. GL is the fallback for machines with no
/// Vulkan ICD — old Intel drivers, some VMs — and is slower but correct.
/// `Backends::all()` is the last resort so a platform we did not think about
/// (Metal on a mac, DX12 on Windows) still works.
const BACKEND_PREFERENCE: [wgpu::Backends; 3] = [
    wgpu::Backends::VULKAN,
    wgpu::Backends::GL,
    wgpu::Backends::all(),
];

/// Instance flags, which in a debug build are not the ones wgpu would pick.
///
/// wgpu turns the **Khronos validation layer** on for debug builds, and on this
/// machine that layer segfaults inside the Intel Vulkan driver the moment a
/// DMA-BUF image's memory is bound — `vkBindImageMemory` called through
/// `VkLayer_khronos_validation` into `libvulkan_intel`, on the first
/// hardware-decoded frame the compositor imports. With the layer out of the way
/// the identical call succeeds and the imported picture is verified correct to a
/// mean channel difference of 1.2 (`examples/hwdecode_pipeline --verify`), so the
/// fault is in the layer or the driver rather than in the descriptor we hand
/// them. It made `cargo test --test compositor` die with signal 11 and no other
/// diagnostic, which is a long afternoon for whoever meets it next.
///
/// What is lost is small and what is kept is the important part: **wgpu's own
/// validation is in wgpu-core and is always on**, so a wrong bind group or an
/// out-of-range copy is still caught with a readable message, routed into
/// `tracing` below. The Vulkan layer is a second, lower net.
///
/// `WGPU_VALIDATION=1` puts it back for anyone debugging something the layer
/// would catch — and who is prepared for the crash on the decode path, or is
/// running with `CHUKCUT_DECODE=software`.
fn instance_flags() -> wgpu::InstanceFlags {
    let mut flags = wgpu::InstanceFlags::from_build_config().with_env();
    if std::env::var("WGPU_VALIDATION").as_deref() != Ok("1") {
        flags.remove(wgpu::InstanceFlags::VALIDATION);
        flags.remove(wgpu::InstanceFlags::GPU_BASED_VALIDATION);
    }
    flags
}

/// The GPU, and what it is willing to do.
pub struct RenderContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    limits: wgpu::Limits,
}

impl RenderContext {
    /// Open a device, blocking. wgpu's request futures resolve on the calling
    /// thread for the native backends, so `pollster` here costs nothing and
    /// saves every caller from being async.
    ///
    /// **This is not how to get a device.** There is exactly one per process
    /// and [`crate::modules::gpu::render_context`] owns it; this constructor is
    /// crate-private, and called from exactly one place, so that stays true.
    /// The reasoning is in the header of `modules/gpu`.
    pub(crate) fn open() -> Result<Self> {
        let context = pollster::block_on(Self::open_async())?;
        if LIVE_DEVICES.fetch_add(1, Ordering::SeqCst) > 0 {
            tracing::error!(
                "a second GPU device was opened in this process; every caller should be                  going through gpu::render_context"
            );
        }
        Ok(context)
    }

    async fn open_async() -> Result<Self> {
        let mut last_error = String::from("no backend was tried");

        for backends in BACKEND_PREFERENCE {
            // Software adapters are worth a second pass: on a headless CI box
            // lavapipe is frequently the only thing present, and a slow frame
            // beats no frame.
            for force_fallback in [false, true] {
                match Self::try_backend(backends, force_fallback).await {
                    Ok(ctx) => {
                        tracing::info!(
                            backend = ?ctx.info.backend,
                            device = %ctx.info.name,
                            device_type = ?ctx.info.device_type,
                            "render device ready"
                        );
                        return Ok(ctx);
                    }
                    Err(e) => last_error = e.to_string(),
                }
            }
        }

        Err(RenderError::NoAdapter(last_error))
    }

    async fn try_backend(backends: wgpu::Backends, force_fallback: bool) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: instance_flags(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: force_fallback,
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::NoAdapter(format!("{backends:?}: {e}")))?;

        // Ask for exactly what the adapter offers. We do not need any optional
        // feature — a textured quad is the whole pipeline — but taking the
        // adapter's limits rather than the defaults is what lets us render 4K
        // and 8K frames on hardware that can.
        // Ask for DMA-BUF import, never require it.
        //
        // This is what lets a hardware-decoded video frame become a texture
        // without a trip through system memory — measured at 1.7 ms against
        // 26.9 ms for the software path, because a VA surface is tiled and
        // transferring it out is a detiling pass over the whole frame.
        //
        // Requesting rather than requiring matters: a machine without the
        // extension must still run the editor on the software decoder, and
        // `Features & wanted` yields the empty set there rather than failing
        // device creation.
        //
        // 16-bit normalised textures are asked for the same way: they are how
        // a 10-bit source (P010, from NVDEC, VAAPI or the software decoder)
        // reaches the shader without being cut to 8 bits first. A device
        // without them gets the top byte of each sample instead.
        let wanted = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF
            | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let optional = adapter.features() & wanted;
        if !optional.contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF) {
            tracing::info!("this adapter cannot import DMA-BUF; decode stays on the CPU path");
        }
        if !optional.contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) {
            tracing::info!("this adapter has no 16-bit textures; 10-bit sources are cut to 8 bits");
        }

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("chukcut render device"),
                required_features: optional,
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| RenderError::DeviceCreation(format!("{backends:?}: {e}")))?;

        // Validation errors would otherwise be printed straight to stderr by
        // wgpu's default handler and lost; route them into tracing instead.
        device.on_uncaptured_error(std::sync::Arc::new(|e| {
            tracing::error!(error = %e, "wgpu validation error");
        }));

        let info = adapter.get_info();
        let limits = device.limits();

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            info,
            limits,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.info
    }

    /// What this device will actually accept. Callers that size a render
    /// target from user input must consult this.
    pub fn limits(&self) -> &wgpu::Limits {
        &self.limits
    }

    /// Whether a decoded video surface can be imported as a texture directly.
    ///
    /// `media` asks this before choosing hardware decode: without it, a
    /// hardware-decoded frame has to be copied out of tiled GPU memory and
    /// converted, which costs more than decoding it in software did.
    pub fn can_import_dmabuf(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
    }

    /// Whether `R16Unorm` and `Rg16Unorm` can be sampled, which is what lets
    /// a 10-bit source keep its precision up to the shader.
    pub fn supports_deep_planes(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
    }

    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.limits.max_texture_dimension_2d
    }

    /// Shrink a requested frame size to something the device can allocate,
    /// preserving the aspect ratio.
    ///
    /// Preserving aspect matters: the preview picks its resolution from the
    /// canvas and a non-uniform clamp would letterbox differently than the
    /// export, which is exactly the class of bug nobody notices until delivery.
    pub fn clamp_size(&self, (width, height): (u32, u32)) -> (u32, u32) {
        let max = self.max_texture_dimension_2d();
        let (width, height) = (width.max(1), height.max(1));
        if width <= max && height <= max {
            return (width, height);
        }
        let scale = (max as f64 / width as f64).min(max as f64 / height as f64);
        (
            ((width as f64 * scale).floor() as u32).clamp(1, max),
            ((height as f64 * scale).floor() as u32).clamp(1, max),
        )
    }

    /// Reject a size we could not render before allocating anything for it.
    pub(crate) fn check_size(&self, (width, height): (u32, u32)) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(RenderError::ZeroSize(width, height));
        }
        let max = self.max_texture_dimension_2d();
        if width > max || height > max {
            return Err(RenderError::FrameTooLarge(width, height, max));
        }
        Ok(())
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        LIVE_DEVICES.fetch_sub(1, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for RenderContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderContext")
            .field("backend", &self.info.backend)
            .field("device", &self.info.name)
            .field("device_type", &self.info.device_type)
            .field("max_texture_dimension_2d", &self.max_texture_dimension_2d())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context that cannot be created is not a test failure — CI has no GPU.
    ///
    /// The binary's shared device rather than a fresh one: see
    /// `render::test_context`.
    fn ctx() -> Option<std::sync::Arc<RenderContext>> {
        super::super::test_context()
    }

    #[test]
    fn clamp_preserves_aspect_ratio() {
        let Some(ctx) = ctx() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let max = ctx.max_texture_dimension_2d();
        let (w, h) = ctx.clamp_size((max * 4, max * 2));
        assert!(w <= max && h <= max);
        // 2:1 in, 2:1 out.
        assert_eq!(w, h * 2);
    }

    #[test]
    fn clamp_leaves_small_sizes_alone() {
        let Some(ctx) = ctx() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        assert_eq!(ctx.clamp_size((1920, 1080)), (1920, 1080));
    }

    #[test]
    fn zero_and_oversized_frames_are_rejected() {
        let Some(ctx) = ctx() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        assert!(ctx.check_size((0, 100)).is_err());
        assert!(ctx
            .check_size((ctx.max_texture_dimension_2d() + 1, 100))
            .is_err());
        assert!(ctx.check_size((640, 360)).is_ok());
    }
}
