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

#[test]
#[ignore]
fn bench_world_zoom_render() {
    let mut m = buildings(1000); // 1M buildings; scale to 4000×4000 if the machine is fast
    let view = View { width: 1280.0, height: 800.0, pixels_per_point: 1.0, camera: Camera::world() };
    for backend in [Backend::Cpu, Backend::Gpu] {
        // Warm-up: builds projected + thinned caches.
        let _ = render_backend(&mut m, &view, backend);
        let t0 = web_time::Instant::now();
        const FRAMES: usize = 10;
        for _ in 0..FRAMES {
            let out = render_backend(&mut m, &view, backend);
            debug_assert!(out.is_some());
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        let tris = match render_backend(&mut m, &view, backend) {
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
        println!("{backend:?}: {ms:.1} ms/frame, gpu triangles: {tris}");
    }
}
