//! Platform GPU-texture sharing for softblit.
//!
//! A [`SharedSurface`] is a GPU texture that both wgpu (softblit's renderer) and a native
//! compositor (Avalonia via `ICompositionGpuInterop`) can see with no copy. softblit renders its
//! final blit into [`SharedSurface::wgpu_texture`]; the compositor samples the same pixels through
//! the handle returned by [`SharedSurface::export_handle`].
//!
//! Everything platform-specific lives behind the trait so Linux (Vulkan opaque-fd) and macOS
//! (IOSurface) can add their own impls without touching the engine. v1 is Windows-only:
//! [`D3DSharedSurface`] backs the texture with a D3D11 keyed-mutex shared texture.

use core::fmt;

#[cfg(windows)]
mod windows_d3d;

#[cfg(windows)]
pub use windows_d3d::{D3DSharedSurface, create_vulkan_device};

#[cfg(windows)]
mod windows_dx12;

#[cfg(windows)]
pub use windows_dx12::{D3D12SharedSurface, create_dx12_device};

#[cfg(windows)]
mod windows_vulkan;

#[cfg(windows)]
pub use windows_vulkan::{VulkanSharedSurface, create_vulkan_export_device};

/// Pixel layout of the shared texture. Only BGRA8 (matching Avalonia's
/// `PlatformGraphicsExternalImageFormat.B8G8R8A8UNorm`) is wired up in v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedFormat {
    Bgra8Unorm,
}

/// How producer and consumer serialize access to the shared texture.
///
/// The keys are the *consumer's* (Avalonia's) keys: it acquires `consumer_acquire_key` and
/// releases `consumer_release_key`. The producer (this crate) mirrors them — it acquires
/// `consumer_release_key` and releases `consumer_acquire_key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    KeyedMutex {
        consumer_acquire_key: u64,
        consumer_release_key: u64,
    },
    /// A shared D3D12 fence (imported by the consumer as a semaphore). The producer signals
    /// monotonically increasing values; the consumer waits for the value published for the frame it
    /// composites. `fence_handle` is a shared NT handle.
    D3D12Fence {
        fence_handle: isize,
    },
    /// A pair of exported binary Vulkan semaphores (the pure Vulkan↔Vulkan path). The producer
    /// signals `render_finished` after rendering; the consumer waits on it before sampling, then
    /// signals `image_available` when done, which the producer waits on before the next frame —
    /// exactly Avalonia's `UpdateWithSemaphoresAsync(image, renderFinished, imageAvailable)` order.
    ///
    /// `memory_size` is the exported image's `VkMemoryRequirements::size`; Avalonia asserts its own
    /// imported image's requirements equal it (`PlatformGraphicsExternalImageProperties.MemorySize`).
    /// Both semaphore fields and the memory handle in [`SharedHandle`] are opaque NT handles.
    VulkanSemaphore {
        memory_size: u64,
        render_finished_handle: isize,
        image_available_handle: isize,
    },
}

/// Everything the consumer needs to import the shared texture.
///
/// `handle` is a raw OS handle value (a Windows `HANDLE` from `IDXGIResource::GetSharedHandle`).
/// It crosses the FFI boundary as a pointer-sized integer alongside the descriptive fields.
#[derive(Clone, Copy, Debug)]
pub struct SharedHandle {
    pub handle: isize,
    pub width: u32,
    pub height: u32,
    pub format: SharedFormat,
    pub sync: SyncKind,
}

/// A GPU texture shared between wgpu and a native compositor.
///
/// Producer render loop each frame: [`begin_producer`](SharedSurface::begin_producer), render into
/// [`wgpu_texture`](SharedSurface::wgpu_texture), submit, then
/// [`end_producer`](SharedSurface::end_producer). The consumer then acquires and composites.
pub trait SharedSurface {
    /// softblit's blit destination. BGRA8, sized to the current surface size.
    fn wgpu_texture(&self) -> &wgpu::Texture;

    /// The handle plus metadata to hand to the compositor. Stable until [`resize`](SharedSurface::resize).
    fn export_handle(&self) -> SharedHandle;

    /// Acquire the shared texture for GPU writes (producer side of the sync primitive).
    fn begin_producer(&self);

    /// Release the shared texture to the consumer. Ensures the producer's GPU writes have landed.
    fn end_producer(&self);

    /// Reallocate the shared texture at a new size, invalidating the previous
    /// [`export_handle`](SharedSurface::export_handle). Call when the target surface resizes.
    fn resize(&mut self, width: u32, height: u32);
}

/// Errors from allocating or importing a shared surface.
#[derive(Debug)]
pub enum NativeError {
    /// No GPU adapter / device could be created for the required backend.
    NoAdapter(String),
    /// The adapter lacks a capability the mechanism needs (e.g. `VULKAN_EXTERNAL_MEMORY_WIN32`).
    Unsupported(String),
    /// A platform GPU API (D3D11/DXGI) call failed.
    #[cfg(windows)]
    Platform(windows_core::Error),
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NativeError::NoAdapter(m) => write!(f, "no suitable GPU adapter: {m}"),
            NativeError::Unsupported(m) => write!(f, "unsupported: {m}"),
            #[cfg(windows)]
            NativeError::Platform(e) => write!(f, "platform GPU error: {e}"),
        }
    }
}

impl std::error::Error for NativeError {}

#[cfg(windows)]
impl From<windows_core::Error> for NativeError {
    fn from(e: windows_core::Error) -> Self {
        NativeError::Platform(e)
    }
}
