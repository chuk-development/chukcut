//! The preview frame that never leaves the GPU.
//!
//! ## What this deletes
//!
//! The preview used to composite into a GPU texture, read 8 MB of RGBA back to
//! system memory, convert it to NV12 on twelve rayon workers, and upload 3 MB of
//! that straight back into a VA surface for the fixed-function JPEG encoder to
//! read. Measured at 1080×1920 on the Raptor Lake iGPU
//! (`docs/research/preview-performance.md`): readback 5.10 ms + convert 1.81 ms +
//! upload 2.59 ms = **9.50 ms of a 15.82 ms frame**, against 0.81 ms of
//! decoding and 0.48 ms of compositing. The encoder itself was 1.59 ms — 28% of
//! its own path — and the rest was carrying pixels to a chip that already had
//! them.
//!
//! Here the compute pass writes NV12 into a buffer whose memory was exported as
//! a DMA-BUF, and the encoder is handed that same memory. Nothing crosses the
//! bus.
//!
//! Every piece of it already shipped in the export
//! (`export::job::ZeroCopy`); what is new is the ownership, because the preview
//! encodes on a **different thread** from the one that composites.
//!
//! ## The rule the whole file exists to keep
//!
//! **The compute pass must not write a buffer the encoder is still reading.**
//! The import is a wrap, not a copy, so overwriting one gives a frame torn
//! between two compositions — intermittent, content-dependent, and looking
//! exactly like a bad shader.
//!
//! The export can rotate blindly because its frames retire in order on one
//! thread. The preview cannot, so each slot carries a state and a slot is only
//! handed out when two independent things agree that it is idle:
//!
//! - **this process** is done with it — the encode job has run and given the
//!   slot back; and
//! - **libavutil** is done with it — `surface_is_shared` asks the reference
//!   count on the surface that was mapped over it, which is the authority. The
//!   slot count only makes the answer usually "yes".
//!
//! A frame that can find no idle slot does not wait: it says so and the caller
//! composites it the copying way. Stalling the render thread behind the encode
//! thread would undo the pipelining that is the reason there are two of them.
//!
//! ## What is deliberately *not* here
//!
//! There is no software fallback from a failed zero-copy encode, and there
//! cannot be: the buffer is device-local memory that nothing on the CPU can
//! read. So the decision to take this path is made **before** compositing, and a
//! failure afterwards costs one frame and switches the path off for the rest of
//! the process. `docs/research/zero-copy-encode.md` records the trap of a
//! "zero-copy" path that silently falls back to a copy; this one falls back
//! loudly and only once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ffmpeg_next::util::frame;
use parking_lot::Mutex;

use crate::modules::export::hwframes::{surface_is_shared, Nv12Dmabuf};
use crate::modules::render::dmabuf::{Nv12Ring, DRM_FORMAT_MOD_LINEAR};
use crate::modules::render::{Nv12Layout, RenderContext};

/// Buffers in the rotation.
///
/// The render thread holds one while it composites, up to
/// `server::MAX_PENDING_ENCODES` are queued, one is being encoded, and one more
/// may still be mapped by a driver that has not let go. Six covers that with a
/// slot to spare, and at 1080p it is 19 MB rather than the export ring's 50.
pub const SLOTS: usize = 6;

/// What a slot's memory is doing.
enum SlotState {
    /// Nothing is reading or writing it.
    Idle,
    /// The render thread is compositing into it, or has and the encode has not
    /// finished. Either way it may not be handed out.
    Busy,
    /// The encode finished and left a surface mapped over the memory. Reusable
    /// as soon as libavutil says nothing else holds it.
    Mapped(frame::Video),
}

/// A rotation of exported buffers, shared by the render and encode threads.
///
/// `Arc`-shared rather than owned by the render thread because the encode job
/// runs elsewhere and needs the file descriptor: an `ExportableBuffer` lends its
/// fd rather than giving it away, so the ring has to outlive every job it
/// handed a slot to.
pub struct PreviewRing {
    ring: Nv12Ring,
    slots: Mutex<Vec<SlotState>>,
    /// The size the buffers were allocated for. A frame smaller than this fits
    /// — [`Nv12Layout::total_bytes`] is monotone in both edges — and a larger
    /// one needs a new ring.
    allocated_for: (u32, u32),
}

impl PreviewRing {
    /// Allocate a rotation big enough for a `size` frame.
    ///
    /// `None` on a device that cannot export DMA-BUF memory, which is a reason
    /// to use the copying path and not a reason to fail.
    pub fn new(ctx: &Arc<RenderContext>, size: (u32, u32)) -> Option<Self> {
        let bytes = Nv12Layout::for_size(size.0, size.1).total_bytes() as u64;
        let ring = Nv12Ring::with_slots(ctx, bytes, SLOTS)?;
        Some(Self {
            ring,
            slots: Mutex::new((0..SLOTS).map(|_| SlotState::Idle).collect()),
            allocated_for: size,
        })
    }

    /// Whether a `size` frame fits the buffers this was allocated for.
    pub fn holds(&self, size: (u32, u32)) -> bool {
        size.0 <= self.allocated_for.0 && size.1 <= self.allocated_for.1
    }

    pub fn allocated_for(&self) -> (u32, u32) {
        self.allocated_for
    }

    /// Take an idle slot, or `None` if every one of them is still in use.
    ///
    /// Marks the slot [`SlotState::Busy`], so it stays reserved from here until
    /// [`Self::give_back`]. **Every path out of a claim must reach that call**
    /// or the slot leaks and the ring shrinks by one for the rest of the
    /// session; [`Claim`] is the thing that makes that hard to get wrong.
    pub fn claim(self: &Arc<Self>) -> Option<Claim> {
        let mut slots = self.slots.lock();
        let index = slots.iter().position(|slot| match slot {
            SlotState::Idle => true,
            SlotState::Busy => false,
            // The reference count is the authority. A driver that is still
            // reading the surface holds a reference, and writing anyway would
            // tear the picture it is reading.
            SlotState::Mapped(surface) => !surface_is_shared(surface),
        })?;
        slots[index] = SlotState::Busy;
        Some(Claim {
            ring: Arc::clone(self),
            index,
            released: false,
        })
    }

    /// The wgpu handle the compute pass writes into.
    pub fn buffer(&self, index: usize) -> &wgpu::Buffer {
        self.ring.at(index).buffer()
    }

    /// Describe slot `index` to libavutil, for a frame of `layout`.
    ///
    /// The strides are [`Nv12Layout`]'s, which is what makes this sound: they
    /// are `render::nv12::ROW_ALIGN`-aligned, and a VAAPI *encoder* reads a
    /// plane with the pitch it wanted rather than the pitch it was given. That
    /// bug destroyed every export at a width that was not a multiple of 64 while
    /// every round-trip test stayed green, because the *importer* honours the
    /// pitch and only the encode engine does not. See the doc on `ROW_ALIGN`.
    pub fn describe(&self, index: usize, layout: &Nv12Layout) -> Nv12Dmabuf<'_> {
        Nv12Dmabuf {
            // Borrowed, not duplicated: `ExportableBuffer` owns the descriptor
            // and outlives every frame made from it, and the lifetime here says
            // so — a `Nv12Dmabuf` cannot escape the ring it describes.
            fd: self.ring.at(index).fd(),
            size: layout.total_bytes(),
            width: layout.width,
            height: layout.height,
            y_offset: 0,
            y_stride: layout.y_stride,
            uv_offset: layout.uv_offset(),
            uv_stride: layout.uv_stride,
            modifier: DRM_FORMAT_MOD_LINEAR,
        }
    }

    /// Release a slot, keeping whatever surface was mapped over it.
    fn give_back(&self, index: usize, surface: Option<frame::Video>) {
        let mut slots = self.slots.lock();
        slots[index] = match surface {
            Some(surface) => SlotState::Mapped(surface),
            None => SlotState::Idle,
        };
    }

    /// How many slots are neither busy nor still held by a driver. For tests.
    #[cfg(test)]
    pub fn idle_slots(&self) -> usize {
        self.slots
            .lock()
            .iter()
            .filter(|slot| match slot {
                SlotState::Idle => true,
                SlotState::Busy => false,
                SlotState::Mapped(surface) => !surface_is_shared(surface),
            })
            .count()
    }
}

impl std::fmt::Debug for PreviewRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewRing")
            .field("slots", &SLOTS)
            .field("allocated_for", &self.allocated_for)
            .finish()
    }
}

/// One slot, reserved.
///
/// The slot goes back when this is dropped, which is what makes "every path out
/// of a claim releases it" true by construction rather than by review — and
/// there are several such paths, because a frame can fail to composite, be
/// judged too late to encode, or be handed to a thread that then finds the
/// encoder gone.
///
/// Moved into the encode job, so the release happens on whichever thread ran the
/// encode. That is deliberate: the surface the encoder produced has to be stored
/// with the slot, and it is that thread that has it.
pub struct Claim {
    ring: Arc<PreviewRing>,
    index: usize,
    /// Set by [`Self::release`]. Without it the `Drop` below would hand the slot
    /// back a second time and throw away the surface that was just stored with
    /// it — which would let the *next* claimant overwrite memory a driver is
    /// still reading, i.e. exactly the corruption this file exists to prevent.
    released: bool,
}

impl Claim {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn ring(&self) -> &Arc<PreviewRing> {
        &self.ring
    }

    /// Give the slot back, remembering the surface mapped over it so the next
    /// claimant can ask libavutil whether the driver has finished with it.
    pub fn release(mut self, surface: Option<frame::Video>) {
        self.ring.give_back(self.index, surface);
        self.released = true;
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        // Reached when a claim is abandoned without an encode — a frame that
        // failed to composite, or one the playhead passed while it was being
        // made. Nothing was mapped over the memory, so the slot is immediately
        // reusable.
        if !self.released {
            self.ring.give_back(self.index, None);
        }
    }
}

// ---------------------------------------------------------------------------
// Asking the encoder whether it can actually read this
// ---------------------------------------------------------------------------

/// Whether this machine's JPEG encoder reads a linear imported surface
/// correctly, asked by trying it.
///
/// **On the Raptor Lake iGPU this repository is developed on, the answer is
/// no**, and the failure is the reason this function exists rather than a
/// comment. Measured with `examples/preview_zerocopy.rs` and pinned down with a
/// row-ramp probe: given one DMA-BUF holding linear NV12, with every stride,
/// offset and modifier the same,
///
/// | encoder | output row 0 | 31 | 32 | 63 | 128 |
/// |---|---|---|---|---|---|
/// | `h264_vaapi`, the export's | 0 | 31 | 32 | 63 | 128 |
/// | uploaded into a pool surface | 0 | 31 | 32 | 63 | 128 |
/// | **`mjpeg_vaapi`, the preview's** | **15** | **15** | **47** | **47** | **143** |
///
/// where the source row's luma *is* its row number. Every block of 32 output
/// rows comes back as the mean of the 32 source rows under it, which is what
/// reading linear memory as 32-row-tiled looks like. The picture is a fine
/// coloured mosaic. So the DMA-BUF, the layout, the modifier and libavutil's
/// import are all correct — the same buffer encodes perfectly through the video
/// encoder in the same process — and it is the fixed-function *JPEG* engine that
/// insists on the driver's own tiling. There is nothing to fix on our side of
/// the boundary; a tiled destination would mean allocating the NV12 as a
/// `VK_EXT_image_drm_format_modifier` image, which is the dance
/// `render::dmabuf`'s header explains this design avoided.
///
/// Nothing about that is a property of *this codebase*, so it is asked rather
/// than assumed: another driver, another chip, or a later iHD may read it
/// correctly, and this costs one encode of a 256x128 ramp, once.
///
/// The alternative — enabling the path and trusting it — is the trap
/// `docs/research/zero-copy-encode.md` names: a "zero-copy" path that silently
/// does the wrong thing is worse than not having one. Here it does not fall back
/// silently either way. It proves itself or it says why it will not be used.
pub fn encoder_can_read_linear(ctx: &Arc<RenderContext>) -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| match probe(ctx) {
        Ok(()) => {
            tracing::info!(
                "the hardware JPEG encoder reads the compositor's own memory correctly; \
                 preview frames will not be copied to the CPU"
            );
            true
        }
        Err(why) => {
            tracing::info!(
                reason = %why,
                "the hardware JPEG encoder cannot read the compositor's memory directly; \
                 preview frames will be read back and converted on the CPU"
            );
            false
        }
    })
}

/// Encode one known picture through the zero-copy path and check what comes
/// back.
///
/// A **row ramp**, deliberately: luma is the row number, so a decoded row's mean
/// says which source row it came from, and the one failure this is looking for —
/// a tiled misread — turns that into the mean of each 32-row block. A flat
/// picture, a gradient along X, or a PSNR against a reference would all survive
/// it. Chroma is a constant 128, so a colour fault cannot be mistaken for a
/// layout one.
fn probe(ctx: &Arc<RenderContext>) -> std::result::Result<(), String> {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;

    let layout = Nv12Layout::for_size(WIDTH, HEIGHT);
    let total = layout.total_bytes();
    let (w, h) = (WIDTH as usize, HEIGHT as usize);

    let mut picture = vec![128u8; total];
    for row in 0..h {
        picture[row * layout.y_stride..][..w].fill(row as u8);
    }

    let ring = Nv12Ring::with_slots(ctx, total as u64, 1)
        .ok_or("this device cannot export DMA-BUF memory")?;
    let exported = ring.at(0);

    // Filled through wgpu rather than through the compute pass, so this asks
    // exactly one question — can the encoder read what is in the buffer — and
    // does not also depend on the shader being right. `render::nv12`'s own tests
    // cover the shader.
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("chukcut zero-copy probe"),
        size: total as u64,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    staging
        .slice(..)
        .get_mapped_range_mut()
        .map_err(|e| e.to_string())?
        .copy_from_slice(&picture);
    staging.unmap();
    let mut encoder = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut zero-copy probe"),
        });
    encoder.copy_buffer_to_buffer(&staging, 0, exported.buffer(), 0, total as u64);
    ctx.queue().submit(Some(encoder.finish()));
    // The stall libva has no other way to get; see `import_nv12_dmabuf`.
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;

    let described = Nv12Dmabuf {
        fd: exported.fd(),
        size: total,
        width: WIDTH,
        height: HEIGHT,
        y_offset: 0,
        y_stride: layout.y_stride,
        uv_offset: layout.uv_offset(),
        uv_stride: layout.uv_stride,
        modifier: DRM_FORMAT_MOD_LINEAR,
    };
    let (jpeg, surface) = super::encoder::encode_preview_jpeg_dmabuf(&described, 95)
        .ok_or("the hardware JPEG encoder would not encode an imported surface at all")?;
    // Dropped before the ring, which is what lets the ring free its memory.
    drop(surface);

    let (rgb, got_w, got_h) = super::encoder::decode_jpeg_rgb(&jpeg)
        .map_err(|e| format!("the result is not a JPEG: {e}"))?;
    if (got_w, got_h) != (WIDTH, HEIGHT) {
        return Err(format!(
            "it came back {got_w}x{got_h} rather than {WIDTH}x{HEIGHT}"
        ));
    }

    for row in 0..h {
        let mean: u32 = (0..w)
            .map(|col| rgb[(row * w + col) * 3] as u32)
            .sum::<u32>()
            / w as u32;
        // Six counts covers JPEG quantisation on a flat row at quality 95 and
        // the round trip through full-range NV12. The failure it is looking for
        // is off by 15 at row 0 and by more further in.
        if mean.abs_diff(row as u32) > 6 {
            return Err(format!(
                "row {row} came back at luma {mean} instead of {row}, so the encoder is not \
                 reading the buffer as the linear NV12 it is (a 32-row block mean is what \
                 reading it as tiled looks like)"
            ));
        }
    }
    Ok(())
}

/// Whether the preview may hand the JPEG encoder the compositor's own memory.
///
/// The kill switch, in the shape `export::job::set_zero_copy` established: a
/// process-wide flag so that both arms of the change can be run in one process
/// rather than compared against a build that no longer exists.
///
/// Nothing calls it today — `examples/preview_zerocopy.rs` measures the two
/// paths by calling each directly, which is more direct — and it is here because
/// the first thing anyone will want when this path misbehaves on a machine we
/// cannot see is a way to turn it off without a rebuild.
///
/// It is an *upper* bound and not a decision: a machine still has to pass
/// [`encoder_can_read_linear`].
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Set once a zero-copy encode has failed, which switches the path off for the
/// rest of the process.
///
/// Not a counter and not per size: an import that the driver refuses is refused
/// deterministically, so a second attempt buys nothing and costs a frame. The
/// one thing this must not do is flap — a path that silently alternates between
/// zero-copy and a readback is the hardest kind of performance bug to read off a
/// log.
static WRITTEN_OFF: AtomicBool = AtomicBool::new(false);

pub fn write_off(why: &str) {
    if !WRITTEN_OFF.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            reason = why,
            "the preview's zero-copy JPEG path failed; every later frame reads back"
        );
    }
}

pub fn written_off() -> bool {
    WRITTEN_OFF.load(Ordering::Relaxed)
}

/// Forget the write-off, so a test or a benchmark can measure both arms.
pub fn reset() {
    WRITTEN_OFF.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(size: (u32, u32)) -> Option<Arc<PreviewRing>> {
        let ctx = crate::modules::render::test_context()?;
        PreviewRing::new(&ctx, size).map(Arc::new)
    }

    #[test]
    fn a_claim_reserves_a_slot_and_a_drop_gives_it_back() {
        let Some(ring) = ring((64, 64)) else {
            eprintln!("skipping: no GPU adapter, or this device cannot export DMA-BUF memory");
            return;
        };
        assert_eq!(ring.idle_slots(), SLOTS);
        let claim = ring.claim().expect("a fresh ring has slots");
        assert_eq!(ring.idle_slots(), SLOTS - 1);
        drop(claim);
        assert_eq!(ring.idle_slots(), SLOTS);
    }

    #[test]
    fn no_two_live_claims_name_the_same_slot() {
        // The whole reason the state lives in a mutex rather than in a cursor:
        // two claims outstanding at once must not describe the same memory, or
        // the compute pass overwrites a frame the encoder is reading.
        let Some(ring) = ring((64, 64)) else {
            eprintln!("skipping: no GPU adapter, or this device cannot export DMA-BUF memory");
            return;
        };
        let claims: Vec<Claim> = (0..SLOTS).map(|_| ring.claim().expect("slot")).collect();
        let mut indices: Vec<usize> = claims.iter().map(Claim::index).collect();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), SLOTS, "a slot was handed out twice");

        // And the ring refuses rather than blocking or wrapping round.
        assert!(ring.claim().is_none(), "a full ring must say so");
        drop(claims);
        assert!(ring.claim().is_some());
    }

    #[test]
    fn the_descriptor_carries_the_layouts_own_strides() {
        // A width that is not a multiple of 64, which is the case the export's
        // stride bug destroyed. The descriptor must hand libavutil the padded
        // stride and not the frame width.
        let Some(ring) = ring((1440, 1080)) else {
            eprintln!("skipping: no GPU adapter, or this device cannot export DMA-BUF memory");
            return;
        };
        let layout = Nv12Layout::for_size(1440, 1080);
        let claim = ring.claim().expect("slot");
        let described = ring.describe(claim.index(), &layout);
        assert_eq!(described.y_stride, layout.y_stride);
        assert_eq!(described.uv_stride, layout.uv_stride);
        assert_eq!(described.uv_offset, layout.uv_offset());
        assert_eq!(described.y_offset, 0);
        assert_eq!(described.modifier, DRM_FORMAT_MOD_LINEAR);
        assert!(
            described.y_stride > 1440,
            "1440 is not 128-aligned, so the stride has to be padded"
        );
    }

    /// What the compute pass actually left in the exported buffer.
    ///
    /// This is the test that says which side of the DMA-BUF a fault is on, and
    /// it earned its place: the first working version of this path produced a
    /// scrambled JPEG at *every* size, aligned ones included, and there are two
    /// entirely different explanations for that — the shader wrote the wrong
    /// bytes, or the encoder read the right ones wrongly. It reads the exported
    /// memory back through wgpu, which is the one route that involves neither
    /// libva nor the encoder, and compares it against the CPU converter.
    #[test]
    fn the_compute_pass_leaves_correct_nv12_in_the_exported_buffer() {
        use crate::modules::render::nv12::{Nv12Converter, READ_FORMAT};
        use crate::modules::render::source::YuvRange;
        use crate::modules::render::texture_pool::{TextureKey, TexturePool};

        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        // Not a multiple of 64, so the padded stride is exercised too.
        let (width, height) = (140u32, 36u32);
        let Some(ring) = PreviewRing::new(&ctx, (width, height)).map(Arc::new) else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            return;
        };

        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let noise = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
                rgba.extend_from_slice(&[(x * 255 / width) as u8, noise, (y * 7) as u8, 255]);
            }
        }

        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                READ_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let claim = ring.claim().expect("slot");
        let layout = Nv12Layout::for_size(width, height);
        Nv12Converter::new(&ctx)
            .convert_into_range(&ctx, &target, ring.buffer(claim.index()), YuvRange::Full)
            .expect("convert into the exported buffer");

        // Back out through wgpu, which touches nothing libva owns.
        let total = layout.total_bytes() as u64;
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("exported readback"),
            size: total,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(ring.buffer(claim.index()), 0, &staging, 0, total);
        ctx.queue().submit(Some(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        rx.recv().expect("callback").expect("map");
        let got = slice.get_mapped_range().expect("range").to_vec();
        staging.unmap();

        let (w, h) = (width as usize, height as usize);
        let mut y = vec![0u8; layout.y_stride * h];
        let mut uv = vec![0u8; layout.uv_stride * h.div_ceil(2)];
        crate::modules::preview::vaapi::rgba_to_nv12(
            &rgba,
            w,
            h,
            &mut y,
            layout.y_stride,
            &mut uv,
            layout.uv_stride,
        );

        for row in 0..h {
            for col in 0..w {
                let (a, b) = (
                    got[row * layout.y_stride + col],
                    y[row * layout.y_stride + col],
                );
                assert!(
                    a.abs_diff(b) <= 1,
                    "luma at {col},{row}: exported {a}, CPU {b}"
                );
            }
        }
        for row in 0..h / 2 {
            for col in 0..w {
                let a = got[layout.uv_offset() + row * layout.uv_stride + col];
                let b = uv[row * layout.uv_stride + col];
                assert!(
                    a.abs_diff(b) <= 1,
                    "chroma byte {col} of row {row}: exported {a}, CPU {b}"
                );
            }
        }
    }

    /// The self-check has to agree with what the encoder actually does.
    ///
    /// It cannot assert *which* answer this machine gives — that is the whole
    /// point of asking at run time — so it asserts the implication: if the probe
    /// says the encoder reads linear memory, then a second, independently built
    /// picture must come back right, and if it says the opposite, that picture
    /// must come back wrong. A probe that said yes and was wrong would put a
    /// scrambled preview in front of the user; one that said no and was wrong
    /// would cost 60% of the frame budget for nothing.
    #[test]
    fn the_self_check_agrees_with_what_the_encoder_actually_does() {
        use crate::modules::render::nv12::{Nv12Converter, READ_FORMAT};
        use crate::modules::render::source::YuvRange;
        use crate::modules::render::texture_pool::{TextureKey, TexturePool};

        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        if !crate::modules::preview::encoder::hardware_available() {
            eprintln!("skipping: no hardware JPEG encoder");
            return;
        }
        let verdict = encoder_can_read_linear(&ctx);
        eprintln!("this machine's JPEG encoder reads linear imported memory: {verdict}");

        // A grey row ramp again, but built the way a real frame is: RGB through
        // the compositor's own compute pass rather than memcpy'd in. So this
        // exercises the shader, the exported buffer, the import and the encoder
        // together, which is what the preview does.
        let (width, height) = (256u32, 128u32);
        let Some(ring) = PreviewRing::new(&ctx, (width, height)).map(Arc::new) else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            assert!(
                !verdict,
                "the probe cannot say yes on a device with no export"
            );
            return;
        };

        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for _ in 0..width {
                rgba.extend_from_slice(&[y as u8, y as u8, y as u8, 255]);
            }
        }
        let pool = TexturePool::default();
        let target = pool.acquire(
            ctx.device(),
            TextureKey::new(
                width,
                height,
                READ_FORMAT,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            ),
        );
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let claim = ring.claim().expect("slot");
        let layout = Nv12Layout::for_size(width, height);
        Nv12Converter::new(&ctx)
            .convert_into_range(&ctx, &target, ring.buffer(claim.index()), YuvRange::Full)
            .expect("convert");
        let encoded = {
            let described = ring.describe(claim.index(), &layout);
            crate::modules::preview::encoder::encode_preview_jpeg_dmabuf(&described, 95)
        };
        let Some((jpeg, surface)) = encoded else {
            assert!(
                !verdict,
                "the probe said yes but the encode refused the surface"
            );
            claim.release(None);
            return;
        };
        claim.release(Some(surface));

        let (rgb, ..) =
            crate::modules::preview::encoder::decode_jpeg_rgb(&jpeg).expect("decode the JPEG");
        let (w, h) = (width as usize, height as usize);
        let worst = (0..h)
            .map(|row| {
                let mean: u32 = (0..w)
                    .map(|col| rgb[(row * w + col) * 3] as u32)
                    .sum::<u32>()
                    / w as u32;
                mean.abs_diff(row as u32)
            })
            .max()
            .unwrap_or(0);

        if verdict {
            assert!(
                worst <= 8,
                "the probe said the encoder reads linear memory, but a row came back {worst} \
                 counts out — the preview would be showing a scrambled picture"
            );
        } else {
            assert!(
                worst > 8,
                "the probe said the encoder cannot read linear memory, but the picture came \
                 back right (worst row off by {worst}) — the preview is paying for a readback \
                 it does not need"
            );
        }
    }

    #[test]
    fn a_ring_holds_anything_no_bigger_than_it_was_allocated_for() {
        let Some(ring) = ring((1080, 1920)) else {
            eprintln!("skipping: no GPU adapter, or this device cannot export DMA-BUF memory");
            return;
        };
        assert!(ring.holds((1080, 1920)));
        assert!(ring.holds((540, 960)));
        assert!(!ring.holds((1920, 1080)), "wider than it was allocated for");
        assert!(!ring.holds((1080, 1921)));
    }
}
