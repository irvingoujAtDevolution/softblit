//! Windows shared-surface impl: a D3D11 keyed-mutex texture, rendered into by wgpu's Vulkan
//! backend via `VK_KHR_external_memory_win32`, composited by Avalonia through its battle-tested
//! `D3D11TextureGlobalSharedHandle` + keyed-mutex path.

use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIKeyedMutex, IDXGIResource};
use windows_core::Interface;

use crate::{NativeError, SharedFormat, SharedHandle, SharedSurface, SyncKind};

const INFINITE: u32 = 0xFFFF_FFFF;
/// The producer acquires this key each frame; the consumer releases it back after compositing.
const PRODUCER_ACQUIRE_KEY: u64 = 0;
/// The producer releases this key after rendering; the consumer acquires it to composite.
const PRODUCER_RELEASE_KEY: u64 = 1;

/// Builds a wgpu device on the Vulkan backend with `VULKAN_EXTERNAL_MEMORY_WIN32` enabled — the
/// device that [`D3DSharedSurface`] needs to import a D3D11 texture. The engine can create its own
/// device this way, or reuse an equivalent one.
pub async fn create_vulkan_device()
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

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("softblit shared device"),
            required_features: wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32,
            required_limits: wgpu::Limits::default(),
            ..wgpu::DeviceDescriptor::default()
        })
        .await
        .map_err(|e| NativeError::NoAdapter(e.to_string()))?;

    Ok((instance, adapter, device, queue))
}

/// A GPU texture backed by a D3D11 `SharedKeyedmutex` texture, rendered into by wgpu (Vulkan) and
/// composited by Avalonia (D3D11). The wgpu device must be a Vulkan-backend device with the
/// `VULKAN_EXTERNAL_MEMORY_WIN32` feature (see [`create_vulkan_device`]).
pub struct D3DSharedSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    width: u32,
    height: u32,

    // The D3D11 objects must outlive the imported wgpu texture: they own the shared memory the
    // Vulkan image is bound to.
    _d3d_device: ID3D11Device,
    _d3d_texture: ID3D11Texture2D,
    keyed_mutex: IDXGIKeyedMutex,
    shared_handle: HANDLE,

    wgpu_texture: wgpu::Texture,
}

impl D3DSharedSurface {
    /// Allocates the shared texture on `device` (which must satisfy the requirements above) and
    /// imports it into wgpu. `queue` is used to flush producer writes before handing off.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, NativeError> {
        let d3d_device = create_d3d_device()?;
        let (d3d_texture, keyed_mutex, shared_handle) =
            create_shared_texture(&d3d_device, width, height)?;
        let wgpu_texture = import_texture(&device, shared_handle, width, height)?;

        Ok(Self {
            device,
            queue,
            width,
            height,
            _d3d_device: d3d_device,
            _d3d_texture: d3d_texture,
            keyed_mutex,
            shared_handle,
            wgpu_texture,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

impl SharedSurface for D3DSharedSurface {
    fn wgpu_texture(&self) -> &wgpu::Texture {
        &self.wgpu_texture
    }

    fn export_handle(&self) -> SharedHandle {
        SharedHandle {
            handle: self.shared_handle.0 as isize,
            width: self.width,
            height: self.height,
            format: SharedFormat::Bgra8Unorm,
            sync: SyncKind::KeyedMutex {
                consumer_acquire_key: PRODUCER_RELEASE_KEY,
                consumer_release_key: PRODUCER_ACQUIRE_KEY,
            },
        }
    }

    fn begin_producer(&self) {
        unsafe {
            self.keyed_mutex
                .AcquireSync(PRODUCER_ACQUIRE_KEY, INFINITE)
                .expect("keyed mutex acquire (producer)");
        }
    }

    fn end_producer(&self) {
        // No Vulkan↔D3D shared semaphore here, so make the handoff safe by draining the queue: the
        // keyed-mutex release must not precede the GPU writes it is meant to publish.
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll device to drain producer submission");
        unsafe {
            self.keyed_mutex
                .ReleaseSync(PRODUCER_RELEASE_KEY)
                .expect("keyed mutex release (producer)");
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (d3d_texture, keyed_mutex, shared_handle) =
            create_shared_texture(&self._d3d_device, width, height)
                .expect("recreate shared D3D11 texture on resize");
        let wgpu_texture = import_texture(&self.device, shared_handle, width, height)
            .expect("reimport shared texture on resize");

        self.width = width;
        self.height = height;
        self._d3d_texture = d3d_texture;
        self.keyed_mutex = keyed_mutex;
        self.shared_handle = shared_handle;
        self.wgpu_texture = wgpu_texture;
    }
}

fn create_d3d_device() -> Result<ID3D11Device, NativeError> {
    let mut device: Option<ID3D11Device> = None;
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| NativeError::Unsupported("D3D11CreateDevice returned no device".into()))
}

fn create_shared_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, IDXGIKeyedMutex, HANDLE), NativeError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width.max(1),
        Height: height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    let texture =
        texture.ok_or_else(|| NativeError::Unsupported("CreateTexture2D returned null".into()))?;

    let dxgi_res: IDXGIResource = texture.cast()?;
    let shared_handle = unsafe { dxgi_res.GetSharedHandle()? };
    let keyed_mutex: IDXGIKeyedMutex = texture.cast()?;

    Ok((texture, keyed_mutex, shared_handle))
}

/// Imports the D3D11 shared texture into wgpu as a `wgpu::Texture` via wgpu-hal's Vulkan
/// `texture_from_d3d11_shared_handle`. The descriptors must agree with the D3D11 texture.
fn import_texture(
    device: &wgpu::Device,
    handle: HANDLE,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, NativeError> {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };
    let hal_desc = wgpu::hal::TextureDescriptor {
        label: Some("softblit shared (hal)"),
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

    // SAFETY: `handle` comes from `GetSharedHandle` on a D3D11 `SharedKeyedmutex` texture created
    // with exactly `hal_desc`'s size/format, and the device is a Vulkan backend device with the
    // `VULKAN_EXTERNAL_MEMORY_WIN32` feature — the contract of `texture_from_d3d11_shared_handle`.
    let hal_texture = unsafe {
        let vk_device = device
            .as_hal::<wgpu::hal::api::Vulkan>()
            .ok_or_else(|| NativeError::Unsupported("wgpu device is not a Vulkan device".into()))?;
        vk_device
            .texture_from_d3d11_shared_handle(handle, &hal_desc)
            .map_err(|e| {
                NativeError::Unsupported(format!("texture_from_d3d11_shared_handle failed: {e:?}"))
            })?
    };

    let wgpu_desc = wgpu::TextureDescriptor {
        label: Some("softblit shared"),
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

    // SAFETY: `hal_texture` was just created from `device` respecting `wgpu_desc` and is initialized
    // by the import.
    Ok(unsafe { device.create_texture_from_hal::<wgpu::hal::api::Vulkan>(hal_texture, &wgpu_desc) })
}
