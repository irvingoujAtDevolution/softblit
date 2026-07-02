//! # softblit-ffi — diplomat bridge over softblit's shared-surface engine
//!
//! Exposes an opaque [`ffi::SoftblitSurface`] that owns a `wgpu::Device`/`Queue`, a platform
//! shared-texture backend, and softblit's `Surface`. C# bindings are generated with
//! `diplomat-tool` (dotnet backend).
//!
//! ## Single-thread requirement
//!
//! `SoftblitSurface` wraps softblit's `Surface`, which is **not `Send`** (thread-affine GPU state).
//! Every call — `create`, `present`, resize, cursor — must happen on the one thread that owns the
//! instance. The C# control must marshal all access onto its owning thread.
//!
//! ## Dirty rects on the boundary
//!
//! The dotnet backend does not lower struct slices, so [`ffi::SoftblitSurface::present`] takes the
//! dirty regions as a flat `&[u32]` of `[x, y, w, h]` quads (length must be a multiple of 4)
//! instead of a slice of a rect struct.

mod imp;

use imp::{Backend, FfiError};
use softblit::{PixelFormat, ScalingMode};
use softblit_native::{SharedHandle, SyncKind};

impl From<ffi::PixelFormatFfi> for PixelFormat {
    fn from(f: ffi::PixelFormatFfi) -> Self {
        use ffi::PixelFormatFfi as F;
        match f {
            F::Rgba8 => PixelFormat::Rgba8,
            F::Bgra8 => PixelFormat::Bgra8,
            F::Rgbx8 => PixelFormat::Rgbx8,
            F::Bgrx8 => PixelFormat::Bgrx8,
            F::Rgb24 => PixelFormat::Rgb24,
            F::Bgr24 => PixelFormat::Bgr24,
            F::Rgb565 => PixelFormat::Rgb565,
            F::Rgb555 => PixelFormat::Rgb555,
            F::Gray8 => PixelFormat::Gray8,
            F::Gray16 => PixelFormat::Gray16,
            F::I420 => PixelFormat::I420,
        }
    }
}

impl From<ffi::ScalingModeFfi> for ScalingMode {
    fn from(s: ffi::ScalingModeFfi) -> Self {
        use ffi::ScalingModeFfi as S;
        match s {
            S::Fit => ScalingMode::Fit,
            S::Fill => ScalingMode::Fill,
            S::Stretch => ScalingMode::Stretch,
            S::Integer => ScalingMode::Integer,
            S::Native1x => ScalingMode::Native1x,
        }
    }
}

impl From<ffi::BackendFfi> for Backend {
    fn from(b: ffi::BackendFfi) -> Self {
        match b {
            ffi::BackendFfi::Vulkan => Backend::Vulkan,
            ffi::BackendFfi::D3D11 => Backend::D3D11,
            ffi::BackendFfi::D3D12 => Backend::D3D12,
        }
    }
}

impl From<FfiError> for ffi::ErrFfi {
    fn from(e: FfiError) -> Self {
        match e {
            FfiError::NoAdapter => ffi::ErrFfi::NoAdapter,
            FfiError::Unsupported => ffi::ErrFfi::Unsupported,
            FfiError::Platform => ffi::ErrFfi::Platform,
            FfiError::InvalidRect => ffi::ErrFfi::InvalidRect,
            FfiError::BufferSizeMismatch => ffi::ErrFfi::BufferSizeMismatch,
            FfiError::SurfaceLost => ffi::ErrFfi::SurfaceLost,
            FfiError::Device => ffi::ErrFfi::Device,
        }
    }
}

fn share_info_from(h: SharedHandle) -> ffi::ShareInfoFfi {
    let mut info = ffi::ShareInfoFfi {
        handle: h.handle,
        width: h.width,
        height: h.height,
        format: 0,
        sync_kind: ffi::SyncKindFfi::KeyedMutex,
        consumer_acquire_key: 0,
        consumer_release_key: 0,
        fence_handle: 0,
        memory_size: 0,
        render_finished_handle: 0,
        image_available_handle: 0,
    };
    match h.sync {
        SyncKind::KeyedMutex {
            consumer_acquire_key,
            consumer_release_key,
        } => {
            info.sync_kind = ffi::SyncKindFfi::KeyedMutex;
            info.consumer_acquire_key = consumer_acquire_key;
            info.consumer_release_key = consumer_release_key;
        }
        SyncKind::D3D12Fence { fence_handle } => {
            info.sync_kind = ffi::SyncKindFfi::D3D12Fence;
            info.fence_handle = fence_handle;
        }
        SyncKind::VulkanSemaphore {
            memory_size,
            render_finished_handle,
            image_available_handle,
        } => {
            info.sync_kind = ffi::SyncKindFfi::VulkanSemaphore;
            info.memory_size = memory_size;
            info.render_finished_handle = render_finished_handle;
            info.image_available_handle = image_available_handle;
        }
    }
    info
}

#[diplomat::bridge]
pub mod ffi {
    use crate::imp::SurfaceInner;
    use softblit::Rect;

    /// Source pixel format; mirrors `softblit::PixelFormat`.
    #[derive(Debug, PartialEq, Eq)]
    pub enum PixelFormatFfi {
        Rgba8,
        Bgra8,
        Rgbx8,
        Bgrx8,
        Rgb24,
        Bgr24,
        Rgb565,
        Rgb555,
        Gray8,
        Gray16,
        I420,
    }

    /// How the source maps onto the presentation surface; mirrors `softblit::ScalingMode`.
    #[derive(Debug, PartialEq, Eq)]
    pub enum ScalingModeFfi {
        Fit,
        Fill,
        Stretch,
        Integer,
        Native1x,
    }

    /// Platform sharing backend to allocate.
    #[derive(Debug, PartialEq, Eq)]
    pub enum BackendFfi {
        Vulkan,
        D3D11,
        D3D12,
    }

    /// Which cross-API sync primitive the consumer must drive; selects which fields of
    /// [`ShareInfoFfi`] are meaningful.
    #[derive(Debug, PartialEq, Eq)]
    pub enum SyncKindFfi {
        /// D3D11 keyed mutex: use `consumer_acquire_key` / `consumer_release_key`.
        KeyedMutex,
        /// Shared D3D12 fence (imported as a semaphore): use `fence_handle`.
        D3D12Fence,
        /// Exported Vulkan memory + two binary semaphores: use `memory_size`,
        /// `render_finished_handle`, `image_available_handle`. `handle` is the image memory handle.
        VulkanSemaphore,
    }

    /// Failure discriminator; the detailed cause is logged on the Rust side via `tracing`.
    #[derive(Debug, PartialEq, Eq)]
    #[diplomat::attr(auto, error)]
    pub enum ErrFfi {
        NoAdapter,
        Unsupported,
        Platform,
        InvalidRect,
        BufferSizeMismatch,
        SurfaceLost,
        Device,
    }

    /// Everything the consumer needs to import the shared texture. `handle` is the shared-texture
    /// handle (keyed-mutex/D3D12) or the exported image-memory NT handle (Vulkan). Branch on
    /// `sync_kind` for which of the remaining fields apply.
    #[derive(Debug)]
    pub struct ShareInfoFfi {
        pub handle: isize,
        pub width: u32,
        pub height: u32,
        /// 0 = BGRA8Unorm.
        pub format: u32,
        pub sync_kind: SyncKindFfi,
        pub consumer_acquire_key: u64,
        pub consumer_release_key: u64,
        pub fence_handle: isize,
        pub memory_size: u64,
        pub render_finished_handle: isize,
        pub image_available_handle: isize,
    }

    /// What one [`SoftblitSurface::present`] did; mirrors `softblit::PresentStats`.
    #[derive(Debug)]
    pub struct PresentStatsFfi {
        pub rects_uploaded: u32,
        pub bytes_uploaded: u64,
        pub skipped: bool,
    }

    /// A softblit presentation surface rendering into a GPU texture shared with a native
    /// compositor. Single-thread only (see the crate docs). Freed automatically when the C# wrapper
    /// is disposed.
    #[diplomat::opaque_mut]
    pub struct SoftblitSurface(SurfaceInner);

    impl SoftblitSurface {
        /// Creates the device + shared surface (default backend: Vulkan) and softblit's engine
        /// around it. `width`/`height` seed both the shared texture and the initial source size.
        pub fn create(
            width: u32,
            height: u32,
            format: PixelFormatFfi,
            scaling: ScalingModeFfi,
            backend: BackendFfi,
        ) -> Result<Box<SoftblitSurface>, ErrFfi> {
            let inner =
                SurfaceInner::create(width, height, format.into(), scaling.into(), backend.into())
                    .map_err(ErrFfi::from)?;
            Ok(Box::new(SoftblitSurface(inner)))
        }

        /// The import descriptor for the current shared texture. Re-fetch after any resize.
        pub fn share_info(&self) -> ShareInfoFfi {
            crate::share_info_from(self.0.share_handle())
        }

        /// Uploads dirty regions and blits into the shared texture, driving the producer sync.
        /// `dirty_rects` is a flat `[x, y, w, h]` u32 sequence; its length must be a multiple of 4.
        pub fn present(
            &mut self,
            bytes: &[u8],
            dirty_rects: &[u32],
        ) -> Result<PresentStatsFfi, ErrFfi> {
            if !dirty_rects.len().is_multiple_of(4) {
                return Err(ErrFfi::InvalidRect);
            }
            let rects: Vec<Rect> = dirty_rects
                .chunks_exact(4)
                .map(|q| Rect::new(q[0], q[1], q[2], q[3]))
                .collect();
            let stats = self.0.present(bytes, &rects).map_err(ErrFfi::from)?;
            Ok(PresentStatsFfi {
                rects_uploaded: stats.rects_uploaded,
                bytes_uploaded: stats.bytes_uploaded,
                skipped: stats.skipped,
            })
        }

        /// Resizes the source framebuffer (e.g. remote resolution change).
        pub fn resize_source(&mut self, width: u32, height: u32) {
            self.0.resize_source(width, height);
        }

        /// Reallocates the shared (target) texture; invalidates the previous [`ShareInfoFfi`], so
        /// re-fetch [`SoftblitSurface::share_info`] and re-import on the consumer side afterward.
        pub fn resize_target(&mut self, width: u32, height: u32) -> Result<(), ErrFfi> {
            self.0.resize_target(width, height).map_err(ErrFfi::from)
        }

        pub fn set_format(&mut self, format: PixelFormatFfi) {
            self.0.set_format(format.into());
        }

        pub fn set_scaling(&mut self, scaling: ScalingModeFfi) {
            self.0.set_scaling(scaling.into());
        }

        /// Installs/replaces the cursor overlay: RGBA8, straight alpha, tightly packed rows.
        pub fn set_cursor(&mut self, image: &[u8], width: u32, height: u32) {
            self.0.set_cursor(image, width, height);
        }

        pub fn clear_cursor(&mut self) {
            self.0.clear_cursor();
        }

        pub fn set_cursor_position(&mut self, x: i32, y: i32) {
            self.0.set_cursor_position(x, y);
        }
    }
}
