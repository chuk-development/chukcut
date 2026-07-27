//! JPEG encoding on the GPU, through VAAPI's picture-encode entrypoint.
//!
//! The preview pays for one JPEG per frame, and on the CPU that bill is the
//! single largest item in the frame budget. This module moves it onto the
//! fixed-function JPEG encoder that has been sitting unused in the iGPU the
//! entire time:
//!
//! ```text
//!   vainfo: VAProfileJPEGBaseline : VAEntrypointEncPicture
//! ```
//!
//! FFmpeg exposes it as `mjpeg_vaapi`, so the whole thing is
//! `export::hwframes` — already written and already sound — plus a codec
//! context and a colour conversion.
//!
//! ## The path a frame takes, and what each step costs
//!
//! ```text
//!   RGBA from the compositor
//!        │  rgba_to_nv12, full-range BT.601       ~2 ms    (CPU, rayon)
//!        ▼
//!   NV12 software frame
//!        │  av_hwframe_transfer_data              ~2-5 ms  (upload)
//!        ▼
//!   VAAPI surface
//!        │  avcodec_send_frame / receive_packet   ~2.5 ms  (fixed-function)
//!        ▼
//!   one JPEG, SOI to EOI, in the packet
//! ```
//!
//! Numbers are for a 1080x1920 frame; the full table is in
//! `docs/research/vaapi-jpeg-preview.md` and the benchmark that produces it is
//! `examples/preview_jpeg_bench.rs`.
//!
//! **Only the last step is the encoder.** That is the thing worth understanding
//! before optimising further: the fixed-function encode is 2.5 ms and the two
//! steps carrying pixels to it cost more than it does. So the remaining win is
//! not a faster encoder, it is not moving the pixels — the compositor already
//! has them on the GPU, and the preview reads them back to system memory only
//! to send them straight back.
//!
//! [`VaapiJpegEncoder::encode_dmabuf`] is the version that does not move them:
//! `render::nv12`'s compute pass writes NV12 into memory exported as a DMA-BUF
//! and this maps that memory as the surface. It is worth **2.4–3.1×** on the
//! serial frame and **it does not work on the Raptor Lake iGPU**, because this
//! encoder reads an imported linear surface as though it were 32-row tiled while
//! `h264_vaapi` reads the same file descriptor correctly.
//! [`super::zerocopy::encoder_can_read_linear`] finds that out by trying, once,
//! and the whole account is in `docs/research/preview-zerocopy-jpeg.md`.
//!
//! ## Traps, all of which cost time to find
//!
//! - **Full range, not limited.** This was the expensive one. JPEG is full
//!   range by definition — there is no range tag in the file and every decoder
//!   assumes 0-255 — but every YUV conversion routine written for video
//!   defaults to *limited* range, 16-235. Feeding limited-range samples to the
//!   encoder produces grey blacks and no white, which looks like a washed-out
//!   preview and reads as a colour-management bug somewhere far away. It shows
//!   up as a PSNR of about 27 dB against the software encoder instead of 37.
//!   The coefficients below are the JFIF ones for exactly this reason.
//! - **swscale is not an option here.** `sws_scale` was the obvious way to do
//!   RGBA to NV12 and was measured at 37 ms for a 1080x1920 frame — three times
//!   what libjpeg-turbo needs to produce a whole finished JPEG. libswscale has
//!   no SIMD path for that pair and falls through to the generic per-pixel C
//!   converter. [`rgba_to_nv12`] is a plain integer transform over rayon and is
//!   roughly twenty times faster. Anyone reaching for swscale here should
//!   measure it first.
//! - **`global_quality` is a 1-100 quality, not a lambda.** Several FFmpeg
//!   encoders divide it by `FF_QP2LAMBDA`; this one does not. Verified by
//!   sweeping it and watching the file size move monotonically.
//! - **`async_depth` defaults to 2**, which means the encoder holds a frame
//!   back and `receive_packet` returns `EAGAIN` for the first one. The preview
//!   wants the frame it just asked for, not throughput, so we ask for 1.
//! - **NV12 has no odd sizes.** Chroma is subsampled by two in both
//!   directions, so an odd width or height cannot be represented. The caller
//!   falls back to software for those rather than silently cropping.
//! - **No JFIF header is written by default** (`jfif` is false), so the output
//!   would be SOI/DQT/SOF0/DHT/SOS/EOI with no APP0. That is a legal JPEG and
//!   both WebKit and libjpeg decode it; we ask for the header anyway, because a
//!   preview frame that some future consumer refuses is not worth the 18 bytes
//!   saved.

use ffmpeg::format::Pixel;
use ffmpeg::util::frame;
use ffmpeg::Dictionary;
use ffmpeg_next as ffmpeg;

use super::error::{PreviewError, Result};
use crate::modules::export::hwframes::{HwDeviceContext, HwFramesContext};

/// FFmpeg's name for the VAAPI picture-encode JPEG encoder.
pub const ENCODER_NAME: &str = "mjpeg_vaapi";

/// Surfaces in the pool.
///
/// The video encoders in `export` need twenty because reordering and reference
/// lists keep surfaces alive long past `send_frame`. JPEG has neither: exactly
/// one surface is in flight at a time, and the extras are only there so a
/// driver that holds the last picture briefly does not stall us.
const POOL_SURFACES: i32 = 4;

/// Bytes per RGBA8 pixel.
const BYTES_PER_PIXEL: usize = 4;

fn ffmpeg_error(what: &str, error: ffmpeg::Error) -> PreviewError {
    PreviewError::Encode(format!("{what}: {error}"))
}

/// Whether a size can be encoded as NV12 at all.
///
/// Chroma is half resolution in both directions, so an odd edge has no
/// representation. This is a property of the format and not of the driver, so
/// it is checked here rather than discovered as an `EINVAL` from
/// `av_hwframe_ctx_init`.
pub fn size_is_encodable(width: u32, height: u32) -> bool {
    width >= 2 && height >= 2 && width % 2 == 0 && height % 2 == 0
}

/// One opened `mjpeg_vaapi` encoder and the surface pool it draws from.
///
/// Fixed to one size and one quality: `avcodec_open2` is not repeatable and
/// `global_quality` is read during it, so changing either means building a new
/// one. [`Self::matches`] is how the caller finds out.
pub struct VaapiJpegEncoder {
    encoder: ffmpeg::encoder::video::Encoder,
    /// Owns the VAAPI device as well. Dropped after `encoder` in declaration
    /// order — which is safe either way, because the codec context holds its
    /// own reference to the pool.
    frames: HwFramesContext,
    /// Staging for [`Self::encode`], which converts and encodes in one call.
    ///
    /// The preview does **not** use this one: it converts into a scratch frame
    /// of its own before taking the encoder's lock, for the reason in
    /// [`Nv12Scratch`]. This exists so a benchmark or a test can still hand the
    /// encoder plain RGBA.
    nv12: Nv12Scratch,
    width: u32,
    height: u32,
    quality: u8,
    /// Monotonic, because an encoder with a non-increasing pts logs a warning
    /// per frame. Nothing downstream reads it.
    pts: i64,
    stages: Stages,
}

/// Where the time in one hardware encode went.
///
/// Kept because the answer is counter-intuitive and would otherwise have to be
/// rediscovered: the encode itself is a rounding error and the colour
/// conversion is most of the bill. See `docs/research/vaapi-jpeg-preview.md`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stages {
    pub convert_micros: u64,
    pub upload_micros: u64,
    pub encode_micros: u64,
}

/// Why this can live in a `static` and be used from a rayon worker.
///
/// The struct owns raw pointers — an `AVCodecContext`, an `AVFrame` and two
/// `AVBufferRef`s — and `ffmpeg-next` marks none of them `Send`. The reasoning
/// is the same as `media::decoder::VideoDecoder`'s, and it has two halves.
///
/// **Thread affinity.** There is none. A libavcodec context is not safe to
/// *use* concurrently, but nothing in it is tied to the thread that created
/// it; moving one is fine as long as use stays exclusive.
/// [`Self::encode`] takes `&mut self`, so the borrow checker guarantees that
/// wherever the encoder ends up.
///
/// **Refcounts.** The two `AVBufferRef`s inside [`HwFramesContext`] are
/// reference counted *atomically* by libavutil, so a handle may cross threads.
/// The codec context's `Rc` keep-alive — the thing that makes `VideoDecoder`
/// delicate — does not apply here: this context is built by
/// `avcodec_alloc_context3`, not from a demuxer's stream parameters, so there
/// is no shared non-atomic count and no second handle anywhere.
///
/// The single `Mutex` in `encoder.rs` is what supplies the exclusivity. Any
/// future code that reaches this encoder by another route must keep it: move
/// it whole, and serialise access.
unsafe impl Send for VaapiJpegEncoder {}

impl VaapiJpegEncoder {
    /// Open an encoder for one size and quality.
    ///
    /// Every failure here — no such encoder in the build, no device, a driver
    /// without the JPEG entrypoint — is a normal outcome on somebody's machine
    /// and comes back as an `Err` the caller turns into "use the CPU".
    pub fn open(width: u32, height: u32, quality: u8) -> Result<Self> {
        if !size_is_encodable(width, height) {
            return Err(PreviewError::FrameSize { width, height });
        }
        let quality = quality.clamp(1, 100);

        let codec = ffmpeg::encoder::find_by_name(ENCODER_NAME).ok_or_else(|| {
            PreviewError::Encode(format!("this FFmpeg build has no {ENCODER_NAME}"))
        })?;

        // `avcodec_alloc_context3(codec)` rather than `ffmpeg-next`'s
        // `Context::new()`, for the reason spelled out in
        // `export::encoder::context_for`: the generic allocation leaves the
        // encoder's private options on libavcodec's defaults instead of the
        // encoder's own, and `jfif`/`huffman`/`async_depth` all live there.
        //
        // SAFETY: `codec.as_ptr()` is a pointer to a static `AVCodec` inside
        // libavcodec, valid for the process. `avcodec_alloc_context3` either
        // returns a freshly allocated context we own or null, and
        // `Context::wrap` takes ownership of it — `None` for the second
        // argument says there is no parent object keeping it alive, which is
        // true here.
        let ptr = unsafe { ffmpeg::ffi::avcodec_alloc_context3(codec.as_ptr()) };
        if ptr.is_null() {
            return Err(PreviewError::Encode(format!(
                "cannot allocate a context for {ENCODER_NAME}"
            )));
        }
        let context = unsafe { ffmpeg::codec::context::Context::wrap(ptr, None) };

        let mut encoder = context
            .encoder()
            .video()
            .map_err(|e| ffmpeg_error("preparing the hardware JPEG encoder", e))?;

        encoder.set_width(width);
        encoder.set_height(height);
        // The format the *encoder* sees is the opaque surface, not the pixels;
        // `hw_frames_ctx` is where it looks for what they really are.
        encoder.set_format(Pixel::VAAPI);
        encoder.set_time_base(ffmpeg::Rational::new(1, 1000));
        // JPEG carries no range tag and every decoder reads it as full range.
        // Saying so here keeps anything that inspects the context honest.
        encoder.set_color_range(ffmpeg::color::Range::JPEG);

        // The process's one display rather than a second one. A preview
        // encoder is built again on every size or quality change, and an export
        // may be encoding on the same chip while it runs.
        let device =
            HwDeviceContext::shared_vaapi().map_err(|e| PreviewError::Encode(e.to_string()))?;
        let frames =
            HwFramesContext::create(device, Pixel::VAAPI, Pixel::NV12, width, height, POOL_SURFACES)
                .map_err(|e| PreviewError::Encode(e.to_string()))?;
        frames
            .attach_to_encoder(&mut encoder)
            .map_err(|e| PreviewError::Encode(e.to_string()))?;

        let mut options = Dictionary::new();
        // 1-100, applied directly. See the module header.
        let quality_text = quality.to_string();
        options.set("global_quality", &quality_text);
        // One frame in, one packet out. The default of 2 makes the first
        // `receive_packet` return EAGAIN, which for a preview is a frame that
        // never arrives rather than a frame that arrives late.
        options.set("async_depth", "1");
        options.set("jfif", "1");
        options.set("huffman", "1");

        let encoder = encoder
            .open_as_with(codec, options)
            .map_err(|e| ffmpeg_error(&format!("opening {ENCODER_NAME}"), e))?;

        Ok(Self {
            encoder,
            frames,
            nv12: Nv12Scratch::new(width, height),
            width,
            height,
            quality,
            pts: 0,
            stages: Stages::default(),
        })
    }

    /// How long the last [`Self::encode`] spent in each stage.
    pub fn stages(&self) -> Stages {
        self.stages
    }

    pub fn matches(&self, width: u32, height: u32, quality: u8) -> bool {
        self.width == width && self.height == height && self.quality == quality.clamp(1, 100)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Convert one tightly packed RGBA frame and encode it.
    ///
    /// **The preview does not call this.** It converts first, outside the
    /// encoder's mutex, and then calls [`Self::encode_nv12`] — see
    /// [`Nv12Scratch`] for why that separation is load-bearing. This is the
    /// convenient version, for benchmarks and tests that hold no lock.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        self.nv12.fill(rgba)?;
        let convert_micros = started.elapsed().as_micros() as u64;

        // Two disjoint field borrows rather than `self.encode_nv12(...)`, which
        // would borrow all of `self` mutably and the staging frame immutably at
        // the same time.
        let out = Self::encode_surface(
            &mut self.encoder,
            &self.frames,
            &mut self.pts,
            &mut self.stages,
            self.nv12.frame(),
        );
        self.stages.convert_micros = convert_micros;
        out
    }

    /// Encode a frame the caller has already converted to NV12.
    ///
    /// `nv12` must be the size this encoder was opened for; an encoder is fixed
    /// to one size, so a mismatch is a caller bug rather than something to
    /// scale.
    pub fn encode_nv12(&mut self, nv12: &Nv12Scratch) -> Result<Vec<u8>> {
        if !nv12.matches(self.width, self.height) {
            let (width, height) = nv12.size();
            return Err(PreviewError::FrameSize { width, height });
        }
        self.stages.convert_micros = 0;
        Self::encode_surface(
            &mut self.encoder,
            &self.frames,
            &mut self.pts,
            &mut self.stages,
            nv12.frame(),
        )
    }

    /// Encode a frame the compositor wrote straight into exported GPU memory.
    ///
    /// This is the zero-copy entry point and the whole point of the exercise.
    /// `buffer` describes a DMA-BUF that `render::dmabuf::ExportableBuffer`
    /// allocated and `render::nv12`'s compute pass filled: the pixels are
    /// already where the encoder can read them, so there is no conversion, no
    /// readback and no upload — the three stages that were 60–76% of a preview
    /// frame (`docs/research/preview-performance.md`).
    ///
    /// Two obligations on the caller, both of which produce silent corruption
    /// rather than an error if they are not met:
    ///
    /// - **The GPU must have finished.** libva cannot be handed a Vulkan
    ///   semaphore, so the only synchronisation is a CPU-side wait after the
    ///   compute submit. `Compositor::render_nv12_into_range` does it.
    /// - **The returned surface must be held** until the caller is willing to
    ///   let the compute pass write that buffer again. It is a *wrap* of the
    ///   compositor's memory, not a copy, and overwriting memory the encoder is
    ///   still reading gives a frame torn between two compositions.
    ///
    /// The samples must be **full range**, which is what `YuvRange::Full` on the
    /// compute pass is for; see the module header.
    pub fn encode_dmabuf(
        &mut self,
        buffer: &crate::modules::export::hwframes::Nv12Dmabuf<'_>,
    ) -> Result<(Vec<u8>, frame::Video)> {
        if (buffer.width, buffer.height) != (self.width, self.height) {
            return Err(PreviewError::FrameSize {
                width: buffer.width,
                height: buffer.height,
            });
        }

        let started = std::time::Instant::now();
        let mut surface = self
            .frames
            .import_nv12_dmabuf(buffer)
            .map_err(|e| PreviewError::Encode(e.to_string()))?;
        self.stages.convert_micros = 0;
        // Filed under `upload` because it is the same slot in the frame budget
        // — how the pixels reached the encoder — and because that is what makes
        // the two paths comparable in one table. It is a mapping, not a copy.
        self.stages.upload_micros = started.elapsed().as_micros() as u64;

        let bytes = Self::encode_mapped(
            &mut self.encoder,
            &mut self.pts,
            &mut self.stages,
            &mut surface,
        )?;
        Ok((bytes, surface))
    }

    /// Encode a VA surface the caller allocated and the GPU has already filled.
    ///
    /// The *other* zero-copy entry point, and the one that works on this chip.
    /// [`Self::encode_dmabuf`] goes the direction the export goes — we allocate
    /// the memory, describe it as linear, and the driver imports it — and
    /// Intel's JPEG engine reads such a surface as though it were 32-row tiled
    /// (`docs/research/preview-zerocopy-jpeg.md`). This goes the other way: the
    /// media driver allocated the surface, so its tiling is by construction the
    /// one the encoder expects, and Vulkan imported *that* and drew into it.
    ///
    /// Two obligations on the caller, both of which produce silent corruption
    /// rather than an error if they are not met, and both identical to
    /// [`Self::encode_dmabuf`]'s:
    ///
    /// - **The GPU must have finished.** libva cannot be handed a Vulkan
    ///   semaphore, so the only synchronisation is a CPU-side wait after the
    ///   render submit. `Nv12PlaneWriter::write_planes` does it.
    /// - **The surface must not be redrawn** until the encoder has finished
    ///   reading it. `preview::vasurface` is what tracks that.
    ///
    /// The samples must be **full range**, as for every other JPEG path here.
    pub fn encode_va_surface(&mut self, surface: &mut frame::Video) -> Result<Vec<u8>> {
        if (surface.width(), surface.height()) != (self.width, self.height) {
            return Err(PreviewError::FrameSize {
                width: surface.width(),
                height: surface.height(),
            });
        }
        // Nothing was converted and nothing was uploaded: the pixels were
        // written where they already needed to be. Both stages are zero so the
        // breakdown in `Stages` stays comparable across the three paths.
        self.stages.convert_micros = 0;
        self.stages.upload_micros = 0;
        Self::encode_mapped(&mut self.encoder, &mut self.pts, &mut self.stages, surface)
    }

    /// An empty VA surface from this encoder's own pool.
    ///
    /// The surfaces the compositor draws into come from here rather than from a
    /// pool of their own so that there is no question whether the encoder will
    /// accept them: it is the pool that is attached to the codec context.
    pub fn empty_surface(&self) -> Result<frame::Video> {
        self.frames
            .empty_frame()
            .map_err(|e| PreviewError::Encode(e.to_string()))
    }

    /// Upload one NV12 frame to a surface and run the fixed-function encoder.
    ///
    /// Free-standing over the fields it needs so both entry points above can
    /// call it without borrowing all of `self`.
    fn encode_surface(
        encoder: &mut ffmpeg::encoder::video::Encoder,
        frames: &HwFramesContext,
        pts: &mut i64,
        stages: &mut Stages,
        nv12: &frame::Video,
    ) -> Result<Vec<u8>> {
        let started = std::time::Instant::now();
        let mut surface = frames
            .empty_frame()
            .map_err(|e| PreviewError::Encode(e.to_string()))?;
        frames
            .upload(nv12, &mut surface)
            .map_err(|e| PreviewError::Encode(e.to_string()))?;
        stages.upload_micros = started.elapsed().as_micros() as u64;
        Self::encode_mapped(encoder, pts, stages, &mut surface)
    }

    /// Send one VA surface to the fixed-function encoder and take the JPEG back.
    ///
    /// The half both entry points share: whether the surface was uploaded into
    /// or mapped over, from here on it is the same picture-encode submission.
    fn encode_mapped(
        encoder: &mut ffmpeg::encoder::video::Encoder,
        pts: &mut i64,
        stages: &mut Stages,
        surface: &mut frame::Video,
    ) -> Result<Vec<u8>> {
        surface.set_pts(Some(*pts));
        *pts += 1;

        let started = std::time::Instant::now();
        encoder
            .send_frame(&*surface)
            .map_err(|e| ffmpeg_error("sending a frame to the hardware JPEG encoder", e))?;

        let mut packet = ffmpeg::Packet::empty();
        encoder.receive_packet(&mut packet).map_err(|e| {
            // EAGAIN here means the encoder is holding the frame back, which
            // `async_depth=1` is supposed to prevent. If a driver ever does it
            // anyway the caller falls back to software, so say which it was.
            ffmpeg_error("the hardware JPEG encoder produced no picture", e)
        })?;

        stages.encode_micros = started.elapsed().as_micros() as u64;

        let data = packet
            .data()
            .ok_or_else(|| PreviewError::Encode("the hardware JPEG packet was empty".into()))?;
        if !super::encoder::is_jpeg(data) {
            return Err(PreviewError::Encode(
                "the hardware JPEG encoder produced something that is not a JPEG".into(),
            ));
        }
        Ok(data.to_vec())
    }
}

/// One reusable NV12 frame, and the conversion into it.
///
/// **This exists so that the conversion happens outside the encoder's mutex**,
/// and that is not a tidiness argument. [`rgba_to_nv12`] dispatches onto the
/// rayon pool. Doing so while holding a lock that rayon workers also want is a
/// deadlock with two distinct routes into it:
///
/// - From a rayon worker, the thread holding the lock joins the work-stealing
///   loop while it waits and runs another job that asks for the same lock.
/// - From any other thread, the dispatch waits for a free worker — and if the
///   pool is full of jobs blocked on that same lock, nobody can finish.
///
/// The second one is not hypothetical: it hung the unit-test binary at default
/// parallelism, with the preview's encode thread holding the lock and waiting
/// for a worker while twelve workers waited for the lock. Converting first and
/// locking second removes the class rather than one route into it. `encoder.rs`
/// keeps one of these per encoding thread.
///
/// Not `Send`: an `AVFrame` may be moved between threads safely, but there is
/// no reason to and `ffmpeg-next` does not say so, so this stays where it was
/// made.
pub struct Nv12Scratch {
    frame: frame::Video,
    width: u32,
    height: u32,
}

impl Nv12Scratch {
    /// Allocate for one size. Reused across frames, so a 3 MB allocation does
    /// not happen thirty times a second.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            frame: frame::Video::new(Pixel::NV12, width, height),
            width,
            height,
        }
    }

    pub fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn frame(&self) -> &frame::Video {
        &self.frame
    }

    /// RGBA straight from the caller's slice into the staging frame.
    ///
    /// Not swscale. `sws_scale` was the obvious thing and was measured at 37 ms
    /// for a 1080x1920 frame — on its own, more than the whole frame budget and
    /// three times what libjpeg-turbo needs to produce a finished JPEG. RGBA to
    /// NV12 has no SIMD path in libswscale; it goes through the generic
    /// per-pixel C converter. [`rgba_to_nv12`] is a plain integer transform
    /// spread over rayon and is roughly twenty times faster.
    pub fn fill(&mut self, rgba: &[u8]) -> Result<()> {
        let (width, height) = (self.width as usize, self.height as usize);
        let want = width * height * BYTES_PER_PIXEL;
        if rgba.len() < want {
            return Err(PreviewError::FrameData {
                got: rgba.len(),
                want,
            });
        }

        // SAFETY: `self.frame` was allocated by `frame::Video::new(NV12, w, h)`
        // and is never reallocated, so plane 0 is at least `linesize[0] * h`
        // bytes and plane 1 at least `linesize[1] * h/2` — libavutil's
        // guarantee for a buffer it allocated. `&mut self` means no other
        // reference to the frame exists for the duration, so building two
        // disjoint slices over its two planes cannot alias: NV12's planes are
        // separate entries in `data[]` and libavutil never overlaps them.
        let (y, y_stride, uv, uv_stride) = unsafe {
            let frame = self.frame.as_mut_ptr();
            let y_stride = (*frame).linesize[0] as usize;
            let uv_stride = (*frame).linesize[1] as usize;
            (
                std::slice::from_raw_parts_mut((*frame).data[0], y_stride * height),
                y_stride,
                std::slice::from_raw_parts_mut((*frame).data[1], uv_stride * height.div_ceil(2)),
                uv_stride,
            )
        };

        rgba_to_nv12(rgba, width, height, y, y_stride, uv, uv_stride);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RGBA to NV12
// ---------------------------------------------------------------------------

// Full-range BT.601, in 8-bit fixed point. These are the JFIF coefficients —
// the ones libjpeg uses — so the hardware path and the software path agree
// about what a colour is. Using the *limited*-range constants here is the trap
// described at the top of this file.
const Y_R: i32 = 77; //  0.299 * 256
const Y_G: i32 = 150; //  0.587 * 256
const Y_B: i32 = 29; //  0.114 * 256
const U_R: i32 = -43; // -0.168736 * 256
const U_G: i32 = -85; // -0.331264 * 256
const U_B: i32 = 128; //  0.5 * 256
const V_R: i32 = 128; //  0.5 * 256
const V_G: i32 = -107; // -0.418688 * 256
const V_B: i32 = -21; // -0.081312 * 256

/// Convert tightly packed RGBA into NV12 planes.
///
/// `width` and `height` must be even — NV12 has no representation for an odd
/// edge, which [`size_is_encodable`] is the caller's way of finding out.
///
/// Chroma is averaged over each 2x2 block **in RGB, before the transform**
/// rather than after. The transform is linear, so the two are the same value up
/// to rounding, and doing it first is a quarter of the multiplies.
///
/// ## The one rule for calling this
///
/// **No lock may be held across it**, and in particular not the one in
/// `encoder.rs` around the hardware encoder. This dispatches onto the rayon
/// pool, and a rayon dispatch under a lock that rayon workers also want
/// deadlocks — from a worker, because a thread blocked in a parallel iterator
/// steals other jobs; and from any thread, because the dispatch waits for a
/// worker the saturated pool will never free. Both were observed. That is what
/// [`Nv12Scratch`] is for, and the full account is on
/// `preview::server::encode_queue`.
pub fn rgba_to_nv12(
    rgba: &[u8],
    width: usize,
    height: usize,
    y: &mut [u8],
    y_stride: usize,
    uv: &mut [u8],
    uv_stride: usize,
) {
    use rayon::prelude::*;

    let src_stride = width * BYTES_PER_PIXEL;
    debug_assert!(rgba.len() >= src_stride * height);
    debug_assert!(width % 2 == 0 && height % 2 == 0);

    // One row pair: the unit of work either way. A row *pair* rather than a row
    // because one line of chroma covers two lines of luma, so splitting
    // anywhere else would make neighbouring tasks share a chroma row.
    let pair = move |src: &[u8], y_rows: &mut [u8], uv_row: &mut [u8]| {
        let (top, bottom) = src.split_at(src_stride);
        let (y_top, y_bottom) = y_rows.split_at_mut(y_stride);

        for x in 0..width {
            let i = x * BYTES_PER_PIXEL;
            let (r0, g0, b0) = (top[i] as i32, top[i + 1] as i32, top[i + 2] as i32);
            let (r1, g1, b1) = (bottom[i] as i32, bottom[i + 1] as i32, bottom[i + 2] as i32);
            y_top[x] = ((Y_R * r0 + Y_G * g0 + Y_B * b0 + 128) >> 8) as u8;
            y_bottom[x] = ((Y_R * r1 + Y_G * g1 + Y_B * b1 + 128) >> 8) as u8;
        }

        for x in (0..width).step_by(2) {
            let i = x * BYTES_PER_PIXEL;
            let j = i + BYTES_PER_PIXEL;
            // Sum of the 2x2 block, so the shift below carries the divide
            // by four along with the fixed-point one.
            let r = top[i] as i32 + top[j] as i32 + bottom[i] as i32 + bottom[j] as i32;
            let g =
                top[i + 1] as i32 + top[j + 1] as i32 + bottom[i + 1] as i32 + bottom[j + 1] as i32;
            let b =
                top[i + 2] as i32 + top[j + 2] as i32 + bottom[i + 2] as i32 + bottom[j + 2] as i32;

            let u = ((U_R * r + U_G * g + U_B * b + 512) >> 10) + 128;
            let v = ((V_R * r + V_G * g + V_B * b + 512) >> 10) + 128;
            uv_row[x] = u.clamp(0, 255) as u8;
            uv_row[x + 1] = v.clamp(0, 255) as u8;
        }
    };

    let src = &rgba[..src_stride * height];
    let y = &mut y[..y_stride * height];
    let uv = &mut uv[..uv_stride * (height / 2)];

    src.par_chunks(src_stride * 2)
        .zip(y.par_chunks_mut(y_stride * 2))
        .zip(uv.par_chunks_mut(uv_stride))
        .for_each(|((src, y_rows), uv_row)| pair(src, y_rows, uv_row));
}

impl std::fmt::Debug for VaapiJpegEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaapiJpegEncoder")
            .field("size", &(self.width, self.height))
            .field("quality", &self.quality)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether this machine can encode JPEG on the GPU.
    ///
    /// Tests that need it skip rather than fail: CI has no `/dev/dri`, and a
    /// red build there would say nothing about this code.
    fn available() -> bool {
        VaapiJpegEncoder::open(64, 64, 80).is_ok()
    }

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[
                    (x % 256) as u8,
                    (y % 256) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]);
            }
        }
        data
    }

    /// Convert into tightly packed planes, which is what the tests want to
    /// reason about. The real path writes into an `AVFrame`'s padded ones.
    fn convert(rgba: &[u8], width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        let mut y = vec![0u8; width * height];
        let mut uv = vec![0u8; width * height / 2];
        rgba_to_nv12(rgba, width, height, &mut y, width, &mut uv, width);
        (y, uv)
    }

    fn solid(width: usize, height: usize, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        v
    }

    /// The test that would have caught the limited-range bug, and the reason
    /// it is first: black must be 0 and white must be 255. In limited range
    /// they are 16 and 235, the picture is visibly washed out, and every other
    /// test here still passes.
    #[test]
    fn black_and_white_land_on_the_ends_of_the_full_range() {
        let (y, uv) = convert(&solid(8, 8, [0, 0, 0]), 8, 8);
        assert!(y.iter().all(|&v| v == 0), "black luma is {}", y[0]);
        assert!(
            uv.iter().all(|&v| v == 128),
            "black should be colourless, got {}",
            uv[0]
        );

        let (y, uv) = convert(&solid(8, 8, [255, 255, 255]), 8, 8);
        assert!(y.iter().all(|&v| v == 255), "white luma is {}", y[0]);
        assert!(uv.iter().all(|&v| v == 128), "white chroma is {}", uv[0]);
    }

    #[test]
    fn primaries_land_where_bt601_says_they_should() {
        // Straight from the JFIF matrix. Two counts of slack for the fixed
        // point; the point is the sign and the magnitude, not the last bit.
        for (rgb, want_y, want_u, want_v) in [
            ([255u8, 0, 0], 76u8, 85u8, 255u8),
            ([0, 255, 0], 150, 44, 21),
            ([0, 0, 255], 29, 255, 107),
        ] {
            let (y, uv) = convert(&solid(4, 4, rgb), 4, 4);
            assert!(
                y[0].abs_diff(want_y) <= 2,
                "{rgb:?} luma {} wanted {want_y}",
                y[0]
            );
            assert!(
                uv[0].abs_diff(want_u) <= 2,
                "{rgb:?} Cb {} wanted {want_u}",
                uv[0]
            );
            assert!(
                uv[1].abs_diff(want_v) <= 2,
                "{rgb:?} Cr {} wanted {want_v}",
                uv[1]
            );
        }
    }

    #[test]
    fn chroma_is_the_average_of_its_two_by_two_block() {
        // One red pixel and three black ones in the top-left block. Averaging
        // in RGB before the transform and averaging the transformed values
        // afterwards are the same number, and this pins which block feeds
        // which chroma sample — the check that catches a half-row offset.
        let (w, h) = (4usize, 2usize);
        let mut rgba = solid(w, h, [0, 0, 0]);
        rgba[0] = 255;
        let (_, uv) = convert(&rgba, w, h);

        let quarter_red = convert(&solid(2, 2, [64, 0, 0]), 2, 2).1;
        assert!(
            uv[0].abs_diff(quarter_red[0]) <= 2 && uv[1].abs_diff(quarter_red[1]) <= 2,
            "block 0 chroma is ({}, {}), expected about ({}, {})",
            uv[0],
            uv[1],
            quarter_red[0],
            quarter_red[1]
        );
        // The block next door saw no red at all.
        assert_eq!((uv[2], uv[3]), (128, 128), "red bled into the next block");
    }

    #[test]
    fn luma_is_per_pixel_and_in_the_right_place() {
        // A single white pixel at (1, 1) of an otherwise black frame. A
        // transposed or stride-confused converter puts it somewhere else and
        // every average-based test still passes.
        let (w, h) = (4usize, 4usize);
        let mut rgba = solid(w, h, [0, 0, 0]);
        let at = (1 * w + 1) * 4;
        rgba[at..at + 3].copy_from_slice(&[255, 255, 255]);
        let (y, _) = convert(&rgba, w, h);
        assert_eq!(y[1 * w + 1], 255);
        assert_eq!(y.iter().filter(|&&v| v != 0).count(), 1);
    }

    #[test]
    fn a_padded_destination_stride_is_respected() {
        // The real destination is an `AVFrame`, whose rows are padded to an
        // alignment. Writing `width` bytes per row into a `stride`-byte row is
        // the classic way to produce a picture that shears.
        let (w, h) = (4usize, 4usize);
        let (stride, uv_stride) = (16usize, 16usize);
        let mut y = vec![7u8; stride * h];
        let mut uv = vec![7u8; uv_stride * h / 2];
        rgba_to_nv12(&solid(w, h, [255, 255, 255]), w, h, &mut y, stride, &mut uv, uv_stride);

        for row in 0..h {
            assert!(
                y[row * stride..row * stride + w].iter().all(|&v| v == 255),
                "row {row} was not written"
            );
            assert!(
                y[row * stride + w..(row + 1) * stride].iter().all(|&v| v == 7),
                "row {row} wrote past the picture into the padding"
            );
        }
    }

    #[test]
    fn odd_sizes_are_refused_before_a_device_is_opened() {
        // No device needed: NV12 cannot represent an odd edge on any driver.
        assert!(!size_is_encodable(101, 37));
        assert!(!size_is_encodable(100, 37));
        assert!(!size_is_encodable(0, 0));
        assert!(size_is_encodable(1080, 1920));
        assert!(matches!(
            VaapiJpegEncoder::open(101, 38, 80),
            Err(PreviewError::FrameSize { .. })
        ));
    }

    #[test]
    fn a_frame_comes_back_as_a_jpeg() {
        if !available() {
            return;
        }
        let mut encoder = VaapiJpegEncoder::open(320, 240, 88).expect("open");
        let out = encoder.encode(&gradient(320, 240)).expect("encode");
        assert!(super::super::encoder::is_jpeg(&out), "no SOI marker");
        assert!(out.ends_with(&[0xFF, 0xD9]), "no EOI marker");
        assert!(out.len() > 512, "{} bytes is not a picture", out.len());
    }

    #[test]
    fn the_same_encoder_serves_many_frames() {
        // A leaked surface per frame shows up here as EAGAIN from the pool,
        // which is the failure mode `POOL_SURFACES` is guessing at.
        if !available() {
            return;
        }
        let mut encoder = VaapiJpegEncoder::open(320, 240, 88).expect("open");
        let frame = gradient(320, 240);
        for _ in 0..40 {
            encoder.encode(&frame).expect("encode");
        }
    }

    #[test]
    fn a_short_buffer_is_an_error_rather_than_a_read_past_the_end() {
        if !available() {
            return;
        }
        let mut encoder = VaapiJpegEncoder::open(320, 240, 88).expect("open");
        let err = encoder.encode(&[0u8; 64]).unwrap_err();
        assert!(matches!(err, PreviewError::FrameData { .. }), "{err}");
    }

    #[test]
    fn higher_quality_produces_more_bytes() {
        if !available() {
            return;
        }
        let frame = gradient(320, 240);
        let low = VaapiJpegEncoder::open(320, 240, 40)
            .unwrap()
            .encode(&frame)
            .unwrap();
        let high = VaapiJpegEncoder::open(320, 240, 95)
            .unwrap()
            .encode(&frame)
            .unwrap();
        assert!(
            low.len() < high.len(),
            "quality 40 gave {} bytes, quality 95 gave {}",
            low.len(),
            high.len()
        );
    }

    #[test]
    fn matches_is_exact_about_size_and_quality() {
        if !available() {
            return;
        }
        let encoder = VaapiJpegEncoder::open(320, 240, 88).expect("open");
        assert!(encoder.matches(320, 240, 88));
        assert!(!encoder.matches(320, 240, 94));
        assert!(!encoder.matches(640, 240, 88));
    }
}
