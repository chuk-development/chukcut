//! The preview frame that never leaves the GPU, allocated the other way round.
//!
//! ## Why there are two of these
//!
//! [`super::zerocopy`] allocates the NV12 destination as a Vulkan buffer,
//! exports it as a `DRM_FORMAT_MOD_LINEAR` DMA-BUF and lets the media driver
//! import it. That is the arrangement the export uses and it is measured at
//! 2.4–3.1× on the preview's serial frame — and **Intel's fixed-function JPEG
//! engine cannot read it.** It treats a linear imported surface as though it
//! were 32-row tiled, while the H.264 engine on the same chip reads the same
//! file descriptor correctly. `docs/research/preview-zerocopy-jpeg.md` is the
//! whole diagnosis.
//!
//! This module goes the other direction and the tiling question disappears:
//!
//! ```text
//!   av_hwframe_get_buffer      VAAPI allocates an NV12 surface
//!        │                     — whatever tiling its own encoder wants
//!        │  av_hwframe_map(READ|WRITE|DIRECT) ──► vaExportSurfaceHandle
//!        ▼
//!   two DRM planes: R8 at offset 0, GR88 further in, one modifier
//!        │
//!        │  render::dmabuf::import_plane_with(PlaneUse::Write)
//!        ▼
//!   two wgpu colour attachments the compositor draws NV12 into
//!        │
//!        ▼
//!   the *same* VA surface, handed back to mjpeg_vaapi
//! ```
//!
//! The layout is the driver's own by construction, so nothing on this side ever
//! has to know or declare what it is. What the driver actually picks, on the
//! Raptor Lake iGPU this was written against: `I915_FORMAT_MOD_Y_TILED`
//! (`0x0100000000000002`) for every entrypoint, and **it ignores
//! `VASurfaceAttribDRMFormatModifiers` entirely** — asking for `LINEAR`,
//! `X_TILED`, `4_TILED` or even `DRM_FORMAT_MOD_INVALID` all return the same
//! Y-tiled surface. That is why declaring a different modifier on the buffer
//! route could never have worked.
//!
//! ## What has to be true on the other side, and is asked rather than assumed
//!
//! Vulkan must accept `R8_UNORM` and `R8G8_UNORM` images with the driver's
//! modifier **and colour-attachment usage**. A driver may expose a modifier for
//! sampling and refuse to render into it; ANV on this chip reports
//! `SAMPLED | STORAGE | COLOR_ATTACHMENT` for both formats under `LINEAR`,
//! `X_TILED` and `Y_TILED`. Where that is not true the import returns `None`
//! and the preview reads frames back exactly as before.
//!
//! ## The rule this file exists to keep
//!
//! **The compositor must not draw a surface the encoder is still reading.** The
//! import is a wrap, not a copy. Each slot therefore carries its surface inside
//! the state machine rather than beside it: a claim takes the surface *out*, and
//! it can only go back when the encode has finished with it. A frame that finds
//! no idle slot does not wait — it says so and the caller composites the copying
//! way, because stalling the render thread behind the encode thread would undo
//! the pipelining that is the reason there are two of them.
//!
//! ## What is deliberately *not* here
//!
//! No software fallback from a failed encode, for the same reason as in
//! `zerocopy`: the surface is device-local and nothing on the CPU can read it.
//! The decision to take this path is made **before** the frame is composited,
//! and a failure afterwards costs one frame and switches the path off for the
//! rest of the process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ffmpeg_next::util::frame;
use parking_lot::Mutex;

use crate::modules::export::hwframes::{surface_is_shared, HwDeviceContext, HwFramesContext};
use crate::modules::media::dmabuf::DmabufFrame;
use crate::modules::render::dmabuf::{import_plane_with, PlaneUse};
use crate::modules::render::nv12::{CHROMA_FORMAT, LUMA_FORMAT};
use crate::modules::render::RenderContext;

/// Surfaces in the rotation.
///
/// The render thread holds one while it composites, up to
/// `server::MAX_PENDING_ENCODES` are queued, and one is being encoded. Six
/// covers that with a slot to spare. A 1080p NV12 surface is 3.1 MB, so the
/// whole rotation is 19 MB.
pub const SLOTS: usize = 6;

/// What a slot's surface is doing.
enum SlotState {
    /// Nobody is reading or writing it, and here it is. The surface lives
    /// *inside* the state rather than beside it so that a claim moving it out is
    /// what makes "two claims cannot name one surface" true by construction.
    Idle(frame::Video),
    /// Claimed: being composited into, queued for encode, or being encoded.
    Busy,
}

/// The two colour attachments one surface's planes were imported as.
///
/// Built once at ring construction and never rebuilt: importing costs a
/// `vaExportSurfaceHandle`, two `vkAllocateMemory` and two `vkCreateImage`, and
/// doing it per frame would be a large part of what this path is saving.
struct Planes {
    luma: wgpu::TextureView,
    chroma: wgpu::TextureView,
}

/// A rotation of VA surfaces the compositor draws into.
///
/// `Arc`-shared rather than owned by the render thread because the encode job
/// runs elsewhere: a claim travels to that thread with the surface in it, and
/// the ring has to outlive every claim it handed out.
///
/// The pool the surfaces came from is **not** a field. `av_hwframe_get_buffer`
/// sets `hw_frames_ctx` on every frame it fills, so each surface holds its own
/// reference and the pool outlives this whatever the drop order — and keeping a
/// `HwFramesContext` here would make the ring `!Send` over a raw `AVBufferRef`
/// that nothing would ever read again.
pub struct SurfaceRing {
    slots: Mutex<Vec<SlotState>>,
    /// Index-addressed and immutable after construction, so the render thread
    /// reaches a slot's attachments without taking the mutex a second time.
    planes: Vec<Planes>,
    size: (u32, u32),
}

impl SurfaceRing {
    /// Allocate the rotation for exactly `size`.
    ///
    /// **Exactly**, unlike `zerocopy::PreviewRing::holds`: a VA surface has a
    /// fixed extent and the encoder is opened for one size, so a smaller frame
    /// cannot borrow a bigger surface the way it can borrow a bigger buffer.
    ///
    /// `None` on any device that cannot do this — no VAAPI, no DMA-BUF import,
    /// a driver that will not export a writable surface, or one that will not
    /// let Vulkan render into its tiling. Every one of those is a reason to use
    /// the copying path and not a reason to fail.
    pub fn new(ctx: &Arc<RenderContext>, size: (u32, u32)) -> Option<Self> {
        if !ctx.can_import_dmabuf() {
            return None;
        }
        let device = HwDeviceContext::shared_vaapi()
            .map_err(|e| tracing::debug!(error = %e, "no VAAPI device for preview surfaces"))
            .ok()?;
        let frames = HwFramesContext::create(
            device,
            ffmpeg_next::format::Pixel::VAAPI,
            ffmpeg_next::format::Pixel::NV12,
            size.0,
            size.1,
            SLOTS as i32,
        )
        .map_err(|e| tracing::debug!(error = %e, "cannot allocate a preview surface pool"))
        .ok()?;

        let mut slots = Vec::with_capacity(SLOTS);
        let mut planes = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let surface = frames
                .empty_frame()
                .map_err(|e| tracing::debug!(error = %e, "cannot take a preview surface"))
                .ok()?;
            planes.push(import(ctx, &surface)?);
            slots.push(SlotState::Idle(surface));
        }

        // `frames` is dropped here on purpose; see the note on the struct.
        drop(frames);
        Some(Self {
            slots: Mutex::new(slots),
            planes,
            size,
        })
    }

    /// Whether this ring's surfaces are the size a `size` frame needs.
    pub fn holds(&self, size: (u32, u32)) -> bool {
        self.size == size
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Take an idle surface, or `None` if every one of them is still in use.
    pub fn claim(self: &Arc<Self>) -> Option<Claim> {
        let mut slots = self.slots.lock();
        let index = slots.iter().position(|slot| match slot {
            // The reference count is the authority on whether the driver has
            // finished, exactly as in `zerocopy`. Ours is the only reference
            // when nothing else holds it, because the DRM map that exported the
            // planes was dropped at construction — see `import`.
            SlotState::Idle(surface) => !surface_is_shared(surface),
            SlotState::Busy => false,
        })?;
        let SlotState::Idle(surface) = std::mem::replace(&mut slots[index], SlotState::Busy) else {
            unreachable!("the position above matched an idle slot");
        };
        Some(Claim {
            ring: Arc::clone(self),
            index,
            surface: Some(surface),
        })
    }

    /// The luma attachment of slot `index`.
    pub fn luma(&self, index: usize) -> &wgpu::TextureView {
        &self.planes[index].luma
    }

    /// The chroma attachment of slot `index`.
    pub fn chroma(&self, index: usize) -> &wgpu::TextureView {
        &self.planes[index].chroma
    }

    fn give_back(&self, index: usize, surface: frame::Video) {
        self.slots.lock()[index] = SlotState::Idle(surface);
    }

    /// How many slots are neither claimed nor still held by a driver. For tests.
    #[cfg(test)]
    pub fn idle_slots(&self) -> usize {
        self.slots
            .lock()
            .iter()
            .filter(|slot| match slot {
                SlotState::Idle(surface) => !surface_is_shared(surface),
                SlotState::Busy => false,
            })
            .count()
    }
}

/// Export one VA surface and import its two planes as colour attachments.
///
/// The DRM map is dropped before returning, and that is load-bearing rather than
/// tidiness: it holds a reference on the surface, and `claim` reads that
/// surface's reference count to decide whether the driver has finished with it.
/// Keeping the map would make every slot look permanently busy. The file
/// descriptors survive because `import_plane_with` duplicates what it imports.
fn import(ctx: &RenderContext, surface: &frame::Video) -> Option<Planes> {
    let mapped = DmabufFrame::map_writable(surface)
        .map_err(|e| tracing::debug!(error = %e, "cannot export a preview surface as DMA-BUF"))
        .ok()?;
    let planes = mapped.planes();
    if planes.len() != 2 {
        tracing::debug!(
            planes = planes.len(),
            "a preview surface came back as something other than two NV12 planes"
        );
        return None;
    }

    let luma = import_plane_with(ctx, &planes[0], PlaneUse::Write)?;
    let chroma = import_plane_with(ctx, &planes[1], PlaneUse::Write)?;
    // The formats are what `render::nv12`'s plane shader declares its targets
    // as; a mismatch would fail at pipeline creation with a message about a
    // colour target rather than about a plane, which is much harder to read.
    if luma.format() != LUMA_FORMAT || chroma.format() != CHROMA_FORMAT {
        tracing::debug!(
            luma = ?luma.format(),
            chroma = ?chroma.format(),
            "a preview surface's planes are not the formats the NV12 shader writes"
        );
        return None;
    }

    let views = Planes {
        luma: luma.create_view(&wgpu::TextureViewDescriptor::default()),
        chroma: chroma.create_view(&wgpu::TextureViewDescriptor::default()),
    };
    // The textures own the imported memory and the views keep them alive, so
    // both may be dropped here; the map may *not* be kept, per the doc above.
    drop(mapped);
    Some(views)
}

impl std::fmt::Debug for SurfaceRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceRing")
            .field("slots", &SLOTS)
            .field("size", &self.size)
            .finish()
    }
}

/// One surface, taken out of the rotation.
///
/// Holds the surface itself rather than an index into it, so there is no way to
/// reach a surface without holding the claim that reserves it. The surface goes
/// back on drop, which makes "every path out of a claim releases it" true by
/// construction — and there are several such paths, because a frame can fail to
/// composite, be judged too late to encode, or reach a thread that finds the
/// encoder gone.
pub struct Claim {
    ring: Arc<SurfaceRing>,
    index: usize,
    /// `None` only between [`Self::release`] and the drop that follows it.
    surface: Option<frame::Video>,
}

impl Claim {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn ring(&self) -> &Arc<SurfaceRing> {
        &self.ring
    }

    /// The surface, for the encoder.
    pub fn surface_mut(&mut self) -> &mut frame::Video {
        self.surface
            .as_mut()
            .expect("a live claim holds its surface")
    }

    /// Give the surface back.
    ///
    /// Not required — the drop below does the same thing — but calling it makes
    /// the hand-back explicit at the point in the encode job where it is
    /// correct, which is after the JPEG bytes have been copied out.
    pub fn release(mut self) {
        self.give_back();
    }

    fn give_back(&mut self) {
        if let Some(surface) = self.surface.take() {
            self.ring.give_back(self.index, surface);
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.give_back();
    }
}

// ---------------------------------------------------------------------------
// Asking the encoder whether this actually works
// ---------------------------------------------------------------------------

/// Whether the hardware JPEG encoder reads a surface the compositor drew into.
///
/// Asked by trying it, once per process, for exactly the reason
/// [`super::zerocopy::encoder_can_read_linear`] exists: a capability list said
/// nothing useful about the linear path either. `vaQuerySurfaceAttributes` for
/// `VAProfileJPEGBaseline` + `VAEntrypointEncPicture` on this driver reports
/// NV12, `DRM_PRIME_2` memory and a 16384 maximum — all of which the *broken*
/// path also satisfied. Only a picture pushed through the real encoder and
/// checked row by row can answer this.
///
/// The check is a **row ramp**: the luma of row `n` is `n`. A tiled misread
/// collapses each 32-row block to its mean (row 0 comes back 15), an ignored
/// pitch shears progressively, and a range mismatch puts black at 16 — three
/// faults that a PSNR cannot tell apart and that want three different responses.
/// Chroma is left flat so a colour fault cannot be mistaken for a layout one.
pub fn encoder_reads_its_own_surface(ctx: &Arc<RenderContext>) -> bool {
    static ANSWER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| match probe(ctx) {
        Ok(()) => {
            tracing::info!(
                "the hardware JPEG encoder reads a surface the compositor drew into; \
                 preview frames will not be copied to the CPU"
            );
            true
        }
        Err(why) => {
            tracing::info!(
                reason = %why,
                "the preview cannot draw into the hardware JPEG encoder's own surfaces; \
                 frames will be read back and converted on the CPU"
            );
            false
        }
    })
}

/// The probe's picture size. 256 wide so a decoded row's mean is stable, 128
/// tall so four 32-row tiles are covered — one is not enough to tell a tiled
/// misread from an offset.
const PROBE: (u32, u32) = (256, 128);

fn probe(ctx: &Arc<RenderContext>) -> std::result::Result<(), String> {
    let worst = round_trip(ctx, PROBE, 0)?;
    if worst > 6 {
        return Err(format!(
            "a row came back {worst} luma counts away from its own number, so the encoder is \
             not reading the surface the compositor drew (a 32-row block mean is what reading \
             it as tiled looks like)"
        ));
    }
    Ok(())
}

/// Draw a row ramp into a surface, encode it, and report the worst row's error.
///
/// Shared by the probe and by the tests that hold the probe to its word, so the
/// three cannot drift apart. Built as RGB through the compositor's own shader
/// rather than memcpy'd in, because that is what a real frame is.
///
/// `offset` shifts the ramp, so a caller running this repeatedly gets a
/// *different* picture each time and a surface still holding the previous
/// frame is a failure rather than a pass.
pub(crate) fn round_trip(
    ctx: &Arc<RenderContext>,
    size: (u32, u32),
    offset: u32,
) -> std::result::Result<u32, String> {
    use crate::modules::render::nv12::{Nv12PlaneWriter, READ_FORMAT};
    use crate::modules::render::source::YuvRange;
    use crate::modules::render::texture_pool::{TextureKey, TexturePool};

    let (width, height) = size;
    let ring = Arc::new(
        SurfaceRing::new(ctx, size)
            .ok_or("this device cannot draw into a VAAPI-allocated NV12 surface")?,
    );

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let level = (y.wrapping_add(offset) % 256) as u8;
        for _ in 0..width {
            rgba.extend_from_slice(&[level, level, level, 255]);
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

    let mut claim = ring.claim().ok_or("a fresh ring has no free surface")?;
    // Full range, because a JPEG carries no range tag and every decoder reads
    // it as 0..255. Limited-range samples would put row 0 at 16 and cost the
    // probe its ability to tell a range fault from a layout one.
    Nv12PlaneWriter::new(ctx)
        .write_planes(
            ctx,
            &target,
            ring.luma(claim.index()),
            ring.chroma(claim.index()),
            YuvRange::Full,
        )
        .map_err(|e| format!("the compositor cannot draw into the surface: {e}"))?;

    let jpeg = super::encoder::encode_preview_jpeg_va_surface(claim.surface_mut(), 95)
        .ok_or("the hardware JPEG encoder would not encode the surface at all")?;
    claim.release();

    let (rgb, got_w, got_h) = super::encoder::decode_jpeg_rgb(&jpeg)
        .map_err(|e| format!("the result is not a JPEG: {e}"))?;
    if (got_w, got_h) != (width, height) {
        return Err(format!(
            "it came back {got_w}x{got_h} rather than {width}x{height}"
        ));
    }

    let (w, h) = (width as usize, height as usize);
    Ok((0..h)
        .map(|row| {
            let mean: u32 = (0..w)
                .map(|col| rgb[(row * w + col) * 3] as u32)
                .sum::<u32>()
                / w as u32;
            // The ramp repeats every 256 rows, which is what a picture taller
            // than 256 rows would otherwise fail on for no good reason.
            mean.abs_diff((row as u32).wrapping_add(offset) % 256)
        })
        .max()
        .unwrap_or(0))
}

/// Whether the preview may draw into the encoder's own surfaces.
///
/// The kill switch, in the shape `export::job::set_zero_copy` and
/// `zerocopy::set_enabled` established: a process-wide flag, so both arms of the
/// change can be measured in one process rather than against a build that no
/// longer exists. It is an *upper* bound and not a decision — a machine still
/// has to pass [`encoder_reads_its_own_surface`].
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Set once an encode from a drawn surface has failed, which switches the path
/// off for the rest of the process.
///
/// Not a counter and not per size, for the reason `zerocopy::write_off` gives:
/// a driver that refuses deterministically will refuse again, and a path that
/// silently alternates between zero-copy and a readback is the hardest kind of
/// performance bug to read off a log.
static WRITTEN_OFF: AtomicBool = AtomicBool::new(false);

pub fn write_off(why: &str) {
    if !WRITTEN_OFF.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            reason = why,
            "the preview's surface-drawing JPEG path failed; every later frame reads back"
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

    fn ring(size: (u32, u32)) -> Option<Arc<SurfaceRing>> {
        let ctx = crate::modules::render::test_context()?;
        SurfaceRing::new(&ctx, size).map(Arc::new)
    }

    #[test]
    fn a_claim_reserves_a_surface_and_a_drop_gives_it_back() {
        let Some(ring) = ring((256, 128)) else {
            eprintln!("skipping: this device cannot draw into a VAAPI surface");
            return;
        };
        assert_eq!(ring.idle_slots(), SLOTS);
        let claim = ring.claim().expect("a fresh ring has surfaces");
        assert_eq!(ring.idle_slots(), SLOTS - 1);
        drop(claim);
        assert_eq!(ring.idle_slots(), SLOTS);
    }

    #[test]
    fn no_two_live_claims_name_the_same_surface() {
        // The reason the surface lives inside the slot state rather than beside
        // it: two claims out at once must not describe the same memory, or the
        // compositor overwrites a frame the encoder is reading.
        let Some(ring) = ring((256, 128)) else {
            eprintln!("skipping: this device cannot draw into a VAAPI surface");
            return;
        };
        let claims: Vec<Claim> = (0..SLOTS).map(|_| ring.claim().expect("surface")).collect();
        let mut indices: Vec<usize> = claims.iter().map(Claim::index).collect();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), SLOTS, "a surface was handed out twice");

        assert!(ring.claim().is_none(), "a full ring must say so");
        drop(claims);
        assert!(ring.claim().is_some());
    }

    /// Every slot, twice round, with a different picture each time.
    ///
    /// One frame proves nothing about the second: a slot is reused, its Vulkan
    /// image is transitioned again, and the encoder read it in between. A frame
    /// that came back holding its predecessor's ramp — which is what a missing
    /// barrier or a slot handed out twice looks like — fails here and passes
    /// every single-frame check.
    #[test]
    fn every_slot_encodes_its_own_picture_lap_after_lap() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        if !crate::modules::preview::encoder::hardware_available()
            || !encoder_reads_its_own_surface(&ctx)
        {
            eprintln!("skipping: this machine does not take the drawn path");
            return;
        }
        // 200 is not a multiple of 64, which is the case `ROW_ALIGN` documents.
        for lap in 0..(SLOTS as u32 * 2 + 1) {
            // 37 is coprime with 256, so no two laps draw the same picture.
            let offset = lap.wrapping_mul(37);
            let worst = round_trip(&ctx, (200, 96), offset).expect("the drawn path runs");
            assert!(
                worst <= 8,
                "lap {lap} (ramp offset {offset}): a row came back {worst} counts out"
            );
        }
    }

    #[test]
    fn a_ring_is_exactly_one_size() {
        // Unlike the buffer ring, which holds anything that fits: a VA surface
        // has a fixed extent and the encoder is opened for one size.
        let Some(ring) = ring((256, 128)) else {
            eprintln!("skipping: this device cannot draw into a VAAPI surface");
            return;
        };
        assert!(ring.holds((256, 128)));
        assert!(!ring.holds((128, 128)));
        assert!(!ring.holds((256, 64)));
    }

    /// The self-check has to agree with what the encoder actually does.
    ///
    /// It cannot assert *which* answer a machine gives — that is the whole
    /// point of asking at run time — so it asserts the implication both ways: a
    /// probe that says yes and is wrong puts a mosaic in front of the user, and
    /// one that says no and is wrong costs 60% of the frame budget for nothing.
    #[test]
    fn the_self_check_agrees_with_what_the_encoder_actually_does() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        if !crate::modules::preview::encoder::hardware_available() {
            eprintln!("skipping: no hardware JPEG encoder");
            return;
        }
        let verdict = encoder_reads_its_own_surface(&ctx);
        eprintln!("this machine's JPEG encoder reads a surface the compositor drew: {verdict}");

        // A different size from the probe's, so this is an independent picture
        // rather than a rerun of the cached answer. 200 is not a multiple of 64,
        // which is the case `render::nv12::ROW_ALIGN` documents the cost of.
        match round_trip(&ctx, (200, 96), 0) {
            Ok(worst) if verdict => assert!(
                worst <= 8,
                "the probe said the encoder reads a drawn surface, but a row came back {worst} \
                 counts out — the preview would be showing a scrambled picture"
            ),
            Ok(worst) => assert!(
                worst > 8,
                "the probe said the encoder cannot read a drawn surface, but the picture came \
                 back right (worst row off by {worst}) — the preview is paying for a readback \
                 it does not need"
            ),
            Err(why) => assert!(
                !verdict,
                "the probe said yes and then the path would not run at all: {why}"
            ),
        }
    }
}
