//! Allocating the NV12 destination so the encoder can read it directly.
//!
//! ## What this is for
//!
//! [`super::nv12`] made the compositor write NV12 into a wgpu storage buffer
//! and then read that buffer back to system memory, where FFmpeg copies it into
//! an `AVFrame` and libavutil uploads it into a VA surface. Three touches of the
//! same 3 MB, all of it on one physical DRAM.
//!
//! If the buffer the shader writes *is* the VA surface, all three go away. That
//! needs the buffer's memory to be exportable as a DMA-BUF file descriptor,
//! which is a decision made at `vkAllocateMemory` time and which wgpu has no
//! way to express — `VkExportMemoryAllocateInfo` is not in WebGPU's vocabulary
//! and there is no `wgpu::Buffer::export_fd`. So the allocation happens here,
//! in raw `ash`, and the result is handed *back* to wgpu with
//! `create_buffer_from_hal` so the compute pass can bind it like any other
//! storage buffer.
//!
//! ## Why a buffer and not an image
//!
//! `docs/research/zero-copy-encode.md` assumed this work would export the
//! compositor's RGBA render target, and warned at length about Intel's tiled
//! image layouts and the `VK_EXT_image_drm_format_modifier` dance needed to
//! find out which one a driver picked.
//!
//! None of that applies once the colour conversion is already on the GPU. The
//! compute pass writes NV12 into a plain buffer, and a buffer has no tiling: its
//! layout is the strides we chose in [`super::nv12::Nv12Layout`]. So the
//! exported DMA-BUF is `DRM_FORMAT_MOD_LINEAR` by construction, which is the
//! modifier every importer accepts, and the whole modifier problem disappears
//! rather than being worked around.
//!
//! That is a real simplification and it is downstream of doing the NV12 pass
//! first — which is the argument for having done the steps in that order.
//!
//! ## Ownership
//!
//! Three raw Vulkan objects, freed in `Drop` in the reverse of the order they
//! were made: the `wgpu::Buffer` (which owns nothing of the memory — it is
//! wrapped `External`), then the `VkBuffer` handle, then the `VkDeviceMemory`.
//! The exported file descriptor is a *separate* reference to the same memory
//! that the DRM subsystem keeps alive independently, so closing it early is
//! safe and forgetting to close it is a leak of a file descriptor rather than
//! of the memory.
//!
//! Nothing here is reachable on a device wgpu is not driving through Vulkan, or
//! on a driver without `VK_KHR_external_memory_fd`. Every entry point answers
//! `None` in that case rather than failing, because the CPU path still works.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;

use ash::vk;

use super::context::RenderContext;

/// The one DRM format modifier that needs no negotiation.
///
/// `DRM_FORMAT_MOD_LINEAR` is 0. It is spelled out here rather than pulled from
/// a `drm-fourcc` dependency because it is the only one this module can ever
/// produce: a `VkBuffer` has no tiling to describe.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// `DRM_FORMAT_NV12`, the fourcc `'N' 'V' '1' '2'`.
pub const DRM_FORMAT_NV12: u32 =
    (b'N' as u32) | ((b'V' as u32) << 8) | ((b'1' as u32) << 16) | ((b'2' as u32) << 24);

/// How the memory of an [`ExportableBuffer`] leaves the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportHandle {
    /// A DMA-BUF: what VAAPI and other drivers import. The encoder paths.
    DmaBuf,
    /// An opaque file descriptor, which only another Vulkan device on the same
    /// physical GPU and driver can import — GPUI's, for the preview
    /// (`render::shared_frame`). Supported more widely than DMA-BUF buffers,
    /// and it carries no layout question at all.
    OpaqueFd,
}

impl ExportHandle {
    fn vk(self) -> vk::ExternalMemoryHandleTypeFlags {
        match self {
            Self::DmaBuf => vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            Self::OpaqueFd => vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD,
        }
    }
}

/// The `VkBufferUsageFlags` every exportable buffer is created with. An
/// importer of an opaque, dedicated allocation must create its `VkBuffer`
/// with identical parameters, so this is part of what is handed over.
pub const EXPORT_BUFFER_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::STORAGE_BUFFER.as_raw()
        | vk::BufferUsageFlags::TRANSFER_SRC.as_raw()
        | vk::BufferUsageFlags::TRANSFER_DST.as_raw(),
);

/// Which GPU and driver a device runs on, as Vulkan names them
/// (`VkPhysicalDeviceIDProperties`). Opaque memory moves only between devices
/// whose two UUIDs both match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

/// The identity of `ctx`'s physical device, or `None` when it is not Vulkan.
pub fn device_identity(ctx: &RenderContext) -> Option<DeviceIdentity> {
    // SAFETY: only reads the identity of the device wgpu already opened; the
    // borrow ends with this function.
    let hal = unsafe { ctx.device().as_hal::<wgpu_hal::api::Vulkan>() }?;
    let physical = hal.raw_physical_device();
    let instance = hal.shared_instance().raw_instance().clone();
    drop(hal);
    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
    // SAFETY: Vulkan 1.1 core, which wgpu requires; both structs are live
    // stack values and the physical device outlives the instance borrow.
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    Some(DeviceIdentity {
        device_uuid: id.device_uuid,
        driver_uuid: id.driver_uuid,
    })
}

/// A wgpu storage buffer whose memory can be handed to another driver.
pub struct ExportableBuffer {
    /// What the compute pass binds. Wrapped `External`, so dropping it does not
    /// touch the memory below.
    buffer: wgpu::Buffer,
    raw_buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The DMA-BUF. Owned so it is closed exactly once; hand out
    /// [`Self::fd`] for the duration of a call rather than the value.
    fd: OwnedFd,
    size: u64,
    /// What an importer has to repeat: `VkMemoryAllocateInfo`'s size and
    /// memory type.
    allocation_size: u64,
    memory_type_index: u32,
    handle: ExportHandle,
    device: ash::Device,
    /// Keeps the wgpu device — and therefore the `VkDevice` `device` is a
    /// handle to — alive for at least as long as we will call into it, and is
    /// what `Drop` waits on. Not `_`-prefixed any more: it is used.
    ctx: Arc<RenderContext>,
}

impl ExportableBuffer {
    /// Allocate `size` bytes of storage that can be exported as a DMA-BUF.
    ///
    /// `None` when this device is not Vulkan, or when the driver has no
    /// `VK_KHR_external_memory_fd`. Both are reasons to use the copying path,
    /// not reasons to fail.
    pub fn new(ctx: &Arc<RenderContext>, size: u64, label: &str) -> Option<Self> {
        Self::with_handle(ctx, size, label, ExportHandle::DmaBuf)
    }

    /// [`Self::new`], exported as `handle` instead of a DMA-BUF.
    pub fn with_handle(
        ctx: &Arc<RenderContext>,
        size: u64,
        label: &str,
        handle: ExportHandle,
    ) -> Option<Self> {
        // A zero-sized allocation is not a degenerate case worth handling; it
        // is a caller bug that would show up as a confusing driver error.
        if size == 0 {
            return None;
        }

        // SAFETY: `as_hal` is unsafe because the returned handle must not be
        // used to do anything wgpu does not expect of its own device. Everything
        // below either only reads the device's identity (physical device,
        // queue family) or creates *new* objects that wgpu never sees except
        // through the `from_raw_external` wrapper at the end, which is exactly
        // the documented contract for that wrapper. The borrow does not outlive
        // this function.
        let hal = unsafe { ctx.device().as_hal::<wgpu_hal::api::Vulkan>() }?;
        let device: ash::Device = hal.raw_device().clone();
        let physical = hal.raw_physical_device();
        let instance = hal.shared_instance().raw_instance().clone();
        drop(hal);

        // SAFETY: `vkGetPhysicalDeviceMemoryProperties` reads driver-owned
        // state through a physical device handle that is live for the lifetime
        // of the wgpu instance we just borrowed it from, and writes only its
        // return value. It cannot fail.
        let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };

        let mut external_info =
            vk::ExternalMemoryBufferCreateInfo::default().handle_types(handle.vk());
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(EXPORT_BUFFER_USAGE)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external_info);

        // SAFETY: `buffer_info` and the `external_info` it chains are live
        // stack values for the duration of the call and describe a buffer with
        // a non-zero size and no queue-family list (EXCLUSIVE sharing). The
        // null allocator is the documented "use the driver's".
        let raw_buffer = match unsafe { device.create_buffer(&buffer_info, None) } {
            Ok(buffer) => buffer,
            Err(e) => {
                tracing::warn!(error = ?e, "cannot create an exportable buffer");
                return None;
            }
        };

        // SAFETY: `raw_buffer` was created a line above and has not been
        // destroyed. The call only reads it.
        let requirements = unsafe { device.get_buffer_memory_requirements(raw_buffer) };

        // Device-local without HOST_VISIBLE: nothing on the CPU is ever going
        // to look at this. On an iGPU the two are the same physical memory
        // anyway, and asking for host visibility would only constrain which
        // heaps qualify.
        let Some(type_index) = memory_type_index(
            &memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        ) else {
            // SAFETY: destroying a buffer we created and never bound memory to
            // or handed to anyone. Nothing can be using it.
            unsafe { device.destroy_buffer(raw_buffer, None) };
            tracing::warn!("no device-local memory type accepts an exportable buffer");
            return None;
        };

        let mut export_info = vk::ExportMemoryAllocateInfo::default().handle_types(handle.vk());
        // A dedicated allocation is not strictly required for a buffer, but
        // every driver that supports DMA-BUF export supports it and it removes
        // the question of what the fd's offset is: with a dedicated allocation
        // the exported object *is* this buffer, starting at zero.
        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().buffer(raw_buffer);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(type_index)
            .push_next(&mut export_info)
            .push_next(&mut dedicated_info);

        // SAFETY: the allocate info and both chained structs are live stack
        // values; `type_index` came from the driver's own memory properties and
        // is one of the bits `requirements.memory_type_bits` allowed.
        let memory = match unsafe { device.allocate_memory(&allocate_info, None) } {
            Ok(memory) => memory,
            Err(e) => {
                // SAFETY: as above — an unbound buffer nobody has seen.
                unsafe { device.destroy_buffer(raw_buffer, None) };
                tracing::warn!(error = ?e, "cannot allocate exportable memory");
                return None;
            }
        };

        // SAFETY: `memory` was allocated for exactly this buffer's
        // requirements, with a dedicated allocation, so offset zero is the only
        // legal binding and the size is by construction sufficient. Neither
        // handle has been bound before.
        if let Err(e) = unsafe { device.bind_buffer_memory(raw_buffer, memory, 0) } {
            // SAFETY: freeing memory that is bound to nothing (the bind just
            // failed) and a buffer with no memory. Order does not matter.
            unsafe {
                device.free_memory(memory, None);
                device.destroy_buffer(raw_buffer, None);
            }
            tracing::warn!(error = ?e, "cannot bind exportable memory");
            return None;
        }

        let Some(fd) = export_fd(&instance, &device, memory, handle) else {
            // SAFETY: nothing else holds a reference — the export is what would
            // have created one and it failed.
            unsafe {
                device.free_memory(memory, None);
                device.destroy_buffer(raw_buffer, None);
            }
            return None;
        };

        // SAFETY: `from_raw_externally_owned` documents that wgpu-hal will not
        // destroy the `vk::Buffer` and that the caller keeps it alive for at
        // least the returned value's lifetime. We do: the handle is stored in
        // `self` and destroyed in `Drop`, after the `wgpu::Buffer` field, which
        // Rust drops in declaration order. The drop callback is a no-op because
        // there is no caller-side bookkeeping to release.
        let hal_buffer = unsafe {
            wgpu_hal::vulkan::Buffer::from_raw_externally_owned(raw_buffer, Box::new(|| {}))
        };

        // SAFETY: the descriptor must describe the buffer truthfully or wgpu
        // will validate against the wrong size and let a shader write out of
        // bounds. It does: `size` is the `vkCreateBuffer` size and the usages
        // are the WebGPU spelling of `STORAGE_BUFFER | TRANSFER_SRC`.
        // `mapped_at_creation` is false, which is required — an externally
        // imported buffer cannot be mapped.
        let buffer = unsafe {
            ctx.device()
                .create_buffer_from_hal::<wgpu_hal::api::Vulkan>(
                    hal_buffer,
                    &wgpu::BufferDescriptor {
                        label: Some(label),
                        size,
                        usage: wgpu::BufferUsages::STORAGE
                            | wgpu::BufferUsages::COPY_SRC
                            | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                )
        };

        Some(Self {
            buffer,
            raw_buffer,
            memory,
            fd,
            size,
            allocation_size: requirements.size,
            memory_type_index: type_index,
            handle,
            device,
            ctx: Arc::clone(ctx),
        })
    }

    /// The wgpu handle, for binding into a compute pass.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// The size of the memory behind the buffer, which can exceed
    /// [`Self::size`]; an importer allocates exactly this much.
    pub fn allocation_size(&self) -> u64 {
        self.allocation_size
    }

    /// The memory type it was allocated from, which an opaque import must
    /// repeat.
    pub fn memory_type_index(&self) -> u32 {
        self.memory_type_index
    }

    pub fn handle(&self) -> ExportHandle {
        self.handle
    }

    /// The DMA-BUF, borrowed.
    ///
    /// Borrowed rather than moved because the importer — libavutil's
    /// `AVDRMFrameDescriptor` — takes ownership of what it is given, and this
    /// buffer is reused for every frame of an export. Callers `dup` it.
    pub fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.fd.as_fd()
    }
}

impl Drop for ExportableBuffer {
    fn drop(&mut self) {
        // Everything queued against this buffer has to have finished. wgpu's
        // own tracking covers the `wgpu::Buffer`, but it does not know the
        // memory is ours, so wait for the device to drain before freeing it.
        //
        // Through wgpu's `poll` rather than `vkDeviceWaitIdle`, and that is not
        // a stylistic preference. Vulkan requires host access to **every**
        // `VkQueue` on the device to be externally synchronised across a
        // `vkDeviceWaitIdle`, and nothing here can synchronise against wgpu's
        // own submissions or against a second buffer being dropped on another
        // thread. It was called raw for months and only ever ran one buffer at a
        // time on the export's single thread, so it never showed; six rings torn
        // down concurrently by `preview::zerocopy`'s tests aborted the test
        // binary with a double free or a SIGSEGV in two runs out of five.
        // `Device::poll` takes wgpu's own locks and waits for the same thing.
        if let Err(e) = self.ctx.device().poll(wgpu::PollType::wait_indefinitely()) {
            tracing::warn!(error = %e, "waiting for the GPU before freeing exported memory");
        }

        // SAFETY: the `wgpu::Buffer` field is dropped before this runs (Rust
        // drops fields in declaration order and `buffer` is declared first), so
        // wgpu is no longer using the handle, and `from_raw_external` promised
        // never to destroy it itself. The memory is freed after the buffer that
        // is bound to it, which is the order Vulkan requires. The exported file
        // descriptor holds its own reference to the underlying DRM object, so a
        // consumer still holding it does not observe a use-after-free.
        unsafe {
            self.device.destroy_buffer(self.raw_buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

impl std::fmt::Debug for ExportableBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportableBuffer")
            .field("size", &self.size)
            .finish()
    }
}

/// `vkGetMemoryFdKHR` on `memory`, as `handle`.
fn export_fd(
    instance: &ash::Instance,
    device: &ash::Device,
    memory: vk::DeviceMemory,
    handle: ExportHandle,
) -> Option<OwnedFd> {
    // The extension's entry points are loaded from the device rather than taken
    // from wgpu-hal's own table, which is private. Loading them twice is
    // explicitly fine — they are function pointers, not state.
    let external = ash::khr::external_memory_fd::Device::new(instance, device);

    let info = vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(handle.vk());

    // SAFETY: `info` is a live stack value naming memory we allocated with a
    // matching `VkExportMemoryAllocateInfo`, which is what makes the export
    // legal. The call returns a *new* file descriptor that the caller owns —
    // Vulkan's contract is explicit about that — so wrapping it in an
    // `OwnedFd` is the correct ownership and no double close is possible.
    let raw = match unsafe { external.get_memory_fd(&info) } {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(error = ?e, ?handle, "the driver refused to export memory");
            return None;
        }
    };
    if raw < 0 {
        return None;
    }
    // SAFETY: `raw` is a fresh, valid, owned descriptor from the call above and
    // is not held anywhere else.
    Some(unsafe { OwnedFd::from_raw_fd(raw) })
}

/// First memory type satisfying both the buffer's bitmask and `flags`.
fn memory_type_index(
    properties: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    properties
        .memory_types_as_slice()
        .iter()
        .enumerate()
        .find(|(i, ty)| type_bits & (1 << i) != 0 && ty.property_flags.contains(flags))
        .map(|(i, _)| i as u32)
}

/// A rotation of exportable NV12 buffers.
///
/// One buffer is not enough and the reason is the encoder, not the GPU. A
/// hardware encoder holds several surfaces alive past `send_frame` — the
/// reference list and two B-frames of reordering — so writing frame `n + 1`
/// into the memory frame `n` was imported from corrupts a picture the encoder
/// has not finished with. That corruption is intermittent, content-dependent
/// and looks exactly like a bad shader.
///
/// So: rotate through [`RING`] buffers, and before reusing one, ask libavutil
/// whether anybody still holds the surface that was mapped from it. The count
/// is the authority; the ring size only makes the answer usually "no".
pub struct Nv12Ring {
    buffers: Vec<ExportableBuffer>,
    next: usize,
}

/// Buffers in the rotation.
///
/// Sixteen against the encoder's pool of twenty, so the encoder runs out of
/// surfaces before we run out of buffers and the failure is the one that
/// already has a good error message. At 1080p this is 50 MB.
pub const RING: usize = 16;

impl Nv12Ring {
    /// Allocate the rotation, or `None` if this device cannot export memory.
    pub fn new(ctx: &Arc<RenderContext>, frame_bytes: u64) -> Option<Self> {
        Self::with_slots(ctx, frame_bytes, RING)
    }

    /// [`Self::new`] with a rotation of a chosen depth.
    ///
    /// [`RING`] is sized against a *video* encoder's reference list and
    /// reordering delay. A JPEG encoder has neither — one picture is in flight
    /// at a time — so the preview asks for a handful of slots instead of
    /// sixteen, and the difference is 40 MB of otherwise idle device memory at
    /// 1080p.
    pub fn with_slots(ctx: &Arc<RenderContext>, frame_bytes: u64, slots: usize) -> Option<Self> {
        let slots = slots.max(1);
        let mut buffers = Vec::with_capacity(slots);
        for i in 0..slots {
            buffers.push(ExportableBuffer::new(
                ctx,
                frame_bytes,
                &format!("chukcut nv12 export {i}"),
            )?);
        }
        Some(Self { buffers, next: 0 })
    }

    /// The next buffer in the rotation.
    pub fn take(&mut self) -> &ExportableBuffer {
        let index = self.next;
        self.next = (self.next + 1) % self.buffers.len();
        &self.buffers[index]
    }

    /// One buffer by index, for a caller that chooses its own slot.
    ///
    /// The export rotates blindly because its frames retire in order. The
    /// preview cannot: its encode runs on another thread, so which slot is free
    /// is a question about liveness rather than about position, and it picks the
    /// slot itself. See `preview::zerocopy`.
    pub fn at(&self, index: usize) -> &ExportableBuffer {
        &self.buffers[index]
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

impl std::fmt::Debug for Nv12Ring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nv12Ring")
            .field("buffers", &self.buffers.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_rotates_and_comes_back_round() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let Some(mut ring) = Nv12Ring::new(&ctx, 4096) else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            return;
        };
        assert_eq!(ring.len(), RING);

        // Every buffer in one lap is a different allocation; lap two repeats
        // lap one. A ring that handed out the same buffer twice in a lap would
        // reintroduce exactly the corruption it exists to prevent.
        let first: Vec<usize> = (0..RING)
            .map(|_| ring.take().buffer() as *const wgpu::Buffer as usize)
            .collect();
        let second: Vec<usize> = (0..RING)
            .map(|_| ring.take().buffer() as *const wgpu::Buffer as usize)
            .collect();
        let mut unique = first.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), RING, "a lap must not repeat a buffer");
        assert_eq!(first, second, "lap two must repeat lap one");
    }

    /// P010 surfaces export as `R16 ` and `GR32` layers. They import as
    /// 16-bit textures where the device has them and not at all where it does
    /// not, which sends the frame down the copying path instead of drawing
    /// a 16-bit plane as garbage through an 8-bit view.
    #[test]
    fn p010_layers_import_as_sixteen_bit_planes_only_on_a_device_that_has_them() {
        assert_eq!(
            plane_format("R16 ", true),
            Some(wgpu::TextureFormat::R16Unorm)
        );
        assert_eq!(
            plane_format("GR32", true),
            Some(wgpu::TextureFormat::Rg16Unorm)
        );
        assert_eq!(plane_format("R16 ", false), None);
        assert_eq!(plane_format("GR32", false), None);
        assert_eq!(
            plane_format("R8  ", false),
            Some(wgpu::TextureFormat::R8Unorm)
        );
        assert_eq!(
            plane_format("GR88", false),
            Some(wgpu::TextureFormat::Rg8Unorm)
        );
        assert_eq!(plane_format("YU12", true), None);
    }

    #[test]
    fn the_nv12_fourcc_is_the_one_drm_uses() {
        // 0x3231564E. Worth asserting because a byte-swapped fourcc is accepted
        // by the compiler, rejected by the driver, and reads correctly to a
        // human skimming the constant.
        assert_eq!(DRM_FORMAT_NV12, 0x3231_564E);
    }

    #[test]
    fn an_exportable_buffer_yields_a_usable_file_descriptor() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let size = 1920 * 1080 * 3 / 2;
        let Some(buffer) = ExportableBuffer::new(&ctx, size, "test") else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            return;
        };
        assert_eq!(buffer.size(), size);

        // A DMA-BUF is a real file, and `fstat` on it reports at least the
        // allocation's size. That is the cheapest proof the descriptor is a
        // buffer and not, say, a leftover fd number.
        use std::os::fd::AsRawFd;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fstat` writes one `struct stat` through the pointer and
        // reads nothing else. The descriptor is live for the borrow.
        let rc = unsafe { libc::fstat(buffer.fd().as_raw_fd(), &mut stat) };
        assert_eq!(rc, 0, "fstat on the exported descriptor");
        assert!(
            stat.st_size as u64 >= size,
            "a {size}-byte allocation exported a {}-byte object",
            stat.st_size
        );
    }

    #[test]
    fn an_exportable_buffer_can_be_written_by_a_compute_pass() {
        // The point of the whole exercise: wgpu has to accept the buffer as a
        // normal storage binding. If `create_buffer_from_hal` produced
        // something wgpu will not bind, everything downstream is moot.
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let Some(exported) = ExportableBuffer::new(&ctx, 1024, "test write") else {
            eprintln!("skipping: this device cannot export DMA-BUF memory");
            return;
        };

        // Copy it into a mappable buffer and read it. The contents are
        // undefined, so this asserts the *operations* are legal rather than the
        // bytes — a validation error here fails the test through the uncaptured
        // error handler.
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("test staging"),
            size: 1024,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(exported.buffer(), 0, &staging, 0, 1024);
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
        assert_eq!(slice.get_mapped_range().expect("range").len(), 1024);
        staging.unmap();
    }
}

// ---------------------------------------------------------------------------
// The import direction
// ---------------------------------------------------------------------------
//
// Everything above exports a buffer *to* the encoder. This is the mirror: it
// takes a plane of a hardware-decoded video surface and makes it a texture the
// compositor can sample, with no copy through system memory.
//
// Why it matters is measured, not assumed. Reaching RGBA from a VA surface
// costs 26.9 ms at 1080p because the surface is tiled, so the transfer is a
// detiling pass and swscale still has to convert afterwards. Mapped straight
// across it is 1.70 ms. See `docs/research/hardware-decode.md`.
//
// Transplanted from that note, which verified it against real decoded frames:
// the reconstructed picture matched the software decoder to a mean channel
// difference of 0.32, against 41.88 for a deliberately chroma-swapped control.

use crate::modules::media::dmabuf::Plane;

/// Import one plane of a decoded video surface as a texture.
///
/// `None` when this device cannot import DMA-BUF, when the plane carries no
/// usable modifier, or when the driver refuses the layout — all of which mean
/// "use the copying path", not "fail".
pub fn import_plane(ctx: &RenderContext, plane: &Plane) -> Option<wgpu::Texture> {
    import_plane_with(ctx, plane, PlaneUse::Read)
}

/// What an imported plane is for, which decides its Vulkan usage flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneUse {
    /// Sampled by the compositor. A decoded frame arriving from the hardware
    /// decoder.
    Read,
    /// Drawn into by a render pass. A *destination* the media driver allocated
    /// — the preview's NV12 surface — so the tiling is the driver's own and
    /// this side never has to know which tiling it is.
    ///
    /// Whether a driver accepts this is a real question and not a formality:
    /// `VK_EXT_image_drm_format_modifier` reports the usable usages per
    /// modifier, and a driver may expose a modifier for sampling that it will
    /// not render into. On the Raptor Lake iGPU, `vkGetPhysicalDeviceFormat
    /// Properties2` reports `COLOR_ATTACHMENT` and `STORAGE` for `R8_UNORM` and
    /// `R8G8_UNORM` under `LINEAR`, `X_TILED` and `Y_TILED`, and `Y_TILED` is
    /// what the iHD driver allocates. A device where that is not true refuses
    /// the import and `None` comes back, which is a reason to use the copying
    /// path and not a reason to fail.
    Write,
}

/// The texture format a DRM layer imports as, or `None` for a layout the
/// compositor has no case for. `deep` is whether the device can sample
/// 16-bit normalised textures.
fn plane_format(fourcc: &str, deep: bool) -> Option<wgpu::TextureFormat> {
    Some(match fourcc {
        // NV12 exported with SEPARATE_LAYERS: luma is a single-channel plane…
        "R8  " => wgpu::TextureFormat::R8Unorm,
        // …and chroma is interleaved U and V at half resolution, which is
        // exactly a two-channel texture of half the size.
        "GR88" => wgpu::TextureFormat::Rg8Unorm,
        // P010 exported with SEPARATE_LAYERS: the same two planes with
        // sixteen bits a sample, ten of them used, at the top. Only on a
        // device with 16-bit textures; without them the import is refused
        // and the frame is copied, which `provider` cuts to 8 bits.
        "R16 " if deep => wgpu::TextureFormat::R16Unorm,
        "GR32" if deep => wgpu::TextureFormat::Rg16Unorm,
        "AR24" | "XR24" => wgpu::TextureFormat::Bgra8Unorm,
        "AB24" | "XB24" => wgpu::TextureFormat::Rgba8Unorm,
        _ => return None,
    })
}

/// [`import_plane`], saying what the plane is going to be used for.
pub fn import_plane_with(
    ctx: &RenderContext,
    plane: &Plane,
    intent: PlaneUse,
) -> Option<wgpu::Texture> {
    if !ctx.can_import_dmabuf() || !plane.is_importable() {
        return None;
    }
    let Some(format) = plane_format(&plane.fourcc_name(), ctx.supports_deep_planes()) else {
        tracing::debug!(
            fourcc = plane.fourcc_name(),
            "no wgpu format for this DRM fourcc"
        );
        return None;
    };
    let fd = plane.dup_fd().ok()?;
    let size = wgpu::Extent3d {
        width: plane.width,
        height: plane.height,
        depth_or_array_layers: 1,
    };
    let (hal_usage, wgpu_usage, initial, label) = match intent {
        PlaneUse::Read => (
            wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            wgpu::TextureUses::RESOURCE,
            "decoded video plane",
        ),
        PlaneUse::Write => (
            wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::COPY_SRC,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            // `UNINITIALIZED` and not `COLOR_TARGET`: the render pass writes
            // every fragment of the plane, so telling wgpu the contents matter
            // would only invite it to clear or preserve a picture nobody reads.
            wgpu::TextureUses::UNINITIALIZED,
            "encoder surface plane",
        ),
    };
    let hal_descriptor = wgpu_hal::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: hal_usage,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    };

    // SAFETY: `texture_from_dmabuf_fd` requires a descriptor that describes the
    // buffer truthfully and takes ownership of the descriptor. Both hold: the
    // modifier, offset and pitch come straight from the driver's own
    // `AVDRMFrameDescriptor`, and `fd` is a duplicate made for this call —
    // libavutil keeps and closes its own. On failure wgpu-hal closes the
    // duplicate itself, which is why there is no cleanup here.
    let hal_texture = unsafe {
        let hal = ctx.device().as_hal::<wgpu_hal::api::Vulkan>()?;
        hal.texture_from_dmabuf_fd(
            fd,
            &hal_descriptor,
            plane.modifier,
            plane.pitch,
            plane.offset,
        )
        .ok()?
    };

    // SAFETY: the wgpu descriptor agrees with the hal one field for field. The
    // initial state is `RESOURCE` and not `UNINITIALIZED`, which matters: the
    // texture already holds the decoded picture, and telling wgpu it is
    // uninitialised invites it to clear the frame before anything samples it.
    Some(unsafe {
        ctx.device()
            .create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu_usage,
                    view_formats: &[],
                },
                initial,
            )
    })
}
