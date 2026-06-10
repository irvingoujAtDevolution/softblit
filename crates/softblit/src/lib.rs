//! # softblit — format-aware framebuffer presentation for WebGPU
//!
//! Presents CPU-produced framebuffers (remote desktop clients, emulators, software renderers)
//! to a canvas with the minimum possible number of copies:
//!
//! | Step | CPU copies |
//! |---|---|
//! | Decoder → framebuffer | 0 (decode in place) |
//! | Framebuffer → GPU staging (`write_texture` / `write_buffer`) | 1 (platform floor on wasm) |
//! | GPU staging → texture → swapchain | 0 |
//!
//! Wire formats with no GPU texture equivalent (RGB24 / BGR24 — e.g. IronVNC's RGB8
//! framebuffer) are uploaded raw and unpacked by a compute pass; no CPU repack ever happens.
//! Dirty rects are accumulated, coalesced, and uploaded individually against a persistent
//! texture; clean regions are never re-uploaded.
//!
//! The GPU-facing API ([`Surface`]) is only available on `wasm32` in this version; the
//! format/rect/scaling core is platform-independent.

mod error;
mod format;
mod rect;
mod scaling;

#[cfg(target_arch = "wasm32")]
mod gpu;
#[cfg(target_arch = "wasm32")]
mod surface;

pub use error::Error;
pub use format::PixelFormat;
pub use rect::Rect;
pub use scaling::ScalingMode;
#[cfg(target_arch = "wasm32")]
pub use surface::{FrameMut, Surface, SurfaceTarget};

/// Initial configuration for a [`Surface`].
#[derive(Clone, Copy, Debug)]
pub struct SurfaceDescriptor {
    /// Source framebuffer size in pixels (e.g. the remote desktop resolution).
    pub source_size: (u32, u32),
    /// Source pixel format.
    pub format: PixelFormat,
    /// How the source maps onto the presentation surface.
    pub scaling: ScalingMode,
}

/// What one `present` call did. This library exists because of benchmarking; the numbers are
/// free.
#[derive(Clone, Copy, Debug, Default)]
pub struct PresentStats {
    /// Dirty rects uploaded after coalescing.
    pub rects_uploaded: u32,
    /// Bytes copied from the framebuffer toward the GPU.
    pub bytes_uploaded: u64,
    /// True when there was nothing to upload and no swapchain damage: the call was a no-op.
    pub skipped: bool,
}
