/// An axis-aligned rectangle in source-framebuffer pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub(crate) fn right(self) -> u64 {
        u64::from(self.x) + u64::from(self.width)
    }

    pub(crate) fn bottom(self) -> u64 {
        u64::from(self.y) + u64::from(self.height)
    }

    pub(crate) fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Whether `self` fits entirely within a `bounds_width` x `bounds_height` framebuffer.
    pub(crate) fn within(self, bounds_width: u32, bounds_height: u32) -> bool {
        self.right() <= u64::from(bounds_width) && self.bottom() <= u64::from(bounds_height)
    }

    /// Clips to a `bounds_width` x `bounds_height` framebuffer. Returns `None` if nothing remains.
    pub(crate) fn clipped(self, bounds_width: u32, bounds_height: u32) -> Option<Self> {
        if self.is_empty() || self.x >= bounds_width || self.y >= bounds_height {
            return None;
        }
        let width = self.width.min(bounds_width - self.x);
        let height = self.height.min(bounds_height - self.y);
        Some(Self {
            width,
            height,
            ..self
        })
    }

    pub(crate) fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x,
            y,
            width: u32::try_from(right - u64::from(x)).expect("union of in-bounds rects fits u32"),
            height: u32::try_from(bottom - u64::from(y))
                .expect("union of in-bounds rects fits u32"),
        }
    }

    /// Whether the two rects overlap or share an edge (merging them loses nothing but
    /// interior-free corner area).
    pub(crate) fn overlaps_or_touches(self, other: Self) -> bool {
        u64::from(self.x) <= other.right()
            && u64::from(other.x) <= self.right()
            && u64::from(self.y) <= other.bottom()
            && u64::from(other.y) <= self.bottom()
    }
}

/// If more than this many rects survive merging, collapse to the bounding box: each upload has a
/// fixed JS-boundary cost on wasm, so long tail-lists of small rects lose to one merged upload.
pub(crate) const MAX_RECTS: usize = 64;

/// If the merged rects cover more than this fraction of their common bounding box, upload the
/// bounding box instead.
pub(crate) const DENSITY_THRESHOLD: f64 = 0.8;

/// Clips `rects` to the framebuffer bounds and coalesces them: overlapping/touching rects are
/// merged to fixpoint, then the list collapses to its bounding box if it is dense or too long.
///
/// The result is a list of disjoint (non-touching) rects, each within bounds.
pub(crate) fn coalesce(rects: &[Rect], bounds_width: u32, bounds_height: u32) -> Vec<Rect> {
    let mut merged: Vec<Rect> = Vec::with_capacity(rects.len().min(MAX_RECTS + 1));

    for rect in rects {
        let Some(mut current) = rect.clipped(bounds_width, bounds_height) else {
            continue;
        };
        // Merging can create a rect that now touches earlier entries, so re-scan until stable.
        loop {
            let mut absorbed = false;
            merged.retain(|existing| {
                if existing.overlaps_or_touches(current) {
                    current = current.union(*existing);
                    absorbed = true;
                    false
                } else {
                    true
                }
            });
            if !absorbed {
                break;
            }
        }
        merged.push(current);
    }

    if merged.len() <= 1 {
        return merged;
    }

    let bbox = merged.iter().copied().reduce(Rect::union).expect("len > 1");
    let covered: u64 = merged.iter().map(|r| r.area()).sum();
    if merged.len() > MAX_RECTS || covered as f64 / bbox.area() as f64 > DENSITY_THRESHOLD {
        return vec![bbox];
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_drops_out_of_bounds() {
        assert_eq!(Rect::new(100, 0, 10, 10).clipped(50, 50), None);
        assert_eq!(Rect::new(0, 0, 0, 10).clipped(50, 50), None);
    }

    #[test]
    fn clip_truncates_overhang() {
        assert_eq!(
            Rect::new(40, 45, 20, 20).clipped(50, 50),
            Some(Rect::new(40, 45, 10, 5))
        );
    }

    #[test]
    fn disjoint_rects_stay_separate() {
        let out = coalesce(
            &[Rect::new(0, 0, 10, 10), Rect::new(20, 20, 10, 10)],
            100,
            100,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn overlapping_rects_merge() {
        let out = coalesce(
            &[Rect::new(0, 0, 10, 10), Rect::new(5, 5, 10, 10)],
            100,
            100,
        );
        assert_eq!(out, vec![Rect::new(0, 0, 15, 15)]);
    }

    #[test]
    fn touching_rects_merge() {
        let out = coalesce(
            &[Rect::new(0, 0, 10, 10), Rect::new(10, 0, 10, 10)],
            100,
            100,
        );
        assert_eq!(out, vec![Rect::new(0, 0, 20, 10)]);
    }

    #[test]
    fn chain_merge_reaches_fixpoint() {
        // Third rect bridges the first two.
        let out = coalesce(
            &[
                Rect::new(0, 0, 10, 10),
                Rect::new(30, 0, 10, 10),
                Rect::new(10, 0, 20, 10),
            ],
            100,
            100,
        );
        assert_eq!(out, vec![Rect::new(0, 0, 40, 10)]);
    }

    #[test]
    fn dense_cluster_collapses_to_bbox() {
        // Two rects covering > 80% of their bounding box but not touching.
        let out = coalesce(
            &[Rect::new(0, 0, 100, 49), Rect::new(0, 51, 100, 49)],
            200,
            200,
        );
        assert_eq!(out, vec![Rect::new(0, 0, 100, 100)]);
    }

    #[test]
    fn sparse_corners_do_not_collapse() {
        let out = coalesce(
            &[Rect::new(0, 0, 10, 10), Rect::new(190, 190, 10, 10)],
            200,
            200,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn too_many_rects_collapse_to_bbox() {
        // 65 disjoint 1x1 rects spaced out on one row.
        let rects: Vec<Rect> = (0..65).map(|i| Rect::new(i * 3, 0, 1, 1)).collect();
        let out = coalesce(&rects, 1000, 1000);
        assert_eq!(out, vec![Rect::new(0, 0, 64 * 3 + 1, 1)]);
    }
}
