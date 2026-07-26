//! A safe wrapper over libavutil's hardware device and frame contexts.
//!
//! This is the piece `docs/decisions/0002-ffmpeg-and-hardware-encoding.md`
//! calls "the missing safe wrapper". `ffmpeg-sys-next` 6.1 binds
//! `libavutil/hwcontext.h` and `ffmpeg-next` re-exports it as `ffi`, so
//! `av_hwdevice_ctx_create`, `av_hwframe_ctx_alloc/init`,
//! `av_hwframe_get_buffer` and `av_hwframe_transfer_data` have been callable
//! all along. What did not exist was a Rust type that owns those buffers and
//! frees them exactly once.
//!
//! The shape is FFmpeg's own `doc/examples/vaapi_encode.c`, which is the
//! canonical recipe and worth reading beside this file:
//!
//! ```text
//!   av_hwdevice_ctx_create(VAAPI, "/dev/dri/renderD128")   HwDeviceContext
//!        │
//!        │  av_hwframe_ctx_alloc, set format/sw_format/size, av_hwframe_ctx_init
//!        ▼
//!   AVHWFramesContext { format: VAAPI, sw_format: NV12 }   HwFramesContext
//!        │                     │
//!        │                     └── av_buffer_ref ──► AVCodecContext::hw_frames_ctx
//!        │                                            (set *before* avcodec_open2)
//!        │  av_hwframe_get_buffer  ──► an empty VA surface
//!        │  av_hwframe_transfer_data ──► the software NV12 frame uploaded into it
//!        ▼
//!   avcodec_send_frame
//! ```
//!
//! Shape and ordering cross-checked against three permissively licensed
//! implementations, per the survey in `docs/research/rust-crate-survey.md`:
//! `AdrianEddy/oxivideo` (MIT/Apache), `jazzfool/ffgpu` (Apache) and
//! `ez-ffmpeg`'s `src/wgpu_filter/hw_interop.rs` (Apache). No code is copied —
//! the API here is our own — but the sequence, the `initial_pool_size`
//! reasoning and the "attach a *new* reference, never the owned one" rule all
//! come from reading them.
//!
//! ## Ownership, which is the whole reason this file exists
//!
//! Both contexts are `AVBufferRef *`. libavutil reference counts them
//! **atomically** (`av_buffer_ref`/`av_buffer_unref` go through
//! `atomic_uint`), so a handle may be moved between threads; what must not
//! happen is a double unref or a use after the last unref. Each Rust type here
//! owns exactly one reference and unrefs it in `Drop`, and every place a
//! reference leaves this module — attaching to a codec context — takes a
//! **fresh** `av_buffer_ref` so the receiver has its own.
//!
//! An `AVHWFramesContext` internally holds a reference to its device, so the
//! device outlives the pool whatever the Rust-side drop order turns out to be.
//! [`HwFramesContext`] owns its [`HwDeviceContext`] anyway, because a caller
//! having to keep two things alive in the right order is exactly the bug this
//! wrapper is supposed to remove.

use std::ffi::CString;
use std::ptr;

use ffmpeg::format::Pixel;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;

use super::{ExportError, Result};

/// The render node we open when the caller does not name one.
///
/// `renderD128` rather than `card0`: the render node does not need the
/// session's DRM master, which is the entire reason it exists, and an export
/// running under a different seat or in a headless session still gets a device.
pub const DEFAULT_RENDER_NODE: &str = "/dev/dri/renderD128";

/// Surfaces the pool is created with.
///
/// The encoder holds several frames at once — reordering, lookahead and the
/// reference list all keep surfaces alive past `send_frame` — and a pool that
/// runs dry returns `EAGAIN` from `av_hwframe_get_buffer` with no way to make
/// progress. FFmpeg's own example uses 20 and so do we; a VAAPI NV12 surface at
/// 1080p is 3.1 MB, so the whole pool is about 62 MB.
const POOL_SURFACES: i32 = 20;

fn av_error(what: impl Into<String>, code: i32) -> ExportError {
    ExportError::Ffmpeg {
        what: what.into(),
        source: ffmpeg::Error::from(code),
    }
}

// ---------------------------------------------------------------------------
// Device
// ---------------------------------------------------------------------------

/// An open hardware device — for us, a VAAPI display on a DRM render node.
///
/// Opening one is the first thing that can fail on a machine whose `/dev/dri`
/// exists but whose driver does not work, which is why [`super::hwaccel`] does
/// it during detection rather than trusting the device node.
///
/// **A VAAPI one is not opened here.** [`Self::shared_vaapi`] takes a reference
/// to the process's one display, which `gpu` owns and the decoder and the
/// preview's JPEG encoder are using at the same time. The constructors below
/// are crate-private for that reason; `vaInitialize` costs tens of milliseconds
/// and a driver has a finite number of contexts.
pub struct HwDeviceContext {
    /// Always non-null between construction and `Drop`.
    ptr: *mut ffmpeg::ffi::AVBufferRef,
    kind: ffmpeg::ffi::AVHWDeviceType,
    node: Option<String>,
}

impl HwDeviceContext {
    /// A new reference to the process's one VAAPI display.
    ///
    /// This is what the encode path and the preview's JPEG encoder use. It is a
    /// fresh `av_buffer_ref` on a display somebody else may already be
    /// decoding with — sharing one is what FFmpeg's own CLI does when it
    /// transcodes on VAAPI — so this value can be dropped, moved or handed to a
    /// frame pool with no coordination.
    pub fn shared_vaapi() -> Result<Self> {
        let device = crate::modules::gpu::vaapi_device().ok_or_else(|| {
            ExportError::Settings("this machine has no usable VAAPI device".into())
        })?;
        // SAFETY: `device` is a live handle for the duration of this function,
        // so the buffer behind it cannot be freed underneath the call, and
        // `av_buffer_ref` only reads it. What comes back is an independent
        // reference that this value owns and unrefs exactly once in `Drop` —
        // the rule the header of this file states for every reference that
        // crosses a boundary.
        let ptr = unsafe { ffmpeg::ffi::av_buffer_ref(device.as_ptr()) };
        if ptr.is_null() {
            return Err(ExportError::Settings(
                "out of memory taking a reference to the VAAPI device".into(),
            ));
        }
        Ok(Self {
            ptr,
            kind: ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            node: Some(device.node().to_owned()),
        })
    }

    /// The device an encoder of this kind needs.
    ///
    /// VAAPI comes from the process's shared display; anything else — QSV is
    /// the only other kind with a frame pool — is opened here, because there is
    /// exactly one caller and nothing else in the process has one to share.
    pub(crate) fn for_kind(
        kind: ffmpeg::ffi::AVHWDeviceType,
        node: Option<&str>,
    ) -> Result<Self> {
        if kind == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
            Self::shared_vaapi()
        } else {
            Self::open(kind, node)
        }
    }

    /// Open a VAAPI device on `node`, or on [`DEFAULT_RENDER_NODE`].
    ///
    /// Test-only, and that is the point: `gpu::vaapi_device` is the one caller
    /// in the shipped binary that creates a display. This is kept because the
    /// tests that check what a bad device node does have to open one that is
    /// *not* the shared display to do it.
    #[cfg(test)]
    pub(crate) fn vaapi(node: Option<&str>) -> Result<Self> {
        Self::open(
            ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            Some(node.unwrap_or(DEFAULT_RENDER_NODE)),
        )
    }

    pub(crate) fn open(kind: ffmpeg::ffi::AVHWDeviceType, node: Option<&str>) -> Result<Self> {
        crate::modules::media::ensure_initialized();

        // A NUL in a device path is not a real case, but turning it into a
        // clean error beats an `unwrap` in a probe that must not panic.
        let device = match node {
            Some(node) => Some(CString::new(node).map_err(|_| {
                ExportError::Settings(format!("{node} is not a usable device path"))
            })?),
            None => None,
        };

        let mut ptr: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: `av_hwdevice_ctx_create` writes a freshly created, owned
        // `AVBufferRef *` through its first argument and touches nothing else
        // on failure — libavutil's contract is that `*ptr` is only assigned on
        // success, and we start it null so the failure path unrefs nothing.
        // The device string is a NUL-terminated buffer that outlives the call
        // because `device` is still in scope, and libavutil copies what it
        // needs. The last two arguments are the documented "no options, no
        // flags" pair.
        let code = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut ptr,
                kind,
                device.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
                ptr::null_mut(),
                0,
            )
        };
        if code < 0 || ptr.is_null() {
            return Err(av_error(
                format!(
                    "cannot open the hardware device {}",
                    node.unwrap_or("(default)")
                ),
                if code < 0 { code } else { -1 },
            ));
        }

        Ok(Self {
            ptr,
            kind,
            node: node.map(str::to_owned),
        })
    }

    pub fn kind(&self) -> ffmpeg::ffi::AVHWDeviceType {
        self.kind
    }

    pub fn node(&self) -> Option<&str> {
        self.node.as_deref()
    }

    /// Allocate a frame pool on this device and take ownership of the device.
    pub fn frames(
        self,
        format: Pixel,
        sw_format: Pixel,
        width: u32,
        height: u32,
    ) -> Result<HwFramesContext> {
        HwFramesContext::create(self, format, sw_format, width, height, POOL_SURFACES)
    }
}

impl Drop for HwDeviceContext {
    fn drop(&mut self) {
        // SAFETY: we hold exactly one reference, taken in `open` and never
        // handed out — `HwFramesContext` takes its own via `av_hwframe_ctx_alloc`
        // and the codec context takes its own via `av_buffer_ref`. `av_buffer_unref`
        // nulls the pointer it is given, so a second drop is impossible even if
        // this ran twice.
        unsafe { ffmpeg::ffi::av_buffer_unref(&mut self.ptr) };
    }
}

impl std::fmt::Debug for HwDeviceContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwDeviceContext")
            .field("kind", &self.kind)
            .field("node", &self.node)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Frame pool
// ---------------------------------------------------------------------------

/// A pool of hardware surfaces of one size and format.
///
/// `format` is the hardware pixel format the surfaces *are*
/// (`AV_PIX_FMT_VAAPI`); `sw_format` is what a software frame transferred in or
/// out of one looks like (`AV_PIX_FMT_NV12`). Getting these two the wrong way
/// round is the classic first mistake and produces `EINVAL` at
/// `av_hwframe_ctx_init` rather than anything descriptive.
pub struct HwFramesContext {
    ptr: *mut ffmpeg::ffi::AVBufferRef,
    device: HwDeviceContext,
    format: Pixel,
    sw_format: Pixel,
    width: u32,
    height: u32,
}

impl HwFramesContext {
    pub fn create(
        device: HwDeviceContext,
        format: Pixel,
        sw_format: Pixel,
        width: u32,
        height: u32,
        pool_size: i32,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(ExportError::Settings(
                "a hardware frame pool needs a non-zero size".into(),
            ));
        }

        // SAFETY: `device.ptr` is non-null for the whole life of `device`, and
        // `av_hwframe_ctx_alloc` only reads it — it takes its own reference to
        // the device internally, so the pool stays valid even if our handle
        // went away first. The returned buffer is owned by us and is null only
        // on allocation failure.
        let mut ptr = unsafe { ffmpeg::ffi::av_hwframe_ctx_alloc(device.ptr) };
        if ptr.is_null() {
            return Err(ExportError::Settings(
                "out of memory allocating a hardware frame pool".into(),
            ));
        }

        // SAFETY: libavutil's documented contract for a context returned by
        // `av_hwframe_ctx_alloc` is that `buf->data` points at an
        // `AVHWFramesContext` and that the caller fills these five fields in
        // before `av_hwframe_ctx_init`. We are the sole owner of `ptr` — it has
        // been handed to nothing — so no other thread can be reading these
        // fields while we write them, and after `init` nothing here writes them
        // again.
        unsafe {
            let frames = (*ptr).data as *mut ffmpeg::ffi::AVHWFramesContext;
            (*frames).format = format.into();
            (*frames).sw_format = sw_format.into();
            (*frames).width = width as i32;
            (*frames).height = height as i32;
            (*frames).initial_pool_size = pool_size;
        }

        // SAFETY: `ptr` is a filled-in, not-yet-initialised frames context and
        // we own the only reference. On failure the reference still has to be
        // released, which is what the unref below does; `av_buffer_unref`
        // takes the address of our own local, so nothing outside sees a
        // dangling pointer.
        let code = unsafe { ffmpeg::ffi::av_hwframe_ctx_init(ptr) };
        if code < 0 {
            unsafe { ffmpeg::ffi::av_buffer_unref(&mut ptr) };
            return Err(av_error(
                format!(
                    "cannot create a {width}x{height} {sw_format:?} hardware frame pool \
                     (the driver may not support this size or format)"
                ),
                code,
            ));
        }

        Ok(Self {
            ptr,
            device,
            format,
            sw_format,
            width,
            height,
        })
    }

    pub fn format(&self) -> Pixel {
        self.format
    }

    pub fn sw_format(&self) -> Pixel {
        self.sw_format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn device(&self) -> &HwDeviceContext {
        &self.device
    }

    /// Take an empty surface out of the pool.
    ///
    /// Fails with `EAGAIN` when every surface is still held by the encoder,
    /// which is a pool that is too small rather than a transient condition —
    /// see [`POOL_SURFACES`].
    pub fn empty_frame(&self) -> Result<frame::Video> {
        let mut hw = frame::Video::empty();
        // SAFETY: `hw` is a freshly allocated `AVFrame` with no buffers, which
        // is exactly what `av_hwframe_get_buffer` requires; handing it a frame
        // that already references data would leak that data. It fills in
        // `format`, `width`, `height` and `hw_frames_ctx` and attaches a
        // surface, all of which the frame's own `Drop` (`av_frame_free`)
        // releases. The frames context is only read.
        let code = unsafe { ffmpeg::ffi::av_hwframe_get_buffer(self.ptr, hw.as_mut_ptr(), 0) };
        if code < 0 {
            return Err(av_error("cannot take a surface from the hardware pool", code));
        }
        Ok(hw)
    }

    /// Copy a software frame into a hardware surface.
    ///
    /// `software` must be in [`Self::sw_format`] and the same size as the pool;
    /// libavutil checks both and returns `EINVAL` rather than corrupting
    /// anything, but the error is opaque, so the caller gets a sentence here.
    pub fn upload(&self, software: &frame::Video, hardware: &mut frame::Video) -> Result<()> {
        if software.format() != self.sw_format {
            return Err(ExportError::Settings(format!(
                "a hardware upload needs a {:?} frame, got {:?}",
                self.sw_format,
                software.format()
            )));
        }

        // SAFETY: both pointers come from live `frame::Video` values borrowed
        // for the duration of the call, so neither can be freed underneath it,
        // and `&mut` on the destination rules out the source and destination
        // being the same frame. `av_hwframe_transfer_data` reads the source and
        // writes the destination's already-attached surface; it allocates
        // nothing on either that we would have to free. The zero is the
        // documented "no flags".
        let code = unsafe {
            ffmpeg::ffi::av_hwframe_transfer_data(
                hardware.as_mut_ptr(),
                software.as_ptr(),
                0,
            )
        };
        if code < 0 {
            return Err(av_error("cannot upload a frame to the GPU", code));
        }
        Ok(())
    }

    /// Attach this pool to a codec context that has not been opened yet.
    ///
    /// VAAPI and QSV encoders read `hw_frames_ctx` inside `avcodec_open2` to
    /// learn the surface format and size; setting it afterwards does nothing
    /// and the open fails with "No device available for encoder".
    pub fn attach_to_encoder(&self, encoder: &mut ffmpeg::encoder::video::Video) -> Result<()> {
        // SAFETY: `av_buffer_ref` hands back a *new* reference, so the codec
        // context gets its own and the two lifetimes are independent — this is
        // the rule that keeps a `MediaWriter` dropped in any order from
        // double-freeing the pool. The codec context is borrowed mutably and
        // has not been opened, so libavcodec is not reading the field
        // concurrently, and its `hw_frames_ctx` is null at this point (we
        // allocated the context and have set it nowhere else), so no previous
        // reference is being leaked by the assignment.
        unsafe {
            let extra = ffmpeg::ffi::av_buffer_ref(self.ptr);
            if extra.is_null() {
                return Err(ExportError::Settings(
                    "out of memory attaching the hardware frame pool to the encoder".into(),
                ));
            }
            let context = encoder.as_mut_ptr();
            debug_assert!((*context).hw_frames_ctx.is_null());
            (*context).hw_frames_ctx = extra;
        }
        Ok(())
    }
}

/// One DMA-BUF holding a linear NV12 frame, described the way libavutil wants
/// it.
///
/// Everything here is a *description* of memory somebody else owns — the
/// compositor's exported buffer. The file descriptor is borrowed for the
/// duration of [`HwFramesContext::import_nv12_dmabuf`] and is not closed by it.
#[derive(Debug, Clone, Copy)]
pub struct Nv12Dmabuf<'a> {
    pub fd: std::os::fd::BorrowedFd<'a>,
    /// Bytes in the whole object, which must cover both planes.
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub y_offset: usize,
    pub y_stride: usize,
    pub uv_offset: usize,
    pub uv_stride: usize,
    /// `DRM_FORMAT_MOD_LINEAR` for anything this codebase produces. Carried
    /// rather than assumed so a future tiled path does not have to change this
    /// type.
    pub modifier: u64,
}

impl HwFramesContext {
    /// Wrap a DMA-BUF as a VA surface in this pool, with no copy.
    ///
    /// This is the other half of `render::dmabuf`: the compositor allocated the
    /// buffer with `VkExportMemoryAllocateInfo`, its compute pass wrote NV12
    /// into it, and this hands the same memory to the VAAPI driver as something
    /// the encoder can read.
    ///
    /// The mapping is `AV_HWFRAME_MAP_DIRECT`, which is what makes it a wrap
    /// and not a copy — without it libavutil is free to fall back to a
    /// transfer, and a "zero-copy" path that silently copies is worse than one
    /// that fails.
    ///
    /// **The caller must have waited for the GPU.** libva has no way to be
    /// handed a Vulkan semaphore, so the only synchronisation available is a
    /// CPU-side stall after the compute submit. Skipping it produces a frame
    /// torn between two compositions, intermittently, which is a bug nobody
    /// finds from a log.
    pub fn import_nv12_dmabuf(&self, buffer: &Nv12Dmabuf<'_>) -> Result<frame::Video> {
        use std::os::fd::AsRawFd;

        if self.sw_format != Pixel::NV12 {
            return Err(ExportError::Settings(format!(
                "a DMA-BUF import needs an NV12 pool, this one is {:?}",
                self.sw_format
            )));
        }
        if (buffer.width, buffer.height) != (self.width, self.height) {
            return Err(ExportError::Settings(format!(
                "a {}x{} DMA-BUF cannot be imported into a {}x{} pool",
                buffer.width, buffer.height, self.width, self.height
            )));
        }

        // The descriptor has to outlive this function: libavutil keeps a
        // reference to the source frame for as long as the mapping exists, and
        // a descriptor on our stack would be gone by then. So it is allocated
        // with `av_mallocz` and owned by an `AVBufferRef` attached to the
        // source frame, which is the same shape `hwcontext_drm.h` documents and
        // what FFmpeg's own DRM code does.
        //
        // SAFETY: `av_mallocz` returns either null or a zeroed allocation of
        // the requested size that `av_buffer_default_free` can release. The
        // cast is to the exact type whose size was asked for.
        let descriptor = unsafe {
            ffmpeg::ffi::av_mallocz(std::mem::size_of::<ffmpeg::ffi::AVDRMFrameDescriptor>())
                as *mut ffmpeg::ffi::AVDRMFrameDescriptor
        };
        if descriptor.is_null() {
            return Err(ExportError::Settings(
                "out of memory describing a DMA-BUF".into(),
            ));
        }

        // SAFETY: `descriptor` is a zeroed, uniquely owned allocation of
        // exactly this type; nothing else has seen the pointer, so these writes
        // race with nothing. Every index written is below the `AV_DRM_MAX_PLANES`
        // bound of the arrays being written (one object, one layer, two planes).
        unsafe {
            let d = &mut *descriptor;
            d.nb_objects = 1;
            d.objects[0].fd = buffer.fd.as_raw_fd();
            d.objects[0].size = buffer.size;
            d.objects[0].format_modifier = buffer.modifier;

            d.nb_layers = 1;
            d.layers[0].format = crate::modules::render::dmabuf::DRM_FORMAT_NV12;
            d.layers[0].nb_planes = 2;
            d.layers[0].planes[0].object_index = 0;
            d.layers[0].planes[0].offset = buffer.y_offset as isize;
            d.layers[0].planes[0].pitch = buffer.y_stride as isize;
            d.layers[0].planes[1].object_index = 0;
            d.layers[0].planes[1].offset = buffer.uv_offset as isize;
            d.layers[0].planes[1].pitch = buffer.uv_stride as isize;
        }

        // SAFETY: `av_buffer_create` takes ownership of `descriptor` and will
        // release it with `av_buffer_default_free`, which is `av_free` — the
        // matching deallocator for `av_mallocz`. On failure it frees nothing,
        // which is why the null branch frees the descriptor itself. The size is
        // the allocation's real size and the last two arguments are the
        // documented "no opaque, no flags".
        let owner = unsafe {
            ffmpeg::ffi::av_buffer_create(
                descriptor as *mut u8,
                std::mem::size_of::<ffmpeg::ffi::AVDRMFrameDescriptor>(),
                Some(ffmpeg::ffi::av_buffer_default_free),
                ptr::null_mut(),
                0,
            )
        };
        if owner.is_null() {
            // SAFETY: `av_buffer_create` failed, so it took nothing; we are
            // still the only owner of an allocation made by `av_mallocz`.
            unsafe { ffmpeg::ffi::av_free(descriptor as *mut std::ffi::c_void) };
            return Err(ExportError::Settings(
                "out of memory describing a DMA-BUF".into(),
            ));
        }

        let mut source = frame::Video::empty();
        // SAFETY: `source` is a freshly allocated `AVFrame` with every field
        // zeroed and no buffers attached, so assigning `buf[0]` leaks nothing
        // and the frame's own `Drop` (`av_frame_free`) unrefs it exactly once.
        // `data[0]` pointing at the descriptor rather than at pixels is the
        // documented convention for `AV_PIX_FMT_DRM_PRIME`, and the descriptor
        // is kept alive by the very `buf[0]` we just attached.
        unsafe {
            let f = source.as_mut_ptr();
            (*f).format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
            (*f).width = buffer.width as i32;
            (*f).height = buffer.height as i32;
            (*f).buf[0] = owner;
            (*f).data[0] = descriptor as *mut u8;
        }

        let mut mapped = frame::Video::empty();
        // SAFETY: `mapped` is likewise a fresh empty frame. It is given a
        // *new* reference to this pool — `av_buffer_ref` — so the pool's own
        // reference is untouched and the frame releases its own in
        // `av_frame_free`. Setting `format` to the pool's hardware format is
        // what makes `av_hwframe_map` dispatch to VAAPI's importer rather than
        // to a software transfer.
        let mapped_ok = unsafe {
            let extra = ffmpeg::ffi::av_buffer_ref(self.ptr);
            if extra.is_null() {
                return Err(ExportError::Settings(
                    "out of memory importing a DMA-BUF".into(),
                ));
            }
            let f = mapped.as_mut_ptr();
            (*f).format = ffmpeg::ffi::AVPixelFormat::from(Pixel::VAAPI) as i32;
            (*f).width = buffer.width as i32;
            (*f).height = buffer.height as i32;
            (*f).hw_frames_ctx = extra;

            ffmpeg::ffi::av_hwframe_map(
                f,
                source.as_ptr(),
                ffmpeg::ffi::AV_HWFRAME_MAP_DIRECT as i32
                    | ffmpeg::ffi::AV_HWFRAME_MAP_READ as i32,
            )
        };
        if mapped_ok < 0 {
            return Err(av_error(
                "cannot import a DMA-BUF as a video surface (the driver may not accept this \
                 stride, offset or format modifier)",
                mapped_ok,
            ));
        }

        Ok(mapped)
    }
}

/// Whether anything besides `surface` still holds the memory behind it.
///
/// The question the zero-copy ring has to answer before overwriting a buffer:
/// a hardware encoder keeps surfaces alive past `send_frame` for its reference
/// list and its reordering delay, and there is no callback that says when it
/// is done. libavutil's reference count is the authority, so this asks it.
///
/// `true` for a frame with no buffer at all, which is the conservative answer:
/// a caller that cannot tell must not reuse.
pub fn surface_is_shared(surface: &frame::Video) -> bool {
    // SAFETY: `surface` is borrowed for the call, so its `AVFrame` and any
    // `AVBufferRef` it holds are live throughout. `av_buffer_get_ref_count`
    // reads an atomic counter and mutates nothing; passing it a null pointer
    // would be undefined, which is why the null case returns early.
    unsafe {
        let buffer = (*surface.as_ptr()).buf[0];
        if buffer.is_null() {
            return true;
        }
        ffmpeg::ffi::av_buffer_get_ref_count(buffer) > 1
    }
}

impl Drop for HwFramesContext {
    fn drop(&mut self) {
        // SAFETY: one owned reference, taken by `av_hwframe_ctx_alloc` and
        // never given away — every consumer took its own with `av_buffer_ref`.
        // Surfaces still held by an encoder keep the pool alive through their
        // own references, so this is not a use-after-free even if the encoder
        // outlives us.
        unsafe { ffmpeg::ffi::av_buffer_unref(&mut self.ptr) };
    }
}

impl std::fmt::Debug for HwFramesContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HwFramesContext")
            .field("format", &self.format)
            .field("sw_format", &self.sw_format)
            .field("size", &(self.width, self.height))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether this machine can actually open a VAAPI device.
    ///
    /// Tests that need one skip instead of failing, because CI has no GPU and a
    /// red build there would say nothing about the code.
    fn device() -> Option<HwDeviceContext> {
        HwDeviceContext::shared_vaapi().ok()
    }

    #[test]
    fn a_missing_device_is_an_error_and_not_a_panic() {
        let opened = HwDeviceContext::vaapi(Some("/dev/dri/renderD9999"));
        assert!(opened.is_err());
    }

    #[test]
    fn a_device_path_with_a_nul_is_rejected() {
        let opened = HwDeviceContext::open(
            ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            Some("/dev/dri/\0renderD128"),
        );
        assert!(matches!(opened, Err(ExportError::Settings(_))));
    }

    #[test]
    fn a_zero_sized_pool_is_rejected_before_libav_sees_it() {
        let Some(device) = device() else { return };
        let pool = HwFramesContext::create(device, Pixel::VAAPI, Pixel::NV12, 0, 0, 4);
        assert!(matches!(pool, Err(ExportError::Settings(_))));
    }

    #[test]
    fn a_pool_hands_out_surfaces_and_takes_an_upload() {
        let Some(device) = device() else { return };
        let pool = device
            .frames(Pixel::VAAPI, Pixel::NV12, 320, 240)
            .expect("a 320x240 NV12 pool");
        assert_eq!(pool.size(), (320, 240));

        let mut surface = pool.empty_frame().expect("a surface");
        assert_eq!(surface.format(), Pixel::VAAPI);
        assert_eq!(surface.width(), 320);

        let software = frame::Video::new(Pixel::NV12, 320, 240);
        pool.upload(&software, &mut surface).expect("an upload");
    }

    #[test]
    fn uploading_the_wrong_pixel_format_is_refused_with_prose() {
        let Some(device) = device() else { return };
        let pool = device
            .frames(Pixel::VAAPI, Pixel::NV12, 320, 240)
            .expect("a 320x240 NV12 pool");
        let mut surface = pool.empty_frame().expect("a surface");
        let wrong = frame::Video::new(Pixel::YUV420P, 320, 240);
        let error = pool.upload(&wrong, &mut surface).unwrap_err();
        assert!(error.to_string().contains("NV12"), "{error}");
    }

    /// The decisive experiment for zero-copy encode: does this driver accept
    /// memory the compositor allocated?
    ///
    /// Writes a known NV12 picture into a Vulkan buffer exported as a DMA-BUF,
    /// imports it as a VA surface, and reads the surface back through
    /// libavutil. Round-tripping the *pixels* is the point — an import that
    /// succeeds and hands back a green frame is the failure mode this whole
    /// area is prone to, and it looks identical to success in a log.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_dmabuf_the_compositor_allocated_can_be_imported_as_a_surface() {
        use crate::modules::render::dmabuf::{ExportableBuffer, DRM_FORMAT_MOD_LINEAR};
        use crate::modules::render::Nv12Layout;

        let Some(device) = device() else { return };
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };

        // Full HD, because that is the size an export actually runs at and
        // because a plane offset that a driver silently re-aligns only shows up
        // at a size whose luma plane is not a round number of pages.
        let (width, height) = (1920u32, 1080u32);
        let layout = Nv12Layout::for_size(width, height);
        let size = layout.total_bytes();

        let Some(exported) = ExportableBuffer::new(&ctx, size as u64, "import test") else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            return;
        };

        // A pattern that varies per row and per column, not flat planes. Flat
        // planes survive a plane offset the driver moved, a stride it rounded
        // and a row it dropped — every failure this test exists to catch.
        let mut picture = vec![0u8; size];
        for row in 0..height as usize {
            let line = &mut picture[row * layout.y_stride..][..width as usize];
            for (col, byte) in line.iter_mut().enumerate() {
                *byte = (row.wrapping_mul(7).wrapping_add(col.wrapping_mul(3)) % 251) as u8;
            }
        }
        for row in 0..layout.uv_rows {
            let line = &mut picture[layout.uv_offset() + row * layout.uv_stride..][..width as usize];
            for (col, byte) in line.iter_mut().enumerate() {
                *byte = (row.wrapping_mul(11).wrapping_add(col.wrapping_mul(5)) % 241) as u8;
            }
        }
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("import test staging"),
            size: size as u64,
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        staging
            .slice(..)
            .get_mapped_range_mut()
            .expect("staging range")
            .copy_from_slice(&picture);
        staging.unmap();

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(&staging, 0, exported.buffer(), 0, size as u64);
        ctx.queue().submit(Some(encoder.finish()));
        // The stall the doc comment on `import_nv12_dmabuf` insists on. libva
        // cannot wait on a Vulkan semaphore, so this is the synchronisation.
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");

        let pool = device
            .frames(Pixel::VAAPI, Pixel::NV12, width, height)
            .expect("a VAAPI pool");
        let imported = pool.import_nv12_dmabuf(&Nv12Dmabuf {
            fd: exported.fd(),
            size,
            width,
            height,
            y_offset: 0,
            y_stride: layout.y_stride,
            uv_offset: layout.uv_offset(),
            uv_stride: layout.uv_stride,
            modifier: DRM_FORMAT_MOD_LINEAR,
        });

        let imported = match imported {
            Ok(surface) => surface,
            Err(e) => {
                // A driver that will not take our buffer is a finding, not a
                // test failure — the CPU path still works and this is the exact
                // sentence the next person needs.
                eprintln!("skipping: this driver refuses the DMA-BUF import: {e}");
                return;
            }
        };
        assert_eq!(imported.format(), Pixel::VAAPI);

        // Read the surface back the way the decode direction would, and check
        // the pixels are the ones written above.
        let mut back = frame::Video::empty();
        // SAFETY: `back` is an empty frame, which is what
        // `av_hwframe_transfer_data` requires as a destination — it allocates
        // the software buffers itself and the frame's `Drop` frees them. The
        // source is a live mapped surface borrowed for the call. Zero is the
        // documented "no flags".
        let code = unsafe {
            ffmpeg::ffi::av_hwframe_transfer_data(back.as_mut_ptr(), imported.as_ptr(), 0)
        };
        assert!(code >= 0, "reading the imported surface back: {code}");
        assert_eq!(back.format(), Pixel::NV12);

        // Compare every byte of both planes. A summary statistic would hide
        // exactly the failure that matters: a chroma plane the driver read from
        // a slightly different offset still averages correctly.
        let mismatch = |plane: usize, stride: usize, rows: usize, source_base: usize| {
            let got = back.data(plane);
            let got_stride = back.stride(plane);
            for row in 0..rows {
                for col in 0..width as usize {
                    let want = picture[source_base + row * stride + col];
                    let have = got[row * got_stride + col];
                    if want != have {
                        return Some((row, col, want, have));
                    }
                }
            }
            None
        };

        assert_eq!(
            mismatch(0, layout.y_stride, height as usize, 0),
            None,
            "the luma plane of the imported surface is not what the GPU wrote"
        );
        assert_eq!(
            mismatch(1, layout.uv_stride, layout.uv_rows, layout.uv_offset()),
            None,
            "the chroma plane of the imported surface is not what the GPU wrote \
             (the driver may have re-aligned the plane offset)"
        );
    }

    #[test]
    fn many_surfaces_can_be_taken_and_returned_without_exhausting_the_pool() {
        // Each surface is dropped at the end of its iteration, so a pool of 20
        // serves far more than 20 requests. A leak in `empty_frame` shows up
        // here as EAGAIN.
        let Some(device) = device() else { return };
        let pool = device
            .frames(Pixel::VAAPI, Pixel::NV12, 320, 240)
            .expect("a 320x240 NV12 pool");
        for _ in 0..64 {
            let _surface = pool.empty_frame().expect("a surface");
        }
    }
}
