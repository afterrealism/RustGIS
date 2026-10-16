//! Thinned copies of a layer's projected geometry, for drawing it zoomed out.
//!
//! A detailed layer seen whole puts many vertices in each pixel: walking them all on every frame
//! makes panning and zooming stall. A thinned copy keeps a vertex only when it is a set distance
//! from the last one kept, so the map draws from far fewer vertices and looks the same. Copies
//! are made per level (a tolerance of 2^level projection units) on first use and kept, a few
//! at a time; the renderer picks the level just under a third of a pixel at the view's zoom.

use rustgis_data::{Coord, Geometry};

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

/// A copy of `g` keeping, along each line and ring, only vertices at least `tol` (measured as
/// |dx| + |dy|) from the last vertex kept, and always each one's first and last. A ring that
/// would keep fewer than four vertices stays whole, so a small polygon still has an area to fill.
pub fn thin(g: &Geometry, tol: f64) -> Geometry {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => g.clone(),
        Geometry::Line(parts) => Geometry::Line(parts.iter().map(|p| thin_run(p, tol)).collect()),
        Geometry::Polygon(polys) => Geometry::Polygon(
            polys
                .iter()
                .map(|rings| {
                    rings
                        .iter()
                        .map(|r| {
                            let kept = thin_run(r, tol);
                            if kept.len() < 4 { r.clone() } else { kept }
                        })
                        .collect()
                })
                .collect(),
        ),
    }
}

/// The vertices of one line or ring kept at a tolerance. A point the projection couldn't show
/// (not finite) is kept, with the vertex after it, so the renderer still breaks the line there.
fn thin_run(run: &[Coord], tol: f64) -> Vec<Coord> {
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

    #[test]
    fn thinning_keeps_ends_and_far_vertices() {
        let run: Vec<Coord> = (0..=100).map(|i| Coord::new(i as f64 * 0.1, 0.0)).collect();
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run]), 1.0) else { panic!("a line") };
        let kept = &parts[0];
        assert_eq!(kept.first(), Some(&Coord::new(0.0, 0.0)));
        assert_eq!(kept.last(), Some(&Coord::new(10.0, 0.0)));
        assert!(kept.len() <= 12, "{} vertices kept", kept.len());
        assert!(kept.windows(2).all(|w| (w[1].x - w[0].x).abs() <= 1.0 + 1e-9));
    }

    #[test]
    fn thinning_breaks_at_unprojectable_points() {
        let run =
            vec![Coord::new(0.0, 0.0), Coord::new(0.01, 0.0), Coord::new(f64::NAN, f64::NAN), Coord::new(0.02, 0.0), Coord::new(5.0, 0.0)];
        let Geometry::Line(parts) = thin(&Geometry::Line(vec![run]), 1.0) else { panic!("a line") };
        let kept = &parts[0];
        assert_eq!(kept.len(), 4);
        assert!(kept[1].x.is_nan());
        assert_eq!(kept[2], Coord::new(0.02, 0.0));
    }

    #[test]
    fn a_small_ring_stays_whole() {
        let ring = vec![Coord::new(0.0, 0.0), Coord::new(0.1, 0.0), Coord::new(0.1, 0.1), Coord::new(0.0, 0.1), Coord::new(0.0, 0.0)];
        let g = Geometry::Polygon(vec![vec![ring.clone()]]);
        assert_eq!(thin(&g, 1.0), g);
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
