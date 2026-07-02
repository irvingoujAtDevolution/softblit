//! Windows shared-surface impl using wgpu's DX12 backend — the all-D3D fallback for machines whose
//! Vulkan driver can't import a D3D11 texture (notably Intel iGPUs). wgpu owns an `ID3D12Resource`
//! created with `D3D12_HEAP_FLAG_SHARED`; the resource and a shared `ID3D12Fence` cross to the
//! consumer as NT handles. Sync is a monotonic fence (imported as a semaphore) rather than a keyed
//! mutex.

use std::cell::Cell;

use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATE_COMMON, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandQueue, ID3D12Device,
    ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows_core::PCWSTR;

use crate::{NativeError, SharedFormat, SharedHandle, SharedSurface, SyncKind};

/// Builds a wgpu device on the DX12 backend. No special features are needed: this path keeps the
/// shared resource inside D3D, so there is no cross-API import to negotiate.
pub async fn create_dx12_device()
-> Result<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue), NativeError> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(desc);

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .map_err(|e| NativeError::NoAdapter(e.to_string()))?;

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("softblit dx12 shared device"),
            ..wgpu::DeviceDescriptor::default()
        })
        .await
        .map_err(|e| NativeError::NoAdapter(e.to_string()))?;

    Ok((instance, adapter, device, queue))
}

/// A GPU texture backed by a shared `ID3D12Resource`, rendered into by wgpu (DX12) and composited
/// by the consumer after opening the resource's NT handle. Sync is via a shared `ID3D12Fence`.
pub struct D3D12SharedSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    d3d12_device: ID3D12Device,
    d3d12_queue: ID3D12CommandQueue,
    width: u32,
    height: u32,

    fence: ID3D12Fence,
    fence_handle: HANDLE,
    fence_value: Cell<u64>,

    resource_handle: HANDLE,
    wgpu_texture: wgpu::Texture,
}

impl D3D12SharedSurface {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, NativeError> {
        // SAFETY: `device` is a DX12 wgpu device (checked below); the raw device/queue are only used
        // to create shared resources on the same device wgpu renders with.
        let (d3d12_device, d3d12_queue) = unsafe {
            let hal_device = device
                .as_hal::<wgpu::hal::api::Dx12>()
                .ok_or_else(|| NativeError::Unsupported("wgpu device is not a DX12 device".into()))?;
            (hal_device.raw_device().clone(), hal_device.raw_queue().clone())
        };

        let fence: ID3D12Fence =
            unsafe { d3d12_device.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }?;
        let fence_handle =
            unsafe { d3d12_device.CreateSharedHandle(&fence, None, GENERIC_ALL.0, PCWSTR::null()) }?;

        let (resource_handle, wgpu_texture) =
            create_shared_resource(&d3d12_device, &device, width, height)?;

        Ok(Self {
            device,
            queue,
            d3d12_device,
            d3d12_queue,
            width,
            height,
            fence,
            fence_handle,
            fence_value: Cell::new(0),
            resource_handle,
            wgpu_texture,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The fence value the producer will have signalled after the most recent [`end_producer`].
    pub fn fence_value(&self) -> u64 {
        self.fence_value.get()
    }
}

impl SharedSurface for D3D12SharedSurface {
    fn wgpu_texture(&self) -> &wgpu::Texture {
        &self.wgpu_texture
    }

    fn export_handle(&self) -> SharedHandle {
        SharedHandle {
            handle: self.resource_handle.0 as isize,
            width: self.width,
            height: self.height,
            format: SharedFormat::Bgra8Unorm,
            sync: SyncKind::D3D12Fence {
                fence_handle: self.fence_handle.0 as isize,
            },
        }
    }

    fn begin_producer(&self) {
        // The consumer's read of the previous frame is ordered by the fence it waited on; nothing to
        // acquire here.
    }

    fn end_producer(&self) {
        // wgpu's submit went to `d3d12_queue` (its present queue); signalling the shared fence on the
        // same queue publishes "render for this frame is done" on the GPU timeline.
        let value = self.fence_value.get() + 1;
        self.fence_value.set(value);
        unsafe {
            self.d3d12_queue
                .Signal(&self.fence, value)
                .expect("signal shared fence");
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (resource_handle, wgpu_texture) =
            create_shared_resource(&self.d3d12_device, &self.device, width, height)
                .expect("recreate shared D3D12 resource on resize");
        self.width = width;
        self.height = height;
        self.resource_handle = resource_handle;
        self.wgpu_texture = wgpu_texture;
    }
}

fn create_shared_resource(
    d3d12_device: &ID3D12Device,
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(HANDLE, wgpu::Texture), NativeError> {
    let width = width.max(1);
    let height = height.max(1);

    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let res_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    };

    let mut resource: Option<ID3D12Resource> = None;
    unsafe {
        d3d12_device.CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_SHARED,
            &res_desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut resource,
        )
    }?;
    let resource =
        resource.ok_or_else(|| NativeError::Unsupported("CreateCommittedResource null".into()))?;

    let handle =
        unsafe { d3d12_device.CreateSharedHandle(&resource, None, GENERIC_ALL.0, PCWSTR::null()) }?;

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // SAFETY: `resource` was just created on this DX12 device with `size`/format matching the
    // descriptors, and is wrapped exactly once.
    let hal_texture = unsafe {
        wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureDimension::D2,
            size,
            1,
            1,
        )
    };
    let wgpu_desc = wgpu::TextureDescriptor {
        label: Some("softblit shared (dx12)"),
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
    // SAFETY: `hal_texture` was created from `device` respecting `wgpu_desc`.
    let texture =
        unsafe { device.create_texture_from_hal::<wgpu::hal::api::Dx12>(hal_texture, &wgpu_desc) };

    Ok((handle, texture))
}
