//! Windows pure-Vulkan shared-surface impl — the only on-screen path on Intel iGPUs, and the
//! natural Linux path too.
//!
//! wgpu is created on its Vulkan backend and *we* (via raw `ash`, reaching through
//! [`wgpu::Device::as_hal`]) allocate an exportable `VkImage` + dedicated `VkDeviceMemory` plus two
//! exportable binary `VkSemaphore`s. The image memory and the semaphores cross to Avalonia's Vulkan
//! compositor as opaque NT handles; Avalonia imports them and composites with
//! `UpdateWithSemaphoresAsync`. wgpu-hal 29 only *imports* Vulkan external memory, so the export
//! side is hand-written here.
//!
//! The image is created byte-for-byte like Avalonia's importer (`VulkanImageBase.Initialize`):
//! `B8G8R8A8_UNORM`, optimal tiling, `COLOR_ATTACHMENT|TRANSFER_DST|TRANSFER_SRC|SAMPLED`, mutable
//! format, dedicated allocation — so both sides compute the same `VkMemoryRequirements::size`, which
//! Avalonia asserts on import.
//!
//! The POSIX-fd Linux variant slots in by swapping `OPAQUE_WIN32` for `OPAQUE_FD` and the two
//! `external_*_win32` extensions/exporters for their `_fd` counterparts.

use std::cell::Cell;

use ash::vk;

use crate::{NativeError, SharedFormat, SharedHandle, SharedSurface, SyncKind};

const IMAGE_FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;
const HANDLE_TYPE: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::OPAQUE_WIN32;
const SEM_HANDLE_TYPE: vk::ExternalSemaphoreHandleTypeFlags =
    vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_WIN32;

/// The layout wgpu leaves the image in after a render pass, and the layout the barrier submits
/// juggle so it agrees with wgpu's own state tracker.
const RENDER_LAYOUT: vk::ImageLayout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
/// The layout Avalonia's importer expects to sample from (`ImportedImage` transitions to it and
/// never leaves it), so the producer must hand the image off in it.
const HANDOFF_LAYOUT: vk::ImageLayout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;

fn vk_err(context: &'static str) -> impl FnOnce(vk::Result) -> NativeError {
    move |e| NativeError::Unsupported(format!("{context}: {e:?}"))
}

/// Builds a wgpu device on the Vulkan backend with **both** `VK_KHR_external_memory_win32` (via the
/// `VULKAN_EXTERNAL_MEMORY_WIN32` feature) and `VK_KHR_external_semaphore_win32` enabled. wgpu-hal
/// auto-enables the memory extension but not the semaphore one, so we inject it through
/// [`open_with_callback`](wgpu::hal::vulkan::Adapter::open_with_callback).
pub async fn create_vulkan_export_device()
-> Result<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue), NativeError> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(desc);

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .map_err(|e| NativeError::NoAdapter(e.to_string()))?;

    if !adapter
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32)
    {
        return Err(NativeError::Unsupported(
            "adapter's Vulkan driver lacks VK_KHR_external_memory_win32 (VULKAN_EXTERNAL_MEMORY_WIN32)"
                .into(),
        ));
    }

    let features = wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
    let limits = wgpu::Limits::default();
    let memory_hints = wgpu::MemoryHints::default();

    // SAFETY: `adapter` is a Vulkan-backend adapter (checked via its features above); the callback
    // only *adds* a device extension the driver supports, which `open_with_callback` explicitly
    // permits. The returned `OpenDevice` is handed straight to `create_device_from_hal`.
    let open = unsafe {
        let hal_adapter = adapter
            .as_hal::<wgpu::hal::api::Vulkan>()
            .ok_or_else(|| NativeError::Unsupported("wgpu adapter is not a Vulkan adapter".into()))?;
        hal_adapter
            .open_with_callback(
                features,
                &limits,
                &memory_hints,
                Some(Box::new(|args: wgpu::hal::vulkan::CreateDeviceCallbackArgs<'_, '_, '_>| {
                    let name = ash::khr::external_semaphore_win32::NAME;
                    if !args.extensions.contains(&name) {
                        args.extensions.push(name);
                    }
                })),
            )
            .map_err(|e| NativeError::NoAdapter(format!("open Vulkan device: {e:?}")))?
    };

    // SAFETY: `open` was just produced by this adapter's `open_with_callback` with `features`.
    let (device, queue) = unsafe {
        adapter
            .create_device_from_hal::<wgpu::hal::api::Vulkan>(
                open,
                &wgpu::DeviceDescriptor {
                    label: Some("softblit vulkan export device"),
                    required_features: features,
                    required_limits: limits,
                    memory_hints,
                    ..wgpu::DeviceDescriptor::default()
                },
            )
            .map_err(|e| NativeError::NoAdapter(e.to_string()))?
    };

    let info = adapter.get_info();
    tracing::info!(
        adapter = %info.name,
        backend = ?info.backend,
        "created wgpu Vulkan export device (external_memory_win32 + external_semaphore_win32)"
    );

    Ok((instance, adapter, device, queue))
}

/// Raw Vulkan handles pulled once out of the wgpu device, so the surface never needs to re-enter
/// `as_hal` on the hot path. `ash::Device`/`ash::Instance` are cheap clones (fn-pointer tables).
struct VkContext {
    instance: ash::Instance,
    device: ash::Device,
    physical: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family_index: u32,
    mem_win32: ash::khr::external_memory_win32::Device,
    sem_win32: ash::khr::external_semaphore_win32::Device,
}

impl VkContext {
    fn from_wgpu(device: &wgpu::Device) -> Result<Self, NativeError> {
        // SAFETY: reading the raw Vulkan handles out of the hal device; they stay valid as long as
        // `device` (held by the surface) is alive, and `ash::{Device,Instance}` clones are just
        // reference-counted dispatch tables.
        unsafe {
            let hal = device
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or_else(|| NativeError::Unsupported("wgpu device is not a Vulkan device".into()))?;
            let instance = hal.shared_instance().raw_instance().clone();
            let raw_device = hal.raw_device().clone();
            let mem_win32 = ash::khr::external_memory_win32::Device::new(&instance, &raw_device);
            let sem_win32 = ash::khr::external_semaphore_win32::Device::new(&instance, &raw_device);
            Ok(Self {
                physical: hal.raw_physical_device(),
                queue: hal.raw_queue(),
                queue_family_index: hal.queue_family_index(),
                instance,
                device: raw_device,
                mem_win32,
                sem_win32,
            })
        }
    }
}

/// A GPU texture backed by an exportable `VkImage`/`VkDeviceMemory`, rendered into by wgpu (Vulkan)
/// and composited by Avalonia's Vulkan compositor after importing the memory + semaphore NT handles.
pub struct VulkanSharedSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,

    vk: VkContext,

    // We own the memory + semaphores + command pool; wgpu owns the `VkImage` (destroyed when the
    // `wgpu::Texture` drops), so memory is freed only after the texture is dropped and the device
    // polled — see `free_image_resources`.
    image_memory: vk::DeviceMemory,
    memory_size: u64,
    memory_handle: isize,

    render_finished: vk::Semaphore,
    image_available: vk::Semaphore,
    render_finished_handle: isize,
    image_available_handle: isize,

    command_pool: vk::CommandPool,
    begin_first_cb: vk::CommandBuffer,
    begin_cb: vk::CommandBuffer,
    end_cb: vk::CommandBuffer,

    wgpu_texture: Option<wgpu::Texture>,
    first_frame: Cell<bool>,
}

impl VulkanSharedSurface {
    /// Allocates the exportable image + semaphores on `device` (which must come from
    /// [`create_vulkan_export_device`]) and imports the image into wgpu.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, NativeError> {
        let vk = VkContext::from_wgpu(&device)?;

        let (image_memory, memory_size, memory_handle, wgpu_texture) =
            create_shared_image(&vk, &device, width, height)?;

        let (render_finished, render_finished_handle) = create_exportable_semaphore(&vk)?;
        let (image_available, image_available_handle) = create_exportable_semaphore(&vk)?;

        let (command_pool, begin_first_cb, begin_cb, end_cb) =
            create_barrier_command_buffers(&vk, wgpu_image(&wgpu_texture))?;

        tracing::info!(
            width,
            height,
            memory_size,
            memory_handle = format_args!("{memory_handle:#x}"),
            render_finished_handle = format_args!("{render_finished_handle:#x}"),
            image_available_handle = format_args!("{image_available_handle:#x}"),
            "allocated exportable Vulkan shared surface"
        );

        Ok(Self {
            device,
            queue,
            width,
            height,
            vk,
            image_memory,
            memory_size,
            memory_handle,
            render_finished,
            image_available,
            render_finished_handle,
            image_available_handle,
            command_pool,
            begin_first_cb,
            begin_cb,
            end_cb,
            wgpu_texture: Some(wgpu_texture),
            first_frame: Cell::new(true),
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Submits one barrier command buffer, optionally waiting/signalling a semaphore, on wgpu's
    /// Vulkan queue.
    fn submit_barrier(
        &self,
        cb: vk::CommandBuffer,
        wait: Option<vk::Semaphore>,
        signal: Option<vk::Semaphore>,
    ) {
        let cbs = [cb];
        let wait_sems = wait.map(|s| [s]);
        let wait_stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let signal_sems = signal.map(|s| [s]);

        let mut submit = vk::SubmitInfo::default().command_buffers(&cbs);
        if let Some(w) = wait_sems.as_ref() {
            submit = submit.wait_semaphores(w).wait_dst_stage_mask(&wait_stages);
        }
        if let Some(s) = signal_sems.as_ref() {
            submit = submit.signal_semaphores(s);
        }

        // SAFETY: `cb` is a pre-recorded, resubmittable command buffer for this surface's image;
        // `self.vk.queue` is wgpu's Vulkan queue used only from this (single) thread, so external
        // synchronization of the queue holds. Semaphores are the surface's own exported binaries.
        unsafe {
            self.vk
                .device
                .queue_submit(self.vk.queue, &[submit], vk::Fence::null())
                .expect("vkQueueSubmit for producer barrier");
        }
    }

    /// Drops the wgpu texture (destroying its `VkImage`), flushes wgpu's deferred destruction, then
    /// frees the image memory. Semaphores/pool are left for the caller.
    fn free_image_resources(&mut self) {
        drop(self.wgpu_texture.take());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll device before freeing Vulkan image memory");
        // SAFETY: the `VkImage` bound to this memory was just destroyed (texture dropped + device
        // polled), so freeing the memory has no live users.
        unsafe {
            self.vk.device.free_memory(self.image_memory, None);
        }
    }
}

impl SharedSurface for VulkanSharedSurface {
    fn wgpu_texture(&self) -> &wgpu::Texture {
        self.wgpu_texture
            .as_ref()
            .expect("VulkanSharedSurface texture accessed after teardown")
    }

    fn export_handle(&self) -> SharedHandle {
        SharedHandle {
            handle: self.memory_handle,
            width: self.width,
            height: self.height,
            format: SharedFormat::Bgra8Unorm,
            sync: SyncKind::VulkanSemaphore {
                memory_size: self.memory_size,
                render_finished_handle: self.render_finished_handle,
                image_available_handle: self.image_available_handle,
            },
        }
    }

    fn begin_producer(&self) {
        // Restore the image to wgpu's expected render layout. On the first frame the image is fresh
        // (UNDEFINED) and nothing has signalled `image_available` yet, so skip the wait.
        if self.first_frame.get() {
            self.submit_barrier(self.begin_first_cb, None, None);
            self.first_frame.set(false);
        } else {
            self.submit_barrier(self.begin_cb, Some(self.image_available), None);
        }
    }

    fn end_producer(&self) {
        // Transition to the compositor's sample layout and signal that this frame is rendered.
        // Ordered on the queue after wgpu's render submit (same queue, single thread).
        self.submit_barrier(self.end_cb, None, Some(self.render_finished));
    }

    fn resize(&mut self, width: u32, height: u32) {
        // Drain any in-flight producer work so the command buffers and image aren't reset while used.
        // SAFETY: waits for the queue to go idle before we recreate its resources.
        unsafe {
            self.vk
                .device
                .queue_wait_idle(self.vk.queue)
                .expect("queue_wait_idle before Vulkan resize");
        }

        self.free_image_resources();

        let (image_memory, memory_size, memory_handle, wgpu_texture) =
            create_shared_image(&self.vk, &self.device, width, height)
                .expect("recreate exportable Vulkan image on resize");

        record_barrier_command_buffers(
            &self.vk,
            wgpu_image(&wgpu_texture),
            self.begin_first_cb,
            self.begin_cb,
            self.end_cb,
        )
        .expect("re-record barrier command buffers on resize");

        self.width = width;
        self.height = height;
        self.image_memory = image_memory;
        self.memory_size = memory_size;
        self.memory_handle = memory_handle;
        self.wgpu_texture = Some(wgpu_texture);
        self.first_frame.set(true);
    }
}

impl Drop for VulkanSharedSurface {
    fn drop(&mut self) {
        // SAFETY: nothing else references these objects once the surface is dropped; we drain the
        // queue first so no submission is still reading them.
        unsafe {
            let _ = self.vk.device.queue_wait_idle(self.vk.queue);
        }
        self.free_image_resources();
        // SAFETY: post-idle teardown of this surface's own Vulkan objects.
        unsafe {
            self.vk.device.destroy_command_pool(self.command_pool, None);
            self.vk.device.destroy_semaphore(self.render_finished, None);
            self.vk.device.destroy_semaphore(self.image_available, None);
        }
    }
}

fn wgpu_image(texture: &wgpu::Texture) -> vk::Image {
    // SAFETY: `texture` is a Vulkan-backend texture created via `create_texture_from_hal`; we only
    // read its raw `VkImage` handle.
    unsafe {
        texture
            .as_hal::<wgpu::hal::api::Vulkan>()
            .map(|t| t.raw_handle())
            .expect("wgpu texture is not a Vulkan texture")
    }
}

fn create_shared_image(
    vk: &VkContext,
    wgpu_device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(vk::DeviceMemory, u64, isize, wgpu::Texture), NativeError> {
    let width = width.max(1);
    let height = height.max(1);

    let mut external_info = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE_TYPE);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(IMAGE_FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .flags(vk::ImageCreateFlags::MUTABLE_FORMAT)
        .push_next(&mut external_info);

    // SAFETY: valid `ImageCreateInfo` with an `ExternalMemoryImageCreateInfo` extension; the device
    // supports OPAQUE_WIN32 external memory (checked at device creation).
    let image = unsafe { vk.device.create_image(&image_info, None) }
        .map_err(vk_err("vkCreateImage (exportable)"))?;

    // If anything below fails, destroy the orphaned image before returning.
    let result = (|| {
        // SAFETY: `image` was just created on this device.
        let mem_req = unsafe { vk.device.get_image_memory_requirements(image) };
        // SAFETY: reading memory properties of the surface's physical device.
        let mem_props = unsafe {
            vk.instance
                .get_physical_device_memory_properties(vk.physical)
        };
        let mem_type_index = (0..mem_props.memory_type_count)
            .find(|&i| {
                (mem_req.memory_type_bits & (1 << i)) != 0
                    && mem_props.memory_types[i as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .ok_or_else(|| {
                NativeError::Unsupported("no DEVICE_LOCAL memory type for shared image".into())
            })?;

        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut export = vk::ExportMemoryAllocateInfo::default().handle_types(HANDLE_TYPE);
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(mem_type_index)
            .push_next(&mut dedicated)
            .push_next(&mut export);

        // SAFETY: dedicated + exportable allocation for `image` sized to its requirements.
        let memory = unsafe { vk.device.allocate_memory(&alloc_info, None) }
            .map_err(vk_err("vkAllocateMemory (exportable)"))?;
        // SAFETY: `memory` is a dedicated allocation for `image`, bound at offset 0.
        unsafe { vk.device.bind_image_memory(image, memory, 0) }
            .map_err(vk_err("vkBindImageMemory"))?;

        let get_info = vk::MemoryGetWin32HandleInfoKHR::default()
            .memory(memory)
            .handle_type(HANDLE_TYPE);
        // SAFETY: `memory` was allocated exportable with OPAQUE_WIN32; the returned NT handle is
        // owned by the caller and handed to the importer (leaked for the spike's lifetime).
        let handle = unsafe { vk.mem_win32.get_memory_win32_handle(&get_info) }
            .map_err(vk_err("vkGetMemoryWin32HandleKHR"))?;

        let texture = wrap_image_in_wgpu(wgpu_device, image, width, height);
        Ok((memory, mem_req.size, handle, texture))
    })();

    if result.is_err() {
        // SAFETY: on the error path the image is unbound/unwrapped; destroy it to avoid a leak.
        unsafe { vk.device.destroy_image(image, None) };
    }
    result
}

fn wrap_image_in_wgpu(
    wgpu_device: &wgpu::Device,
    image: vk::Image,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hal_desc = wgpu::hal::TextureDescriptor {
        label: Some("softblit vulkan shared (hal)"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUses::COLOR_TARGET
            | wgpu::TextureUses::RESOURCE
            | wgpu::TextureUses::COPY_SRC,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: vec![],
    };

    // SAFETY: `image` was created on `wgpu_device`'s Vulkan device with exactly `hal_desc`'s
    // size/format/usage and has its memory bound. `TextureMemory::External` tells wgpu the memory is
    // ours; wgpu owns the `VkImage` and destroys it when the returned texture drops.
    let hal_texture = unsafe {
        let hal_device = wgpu_device
            .as_hal::<wgpu::hal::api::Vulkan>()
            .expect("wgpu device is not a Vulkan device");
        hal_device.texture_from_raw(
            image,
            &hal_desc,
            None,
            wgpu::hal::vulkan::TextureMemory::External,
        )
    };

    let wgpu_desc = wgpu::TextureDescriptor {
        label: Some("softblit vulkan shared"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    };
    // SAFETY: `hal_texture` was created from `wgpu_device` respecting `wgpu_desc`.
    unsafe { wgpu_device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(hal_texture, &wgpu_desc) }
}

fn create_exportable_semaphore(vk: &VkContext) -> Result<(vk::Semaphore, isize), NativeError> {
    let mut export = vk::ExportSemaphoreCreateInfo::default().handle_types(SEM_HANDLE_TYPE);
    let info = vk::SemaphoreCreateInfo::default().push_next(&mut export);
    // SAFETY: valid exportable binary semaphore create info; device supports OPAQUE_WIN32 semaphores.
    let semaphore = unsafe { vk.device.create_semaphore(&info, None) }
        .map_err(vk_err("vkCreateSemaphore (exportable)"))?;

    let get_info = vk::SemaphoreGetWin32HandleInfoKHR::default()
        .semaphore(semaphore)
        .handle_type(SEM_HANDLE_TYPE);
    // SAFETY: `semaphore` was created exportable; returned NT handle is owned by the caller and
    // handed to the importer.
    let handle = unsafe { vk.sem_win32.get_semaphore_win32_handle(&get_info) }
        .map_err(vk_err("vkGetSemaphoreWin32HandleKHR"))?;
    Ok((semaphore, handle))
}

fn create_barrier_command_buffers(
    vk: &VkContext,
    image: vk::Image,
) -> Result<
    (
        vk::CommandPool,
        vk::CommandBuffer,
        vk::CommandBuffer,
        vk::CommandBuffer,
    ),
    NativeError,
> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .queue_family_index(vk.queue_family_index)
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
    // SAFETY: pool created on wgpu's queue family.
    let pool = unsafe { vk.device.create_command_pool(&pool_info, None) }
        .map_err(vk_err("vkCreateCommandPool"))?;

    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(3);
    // SAFETY: allocating 3 primary command buffers from the pool just created.
    let cbs = unsafe { vk.device.allocate_command_buffers(&alloc_info) }
        .map_err(vk_err("vkAllocateCommandBuffers"))?;
    let (begin_first, begin, end) = (cbs[0], cbs[1], cbs[2]);

    record_barrier_command_buffers(vk, image, begin_first, begin, end)?;
    Ok((pool, begin_first, begin, end))
}

/// (Re-)records the three static barrier command buffers for `image`.
fn record_barrier_command_buffers(
    vk: &VkContext,
    image: vk::Image,
    begin_first: vk::CommandBuffer,
    begin: vk::CommandBuffer,
    end: vk::CommandBuffer,
) -> Result<(), NativeError> {
    record_transition(vk, begin_first, image, vk::ImageLayout::UNDEFINED, RENDER_LAYOUT)?;
    record_transition(vk, begin, image, HANDOFF_LAYOUT, RENDER_LAYOUT)?;
    record_transition(vk, end, image, RENDER_LAYOUT, HANDOFF_LAYOUT)?;
    Ok(())
}

fn record_transition(
    vk: &VkContext,
    cb: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) -> Result<(), NativeError> {
    let begin_info = vk::CommandBufferBeginInfo::default();
    let barrier = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });

    // SAFETY: `cb` is owned by this surface's pool and not in flight (fresh, or drained via
    // queue_wait_idle on resize). The barrier targets this surface's image with a valid range.
    unsafe {
        vk.device
            .reset_command_buffer(cb, vk::CommandBufferResetFlags::empty())
            .map_err(vk_err("vkResetCommandBuffer"))?;
        vk.device
            .begin_command_buffer(cb, &begin_info)
            .map_err(vk_err("vkBeginCommandBuffer"))?;
        vk.device.cmd_pipeline_barrier(
            cb,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
        vk.device
            .end_command_buffer(cb)
            .map_err(vk_err("vkEndCommandBuffer"))?;
    }
    Ok(())
}
