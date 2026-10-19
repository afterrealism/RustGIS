//! Manual timing harness for LOD work: `cargo test -p rustgis-render --test lod_bench --
//! --ignored --nocapture`. Not run in CI (timing is machine-dependent; correctness gates
//! live in the unit tests).

use rustgis_data::{Coord, Dataset, Geometry, GeometryType};
use rustgis_map::{Camera, Map};
use rustgis_render::{Backend, RenderOutput, View, render_backend};
/// `n`×`n` grid of ~4-vertex buildings spanning the world — the zoomed-out worst case.
fn buildings(n: usize) -> Map {
    let mut m = Map::new("bench");
    let mut d = Dataset::new("b", GeometryType::Polygon, vec![]);
    let span = 0.02; // ~2 km cells in degrees
    for i in 0..n * n {
        let x = (i % n) as f64 * span - 180.0 + span;
        let y = (i / n) as f64 * span - 85.0 + span;
        d.push(
            Some(Geometry::Polygon(vec![vec![vec![
                Coord::new(x, y),
                Coord::new(x + 0.3 * span, y),
                Coord::new(x + 0.3 * span, y + 0.3 * span),
                Coord::new(x, y + 0.3 * span),
            ]]])),
            vec![],
        );
    }
    m.add_dataset(d);
    m
}

fn bench(m: &mut Map, view: &View, label: &str) {
    for backend in [Backend::Cpu, Backend::Gpu] {
        // Warm-up: builds projected + thinned caches.
        let _ = render_backend(m, view, backend);
        let t0 = web_time::Instant::now();
        const FRAMES: usize = 10;
        for _ in 0..FRAMES {
            let out = render_backend(m, view, backend);
            debug_assert!(out.is_some());
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        let tris = match render_backend(m, view, backend) {
            Some(RenderOutput::Gpu(scene)) => scene
                .cmds
                .iter()
                .map(|c| match c {
                    rustgis_render::gpu_scene::DrawCmd::Mesh { indices, .. } => indices.len() / 3,
                    _ => 0,
                })
                .sum::<usize>(),
            _ => 0,
        };
        println!("{label} {backend:?}: {ms:.1} ms/frame, gpu triangles: {tris}");
    }
}

#[test]
#[ignore]
fn bench_world_zoom_render() {
    let mut m = buildings(1000); // 1M buildings; scale to 4000×4000 if the machine is fast
    let view = View { width: 1280.0, height: 800.0, pixels_per_point: 1.0, camera: Camera::world() };
    bench(&mut m, &view, "world");
    // Zoomed in on a ~1° window mid-grid — culling wins here; at world zoom everything is
    // visible, so the prefix is the whole index.
    let zoomed = View { camera: Camera { center: rustgis_geom::lonlat_to_merc(Coord::new(-170.0, -75.0)), res: 100.0 }, ..view };
    bench(&mut m, &zoomed, "zoomed");
}

/// `n` points spread over the world, one circle-marker layer — the dense point-cloud case the
/// sub-pixel marker fast path targets (Amsterdam's points layer at world zoom).
fn points(n: usize) -> Map {
    use rustgis_map::{Marker, Renderer, Rgba, Symbol};
    let mut m = Map::new("pts");
    let mut d = Dataset::new("p", GeometryType::Point, vec![]);
    let golden = (5f64.sqrt() - 1.0) / 2.0;
    let (mut x, mut y) = (0.5f64, 0.5f64);
    for _ in 0..n {
        let t = golden * std::f64::consts::TAU;
        (x, y) = ((x + t.cos()) % 1.0, (y + t.sin()) % 1.0);
        d.push(Some(Geometry::Point(Coord::new(x * 360.0 - 180.0, y * 160.0 - 80.0))), vec![]);
    }
    let id = m.add_dataset(d);
    if let Some(l) = m.layer_mut(id) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0xE0, 0x70, 0x20),
            stroke: Rgba::rgb(0, 0, 0).with_alpha(0),
            stroke_width: 0.0,
            size: 1.0,
            marker: Marker::Circle,
        });
    }
    m
}

fn set_marker_size(m: &mut Map, size: f64) {
    use rustgis_map::Renderer;
    let id = m.layers.first().map(|l| l.id).unwrap_or_default();
    if let Some(l) = m.layer_mut(id)
        && let Renderer::Single(s) = &mut l.renderer
    {
        s.size = size as f32;
    }
}

#[test]
#[ignore]
fn bench_subpixel_markers() {
    // 500k points at world zoom: size 6 draws every marker through the path builder, size 1
    // through the direct-pixel fast path. The gap between the two numbers is the fix's value.
    let mut m = points(500_000);
    let view = View { width: 1280.0, height: 800.0, pixels_per_point: 1.0, camera: Camera::world() };
    set_marker_size(&mut m, 6.0);
    bench(&mut m, &view, "markers size 6 (path)");
    set_marker_size(&mut m, 1.0);
    bench(&mut m, &view, "markers size 1 (fast path)");
}

/// What Add Data's eager warm costs per layer size (`Layer::projected`, which includes the
/// eager cull-index sort above `CULL_INDEX_ABOVE`): the numbers behind
/// `EAGER_PROJECT_UP_TO`'s threshold. `push_next` keeps the fixture build linear.
#[test]
#[ignore]
fn bench_eager_warm() {
    use rustgis_data::{Coord, Dataset, Geometry, GeometryType};
    use rustgis_map::cull::CULL_INDEX_ABOVE;
    let warm = |label: &str, n: usize, d: Dataset| {
        let mut m = Map::new("warm");
        let id = m.add_dataset(d);
        let t0 = web_time::Instant::now();
        m.layer_mut(id).map(|l| l.projected());
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        println!("warm {label}: {ms:.1} ms (cull sort: {})", n > CULL_INDEX_ABOVE);
    };
    for n in [50_000usize, 100_000, 250_000, 500_000] {
        let mut d = Dataset::new("pts", GeometryType::Point, vec![]);
        for i in 0..n {
            let x = i as f64 * 360.0 / n as f64 - 180.0;
            d.push_next(Some(Geometry::Point(Coord::new(x, (x * 3.0).sin()))), vec![]);
        }
        warm(&format!("{n:>6} points"), n, d);
    }
    // The gate counts features, not vertices: many-vertex features are the real worst case.
    for (n, verts) in [(100_000usize, 50usize), (500_000, 10)] {
        let mut d = Dataset::new("lines", GeometryType::Polyline, vec![]);
        for i in 0..n {
            let run = (0..verts).map(|v| Coord::new(v as f64 * 0.001 - 180.0 + i as f64 * 1e-5, (i % 90) as f64)).collect();
            d.push_next(Some(Geometry::Line(vec![run])), vec![]);
        }
        warm(&format!("{n:>6} lines x{verts}"), n, d);
    }
}

/// Does rendering a `Map` clone (the worker's settle frame) redo thinned-copy work the
/// original already paid? A: settle renders on fresh clones (worker behaviour) vs B: settle
/// renders on the original (UI-thread behaviour). If A stays far above B, the worker's
/// warmed caches are worth writing back; if they converge, they are not.
#[test]
#[ignore]
fn bench_worker_clone_cache_loss() {
    use rustgis_data::{Coord, Dataset, Geometry, GeometryType};
    use rustgis_render::{Backend, RenderOutput, render_backend_lod};
    // 6k lines x 60 vertices = 360k thinnable vertices, past THIN_ABOVE.
    let mut d = Dataset::new("lines", GeometryType::Polyline, vec![]);
    for i in 0..6_000 {
        let run = (0..60).map(|v| Coord::new(v as f64 * 0.01 - 180.0 + i as f64 * 1e-4, ((i + v) % 90) as f64)).collect();
        d.push_next(Some(Geometry::Line(vec![run])), vec![]);
    }
    let mut m = Map::new("loss");
    m.add_dataset(d);
    let view = View { width: 1280.0, height: 800.0, pixels_per_point: 1.0, camera: Camera::world() };
    // Warm the interaction level on the original — the UI map's state mid-gesture.
    let _ = render_backend_lod(&mut m, &view, Backend::Cpu, 4.0);
    let cpu = |m: &mut Map, bias: f64| matches!(render_backend_lod(m, &view, Backend::Cpu, bias), Some(RenderOutput::Cpu(_)));
    for round in 1..=3 {
        let t0 = web_time::Instant::now();
        assert!(cpu(&mut m.clone(), 1.0), "clone settle renders");
        println!("clone settle {round}: {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
    for round in 1..=3 {
        let t0 = web_time::Instant::now();
        assert!(cpu(&mut m, 1.0), "original settle renders");
        println!("original settle {round}: {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
}
