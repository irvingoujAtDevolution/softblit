/// Pixel format of the source framebuffer, as produced by the caller's decoder.
///
/// Two implementation classes, invisible to the caller:
///
/// - **Direct** formats have a matching GPU texture format; dirty rects are uploaded with
///   `write_texture` straight into the persistent texture.
/// - **Packed** formats (3 bytes/pixel) have no GPU texture equivalent; dirty rects are uploaded
///   raw into a storage buffer and unpacked into the persistent `rgba8unorm` texture by a compute
///   pass. No CPU repack ever happens.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// 4 bytes/pixel, `R G B A` byte order. Direct (`rgba8unorm`).
    Rgba8,
    /// 4 bytes/pixel, `B G R A` byte order. Direct (`bgra8unorm`).
    Bgra8,
    /// 4 bytes/pixel, `R G B X` byte order; X is ignored and alpha is forced to 1 in the blit.
    Rgbx8,
    /// 4 bytes/pixel, `B G R X` byte order; X is ignored and alpha is forced to 1 in the blit.
    Bgrx8,
    /// 3 bytes/pixel, `R G B` byte order. Packed (storage buffer + compute unpack).
    Rgb24,
    /// 3 bytes/pixel, `B G R` byte order. Packed (storage buffer + compute unpack).
    Bgr24,
}

impl PixelFormat {
    /// Bytes per pixel in the source framebuffer. Rows are always tightly packed
    /// (`stride == width * bytes_per_pixel`).
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 | Self::Rgbx8 | Self::Bgrx8 => 4,
            Self::Rgb24 | Self::Bgr24 => 3,
        }
    }

    /// Whether the alpha channel of the source is meaningless and must be forced to 1.
    pub(crate) fn force_opaque(self) -> bool {
        match self {
            Self::Rgba8 | Self::Bgra8 => false,
            Self::Rgbx8 | Self::Bgrx8 | Self::Rgb24 | Self::Bgr24 => true,
        }
    }

    /// Whether this format goes through the storage-buffer + compute-unpack path.
    pub(crate) fn is_packed(self) -> bool {
        matches!(self, Self::Rgb24 | Self::Bgr24)
    }

    /// For packed formats: whether byte order is B G R (swizzle in the unpack shader).
    pub(crate) fn packed_is_bgr(self) -> bool {
        matches!(self, Self::Bgr24)
    }
}
