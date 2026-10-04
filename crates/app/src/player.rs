//! The preview, as GPUI sees it.
//!
//! All the work is the engine's `preview::player::FramePlayer`: decode-ahead,
//! render-ahead during playback, a GPU swizzle to BGRA. This wrapper turns its
//! frames into something GPUI can paint, which is the one step that needs
//! `gpui`.
//!
//! Frames normally stay on the GPU: the engine exports their memory and the
//! patched GPUI (`vendor/README.md`) imports it into its own device and draws
//! it with `Window::paint_external_buffer` — no readback, no upload. Where
//! that is impossible the frames are read back and painted as `RenderImage`s:
//!
//! - `CHUKCUT_PREVIEW_READBACK=1` asks for the readback from the start;
//! - the engine's device cannot export memory (not Vulkan, lavapipe without
//!   the extension) — the engine reads back by itself;
//! - GPUI cannot import it (another GPU, another driver) — its renderer
//!   reports the failure and the next [`Player::take`] switches over.
//!
//! `docs/decisions/0027-preview-frames-shared-with-gpui.md`.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chukcut_engine::modules::preview::player::{
    FramePixels, FramePlayer, PlayerRequest, PlayerStats, Sharing,
};
use chukcut_engine::modules::project::{Micros, Project};
use chukcut_engine::modules::render::SharedFrame;
use gpui::{
    canvas, img, AnyElement, ExternalBuffer, ExternalBufferInfo, IntoElement, RenderImage, Styled,
};

/// One rendered preview frame.
pub struct Frame {
    pub time: Micros,
    pub picture: Picture,
}

/// A preview frame in whichever form GPUI can draw.
#[derive(Clone)]
pub enum Picture {
    /// Read back; uploaded into the window's atlas when painted, so the old
    /// one must be dropped with `cx.drop_image` once replaced.
    Image(Arc<RenderImage>),
    /// In GPU memory shared with GPUI. Dropping the last clone lets the
    /// engine reuse the buffer.
    Shared(ExternalBuffer),
}

impl Picture {
    /// The picture, filling its parent.
    pub fn element(&self) -> AnyElement {
        match self {
            Picture::Image(image) => img(Arc::clone(image)).size_full().into_any_element(),
            Picture::Shared(buffer) => {
                let buffer = buffer.clone();
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| window.paint_external_buffer(bounds, buffer),
                )
                .size_full()
                .into_any_element()
            }
        }
    }
}

pub struct Player {
    inner: FramePlayer,
    /// Set by GPUI's renderer when it cannot import a shared frame.
    import_failed: Arc<AtomicBool>,
    /// How the last frame arrived, so a change is logged once.
    arrived: std::cell::Cell<Option<Sharing>>,
}

impl Player {
    pub fn new() -> Self {
        let inner = FramePlayer::new();
        let readback = std::env::var_os("CHUKCUT_PREVIEW_READBACK").is_some_and(|v| v != "0");
        inner.use_shared_frames(!readback);
        Self {
            inner,
            import_failed: Arc::new(AtomicBool::new(false)),
            arrived: std::cell::Cell::new(None),
        }
    }

    /// Say what should be on screen: the project after edit `generation`, at
    /// `time`, `size` device pixels big. `playing` is normal-speed playback on
    /// the audio clock, which renders ahead; anything else renders exactly
    /// `time`, newest request first.
    pub fn request(
        &self,
        project: Arc<Project>,
        generation: u64,
        time: Micros,
        size: (u32, u32),
        playing: bool,
    ) {
        self.inner.request(PlayerRequest {
            project,
            generation,
            time,
            size,
            playing,
        });
    }

    /// The frame to show now that the clock reads `clock`, if a new one is due.
    pub fn take(&self, clock: Micros) -> Option<Frame> {
        if self.import_failed.swap(false, Ordering::Relaxed) {
            tracing::warn!("GPUI cannot draw shared preview frames; reading them back");
            // Re-renders the current request, so a paused preview does not
            // stay blank.
            self.inner.use_shared_frames(false);
        }
        let frame = self.inner.take(clock)?;
        let sharing = self.inner.sharing();
        if self.arrived.replace(Some(sharing)) != Some(sharing) {
            tracing::info!(?sharing, "preview frames reach GPUI");
        }
        let picture = match frame.pixels {
            // Already BGRA, which is what `RenderImage` stores; the buffer is
            // only wrapped, never copied or swizzled here.
            FramePixels::Bgra(bytes) => {
                let buffer = image::RgbaImage::from_raw(frame.width, frame.height, bytes)?;
                Picture::Image(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
            }
            FramePixels::Shared(shared) => {
                Picture::Shared(external_buffer(&shared, &self.import_failed))
            }
        };
        Some(Frame {
            time: frame.time,
            picture,
        })
    }

    /// Preview from proxy media where it exists.
    pub fn use_proxies(&self, on: bool) {
        self.inner.use_proxies(on);
    }

    pub fn failure(&self) -> Option<String> {
        self.inner.failure()
    }

    /// Whether frames reach GPUI through shared memory right now.
    #[cfg(test)]
    pub fn sharing(&self) -> Sharing {
        self.inner.sharing()
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> PlayerStats {
        self.inner.stats()
    }
}

/// Describe an engine frame to GPUI. The buffer's `Arc` is the owner: while
/// GPUI holds a clone the engine's pool will not write into it.
pub fn external_buffer(frame: &SharedFrame, failed: &Arc<AtomicBool>) -> ExternalBuffer {
    let buffer = &frame.buffer;
    let identity = buffer.identity();
    let info = ExternalBufferInfo {
        allocation_id: buffer.id(),
        frame_id: frame.frame_id,
        fd: buffer.fd(),
        buffer_size: buffer.buffer_size(),
        usage: buffer.usage(),
        allocation_size: buffer.allocation_size(),
        memory_type_index: buffer.memory_type_index(),
        device_uuid: identity.device_uuid,
        driver_uuid: identity.driver_uuid,
        width: frame.width,
        height: frame.height,
        bytes_per_row: frame.bytes_per_row,
    };
    let owner: Arc<dyn Any + Send + Sync> = Arc::clone(buffer) as _;
    // SAFETY: the descriptor belongs to `buffer`, which `owner` keeps alive,
    // and every other field is read from the same allocation.
    unsafe { ExternalBuffer::new(info, owner, Arc::clone(failed)) }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    use chukcut_engine::modules::project::{CanvasConfig, SAMPLE_SLACK};
    use chukcut_engine::modules::render::{
        BgraReadback, Compositor, EmptySourceProvider, SharedFrames,
    };
    use gpui_wgpu::wgpu;

    use super::*;

    /// A device standing in for GPUI's: the wgpu GPUI links, on the adapter
    /// it prefers (a discrete GPU). The one place this crate opens a device of
    /// its own, and only in tests: the window's real one is not reachable
    /// without a display.
    fn gpui_device() -> Option<&'static (wgpu::Device, wgpu::Queue)> {
        static DEVICE: OnceLock<Option<(wgpu::Device, wgpu::Queue)>> = OnceLock::new();
        DEVICE
            .get_or_init(|| {
                gpui_adapters()
                    .into_iter()
                    .find_map(|adapter| block_on(adapter.request_device(&Default::default())).ok())
            })
            .as_ref()
    }

    /// The adapters in the order GPUI's `WgpuContext` tries them with no
    /// compositor hint (Vulkan and GL; discrete, integrated, other, virtual,
    /// CPU; Vulkan first within each). It takes the first that opens.
    fn gpui_adapters() -> Vec<wgpu::Adapter> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let mut adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        adapters.sort_by_key(|adapter| {
            let info = adapter.get_info();
            let kind = match info.device_type {
                wgpu::DeviceType::DiscreteGpu => 0,
                wgpu::DeviceType::IntegratedGpu => 1,
                wgpu::DeviceType::Other => 2,
                wgpu::DeviceType::VirtualGpu => 3,
                wgpu::DeviceType::Cpu => 4,
            };
            (kind, u8::from(info.backend != wgpu::Backend::Vulkan))
        });
        adapters
    }

    /// wgpu's native futures resolve on the first poll.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
            std::thread::yield_now();
        }
    }

    fn solid(width: u32, height: u32) -> Project {
        Project::new(
            "shared",
            CanvasConfig {
                width,
                height,
                background: [0.9, 0.3, 0.1, 1.0],
            },
            30.0,
        )
    }

    /// Read `buffer` on GPUI's stand-in device, rows unpadded.
    fn read_imported(
        (device, queue): &(wgpu::Device, wgpu::Queue),
        buffer: &wgpu::Buffer,
        info: &ExternalBufferInfo,
    ) -> Vec<u8> {
        let bytes = u64::from(info.bytes_per_row) * u64::from(info.height);
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes);
        queue.submit(Some(encoder.finish()));
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let view = staging.slice(..).get_mapped_range();
        view.chunks(info.bytes_per_row as usize)
            .flat_map(|row| row[..info.width as usize * 4].to_vec())
            .collect()
    }

    /// The shared path end to end below the window: the engine renders into
    /// exported memory, GPUI's wgpu imports it, and what it reads is exactly
    /// what the readback would have produced. Skipped where either side
    /// cannot share — which is when the app falls back.
    #[test]
    fn a_shared_frame_imports_into_gpuis_device_byte_for_byte() {
        let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let Some(mut shared) = SharedFrames::<()>::new(Arc::clone(&ctx), 2) else {
            eprintln!("skipping: the engine's device cannot export memory");
            return;
        };
        let Some(gpui) = gpui_device() else {
            eprintln!("skipping: no device for GPUI");
            return;
        };
        let mut readback = BgraReadback::<()>::new(Arc::clone(&ctx), 2);
        let compositor = Compositor::new(Arc::clone(&ctx));
        // 330 px: rows of 1320 bytes, padded to 1536 in the shared buffer.
        let target = compositor
            .render_to_texture(&solid(330, 120), 0, (330, 120), &EmptySourceProvider)
            .expect("render");
        shared.submit(&target, ()).expect("room");
        readback.submit(&target, ()).expect("room");
        compositor.pool().release(target);
        let frame = shared.collect_oldest().expect("a frame").1.expect("done");
        let expected = readback.collect_oldest().expect("a frame").1.expect("done");

        let failed = Arc::new(AtomicBool::new(false));
        let external = external_buffer(&frame, &failed);
        let info = *external.info();
        let imported = match gpui_wgpu::import_external_buffer(&gpui.0, &info) {
            Ok(buffer) => buffer,
            Err(reason) if reason.contains("another GPU") => {
                eprintln!("skipping: GPUI's stand-in device is on another adapter");
                return;
            }
            Err(reason) => panic!("a same-GPU import failed: {reason}"),
        };
        assert_eq!(read_imported(gpui, &imported, &info), expected.data);
    }

    /// Memory from another GPU or driver is refused before anything is
    /// imported, and that refusal is what sends the app to the readback.
    #[test]
    fn memory_from_another_gpu_is_refused() {
        let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
            return;
        };
        let Some(mut shared) = SharedFrames::<()>::new(Arc::clone(&ctx), 1) else {
            eprintln!("skipping: the engine's device cannot export memory");
            return;
        };
        let Some(gpui) = gpui_device() else {
            return;
        };
        let compositor = Compositor::new(Arc::clone(&ctx));
        let target = compositor
            .render_to_texture(&solid(64, 64), 0, (64, 64), &EmptySourceProvider)
            .expect("render");
        shared.submit(&target, ()).expect("room");
        compositor.pool().release(target);
        let frame = shared.collect_oldest().expect("a frame").1.expect("done");
        let mut info = *external_buffer(&frame, &Arc::new(AtomicBool::new(false))).info();
        info.device_uuid[0] ^= 0xff;
        assert!(gpui_wgpu::import_external_buffer(&gpui.0, &info).is_err());
    }

    fn wait_for(player: &Player) -> Frame {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if let Some(frame) = player.take(SAMPLE_SLACK) {
                return frame;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no frame within 10 s");
    }

    /// The automatic fallback: once GPUI's renderer reports it cannot draw a
    /// shared frame, the next frames are `RenderImage`s — including a paused
    /// preview's, which is rendered again rather than left blank.
    #[test]
    fn a_failed_import_switches_the_player_to_images() {
        if chukcut_engine::modules::gpu::render_context().is_none() {
            return;
        }
        let player = Player::new();
        let project = Arc::new(solid(160, 90));
        player.request(Arc::clone(&project), 1, SAMPLE_SLACK, (160, 90), false);
        let first = wait_for(&player);
        match &first.picture {
            Picture::Shared(external) => {
                assert_eq!(player.sharing(), Sharing::Shared);
                // What GPUI's renderer does when the import fails.
                external.report_failure();
            }
            Picture::Image(_) => {
                eprintln!("skipping: frames are read back here already");
                return;
            }
        }
        // No new request: the switch alone must bring a new frame.
        let second = wait_for(&player);
        assert!(matches!(second.picture, Picture::Image(_)));
        assert_eq!(player.sharing(), Sharing::Readback);
    }
}
