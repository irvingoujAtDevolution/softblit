/// Pixel format of the source framebuffer, as produced by the caller's decoder.
///
/// Three implementation classes, invisible to the caller:
///
/// - **Direct** formats have a matching GPU texture format; dirty rects are uploaded with
///   `write_texture` straight into the persistent texture.
/// - **Packed** formats (interleaved, 1–3 bytes/pixel) have no GPU texture equivalent; dirty
///   rects are uploaded raw into a storage buffer and unpacked into the persistent
///   `rgba8unorm` texture by a compute pass. No CPU repack happens (unless the adapter lacks
///   compute shaders — WebGL2 fallback — in which case rects are expanded on the CPU).
/// - **Planar** (I420): three planes in one storage buffer, color-converted in the unpack pass.
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
    /// 3 bytes/pixel, `R G B` byte order. Packed.
    Rgb24,
    /// 3 bytes/pixel, `B G R` byte order. Packed.
    Bgr24,
    /// 2 bytes/pixel, little-endian `RRRRRGGG GGGBBBBB` (5-6-5). Packed.
    Rgb565,
    /// 2 bytes/pixel, little-endian `XRRRRRGG GGGBBBBB` (1-5-5-5, top bit ignored). Packed.
    Rgb555,
    /// 1 byte/pixel grayscale, broadcast to RGB. Packed.
    Gray8,
    /// 2 bytes/pixel little-endian grayscale, normalized by 65535. Packed.
    Gray16,
    /// Planar YUV 4:2:0 (Y plane, then U, then V, each tightly packed; chroma dimensions are
    /// `ceil(w/2) x ceil(h/2)`). BT.601 limited-range color conversion in the unpack pass.
    /// Dirty rects are expanded outward to even coordinates.
    I420,
}

/// How a non-direct format is laid out and decoded by the unpack pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackedKind {
    /// One interleaved sample of `bytes_per_pixel` bytes per pixel.
    Interleaved { bytes_per_pixel: u32 },
    /// I420 three-plane layout.
    Planar420,
}

impl PixelFormat {
    /// Bytes per pixel for interleaved formats (`None` for planar I420, which has no single
    /// per-pixel byte count). Rows of interleaved formats are always tightly packed
    /// (`stride == width * bytes_per_pixel`).
    pub fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgba8 | Self::Bgra8 | Self::Rgbx8 | Self::Bgrx8 => Some(4),
            Self::Rgb24 | Self::Bgr24 => Some(3),
            Self::Rgb565 | Self::Rgb555 | Self::Gray16 => Some(2),
            Self::Gray8 => Some(1),
            Self::I420 => None,
        }
    }

    /// Total framebuffer byte length for a `width` x `height` source.
    pub fn frame_len(self, width: u32, height: u32) -> usize {
        let (w, h) = (width as usize, height as usize);
        match self {
            Self::I420 => {
                let chroma = w.div_ceil(2) * h.div_ceil(2);
                w * h + 2 * chroma
            }
            other => {
                w * h
                    * other
                        .bytes_per_pixel()
                        .expect("all non-I420 formats are interleaved")
            }
        }
    }

    /// Whether the alpha channel of the source is meaningless and must be forced to 1.
    pub(crate) fn force_opaque(self) -> bool {
        !matches!(self, Self::Rgba8 | Self::Bgra8)
    }

    /// `Some` for formats that go through the storage-buffer + compute-unpack path.
    pub(crate) fn packed_kind(self) -> Option<PackedKind> {
        match self {
            Self::Rgba8 | Self::Bgra8 | Self::Rgbx8 | Self::Bgrx8 => None,
            Self::Rgb24 | Self::Bgr24 => Some(PackedKind::Interleaved { bytes_per_pixel: 3 }),
            Self::Rgb565 | Self::Rgb555 | Self::Gray16 => {
                Some(PackedKind::Interleaved { bytes_per_pixel: 2 })
            }
            Self::Gray8 => Some(PackedKind::Interleaved { bytes_per_pixel: 1 }),
            Self::I420 => Some(PackedKind::Planar420),
        }
    }

    /// Format discriminant used by the unpack compute shader (and the CPU-expand fallback).
    pub(crate) fn shader_id(self) -> u32 {
        match self {
            Self::Rgb24 => 0,
            Self::Bgr24 => 1,
            Self::Rgb565 => 2,
            Self::Rgb555 => 3,
            Self::Gray8 => 4,
            Self::Gray16 => 5,
            Self::I420 => 6,
            _ => u32::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_len_interleaved() {
        assert_eq!(PixelFormat::Rgb24.frame_len(800, 500), 800 * 500 * 3);
        assert_eq!(PixelFormat::Gray8.frame_len(7, 3), 21);
        assert_eq!(PixelFormat::Rgb565.frame_len(4, 4), 32);
    }

    #[test]
    fn frame_len_i420() {
        // Even dimensions: w*h * 1.5.
        assert_eq!(PixelFormat::I420.frame_len(800, 500), 800 * 500 * 3 / 2);
        // Odd dimensions round chroma planes up.
        assert_eq!(PixelFormat::I420.frame_len(5, 3), 15 + 2 * (3 * 2));
    }
}
