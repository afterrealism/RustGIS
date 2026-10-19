//! A static interval index over a layer's projected extents: rows sorted by extent min_x.
//! "What can intersect this view" becomes a binary search for the min_x cutoff plus a scan
//! of that prefix — instead of testing every feature on every frame. Built lazily per
//! (geometry revision, projection); the render loop's painter's order is row order, so
//! candidates are collected then sorted back ascending.

use rustgis_geom::Extent;

/// Layers with fewer features than this cull with the plain linear scan (index build isn't
/// worth it).
pub const CULL_INDEX_ABOVE: usize = 100_000;

/// Layers with **at most** this many features are eagerly projected when added, moving the
/// projection + cull-index cost from the first pan to load-time where a pause belongs.
/// Measured one-time cost (release, `lod_bench::bench_eager_warm`): 2–15 ms for point layers
/// up to the cap; ~120 ms for the many-vertex worst case (500k ten-vertex lines, 5 M
/// vertices). On wasm the same work runs on the browser's single main thread, several times
/// slower — there the cap is a tenth, keeping the worst-case pause imperceptible. Layers
/// over the cap stay lazy: their warm cost lands in the first render, which the background
/// settle worker already takes off the UI thread.
#[cfg(not(target_arch = "wasm32"))]
pub const EAGER_PROJECT_UP_TO: usize = 500_000;
#[cfg(target_arch = "wasm32")]
pub const EAGER_PROJECT_UP_TO: usize = 50_000;

/// Rows sorted by their projected extent's min_x, over a shared copy of the layer's extents.
#[derive(Clone, Debug)]
pub struct CullIndex {
    /// Row indices sorted by their extent's min_x; `min_xs[i]` belongs to `rows[i]`.
    rows: Vec<u32>,
    min_xs: Vec<f64>,
    /// The extents `rows` sorts by (an Arc clone of `Layer::projected_extents`).
    extents: std::sync::Arc<Vec<Extent>>,
}

impl CullIndex {
    /// The sort key for row `i`: its extent's `min_x`, with anything non-finite (NaN in
    /// either sign, or an infinity from an empty extent) mapped to `+INFINITY`.
    /// `total_cmp` alone is not enough: it orders a sign-bit-set NaN *before* `-inf`, which
    /// would put a NaN key at the front of `min_xs` and make `partition_point` cut the
    /// candidate list to nothing — every feature silently dropped. As `+INFINITY` a
    /// non-finite key sorts truly last, past every cutoff, where `intersects` rejects the
    /// row anyway; and `total_cmp` (not `partial_cmp`) keeps the comparator a total order,
    /// which `sort_by` may require since Rust 1.81.
    fn key(extents: &[Extent], i: u32) -> f64 {
        match extents.get(i as usize).map(|e| e.min_x) {
            Some(x) if x.is_finite() => x,
            _ => f64::INFINITY,
        }
    }

    /// Indexes `extents` (row for row with the layer's features) by ascending min_x.
    pub fn build(extents: &std::sync::Arc<Vec<Extent>>) -> CullIndex {
        let mut order: Vec<u32> = (0..extents.len() as u32).collect();
        order.sort_by(|&a, &b| Self::key(extents, a).total_cmp(&Self::key(extents, b)));
        let min_xs = order.iter().map(|&r| Self::key(extents, r)).collect();
        CullIndex { rows: order, min_xs, extents: extents.clone() }
    }

    /// Rows whose extent can intersect `e`, ascending. Empty extents never match, exactly
    /// like `Extent::intersects` on them (`Geometry::extent` skips non-finite coordinates,
    /// so min_x is finite or the extent is `Extent::EMPTY` and sorts past the cutoff).
    pub fn candidates(&self, e: &Extent, out: &mut Vec<u32>) {
        out.clear();
        // First index whose min_x exceeds e.max_x: everything before it starts left of the
        // view's right edge, everything at or past it fails `min_x <= e.max_x`.
        let hi = self.min_xs.partition_point(|&x| x <= e.max_x);
        let Some(rows) = self.rows.get(..hi) else { return };
        for &r in rows {
            if self.extents.get(r as usize).is_some_and(|x| x.intersects(e)) {
                out.push(r);
            }
        }
        out.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(x0: f64, y0: f64, x1: f64, y1: f64) -> Extent {
        Extent { min_x: x0, min_y: y0, max_x: x1, max_y: y1 }
    }

    #[test]
    fn candidates_match_a_linear_scan() {
        let extents = std::sync::Arc::new(vec![
            ext(0.0, 0.0, 1.0, 1.0),   // 0: left
            ext(5.0, 5.0, 6.0, 6.0),   // 1: middle, y-miss
            ext(5.5, 0.0, 6.5, 1.0),   // 2: hit
            ext(20.0, 0.0, 21.0, 1.0), // 3: right of view
            Extent::EMPTY,             // 4: empty never hits
        ]);
        let idx = CullIndex::build(&extents);
        for view in [ext(4.0, -1.0, 10.0, 2.0), ext(-100.0, -100.0, 100.0, 100.0), ext(50.0, 50.0, 60.0, 60.0)] {
            let mut got = Vec::new();
            idx.candidates(&view, &mut got);
            let want: Vec<u32> = (0..extents.len() as u32).filter(|&r| extents[r as usize].intersects(&view)).collect();
            assert_eq!(got, want, "view {view:?}");
        }
    }

    #[test]
    fn candidates_come_in_row_order() {
        let extents = std::sync::Arc::new(vec![ext(10.0, 0.0, 11.0, 1.0), ext(0.0, 0.0, 1.0, 1.0), ext(5.0, 0.0, 6.0, 1.0)]);
        let idx = CullIndex::build(&extents);
        let mut got = Vec::new();
        idx.candidates(&ext(-1.0, -1.0, 20.0, 2.0), &mut got);
        assert_eq!(got, vec![0, 1, 2], "ascending row order, not min_x order");
    }

    #[test]
    fn a_nan_extent_neither_panics_nor_drops_the_finite_rows() {
        // A NaN min_x (either sign) must sort last, not first: at the front it makes the
        // partition cutoff zero and silently drops every feature. The NaN row itself never
        // matches — `intersects` rejects it, exactly as for an empty extent.
        let nan = Extent { min_x: f64::NAN, min_y: f64::NAN, max_x: f64::NAN, max_y: f64::NAN };
        let neg_nan = Extent { min_x: -f64::NAN, min_y: -f64::NAN, max_x: -f64::NAN, max_y: -f64::NAN };
        let extents = std::sync::Arc::new(vec![nan, ext(0.0, 0.0, 1.0, 1.0), neg_nan, ext(5.0, 0.0, 6.0, 1.0)]);
        let idx = CullIndex::build(&extents);
        let mut got = Vec::new();
        idx.candidates(&ext(-1.0, -1.0, 20.0, 2.0), &mut got);
        assert_eq!(got, vec![1, 3], "the finite rows survive; the NaN rows never match");
    }
}
