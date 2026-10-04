//! Preview frames that GPUI draws straight out of GPU memory.
//!
//! The readback path ([`super::BgraReadback`]) brings every frame to system
//! memory because GPUI renders with a Vulkan device of its own and, until
//! `vendor/gpui-pre` grew `Window::paint_external_buffer`, could only draw
//! bytes. This is the other half of that element: the composited frame is
//! swizzled to BGRA by the same compute pass, but into a buffer whose memory
//! is **exported as an opaque file descriptor**. GPUI's renderer imports that
//! memory into its own device once per buffer and copies each new frame into a
//! texture with one `copy_buffer_to_texture` — GPU to GPU, nothing on the CPU.
//! `docs/decisions/0027-preview-frames-shared-with-gpui.md` is why it is a
//! buffer and an opaque fd rather than an image or a DMA-BUF.
//!
//! ## What the two sides promise each other
//!
//! - **Contents are final when a frame is handed out.** A job is collected only
//!   after its submission has finished on this device
//!   (`on_submitted_work_done`), so the importer never needs a semaphore.
//! - **A buffer is not written while anybody holds it.** Each
//!   [`SharedBuffer`] is an `Arc`; the pool reuses one only when it holds the
//!   last reference. The UI holds a frame while it is on screen, and GPUI's
//!   renderer holds it until its copy has finished on *its* queue. Liveness is
//!   the authority, exactly as in `preview::zerocopy`.
//! - **Rows are padded to 256 bytes**, which is what a buffer-to-texture copy
//!   requires, so the importer copies the buffer as it is.
//!
//! The pool grows on demand: the player's ring holds up to
//! [`READ_AHEAD`](crate::modules::preview::player::READ_AHEAD) frames, the
//! screen one more, GPUI's queue one or two. [`MAX_BUFFERS`] caps a
//! misbehaving consumer; past it, a frame is refused rather than a buffer
//! overwritten.

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::context::RenderContext;
use super::dmabuf::{
    device_identity, DeviceIdentity, ExportHandle, ExportableBuffer, EXPORT_BUFFER_USAGE,
};
use super::error::{RenderError, Result};
use super::nv12::READ_FORMAT;
use super::readback::ReadbackStats;
use super::texture_pool::PooledTexture;

const SHADER: &str = r#"
struct Params { width: u32, height: u32, row_words: u32, pad: u32 }

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> packed: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let c = textureLoad(source, vec2<i32>(i32(id.x), i32(id.y)), 0);
    // B, G, R, A in memory — what GPUI's `Bgra8Unorm` texture holds.
    packed[id.y * params.row_words + id.x] = pack4x8unorm(c.bgra);
}
"#;

/// Buffers one size may have alive at once. The player needs about eight
/// (four ahead, one on screen, one in GPUI's queue, one being written, one
/// spare); twelve leaves room for a slow consumer. At 4K that is 400 MB of
/// device memory in the worst case, against 12 GB on the reference card.
pub const MAX_BUFFERS: usize = 12;

/// Row alignment of `copy_buffer_to_texture`.
const ROW_ALIGNMENT: u32 = 256;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// One exported allocation. Everything an importer needs to allocate the same
/// memory on its own device.
pub struct SharedBuffer {
    id: u64,
    buffer: ExportableBuffer,
    identity: DeviceIdentity,
}

impl SharedBuffer {
    /// Unique in the process, never reused: an importer may cache by it.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The opaque file descriptor. Valid as long as this value lives; an
    /// importer `dup`s it, because a successful import consumes the
    /// descriptor it is given.
    pub fn fd(&self) -> RawFd {
        self.buffer.fd().as_raw_fd()
    }

    /// `VkBufferCreateInfo::size`.
    pub fn buffer_size(&self) -> u64 {
        self.buffer.size()
    }

    /// `VkMemoryAllocateInfo::allocationSize`.
    pub fn allocation_size(&self) -> u64 {
        self.buffer.allocation_size()
    }

    /// `VkMemoryAllocateInfo::memoryTypeIndex`.
    pub fn memory_type_index(&self) -> u32 {
        self.buffer.memory_type_index()
    }

    /// `VkBufferCreateInfo::usage`, raw. The allocation is dedicated, so an
    /// importer's buffer must be created identically.
    pub fn usage(&self) -> u32 {
        EXPORT_BUFFER_USAGE.as_raw()
    }

    pub fn identity(&self) -> DeviceIdentity {
        self.identity
    }

    /// The buffer on this device, for tests and benchmarks that read it back.
    pub fn buffer(&self) -> &wgpu::Buffer {
        self.buffer.buffer()
    }
}

impl std::fmt::Debug for SharedBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBuffer")
            .field("id", &self.id)
            .field("size", &self.buffer.size())
            .finish()
    }
}

/// A finished frame in a [`SharedBuffer`]: BGRA8, `height` rows of
/// `bytes_per_row` bytes, the first `width * 4` of each meaningful.
#[derive(Debug, Clone)]
pub struct SharedFrame {
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: u32,
    /// New for every frame, so a consumer copies each one exactly once.
    pub frame_id: u64,
    pub buffer: Arc<SharedBuffer>,
}

struct Job<T> {
    tag: T,
    frame: SharedFrame,
    submission: wgpu::SubmissionIndex,
    done: Arc<AtomicBool>,
}

/// The shared counterpart of [`super::BgraReadback`]: the same submit and
/// collect shape, so the player drives either.
pub struct SharedFrames<T> {
    ctx: Arc<RenderContext>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    identity: DeviceIdentity,
    /// Buffers for `size`. Dropped from the pool on a resize; anyone still
    /// holding one keeps it alive until they let go.
    pool: Vec<Arc<SharedBuffer>>,
    size: (u32, u32),
    jobs: VecDeque<Job<T>>,
    depth: usize,
    stats: ReadbackStats,
}

impl<T> SharedFrames<T> {
    /// A ring that keeps up to `depth` frames on the GPU at once, or `None`
    /// when this device cannot export memory as an opaque fd — not Vulkan,
    /// or no `VK_KHR_external_memory_fd`. The caller reads back instead.
    pub fn new(ctx: Arc<RenderContext>, depth: usize) -> Option<Self> {
        let identity = device_identity(&ctx)?;
        // Prove the export works once, with a buffer too small to matter,
        // rather than finding out on the first frame.
        ExportableBuffer::with_handle(&ctx, 4096, "chukcut shared probe", ExportHandle::OpaqueFd)?;

        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut shared frame"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut shared frame"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut shared frame"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("chukcut shared frame"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        // One uniform for every job: `write_buffer` is ordered with `submit`,
        // so each submission sees the values written just before it.
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut shared frame params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Some(Self {
            ctx,
            pipeline,
            layout,
            params,
            identity,
            pool: Vec::new(),
            size: (0, 0),
            jobs: VecDeque::new(),
            depth: depth.max(1),
            stats: ReadbackStats::default(),
        })
    }

    pub fn identity(&self) -> DeviceIdentity {
        self.identity
    }

    pub fn in_flight(&self) -> usize {
        self.jobs.len()
    }

    pub fn has_room(&self) -> bool {
        self.jobs.len() < self.depth
    }

    /// Buffers the pool holds for the current size, in use or not.
    pub fn buffers(&self) -> usize {
        self.pool.len()
    }

    /// `wait_ns` is the time spent blocked on the GPU; `copy_ns` stays zero,
    /// there is no copy.
    pub fn stats(&self) -> ReadbackStats {
        self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = ReadbackStats::default();
    }

    /// Swizzle `target` into a free shared buffer and return at once. `Err`
    /// when the ring is full, when every buffer is still held and the pool is
    /// at [`MAX_BUFFERS`], or when the target cannot be read.
    pub fn submit(&mut self, target: &PooledTexture, tag: T) -> std::result::Result<(), T> {
        if !self.has_room() {
            return Err(tag);
        }
        let started = Instant::now();
        let size = (target.width(), target.height());
        let Some(view) = target.view_as(READ_FORMAT) else {
            tracing::warn!(format = ?target.format(), "a render target the shared path cannot read");
            return Err(tag);
        };
        let bytes_per_row = bytes_per_row(size.0);
        let Some(buffer) = self.free_buffer(size, bytes_per_row) else {
            return Err(tag);
        };

        self.ctx.queue().write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[size.0, size.1, bytes_per_row / 4, 0u32]),
        );
        let device = self.ctx.device();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut shared frame"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffer.buffer.buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut shared frame"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chukcut shared swizzle"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(16), size.1.div_ceil(16), 1);
        }
        let submission = self.ctx.queue().submit(Some(encoder.finish()));
        // Fires once the queue's last submission at registration is done —
        // this one, or another thread's that followed it. Late, never early,
        // which is all `try_collect` needs; `collect_oldest` waits on the
        // submission index itself.
        let done = Arc::new(AtomicBool::new(false));
        {
            let done = Arc::clone(&done);
            self.ctx
                .queue()
                .on_submitted_work_done(move || done.store(true, Ordering::Release));
        }
        self.jobs.push_back(Job {
            tag,
            frame: SharedFrame {
                width: size.0,
                height: size.1,
                bytes_per_row,
                frame_id: next_id(),
                buffer,
            },
            submission,
            done,
        });
        self.stats.submit_ns += started.elapsed().as_nanos() as u64;
        Ok(())
    }

    /// The oldest frame, if the GPU has finished it. Never blocks.
    pub fn try_collect(&mut self) -> Option<(T, Result<SharedFrame>)> {
        let job = self.jobs.front()?;
        if !job.done.load(Ordering::Acquire) {
            // Callbacks only run inside a poll; a non-blocking one is cheap.
            let _ = self.ctx.device().poll(wgpu::PollType::Poll);
            if !job.done.load(Ordering::Acquire) {
                return None;
            }
        }
        self.finish_oldest()
    }

    /// The oldest frame, waiting for the GPU if it has to.
    pub fn collect_oldest(&mut self) -> Option<(T, Result<SharedFrame>)> {
        let job = self.jobs.front()?;
        if !job.done.load(Ordering::Acquire) {
            let waited = Instant::now();
            let polled = self.ctx.device().poll(wgpu::PollType::Wait {
                submission_index: Some(job.submission.clone()),
                timeout: Some(Duration::from_secs(5)),
            });
            self.stats.wait_ns += waited.elapsed().as_nanos() as u64;
            match polled {
                // The submission is done, whatever the callback says: it
                // belongs to the last submit on the queue when it was
                // registered, which another thread's work can follow.
                Ok(_) => job.done.store(true, Ordering::Release),
                Err(error) => {
                    tracing::warn!(%error, "waiting for a shared preview frame failed");
                }
            }
        }
        self.finish_oldest()
    }

    /// Forget every frame in flight; their tags come back for accounting.
    pub fn discard_all(&mut self) -> Vec<T> {
        self.jobs.drain(..).map(|job| job.tag).collect()
    }

    fn finish_oldest(&mut self) -> Option<(T, Result<SharedFrame>)> {
        let job = self.jobs.pop_front()?;
        if !job.done.load(Ordering::Acquire) {
            // Only after a wait that timed out. Handing the frame out would
            // show half-written pixels; the pool will not reuse its buffer
            // before the GPU is done (`ExportableBuffer` waits in `Drop`, and
            // a later submission on this queue orders after this one).
            return Some((
                job.tag,
                Err(RenderError::Readback(
                    "the GPU did not finish a preview frame in time".into(),
                )),
            ));
        }
        self.stats.frames += 1;
        Some((job.tag, Ok(job.frame)))
    }

    /// A buffer nobody else holds, for frames of `size`, or a new one.
    fn free_buffer(&mut self, size: (u32, u32), bytes_per_row: u32) -> Option<Arc<SharedBuffer>> {
        if self.size != size {
            self.pool.clear();
            self.size = size;
        }
        if let Some(free) = self.pool.iter().find(|b| Arc::strong_count(b) == 1) {
            return Some(Arc::clone(free));
        }
        if self.pool.len() >= MAX_BUFFERS {
            tracing::warn!(
                held = self.pool.len(),
                "every shared preview buffer is still held; dropping a frame"
            );
            return None;
        }
        let bytes = u64::from(bytes_per_row) * u64::from(size.1);
        let buffer = ExportableBuffer::with_handle(
            &self.ctx,
            bytes,
            "chukcut shared preview frame",
            ExportHandle::OpaqueFd,
        )?;
        let shared = Arc::new(SharedBuffer {
            id: next_id(),
            buffer,
            identity: self.identity,
        });
        self.pool.push(Arc::clone(&shared));
        Some(shared)
    }
}

/// `width` BGRA pixels, rounded up to the copy alignment.
pub fn bytes_per_row(width: u32) -> u32 {
    (width * 4).next_multiple_of(ROW_ALIGNMENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::render::{Compositor, EmptySourceProvider as Nothing};

    /// Copy a shared frame's rows back out on the engine's own device, tightly
    /// packed, the way an importer would see them.
    fn read(ctx: &RenderContext, frame: &SharedFrame) -> Vec<u8> {
        let bytes = u64::from(frame.bytes_per_row) * u64::from(frame.height);
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("test staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device().create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(frame.buffer.buffer(), 0, &staging, 0, bytes);
        ctx.queue().submit(Some(encoder.finish()));
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let view = staging.slice(..).get_mapped_range().expect("mapped");
        let mut out = Vec::new();
        for row in view.chunks(frame.bytes_per_row as usize) {
            out.extend_from_slice(&row[..frame.width as usize * 4]);
        }
        out
    }

    fn solid_project(width: u32, height: u32) -> crate::modules::project::Project {
        crate::modules::project::Project::new(
            "shared",
            crate::modules::project::CanvasConfig {
                width,
                height,
                background: [0.2, 0.4, 0.6, 1.0],
            },
            30.0,
        )
    }

    #[test]
    fn rows_are_padded_for_a_texture_copy() {
        assert_eq!(bytes_per_row(64), 256);
        assert_eq!(bytes_per_row(65), 512);
        assert_eq!(bytes_per_row(1920), 7680);
        assert_eq!(bytes_per_row(3840), 15360);
        assert_eq!(bytes_per_row(161), 768);
    }

    /// The shared buffer holds what the readback would have returned, byte
    /// for byte, at an awkward width whose rows need padding.
    #[test]
    fn a_shared_frame_holds_the_same_bytes_as_the_readback() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let Some(mut shared) = SharedFrames::<u32>::new(Arc::clone(&ctx), 3) else {
            eprintln!("skipping: this device cannot export opaque memory");
            return;
        };
        let mut readback = super::super::BgraReadback::<u32>::new(Arc::clone(&ctx), 3);
        let compositor = Compositor::new(Arc::clone(&ctx));
        let project = solid_project(322, 180);
        let target = compositor
            .render_to_texture(&project, 0, (322, 180), &Nothing)
            .expect("render");
        shared.submit(&target, 7).expect("room");
        readback.submit(&target, 7).expect("room");
        compositor.pool().release(target);

        let (tag, frame) = shared.collect_oldest().expect("a frame");
        let frame = frame.expect("finished");
        assert_eq!(tag, 7);
        assert_eq!((frame.width, frame.height), (322, 180));
        assert_eq!(frame.bytes_per_row, 1536);
        let (_, expected) = readback.collect_oldest().expect("a frame");
        assert_eq!(read(&ctx, &frame), expected.expect("readback").data);
    }

    /// A buffer someone still holds is never written; a released one is.
    #[test]
    fn a_held_buffer_is_not_reused_and_a_released_one_is() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let Some(mut shared) = SharedFrames::<()>::new(Arc::clone(&ctx), 3) else {
            eprintln!("skipping: this device cannot export opaque memory");
            return;
        };
        let compositor = Compositor::new(Arc::clone(&ctx));
        let project = solid_project(64, 32);
        let mut frame = || {
            let target = compositor
                .render_to_texture(&project, 0, (64, 32), &Nothing)
                .expect("render");
            shared.submit(&target, ()).expect("room");
            compositor.pool().release(target);
            shared
                .collect_oldest()
                .expect("a frame")
                .1
                .expect("finished")
        };
        let first = frame();
        let second = frame();
        assert_ne!(first.buffer.id(), second.buffer.id(), "first is still held");
        assert_ne!(first.frame_id, second.frame_id);
        let held = second.buffer.id();
        let released = first.buffer.id();
        drop(first);
        let third = frame();
        assert_eq!(
            third.buffer.id(),
            released,
            "the released buffer comes back"
        );
        assert_ne!(third.buffer.id(), held);
        assert_eq!(shared.buffers(), 2);
    }

    #[test]
    fn the_exported_buffer_describes_itself_for_an_importer() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let Some(mut shared) = SharedFrames::<()>::new(Arc::clone(&ctx), 1) else {
            eprintln!("skipping: this device cannot export opaque memory");
            return;
        };
        let compositor = Compositor::new(Arc::clone(&ctx));
        let target = compositor
            .render_to_texture(&solid_project(100, 50), 0, (100, 50), &Nothing)
            .expect("render");
        shared.submit(&target, ()).expect("room");
        assert!(!shared.has_room(), "a depth of one is full with one job");
        compositor.pool().release(target);
        let (_, frame) = shared.collect_oldest().expect("a frame");
        let frame = frame.expect("finished");
        let buffer = &frame.buffer;
        assert!(buffer.fd() >= 0);
        assert_eq!(buffer.buffer_size(), 512 * 50);
        assert!(buffer.allocation_size() >= buffer.buffer_size());
        assert_eq!(buffer.identity(), shared.identity());
        assert_ne!(buffer.identity().device_uuid, [0; 16]);
    }
}
