//! Thinned copies of a layer's projected geometry, for drawing it zoomed out.
//!
//! A detailed layer seen whole puts many vertices in each pixel: walking them all on every frame
//! makes panning and zooming stall. A thinned copy drops the vertices that matter least to the
//! shape — Visvalingam–Whyatt: repeat removal of the vertex whose triangle with its neighbours
//! has the smallest area, until every remaining triangle clears an area threshold — so the map
//! draws from far fewer vertices and looks the same. Copies are made per level (a tolerance of
//! 2^level projection units) on first use and kept, a few at a time; the renderer picks the
//! level just under a third of a pixel at the view's zoom.

use rustgis_data::{Coord, Geometry};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// The tolerance, in device pixels, a thinned copy keeps within.
pub const THIN_PX: f64 = 0.3;

/// Layers with fewer line and ring vertices than this are always drawn from the full geometry.
pub const THIN_ABOVE: usize = 200_000;

/// A level is kept as a copy only when it drops at least this share of the vertices; otherwise
/// it draws from the full geometry (zoomed in past the vertex spacing, or many tiny features).
pub const MIN_SAVING: f64 = 0.25;

/// How many levels a layer keeps.
pub const LEVELS_KEPT: usize = 3;

/// The level for a tolerance in projection units: the largest whose tolerance (2^level) is at
/// most `tol`. `None` for a tolerance that is not a positive number.
pub fn level(tol: f64) -> Option<i32> {
    (tol.is_finite() && tol > 0.0).then(|| tol.log2().floor().clamp(-60.0, 60.0) as i32)
}

/// A level's tolerance in projection units.
pub fn tolerance(level: i32) -> f64 {
    2f64.powi(level)
}

/// A copy of `g` thinned by Visvalingam–Whyatt (see [`thin_run_vw`]) at a tolerance of `tol`
/// projection units: vertices whose removal changes the shape by less than the tolerance's
/// area scale are dropped. Every line and ring keeps its first and last vertex, and a ring
/// keeps at least four vertices, so a small polygon still has an area to fill. A point the
/// projection couldn't show (not finite) is pinned, so the renderer still breaks the line there.
pub fn thin(g: &Geometry, tol: f64) -> Geometry {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => g.clone(),
        Geometry::Line(parts) => Geometry::Line(parts.iter().map(|p| thin_run(p, tol, false)).collect()),
        Geometry::Polygon(polys) => {
            Geometry::Polygon(polys.iter().map(|rings| rings.iter().map(|r| thin_run(r, tol, true)).collect()).collect())
        }
    }
}

/// The area threshold for a distance tolerance: a vertex whose perpendicular deviation from
/// its neighbours' chord is under ~`tol` on a ~`tol`-scale segment has triangle area < tol².
/// One named helper so the mapping can be recalibrated (the plan's risk table: `0.5 * tol * tol`).
fn area_tol(tol: f64) -> f64 {
    tol * tol
}

/// Twice the area of the triangle a-b-c; NaN when any point is non-finite.
fn twice_area(a: &Coord, b: &Coord, c: &Coord) -> f64 {
    ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs()
}

/// The vertices of one line or ring kept at a tolerance, stopping at 4 vertices for a ring
/// (a fillable area) and 2 for a line.
fn thin_run(run: &[Coord], tol: f64, closed: bool) -> Vec<Coord> {
    thin_run_vw(run, area_tol(tol), if closed { 4 } else { 2 })
}

/// Visvalingam–Whyatt: repeatedly remove the vertex with the smallest effective area until
/// every remaining triangle clears `min_area` or the run hits `floor` vertices. Ends are
/// pinned, and non-finite vertices are pinned: their triangles' areas are NaN, and NaN
/// comparisons are false, so the heap-pop order alone would not spare them — the explicit
/// `is_finite` guard in the break condition does (review doc §1.3). O(n log n) via a min-heap
/// with lazy deletion: a popped entry whose stored bits differ from the vertex's current area
/// bits is stale (superseded by a re-push), and `removed` covers a re-push whose area came out
/// unchanged. Rebuilt in original order by filtering on `removed`, so closed rings (first ==
/// last by value) survive intact.
fn thin_run_vw(run: &[Coord], min_area: f64, floor: usize) -> Vec<Coord> {
    let n = run.len();
    if n <= floor.max(3) {
        return run.to_vec();
    }
    let mut prev: Vec<usize> = (0..n).map(|i| i.wrapping_sub(1)).collect();
    let mut next: Vec<usize> = (0..n).map(|i| i + 1).collect();
    let mut area = vec![f64::INFINITY; n]; // ends stay pinned at Infinity
    let mut removed = vec![false; n];
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
    let calc = |prev: &[usize], next: &[usize], area: &mut [f64], heap: &mut BinaryHeap<Reverse<(u64, usize)>>, i: usize| {
        let (p, x) = (prev[i], next[i]);
        if p == usize::MAX || x >= run.len() {
            return; // an end: stays pinned at Infinity
        }
        area[i] = twice_area(&run[p], &run[i], &run[x]);
        heap.push(Reverse((area[i].to_bits(), i)));
    };
    for i in 1..n.saturating_sub(1) {
        calc(&prev, &next, &mut area, &mut heap, i);
    }
    let mut live = n;
    while let Some(Reverse((bits, i))) = heap.pop() {
        if removed[i] || bits != area[i].to_bits() {
            continue; // already gone, or stale: superseded by a re-push
        }
        if !area[i].is_finite() || area[i] >= min_area || live <= floor {
            break; // NaN/inf is pinned, the heap minimum clears the threshold, or the floor is hit
        }
        let (p, x) = (prev[i], next[i]);
        removed[i] = true;
        if p != usize::MAX {
            next[p] = x;
        }
        if x < n {
            prev[x] = p;
        }
        live -= 1;
        if p != usize::MAX && p != 0 {
            calc(&prev, &next, &mut area, &mut heap, p);
        }
        if x < n && x != n - 1 {
            calc(&prev, &next, &mut area, &mut heap, x);
        }
    }
    run.iter().zip(&removed).filter_map(|(c, r)| (!r).then_some(*c)).collect()
}

/// The number of vertices thinning can drop: those of its lines and rings (points are drawn
/// as they are).
pub fn thinnable(g: &Geometry) -> usize {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => 0,
        _ => vertices(g),
    }
}

/// The number of vertices in a geometry.
pub fn vertices(g: &Geometry) -> usize {
    match g {
        Geometry::Point(_) => 1,
        Geometry::MultiPoint(p) => p.len(),
        Geometry::Line(parts) => parts.iter().map(Vec::len).sum(),
        Geometry::Polygon(polys) => polys.iter().flatten().map(Vec::len).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-VW greedy rule (keep a vertex when |dx|+|dy| >= tol from the last vertex kept),
    /// kept as the comparison baseline for the budget-parity test below.
    fn greedy_run(run: &[Coord], tol: f64) -> Vec<Coord> {
        let mut out = Vec::new();
        let mut last: Option<Coord> = None;
        let n = run.len();
        for (i, c) in run.iter().enumerate() {
            let finite = c.x.is_finite() && c.y.is_finite();
            let keep = match last {
                Some(l) if finite && i + 1 != n => (c.x - l.x).abs() + (c.y - l.y).abs() >= tol,
                _ => true,
            };
            if keep {
                out.push(*c);
                last = finite.then_some(*c);
            }
        }
        out
    }

    #[test]
    fn vw_keeps_a_sharp_corner_greedy_drops_at_the_same_budget() {
        // A sharp corner in a dense collinear run: at the same kept-vertex budget VW keeps the
        // corner and drops collinear points, the greedy rule the opposite (review doc §1.4).
        let corner = Coord::new(10.5, 1.0);
        let mut run: Vec<Coord> = (0..=20).map(|i| Coord::new(i as f64, 0.0)).collect();
        run.insert(11, corner);
        // area_tol of FRAC_1_SQRT_2 is 0.5; the corner's triangle with its neighbours (1.0) clears it.
        let tol = std::f64::consts::FRAC_1_SQRT_2;
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run.clone()]), tol) else { panic!("a line") };
        let vw = &parts[0];
        assert!(vw.contains(&corner), "VW keeps the corner: {vw:?}");
        let budget = vw.len();
        let greedy = greedy_run(&run, 6.0);
        assert_eq!(greedy.len(), budget, "greedy reaches the same budget of {budget}: {greedy:?}");
        assert!(!greedy.contains(&corner), "greedy at that budget drops the corner");
    }

    #[test]
    fn vw_thins_a_closed_ring_to_its_floor_and_keeps_it_closed() {
        // RustGIS rings close by repeating the first coordinate (first == last by value).
        let mut ring: Vec<Coord> = (0..100)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 100.0;
                Coord::new(a.cos(), a.sin())
            })
            .collect();
        ring.push(ring[0]);
        let Geometry::Polygon(polys) = thin(&Geometry::Polygon(vec![vec![ring.clone()]]), 10.0) else { panic!("a polygon") };
        let out = &polys[0][0];
        assert!(out.len() >= 4, "ring floor: {}", out.len());
        assert!(out.len() < ring.len(), "VW thins where the greedy rule kept the ring whole: {}", out.len());
        assert_eq!(out.first(), Some(&ring[0]));
        assert_eq!(out.last(), Some(&ring[0]), "still closed: first == last");
    }

    #[test]
    fn vw_keeps_ends_and_pins_unprojectable_points() {
        let run =
            vec![Coord::new(0.0, 0.0), Coord::new(0.01, 0.0), Coord::new(f64::NAN, f64::NAN), Coord::new(0.02, 0.0), Coord::new(5.0, 0.0)];
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run]), 1.0) else { panic!("a line") };
        assert_eq!(parts[0].first(), Some(&Coord::new(0.0, 0.0)));
        assert_eq!(parts[0].last(), Some(&Coord::new(5.0, 0.0)));
        assert!(parts[0].iter().any(|c| c.x.is_nan()), "the break point is pinned");
    }

    #[test]
    fn vw_ring_floor_keeps_four_vertices() {
        // A dense ring never thins below 4 vertices (a fillable area), matching the old rule.
        let ring: Vec<Coord> = (0..100)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 100.0;
                Coord::new(a.cos(), a.sin())
            })
            .collect();
        let Geometry::Polygon(polys) = thin(&Geometry::Polygon(vec![vec![ring]]), 10.0) else { panic!("a polygon") };
        assert!(polys[0][0].len() >= 4, "ring floor: {}", polys[0][0].len());
    }

    #[test]
    fn vw_monotone_in_tolerance() {
        let run: Vec<Coord> = (0..=200).map(|i| Coord::new(i as f64 * 0.1, (i as f64 * 0.35).sin())).collect();
        let count = |tol| match thin(&Geometry::Line(vec![run.clone()]), tol) {
            Geometry::Line(p) => p[0].len(),
            _ => 0,
        };
        assert!(count(0.01) >= count(0.5) && count(0.5) >= count(5.0));
    }

    #[test]
    fn a_collinear_run_thins_to_its_ends() {
        // Collinear interior points carry no area: VW drops them at any tolerance, keeping
        // exactly the ends. (The greedy rule kept one vertex every tol, never over ~tol apart.)
        let run: Vec<Coord> = (0..=100).map(|i| Coord::new(i as f64 * 0.1, 0.0)).collect();
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run]), 1.0) else { panic!("a line") };
        let kept = &parts[0];
        assert_eq!(kept.first(), Some(&Coord::new(0.0, 0.0)));
        assert_eq!(kept.last(), Some(&Coord::new(10.0, 0.0)));
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn thinning_breaks_at_unprojectable_points() {
        // VW pins a non-finite vertex and both its neighbours (their triangles are NaN), so the
        // renderer still breaks the line there — one vertex more than the greedy rule kept.
        let run =
            vec![Coord::new(0.0, 0.0), Coord::new(0.01, 0.0), Coord::new(f64::NAN, f64::NAN), Coord::new(0.02, 0.0), Coord::new(5.0, 0.0)];
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run]), 1.0) else { panic!("a line") };
        let kept = &parts[0];
        assert_eq!(kept.len(), 5);
        assert!(kept[2].x.is_nan());
        assert_eq!(kept[1], Coord::new(0.01, 0.0));
        assert_eq!(kept[3], Coord::new(0.02, 0.0));
    }

    #[test]
    fn a_small_ring_stays_whole() {
        // A 5-vertex closed ring at tol 1.0 thins to VW's 4-vertex floor (a fillable area) with
        // first and last preserved — not the unchanged clone the greedy rule kept.
        let ring = vec![Coord::new(0.0, 0.0), Coord::new(0.1, 0.0), Coord::new(0.1, 0.1), Coord::new(0.0, 0.1), Coord::new(0.0, 0.0)];
        let g = Geometry::Polygon(vec![vec![ring.clone()]]);
        let Geometry::Polygon(polys) = thin(&g, 1.0) else { panic!("a polygon") };
        let out = &polys[0][0];
        assert!(out.len() >= 4, "ring floor: {}", out.len());
        assert_eq!(out.first(), Some(&ring[0]));
        assert_eq!(out.last(), Some(&ring[0]));
        assert_eq!(thinnable(&Geometry::MultiPoint(ring)), 0);
    }

    #[test]
    fn levels_are_powers_of_two_under_the_tolerance() {
        assert_eq!(level(1.0), Some(0));
        assert_eq!(level(3.9), Some(1));
        assert_eq!(level(0.3), Some(-2));
        assert!(tolerance(level(1234.5).unwrap_or(0)) <= 1234.5);
        assert_eq!(level(0.0), None);
        assert_eq!(level(f64::NAN), None);
        assert_eq!(level(-1.0), None);
    }
}
