//! Hardware video decode, on VAAPI.
//!
//! This is the mirror of `export::hwframes`, which does the same job for the
//! encoder. Read that file first: the ownership discipline here is copied from
//! it deliberately, and the reasons it gives — one owned `AVBufferRef` per Rust
//! value, a *fresh* `av_buffer_ref` for anything that leaves the module, unref
//! exactly once in `Drop` — apply unchanged.
//!
//! ## What libavcodec needs, and what it does for us
//!
//! Decoding on VAAPI is not a different decoder. It is the *same* `h264`
//! decoder with a hardware accelerator bolted underneath, selected through a
//! callback:
//!
//! ```text
//!   av_hwdevice_ctx_create(VAAPI, "/dev/dri/renderD128")      VaapiDevice
//!        │
//!        │  av_buffer_ref ──► AVCodecContext::hw_device_ctx
//!        │  AVCodecContext::get_format = select_vaapi
//!        ▼
//!   avcodec_open2
//!        │
//!        │  first packet: libavcodec calls get_format with the formats this
//!        │  stream could be decoded into. We answer AV_PIX_FMT_VAAPI, and it
//!        │  allocates an AVHWFramesContext for us — that part we do *not*
//!        │  have to build, unlike the encode direction.
//!        ▼
//!   AVFrame { format: AV_PIX_FMT_VAAPI, data[3]: VASurfaceID }
//! ```
//!
//! Two consequences worth knowing before touching this:
//!
//! - **`get_format` is called during decode, not during open.** So a codec the
//!   driver cannot actually drive opens fine and fails on the first frame.
//!   That is why [`capabilities`] exists.
//! - **libavcodec retries.** If we answer `AV_PIX_FMT_VAAPI` and initialising
//!   the accelerator fails — 10-bit HEVC on a chip that only does 8-bit, say —
//!   `ff_get_format` drops that entry and calls us again. Answering with the
//!   first remaining format lets the stream decode in software rather than
//!   dying, which is why [`select_vaapi`] falls through instead of returning
//!   `AV_PIX_FMT_NONE`. The decoder then observes a software frame and says so;
//!   nothing lies about which path ran.
//!
//! ## Why capability detection decodes something
//!
//! `docs/STATUS.md` records the trap in the encode direction: an encoder being
//! in the FFmpeg build says nothing about whether the driver can drive it.
//! Decode has the same trap with the sign flipped — this chip decodes AV1 and
//! cannot encode it — so the same answer applies. Each codec is probed by
//! feeding the decoder a real, tiny, embedded bitstream and checking that what
//! comes back is a VA surface. `vainfo` is *evidence*, not an API; this is.

use std::ffi::CString;
use std::ptr;
use std::sync::OnceLock;

use ffmpeg::format::Pixel;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;

use super::{ensure_initialized, MediaError, Result};

/// The render node opened when the caller does not name one.
///
/// `renderD128` and not `card0`, for the reason `export::hwframes` gives: the
/// render node does not need the session's DRM master, so a decode running
/// under a different seat or headless still gets a device.
pub const DEFAULT_RENDER_NODE: &str = "/dev/dri/renderD128";

/// Extra surfaces to ask libavcodec for beyond what the codec needs.
///
/// The decoder sizes its surface pool from the codec's reference-frame
/// requirement. This decoder holds two frames past that on purpose — `last`,
/// which is still the visible one, and `pending`, the frame it overshot on —
/// and a caller mapping a frame to DMA-BUF holds a third for as long as the
/// texture lives. Without the headroom those holds exhaust the pool and
/// `receive_frame` starts failing part-way through a file, which looks exactly
/// like a corrupt stream and is not one.
const EXTRA_HW_FRAMES: i32 = 6;

/// VAAPI displays *opened* in this process. One, unless somebody went around
/// `gpu`.
///
/// Opens, not live handles, and never decremented — which is the difference
/// from `render::context::LIVE_DEVICES`. A [`VaapiDevice`] is `Clone` and a
/// clone is another reference to the same display, so counting drops would make
/// an ordinary hand-out look like a close. The process's one display lives in a
/// `static` and is never closed anyway.
///
/// The counterpart of `render::context::LIVE_DEVICES`, and here for the same
/// reason: the cost of a second display is intermittent and points nowhere near
/// its cause — `vaInitialize` is tens of milliseconds, a driver has a finite
/// number of contexts, and running out of them shows up as a decode failing
/// part-way through a timeline with many clips. A counter costs nothing and
/// turns that into a line in the log naming the cause.
///
/// The `#[cfg(test)]` opens are the exception and are deliberate: a test that
/// checks what a bad device node does has to try to open one.
static LIVE_DISPLAYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn hw_error(what: impl Into<String>, code: i32) -> MediaError {
    MediaError::Hardware {
        what: what.into(),
        source: ffmpeg::Error::from(code),
    }
}

// ---------------------------------------------------------------------------
// The device
// ---------------------------------------------------------------------------

/// An open VAAPI display on a DRM render node.
///
/// Cloning takes a new libavutil reference rather than opening a second
/// display, which matters: every `vaInitialize` costs tens of milliseconds and
/// the driver has a finite number of contexts.
///
/// The process has one of these and [`crate::modules::gpu::vaapi_device`] hands
/// out the references — to this decoder, to the export encoder and to the
/// preview's hardware JPEG encoder alike. The constructor below is crate-private
/// so that stays true.
pub struct VaapiDevice {
    /// Always non-null between construction and `Drop`.
    ptr: *mut ffmpeg::ffi::AVBufferRef,
    node: String,
}

/// # Safety
///
/// The only field that is not plainly `Send`/`Sync` is the `AVBufferRef`.
/// libavutil reference-counts those **atomically** — `av_buffer_ref` and
/// `av_buffer_unref` go through `atomic_uint` — so a handle may be moved
/// between threads and two handles may be dropped from two threads without a
/// race. This is the same argument `export::hwframes` makes in prose at the top
/// of the file, and it is why that module needs no such impl: it never puts a
/// device in a `static`.
///
/// `Sync` additionally requires that `&VaapiDevice` be usable from two threads
/// at once. Every method here either reads `ptr` to hand it to libavutil, which
/// takes its own reference, or reads `node`; nothing mutates through `&self`.
///
/// What this impl does *not* claim is that the VADisplay behind the pointer may
/// be *driven* concurrently. It is shared by
/// [`crate::modules::gpu::vaapi_device`] across decoders, and those decoders are
/// serialised by `provider::MediaSourceProvider`'s mutex for the same reason
/// `VideoDecoder`'s `Send` impl needs it. The encoders on the other side of the
/// process drive their own codec contexts on the same display, which is what
/// FFmpeg's own CLI does when it transcodes on VAAPI.
unsafe impl Send for VaapiDevice {}
unsafe impl Sync for VaapiDevice {}

impl VaapiDevice {
    /// Open a VAAPI device on `node`, or on [`DEFAULT_RENDER_NODE`].
    ///
    /// Crate-private, and called from exactly one place:
    /// [`crate::modules::gpu::vaapi_device`], which opens the process's only
    /// display and hands out references to it. Everything else takes one of
    /// those.
    pub(crate) fn open(node: Option<&str>) -> Result<Self> {
        ensure_initialized();

        let node = node.unwrap_or(DEFAULT_RENDER_NODE);
        // A NUL in a device path is not a real case, but turning it into a
        // clean error beats an `unwrap` in a probe that must not panic.
        let device = CString::new(node)
            .map_err(|_| MediaError::NoHardware(format!("{node} is not a usable device path")))?;

        let mut ptr: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        // SAFETY: `av_hwdevice_ctx_create` writes a freshly created, owned
        // `AVBufferRef *` through its first argument and assigns it only on
        // success, so starting from null leaves the failure path with nothing
        // to release. `device` is a NUL-terminated buffer still in scope for
        // the whole call and libavutil copies what it keeps. The last two
        // arguments are the documented "no options, no flags" pair.
        let code = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut ptr,
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                device.as_ptr(),
                ptr::null_mut(),
                0,
            )
        };
        if code < 0 || ptr.is_null() {
            return Err(MediaError::NoHardware(format!(
                "cannot open the VAAPI device {node} ({})",
                ffmpeg::Error::from(if code < 0 { code } else { -1 })
            )));
        }

        if LIVE_DISPLAYS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            tracing::error!(
                node,
                "a second VAAPI display was opened in this process; every caller \
                 should be going through gpu::vaapi_device"
            );
        }

        Ok(Self {
            ptr,
            node: node.to_owned(),
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    /// The libavutil reference this handle owns.
    ///
    /// Crate-private and deliberately not a `pub` accessor: the only correct
    /// thing to do with it is `av_buffer_ref` it, which is what
    /// `export::hwframes::HwDeviceContext::shared_vaapi` does. Storing the
    /// pointer instead would outlive this handle.
    pub(crate) fn as_ptr(&self) -> *mut ffmpeg::ffi::AVBufferRef {
        self.ptr
    }

    /// Point a not-yet-opened codec context at this device.
    ///
    /// Must happen before `avcodec_open2`: libavcodec copies nothing here, but
    /// `get_format` is consulted on the first packet and needs the device
    /// already in place.
    pub fn attach_to_decoder(&self, context: &mut ffmpeg::codec::context::Context) -> Result<()> {
        // SAFETY: `av_buffer_ref` returns a *new* reference, so the codec
        // context owns one independent of ours and `avcodec_free_context` may
        // release it in either order — the rule `export::hwframes` states for
        // the encode direction, unchanged. The context is borrowed mutably and
        // has not been opened, so libavcodec is not reading these fields
        // concurrently; `hw_device_ctx` is null at this point because we
        // allocated the context and set it nowhere else, so the assignment
        // leaks no earlier reference.
        unsafe {
            let extra = ffmpeg::ffi::av_buffer_ref(self.ptr);
            if extra.is_null() {
                return Err(MediaError::NoHardware(
                    "out of memory attaching the VAAPI device to a decoder".into(),
                ));
            }
            let raw = context.as_mut_ptr();
            debug_assert!((*raw).hw_device_ctx.is_null());
            (*raw).hw_device_ctx = extra;
            (*raw).get_format = Some(select_vaapi);
            (*raw).extra_hw_frames = EXTRA_HW_FRAMES;
        }
        Ok(())
    }
}

impl Clone for VaapiDevice {
    fn clone(&self) -> Self {
        // SAFETY: `self.ptr` is non-null for the whole life of `self`, and
        // `av_buffer_ref` only reads it, returning an independent reference
        // that this new value owns and will unref exactly once.
        let ptr = unsafe { ffmpeg::ffi::av_buffer_ref(self.ptr) };
        assert!(!ptr.is_null(), "out of memory cloning a VAAPI device");
        Self {
            ptr,
            node: self.node.clone(),
        }
    }
}

impl Drop for VaapiDevice {
    fn drop(&mut self) {
        // SAFETY: exactly one reference, taken in `open` or `clone` and never
        // handed out — consumers take their own with `av_buffer_ref`. Frames
        // still alive hold the device through their own references, so this is
        // not a use-after-free even when a surface outlives us.
        unsafe { ffmpeg::ffi::av_buffer_unref(&mut self.ptr) };
    }
}

impl std::fmt::Debug for VaapiDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaapiDevice")
            .field("node", &self.node)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Format selection
// ---------------------------------------------------------------------------

/// Choose the hardware surface format when libavcodec offers it.
///
/// `fmt` is a `AV_PIX_FMT_NONE`-terminated list of the formats this stream
/// could be decoded into, best first. Returning something not in the list is a
/// contract violation, so the fallback is the list's own head rather than
/// anything invented here.
///
/// Falling back rather than refusing is deliberate: libavcodec calls this again
/// with `AV_PIX_FMT_VAAPI` removed when the accelerator fails to initialise,
/// and a file that will not hardware-decode must still open.
unsafe extern "C" fn select_vaapi(
    _context: *mut ffmpeg::ffi::AVCodecContext,
    fmt: *const ffmpeg::ffi::AVPixelFormat,
) -> ffmpeg::ffi::AVPixelFormat {
    use ffmpeg::ffi::AVPixelFormat;

    if fmt.is_null() {
        return AVPixelFormat::AV_PIX_FMT_NONE;
    }

    // SAFETY: libavcodec's contract for `get_format` is that `fmt` points at an
    // array of at least one entry terminated by `AV_PIX_FMT_NONE`, valid for
    // the duration of the call. The walk stops at that terminator and at a
    // hard bound, so a caller that broke the contract cannot run us off the end
    // of the allocation.
    unsafe {
        let mut i = 0isize;
        let head = *fmt;
        while i < 64 {
            let candidate = *fmt.offset(i);
            if candidate == AVPixelFormat::AV_PIX_FMT_NONE {
                break;
            }
            if candidate == AVPixelFormat::AV_PIX_FMT_VAAPI {
                return candidate;
            }
            i += 1;
        }
        head
    }
}

// ---------------------------------------------------------------------------
// Getting pixels back out
// ---------------------------------------------------------------------------

/// Copy a hardware surface into system memory.
///
/// This is the correct fallback and the boring one: it is a full-frame read
/// across the memory bus, and on an integrated GPU it is a copy of memory to
/// itself. `dmabuf::DmabufFrame` is the path that avoids it.
///
/// `software` may be an empty frame, in which case libavutil allocates and
/// picks a format; passing one that already has buffers of the right size and
/// format reuses them, which is what the decoder does so a 1080p decode does
/// not malloc three megabytes per frame.
pub fn transfer_to_software(hardware: &frame::Video, software: &mut frame::Video) -> Result<()> {
    // SAFETY: both pointers come from live `frame::Video` values borrowed for
    // the call, so neither can be freed underneath it, and `&mut` on the
    // destination rules out source and destination being the same frame.
    // `av_hwframe_transfer_data` reads the source surface and writes the
    // destination, allocating the destination's buffers if it has none — which
    // `av_frame_free` in the frame's own `Drop` then releases. The zero is the
    // documented "no flags".
    let code = unsafe {
        ffmpeg::ffi::av_hwframe_transfer_data(software.as_mut_ptr(), hardware.as_ptr(), 0)
    };
    if code < 0 {
        return Err(hw_error("cannot read a frame back from the GPU", code));
    }
    Ok(())
}

/// Whether this frame is a hardware surface rather than pixels in memory.
pub fn is_hardware_frame(frame: &frame::Video) -> bool {
    frame.format() == Pixel::VAAPI
}

// ---------------------------------------------------------------------------
// Capability detection
// ---------------------------------------------------------------------------

/// A codec we care about being able to decode on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HwCodec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

impl HwCodec {
    pub const ALL: [HwCodec; 4] = [HwCodec::H264, HwCodec::Hevc, HwCodec::Vp9, HwCodec::Av1];

    pub fn label(self) -> &'static str {
        match self {
            HwCodec::H264 => "H.264",
            HwCodec::Hevc => "HEVC",
            HwCodec::Vp9 => "VP9",
            HwCodec::Av1 => "AV1",
        }
    }

    pub fn id(self) -> ffmpeg::codec::Id {
        use ffmpeg::codec::Id;
        match self {
            HwCodec::H264 => Id::H264,
            HwCodec::Hevc => Id::HEVC,
            HwCodec::Vp9 => Id::VP9,
            HwCodec::Av1 => Id::AV1,
        }
    }

    pub fn from_id(id: ffmpeg::codec::Id) -> Option<Self> {
        use ffmpeg::codec::Id;
        match id {
            Id::H264 => Some(HwCodec::H264),
            Id::HEVC => Some(HwCodec::Hevc),
            Id::VP9 => Some(HwCodec::Vp9),
            Id::AV1 => Some(HwCodec::Av1),
            _ => None,
        }
    }

    /// One real frame of this codec, 320×240, flat grey.
    ///
    /// Embedded rather than generated: generating one needs an *encoder* in the
    /// build, and this chip is the exact counterexample — it decodes AV1 and
    /// cannot encode it, so a probe that encoded first would report AV1 decode
    /// as unavailable on the machine where it works.
    ///
    /// H.264 and HEVC are Annex-B elementary streams and VP9 and AV1 are the
    /// payload of a single IVF frame, i.e. in every case exactly what a demuxer
    /// would hand a decoder. That is what lets the probe skip the demuxer.
    fn probe_bitstream(self) -> &'static [u8] {
        match self {
            HwCodec::H264 => include_bytes!("probe_streams/h264.bin"),
            HwCodec::Hevc => include_bytes!("probe_streams/hevc.bin"),
            HwCodec::Vp9 => include_bytes!("probe_streams/vp9.bin"),
            HwCodec::Av1 => include_bytes!("probe_streams/av1.bin"),
        }
    }
}

/// What this machine will actually do with one codec.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HwDecodeSupport {
    pub codec: HwCodec,
    /// What FFmpeg calls the decoder — `h264`, not `h264_vaapi`. VAAPI decode
    /// is the ordinary decoder with an accelerator underneath, and confusing
    /// the two sends people looking for a decoder that does not exist.
    pub decoder_name: String,
    /// A decoder for this codec is in the build at all.
    pub in_build: bool,
    /// That decoder *claims* it can produce `AV_PIX_FMT_VAAPI` given a device.
    /// A claim, from a static table — the thing that means nothing on its own.
    pub declares_vaapi: bool,
    /// A real frame went in and a VA surface came out. This is the only field
    /// worth believing.
    pub usable: bool,
    /// Why not, in prose, when `usable` is false.
    pub note: Option<String>,
}

/// Which codecs this machine hardware-decodes, established by decoding.
///
/// Cached for the process: a GPU is not hot-plugged mid-session, and the probe
/// opens a decoder per codec. Wrapped in `catch_unwind` for the reason
/// `export::hwaccel` gives — enumerating optional vendor hardware is exactly the
/// code that aborts on a broken driver, and a panic here must not take the app
/// down for a feature nobody asked for.
pub fn capabilities() -> &'static [HwDecodeSupport] {
    static DETECTED: OnceLock<Vec<HwDecodeSupport>> = OnceLock::new();
    DETECTED.get_or_init(|| {
        std::panic::catch_unwind(probe_all).unwrap_or_else(|_| {
            tracing::warn!("hardware decode detection panicked; decoding in software");
            Vec::new()
        })
    })
}

/// Whether `codec` decodes on the GPU here. The question the decoder asks.
pub fn supports(codec: HwCodec) -> bool {
    capabilities()
        .iter()
        .any(|support| support.codec == codec && support.usable)
}

fn probe_all() -> Vec<HwDecodeSupport> {
    ensure_initialized();
    HwCodec::ALL.iter().map(|codec| probe(*codec)).collect()
}

fn probe(codec: HwCodec) -> HwDecodeSupport {
    let default = ffmpeg::codec::decoder::find(codec.id());
    let chosen = hardware_decoder(codec.id()).or(default);

    let mut support = HwDecodeSupport {
        codec,
        decoder_name: chosen
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| format!("{:?}", codec.id())),
        in_build: default.is_some(),
        declares_vaapi: false,
        usable: false,
        note: None,
    };

    if !support.in_build {
        support.note = Some(format!("no {} decoder in this FFmpeg build", codec.label()));
        return support;
    }

    let Some(decoder) = hardware_decoder(codec.id()) else {
        support.note = Some(format!(
            "no {} decoder in this FFmpeg build has a VAAPI configuration",
            codec.label()
        ));
        return support;
    };
    support.declares_vaapi = true;
    let _ = decoder;

    let started = std::time::Instant::now();
    match trial_decode(codec) {
        Ok(()) => {
            support.usable = true;
            tracing::debug!(
                codec = codec.label(),
                took_ms = started.elapsed().as_secs_f64() * 1000.0,
                "hardware decode probe succeeded"
            );
        }
        Err(error) => {
            tracing::debug!(codec = codec.label(), %error, "hardware decode probe failed");
            support.note = Some(format!(
                "{} is in the build but this driver could not decode a test frame with it \
                 ({error}). Software decoding still works.",
                codec.label()
            ));
        }
    }
    support
}

/// The decoder for `id` that can drive VAAPI, which is frequently **not** the
/// one `avcodec_find_decoder` returns.
///
/// This cost a session to find and it is the single most surprising thing in
/// this file. `avcodec_find_decoder(AV_CODEC_ID_AV1)` on an ordinary Ubuntu
/// build returns **`libdav1d`**, which is an excellent software decoder with no
/// hardware support at all — so attaching a VAAPI device to it does nothing,
/// `get_format` is never offered `AV_PIX_FMT_VAAPI`, and every frame decodes on
/// the CPU while the code believes it is on the GPU. The native `av1` decoder,
/// which exists in the same build purely to drive accelerators, is the one that
/// works. FFmpeg's own CLI does exactly this and says so out loud: *"Selecting
/// decoder 'av1' because of requested hwaccel method vaapi"*.
///
/// So the decoder is chosen by capability rather than by name or by default,
/// which also means a future build that moves the hwaccel elsewhere keeps
/// working without a table here to update.
pub fn hardware_decoder(id: ffmpeg::codec::Id) -> Option<ffmpeg::Codec> {
    let mut opaque: *mut std::ffi::c_void = ptr::null_mut();
    loop {
        // SAFETY: `av_codec_iterate` walks libavcodec's own static registry,
        // threading its position through `opaque`, and returns null exactly
        // once at the end — which is the loop's only exit. The pointers it
        // returns are into static storage that is never freed, so the `Codec`
        // wrapper built from one can outlive this call. `opaque` starts null as
        // the API requires.
        let raw = unsafe { ffmpeg::ffi::av_codec_iterate(&mut opaque) };
        if raw.is_null() {
            return None;
        }
        // SAFETY: `raw` is non-null and points at a live `AVCodec` in static
        // storage. `Codec::wrap` wants `*mut` and every method it exposes reads
        // through `*const`; libavcodec's own API is `const`-correct here and
        // nothing below writes.
        let codec = unsafe { ffmpeg::Codec::wrap(raw as *mut _) };
        if codec.is_decoder() && codec.id() == id && declares_vaapi(&codec) {
            return Some(codec);
        }
    }
}

/// Whether a decoder declares a VAAPI configuration driven by a device context.
///
/// Cheap and honest about what it is: a lookup in a static table that says what
/// the *codec* supports, saying nothing about the driver. It exists to skip the
/// expensive probe for codecs that cannot possibly work, not to answer the
/// question.
fn declares_vaapi(decoder: &ffmpeg::Codec) -> bool {
    let mut index = 0;
    loop {
        // SAFETY: `avcodec_get_hw_config` returns a pointer into libavcodec's
        // own static tables — never freed, valid for the process — or null once
        // `index` runs past the end, which is the loop's exit. `decoder` is a
        // live `Codec` wrapping a non-null `AVCodec *` for the duration.
        let config = unsafe { ffmpeg::ffi::avcodec_get_hw_config(decoder.as_ptr(), index) };
        if config.is_null() {
            return false;
        }
        // SAFETY: non-null by the check above, and the struct is plain data in
        // static storage.
        let (pix_fmt, methods, device_type) =
            unsafe { ((*config).pix_fmt, (*config).methods, (*config).device_type) };
        let by_device = methods & ffmpeg::ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0;
        if by_device
            && pix_fmt == ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_VAAPI
            && device_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
        {
            return true;
        }
        index += 1;
    }
}

/// Decode one embedded frame and insist that a VA surface comes back.
fn trial_decode(codec: HwCodec) -> Result<()> {
    let device = crate::modules::gpu::vaapi_device()
        .ok_or_else(|| MediaError::NoHardware("no VAAPI device on this machine".into()))?;
    let found = hardware_decoder(codec.id()).ok_or_else(|| {
        MediaError::NoHardware(format!("no {} decoder can drive VAAPI", codec.label()))
    })?;

    let mut context = ffmpeg::codec::context::Context::new();
    device.attach_to_decoder(&mut context)?;
    let mut decoder = context
        .decoder()
        .open_as(found)
        .map_err(|source| MediaError::Hardware {
            what: format!("cannot open a hardware {} decoder", codec.label()),
            source,
        })?
        .video()
        .map_err(|source| MediaError::Hardware {
            what: format!("the {} decoder is not a video decoder", codec.label()),
            source,
        })?;

    let packet = ffmpeg::Packet::copy(codec.probe_bitstream());
    decoder
        .send_packet(&packet)
        .map_err(|source| MediaError::Hardware {
            what: format!("the driver refused a {} test frame", codec.label()),
            source,
        })?;
    // A single-frame stream needs the flush: the decoder is entitled to hold
    // the only frame it has until it is told nothing more is coming.
    let _ = decoder.send_eof();

    let mut frame = frame::Video::empty();
    decoder
        .receive_frame(&mut frame)
        .map_err(|source| MediaError::Hardware {
            what: format!("the driver decoded no {} frame", codec.label()),
            source,
        })?;

    if !is_hardware_frame(&frame) {
        return Err(MediaError::NoHardware(format!(
            "the {} test frame came back as {:?} rather than a VA surface, \
             so this codec would decode in software anyway",
            codec.label(),
            frame.format()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_probe_bitstream_is_present_and_plausible() {
        // A truncated or missing blob would make the probe report the codec
        // unusable on a machine where it works, which is the worst failure
        // this file has: a silent downgrade to software.
        for codec in HwCodec::ALL {
            let bytes = codec.probe_bitstream();
            assert!(
                bytes.len() > 16,
                "{} probe bitstream is {} bytes",
                codec.label(),
                bytes.len()
            );
        }
        // H.264 and HEVC are Annex-B, so they start with a start code.
        assert_eq!(&HwCodec::H264.probe_bitstream()[..4], &[0, 0, 0, 1]);
        assert_eq!(&HwCodec::Hevc.probe_bitstream()[..4], &[0, 0, 0, 1]);
    }

    #[test]
    fn codec_ids_round_trip() {
        for codec in HwCodec::ALL {
            assert_eq!(HwCodec::from_id(codec.id()), Some(codec));
        }
        assert_eq!(HwCodec::from_id(ffmpeg::codec::Id::MPEG2VIDEO), None);
    }

    #[test]
    fn detection_never_panics_and_always_says_why() {
        // The call itself is the test on a box with no GPU. Where there is one,
        // the invariant is the same one `export::hwaccel` holds: an entry is
        // either usable or carries prose explaining what went wrong.
        for support in capabilities() {
            assert_eq!(
                support.note.is_some(),
                !support.usable,
                "{:?} said usable={} note={:?}",
                support.codec,
                support.usable,
                support.note
            );
        }
    }

    #[test]
    fn a_missing_device_node_is_an_error_and_not_a_panic() {
        assert!(VaapiDevice::open(Some("/dev/dri/renderD9999")).is_err());
    }

    #[test]
    fn a_device_path_with_a_nul_is_rejected_before_libav_sees_it() {
        assert!(matches!(
            VaapiDevice::open(Some("/dev/dri/\0renderD128")),
            Err(MediaError::NoHardware(_))
        ));
    }

    // The device is shared rather than reopened: `gpu::tests` holds that one,
    // because `gpu` is where the sharing now happens.
}
