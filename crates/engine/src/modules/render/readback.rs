//! Reading composited frames back as BGRA without stalling the thread that
//! renders them.
//!
//! The native player hands GPUI a `RenderImage`, which is BGRA bytes in system
//! memory, because GPUI draws with a wgpu device of its own and has no way yet
//! to sample a texture of ours (`docs/research/gpui-shared-texture.md`). So the
//! frame must come back. What does not have to happen is everything
//! [`Compositor::render_frame`](super::Compositor::render_frame) does on the
//! way:
//!
//! - **It blocks.** `map_async` and then `device.poll(wait)` on the render
//!   thread, so the GPU's composite, the copy and the map are all serial with
//!   the next frame's decode. Here a frame's copy is *queued* and collected
//!   later; the thread decodes the next frame meanwhile.
//! - **It pads and unpads.** `copy_texture_to_buffer` needs 256-byte rows, so
//!   every row was copied out separately. The swizzle pass below writes into a
//!   storage buffer with tight rows, and the copy-out is one `memcpy`.
//! - **It is RGBA.** The player used to swap R and B on the CPU, a full pass
//!   over every byte. The same compute pass that packs the rows writes BGRA.
//!
//! Each slot of the ring owns its buffers and reuses them while the frame size
//! stays the same, so a steady playback allocates no GPU memory at all.
//!
//! The pass reads the target through its `Rgba8Unorm` view ([`READ_FORMAT`]),
//! i.e. the stored sRGB-encoded bytes rather than linearised floats, and
//! `pack4x8unorm` writes them back bit-exact: what comes out is byte for byte
//! what `render_frame` returns, with R and B swapped. A test holds it to that.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::context::RenderContext;
use super::error::{RenderError, Result};
use super::nv12::READ_FORMAT;
use super::texture_pool::PooledTexture;

const SHADER: &str = r#"
struct Params { width: u32, height: u32 }

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> packed: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let c = textureLoad(source, vec2<i32>(i32(id.x), i32(id.y)), 0);
    // `pack4x8unorm` puts .x in the lowest byte, which is the first in memory:
    // B, G, R, A.
    packed[id.y * params.width + id.x] = pack4x8unorm(c.bgra);
}
"#;

/// Map states, written by wgpu's callback and read by the owner.
const PENDING: u8 = 0;
const MAPPED: u8 = 1;
const FAILED: u8 = 2;

/// One frame's worth of BGRA, tightly packed: `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgraFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Where the time of a collected frame went, in nanoseconds of wall clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadbackStats {
    pub frames: u64,
    /// Recording and submitting the swizzle pass and the copy.
    pub submit_ns: u64,
    /// Blocked waiting for a map that had not finished. Zero is the goal: it
    /// means the GPU finished while the thread was doing something useful.
    pub wait_ns: u64,
    /// The one `memcpy` out of the mapped buffer.
    pub copy_ns: u64,
}

struct Slot<T> {
    size: (u32, u32),
    storage: wgpu::Buffer,
    staging: wgpu::Buffer,
    params: wgpu::Buffer,
    /// `Some` while a frame is in flight in this slot.
    job: Option<Job<T>>,
}

struct Job<T> {
    tag: T,
    submission: wgpu::SubmissionIndex,
    state: Arc<AtomicU8>,
}

/// A ring of readback slots, each holding one frame from submission until it
/// is collected. `T` is whatever the caller wants back with the frame — a
/// timeline time, a generation.
pub struct BgraReadback<T> {
    ctx: Arc<RenderContext>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    slots: Vec<Option<Slot<T>>>,
    /// Slot indices in submission order, oldest first. Frames are collected in
    /// this order so a caller never sees frame N+1 before frame N.
    order: VecDeque<usize>,
    stats: ReadbackStats,
}

impl<T> BgraReadback<T> {
    /// A ring of `depth` slots. Two is double buffering: one frame being
    /// mapped while the next is composited. Three covers a GPU that is briefly
    /// a frame behind.
    pub fn new(ctx: Arc<RenderContext>, depth: usize) -> Self {
        let device = ctx.device();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chukcut bgra readback"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut bgra readback"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut bgra readback"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("chukcut bgra readback"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            ctx,
            pipeline,
            layout,
            slots: (0..depth.max(1)).map(|_| None).collect(),
            order: VecDeque::new(),
            stats: ReadbackStats::default(),
        }
    }

    /// Frames submitted and not yet collected.
    pub fn in_flight(&self) -> usize {
        self.order.len()
    }

    /// Whether [`Self::submit`] would find a free slot.
    pub fn has_room(&self) -> bool {
        self.order.len() < self.slots.len()
    }

    pub fn stats(&self) -> ReadbackStats {
        self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = ReadbackStats::default();
    }

    /// Queue `target` for readback and return at once.
    ///
    /// `target` must be a compositor render target (viewable as
    /// [`READ_FORMAT`]); it can go back to the pool as soon as this returns,
    /// because the copy has been submitted. `Err(tag)` when every slot is in
    /// flight — collect one first.
    pub fn submit(&mut self, target: &PooledTexture, tag: T) -> std::result::Result<(), T> {
        let Some(index) = self.free_slot() else {
            return Err(tag);
        };
        let started = Instant::now();
        let size = (target.width(), target.height());
        let Some(view) = target.view_as(READ_FORMAT) else {
            tracing::warn!(
                format = ?target.format(),
                "a render target that cannot be read as {READ_FORMAT:?} reached the readback"
            );
            return Err(tag);
        };
        self.ensure_slot(index, size);
        let device = self.ctx.device();
        let slot = self.slots[index].as_mut().expect("just ensured");

        self.ctx.queue().write_buffer(
            &slot.params,
            0,
            bytemuck::cast_slice(&[size.0, size.1, 0u32, 0u32]),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chukcut bgra readback"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: slot.storage.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: slot.params.as_entire_binding(),
                },
            ],
        });
        let bytes = frame_bytes(size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut bgra readback"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("chukcut bgra swizzle"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(16), size.1.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&slot.storage, 0, &slot.staging, 0, bytes);
        let submission = self.ctx.queue().submit(Some(encoder.finish()));

        let state = Arc::new(AtomicU8::new(PENDING));
        {
            let state = Arc::clone(&state);
            slot.staging
                .slice(0..bytes)
                .map_async(wgpu::MapMode::Read, move |result| {
                    state.store(
                        if result.is_ok() { MAPPED } else { FAILED },
                        Ordering::Release,
                    );
                });
        }
        slot.job = Some(Job {
            tag,
            submission,
            state,
        });
        self.order.push_back(index);
        self.stats.submit_ns += started.elapsed().as_nanos() as u64;
        Ok(())
    }

    /// The oldest frame, if its map has finished. Never blocks.
    pub fn try_collect(&mut self) -> Option<(T, Result<BgraFrame>)> {
        let &index = self.order.front()?;
        let done = self.job_state(index) != PENDING;
        if !done {
            // Callbacks only run inside a poll. A non-blocking one is cheap.
            let _ = self.ctx.device().poll(wgpu::PollType::Poll);
            if self.job_state(index) == PENDING {
                return None;
            }
        }
        Some(self.finish_oldest())
    }

    /// The oldest frame, waiting for it if it is still on the GPU. `None` when
    /// nothing is in flight.
    pub fn collect_oldest(&mut self) -> Option<(T, Result<BgraFrame>)> {
        let &index = self.order.front()?;
        if self.job_state(index) == PENDING {
            let waited = Instant::now();
            let submission = self.slots[index]
                .as_ref()
                .and_then(|slot| slot.job.as_ref())
                .map(|job| job.submission.clone());
            let polled = self.ctx.device().poll(wgpu::PollType::Wait {
                submission_index: submission,
                // Long enough for any real frame, short enough that a lost
                // device shows up as an error rather than a frozen player.
                timeout: Some(Duration::from_secs(5)),
            });
            self.stats.wait_ns += waited.elapsed().as_nanos() as u64;
            if let Err(error) = polled {
                tracing::warn!(%error, "waiting for a readback failed");
            }
        }
        Some(self.finish_oldest())
    }

    /// Forget every frame in flight. Their tags are returned so the caller can
    /// account for them; their pixels are never copied.
    pub fn discard_all(&mut self) -> Vec<T> {
        let mut tags = Vec::new();
        while let Some(index) = self.order.pop_front() {
            if let Some(slot) = self.slots[index].as_mut() {
                if let Some(job) = slot.job.take() {
                    // Unmapping a buffer whose map is still pending aborts the
                    // map; one that finished is simply released.
                    slot.staging.unmap();
                    tags.push(job.tag);
                }
            }
        }
        tags
    }

    fn job_state(&self, index: usize) -> u8 {
        self.slots[index]
            .as_ref()
            .and_then(|slot| slot.job.as_ref())
            .map_or(FAILED, |job| job.state.load(Ordering::Acquire))
    }

    fn finish_oldest(&mut self) -> (T, Result<BgraFrame>) {
        let index = self.order.pop_front().expect("caller checked");
        let slot = self.slots[index]
            .as_mut()
            .expect("an in-flight slot exists");
        let job = slot.job.take().expect("an in-flight slot has a job");
        let (width, height) = slot.size;
        let bytes = frame_bytes(slot.size);
        let result = match job.state.load(Ordering::Acquire) {
            MAPPED => {
                let started = Instant::now();
                let data = {
                    let view = slot.staging.slice(0..bytes).get_mapped_range();
                    view.map(|view| view.to_vec())
                };
                slot.staging.unmap();
                self.stats.copy_ns += started.elapsed().as_nanos() as u64;
                self.stats.frames += 1;
                data.map(|data| BgraFrame {
                    width,
                    height,
                    data,
                })
                .map_err(|error| RenderError::Readback(error.to_string()))
            }
            PENDING => {
                // Only after a timed-out wait. Abort the map so the slot can be
                // reused.
                slot.staging.unmap();
                Err(RenderError::Readback(
                    "the GPU did not finish a preview frame in time".into(),
                ))
            }
            _ => {
                slot.staging.unmap();
                Err(RenderError::Readback(
                    "mapping a preview frame for reading failed".into(),
                ))
            }
        };
        (job.tag, result)
    }

    fn free_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_none_or(|slot| slot.job.is_none()))
    }

    /// Slot `index`, with buffers for `size` — reused when the size has not
    /// changed, which during playback is always.
    fn ensure_slot(&mut self, index: usize, size: (u32, u32)) {
        let fits = self.slots[index]
            .as_ref()
            .is_some_and(|slot| slot.size == size);
        if !fits {
            let device = self.ctx.device();
            let bytes = frame_bytes(size);
            self.slots[index] = Some(Slot {
                size,
                storage: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chukcut bgra packed"),
                    size: bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                staging: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chukcut bgra staging"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                params: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chukcut bgra params"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                job: None,
            });
        }
    }
}

impl<T> Drop for BgraReadback<T> {
    fn drop(&mut self) {
        self.discard_all();
    }
}

fn frame_bytes(size: (u32, u32)) -> u64 {
    // Never zero: a zero-sized buffer is a validation error, and the compositor
    // refuses a zero-sized frame before it gets here anyway.
    (size.0.max(1) as u64 * size.1.max(1) as u64 * 4).max(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{
        CanvasConfig, Project, Segment, TimeRange, Track, TrackKind, Transform,
    };
    use crate::modules::render::source::{SolidColorProvider, SolidSource};
    use crate::modules::render::Compositor;

    /// A canvas whose width is not a multiple of 64, so the old path had to
    /// unpad rows, with one opaque and one half-transparent clip so blending
    /// and every channel are exercised.
    fn project() -> Project {
        let mut project = Project::new(
            "readback",
            CanvasConfig {
                width: 150,
                height: 90,
                background: [0.1, 0.2, 0.3, 1.0],
            },
            30.0,
        );
        let mut track = Track::new(TrackKind::Video, "V1");
        for (id, x, opacity) in [("a", -0.4, 1.0), ("b", 0.4, 0.5)] {
            track.segments.push(Segment {
                id: id.into(),
                material_id: id.into(),
                target_range: TimeRange::new(0, 1_000_000),
                source_range: TimeRange::new(0, 1_000_000),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                transform: Transform {
                    position: [x, 0.1],
                    scale: [0.5, 0.7],
                    opacity,
                    ..Transform::default()
                },
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            });
        }
        project.tracks.push(track);
        project
    }

    #[test]
    fn bgra_readback_is_the_rgba_readback_with_red_and_blue_swapped() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let project = project();
        let sources = SolidColorProvider::new()
            .with("a", SolidSource::new([0.9, 0.5, 0.1, 1.0], 40, 30))
            .with("b", SolidSource::new([0.2, 0.8, 0.6, 1.0], 40, 30));
        let compositor = Compositor::new(Arc::clone(&ctx));
        let size = (150, 90);
        let rgba = compositor
            .render_frame(&project, 0, size, &sources)
            .unwrap();

        let mut readback = BgraReadback::new(Arc::clone(&ctx), 2);
        let target = compositor
            .render_to_texture(&project, 0, size, &sources)
            .unwrap();
        readback.submit(&target, "frame").ok().unwrap();
        compositor.pool().release(target);
        let (tag, frame) = readback.collect_oldest().unwrap();
        let frame = frame.unwrap();
        assert_eq!(tag, "frame");
        assert_eq!((frame.width, frame.height), size);

        let mut expected = rgba;
        for pixel in expected.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        assert_eq!(frame.data, expected);
    }

    #[test]
    fn frames_come_back_in_submission_order_and_the_ring_refuses_when_full() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let project = project();
        let sources = SolidColorProvider::new()
            .with("a", SolidSource::new([1.0, 0.0, 0.0, 1.0], 8, 8))
            .with("b", SolidSource::new([0.0, 1.0, 0.0, 1.0], 8, 8));
        let compositor = Compositor::new(Arc::clone(&ctx));
        let mut readback = BgraReadback::new(Arc::clone(&ctx), 2);
        for n in 0..2 {
            let target = compositor
                .render_to_texture(&project, 0, (64, 32), &sources)
                .unwrap();
            assert!(readback.submit(&target, n).is_ok());
            compositor.pool().release(target);
        }
        let target = compositor
            .render_to_texture(&project, 0, (64, 32), &sources)
            .unwrap();
        assert_eq!(readback.submit(&target, 2), Err(2), "two slots, both busy");
        assert_eq!(readback.collect_oldest().unwrap().0, 0);
        assert!(readback.submit(&target, 2).is_ok());
        compositor.pool().release(target);
        assert_eq!(readback.collect_oldest().unwrap().0, 1);
        assert_eq!(readback.collect_oldest().unwrap().0, 2);
        assert!(readback.collect_oldest().is_none());
        assert_eq!(readback.discard_all(), Vec::<i32>::new());
    }
}
