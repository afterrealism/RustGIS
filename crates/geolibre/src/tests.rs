use super::*;
use rustgis_analysis::RasterInput;
use rustgis_data::raster::{ColorHint, GeoTransform, Raster};
use rustgis_data::{Feature, Field, FieldType, Geometry};

/// A 40 × 40 DEM rising to the east (UTM 17N, 30 m pixels) and a point inside it.
struct Fixture {
    dem: Raster,
    points: Dataset,
    /// The DEM as an embedded GeoTIFF, as a browser upload or a tool's output is kept.
    tiff: Vec<u8>,
}

impl Fixture {
    fn new() -> Fixture {
        let gt = GeoTransform { origin_x: 500_000.0, origin_y: 4_650_000.0, pixel_width: 30.0, pixel_height: -30.0 };
        let band: Vec<f32> = (0..40 * 40).map(|i| (i % 40) as f32 * 3.0 + (i / 40) as f32).collect();
        let dem = Raster::new("dem", 40, 40, vec![band], gt, Crs::from_epsg(32617).unwrap(), None, ColorHint::Gray, "Float32").unwrap();
        // The DEM's centre, in longitude/latitude.
        let centre = Crs::from_epsg(32617).unwrap().transformer().unwrap().to_lonlat(Coord::new(500_600.0, 4_649_400.0)).unwrap();
        let mut points = Dataset::new("pts", rustgis_data::GeometryType::Point, vec![Field::new("id", FieldType::Integer)]);
        points.features.push(Feature { id: 1, geometry: Some(Geometry::Point(centre)), attrs: vec![rustgis_data::Value::Int(1)] });
        let tiff = rustgis_data::raster::geotiff::write(&dem, 0).unwrap();
        Fixture { dem, points, tiff }
    }
}

impl Inputs for Fixture {
    fn dataset(&self, layer: &str) -> Option<&Dataset> {
        (layer == "pts").then_some(&self.points)
    }
    fn selection(&self, _: &str) -> &[usize] {
        &[]
    }
    fn raster(&self, layer: &str) -> Option<RasterInput<'_>> {
        match layer {
            "dem" => Some(RasterInput { raster: &self.dem, path: None, embedded: None }),
            "embedded" => Some(RasterInput { raster: &self.dem, path: None, embedded: Some(("dem.tif", &self.tiff)) }),
            _ => None,
        }
    }
}

/// The fixture, or `None` (the test is skipped) when no runner is installed: set
/// `RUSTGIS_GEOLIBRE` to a `geolibre` executable, or put one on PATH, to run these.
fn setup() -> Option<Fixture> {
    register();
    if runner::find().is_none() {
        eprintln!("skipped: no GeoLibre runner (set {})", runner::ENV);
        return None;
    }
    Some(Fixture::new())
}

#[test]
fn runner_values_and_choices() {
    assert_eq!(arg_text(&json!("a b")), "a b");
    assert_eq!(arg_text(&json!(true)), "true");
    assert_eq!(arg_text(&json!(2.5)), "2.5");
    // Not a runner: refused, and the choice stays as it was.
    let before = runner::find();
    let me = std::env::current_exe().unwrap();
    assert!(runner::version_of(&me).is_none());
    assert!(set_runner(Some(me)).is_err());
    assert!(set_runner(Some(std::path::PathBuf::from("/no/such/geolibre"))).is_err());
    assert_eq!(runner::find(), before);
    assert_eq!(label_of("z_factor"), "Z factor");
    // Reloading the tools reuses the strings it made rather than leaking new copies.
    assert!(std::ptr::eq(leak("Slope"), leak(String::from("Slope"))));
    // A point that can't be converted drops the geometry instead of writing nulls.
    let line = Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0)]]);
    assert!(convert(&line, |c| Some(Coord::new(c.x * 2.0, c.y))).is_some());
    assert!(convert(&line, |c| (c.x < 0.5).then_some(c)).is_none());
    assert!(convert(&line, |_| Some(Coord::new(f64::NAN, 0.0))).is_none());
    assert_eq!(safe("a b/c.tif"), "a_b_c_tif");
    // Only the runner Install put in place can be uninstalled, not one of the user's own.
    if let Some(dir) = install::install_dir() {
        assert!(install::is_installed(&dir.join(runner::exe_name())));
        assert!(!install::is_installed(&dir.join("mine").join(runner::exe_name())));
        assert!(!install::is_installed(&dir.join("other-program")));
    }
}

#[test]
fn manifests_become_tool_forms() {
    let Some(_) = setup() else { return };
    let tools = rustgis_analysis::external_tools_of(crate::SOURCE);
    assert!(tools.len() > 1000, "{}", tools.len());
    let slope = rustgis_analysis::tool("geolibre:slope").unwrap();
    assert!(rustgis_analysis::is_external(slope));
    // Grouped by the runner's taxonomy; a runner from before it, by the coarse category.
    assert!(matches!(slope.toolbox, "GeoLibre › Terrain › Derivatives" | "GeoLibre › Raster"), "{}", slope.toolbox);
    assert_eq!(slope.params.first().map(|p| p.kind), Some(ParamKind::RasterLayer));
    assert!(slope.params.iter().all(|p| p.name != "output"), "outputs are not form fields");
    let units = slope.params.iter().find(|p| p.name == "units").unwrap();
    assert!(matches!(units.kind, ParamKind::Choice(c) if c.contains(&"percent")));
    // Defaults are JSON text (as for the built-in tools): the form and the runner parse it.
    assert_eq!(units.default, "\"degrees\"");
    assert_eq!(serde_json::from_str::<Value>(units.default).unwrap(), json!("degrees"));
    // The built-in Buffer still wins by name.
    assert_eq!(rustgis_analysis::tool("Buffer").map(|t| t.id), Some("buffer"));
}

#[test]
fn inputs_of_several_layers_are_layer_lists() {
    // A composite takes several rasters: the form picks layers, not a bare text box (#270).
    let manifest = json!({"id": "composite", "params": [
        {"name": "inputs", "io_role": "input", "data_kind": "raster", "schema": {"cardinality": "many"}},
        {"name": "vectors", "io_role": "input", "data_kind": "vector", "schema": {"cardinality": "many"}},
        {"name": "files", "io_role": "input", "data_kind": "lidar", "schema": {"cardinality": "many"}},
        {"name": "dem", "io_role": "input", "data_kind": "raster"},
        {"name": "output", "io_role": "output", "data_kind": "raster"}
    ]});
    let (spec, _) = spec_of(&manifest).unwrap();
    let kinds: Vec<(&str, ParamKind)> = spec.params.iter().map(|p| (p.name, p.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("inputs", ParamKind::RasterLayers),
            ("vectors", ParamKind::VectorLayers),
            ("files", ParamKind::Text),
            ("dem", ParamKind::RasterLayer)
        ]
    );
}

#[test]
fn slope_runs_on_a_raster_layer() {
    let Some(f) = setup() else { return };
    let r = rustgis_analysis::run("geolibre:slope", &f, &json!({"input": "dem", "units": "degrees"})).unwrap();
    // An embedded raster is staged as a file with its extension.
    assert!(rustgis_analysis::run("geolibre:slope", &f, &json!({"input": "embedded"})).is_ok());
    let Output::Layers(layers) = r.output else { panic!("layers") };
    let Some(OutputLayer::Raster { name, bytes, file_name }) = layers.first() else { panic!("a raster: {layers:?}") };
    assert_eq!(name, "Slope");
    let out = rustgis_data::raster::read_bytes(file_name, bytes).unwrap();
    assert_eq!((out.width, out.height), (40, 40));
    assert_eq!(out.crs, Crs::from_epsg(32617).unwrap(), "keeps the DEM's coordinate system");
    // A 3 m rise per 30 m pixel is about 5.7°.
    let mid = out.bands[0].data[20 * 40 + 20];
    assert!((mid - 5.8).abs() < 0.5, "{mid}");
}

#[test]
fn vector_tools_work_in_metres_and_return_lonlat() {
    let Some(f) = setup() else { return };
    let r = rustgis_analysis::run("geolibre:buffer_vector", &f, &json!({"input": "pts", "distance": "1000"})).unwrap();
    let Output::Layers(layers) = r.output else { panic!("layers") };
    let Some(OutputLayer::Vector(ds)) = layers.first() else { panic!("a vector layer: {layers:?}") };
    let Some(Some(Geometry::Polygon(polys))) = ds.features.first().map(|f| f.geometry.clone()) else { panic!("a polygon") };
    let area = rustgis_geom::geodesic_ring_area_m2(&polys[0][0]).abs();
    let circle = std::f64::consts::PI * 1000.0 * 1000.0;
    assert!((area / circle - 1.0).abs() < 0.05, "a 1 km buffer, not 1000 degrees: {area}");
}

#[test]
fn rasters_and_points_meet_in_one_coordinate_system() {
    let Some(f) = setup() else { return };
    let r = rustgis_analysis::run("geolibre:extract_raster_values_at_points", &f, &json!({"rasters": "dem", "points": "pts"})).unwrap();
    let Output::Layers(layers) = r.output else { panic!("layers") };
    let Some(OutputLayer::Vector(ds)) = layers.first() else { panic!("points: {layers:?}") };
    let p: Option<Coord> = ds.features.first().and_then(|f| f.geometry.as_ref()).map(|g| g.extent().center());
    let centre = f.points.extent().center();
    assert!(p.is_some_and(|c| (c.x - centre.x).abs() < 1e-6 && (c.y - centre.y).abs() < 1e-6), "back in lon/lat: {p:?}");
    // The sampled value is the DEM's at its centre (column 20, row 20).
    let value = ds.fields.iter().position(|fl| fl.name != "id" && fl.name != "FID").and_then(|i| ds.features[0].attrs.get(i).cloned());
    let v = value.and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
    assert!((v - (20.0 * 3.0 + 20.0)).abs() < 3.5, "{v}");
}

#[test]
fn bad_inputs_are_errors_not_crashes() {
    let Some(f) = setup() else { return };
    assert!(rustgis_analysis::run("geolibre:slope", &f, &json!({})).unwrap_err().contains("required"));
    assert!(rustgis_analysis::run("geolibre:slope", &f, &json!({"input": "nope"})).unwrap_err().contains("no layer or file"));
    assert!(rustgis_analysis::run("geolibre:slope", &f, &json!({"input": "pts"})).is_err(), "a vector where a raster belongs");
    // Every tool with junk in every field fails cleanly: inputs that don't exist stop it before
    // it runs.
    for t in rustgis_analysis::external_tools_of(crate::SOURCE) {
        if !t.params.iter().any(|p| matches!(p.kind, ParamKind::RasterLayer | ParamKind::VectorLayer | ParamKind::Layer | ParamKind::Path))
        {
            continue;
        }
        let junk: serde_json::Map<String, Value> = t.params.iter().map(|p| (p.name.to_string(), json!("nope;(("))).collect();
        assert!(rustgis_analysis::run(t.id, &f, &Value::Object(junk)).is_err(), "{}", t.id);
    }
}

/// The browser's path, with no runner: prepare a run, then finish it from files a runner
/// would have written.
#[test]
fn prepare_and_finish_without_a_runner() {
    let f = Fixture::new();
    let manifest = json!({
        "id": "test_buffer", "display_name": "Test Buffer", "category": "Vector", "summary": "",
        "params": [
            {"name": "input", "io_role": "input", "data_kind": "vector", "required": true, "schema": {"kind": "input", "cardinality": "single"}},
            {"name": "others", "io_role": "input", "data_kind": "raster", "required": false, "schema": {"kind": "input", "cardinality": "multiple"}},
            {"name": "distance", "data_kind": "number", "required": true, "schema": {"kind": "scalar"}},
            {"name": "output", "io_role": "output", "data_kind": "vector", "required": false, "schema": {"kind": "output"}}
        ]
    });
    let (spec, meta) = spec_of(&manifest).unwrap();
    // No `taxonomy` (an older runner): the coarse category. With one: category › subcategory.
    assert_eq!(spec.toolbox, "GeoLibre › Vector");
    let placed = json!({"id": "slope", "taxonomy": {"category": "Terrain", "subcategory": "Derivatives"}});
    assert_eq!(toolbox_of(&placed, "Raster"), "GeoLibre › Terrain › Derivatives");
    assert_eq!(toolbox_of(&json!({"taxonomy": {"category": " ", "subcategory": 3}}), "Raster"), "GeoLibre › Raster");
    let params = rustgis_analysis::resolve_params(&spec, &json!({"input": "pts", "others": "dem;dem", "distance": "250"})).unwrap();
    let job = prepare_with(&spec, &meta, &f, &params).unwrap();
    // Inputs are staged as numbered files, even the same layer twice.
    let names: Vec<&str> = job.files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["in0-pts.geojson", "in1-dem.tif", "in2-dem.tif"]);
    // The working CRS is the raster's (UTM 17N): the GeoJSON says so.
    assert!(String::from_utf8_lossy(&job.files[0].1).contains("EPSG::32617"));
    assert_eq!(
        job.argv("/work"),
        [
            "--input=/work/in0-pts.geojson",
            "--others=/work/in1-dem.tif;/work/in2-dem.tif",
            "--distance=250",
            "--output=/work/output.geojson"
        ]
    );
    // Whole numbers go as integers (the form sends numbers as floats).
    assert_eq!(option_value(&json!(3.0), Data::Number), json!(3));
    assert_eq!(option_value(&json!("2.5"), Data::Number), json!(2.5));
    assert!(job.argv("C:\\scratch").iter().any(|a| a == "--output=C:\\scratch\\output.geojson"), "Windows folders");

    // Finish from what a runner returns: the output in UTM, and a JSON results line.
    let utm = Crs::from_epsg(32617).unwrap().transformer().unwrap();
    let c = utm.from_lonlat(f.points.extent().center()).unwrap();
    let out = format!(
        r#"{{"type":"FeatureCollection","features":[{{"type":"Feature","properties":{{"id":1}},"geometry":{{"type":"Point","coordinates":[{},{}]}}}}]}}"#,
        c.x, c.y
    );
    let lines = vec!["working…".to_string(), r#"{"output":"/work/output.geojson","log":"/work/my_output.geojson","count":1}"#.to_string()];
    let r = finish(&job, &[("output.geojson".into(), out.into_bytes())], &lines, None);
    let Output::Layers(layers) = r.output else { panic!("layers") };
    let Some(OutputLayer::Vector(ds)) = layers.first() else { panic!("a vector: {layers:?}") };
    assert_eq!(ds.name, "Test Buffer");
    let p = ds.extent().center();
    let want = f.points.extent().center();
    assert!((p.x - want.x).abs() < 1e-6 && (p.y - want.y).abs() < 1e-6, "back in lon/lat: {p:?}");
    assert!(r.messages.contains(&"working…".to_string()));
    assert!(r.messages.iter().any(|m| m == "Count: 1"), "results, without the output path: {:?}", r.messages);
    assert!(!r.messages.iter().any(|m| m.contains("/work/output.geojson")), "the output came back as a layer");
    assert!(r.messages.iter().any(|m| m.contains("my_output.geojson")), "another file isn't mistaken for it");
    // No files back: a message, not a crash.
    let empty = finish(&job, &[], &[], None);
    assert!(matches!(empty.output, Output::Layers(ref l) if l.is_empty()) && !empty.messages.is_empty());
}
