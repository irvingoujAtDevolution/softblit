/// How the source framebuffer maps onto the presentation surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalingMode {
    /// Letterbox: preserve aspect ratio, fit entirely within the surface, linear filtering.
    Fit,
    /// Crop: preserve aspect ratio, cover the entire surface, linear filtering.
    Fill,
    /// Ignore aspect ratio, fill the surface exactly, linear filtering.
    Stretch,
    /// Integer multiples only, nearest filtering, centered (emulators).
    /// Uses the largest integer scale that fits, with a minimum of 1x (cropping if needed).
    Integer,
    /// 1:1 pixels, centered, nearest filtering (cropping if the source is larger).
    Native1x,
}

impl ScalingMode {
    /// Nearest-neighbour modes must never interpolate.
    pub(crate) fn filter_linear(self) -> bool {
        match self {
            Self::Fit | Self::Fill | Self::Stretch => true,
            Self::Integer | Self::Native1x => false,
        }
    }

    /// NDC half-extents of the destination quad: the quad spans `[-sx, sx] x [-sy, sy]`,
    /// centered. Values > 1 crop (Fill / oversized Native1x).
    pub(crate) fn ndc_scale(self, source: (u32, u32), target: (u32, u32)) -> (f32, f32) {
        let (sw, sh) = (source.0.max(1) as f32, source.1.max(1) as f32);
        let (tw, th) = (target.0.max(1) as f32, target.1.max(1) as f32);

        let (dest_w, dest_h) = match self {
            Self::Stretch => (tw, th),
            Self::Fit => {
                let r = (tw / sw).min(th / sh);
                (sw * r, sh * r)
            }
            Self::Fill => {
                let r = (tw / sw).max(th / sh);
                (sw * r, sh * r)
            }
            Self::Integer => {
                let k = ((tw / sw).min(th / sh)).floor().max(1.0);
                (sw * k, sh * k)
            }
            Self::Native1x => (sw, sh),
        };

        (dest_w / tw, dest_h / th)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5
    }

    #[test]
    fn stretch_covers_surface() {
        assert!(close(
            ScalingMode::Stretch.ndc_scale((800, 500), (1024, 640)),
            (1.0, 1.0)
        ));
    }

    #[test]
    fn fit_letterboxes_wide_target() {
        // 100x100 source on 200x100 target: height-limited, half-width quad.
        assert!(close(
            ScalingMode::Fit.ndc_scale((100, 100), (200, 100)),
            (0.5, 1.0)
        ));
    }

    #[test]
    fn fill_crops_wide_target() {
        // Same shapes under Fill: width-limited, quad overflows vertically.
        assert!(close(
            ScalingMode::Fill.ndc_scale((100, 100), (200, 100)),
            (1.0, 2.0)
        ));
    }

    #[test]
    fn integer_floors_scale() {
        // 100x100 in 250x230 -> 2x -> 200x230ths by 200x230ths.
        assert!(close(
            ScalingMode::Integer.ndc_scale((100, 100), (250, 230)),
            (200.0 / 250.0, 200.0 / 230.0)
        ));
    }

    #[test]
    fn integer_minimum_is_one() {
        // Source larger than target still renders at 1x (cropped).
        assert!(close(
            ScalingMode::Integer.ndc_scale((400, 400), (200, 200)),
            (2.0, 2.0)
        ));
    }

    #[test]
    fn native_1x_is_exact() {
        assert!(close(
            ScalingMode::Native1x.ndc_scale((800, 500), (1600, 1000)),
            (0.5, 0.5)
        ));
    }
}
