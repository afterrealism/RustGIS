//! Lyon tessellation of layer geometry into GPU meshes, matching the tiny-skia CPU path's
//! rules: EvenOdd holes, round joins and caps, the 0.35 physical-pixel decimation of
//! `Ctx::ring_path`, and screen-constant stroke widths — strokes tessellate at width 1.0 in
//! map units and the vertex shader displaces along lyon's unit normal by
//! `stroke_px * map_per_px / 2`, so one tessellation serves every zoom.

use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, LineCap, LineJoin, StrokeOptions, StrokeTessellator, VertexBuffers,
};
use rustgis_data::Geometry;
use rustgis_geom::{Coord, Extent};
use rustgis_map::{Layer, Rgba, Symbol};

use crate::SELECTION;
use crate::gpu_scene::{DrawCmd, MeshVertex, PointInstance};

/// Premultiplied 0..1 RGBA for the GPU (tiny-skia stores premultiplied; wgpu blends the same way).
pub(crate) fn premul(c: Rgba) -> [f32; 4] {
    let a = c.a() as f32 / 255.0;
    [c.r() as f32 / 255.0 * a, c.g() as f32 / 255.0 * a, c.b() as f32 / 255.0 * a, a]
}

fn rel(c: Coord, origin: [f64; 2]) -> [f32; 2] {
    [(c.x - origin[0]) as f32, (c.y - origin[1]) as f32]
}

/// Drops consecutive points closer than 0.35 physical px, keeping the first and always the
/// last — the exact rule of `Ctx::ring_path` (the dropped point doesn't become the anchor).
/// `map_per_px` is map units per physical pixel (`camera.res / pixels_per_point` on the flat
/// map, where `to_screen` is affine).
fn decimate(ring: &[Coord], map_per_px: f64) -> Vec<Coord> {
    let threshold = 0.35 * map_per_px;
    let n = ring.len();
    let mut out = Vec::with_capacity(n.min(1024));
    let mut last: Option<Coord> = None;
    for (i, c) in ring.iter().enumerate() {
        let keep = !matches!(last, Some(l) if (c.x - l.x).abs() + (c.y - l.y).abs() < threshold && i + 1 != n);
        if keep {
            out.push(*c);
            last = Some(*c);
        }
    }
    out
}

fn path_of(rings: &[Vec<Coord>], origin: [f64; 2], map_per_px: f64, min_points: usize) -> Option<Path> {
    let mut b = Path::builder();
    let mut any = false;
    for ring in rings {
        let pts = decimate(ring, map_per_px);
        if pts.len() < min_points {
            continue;
        }
        any = true;
        let p0 = rel(pts[0], origin);
        b.begin(lyon::math::point(p0[0], p0[1]));
        for c in &pts[1..] {
            let p = rel(*c, origin);
            b.line_to(lyon::math::point(p[0], p[1]));
        }
        b.close();
    }
    any.then(|| b.build())
}

/// Tessellates polygon fills (all rings in one EvenOdd path, like the CPU's `Ctx::polygon`).
/// `None` when nothing would draw or tessellation fails (the caller falls the layer back to
/// the CPU pixmap).
pub(crate) fn fill_polygon(
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    color: [f32; 4],
    map_per_px: f64,
) -> Option<(Vec<MeshVertex>, Vec<u32>)> {
    let mut out: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    for rings in polys {
        let Some(path) = path_of(rings, origin, map_per_px, 3) else { continue };
        tess.tessellate_path(
            &path,
            &FillOptions::default().with_fill_rule(FillRule::EvenOdd),
            &mut BuffersBuilder::new(&mut out, |v: lyon::tessellation::FillVertex| {
                let p = v.position();
                MeshVertex { position: [p.x, p.y], normal: [0.0, 0.0], color, stroke_px: 0.0, _pad: 0.0 }
            }),
        )
        .ok()?;
    }
    (!out.indices.is_empty()).then_some((out.vertices, out.indices))
}

/// Strokes rings (closed, polygon outlines) or line parts (open) at width 1.0; the vertex
/// shader applies the real width. Ring outlines take no width floor (`Ctx::polygon`),
/// line parts floor at half a physical pixel (`Ctx::line`).
fn stroke(
    rings: &[Vec<Coord>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
    closed: bool,
    min_points: usize,
) -> Option<(Vec<MeshVertex>, Vec<u32>)> {
    let mut out: VertexBuffers<MeshVertex, u32> = VertexBuffers::new();
    let mut tess = StrokeTessellator::new();
    let options = StrokeOptions::default().with_line_width(1.0).with_line_cap(LineCap::Round).with_line_join(LineJoin::Round);
    for ring in rings {
        let pts = decimate(ring, map_per_px);
        if pts.len() < min_points {
            continue;
        }
        let mut b = Path::builder();
        let p0 = rel(pts[0], origin);
        b.begin(lyon::math::point(p0[0], p0[1]));
        for c in &pts[1..] {
            let p = rel(*c, origin);
            b.line_to(lyon::math::point(p[0], p[1]));
        }
        b.end(closed);
        // position_on_path = position - normal * half_width, half_width = 0.5 at width 1.0.
        tess.tessellate_path(
            &b.build(),
            &options,
            &mut BuffersBuilder::new(&mut out, |v: lyon::tessellation::StrokeVertex| {
                let (p, n) = (v.position(), v.normal());
                MeshVertex { position: [p.x - n.x * 0.5, p.y - n.y * 0.5], normal: [n.x, n.y], color, stroke_px: width_px, _pad: 0.0 }
            }),
        )
        .ok()?;
    }
    (!out.indices.is_empty()).then_some((out.vertices, out.indices))
}

/// Polygon outlines: every ring stroked (the CPU strokes exterior and holes alike).
pub(crate) fn stroke_rings(
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
) -> Option<(Vec<MeshVertex>, Vec<u32>)> {
    let rings: Vec<Vec<Coord>> = polys.iter().flatten().cloned().collect();
    stroke(&rings, origin, color, width_px, map_per_px, true, 3)
}

/// Line features (round caps, round joins, 0.5 px minimum width).
pub(crate) fn stroke_lines(
    parts: &[Vec<Coord>],
    origin: [f64; 2],
    color: [f32; 4],
    width_px: f32,
    map_per_px: f64,
) -> Option<(Vec<MeshVertex>, Vec<u32>)> {
    stroke(parts, origin, color, width_px.max(0.5), map_per_px, false, 2)
}

/// One marker (two when there's a halo: the halo is a larger filled shape drawn first,
/// matching the CPU's 3.5 px halo stroke drawn under the fill — `Ctx::marker`).
pub(crate) fn marker_instances(c: Coord, origin: [f64; 2], s: &Symbol, ppp: f32, halo: Option<Rgba>) -> Vec<PointInstance> {
    let r = (s.size.max(1.0) * ppp) / 2.0;
    let shape = s.marker as u32;
    let mut out = Vec::with_capacity(2);
    if let Some(h) = halo {
        out.push(PointInstance {
            position: rel(c, origin),
            color: premul(h),
            stroke: [0.0; 4],
            radius_px: r + 1.75 * ppp,
            stroke_px: 0.0,
            shape,
            _pad: 0.0,
        });
    }
    let stroke_px = if s.stroke_width > 0.0 && s.stroke.a() > 0 { s.stroke_width * ppp } else { 0.0 };
    out.push(PointInstance {
        position: rel(c, origin),
        color: premul(s.fill),
        stroke: premul(s.stroke),
        radius_px: r,
        stroke_px,
        shape,
        _pad: 0.0,
    });
    out
}

fn extend_mesh(v: &mut Vec<MeshVertex>, i: &mut Vec<u32>, part: Option<(Vec<MeshVertex>, Vec<u32>)>) {
    if let Some((pv, pi)) = part {
        let base = v.len() as u32;
        v.extend(pv);
        i.extend(pi.into_iter().map(|x| x + base));
    }
}

/// Appends one feature's GPU geometry, in the CPU path's per-feature order (fill, then
/// outline — `Ctx::feature`). Transparent fills/strokes are skipped: tiny-skia draws them as
/// no-ops, and not emitting them is identical output for less work.
fn append_geometry(
    v: &mut Vec<MeshVertex>,
    i: &mut Vec<u32>,
    pts: &mut Vec<PointInstance>,
    g: &Geometry,
    s: &Symbol,
    origin: [f64; 2],
    wireframe: bool,
    ppp: f32,
    map_per_px: f64,
) {
    match g {
        Geometry::Point(c) => pts.extend(marker_instances(*c, origin, s, ppp, None)),
        Geometry::MultiPoint(cs) => cs.iter().for_each(|c| pts.extend(marker_instances(*c, origin, s, ppp, None))),
        Geometry::Line(parts) => {
            if s.stroke.a() > 0 {
                extend_mesh(v, i, stroke_lines(parts, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px));
            }
        }
        Geometry::Polygon(polys) if wireframe => {
            extend_mesh(v, i, stroke_rings(polys, origin, premul(s.stroke.with_alpha(255)), s.stroke_width.max(1.0) * ppp, map_per_px));
        }
        Geometry::Polygon(polys) => {
            if s.fill.a() > 0 {
                extend_mesh(v, i, fill_polygon(polys, origin, premul(s.fill), map_per_px));
            }
            if s.stroke_width > 0.0 && s.stroke.a() > 0 {
                extend_mesh(v, i, stroke_rings(polys, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px));
            }
        }
    }
}

/// Collects one GPU-eligible layer into Mesh/Points commands. `geoms`, `rows` and `visible`
/// are the render loop's own (projected geometries, row filter, extent cull), so the GPU
/// draws exactly the features the CPU would. Returns the layer's origin for
/// [`collect_highlight`], `None` when nothing visible was collected.
pub(crate) fn collect_layer(
    cmds: &mut Vec<DrawCmd>,
    layer: &Layer,
    geoms: &[Option<&Geometry>],
    rows: &[bool],
    visible: &dyn Fn(&Extent) -> bool,
    wireframe: bool,
    ppp: f32,
    map_per_px: f64,
) -> Option<[f64; 2]> {
    // Stable origin: the centre of the drawn features' extent. Two passes cost one extra
    // extent fold — the same order of work as the culling pass the CPU render already does.
    let mut ext = Extent::EMPTY;
    for (row, g) in geoms.iter().enumerate() {
        let Some(g) = g else { continue };
        if rows.get(row) != Some(&true) || !visible(&g.extent()) {
            continue;
        }
        ext = ext.union(&g.extent());
    }
    if ext.is_empty() {
        return None;
    }
    let origin = [ext.center().x, ext.center().y];
    let (mut v, mut i, mut pts) = (Vec::new(), Vec::new(), Vec::new());
    for (row, g) in geoms.iter().enumerate() {
        let Some(g) = g else { continue };
        if rows.get(row) != Some(&true) || !visible(&g.extent()) {
            continue;
        }
        append_geometry(&mut v, &mut i, &mut pts, g, &layer.symbol(row), origin, wireframe, ppp, map_per_px);
    }
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
    if !pts.is_empty() {
        cmds.push(DrawCmd::Points { origin, instances: pts });
    }
    Some(origin)
}

/// Selection highlight of one feature, after its layer (`Ctx::highlight`): haloed markers,
/// a wider cyan under-stroke for lines, translucent cyan fill + outline for polygons.
pub(crate) fn collect_highlight(cmds: &mut Vec<DrawCmd>, g: &Geometry, s: &Symbol, ppp: f32, map_per_px: f64) {
    let e = g.extent();
    let origin = [e.center().x, e.center().y];
    match g {
        Geometry::Point(c) => cmds.push(DrawCmd::Points { origin, instances: marker_instances(*c, origin, s, ppp, Some(SELECTION)) }),
        Geometry::MultiPoint(cs) => {
            let instances: Vec<PointInstance> = cs.iter().flat_map(|c| marker_instances(*c, origin, s, ppp, Some(SELECTION))).collect();
            cmds.push(DrawCmd::Points { origin, instances });
        }
        Geometry::Line(parts) => {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            extend_mesh(&mut v, &mut i, stroke_lines(parts, origin, premul(SELECTION), (s.stroke_width + 3.0) * ppp, map_per_px));
            extend_mesh(&mut v, &mut i, stroke_lines(parts, origin, premul(s.stroke), s.stroke_width * ppp, map_per_px));
            if !i.is_empty() {
                cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
            }
        }
        Geometry::Polygon(polys) => {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            extend_mesh(&mut v, &mut i, fill_polygon(polys, origin, premul(SELECTION.with_alpha(40)), map_per_px));
            extend_mesh(&mut v, &mut i, stroke_rings(polys, origin, premul(SELECTION), 2.0 * ppp, map_per_px));
            if !i.is_empty() {
                cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
            }
        }
    }
}

/// The built-in basemap's land polygons (already in map units), fill plus optional outline.
pub(crate) fn collect_basemap(
    cmds: &mut Vec<DrawCmd>,
    polys: &[Vec<Vec<Coord>>],
    origin: [f64; 2],
    land: Rgba,
    outline: Rgba,
    ppp: f32,
    map_per_px: f64,
) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    extend_mesh(&mut v, &mut i, fill_polygon(polys, origin, premul(land), map_per_px));
    if outline.a() > 0 {
        extend_mesh(&mut v, &mut i, stroke_rings(polys, origin, premul(outline), 0.6 * ppp, map_per_px));
    }
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
}

/// The graticule lines (already in map units).
pub(crate) fn collect_graticule(cmds: &mut Vec<DrawCmd>, lines: &[Vec<Coord>], origin: [f64; 2], grat: Rgba, ppp: f32, map_per_px: f64) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    extend_mesh(&mut v, &mut i, stroke_lines(lines, origin, premul(grat), 0.6 * ppp, map_per_px));
    if !i.is_empty() {
        cmds.push(DrawCmd::Mesh { origin, vertices: v, indices: i });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_map::Marker;

    fn square(x: f64, y: f64, s: f64) -> Vec<Coord> {
        vec![Coord::new(x, y), Coord::new(x + s, y), Coord::new(x + s, y + s), Coord::new(x, y + s)]
    }

    fn point_covered(verts: &[MeshVertex], idx: &[u32], x: f32, y: f32) -> bool {
        idx.chunks(3).any(|t| {
            let [a, b, c] = [verts[t[0] as usize].position, verts[t[1] as usize].position, verts[t[2] as usize].position];
            let d = |o: [f32; 2], u: [f32; 2], v: [f32; 2]| (u[0] - o[0]) * (v[1] - o[1]) - (v[0] - o[0]) * (u[1] - o[1]);
            let (da, db, dc) = (d(a, b, c), d([x, y], b, c), d([x, y], c, a));
            da != 0.0 && db.signum() == da.signum() && dc.signum() == da.signum() && d([x, y], a, b).signum() == da.signum()
        })
    }

    #[test]
    fn polygon_with_hole_tessellates_evenodd() {
        let polys = vec![vec![square(0.0, 0.0, 100.0), square(30.0, 30.0, 40.0)]];
        let (v, i) = fill_polygon(&polys, [0.0, 0.0], [1.0, 0.0, 0.0, 1.0], 0.0).expect("tessellate");
        assert!(i.len() >= 24, "a hole needs interior triangles: {}", i.len());
        assert!(!point_covered(&v, &i, 50.0, 50.0), "hole center must stay uncovered");
        // (10, 5): inside, and not on a tessellation diagonal (edge points read as uncovered).
        assert!(point_covered(&v, &i, 10.0, 5.0), "corner region must be covered");
    }

    #[test]
    fn decimate_matches_ring_path_rule() {
        // map_per_px = 2: threshold 0.7. Points at 0.5 apart drop, 0.8 apart keep.
        let ring = vec![
            Coord::new(0.0, 0.0),
            Coord::new(0.5, 0.0),  // 0.5 from anchor: drop
            Coord::new(1.0, 0.0),  // 1.0 from anchor (not the dropped 0.5): keep
            Coord::new(1.5, 0.0),  // 0.5: drop
            Coord::new(1.7, 0.0),  // 0.7: not < 0.7, keep
            Coord::new(1.71, 0.0), // last point: always kept
        ];
        let out = decimate(&ring, 2.0);
        assert_eq!(out, vec![Coord::new(0.0, 0.0), Coord::new(1.0, 0.0), Coord::new(1.7, 0.0), Coord::new(1.71, 0.0)]);
    }

    #[test]
    fn stroke_vertices_carry_centerline_and_normal() {
        let parts = vec![vec![Coord::new(0.0, 0.0), Coord::new(100.0, 0.0)]];
        let (v, i) = stroke_lines(&parts, [0.0, 0.0], [0.0, 0.0, 1.0, 1.0], 2.0, 0.0).expect("stroke");
        assert!(!i.is_empty());
        for vtx in &v {
            assert!(vtx.position[1].abs() < 1e-4, "centerline stays on the path: y={}", vtx.position[1]);
            let len = (vtx.normal[0] * vtx.normal[0] + vtx.normal[1] * vtx.normal[1]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "unit normal (round caps point along x): {:?}", vtx.normal);
            assert_eq!(vtx.stroke_px, 2.0);
        }
    }

    #[test]
    fn ring_stroke_has_no_width_floor_and_closes() {
        let polys = vec![vec![square(0.0, 0.0, 10.0)]];
        let (v, i) = stroke_rings(&polys, [0.0, 0.0], [0.0; 4], 0.2, 0.0).expect("stroke");
        assert!(!i.is_empty());
        assert!(v.iter().all(|x| x.stroke_px == 0.2), "polygon outlines keep their exact width");
    }

    #[test]
    fn line_width_floors_at_half_pixel() {
        let parts = vec![vec![Coord::new(0.0, 0.0), Coord::new(10.0, 0.0)]];
        let (v, _) = stroke_lines(&parts, [0.0, 0.0], [0.0; 4], 0.2, 0.0).expect("stroke");
        assert!(v.iter().all(|x| x.stroke_px == 0.5));
    }

    #[test]
    fn marker_halo_emits_two_instances() {
        let s =
            Symbol { fill: Rgba::rgb(255, 0, 0), stroke: Rgba::rgb(255, 255, 255), stroke_width: 1.0, size: 8.0, marker: Marker::Circle };
        let two = marker_instances(Coord::new(5.0, 5.0), [5.0, 5.0], &s, 2.0, Some(Rgba::rgb(0, 255, 255)));
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].radius_px, 8.0 + 3.5, "halo = r + 1.75*ppp"); // r = 8*2/2 = 8
        assert_eq!(two[0].stroke_px, 0.0);
        assert_eq!(two[1].radius_px, 8.0);
        assert_eq!(two[1].stroke_px, 2.0);
        let one = marker_instances(Coord::new(5.0, 5.0), [5.0, 5.0], &s, 2.0, None);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn premultiplies_alpha() {
        assert_eq!(premul(Rgba::rgb(255, 0, 0).with_alpha(128)), [128.0 / 255.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
