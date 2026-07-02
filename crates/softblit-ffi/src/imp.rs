//! The real, non-FFI engine logic. The diplomat bridge in [`crate::ffi`] is a thin marshalling
//! shell over [`SurfaceInner`]; keeping the substance here lets the Rust smoke test drive the
//! whole create -> share -> present path without the generated C ABI in the way.
//!
//! `SurfaceInner` wraps a `wgpu::Device`/`Queue`, a boxed [`SharedSurface`] backend (the shared
//! GPU texture + cross-API sync), and softblit's [`Surface`]. It is **not `Send`**: softblit's
//! `Surface` owns thread-affine GPU state, so a single owning thread must make every call.

use softblit::{Error, PixelFormat, PresentStats, Rect, ScalingMode, Surface, SurfaceDescriptor};
use softblit_native::{SharedHandle, SharedSurface};

/// Which platform sharing backend to allocate. Vulkan is the default (works on Intel iGPUs and is
/// the Linux-ready path); the D3D backends target discrete GPUs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Vulkan,
    D3D11,
    D3D12,
}

/// Why creating or driving a [`SurfaceInner`] failed. Flat discriminator so the C# side can branch;
/// the detailed cause is logged via `tracing` at the failure site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FfiError {
    NoAdapter,
    Unsupported,
    Platform,
    InvalidRect,
    BufferSizeMismatch,
    SurfaceLost,
    Device,
}

impl From<&Error> for FfiError {
    fn from(e: &Error) -> Self {
        match e {
            Error::WebGpuUnavailable { .. } => FfiError::NoAdapter,
            Error::SurfaceLost => FfiError::SurfaceLost,
            Error::InvalidRect { .. } => FfiError::InvalidRect,
            Error::BufferSizeMismatch { .. } => FfiError::BufferSizeMismatch,
            Error::Device { .. } => FfiError::Device,
            _ => FfiError::Device,
        }
    }
}

pub struct SurfaceInner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    backend: Box<dyn SharedSurface>,
    surface: Surface,
}

impl SurfaceInner {
    /// Picks the backend, creates a `Device`/`Queue` plus a shared BGRA8 `RENDER_ATTACHMENT`
    /// texture, then builds softblit's `Surface` around a clone of that texture. `width`/`height`
    /// seed both the shared (target) texture and the initial source framebuffer size.
    pub fn create(
        width: u32,
        height: u32,
        format: PixelFormat,
        scaling: ScalingMode,
        backend: Backend,
    ) -> Result<SurfaceInner, FfiError> {
        let width = width.max(1);
        let height = height.max(1);

        let (device, queue, shared): (wgpu::Device, wgpu::Queue, Box<dyn SharedSurface>) =
            create_backend(backend, width, height)?;

        let target = shared.wgpu_texture().clone();
        let desc = SurfaceDescriptor {
            source_size: (width, height),
            format,
            scaling,
        };
        let surface =
            Surface::new_shared(device.clone(), queue.clone(), target, desc).map_err(|e| {
                tracing::error!(error = %e, "Surface::new_shared failed");
                FfiError::from(&e)
            })?;

        Ok(SurfaceInner {
            device,
            queue,
            backend: shared,
            surface,
        })
    }

    pub fn share_handle(&self) -> SharedHandle {
        self.backend.export_handle()
    }

    /// Acquires the shared texture, uploads the dirty regions and blits, then releases it to the
    /// consumer. Mirrors the producer sync order every backend expects.
    pub fn present(&mut self, bytes: &[u8], dirty: &[Rect]) -> Result<PresentStats, FfiError> {
        self.backend.begin_producer();
        let result = self.surface.present_external(bytes, dirty);
        self.backend.end_producer();
        result.map_err(|e| {
            tracing::error!(error = %e, "present_external failed");
            FfiError::from(&e)
        })
    }

    pub fn resize_source(&mut self, width: u32, height: u32) {
        self.surface.resize_source(width.max(1), height.max(1));
    }

    /// Reallocates the shared (target) texture at a new size and rebuilds softblit's `Surface`
    /// around it, preserving source size / format / scaling. The previous share handle is
    /// invalidated; the caller must re-fetch [`SurfaceInner::share_handle`] and re-present. Any
    /// installed cursor overlay is dropped by the rebuild.
    pub fn resize_target(&mut self, width: u32, height: u32) -> Result<(), FfiError> {
        let source_size = self.surface.source_size();
        let format = self.surface.format();
        let scaling = self.surface.scaling();

        self.backend.resize(width.max(1), height.max(1));
        let target = self.backend.wgpu_texture().clone();
        let desc = SurfaceDescriptor {
            source_size,
            format,
            scaling,
        };
        self.surface =
            Surface::new_shared(self.device.clone(), self.queue.clone(), target, desc).map_err(
                |e| {
                    tracing::error!(error = %e, "Surface::new_shared failed during resize_target");
                    FfiError::from(&e)
                },
            )?;
        Ok(())
    }

    pub fn set_format(&mut self, format: PixelFormat) {
        self.surface.set_format(format);
    }

    pub fn set_scaling(&mut self, scaling: ScalingMode) {
        self.surface.set_scaling(scaling);
    }

    pub fn set_cursor(&mut self, image: &[u8], width: u32, height: u32) {
        self.surface.set_cursor(Some((image, width, height)));
    }

    pub fn clear_cursor(&mut self) {
        self.surface.set_cursor(None);
    }

    pub fn set_cursor_position(&mut self, x: i32, y: i32) {
        self.surface.set_cursor_position(x, y);
    }
}

#[cfg(windows)]
fn create_backend(
    backend: Backend,
    width: u32,
    height: u32,
) -> Result<(wgpu::Device, wgpu::Queue, Box<dyn SharedSurface>), FfiError> {
    use softblit_native::{
        D3D12SharedSurface, D3DSharedSurface, VulkanSharedSurface, create_dx12_device,
        create_vulkan_device, create_vulkan_export_device,
    };

    let map_native = |e: softblit_native::NativeError| {
        tracing::error!(error = %e, ?backend, "backend creation failed");
        match e {
            softblit_native::NativeError::NoAdapter(_) => FfiError::NoAdapter,
            softblit_native::NativeError::Unsupported(_) => FfiError::Unsupported,
            softblit_native::NativeError::Platform(_) => FfiError::Platform,
        }
    };

    match backend {
        Backend::Vulkan => {
            let (_instance, _adapter, device, queue) =
                pollster::block_on(create_vulkan_export_device()).map_err(map_native)?;
            let shared = VulkanSharedSurface::new(device.clone(), queue.clone(), width, height)
                .map_err(map_native)?;
            Ok((device, queue, Box::new(shared)))
        }
        Backend::D3D11 => {
            let (_instance, _adapter, device, queue) =
                pollster::block_on(create_vulkan_device()).map_err(map_native)?;
            let shared = D3DSharedSurface::new(device.clone(), queue.clone(), width, height)
                .map_err(map_native)?;
            Ok((device, queue, Box::new(shared)))
        }
        Backend::D3D12 => {
            let (_instance, _adapter, device, queue) =
                pollster::block_on(create_dx12_device()).map_err(map_native)?;
            let shared = D3D12SharedSurface::new(device.clone(), queue.clone(), width, height)
                .map_err(map_native)?;
            Ok((device, queue, Box::new(shared)))
        }
    }
}

#[cfg(not(windows))]
fn create_backend(
    _backend: Backend,
    _width: u32,
    _height: u32,
) -> Result<(wgpu::Device, wgpu::Queue, Box<dyn SharedSurface>), FfiError> {
    Err(FfiError::Unsupported)
}
