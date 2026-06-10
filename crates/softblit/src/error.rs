use core::fmt;

use crate::rect::Rect;

#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// No WebGPU adapter/device is available. The caller should fall back (e.g. Canvas2D) or
    /// report the platform as unsupported.
    WebGpuUnavailable { reason: String },
    /// The presentation surface was lost and could not be restored by reconfiguring.
    /// Recreate the [`crate::Surface`] (browser context-loss semantics; no silent auto-recovery).
    SurfaceLost,
    /// A dirty rect lies outside the source framebuffer.
    InvalidRect { rect: Rect, bounds: (u32, u32) },
    /// The byte slice passed to [`crate::Surface::present_external`] does not match
    /// `width * height * bytes_per_pixel` for the current source size and format.
    BufferSizeMismatch { expected: usize, actual: usize },
    /// An unrecoverable device-side error.
    Device { reason: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebGpuUnavailable { reason } => write!(f, "WebGPU is unavailable: {reason}"),
            Self::SurfaceLost => write!(f, "presentation surface lost; recreate the Surface"),
            Self::InvalidRect { rect, bounds } => write!(
                f,
                "dirty rect {}x{}@({},{}) exceeds framebuffer bounds {}x{}",
                rect.width, rect.height, rect.x, rect.y, bounds.0, bounds.1
            ),
            Self::BufferSizeMismatch { expected, actual } => write!(
                f,
                "framebuffer byte size mismatch: expected {expected}, got {actual}"
            ),
            Self::Device { reason } => write!(f, "device error: {reason}"),
        }
    }
}

impl core::error::Error for Error {}
