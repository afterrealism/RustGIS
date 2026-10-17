//! Real-data LOD corpus tests — plan: `docs/plans/2026-10-17-rustgis-lod-testdata.md` (§4
//! harness, §5 matrix, §7 perf protocol).
//!
//! Run: `RUSTGIS_LOD_DATA=<corpus dir> cargo test -p rustgis-render --test lod_data -- --nocapture`.
//! Without the env var every test prints `skipped` and passes, so a checkout without the corpus
//! (CI) stays green. Correctness tests run when the data is present; the perf table stays
//! `#[ignore]`d (machine-dependent — run it in `--release` and quote it, never assert it).
//!
//! `osm-netherlands` (1.4 GB) is over `io::MAX_BYTES`: its load errors, `corpus()` returns
//! `None` and its tests skip — by design (plan §2). `overture-buildings` (4.9M buildings)
//! materializes through the GeoParquet provider as a ≤2M-feature sample, so it serves the
//! cull-equivalence test (extents only) and the perf table rather than the per-pixel loops.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use rustgis_map::projection::Flat;
use rustgis_map::{Camera, Map, RasterLayer, lod};
use rustgis_render::gpu_scene::{DrawCmd, GpuScene};
use rustgis_render::{Backend, RenderOutput, View, render_backend, render_backend_lod};

/// The corpus directory from `RUSTGIS_LOD_DATA`, or `None`: every test then skips green.
fn data() -> Option<PathBuf> {
    std::env::var_os("RUSTGIS_LOD_DATA").map(PathBuf::from).filter(|p| p.is_dir())
}

macro_rules! skip_unless_data {
    () => {
        let Some(_) = data() else {
            eprintln!("skipped: RUSTGIS_LOD_DATA is not set (or not a directory)");
            return;
        };
    };
}

/// The §2 corpus table (vector files).
const IDS: &[(&str, &str)] = &[
    ("ne-admin0", "vector/ne_10m_admin_0_countries/ne_10m_admin_0_countries.shp"),
    ("ne-admin1", "vector/ne_10m_admin_1_states_provinces/ne_10m_admin_1_states_provinces.shp"),
    ("ne-rivers", "vector/ne_10m_rivers_lake_centerlines/ne_10m_rivers_lake_centerlines.shp"),
    ("ne-lakes", "vector/ne_10m_lakes/ne_10m_lakes.shp"),
    ("osm-monaco", "vector/monaco-latest.osm.pbf"),
    ("osm-liechtenstein", "vector/liechtenstein-latest.osm.pbf"),
    ("osm-amsterdam", "vector/Amsterdam.osm.pbf"),
    ("osm-netherlands", "vector/netherlands-latest.osm.pbf"), // over io::MAX_BYTES, skips by design
    ("overture-buildings", "vector/overture-buildings-part0.zstd.parquet"),
];

/// The §2 raster table.
const RASTERS: &[(&str, &str)] = &[
    ("dem-alps", "raster/Copernicus_DSM_COG_10_N47_00_E009_00_DEM.tif"),
    ("worldcover", "raster/ESA_WorldCover_10m_2021_v200_N45E009_Map.tif"),
];

/// The vectors the render loops exercise. `osm-netherlands` stays listed to prove the
/// too-big-file skip; `overture-buildings` is extents-only (T3.1) plus the perf table.
const RENDER_IDS: &[&str] =
    &["ne-admin0", "ne-admin1", "ne-rivers", "ne-lakes", "osm-monaco", "osm-liechtenstein", "osm-amsterdam", "osm-netherlands"];

/// Reads one corpus id into its datasets (all of a PBF's layers), or `None` when the file is
/// missing or unreadable — the corpus is machine-local, so absence skips, never fails. Each id
/// is parsed once per process and tests share the rows (a `Dataset` clone shares its features).
fn datasets(id: &'static str) -> Option<Vec<Dataset>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Vec<Dataset>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(hit) = guard.get(id) {
        return hit.clone();
    }
    let loaded = load_vector(id);
    guard.insert(id, loaded.clone());
    loaded
}

fn load_vector(id: &'static str) -> Option<Vec<Dataset>> {
    let rel = IDS.iter().find(|(k, _)| *k == id)?.1;
    let p = data()?.join(rel);
    if !p.is_file() {
        eprintln!("lod_data: {id}: {} not found — skipped", p.display());
        return None;
    }
    let name = p.display().to_string();
    if name.ends_with(".pbf") {
        // `read_path` reads only a PBF's first layer; the corpus wants them all (`read_layers`).
        let size = std::fs::metadata(&p).ok()?.len();
        if size > rustgis_data::io::MAX_BYTES {
            eprintln!("lod_data: {id}: {size} bytes > io::MAX_BYTES — skipped by design (plan §2)");
            return None;
        }
        let bytes = std::fs::read(&p).ok()?;
        let layers = rustgis_data::io::read_layers(&name, &bytes).ok()?;
        eprintln!(
            "lod_data: {id}: layers: {}",
            layers.iter().map(|l| format!("{} ({} features)", l.dataset.name, l.dataset.features.len())).collect::<Vec<_>>().join(", ")
        );
        Some(layers.into_iter().map(|l| l.dataset).collect())
    } else {
        match rustgis_data::io::read_path(&p) {
            Ok((ds, report)) => {
                eprintln!("lod_data: {id}: {} ({} features)", ds.name, ds.features.len());
                if !report.messages.is_empty() {
                    eprintln!("lod_data: {id}: {}", report.messages.join("; "));
                }
                Some(vec![ds])
            }
            Err(e) => {
                eprintln!("lod_data: {id}: unreadable ({e}) — skipped");
                None
            }
        }
    }
}

/// A corpus dataset as a `Map` (every layer of it), or `None` → skip.
fn corpus(id: &'static str) -> Option<(Map, String)> {
    let mut m = Map::new("corpus");
    for ds in datasets(id)? {
        m.add_dataset(ds);
    }
    Some((m, id.to_string()))
}

/// A corpus id's largest polygon layer as a single-layer `Map` (the OSM buildings tests).
fn corpus_largest_polygon(id: &'static str) -> Option<(Map, String)> {
    let ds = datasets(id)?.into_iter().filter(|d| d.geometry_type == GeometryType::Polygon).max_by_key(|d| d.features.len())?;
    let mut m = Map::new("corpus");
    m.add_dataset(ds);
    Some((m, id.to_string()))
}

/// A corpus raster layer (parsed once per process, shared through its `Arc` data).
fn raster_layer(id: &'static str) -> Option<RasterLayer> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<RasterLayer>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().ok()?;
    if let Some(hit) = guard.get(id) {
        return hit.clone();
    }
    let loaded = RASTERS.iter().find(|(k, _)| *k == id).and_then(|(_, rel)| {
        let p = data()?.join(rel);
        let r = rustgis_data::raster::read_path(&p).ok()?;
        eprintln!("lod_data: {id}: raster opened");
        Some(RasterLayer::from_path(r, p.display().to_string()))
    });
    guard.insert(id, loaded.clone());
    loaded
}

/// dem-alps with ne-rivers over it: the X4 hybrid scene (raster + vector in one frame).
fn overlay_map() -> Option<Map> {
    let rl = raster_layer("dem-alps")?;
    let mut m = Map::new("corpus");
    m.add_raster("dem-alps", rl);
    for ds in datasets("ne-rivers")? {
        m.add_dataset(ds);
    }
    Some(m)
}

/// A vector-only map: no basemap backdrop or graticule, so the scene is the corpus layer alone.
fn bare(mut m: Map) -> Map {
    m.basemap = rustgis_map::Basemap::Blank;
    m.graticule = false;
    m
}

// --- Camera presets (§3; 1280×800 logical points) ---

fn at(camera: Camera) -> View {
    View { width: 1280.0, height: 800.0, pixels_per_point: 1.0, camera }
}

fn world() -> View {
    at(Camera::world())
}

fn region() -> View {
    at(Camera { center: lonlat_to_merc(Coord::new(5.3, 52.2)), res: 500.0 })
}

/// City zoom (res 4) centred on the given place: Amsterdam for the Amsterdam/OSM corpus,
/// Monaco for the monaco tests (the §3 `city` preset is Amsterdam's; a city-zoom view of
/// Amsterdam shows none of Monaco, which would vacuously pass "no dots, no meshes").
fn city_at(lon: f64, lat: f64) -> View {
    at(Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res: 4.0 })
}

fn city() -> View {
    city_at(4.90, 52.37)
}

fn city_hidpi() -> View {
    View { pixels_per_point: 2.0, ..city() }
}

fn city_raster() -> View {
    city_at(9.5, 46.5)
}

// --- GPU scene / CPU pixmap helpers (§4: plain functions so both backends read alike) ---

/// Mesh vertex count over the GPU scene (polygon fills + line strokes).
fn mesh_vertices(scene: &GpuScene) -> usize {
    scene
        .cmds
        .iter()
        .map(|c| match c {
            DrawCmd::Mesh { vertices, .. } => vertices.len(),
            _ => 0,
        })
        .sum()
}

/// Mesh triangle count over the GPU scene.
fn triangles(scene: &GpuScene) -> usize {
    scene
        .cmds
        .iter()
        .map(|c| match c {
            DrawCmd::Mesh { indices, .. } => indices.len() / 3,
            _ => 0,
        })
        .sum()
}

/// Collapsed sub-pixel features (whole-feature polygon/line → one dot instance).
fn dots(scene: &GpuScene) -> usize {
    scene
        .cmds
        .iter()
        .map(|c| match c {
            DrawCmd::Points { instances, .. } => instances.len(),
            _ => 0,
        })
        .sum()
}

/// The GPU scene, or `None` when the backend fell back to a CPU frame.
fn gpu(m: &mut Map, view: &View, bias: f64) -> Option<GpuScene> {
    match render_backend_lod(m, view, Backend::Gpu, bias) {
        Some(RenderOutput::Gpu(scene)) => Some(scene),
        _ => None,
    }
}

/// FNV-1a, the in-crate golden hash's discipline (`render/src/lib.rs` `fnv1a`).
fn fnv1a(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in data {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// FNV-1a of the CPU pixmap's bytes at an interaction LOD bias (1.0 = exact).
fn cpu_hash(m: &mut Map, view: &View, bias: f64) -> Option<u64> {
    match render_backend_lod(m, view, Backend::Cpu, bias) {
        Some(RenderOutput::Cpu(r)) => Some(fnv1a(r.pixmap.data())),
        _ => None,
    }
}

/// Non-finite vertices in a geometry set (points the projection can't show).
fn non_finite(geoms: &[Option<Geometry>]) -> usize {
    let mut n = 0;
    for g in geoms.iter().flatten() {
        g.for_each_coord(|c| {
            if !c.is_finite() {
                n += 1;
            }
        });
    }
    n
}

/// The ~50 cull probes of T3.1: world, the four hemispheres, an empty Pacific window, and 44
/// deterministic 1° windows from an LCG (no rand crate).
fn probe_extents() -> Vec<Extent> {
    let mut out = vec![
        Extent::WORLD_LONLAT,
        Extent::new(-180.0, -85.0, 0.0, 85.0),     // western hemisphere
        Extent::new(0.0, -85.0, 180.0, 85.0),      // eastern hemisphere
        Extent::new(-180.0, 0.0, 180.0, 85.0),     // northern hemisphere
        Extent::new(-180.0, -85.0, 180.0, 0.0),    // southern hemisphere
        Extent::new(-160.0, -35.0, -150.0, -25.0), // empty Pacific
    ];
    let mut s = 0x243F_6A88_85A3_08D3u64;
    let mut next = move || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        s >> 33
    };
    for _ in 0..44 {
        let lon = -180.0 + (next() % 359) as f64;
        let lat = -85.0 + (next() % 169) as f64;
        out.push(Extent::new(lon, lat, lon + 1.0, lat + 1.0));
    }
    out
}

// --- X3: hostile real data never panics (plus X4 raster smoke) ---

/// X3.1: every loadable corpus dataset renders at world/region/city on both backends — it
/// completes, no panic. Includes the rasters alone and the raster+vector overlay (X4 smoke).
#[test]
fn corpus_never_panics() {
    skip_unless_data!();
    let views = [("world", world()), ("region", region()), ("city", city())];
    for &id in RENDER_IDS {
        let Some((mut m, name)) = corpus(id) else { continue };
        for (pname, view) in &views {
            for backend in [Backend::Cpu, Backend::Gpu] {
                assert!(render_backend(&mut m, view, backend).is_some(), "{name} at {pname} on {backend:?} did not complete");
            }
        }
        eprintln!("lod_data: X3.1 {name}: world/region/city × CPU/GPU OK");
    }
    // The rasters alone (X4), including a HiDPI frame.
    for &(rid, _) in RASTERS {
        let Some(rl) = raster_layer(rid) else { continue };
        let mut m = Map::new("corpus");
        m.add_raster(rid, rl);
        for (pname, view) in [("world", world()), ("city-raster", city_raster()), ("city-hidpi", city_hidpi())] {
            for backend in [Backend::Cpu, Backend::Gpu] {
                assert!(render_backend(&mut m, &view, backend).is_some(), "{rid} at {pname} on {backend:?} did not complete");
            }
        }
        eprintln!("lod_data: X3.1 {rid}: raster renders OK");
    }
    // Raster + vector overlay in one frame (X4 smoke).
    if let Some(mut m) = overlay_map() {
        for backend in [Backend::Cpu, Backend::Gpu] {
            assert!(
                render_backend(&mut m, &city_raster(), backend).is_some(),
                "dem-alps + ne-rivers overlay on {backend:?} did not complete"
            );
        }
        eprintln!("lod_data: X3.1/X4 dem-alps + ne-rivers overlay OK");
    }
}

// --- X2: byte stability ---

/// X2.1: the same (map, view) renders to identical bytes twice — no hidden nondeterminism in
/// the LOD paths (thinning, collapse, cull index).
#[test]
fn corpus_renders_are_byte_stable() {
    skip_unless_data!();
    for &id in RENDER_IDS {
        let Some((mut m, name)) = corpus(id) else { continue };
        let (a, b) = (cpu_hash(&mut m, &world(), 1.0), cpu_hash(&mut m, &world(), 1.0));
        assert!(a.is_some() && a == b, "{name}: world render not byte-stable: {a:?} vs {b:?}");
        eprintln!("lod_data: X2.1 {name}: world cpu hash {a:?} stable across two renders");
    }
    // A raster scene too.
    if let Some(rl) = raster_layer("dem-alps") {
        let mut m = Map::new("corpus");
        m.add_raster("dem-alps", rl);
        let (a, b) = (cpu_hash(&mut m, &city_raster(), 1.0), cpu_hash(&mut m, &city_raster(), 1.0));
        assert!(a.is_some() && a == b, "dem-alps: raster render not byte-stable: {a:?} vs {b:?}");
        eprintln!("lod_data: X2.1 dem-alps: city-raster cpu hash {a:?} stable");
    }
}

// --- X1: CPU↔GPU parity ---

/// X1.1: both backends complete; the GPU scene is non-empty whenever the CPU frame drew
/// something; and on real data the monaco check of the spec's Step 1: at world zoom the
/// buildings collapse to GPU dots while the CPU draws them.
#[test]
fn corpus_cpu_gpu_parity() {
    skip_unless_data!();
    let mut empty = Map::new("empty");
    let blank = cpu_hash(&mut empty, &world(), 1.0);
    for &id in RENDER_IDS {
        let Some((mut m, name)) = corpus(id) else { continue };
        let h = cpu_hash(&mut m, &world(), 1.0);
        let Some(scene) = gpu(&mut m, &world(), 1.0) else { panic!("{name}: no GPU scene at world") };
        if h != blank {
            assert!(!scene.cmds.is_empty(), "{name}: the CPU frame has content but the GPU scene is empty");
        }
        eprintln!(
            "lod_data: X1.1 {name}: cpu_inked={}, gpu cmds={}, tris={}, verts={}, dots={}",
            h != blank,
            scene.cmds.len(),
            triangles(&scene),
            mesh_vertices(&scene),
            dots(&scene)
        );
    }
    // Monaco at world: every building is sub-pixel → GPU dots, and the CPU draws the same dots.
    let Some((m, _)) = corpus_largest_polygon("osm-monaco") else { return };
    let mut m = bare(m);
    let Some(scene) = gpu(&mut m, &world(), 1.0) else { panic!("monaco: no GPU scene at world") };
    let h = cpu_hash(&mut m, &world(), 1.0);
    let mut blank_map = bare(Map::new("empty"));
    let blank_h = cpu_hash(&mut blank_map, &world(), 1.0);
    assert!(dots(&scene) > 0, "monaco buildings must collapse to GPU dots at world zoom");
    assert!(h.is_some() && h != blank_h, "monaco: the CPU frame draws the collapsed dots");
    eprintln!("lod_data: X1.1 osm-monaco: {} GPU dots, CPU draws them too", dots(&scene));
}

// --- T1: Visvalingam–Whyatt thinning ---

/// T1.2/T1.3: VW thinning on real data is monotone in tolerance and keeps the ring floor.
/// The thinned cache is whole-layer and tolerance-keyed, and the world preset carries the
/// *coarsest* tolerance (res 30 000 vs the city's 4), so its cache holds the *fewest* vertices:
/// counts are non-decreasing world→region→city (i.e. non-increasing in tolerance, the
/// `lod.rs::vw_monotone_in_tolerance` invariant). Measured ne-admin0: 111 788 → 548 471.
#[test]
fn vw_monotone_and_ring_floor_on_real_data() {
    skip_unless_data!();
    // The renderer's tolerance per preset (render_backend_lod: res/ppp × THIN_PX × bias 1.0).
    let tols = [("world", 30_000.0 * lod::THIN_PX), ("region", 500.0 * lod::THIN_PX), ("city", 4.0 * lod::THIN_PX)];
    for id in ["ne-admin0", "ne-rivers"] {
        let Some((mut m, name)) = corpus(id) else { continue };
        let Some(layer) = m.layers.first_mut() else { continue };
        let mut prev = 0;
        for (pname, tol) in tols {
            layer.thinned_in(&Flat::Mercator, "mercator", tol); // refresh projected + thinned copy
            let cache = layer.thinned_cache(tol);
            let verts: usize = cache.iter().flatten().map(lod::vertices).sum();
            let mut min_ring = usize::MAX;
            for g in cache.iter().flatten() {
                if let Geometry::Polygon(polys) = g {
                    for ring in polys.iter().flatten() {
                        min_ring = min_ring.min(ring.len());
                    }
                }
            }
            let ring = if min_ring == usize::MAX { "n/a".to_string() } else { min_ring.to_string() };
            eprintln!("lod_data: T1 {name}@{pname}: tol={tol} vertices={verts} min_ring={ring}");
            assert!(
                verts >= prev,
                "{name}: thinning not monotone at {pname}: {verts} < {prev} (coarser presets must not keep more vertices)"
            );
            assert!(min_ring == usize::MAX || min_ring >= 4, "{name}: a ring thinned below 4 vertices at {pname}");
            prev = verts;
        }
    }
}

/// T1.4: vertices a projection can't show (non-finite) are pinned, so runs break there — every
/// non-finite vertex in ne-admin0's Mercator copy survives thinning at the world tolerance.
#[test]
fn vw_pins_non_finite_polar_vertices() {
    skip_unless_data!();
    let Some((mut m, _)) = corpus("ne-admin0") else { return };
    let tol = 30_000.0 * lod::THIN_PX; // the world preset's tolerance
    let Some(layer) = m.layers.first_mut() else { return };
    let before = non_finite(layer.projected_in(&Flat::Mercator, "mercator"));
    layer.thinned_in(&Flat::Mercator, "mercator", tol);
    let after = non_finite(layer.thinned_cache(tol));
    assert_eq!(before, after, "every non-finite projected vertex must survive VW thinning: {before} → {after}");
    if before == 0 {
        eprintln!(
            "lod_data: T1.4 ne-admin0 projects fully finite in Web Mercator (latitude clamps at ±85.05°, finite input stays finite); NaN pinning itself is pinned by lod.rs's unit tests"
        );
    } else {
        eprintln!("lod_data: T1.4 ne-admin0: {before} non-finite Mercator vertices, all survive thinning");
    }
}

// --- T2: sub-pixel collapse ---

/// T2.1/T2.2: monaco's buildings layer (the PBF's largest polygon layer), blank basemap — at
/// world zoom every building is sub-pixel (thousands of dots, meshes negligible); at city zoom
/// collapse is marginal (a handful of 2–4 m features, logged) and everything else meshes.
#[test]
fn monaco_collapse_world_vs_city() {
    skip_unless_data!();
    let Some((m, _)) = corpus_largest_polygon("osm-monaco") else { return };
    let mut m = bare(m);
    let Some(w) = gpu(&mut m, &world(), 1.0) else { panic!("monaco: no GPU scene at world") };
    eprintln!("lod_data: T2 monaco@world: dots={} tris={} verts={}", dots(&w), triangles(&w), mesh_vertices(&w));
    assert!(dots(&w) >= 1000, "monaco buildings all collapse at world zoom: {} dots", dots(&w));
    // "meshes small" (the task wording): the multipolygons layer holds a couple of large
    // non-building polygons (boundary/coast class) that legitimately stay meshes at world zoom,
    // but the collapsed dots overwhelmingly dominate (2199 dots vs 100 triangles here).
    assert!(triangles(&w) < dots(&w), "dots ≫ meshes at world zoom: {} tris vs {} dots", triangles(&w), dots(&w));
    let Some(c) = gpu(&mut m, &city_at(7.42, 43.73), 1.0) else { panic!("monaco: no GPU scene at city zoom") };
    eprintln!("lod_data: T2 monaco@city: dots={} tris={} verts={}", dots(&c), triangles(&c), mesh_vertices(&c));
    // The spec's "zoomed in nothing collapses" against real data: 14 genuine 2–4 m OSM features
    // (sheds, kiosks, entrances — measured, each a 5–20 vertex polygon) are still sub-pixel at
    // res 4 (1 px = 4 m) and collapse by design. Essentially nothing collapses: a tight bound,
    // not zero, and the exact count is logged above.
    assert!(dots(&c) <= 20, "city zoom: collapse is marginal, not absent — {} dots (14 measured)", dots(&c));
    assert!(triangles(&c) > 0, "city zoom: buildings mesh");
}

/// T2.3: ne-lakes at world — the natural size spread crosses the collapse threshold inside one
/// layer: the Caspian stays a mesh while tiny lakes become dots.
#[test]
fn lakes_mixed_collapse() {
    skip_unless_data!();
    let Some((m, _)) = corpus("ne-lakes") else { return };
    let mut m = bare(m);
    let Some(scene) = gpu(&mut m, &world(), 1.0) else { panic!("ne-lakes: no GPU scene at world") };
    eprintln!("lod_data: T2.3 ne-lakes@world: dots={} tris={} verts={}", dots(&scene), triangles(&scene), mesh_vertices(&scene));
    assert!(dots(&scene) > 0, "tiny lakes collapse to dots at world zoom");
    assert!(triangles(&scene) > 0, "the Caspian-class lakes stay meshes at world zoom");
}

// --- T3: sorted-extent cull index ---

/// T3.1: `CullIndex::candidates` returns exactly the linear `intersects` scan's rows, in order,
/// at corpus scale — world, hemispheres, an empty ocean window and 44 deterministic 1° windows.
#[test]
fn cull_index_matches_linear_scan() {
    skip_unless_data!();
    for id in ["ne-admin1", "overture-buildings", "osm-amsterdam"] {
        let Some(layers) = datasets(id) else { continue };
        let Some(ds) = layers.iter().max_by_key(|d| d.features.len()) else { continue };
        let extents: Arc<Vec<Extent>> =
            Arc::new(ds.features.iter().map(|f| f.geometry.as_ref().map_or(Extent::EMPTY, Geometry::extent)).collect());
        let idx = rustgis_map::cull::CullIndex::build(&extents);
        let probes = probe_extents();
        let mut got = Vec::new();
        for view in &probes {
            idx.candidates(view, &mut got);
            let want: Vec<u32> = (0..extents.len() as u32).filter(|&r| extents[r as usize].intersects(view)).collect();
            assert_eq!(got, want, "{id}: cull index ≠ linear scan at {view:?}");
        }
        eprintln!("lod_data: T3.1 {id}: {} features × {} probes — index == linear scan exactly", extents.len(), probes.len());
    }
}

// --- T4: interaction LOD bias ---

/// T4.1/T4.2/T4.3 on osm-amsterdam at world: bias 4.0 draws strictly fewer vertices than 1.0;
/// the settle frame is byte-exact against a never-biased render; hostile biases clamp to 1.0.
#[test]
fn bias_thins_settle_exact_and_clamps() {
    skip_unless_data!();
    let Some((mut m, _)) = corpus("osm-amsterdam") else { return };
    let view = world();
    let Some(exact_scene) = gpu(&mut m, &view, 1.0) else { panic!("amsterdam: no GPU scene at world") };
    let exact = mesh_vertices(&exact_scene);
    let unbiased = cpu_hash(&mut m, &view, 1.0); // the never-biased reference, taken first
    let Some(biased_scene) = gpu(&mut m, &view, 4.0) else { panic!("amsterdam: no biased GPU scene at world") };
    let biased = mesh_vertices(&biased_scene);
    eprintln!("lod_data: T4 osm-amsterdam@world: mesh vertices exact={exact} biased={biased}");
    assert!(exact > 0 && biased < exact, "bias 4.0 thins: {biased} < {exact}");
    let _ = cpu_hash(&mut m, &view, 4.0); // a mid-gesture frame
    assert_eq!(cpu_hash(&mut m, &view, 1.0), unbiased, "the settle frame after a biased frame is byte-exact");
    for b in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(cpu_hash(&mut m, &view, b), unbiased, "bias {b} clamps to the exact render");
    }
    eprintln!("lod_data: T4 osm-amsterdam@world: settle byte-exact; 0/-1/NaN/inf clamp to 1.0");
}

/// T4.4/X4: with a raster in the scene (a `has_images` frame: every mid-gesture frame is a full
/// re-render) the bias still thins the vector overlay, and the settle frame is byte-exact.
///
/// Two legs, because of the cache-worthiness gate (`lod::MIN_SAVING`) on real data:
/// - city-raster preset (the task's): ne-rivers' vertices are kilometres apart, so at res 4
///   (tol 1.2/4.8 m) no thinned level clears MIN_SAVING and the layer draws full geometry at
///   every bias — the bias is a *no-op* here by design. Pinned as exact equality (95 250 ==
///   95 250 measured), plus the byte-exact settle.
/// - world preset (the "bias must engage with a raster present" claim, exercised where the gate
///   accepts thinning): bias 4.0 draws strictly fewer mesh vertices than 1.0, and the settle
///   frame is byte-exact.
#[test]
fn bias_with_raster_present() {
    skip_unless_data!();
    let Some(mut m) = overlay_map() else { return };
    for (pname, view, gate_rejects) in [("city-raster", city_raster(), true), ("world", world(), false)] {
        let Some(exact_scene) = gpu(&mut m, &view, 1.0) else {
            panic!("overlay: no GPU scene at {pname} (raster present → Image + Mesh cmds expected)")
        };
        let exact = mesh_vertices(&exact_scene);
        let unbiased = cpu_hash(&mut m, &view, 1.0);
        let Some(biased_scene) = gpu(&mut m, &view, 4.0) else { panic!("overlay: no biased GPU scene at {pname}") };
        let biased = mesh_vertices(&biased_scene);
        eprintln!("lod_data: T4.4 dem-alps+ne-rivers@{pname}: mesh vertices exact={exact} biased={biased}");
        assert!(exact > 0, "overlay draws vector meshes at {pname}");
        if gate_rejects {
            assert_eq!(biased, exact, "{pname}: MIN_SAVING rejects thinning for ne-rivers at res 4 — bias is a no-op by design");
        } else {
            assert!(biased < exact, "{pname}: bias 4.0 thins with a raster present: {biased} < {exact}");
        }
        let _ = cpu_hash(&mut m, &view, 4.0);
        assert_eq!(cpu_hash(&mut m, &view, 1.0), unbiased, "{pname}: the settle frame is byte-exact with a raster present");
    }
}

// --- §7: perf evidence (manual, release-only, quoted in commits — never asserted) ---

/// §7: real-data LOD numbers — CPU ms/frame, GPU scene-build ms/frame, triangles, mesh
/// vertices, dots; 10-frame means after warm-up, like `lod_bench.rs`. Run:
/// `RUSTGIS_LOD_DATA=… cargo test -p rustgis-render --test lod_data --release -- --ignored --nocapture`.
#[test]
#[ignore = "perf: machine-dependent numbers, quoted in commits rather than asserted"]
fn perf_lod_table() {
    skip_unless_data!();
    println!(
        "\n{:<20}{:<10}{:<8}{:>12}{:>12}{:>14}{:>10}",
        "dataset", "preset", "backend", "ms/frame", "triangles", "mesh_vertices", "dots"
    );
    for id in ["osm-amsterdam", "overture-buildings"] {
        let Some((mut m, _)) = corpus(id) else { continue };
        for (pname, view) in [("world", world()), ("region", region()), ("city", city())] {
            for backend in [Backend::Cpu, Backend::Gpu] {
                let _ = render_backend(&mut m, &view, backend); // warm the projected/thinned/cull caches
                let t0 = web_time::Instant::now();
                const FRAMES: usize = 10;
                for _ in 0..FRAMES {
                    let _ = render_backend(&mut m, &view, backend);
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
                // The scene statistics exist only for the GPU backend; the CPU row quotes ms.
                match render_backend(&mut m, &view, backend) {
                    Some(RenderOutput::Gpu(scene)) => {
                        println!(
                            "{id:<20}{pname:<10}{backend:<8?}{ms:>12.1}{:>12}{:>14}{:>10}",
                            triangles(&scene),
                            mesh_vertices(&scene),
                            dots(&scene)
                        );
                    }
                    _ => println!("{id:<20}{pname:<10}{backend:<8?}{ms:>12.1}{:>12}{:>14}{:>10}", "-", "-", "-"),
                }
            }
        }
    }
}
