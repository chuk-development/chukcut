//! chukcut patch (`vendor/README.md`): drawing `gpui::ExternalBuffer`s.
//!
//! The producer (chukcut's engine) renders on a Vulkan device of its own and
//! exports a BGRA8 buffer's memory as an opaque file descriptor. Here that
//! memory is imported into this renderer's device once per allocation, and
//! each new picture is copied into a texture of ours with one
//! `copy_buffer_to_texture` before the frame's render pass. The texture is
//! then drawn with the polychrome sprite pipeline, like any image.
//!
//! Anything that keeps the import from working — another GPU, no
//! `VK_KHR_external_memory_fd`, a backend that is not Vulkan — is reported to
//! the producer through `ExternalBuffer::report_failure`, and the surface is
//! not drawn; the producer falls back to `RenderImage`s.

use std::any::Any;
use std::os::fd::{BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};
use std::sync::{Arc, Weak};

use ash::vk;
use collections::FxHashMap;
use gpui::{ExternalBufferInfo, PaintSurface};

/// Textures kept for pictures that are no longer on screen, to be reused by
/// the next ones of the same size. Two: the picture just replaced, and one
/// being replaced while a second window shows the old one.
const SPARE_TEXTURES: usize = 2;

struct Import {
    buffer: wgpu::Buffer,
    /// The import is dropped once the producer has dropped the allocation.
    owner: Weak<dyn Any + Send + Sync>,
}

struct Picture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

/// Import `info`'s memory into `device`, as the renderer does on first sight
/// of an allocation. Public so a producer can test and measure the import
/// without a window; a renderer never needs it.
pub fn import_external_buffer(
    device: &wgpu::Device,
    info: &ExternalBufferInfo,
) -> Result<wgpu::Buffer, String> {
    ExternalBuffers::default().import(device, info)
}

#[derive(Default)]
pub(crate) struct ExternalBuffers {
    /// This device's `(deviceUUID, driverUUID)`; `Some(None)` when it is not a
    /// Vulkan device.
    identity: Option<Option<([u8; 16], [u8; 16])>>,
    imports: FxHashMap<u64, Import>,
    /// Allocations that failed to import, so they are not retried per frame.
    refused: FxHashMap<u64, Weak<dyn Any + Send + Sync>>,
    /// Copied pictures by `frame_id`.
    pictures: FxHashMap<u64, Picture>,
    spare: Vec<Picture>,
}

impl ExternalBuffers {
    /// Import and copy whatever `surfaces` show that is not on a texture yet.
    /// The owners of the memory read by the recorded copies are pushed to
    /// `owners`, to be held until the submission has finished.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        surfaces: &[PaintSurface],
        bind_group: impl Fn(&wgpu::TextureView) -> wgpu::BindGroup,
        owners: &mut Vec<Arc<dyn Any + Send + Sync>>,
    ) {
        self.imports
            .retain(|_, import| import.owner.strong_count() > 0);
        self.refused.retain(|_, owner| owner.strong_count() > 0);

        let shown = |frame_id: u64| surfaces.iter().any(|s| s.external.info().frame_id == frame_id);
        let gone: Vec<u64> = self
            .pictures
            .keys()
            .copied()
            .filter(|&frame_id| !shown(frame_id))
            .collect();
        for frame_id in gone {
            if let Some(picture) = self.pictures.remove(&frame_id) {
                self.spare.push(picture);
            }
        }
        if self.spare.len() > SPARE_TEXTURES {
            self.spare.drain(..self.spare.len() - SPARE_TEXTURES);
        }

        for surface in surfaces {
            let external = &surface.external;
            let info = *external.info();
            if self.pictures.contains_key(&info.frame_id)
                || self.refused.contains_key(&info.allocation_id)
            {
                continue;
            }
            if !self.imports.contains_key(&info.allocation_id) {
                match self.import(device, &info) {
                    Ok(buffer) => {
                        self.imports.insert(
                            info.allocation_id,
                            Import {
                                buffer,
                                owner: Arc::downgrade(external.owner()),
                            },
                        );
                    }
                    Err(reason) => {
                        log::warn!("cannot draw an external buffer: {reason}");
                        external.report_failure();
                        self.refused
                            .insert(info.allocation_id, Arc::downgrade(external.owner()));
                        continue;
                    }
                }
            }
            let size = (info.width, info.height);
            let picture = match self.spare.iter().position(|p| p.size == size) {
                Some(index) => self.spare.swap_remove(index),
                None => {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("external_buffer_texture"),
                        size: wgpu::Extent3d {
                            width: info.width,
                            height: info.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Bgra8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    Picture {
                        bind_group: bind_group(&view),
                        texture,
                        size,
                    }
                }
            };
            let import = &self.imports[&info.allocation_id];
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &import.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(info.bytes_per_row),
                        rows_per_image: None,
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &picture.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: info.width,
                    height: info.height,
                    depth_or_array_layers: 1,
                },
            );
            owners.push(Arc::clone(external.owner()));
            self.pictures.insert(info.frame_id, picture);
        }
    }

    /// The texture bind group for `frame_id`, if it was prepared.
    pub(crate) fn bind_group(&self, frame_id: u64) -> Option<&wgpu::BindGroup> {
        self.pictures.get(&frame_id).map(|p| &p.bind_group)
    }

    fn identity(&mut self, device: &wgpu::Device) -> Option<([u8; 16], [u8; 16])> {
        *self.identity.get_or_insert_with(|| {
            // SAFETY: only reads the identity of the device; the guard is
            // dropped before returning.
            let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }?;
            let physical = hal.raw_physical_device();
            let instance = hal.shared_instance().raw_instance().clone();
            drop(hal);
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
            // SAFETY: Vulkan 1.1 core; both structs live on the stack.
            unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
            Some((id.device_uuid, id.driver_uuid))
        })
    }

    /// Allocate the exported memory on this device and wrap it as a wgpu
    /// buffer that wgpu owns (and frees, once the GPU is done with it).
    fn import(
        &mut self,
        device: &wgpu::Device,
        info: &ExternalBufferInfo,
    ) -> Result<wgpu::Buffer, String> {
        let Some(identity) = self.identity(device) else {
            return Err("the renderer is not on Vulkan".into());
        };
        if identity != (info.device_uuid, info.driver_uuid) {
            return Err("the picture is on another GPU or driver".into());
        }
        if info.bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT != 0
            || u64::from(info.bytes_per_row) * u64::from(info.height) > info.buffer_size
            || info.width * 4 > info.bytes_per_row
        {
            return Err(format!("an inconsistent picture layout: {info:?}"));
        }

        // SAFETY: the raw handles below create new objects only, and the
        // guard does not outlive this block.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or("the renderer is not on Vulkan")?;
        if !hal
            .enabled_device_extensions()
            .contains(&ash::khr::external_memory_fd::NAME)
        {
            return Err("VK_KHR_external_memory_fd is not enabled".into());
        }
        let raw = hal.raw_device().clone();
        drop(hal);

        let mut external = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let create = vk::BufferCreateInfo::default()
            .size(info.buffer_size)
            .usage(vk::BufferUsageFlags::from_raw(info.usage))
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .push_next(&mut external);
        // SAFETY: a live create info describing a non-empty buffer.
        let buffer = unsafe { raw.create_buffer(&create, None) }
            .map_err(|e| format!("vkCreateBuffer: {e}"))?;
        let destroy = |raw: &ash::Device| {
            // SAFETY: the buffer was created above and never handed out.
            unsafe { raw.destroy_buffer(buffer, None) }
        };

        // SAFETY: `buffer` is live.
        let requirements = unsafe { raw.get_buffer_memory_requirements(buffer) };
        if requirements.memory_type_bits & (1 << info.memory_type_index) == 0
            || requirements.size > info.allocation_size
        {
            destroy(&raw);
            return Err("the exported memory does not fit a buffer here".into());
        }

        // SAFETY: the producer promises the descriptor is open while its owner
        // lives, and the caller holds the owner.
        let fd = unsafe { BorrowedFd::borrow_raw(info.fd) }
            .try_clone_to_owned()
            .map_err(|e| {
                destroy(&raw);
                format!("dup: {e}")
            })?
            .into_raw_fd();
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD)
            .fd(fd);
        // The exporter made a dedicated allocation; an import of one must say
        // so, with an identically created buffer.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(info.allocation_size)
            .memory_type_index(info.memory_type_index)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        // SAFETY: live structs; a successful import takes ownership of `fd`.
        let memory = match unsafe { raw.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(e) => {
                // A failed import leaves the descriptor with us.
                // SAFETY: `fd` is the descriptor duplicated above.
                drop(unsafe { OwnedFd::from_raw_fd(fd) });
                destroy(&raw);
                return Err(format!("importing the memory: {e}"));
            }
        };
        // SAFETY: dedicated memory for exactly this buffer, bound once.
        if let Err(e) = unsafe { raw.bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: neither object reached anyone else.
            unsafe { raw.free_memory(memory, None) };
            destroy(&raw);
            return Err(format!("vkBindBufferMemory: {e}"));
        }

        // SAFETY: wgpu takes over the buffer and the memory and frees both
        // when the `wgpu::Buffer` is dropped and its last use has finished.
        // The descriptor below is a subset of the real usage.
        let hal_buffer = unsafe {
            wgpu::hal::vulkan::Buffer::from_raw_managed(buffer, memory, 0, info.allocation_size)
        };
        Ok(unsafe {
            device.create_buffer_from_hal::<wgpu::hal::api::Vulkan>(
                hal_buffer,
                &wgpu::BufferDescriptor {
                    label: Some("external_buffer"),
                    size: info.buffer_size,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                },
            )
        })
    }
}
