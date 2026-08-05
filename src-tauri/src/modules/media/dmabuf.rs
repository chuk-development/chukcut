//! Handing a decoded frame to the GPU without it ever touching the CPU.
//!
//! A VA surface already lives in GPU memory. `av_hwframe_transfer_data` copies
//! it out to system memory so swscale can turn it into RGBA, and then the
//! renderer copies it straight back in — on an integrated GPU, memory copied to
//! itself, twice, per frame. This module is the way out of that: VAAPI can hand
//! back the surface's underlying DRM buffers as file descriptors, and Vulkan can
//! import a DRM buffer as an image.
//!
//! ```text
//!   AVFrame (AV_PIX_FMT_VAAPI)          the decoder's surface
//!        │
//!        │  av_hwframe_map(READ|DIRECT) ──► vaSyncSurface
//!        │                              ──► vaExportSurfaceHandle
//!        ▼
//!   AVFrame (AV_PIX_FMT_DRM_PRIME), data[0] = AVDRMFrameDescriptor
//!        │
//!        │  one fd per object, one DRM fourcc + modifier per layer
//!        ▼
//!   wgpu_hal::vulkan::Device::texture_from_dmabuf_fd
//! ```
//!
//! ## The shape of what comes back, which is the awkward part
//!
//! FFmpeg exports with `VA_EXPORT_SURFACE_SEPARATE_LAYERS`, so an NV12 surface
//! arrives as **two layers over one object**: layer 0 is `R8` at offset 0, layer
//! 1 is `GR88` further into the same buffer. `wgpu-hal`'s importer takes one fd,
//! one modifier, one offset and one stride — it is single-plane by
//! construction — so NV12 means importing the same buffer twice as two
//! textures and doing the YUV→RGB conversion in a shader.
//!
//! `docs/research/hardware-decode.md` measures both routes and recommends the
//! other one: put the VAAPI VPP engine in front, convert NV12 to a packed
//! BGRA surface on fixed-function hardware, and export *that* — one layer, one
//! plane, one texture, no shader change anywhere. See [`super::vpp`]. This
//! module deliberately describes whatever it is given rather than assuming
//! either, because the same code exports both.
//!
//! ## Ownership of the file descriptors
//!
//! The descriptors belong to the mapped frame: libavutil closes them when the
//! frame is freed. `texture_from_dmabuf_fd` **takes ownership** of the fd it is
//! given. Those two facts are incompatible, which is why every fd leaves here
//! through [`Plane::dup_fd`] — a duplicate the caller owns, over the same
//! buffer object. Handing out the original would give Vulkan and libavutil the
//! same fd to close.

use std::os::fd::{BorrowedFd, OwnedFd};

use ffmpeg::format::Pixel;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;

use super::{MediaError, Result};

/// `DRM_FORMAT_MOD_INVALID`, which is what libavutil writes when the driver did
/// not tell it a modifier. Importing with it is not allowed; a frame carrying
/// it has to go the copy route.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// `DRM_FORMAT_MOD_LINEAR`. Every importer accepts it, which is why the
/// research note suggests reaching for it when a tiled modifier is refused.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// One plane of one layer, flattened into what an importer actually needs.
///
/// The `AVDRMFrameDescriptor` splits this across three nested arrays — objects,
/// layers, planes — and every consumer immediately joins them back together, so
/// the join happens once, here.
#[derive(Debug, Clone)]
pub struct Plane {
    /// `DRM_FORMAT_*` fourcc of the *layer* this plane belongs to: `R8` and
    /// `GR88` for the two halves of NV12, `AR24`/`AB24` for a packed BGRA.
    pub fourcc: u32,
    /// `DRM_FORMAT_MOD_*` of the object backing this plane. Tiling. An importer
    /// that does not know this modifier must refuse rather than guess.
    pub modifier: u64,
    /// Byte offset of this plane inside the object.
    pub offset: u64,
    /// Bytes per row.
    pub pitch: u64,
    /// Size of the whole object, not of this plane.
    pub object_size: usize,
    /// Pixel size of this plane, with chroma subsampling already applied — so
    /// the UV plane of a 1920×1080 NV12 surface reports 960×540 and not
    /// 1920×1080. An importer sizes its image from this.
    pub width: u32,
    pub height: u32,
    /// The descriptor's own fd, borrowed. Duplicate it with [`Self::dup_fd`]
    /// before handing it to anything that closes what it is given.
    fd: i32,
}

impl Plane {
    /// A duplicate of the buffer's file descriptor, owned by the caller.
    ///
    /// `F_DUPFD_CLOEXEC`, via `BorrowedFd::try_clone_to_owned`: an fd that
    /// survives an `exec` into a child process is a leak nobody notices until a
    /// long-running export is holding a hundred of them.
    pub fn dup_fd(&self) -> Result<OwnedFd> {
        // SAFETY: `self.fd` comes from the `AVDRMFrameDescriptor` owned by the
        // `DmabufFrame` this plane was read out of, and `Plane` is only ever
        // produced by [`DmabufFrame::planes`], which borrows that frame for the
        // call. The frame closes the fd in its own `Drop`, so the descriptor is
        // open for as long as any `Plane` describing it can be reached.
        let borrowed = unsafe { BorrowedFd::borrow_raw(self.fd) };
        borrowed.try_clone_to_owned().map_err(|source| {
            MediaError::Invalid(format!("cannot duplicate a DMA-BUF descriptor: {source}"))
        })
    }

    /// The fourcc as the four characters it actually is — `"NV12"`, `"AR24"`.
    /// Every DRM error message in the world quotes these, so the log line
    /// should too.
    pub fn fourcc_name(&self) -> String {
        fourcc_name(self.fourcc)
    }

    /// Whether this plane can be imported at all.
    ///
    /// An unknown modifier is the one hard stop: Vulkan's
    /// `VK_EXT_image_drm_format_modifier` needs an explicit modifier and there
    /// is no "figure it out" value.
    pub fn is_importable(&self) -> bool {
        self.modifier != DRM_FORMAT_MOD_INVALID && self.pitch > 0 && self.width > 0
    }
}

/// `DRM_FORMAT_*` as its four characters.
pub fn fourcc_name(fourcc: u32) -> String {
    let bytes = fourcc.to_le_bytes();
    bytes
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

/// A decoded frame exported as DRM buffers.
///
/// Owns the mapped `AVFrame`. That frame internally holds a reference to the
/// VA surface it was mapped from, so the surface cannot be recycled by the
/// decoder while this value is alive — which is exactly the guarantee a texture
/// built on these descriptors needs, and the reason a `DmabufFrame` should be
/// kept for as long as its texture is.
pub struct DmabufFrame {
    mapped: frame::Video,
    width: u32,
    height: u32,
}

/// # Safety
///
/// The `AVFrame` behind this value is refcounted atomically by libavutil and
/// carries no thread affinity, exactly as `export::hwframes`' buffers do.
/// `DmabufFrame` hands out no interior pointers except through `&self` borrows,
/// and the only mutation is the `av_frame_free` in `Drop`, which `&mut self`
/// makes exclusive.
///
/// The reason this impl is needed at all is that the frame must be able to
/// travel to whichever thread owns the wgpu device, which is not the thread that
/// decoded it.
unsafe impl Send for DmabufFrame {}

impl DmabufFrame {
    /// Export the VA surface behind `hardware` as DRM buffers.
    ///
    /// `AV_HWFRAME_MAP_READ` is not optional and is not only about permissions:
    /// it is what makes libavutil call `vaSyncSurface` first, which is the only
    /// synchronisation in this path. Without it the fds would describe a
    /// surface the decoder may still be writing.
    pub fn map(hardware: &frame::Video) -> Result<Self> {
        Self::map_with(hardware, ffmpeg::ffi::AV_HWFRAME_MAP_READ as i32)
    }

    /// Export a surface that is going to be **written** as well as read.
    ///
    /// The preview's encoder surfaces take this route: VAAPI allocates them,
    /// Vulkan imports the exported planes as colour attachments, and the
    /// compositor draws NV12 straight into them. `VA_EXPORT_SURFACE_READ_WRITE`
    /// rather than `READ_ONLY` is the whole difference, and a driver that
    /// hands back a read-only buffer object would make the Vulkan import fail
    /// rather than corrupt anything.
    ///
    /// `AV_HWFRAME_MAP_READ` stays set alongside the write, because it is what
    /// makes libavutil call `vaSyncSurface` first. This happens once per
    /// surface at setup rather than per frame, so the stall costs nothing.
    pub fn map_writable(hardware: &frame::Video) -> Result<Self> {
        Self::map_with(
            hardware,
            ffmpeg::ffi::AV_HWFRAME_MAP_READ as i32 | ffmpeg::ffi::AV_HWFRAME_MAP_WRITE as i32,
        )
    }

    fn map_with(hardware: &frame::Video, access: i32) -> Result<Self> {
        if hardware.format() != Pixel::VAAPI {
            return Err(MediaError::Invalid(format!(
                "only a VA surface can be exported as DMA-BUF, got {:?}",
                hardware.format()
            )));
        }

        let mut mapped = frame::Video::empty();
        mapped.set_format(Pixel::DRM_PRIME);

        // SAFETY: `mapped` is a freshly allocated `AVFrame` with no buffers and
        // its `format` set, which is the documented input to `av_hwframe_map`
        // for the map-to-a-format direction; handing it a frame that already
        // referenced data would leak that data. `hardware` is borrowed for the
        // call so its surface cannot be freed underneath it, and `&mut` on the
        // destination rules out the two being the same frame. On success
        // `mapped` owns a buffer whose destructor unmaps the surface and closes
        // the exported descriptors, and `frame::Video`'s own `Drop` runs it.
        let code = unsafe {
            ffmpeg::ffi::av_hwframe_map(
                mapped.as_mut_ptr(),
                hardware.as_ptr(),
                access | ffmpeg::ffi::AV_HWFRAME_MAP_DIRECT as i32,
            )
        };
        if code < 0 {
            return Err(MediaError::Hardware {
                what: "cannot export the decoded surface as DMA-BUF (this FFmpeg may be built \
                       without libdrm, or the driver may not support surface export)"
                    .into(),
                source: ffmpeg::Error::from(code),
            });
        }

        Ok(Self {
            mapped,
            width: hardware.width(),
            height: hardware.height(),
        })
    }

    /// Size of the picture, in pixels, before any chroma subsampling.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many distinct DRM buffers back this frame. One, on every driver
    /// seen so far; the format allows up to four.
    pub fn objects(&self) -> usize {
        self.descriptor()
            .map(|d| d.nb_objects as usize)
            .unwrap_or(0)
    }

    /// Every plane, in the order a software frame would have them.
    ///
    /// NV12 gives two — an `R8` luma plane and a `GR88` chroma plane at half
    /// resolution — and a packed format gives one. A caller that wants a single
    /// texture should check the length and refuse rather than silently show the
    /// luma plane, which is a plausible-looking greyscale picture and therefore
    /// the worst possible failure mode.
    pub fn planes(&self) -> Vec<Plane> {
        let Some(descriptor) = self.descriptor() else {
            return Vec::new();
        };

        let mut planes = Vec::new();
        let layers = (descriptor.nb_layers as usize).min(descriptor.layers.len());
        for layer in &descriptor.layers[..layers] {
            let count = (layer.nb_planes as usize).min(layer.planes.len());
            for (index, plane) in layer.planes[..count].iter().enumerate() {
                let object = descriptor
                    .objects
                    .get(plane.object_index.max(0) as usize)
                    .copied();
                let Some(object) = object else { continue };

                // Chroma planes of a subsampled layer are half size in each
                // axis. The descriptor does not say so — it is a property of
                // the fourcc — so it is derived from the pitch relative to the
                // first plane of the layer, which is the one thing that is
                // always true whatever the format.
                let (width, height) = plane_size(layer.format, index, self.width, self.height);

                planes.push(Plane {
                    fourcc: layer.format,
                    modifier: object.format_modifier,
                    offset: plane.offset.max(0) as u64,
                    pitch: plane.pitch.max(0) as u64,
                    object_size: object.size,
                    width,
                    height,
                    fd: object.fd,
                });
            }
        }
        planes
    }

    /// Whether every plane carries a modifier an importer can act on.
    pub fn is_importable(&self) -> bool {
        let planes = self.planes();
        !planes.is_empty() && planes.iter().all(Plane::is_importable)
    }

    /// The mapped frame, for a caller that needs the `AVFrame` itself.
    pub fn frame(&self) -> &frame::Video {
        &self.mapped
    }

    fn descriptor(&self) -> Option<&ffmpeg::ffi::AVDRMFrameDescriptor> {
        // SAFETY: libavutil's contract for an `AV_PIX_FMT_DRM_PRIME` frame is
        // that `data[0]` points at an `AVDRMFrameDescriptor` owned by the
        // frame's own buffer, valid until the frame is freed. `&self` borrows
        // the frame for the returned reference's lifetime, so it cannot be
        // freed while that reference exists. The null check covers a frame that
        // was never successfully mapped, which `map` makes unreachable but
        // which costs nothing to rule out here.
        unsafe {
            let raw = self.mapped.as_ptr();
            let data = (*raw).data[0] as *const ffmpeg::ffi::AVDRMFrameDescriptor;
            data.as_ref()
        }
    }
}

impl std::fmt::Debug for DmabufFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let planes = self.planes();
        f.debug_struct("DmabufFrame")
            .field("size", &(self.width, self.height))
            .field("objects", &self.objects())
            .field(
                "layers",
                &planes
                    .iter()
                    .map(|p| {
                        format!(
                            "{} {}x{} @{}+{}",
                            p.fourcc_name(),
                            p.width,
                            p.height,
                            p.offset,
                            p.pitch
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .field(
                "modifier",
                &planes.first().map(|p| format!("{:#018x}", p.modifier)),
            )
            .finish()
    }
}

/// Pixel size of plane `index` of a layer in `fourcc`.
///
/// Only the subsampled cases matter and there are few of them, so this is a
/// table rather than a `av_pix_fmt_desc_get` lookup: the DRM fourcc space and
/// the libav pixel-format space do not map onto each other cleanly, and a wrong
/// answer here produces a texture that samples off the end of its plane.
fn plane_size(fourcc: u32, index: usize, width: u32, height: u32) -> (u32, u32) {
    const NV12: u32 = fourcc_code(b"NV12");
    const NV21: u32 = fourcc_code(b"NV21");
    const P010: u32 = fourcc_code(b"P010");
    const GR88: u32 = fourcc_code(b"GR88");
    const GR32: u32 = fourcc_code(b"GR32");
    const YUV420: u32 = fourcc_code(b"YU12");

    match (fourcc, index) {
        // Two-plane 4:2:0 as one layer: plane 1 is the interleaved chroma.
        (NV12 | NV21 | P010, 1) => (width.div_ceil(2), height.div_ceil(2)),
        (YUV420, 1 | 2) => (width.div_ceil(2), height.div_ceil(2)),
        // Exported with SEPARATE_LAYERS, the chroma half of NV12 arrives as its
        // own `GR88` layer — two bytes per texel at half resolution in both
        // axes, which is precisely an `Rg8Unorm` texture of half the size.
        (GR88 | GR32, _) => (width.div_ceil(2), height.div_ceil(2)),
        _ => (width, height),
    }
}

/// `fourcc_code` from `drm_fourcc.h`, as a const fn so the table above can be
/// `match` arms.
const fn fourcc_code(name: &[u8; 4]) -> u32 {
    (name[0] as u32) | ((name[1] as u32) << 8) | ((name[2] as u32) << 16) | ((name[3] as u32) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_names_read_back_as_themselves() {
        assert_eq!(fourcc_name(fourcc_code(b"NV12")), "NV12");
        assert_eq!(fourcc_name(fourcc_code(b"AR24")), "AR24");
        assert_eq!(fourcc_name(fourcc_code(b"R8  ")), "R8  ");
        assert_eq!(fourcc_name(fourcc_code(b"GR88")), "GR88");
    }

    #[test]
    fn a_chroma_plane_is_half_the_picture_in_both_axes() {
        // Getting this wrong is how a UV texture samples off the end of its
        // plane, which on a tiled buffer is not a crash — it is garbage colour.
        assert_eq!(
            plane_size(fourcc_code(b"R8  "), 0, 1920, 1080),
            (1920, 1080)
        );
        assert_eq!(plane_size(fourcc_code(b"GR88"), 0, 1920, 1080), (960, 540));
        assert_eq!(
            plane_size(fourcc_code(b"NV12"), 0, 1920, 1080),
            (1920, 1080)
        );
        assert_eq!(plane_size(fourcc_code(b"NV12"), 1, 1920, 1080), (960, 540));
        // Odd sizes round up, so the chroma plane covers every luma sample.
        assert_eq!(plane_size(fourcc_code(b"GR88"), 0, 1921, 1081), (961, 541));
        // A packed format is one full-size plane.
        assert_eq!(plane_size(fourcc_code(b"AR24"), 0, 640, 480), (640, 480));
    }

    #[test]
    fn an_invalid_modifier_is_not_importable() {
        let plane = Plane {
            fourcc: fourcc_code(b"AR24"),
            modifier: DRM_FORMAT_MOD_INVALID,
            offset: 0,
            pitch: 2560,
            object_size: 0,
            width: 640,
            height: 480,
            fd: -1,
        };
        assert!(!plane.is_importable());
        assert!(Plane {
            modifier: DRM_FORMAT_MOD_LINEAR,
            ..plane
        }
        .is_importable());
    }

    #[test]
    fn mapping_something_that_is_not_a_va_surface_is_refused_with_prose() {
        let software = frame::Video::new(Pixel::NV12, 64, 64);
        let error = DmabufFrame::map(&software).unwrap_err();
        assert!(error.to_string().contains("VA surface"), "{error}");
    }
}
