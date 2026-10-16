//! Vector readers and writers: GeoJSON, CSV (with longitude/latitude columns) and Shapefile.

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{Coord, DataError, Dataset, Field, FieldType, Geometry, GeometryType, ReadReport, Result, Value};

/// Largest file read into memory (256 MiB). Parsing JSON can take several times the file size.
pub const MAX_BYTES: u64 = 256 << 20;

/// File types the readers understand, for open dialogs.
pub const READ_EXTENSIONS: &[&str] = &[
    "geojson",
    "json",
    "csv",
    "tsv",
    "txt",
    "shp",
    "zip",
    "kml",
    "kmz",
    "gpx",
    "gpkg",
    "parquet",
    "geoparquet",
    "fgb",
    "xlsx",
    "xlsm",
    "xlsb",
    "xls",
    "ods",
    "rss",
    "atom",
    "georss",
    "landxml",
    "xml",
    "gml",
    "dxf",
    "mif",
    "pbf",
    "topojson",
    "las",
    "laz",
];

/// Most XML nodes parsed from one document (KML, GPX): each costs tens of bytes, and the
/// browser's memory is smaller (a 1M-point GPX track is about 8M nodes).
pub(crate) const XML_NODES: u32 = if cfg!(target_arch = "wasm32") { 8_000_000 } else { 25_000_000 };

/// A layer read from a file that holds several, with the styles its features' `style` field
/// names (KML).
pub struct LayerRead {
    pub dataset: Dataset,
    pub report: ReadReport,
    pub styles: Vec<(String, crate::kml::Style)>,
}

/// Whether a file holds several layers ([`read_layers`] reads them all): a zip, KML, KMZ or GPX.
pub fn is_multi_layer_name(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "zip" | "kml" | "kmz" | "gpx" | "rss" | "atom" | "georss" | "landxml" | "xml" | "gml" | "dxf" | "pbf" | "topojson"
    )
}

/// Reads every layer of a file held in memory: the Shapefiles and other files of a zip, a KML or
/// KMZ's points, lines and polygons (with their styles), or a GPX's waypoints, routes, tracks and
/// track points. Other files give their one layer.
pub fn read_layers(file_name: &str, bytes: &[u8]) -> Result<Vec<LayerRead>> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({} bytes)", bytes.len())));
    }
    let ext = Path::new(file_name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let plain =
        |v: Vec<(Dataset, ReadReport)>| v.into_iter().map(|(dataset, report)| LayerRead { dataset, report, styles: Vec::new() }).collect();
    let mut out: Vec<LayerRead> = match ext.as_str() {
        "zip" => plain(read_zip(file_name, bytes)?),
        "kml" | "kmz" => crate::kml::read(file_name, bytes)?
            .into_iter()
            .map(|l| LayerRead { dataset: l.dataset, report: l.report, styles: l.styles })
            .collect(),
        "gpx" => plain(crate::gpx::read(file_name, bytes)?),
        "rss" | "atom" | "georss" => plain(crate::georss::read(file_name, bytes)?),
        "landxml" => plain(crate::landxml::read(file_name, bytes, None)?),
        // Plain XML: whichever of the XML formats its contents say.
        "xml" => {
            let text = String::from_utf8_lossy(bytes.get(..bytes.len().min(8192)).unwrap_or(bytes));
            if crate::landxml::looks_like_landxml(&text) {
                plain(crate::landxml::read(file_name, bytes, None)?)
            } else if crate::georss::looks_like_georss(&text) {
                plain(crate::georss::read(file_name, bytes)?)
            } else if text.contains("<gpx") {
                plain(crate::gpx::read(file_name, bytes)?)
            } else if text.contains("<kml") {
                crate::kml::read(file_name, bytes)?
                    .into_iter()
                    .map(|l| LayerRead { dataset: l.dataset, report: l.report, styles: l.styles })
                    .collect()
            } else {
                return Err(DataError::Format(format!(
                    "{file_name}: an XML file RustGIS doesn't read (it reads GeoRSS, LandXML, KML and GPX)"
                )));
            }
        }
        "gml" => plain(crate::gml::read(file_name, bytes)?),
        "dxf" => plain(crate::dxf::read(file_name, bytes, &crate::dxf::Options::default())?),
        // OpenStreetMap PBF, not a vector tile (`.pbf` too).
        "pbf" if crate::osm::looks_like_osm_pbf(bytes) => plain(crate::osm::read(file_name, bytes)?),
        "pbf" => {
            return Err(DataError::Unsupported(format!(
                "{file_name}: a .pbf that isn't OpenStreetMap data (vector tiles are added with Add Tile Layer)"
            )));
        }
        // A TopoJSON topology, named .topojson or .json.
        "topojson" | "json" if ext == "topojson" || std::str::from_utf8(bytes).is_ok_and(crate::topojson::looks_like_topojson) => {
            let text = std::str::from_utf8(bytes).map_err(|_| DataError::Format(format!("{file_name} is not UTF-8 text")))?;
            plain(crate::topojson::read(file_name, text)?)
        }
        _ => plain(vec![read_bytes(file_name, bytes)?]),
    };
    if out.is_empty() {
        return Err(DataError::Format(format!("{file_name}: nothing to draw")));
    }
    for l in &mut out {
        l.dataset.source.get_or_insert_with(|| file_name.to_string());
    }
    Ok(out)
}

/// The extension of a file from its first bytes, for a download whose address doesn't say:
/// GeoParquet, FlatGeobuf, GeoTIFF, a zip, a GeoPackage, KML, GPX, GeoJSON or delimited text
/// (`None` for anything else, such as a web page).
pub fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    let head = bytes.get(..bytes.len().min(4096)).unwrap_or(bytes);
    let text = String::from_utf8_lossy(head);
    let start = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    let mut lines = start.lines().filter(|l| !l.trim().is_empty());
    let (first_line, second_line) = (lines.next().unwrap_or(""), lines.next());
    // A separator splitting the first two lines alike (a header and a row): not a sentence with a
    // comma, such as a plain-text error.
    let split_alike = second_line.is_some_and(|second| {
        [',', ';', '\t', '|'].iter().any(|&c| {
            let n = first_line.matches(c).count();
            n > 0 && second.matches(c).count() == n
        })
    });
    Some(match bytes {
        [b'P', b'A', b'R', b'1', ..] => "parquet",
        [b'f', b'g', b'b', ..] => "fgb",
        [b'I', b'I', 42 | 43, 0, ..] | [b'M', b'M', 0, 42 | 43, ..] => "tif",
        [b'P', b'K', 3, 4, ..] => "zip",
        _ if bytes.starts_with(b"SQLite format 3\0") => "gpkg",
        // LiDAR: LAZ marks its point format's top bits.
        _ if crate::lidar::looks_like_las(bytes) => {
            if bytes.get(104).is_some_and(|f| f & 0xC0 != 0) {
                "laz"
            } else {
                "las"
            }
        }
        _ if start.starts_with('{') || start.starts_with('[') => "geojson",
        _ if start.starts_with('<') && text.contains("<kml") => "kml",
        _ if start.starts_with('<') && text.contains("<gpx") => "gpx",
        // Text whose first two lines a separator splits alike, and no control characters.
        _ if !start.starts_with('<') && split_alike && !head.iter().any(|&b| b < 9 || (b > 13 && b < 32)) => "csv",
        _ => return None,
    })
}

/// The first layer of a file holding several ([`read_layers`]), saying what else it holds.
fn first_layer(file_name: &str, bytes: &[u8]) -> Result<(Dataset, ReadReport)> {
    let layers = read_layers(file_name, bytes)?;
    let rest: Vec<String> = layers.iter().skip(1).map(|l| l.dataset.name.clone()).collect();
    let first = layers.into_iter().next().ok_or_else(|| DataError::Format(format!("{file_name}: empty")))?;
    let mut first = (first.dataset, first.report);
    if !rest.is_empty() {
        let leaf = Path::new(file_name).file_name().and_then(|n| n.to_str()).unwrap_or(file_name);
        first.1.messages.push(format!("{leaf}: read its first layer; it also holds {}", rest.join(", ")));
    }
    Ok(first)
}

/// Reads vector data held in memory (a browser upload or a drop), choosing the reader by the
/// file name's extension. A Shapefile needs its sidecar files, so it comes zipped (`.zip`, whose
/// first layer is read).
pub fn read_bytes(file_name: &str, bytes: &[u8]) -> Result<(Dataset, ReadReport)> {
    let path = Path::new(file_name);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    if bytes.len() as u64 > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({} bytes)", bytes.len())));
    }
    let text = || std::str::from_utf8(bytes).map_err(|_| DataError::Format(format!("{file_name} is not UTF-8 text")));
    let mut out = match ext.as_str() {
        // A TopoJSON topology named .json: its first object, saying what else it holds.
        "json" if crate::topojson::looks_like_topojson(text()?) => return first_layer(file_name, bytes),
        "geojson" | "json" => read_geojson(text()?, &name)?,
        "csv" | "tsv" | "txt" => read_csv(text()?, &name)?,
        // A MIF without its .mid (an upload of the one file): no attributes.
        "mif" => crate::mapinfo::read_bytes(bytes, None, &name)?,
        "shp" => return Err(DataError::Unsupported("shp on its own: zip it with its .dbf, .shx and .prj and add the zip".into())),
        // The first layer (`read_layers` reads them all), saying what else the file holds.
        "zip" | "kml" | "kmz" | "gpx" | "gml" | "dxf" | "pbf" | "topojson" => return first_layer(file_name, bytes),
        // A copy, since only borrowed bytes are given; `read_owned` takes them without one.
        e if crate::source::EXTENSIONS.contains(&e) => return read_owned(file_name, bytes.to_vec()),
        other => return Err(DataError::Unsupported(other.to_string())),
    };
    out.0.source = Some(file_name.to_string());
    Ok(out)
}

/// Reads a GeoPackage, GeoParquet or FlatGeobuf held in memory without copying it (other types go
/// to [`read_bytes`]).
pub fn read_owned(file_name: &str, bytes: Vec<u8>) -> Result<(Dataset, ReadReport)> {
    if !crate::source::is_source_name(file_name) {
        return read_bytes(file_name, &bytes);
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({} bytes)", bytes.len())));
    }
    let mut out = crate::source::materialize(crate::source::open_bytes(file_name, bytes, None)?.as_ref())?;
    out.0.source = Some(file_name.to_string());
    Ok(out)
}

/// Reads a vector file by extension.
pub fn read_path(path: &Path) -> Result<(Dataset, ReadReport)> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    let mut out = match ext.as_str() {
        "geojson" | "json" => {
            check_size(path)?;
            let text = std::fs::read_to_string(path)?;
            // A TopoJSON topology named .json, as read_bytes reads it.
            if ext == "json" && crate::topojson::looks_like_topojson(&text) {
                let leaf = path.file_name().and_then(|n| n.to_str()).unwrap_or("layer.json");
                first_layer(leaf, text.as_bytes())?
            } else {
                read_geojson(&text, &name)?
            }
        }
        "csv" | "tsv" | "txt" => {
            check_size(path)?;
            read_csv(&std::fs::read_to_string(path)?, &name)?
        }
        "shp" => {
            check_size(path)?;
            let dbf = path.with_extension("dbf");
            if dbf.exists() {
                check_size(&dbf)?;
            }
            read_shapefile(path, &name)?
        }
        "zip" | "kml" | "kmz" | "gpx" | "gml" | "dxf" | "pbf" | "topojson" => {
            check_size(path)?;
            read_bytes(&path.display().to_string(), &std::fs::read(path)?)?
        }
        // With its .mid beside it (spelt mid, MID or Mid).
        "mif" => {
            check_size(path)?;
            let mid = ["mid", "MID", "Mid"].iter().map(|e| path.with_extension(e)).find(|p| p.is_file());
            let mid = match mid {
                Some(m) => {
                    check_size(&m)?;
                    Some(std::fs::read(m)?)
                }
                None => None,
            };
            crate::mapinfo::read_bytes(&std::fs::read(path)?, mid.as_deref(), &name)?
        }
        // Read in place by a provider, so there's no size cap on the file, only on the features.
        e if crate::source::EXTENSIONS.contains(&e) => crate::source::materialize(crate::source::open_path(path, None)?.as_ref())?,
        other => return Err(DataError::Unsupported(other.to_string())),
    };
    out.0.source = Some(path.display().to_string());
    Ok(out)
}

fn check_size(path: &Path) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({len} bytes)")));
    }
    Ok(())
}

/// The field KML and GPX name placemarks by: one called `name`, if any.
fn name_field(ds: &Dataset) -> Option<usize> {
    ds.fields.iter().position(|f| f.name.eq_ignore_ascii_case("name"))
}

/// Features a format that holds only features with geometry leaves out.
fn without_geometry(ds: &Dataset) -> usize {
    ds.features.iter().filter(|f| f.geometry.is_none()).count()
}

/// A dataset encoded in the format of extension `ext`, as one file, and the number of features the
/// format couldn't hold. A Shapefile (`shp` or `zip`) comes as a zip of its files, named after the
/// dataset. GeoPackage needs a file ([`write_path`]).
pub fn write_bytes(ds: &Dataset, ext: &str) -> Result<(Vec<u8>, usize)> {
    let fmt = |e: serde_json::Error| DataError::Format(e.to_string());
    Ok(match ext.to_ascii_lowercase().as_str() {
        "geojson" | "json" => (serde_json::to_vec(&to_geojson(ds)).map_err(fmt)?, 0),
        "csv" => (to_csv(ds)?.into_bytes(), 0),
        "fgb" => (crate::source::fgb::write(ds)?, without_geometry(ds)),
        "parquet" | "geoparquet" => (crate::source::parquet::write(ds)?, 0),
        "kml" => (crate::interop::to_kml(ds, name_field(ds)).into_bytes(), without_geometry(ds)),
        "kmz" => {
            let kml = crate::interop::to_kml(ds, name_field(ds)).into_bytes();
            (crate::zip::write(&[("doc.kml".to_string(), kml)]).map_err(DataError::Format)?, without_geometry(ds))
        }
        "gpx" => (crate::interop::to_gpx(ds, name_field(ds)).into_bytes(), without_geometry(ds)),
        "shp" | "zip" => {
            let (parts, left_out) = shapefile_parts(ds)?;
            let base: String = ds.name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
            let base = if base.trim().is_empty() { "layer".to_string() } else { base };
            let entries: Vec<(String, Vec<u8>)> = parts.into_iter().map(|(e, b)| (format!("{base}.{e}"), b)).collect();
            (crate::zip::write(&entries).map_err(DataError::Format)?, left_out)
        }
        other => return Err(DataError::Unsupported(other.to_string())),
    })
}

/// Writes a dataset by extension (`.geojson`/`.json`, `.csv`, `.fgb`, `.gpkg` on the desktop,
/// `.parquet`/`.geoparquet`, `.shp`, `.zip`, `.kml`, `.kmz` or `.gpx`); returns the number of features the
/// format couldn't hold (FlatGeobuf, KML, GPX and Shapefile hold only features with geometry). A
/// Shapefile is written as its `.shp`, `.shx`, `.dbf`, `.prj` and `.cpg` files (or zipped, to a
/// `.zip`).
pub fn write_path(ds: &Dataset, path: &Path) -> Result<usize> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "gpkg" => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                crate::source::gpkg::write(ds, path)?;
                Ok(0)
            }
            #[cfg(target_arch = "wasm32")]
            Err(DataError::Unsupported("GeoPackage in the browser".into()))
        }
        "shp" => {
            let (parts, left_out) = shapefile_parts(ds)?;
            // Every part is written to a temporary file first and renamed into place only once all
            // are written, the .shp last, so a failed write leaves the old Shapefile whole.
            let mut written: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
            let staged = parts.iter().try_for_each(|(e, bytes)| {
                // The .shp keeps the name given (`Roads.SHP` stays so); the others take its stem.
                let target = if *e == "shp" { path.to_path_buf() } else { path.with_extension(e) };
                let tmp = temp_beside(&target)?;
                written.push((tmp.clone(), target));
                write_new(&tmp, bytes)
            });
            if let Err(e) = staged {
                for (tmp, _) in &written {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err(e.into());
            }
            written.sort_by_key(|(_, target)| target == path);
            // The old parts are moved aside first and put back if any new part fails to land, so
            // the old Shapefile stays a matching set.
            let mut backups: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
            let mut placed: Vec<std::path::PathBuf> = Vec::new();
            let mut result = Ok(());
            for (tmp, target) in &written {
                if target.exists() {
                    // Any failure here rolls back like a failed rename.
                    let bak = match temp_beside(target) {
                        Ok(b) => b,
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    };
                    if let Err(e) = std::fs::rename(target, &bak) {
                        result = Err(e);
                        break;
                    }
                    backups.push((bak, target.clone()));
                }
                if let Err(e) = std::fs::rename(tmp, target) {
                    result = Err(e);
                    break;
                }
                placed.push(target.clone());
            }
            if let Err(e) = result {
                for p in &placed {
                    let _ = std::fs::remove_file(p);
                }
                for (bak, target) in &backups {
                    let _ = std::fs::rename(bak, target);
                }
                for (tmp, _) in &written {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err(e.into());
            }
            for (bak, _) in &backups {
                let _ = std::fs::remove_file(bak);
            }
            // A spatial index left from an older file of the same name would no longer match.
            for stale in ["qix", "sbn", "sbx"] {
                let _ = std::fs::remove_file(path.with_extension(stale));
            }
            Ok(left_out)
        }
        _ => {
            let (bytes, left_out) = write_bytes(ds, &ext)?;
            write_atomic(path, &bytes)?;
            Ok(left_out)
        }
    }
}

/// A new temporary name beside `path`, unique per call, so concurrent writes to the same target
/// never share one.
pub(crate) fn temp_beside(path: &Path) -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Some(name) = path.file_name() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file path", path.display())));
    };
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    Ok(path.with_file_name(tmp_name))
}

/// Writes and syncs a new file (`create_new`, so an existing file or symlink is never followed);
/// removes it on failure.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let res = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(path);
    }
    res
}

/// Writes through a temporary file that is synced and renamed into place, so an interrupted
/// write never truncates an existing file. The temporary file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(path)?;
    write_new(&tmp, bytes)?;
    let res = std::fs::rename(&tmp, path);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

// ---------------------------------------------------------------------------------------------
// GeoJSON (RFC 7946)

fn coord(v: &Json) -> Option<Coord> {
    let a = v.as_array()?;
    let c = Coord::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    c.is_finite().then_some(c)
}

fn coords(v: &Json) -> Option<Vec<Coord>> {
    v.as_array()?.iter().map(coord).collect()
}

fn rings(v: &Json) -> Option<Vec<Vec<Coord>>> {
    v.as_array()?.iter().map(coords).collect()
}

/// Parses a GeoJSON geometry object (GeometryCollection keeps its first member).
pub fn geometry_from_geojson(g: &Json) -> Option<Geometry> {
    let c = g.get("coordinates");
    match g.get("type")?.as_str()? {
        "Point" => coord(c?).map(Geometry::Point),
        "MultiPoint" => coords(c?).map(Geometry::MultiPoint),
        "LineString" => coords(c?).map(|l| Geometry::Line(vec![l])),
        "MultiLineString" => rings(c?).map(Geometry::Line),
        "Polygon" => rings(c?).map(|r| Geometry::Polygon(vec![r])),
        "MultiPolygon" => c?.as_array()?.iter().map(rings).collect::<Option<Vec<_>>>().map(Geometry::Polygon),
        "GeometryCollection" => g.get("geometries")?.as_array()?.iter().find_map(geometry_from_geojson),
        _ => None,
    }
}

pub fn geometry_to_geojson(g: &Geometry) -> Json {
    fn c(c: &Coord) -> Json {
        json!([c.x, c.y])
    }
    fn l(v: &[Coord]) -> Json {
        Json::Array(v.iter().map(c).collect())
    }
    match g {
        Geometry::Point(p) => json!({"type": "Point", "coordinates": c(p)}),
        Geometry::MultiPoint(v) => json!({"type": "MultiPoint", "coordinates": l(v)}),
        Geometry::Line(parts) if let [only] = parts.as_slice() => json!({"type": "LineString", "coordinates": l(only)}),
        Geometry::Line(parts) => json!({"type": "MultiLineString", "coordinates": parts.iter().map(|p| l(p)).collect::<Vec<_>>()}),
        Geometry::Polygon(polys) if let [only] = polys.as_slice() => {
            json!({"type": "Polygon", "coordinates": only.iter().map(|r| l(r)).collect::<Vec<_>>()})
        }
        Geometry::Polygon(polys) => json!({"type": "MultiPolygon", "coordinates": polys
            .iter()
            .map(|rings| rings.iter().map(|r| l(r)).collect::<Vec<_>>())
            .collect::<Vec<_>>()}),
    }
}

/// Reads a GeoJSON FeatureCollection, Feature or bare geometry.
///
/// A FeatureCollection is read a feature at a time: each one is parsed, turned into a feature and
/// dropped before the next, so the whole document is never held as JSON values.
pub fn read_geojson(text: &str, name: &str) -> Result<(Dataset, ReadReport)> {
    let mut features = Collector::default();
    match stream_features(text, &mut features) {
        Ok(rest) => match rest.get("type").and_then(Json::as_str) {
            Some("FeatureCollection") => Ok(features.finish(name)),
            Some(_) => read_geojson_value(Json::Object(rest), name),
            None => Err(DataError::Format("GeoJSON: missing \"type\"".into())),
        },
        // Not an object, or "features" not an array: read whole, as before.
        Err(_) => read_geojson_value(serde_json::from_str(text).map_err(|e| DataError::Format(format!("GeoJSON: {e}")))?, name),
    }
}

/// What takes a FeatureCollection's features as they're read.
pub(crate) trait FeatureSink {
    /// The `"features"` member starts (again, when a document repeats it: the last one wins).
    fn restart(&mut self);
    /// One element of `"features"`, as its text.
    fn feature(&mut self, raw: &str);
}

/// Reads a GeoJSON document's top level, handing each element of `"features"` to `sink` as it is
/// read; returns the other members. Fails for a document that isn't an object, or whose
/// `"features"` isn't an array.
pub(crate) fn stream_features(text: &str, sink: &mut dyn FeatureSink) -> std::result::Result<Map<String, Json>, serde_json::Error> {
    use serde::de::DeserializeSeed;
    let mut de = serde_json::Deserializer::from_str(text);
    let rest = Streamed { sink }.deserialize(&mut de)?;
    // Only whitespace may follow, as when read whole.
    de.end()?;
    Ok(rest)
}

/// The top level of a GeoJSON document: its members, but for `"features"`, which go to a sink.
struct Streamed<'a> {
    sink: &'a mut dyn FeatureSink,
}

impl<'de> serde::de::DeserializeSeed<'de> for Streamed<'_> {
    type Value = Map<String, Json>;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Streamed<'_> {
    type Value = Map<String, Json>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a GeoJSON object")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Self::Value, A::Error> {
        let mut rest = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if key == "features" {
                self.sink.restart();
                map.next_value_seed(Features { sink: &mut *self.sink })?;
            } else {
                rest.insert(key, map.next_value()?);
            }
        }
        Ok(rest)
    }
}

/// A FeatureCollection's `"features"` array, a feature at a time.
struct Features<'a> {
    sink: &'a mut dyn FeatureSink,
}

impl<'de> serde::de::DeserializeSeed<'de> for Features<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Features<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of features")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while let Some(raw) = seq.next_element::<&'de serde_json::value::RawValue>()? {
            self.sink.feature(raw.get());
        }
        Ok(())
    }
}

/// A GeoJSON layer's schema as features are read: the union of property keys in first-seen
/// order. A type stays unknown until a non-null value is seen (so `[null, 1, 2]` is Integer),
/// then widens; all-null columns become Text.
#[derive(Default)]
pub(crate) struct Schema {
    order: Vec<(String, Option<FieldType>)>,
    index: std::collections::HashMap<String, usize>,
}

impl Schema {
    /// Counts a property; returns its field's index.
    pub(crate) fn see(&mut self, k: &str, v: &Json) -> usize {
        let ty = json_type(v);
        match self.index.get(k) {
            Some(&i) => {
                if let Some((_, cur)) = self.order.get_mut(i) {
                    *cur = match *cur {
                        None => ty,
                        Some(c) => Some(widen(c, ty)),
                    };
                }
                i
            }
            None => {
                self.index.insert(k.to_string(), self.order.len());
                self.order.push((k.to_string(), ty));
                self.order.len() - 1
            }
        }
    }

    /// The fields, typed by every value seen.
    pub(crate) fn fields(&self) -> Vec<Field> {
        self.order.iter().map(|(k, ty)| Field::new(k.clone(), ty.unwrap_or(FieldType::Text))).collect()
    }
}

/// A feature's attributes from its properties, for `fields` (see [`Schema::fields`]).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn geojson_attrs(f: &Json, fields: &[Field]) -> Vec<Value> {
    let props = f.get("properties").and_then(Json::as_object);
    fields
        .iter()
        .map(|field| coerce(props.and_then(|p| p.get(&field.name)).map(Value::from_json).unwrap_or(Value::Null), field.ty))
        .collect()
}

/// A feature's geometry: `Ok(None)` for none, `Err` for one that can't be read.
pub(crate) fn geojson_geometry(f: &Json) -> std::result::Result<Option<Geometry>, ()> {
    match f.get("geometry").filter(|g| !g.is_null()) {
        None => Ok(None),
        Some(g) => geometry_from_geojson(g).map(Some).ok_or(()),
    }
}

/// Features as they're read, and the schema so far.
#[derive(Default)]
struct Collector {
    schema: Schema,
    /// Each feature's geometry and its values by field, typed once every feature is in.
    features: Vec<(Option<Geometry>, Vec<(usize, Value)>)>,
    skipped: usize,
}

impl FeatureSink for Collector {
    fn restart(&mut self) {
        *self = Collector::default();
    }

    fn feature(&mut self, raw: &str) {
        // Valid JSON, but its numbers may not fit (1e400): such a feature is skipped and counted.
        match serde_json::from_str::<Json>(raw) {
            Ok(f) => self.add(&f),
            Err(_) => self.skipped += 1,
        }
    }
}

impl Collector {
    fn add(&mut self, f: &Json) {
        let props = f.get("properties").and_then(Json::as_object);
        // Every feature's properties count in the schema, those with unreadable geometry too.
        let attrs: Vec<(usize, Value)> = props.into_iter().flatten().map(|(k, v)| (self.schema.see(k, v), Value::from_json(v))).collect();
        match geojson_geometry(f) {
            Ok(geom) => self.features.push((geom, attrs)),
            Err(()) => self.skipped += 1,
        }
    }

    fn finish(self, name: &str) -> (Dataset, ReadReport) {
        let mut report = ReadReport { skipped: self.skipped, ..ReadReport::default() };
        let mut ds = Dataset::new(name, GeometryType::None, self.schema.fields());
        ds.features.reserve(self.features.len());
        for (geom, values) in self.features {
            let mut attrs = vec![Value::Null; ds.fields.len()];
            for (i, v) in values {
                if let (Some(slot), Some(field)) = (attrs.get_mut(i), ds.fields.get(i)) {
                    *slot = coerce(v, field.ty);
                }
            }
            ds.push_next(geom, attrs);
        }
        ds.geometry_type = dominant_type(&ds);
        if report.skipped > 0 {
            report.messages.push(format!("{} features had unreadable geometry", report.skipped));
        }
        (ds, report)
    }
}

/// Reads a GeoJSON document already parsed: a FeatureCollection, Feature or bare geometry.
fn read_geojson_value(root: Json, name: &str) -> Result<(Dataset, ReadReport)> {
    let mut features = Collector::default();
    match root.get("type").and_then(Json::as_str) {
        Some("FeatureCollection") => {
            for f in root.get("features").and_then(Json::as_array).into_iter().flatten() {
                features.add(f);
            }
        }
        Some("Feature") => features.add(&root),
        Some(_) => features.add(&json!({"type": "Feature", "geometry": &root, "properties": {}})),
        None => return Err(DataError::Format("GeoJSON: missing \"type\"".into())),
    }
    Ok(features.finish(name))
}

fn json_type(v: &Json) -> Option<FieldType> {
    match v {
        Json::Bool(_) => Some(FieldType::Boolean),
        Json::Number(n) if n.is_i64() => Some(FieldType::Integer),
        Json::Number(_) => Some(FieldType::Double),
        Json::Null => None,
        _ => Some(FieldType::Text),
    }
}

fn widen(a: FieldType, b: Option<FieldType>) -> FieldType {
    match (a, b) {
        (a, None) => a,
        (a, Some(b)) if a == b => a,
        (FieldType::Integer, Some(FieldType::Double)) | (FieldType::Double, Some(FieldType::Integer)) => FieldType::Double,
        _ => FieldType::Text,
    }
}

fn coerce(v: Value, ty: FieldType) -> Value {
    match (ty, v) {
        (_, Value::Null) => Value::Null,
        (FieldType::Double, Value::Int(i)) => Value::Float(i as f64),
        (FieldType::Text, v @ Value::Text(_)) => v,
        (FieldType::Text, v) => Value::Text(v.to_string()),
        (_, v) => v,
    }
}

pub(crate) fn dominant_type(ds: &Dataset) -> GeometryType {
    let mut counts = [0usize; 3];
    for g in ds.features.iter().filter_map(|f| f.geometry.as_ref()) {
        match g.kind() {
            GeometryType::Point => counts[0] += 1,
            GeometryType::Polyline => counts[1] += 1,
            GeometryType::Polygon => counts[2] += 1,
            GeometryType::None => {}
        }
    }
    match counts.iter().enumerate().max_by_key(|(_, n)| **n) {
        Some((_, 0)) | None => GeometryType::None,
        Some((0, _)) => GeometryType::Point,
        Some((1, _)) => GeometryType::Polyline,
        Some(_) => GeometryType::Polygon,
    }
}

pub fn to_geojson(ds: &Dataset) -> Json {
    let features: Vec<Json> = ds
        .features
        .iter()
        .map(|f| {
            let mut props = Map::new();
            for (field, v) in ds.fields.iter().zip(&f.attrs) {
                props.insert(field.name.clone(), v.to_json());
            }
            json!({"type": "Feature", "id": f.id, "properties": props, "geometry": f.geometry.as_ref().map(geometry_to_geojson)})
        })
        .collect();
    json!({"type": "FeatureCollection", "name": ds.name, "features": features})
}

// ---------------------------------------------------------------------------------------------
// CSV

const LON_NAMES: &[&str] = &["lon", "lng", "long", "longitude", "x", "lon_dd", "longitude_dd"];
const LAT_NAMES: &[&str] = &["lat", "latitude", "y", "lat_dd", "latitude_dd"];

/// Projected coordinate columns, recognised as well as longitude/latitude ones.
const X_NAMES: &[&str] = &["easting", "point_x", "xcoord", "x_coord"];
const Y_NAMES: &[&str] = &["northing", "point_y", "ycoord", "y_coord"];
/// Columns that may hold WKT geometry.
const WKT_NAMES: &[&str] = &["wkt", "geometry", "geom", "the_geom", "wkb_geometry", "shape", "ewkt"];

/// How to read a delimited text file; everything left out is found from the file.
#[derive(Clone, Debug, Default)]
pub struct CsvOptions {
    /// The column of x (longitude, easting) values.
    pub x: Option<String>,
    /// The column of y (latitude, northing) values.
    pub y: Option<String>,
    /// A column of WKT (or EWKT) geometry.
    pub wkt: Option<String>,
    /// The coordinate system of the coordinates (otherwise an EWKT SRID, or longitude/latitude).
    pub crs: Option<crate::raster::Crs>,
    /// The field separator (otherwise the most common of `,`, `;`, tab and `|` in the header).
    pub delimiter: Option<u8>,
}

/// The separator of a delimited text file: the most common of `,`, `;`, tab and `|` in its first
/// line, outside quotes (`,` when none is).
pub fn sniff_delimiter(text: &str) -> u8 {
    let seps = *b",;\t|";
    // Each separator's count on each of the first lines, outside quotes.
    let counts: Vec<[usize; 4]> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .map(|line| {
            let mut c = [0usize; 4];
            let mut quoted = false;
            for ch in line.chars() {
                match ch {
                    '"' => quoted = !quoted,
                    ',' if !quoted => c[0] += 1,
                    ';' if !quoted => c[1] += 1,
                    '\t' if !quoted => c[2] += 1,
                    '|' if !quoted => c[3] += 1,
                    _ => {}
                }
            }
            c
        })
        .collect();
    let Some(header) = counts.first() else { return b',' };
    // One that splits every line into the same number of columns (as `Name; Surname` in a
    // one-column file doesn't, once there are rows); the most columns wins, a tie keeps the
    // order above, so the comma.
    let consistent = |i: usize| header[i] > 0 && counts.iter().all(|c| c[i] == header[i]);
    let pick = (0..4).filter(|&i| counts.len() > 1 && consistent(i)).fold(None, |b: Option<usize>, i| match b {
        Some(b) if header[b] >= header[i] => Some(b),
        _ => Some(i),
    });
    // Otherwise (a header alone, or ragged rows): the comma if the header has one, else the most
    // common.
    let pick = pick.unwrap_or_else(|| if header[0] > 0 { 0 } else { (0..4).fold(0, |b, i| if header[i] > header[b] { i } else { b }) });
    seps.get(pick).copied().unwrap_or(b',')
}

/// The x/y columns found by name, as a pair from one family: longitude/latitude, else
/// easting/northing.
fn found_xy(headers: &[String]) -> Option<(usize, usize)> {
    let find = |names: &[&str]| headers.iter().position(|h| names.contains(&h.trim().to_ascii_lowercase().as_str()));
    find(LON_NAMES).zip(find(LAT_NAMES)).or_else(|| find(X_NAMES).zip(find(Y_NAMES)))
}

/// A WKT column found: one named like geometry whose first value reads as WKT.
fn found_wkt(headers: &[String], rows: &[Vec<String>]) -> Option<usize> {
    // The name first: looking for a column's first value costs a pass over the rows.
    headers.iter().enumerate().filter(|(_, h)| WKT_NAMES.contains(&h.trim().to_ascii_lowercase().as_str())).find_map(|(i, _)| {
        let first = rows.iter().filter_map(|r| r.get(i)).map(|s| s.trim()).find(|s| !s.is_empty())?;
        crate::interop::parse_wkt(first).is_ok().then_some(i)
    })
}

/// The columns of delimited text or a sheet, and those its geometry is read from when none are
/// given (see [`read_csv_with`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Columns {
    pub names: Vec<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub wkt: Option<String>,
    /// The field separator (delimited text only).
    pub delimiter: Option<u8>,
}

/// The rows [`csv_columns`] reads: a WKT column is found by its first value.
const COLUMN_SAMPLE_ROWS: usize = 1000;

/// The columns of rows under a header row, as [`read_rows`] would find them.
pub(crate) fn columns_of(headers: &[String], rows: &[Vec<String>]) -> Columns {
    let xy = found_xy(headers);
    let wkt = if xy.is_some() { None } else { found_wkt(headers, rows) };
    let name = |i: usize| headers.get(i).map(|h| h.trim().to_string());
    Columns {
        names: headers.iter().map(|h| h.trim().to_string()).collect(),
        x: xy.and_then(|(x, _)| name(x)),
        y: xy.and_then(|(_, y)| name(y)),
        wkt: wkt.and_then(name),
        delimiter: None,
    }
}

/// The columns of delimited text (split at `delimiter`, or the one found), without reading all
/// of it: a WKT column whose first value is past the first [`COLUMN_SAMPLE_ROWS`] rows isn't found.
pub fn csv_columns(text: &str, delimiter: Option<u8>) -> Result<Columns> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = delimiter.unwrap_or_else(|| sniff_delimiter(text));
    let mut rdr = csv::ReaderBuilder::new().flexible(true).delimiter(delimiter).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| DataError::Format(format!("CSV: {e}")))?.iter().map(|s| s.to_string()).collect();
    let rows: Vec<Vec<String>> =
        rdr.records().flatten().take(COLUMN_SAMPLE_ROWS).map(|r| r.iter().map(|s| s.to_string()).collect()).collect();
    Ok(Columns { delimiter: Some(delimiter), ..columns_of(&headers, &rows) })
}

/// Reads CSV; columns named like longitude/latitude become point geometry.
pub fn read_csv(text: &str, name: &str) -> Result<(Dataset, ReadReport)> {
    read_csv_with(text, name, &CsvOptions::default())
}

/// Reads delimited text: points from x/y columns (longitude/latitude unless `crs` says
/// otherwise), or geometry from a WKT column; without either it is a table.
pub fn read_csv_with(text: &str, name: &str, opts: &CsvOptions) -> Result<(Dataset, ReadReport)> {
    // Spreadsheet exports often start with a byte-order mark, which would hide the first header.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = opts.delimiter.unwrap_or_else(|| sniff_delimiter(text));
    let mut rdr = csv::ReaderBuilder::new().flexible(true).delimiter(delimiter).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| DataError::Format(format!("CSV: {e}")))?.iter().map(|s| s.to_string()).collect();
    let mut malformed = 0usize;
    let rows: Vec<Vec<String>> = rdr
        .records()
        .filter_map(|r| match r {
            Ok(r) => Some(r.iter().map(|s| s.to_string()).collect()),
            Err(_) => {
                malformed += 1;
                None
            }
        })
        .collect();
    read_rows(&headers, &rows, malformed, name, opts)
}

/// Rows of text cells under a header row (delimited text, a spreadsheet's sheet) as a dataset, as
/// [`read_csv_with`] describes; `malformed` rows were already passed over.
pub(crate) fn read_rows(
    headers: &[String],
    rows: &[Vec<String>],
    malformed: usize,
    name: &str,
    opts: &CsvOptions,
) -> Result<(Dataset, ReadReport)> {
    let named = |n: &Option<String>, what: &str| -> Result<Option<usize>> {
        match n.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => headers
                .iter()
                .position(|h| h.trim().eq_ignore_ascii_case(n))
                .map(Some)
                .ok_or_else(|| DataError::Format(format!("no {what} column '{n}' (columns: {})", headers.join(", ")))),
            None => Ok(None),
        }
    };
    let (x, y) = (named(&opts.x, "x")?, named(&opts.y, "y")?);
    let wkt_given = named(&opts.wkt, "WKT")?;
    if wkt_given.is_some() && (x.is_some() || y.is_some()) {
        return Err(DataError::Format("give x and y columns, or a WKT column, not both".into()));
    }
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (Some(x), Some(y)),
        (None, None) if wkt_given.is_none() => found_xy(headers).unzip(),
        (None, None) => (None, None),
        _ => return Err(DataError::Format("give both the x and the y column".into())),
    };
    let xy = x.zip(y);
    // A WKT column: the one named, or (without x/y columns) one found.
    let wkt = wkt_given.or_else(|| if xy.is_some() { None } else { found_wkt(headers, rows) });
    // Infer each column's type from its values; a WKT column is the geometry, not an attribute.
    let columns: Vec<usize> = (0..headers.len()).filter(|&i| Some(i) != wkt).collect();
    let fields: Vec<Field> = columns
        .iter()
        .filter_map(|&i| Some(Field::new(headers.get(i)?.trim(), text_type(rows.iter().filter_map(|r| r.get(i)).map(|s| s.as_str())))))
        .collect();
    let mut report = ReadReport { skipped: malformed, ..ReadReport::default() };
    if malformed > 0 {
        report.messages.push(format!("{malformed} rows were malformed and skipped"));
    }
    let gt = if xy.is_some() { GeometryType::Point } else { GeometryType::None };
    let mut ds = Dataset::new(name, gt, fields);
    let mut srid: Option<u32> = None;
    let mut bad_wkt = 0usize;
    for r in rows {
        let cell = |i: usize| r.get(i).map(|s| s.trim()).unwrap_or("");
        let geom = if let Some((x, y)) = xy {
            match cell(x).parse::<f64>().ok().zip(cell(y).parse::<f64>().ok()) {
                Some((x, y)) if x.is_finite() && y.is_finite() => Some(Geometry::Point(Coord::new(x, y))),
                _ => {
                    report.skipped += 1;
                    None
                }
            }
        } else if let Some(w) = wkt {
            match cell(w) {
                "" => None,
                t => match crate::interop::parse_wkt(t) {
                    Ok((g, s)) => {
                        srid = srid.or(s);
                        g
                    }
                    Err(_) => {
                        bad_wkt += 1;
                        None
                    }
                },
            }
        } else {
            None
        };
        let attrs = columns
            .iter()
            .zip(&ds.fields)
            .map(|(&i, f)| Value::parse(&unguard(r.get(i).map(|s| s.as_str()).unwrap_or("")), f.ty))
            .collect();
        ds.push_next(geom, attrs);
    }
    if report.skipped > malformed {
        report.messages.push(format!("{} rows had no usable coordinates", report.skipped - malformed));
    }
    if bad_wkt > 0 {
        report.messages.push(format!("{bad_wkt} rows had geometry that isn't readable WKT"));
    }
    if wkt.is_some() {
        // The most common kind, as GeoJSON's reader takes it; rows of other kinds are kept (they
        // draw), and said.
        ds.geometry_type = dominant_type(&ds);
        let other = ds.features.iter().filter_map(|f| f.geometry.as_ref()).filter(|g| g.kind() != ds.geometry_type).count();
        if other > 0 {
            report
                .messages
                .push(format!("{other} rows have another kind of geometry than the layer's {}", ds.geometry_type.label().to_lowercase()));
        }
    }
    // Coordinates in another system become longitude/latitude.
    let crs = match (opts.crs, srid) {
        (Some(c), _) => Some(c),
        (None, Some(code)) => match crate::raster::Crs::from_epsg(code) {
            Some(c) => Some(c),
            None => {
                report.messages.push(format!("unknown coordinate system SRID {code}; coordinates are read as longitude/latitude"));
                None
            }
        },
        (None, None) => None,
    };
    match crs.filter(|c| *c != crate::raster::Crs::LonLat) {
        Some(c) => {
            let t = c.transformer().map_err(DataError::Format)?;
            let mut lost = 0usize;
            for f in &mut ds.features {
                if let Some(g) = f.geometry.take() {
                    let out = g.map_coords(&|p| t.to_lonlat(p).unwrap_or(Coord::new(f64::NAN, f64::NAN)));
                    let mut ok = true;
                    out.for_each_coord(|c| ok &= c.is_finite());
                    if ok {
                        f.geometry = Some(out);
                    } else {
                        lost += 1;
                    }
                }
            }
            if lost > 0 {
                report.skipped += lost;
                report.messages.push(format!("{lost} rows couldn't be converted from {} to longitude/latitude", c.label()));
            }
        }
        None => {
            let outside = ds.features.iter().filter_map(|f| f.geometry.as_ref()).any(out_of_lonlat);
            if outside {
                report.messages.push(
                    "coordinates aren't longitude/latitude: give the file's coordinate system (\"crs\", such as EPSG:2263) to place them"
                        .into(),
                );
            }
        }
    }
    Ok((ds, report))
}

/// Whether a geometry has coordinates outside longitude/latitude's range.
fn out_of_lonlat(g: &Geometry) -> bool {
    let e = g.extent();
    e.min_x < -180.0 || e.max_x > 180.0 || e.min_y < -90.0 || e.max_y > 90.0
}

/// The type of a column of text values: Integer or Double when every non-empty value is one,
/// otherwise (or with no values) Text.
pub(crate) fn text_type<'a>(vals: impl Iterator<Item = &'a str>) -> FieldType {
    let mut ty: Option<FieldType> = None;
    for v in vals.map(str::trim).filter(|s| !s.is_empty()) {
        let t = if has_leading_zero(v) {
            // Identifiers such as ZIP codes keep their leading zeros.
            FieldType::Text
        } else if v.parse::<i64>().is_ok() {
            FieldType::Integer
        } else if v.parse::<f64>().is_ok_and(f64::is_finite) {
            FieldType::Double
        } else {
            FieldType::Text
        };
        ty = Some(match ty {
            None => t,
            Some(prev) => widen(prev, Some(t)),
        });
    }
    ty.unwrap_or(FieldType::Text)
}

/// Types a dataset's Text columns from their values (see [`text_type`]), for readers whose
/// formats hold every value as text (KML, GPX); the columns named in `keep` stay text.
pub(crate) fn type_text_columns(ds: &mut Dataset, keep: &[&str]) {
    for i in 0..ds.fields.len() {
        if ds.fields.get(i).is_none_or(|f| f.ty != FieldType::Text || keep.iter().any(|k| f.name.eq_ignore_ascii_case(k))) {
            continue;
        }
        let ty = text_type(ds.features.iter().filter_map(|f| match f.attrs.get(i) {
            Some(Value::Text(t)) => Some(t.as_str()),
            _ => None,
        }));
        if ty == FieldType::Text {
            continue;
        }
        if let Some(f) = ds.fields.get_mut(i) {
            f.ty = ty;
        }
        for f in &mut ds.features {
            if let Some(v) = f.attrs.get_mut(i)
                && let Value::Text(t) = v
            {
                *v = if t.trim().is_empty() { Value::Null } else { Value::parse(t, ty) };
            }
        }
    }
}

/// `0123` or `-007` (but not `0`, `0.5` or `-0.5`): a code, not a number.
fn has_leading_zero(v: &str) -> bool {
    let d = v.strip_prefix(['-', '+']).unwrap_or(v);
    d.len() > 1 && d.starts_with('0') && d.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit())
}

/// Undoes the export formula guard: `'=x` reads back as `=x`.
fn unguard(cell: &str) -> std::borrow::Cow<'_, str> {
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(FORMULA_START) => std::borrow::Cow::Owned(rest.to_string()),
        _ => std::borrow::Cow::Borrowed(cell),
    }
}

const FORMULA_START: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// A cell's text; text that a spreadsheet would run as a formula is prefixed with `'`.
fn csv_cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Text(t) if t.starts_with(FORMULA_START) => format!("'{t}"),
        other => other.to_string(),
    }
}

pub fn to_csv(ds: &Dataset) -> Result<String> {
    let mut w = csv::Writer::from_writer(Vec::new());
    let point = ds.geometry_type == GeometryType::Point;
    let mut head: Vec<String> = ds.fields.iter().map(|f| f.name.clone()).collect();
    if point {
        // Coordinate columns get names that don't collide with existing fields.
        for base in ["longitude", "latitude"] {
            let mut name = base.to_string();
            let mut i = 1;
            while head.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
                name = format!("{base}_{i}");
                i += 1;
            }
            head.push(name);
        }
    }
    w.write_record(&head).map_err(|e| DataError::Format(e.to_string()))?;
    for f in &ds.features {
        let mut row: Vec<String> = f.attrs.iter().map(csv_cell).collect();
        if point {
            let c = f.geometry.as_ref().and_then(|g| g.label_point());
            row.push(c.map(|c| c.x.to_string()).unwrap_or_default());
            row.push(c.map(|c| c.y.to_string()).unwrap_or_default());
        }
        w.write_record(&row).map_err(|e| DataError::Format(e.to_string()))?;
    }
    let bytes = w.into_inner().map_err(|e| DataError::Format(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| DataError::Format(e.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Shapefile

fn read_shapefile(path: &Path, name: &str) -> Result<(Dataset, ReadReport)> {
    let dbf = path.with_extension("dbf");
    if !dbf.is_file() {
        return Err(DataError::Format(format!("Shapefile: {} is missing", dbf.display())));
    }
    let shx = path.with_extension("shx");
    let parts = ShapefileParts {
        shp: std::fs::read(path)?,
        shx: if shx.is_file() { Some(std::fs::read(shx)?) } else { None },
        dbf: std::fs::read(dbf)?,
        prj: crate::raster::read_prj(path),
    };
    read_shapefile_parts(parts, name)
}

/// Most layers read from one zip archive.
pub const MAX_ZIP_LAYERS: usize = 32;

/// Reads the vector data in a zip archive: each Shapefile in it (its `.shp` with the `.dbf`,
/// `.shx` and `.prj` beside it), then every other file the readers understand (GeoJSON, CSV,
/// GeoParquet, FlatGeobuf…; `.txt` files in archives are readmes, so they're skipped), at most
/// [`MAX_ZIP_LAYERS`]. Folders inside are fine; macOS
/// resource forks (`__MACOSX/`, `._*`) are ignored.
pub fn read_zip(file_name: &str, bytes: &[u8]) -> Result<Vec<(Dataset, ReadReport)>> {
    let lower = |n: &str| n.to_ascii_lowercase();
    let is_fork = |n: &str| n.split('/').any(|part| part == "__MACOSX" || part.starts_with("._"));
    let ext_of = |n: &str| Path::new(n).extension().and_then(|e| e.to_str()).map(lower).unwrap_or_default();
    // A `.pbf` is read only as an OpenStreetMap extract (`.osm.pbf`): a vector tile set's
    // `z/x/y.pbf` files aren't extracted at all.
    let wanted = |n: &str| {
        let e = ext_of(n);
        !is_fork(n)
            && (matches!(e.as_str(), "shp" | "shx" | "dbf" | "prj" | "mid")
                || (e == "pbf" && lower(n).ends_with(".osm.pbf"))
                || (!matches!(e.as_str(), "zip" | "txt" | "pbf") && READ_EXTENSIONS.contains(&e.as_str())))
    };
    let entries = crate::zip::read_matching(bytes, wanted).map_err(|e| DataError::Format(format!("{file_name}: {e}")))?;
    // Sidecar files by their path without the extension, compared without regard to case.
    let stem = |n: &str| lower(n.get(..n.len().saturating_sub(ext_of(n).len() + 1)).unwrap_or(n));
    // Entries are taken out as they're used, so no file's bytes are copied.
    let mut entries: Vec<Option<(String, Vec<u8>)>> = entries.into_iter().map(Some).collect();
    let mut shps: Vec<String> = entries.iter().flatten().filter(|(n, _)| ext_of(n) == "shp").map(|(n, _)| n.clone()).collect();
    shps.sort();
    let take = |entries: &mut Vec<Option<(String, Vec<u8>)>>, base: &str, ext: &str| -> Option<(String, Vec<u8>)> {
        entries.iter_mut().find(|e| e.as_ref().is_some_and(|(n, _)| ext_of(n) == ext && stem(n) == base))?.take()
    };
    let mut out = Vec::new();
    // Members that couldn't be read are skipped, and said in the first layer's report.
    let mut skipped: Vec<String> = Vec::new();
    for n in shps {
        if out.len() >= MAX_ZIP_LAYERS {
            break;
        }
        let base = stem(&n);
        let name = Path::new(&n).file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
        let shp = take(&mut entries, &base, "shp").map(|(_, b)| b).unwrap_or_default();
        let shx = take(&mut entries, &base, "shx").map(|(_, b)| b);
        let prj = take(&mut entries, &base, "prj").map(|(_, b)| String::from_utf8_lossy(&b).into_owned());
        let Some((_, dbf)) = take(&mut entries, &base, "dbf") else {
            skipped.push(format!("{n} (no .dbf beside it)"));
            continue;
        };
        match read_shapefile_parts(ShapefileParts { shp, shx, dbf, prj }, &name) {
            Ok(mut read) => {
                read.0.source = Some(format!("{file_name}/{n}"));
                out.push(read);
            }
            Err(e) => skipped.push(format!("{n} ({e})")),
        }
    }
    // MapInfo files with their .mid.
    let mut mifs: Vec<String> = entries.iter().flatten().filter(|(n, _)| ext_of(n) == "mif").map(|(n, _)| n.clone()).collect();
    mifs.sort();
    for n in mifs {
        if out.len() >= MAX_ZIP_LAYERS {
            break;
        }
        let base = stem(&n);
        let name = Path::new(&n).file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
        let mif = take(&mut entries, &base, "mif").map(|(_, b)| b).unwrap_or_default();
        let mid = take(&mut entries, &base, "mid").map(|(_, b)| b);
        match crate::mapinfo::read_bytes(&mif, mid.as_deref(), &name) {
            Ok(mut read) => {
                read.0.source = Some(format!("{file_name}/{n}"));
                out.push(read);
            }
            Err(e) => skipped.push(format!("{n} ({e})")),
        }
    }
    let mut others: Vec<(String, Vec<u8>)> =
        entries.into_iter().flatten().filter(|(n, _)| !matches!(ext_of(n).as_str(), "shp" | "shx" | "dbf" | "prj" | "mid")).collect();
    others.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, b) in others {
        if out.len() >= MAX_ZIP_LAYERS {
            skipped.push(format!("{n} (past the {MAX_ZIP_LAYERS}-layer limit)"));
            continue;
        }
        let leaf = n.rsplit('/').next().unwrap_or(&n);
        match read_bytes(leaf, &b) {
            Ok(mut read) => {
                read.0.source = Some(format!("{file_name}/{n}"));
                out.push(read);
            }
            Err(e) => skipped.push(format!("{n} ({e})")),
        }
    }
    if out.is_empty() {
        let why = if skipped.is_empty() { String::new() } else { format!(": {}", skipped.join("; ")) };
        return Err(DataError::Format(format!("{file_name}: no Shapefile or other vector data in the archive could be read{why}")));
    }
    if let Some((_, report)) = out.first_mut()
        && !skipped.is_empty()
    {
        report.messages.push(format!("{file_name}: skipped {}", skipped.join("; ")));
    }
    Ok(out)
}

/// The files of a Shapefile, read into memory.
struct ShapefileParts {
    shp: Vec<u8>,
    shx: Option<Vec<u8>>,
    dbf: Vec<u8>,
    prj: Option<String>,
}

fn read_shapefile_parts(parts: ShapefileParts, name: &str) -> Result<(Dataset, ReadReport)> {
    use shapefile::Shape;
    use shapefile::dbase::FieldValue;
    use std::io::Cursor;
    let fmt = |e: shapefile::Error| DataError::Format(format!("Shapefile: {e}"));
    let dbf = shapefile::dbase::Reader::new(Cursor::new(parts.dbf)).map_err(|e| DataError::Format(format!("Shapefile: the .dbf: {e}")))?;
    // Field order comes from the .dbf header (records are unordered maps).
    let order: Vec<(String, FieldType)> = dbf
        .fields()
        .iter()
        .filter(|f| f.name() != "DeletionFlag")
        .map(|f| {
            use shapefile::dbase::FieldType as D;
            let ty = match f.field_type() {
                D::Numeric | D::Float | D::Double | D::Currency => FieldType::Double,
                D::Integer => FieldType::Integer,
                D::Logical => FieldType::Boolean,
                _ => FieldType::Text,
            };
            (f.name().to_string(), ty)
        })
        .collect();
    let shapes = match parts.shx {
        Some(shx) => shapefile::ShapeReader::with_shx(Cursor::new(parts.shp), Cursor::new(shx)),
        None => shapefile::ShapeReader::new(Cursor::new(parts.shp)),
    }
    .map_err(fmt)?;
    let mut reader = shapefile::Reader::new(shapes, dbf);
    let fields: Vec<Field> = order.iter().map(|(n, t)| Field::new(n.clone(), *t)).collect();
    let mut ds = Dataset::new(name, GeometryType::None, fields);
    let mut report = ReadReport::default();
    let to_lonlat = shapefile_crs(parts.prj.as_deref(), &mut report);
    let (mut converted, mut unconverted) = (0usize, 0usize);
    let pt = |x: f64, y: f64| Coord::new(x, y);
    for item in reader.iter_shapes_and_records() {
        let Ok((shape, record)) = item else {
            report.skipped += 1;
            continue;
        };
        let geom = match shape {
            Shape::NullShape => None,
            Shape::Point(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::PointM(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::PointZ(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::Multipoint(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::MultipointM(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::MultipointZ(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::Polyline(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::PolylineM(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::PolylineZ(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::Polygon(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::PolygonM(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::PolygonZ(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::Multipatch(_) => {
                report.skipped += 1;
                continue;
            }
        };
        let geom = match (&to_lonlat, geom) {
            (Some(t), Some(g)) => {
                let mut ok = true;
                let out = g.map_coords(&|c| t.to_lonlat(c).unwrap_or(Coord::new(f64::NAN, f64::NAN)));
                out.for_each_coord(|c| ok &= c.is_finite());
                converted += usize::from(ok);
                unconverted += usize::from(!ok);
                ok.then_some(out)
            }
            (_, g) => g,
        };
        let attrs = ds
            .fields
            .iter()
            .map(|f| match record.get(&f.name) {
                Some(FieldValue::Character(Some(s))) => Value::Text(s.trim().to_string()),
                Some(FieldValue::Numeric(Some(n))) if n.is_finite() => Value::Float(*n),
                Some(FieldValue::Float(Some(n))) => Value::Float(f64::from(*n)),
                Some(FieldValue::Integer(i)) => Value::Int(i64::from(*i)),
                Some(FieldValue::Double(d)) => Value::Float(*d),
                Some(FieldValue::Currency(d)) => Value::Float(*d),
                Some(FieldValue::Logical(Some(b))) => Value::Bool(*b),
                Some(FieldValue::Memo(s)) => Value::Text(s.clone()),
                Some(FieldValue::Date(Some(d))) => Value::Text(format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())),
                _ => Value::Null,
            })
            .collect();
        ds.push_next(geom, attrs);
    }
    ds.geometry_type = dominant_type(&ds);
    integral_numeric_to_integer(&mut ds);
    let e = ds.extent();
    if !e.is_empty() && (e.min_x < -180.5 || e.max_x > 180.5 || e.min_y < -90.5 || e.max_y > 90.5) {
        report.messages.push("coordinates are not longitude/latitude; RustGIS reads them as WGS 84 and they may draw out of place".into());
    }
    if report.skipped > 0 {
        report.messages.push(format!("{} shapes were unreadable or unsupported", report.skipped));
    }
    if unconverted > 0 && converted == 0 {
        report.messages.push("no shape could be converted to longitude/latitude: the .prj doesn't seem to match the coordinates".into());
    } else if unconverted > 0 {
        report.messages.push(format!("{unconverted} shapes couldn't be converted to longitude/latitude and have no geometry"));
    }
    Ok((ds, report))
}

/// Converts a Shapefile's coordinates to longitude/latitude, from the coordinate system in its
/// `.prj`: `None` for longitude/latitude, or when there's no `.prj` or it isn't understood (said in
/// the report).
fn shapefile_crs(prj: Option<&str>, report: &mut ReadReport) -> Option<crate::raster::Transformer> {
    let crs = match crate::raster::wkt::read_wkt(prj?) {
        Ok((c, note)) => {
            if let Some(n) = note {
                report.messages.push(format!("the .prj file: {n}"));
            }
            c
        }
        Err(e) => {
            report.messages.push(format!("the .prj file: {e}; coordinates are read as longitude/latitude"));
            return None;
        }
    };
    if crs == crate::raster::Crs::LonLat {
        return None;
    }
    match crs.transformer() {
        Ok(t) => Some(t),
        Err(e) => {
            report.messages.push(format!("the .prj file: {e}; coordinates are read as longitude/latitude"));
            None
        }
    }
}

/// The .dbf header doesn't say whether a Numeric column has decimals, so a Double column whose
/// values are all whole numbers within i64 range is read as Integer.
fn integral_numeric_to_integer(ds: &mut Dataset) {
    const LIMIT: f64 = 9.0e15;
    for fi in 0..ds.fields.len() {
        if ds.fields.get(fi).map(|f| f.ty) != Some(FieldType::Double) {
            continue;
        }
        let whole = ds.features.iter().all(|f| match f.attrs.get(fi) {
            Some(Value::Float(v)) => v.fract() == 0.0 && v.abs() < LIMIT,
            _ => true,
        });
        let any = ds.features.iter().any(|f| matches!(f.attrs.get(fi), Some(Value::Float(_))));
        if !whole || !any {
            continue;
        }
        if let Some(f) = ds.fields.get_mut(fi) {
            f.ty = FieldType::Integer;
        }
        for f in &mut ds.features {
            if let Some(v) = f.attrs.get_mut(fi)
                && let Value::Float(x) = *v
            {
                *v = Value::Int(x as i64);
            }
        }
    }
}

/// WGS 84 as ESRI WKT, for a Shapefile's `.prj`.
const WGS84_PRJ: &str = "GEOGCS[\"GCS_WGS_1984\",DATUM[\"D_WGS_1984\",SPHEROID[\"WGS_1984\",6378137.0,298.257223563]],PRIMEM[\"Greenwich\",0.0],UNIT[\"Degree\",0.0174532925199433]]";

/// The longest prefix of `s` that fits in `max` bytes, cut at a character boundary.
pub(crate) fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

/// dBase field names (at most 10 bytes, unique without regard to case) for the fields.
fn dbf_names(fields: &[Field]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(fields.len());
    for f in fields {
        let base = cut(f.name.trim(), 10);
        let base = if base.is_empty() { "FIELD" } else { base };
        let taken = |n: &str, out: &[String]| out.iter().any(|o| o.eq_ignore_ascii_case(n));
        let name = if !taken(base, &out) {
            base.to_string()
        } else {
            (1..100_000)
                .map(|i| {
                    let tag = format!("_{i}");
                    format!("{}{tag}", cut(base, 10 - tag.len()))
                })
                .find(|n| !taken(n, &out))
                .unwrap_or_else(|| base.to_string())
        };
        out.push(name);
    }
    out
}

/// A dataset as the files of a Shapefile: (extension, bytes) for `.shp`, `.shx`, `.dbf`, `.prj`
/// (WGS 84) and `.cpg` (UTF-8). Shapefiles hold one kind of shape and features with geometry:
/// features without it (or with nothing drawable) are left out, and the count of them returned.
/// Field names are cut to dBase's 10 bytes and text to 254.
pub fn shapefile_parts(ds: &Dataset) -> Result<(Vec<(&'static str, Vec<u8>)>, usize)> {
    use shapefile::dbase::{FieldName, FieldValue, Record, TableWriterBuilder};
    use std::io::Cursor;
    let fmt = |e: shapefile::Error| DataError::Format(format!("Shapefile: {e}"));
    let names = dbf_names(&ds.fields);
    let dbf_name = |n: &str| FieldName::try_from(n).map_err(|e| DataError::Format(format!("Shapefile: field {n}: {e}")));
    let mut table = TableWriterBuilder::new();
    // Per field: decimals of a Double field (numbers too large for 24 characters become null).
    let mut decimals = vec![0usize; ds.fields.len()];
    for (k, (f, n)) in ds.fields.iter().zip(&names).enumerate() {
        table = match f.ty {
            // 18 digits: GDAL reads a wider field as a real number.
            FieldType::Integer => table.add_numeric_field(dbf_name(n)?, 18, 0),
            FieldType::Double => {
                let max = ds
                    .features
                    .iter()
                    .filter_map(|r| r.attrs.get(k).and_then(Value::as_f64))
                    .filter(|v| v.is_finite())
                    .fold(0.0f64, |m, v| m.max(v.abs()));
                // Digits before the point, a sign and the point itself; the rest are decimals.
                let int_digits = if max >= 1.0 { (max.log10().floor() as usize).saturating_add(1) } else { 1 };
                let d = 24usize.saturating_sub(int_digits + 2).min(15);
                if let Some(slot) = decimals.get_mut(k) {
                    *slot = d;
                }
                table.add_numeric_field(dbf_name(n)?, 24, u8::try_from(d).unwrap_or(15))
            }
            FieldType::Boolean => table.add_logical_field(dbf_name(n)?),
            FieldType::Text => {
                let longest = ds.features.iter().filter_map(|r| r.attrs.get(k)).map(|v| v.to_string().len()).max().unwrap_or(0);
                table.add_character_field(dbf_name(n)?, u8::try_from(longest.clamp(1, 254)).unwrap_or(254))
            }
        };
    }
    let record = |r: &crate::Feature| -> Record {
        let mut rec = Record::default();
        for (k, (f, n)) in ds.fields.iter().zip(&names).enumerate() {
            let v = r.attrs.get(k).unwrap_or(&Value::Null);
            let value = match f.ty {
                FieldType::Integer | FieldType::Double => {
                    let x = v.as_f64().filter(|x| {
                        // Whatever doesn't fit the field is null rather than cut short; so is a
                        // whole number past 2^53, which dBase's decimal text would hold only
                        // rounded (it goes through f64).
                        let d = decimals.get(k).copied().unwrap_or(0);
                        // Likewise a number too small for the field's decimals, which would read 0.
                        let exact = !(matches!(v, Value::Int(i) if i.unsigned_abs() > 1 << 53)
                            || f.ty == FieldType::Double && *x != 0.0 && x.abs() < 0.5 * 10f64.powi(-(d as i32)));
                        exact && format!("{x:.d$}").len() <= if f.ty == FieldType::Integer { 18 } else { 24 }
                    });
                    FieldValue::Numeric(if f.ty == FieldType::Integer { x.map(f64::round) } else { x })
                }
                FieldType::Boolean => FieldValue::Logical(match v {
                    Value::Bool(b) => Some(*b),
                    Value::Int(i) => Some(*i != 0),
                    _ => None,
                }),
                FieldType::Text => FieldValue::Character((!v.is_null()).then(|| cut(&v.to_string(), 254).to_string())),
            };
            rec.insert(n.clone(), value);
        }
        rec
    };

    let pt = |c: &Coord| shapefile::Point::new(c.x, c.y);
    let finite = |c: &&Coord| c.x.is_finite() && c.y.is_finite();
    let (mut shp, mut shx, mut dbf) = (Cursor::new(Vec::new()), Cursor::new(Vec::new()), Cursor::new(Vec::new()));
    let mut left_out = 0;
    let mut written = 0;
    {
        let shapes = shapefile::ShapeWriter::with_shx(&mut shp, &mut shx);
        let mut w = shapefile::Writer::new(shapes, table.build_with_dest(&mut dbf));
        let multi_points = ds.features.iter().any(|f| matches!(f.geometry, Some(Geometry::MultiPoint(_))));
        for f in &ds.features {
            let ok = match (&f.geometry, ds.geometry_type) {
                (Some(Geometry::Point(c)), GeometryType::Point) if !multi_points && finite(&c) => {
                    w.write_shape_and_record(&pt(c), &record(f)).map_err(fmt)?;
                    true
                }
                (Some(g @ (Geometry::Point(_) | Geometry::MultiPoint(_))), GeometryType::Point) => {
                    let mut pts = Vec::new();
                    g.for_each_coord(|c| pts.push(c));
                    let pts: Vec<shapefile::Point> = pts.iter().filter(finite).map(pt).collect();
                    if pts.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Multipoint::new(pts), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                (Some(Geometry::Line(parts)), GeometryType::Polyline) => {
                    let parts: Vec<Vec<shapefile::Point>> =
                        parts.iter().map(|p| p.iter().filter(finite).map(pt).collect::<Vec<_>>()).filter(|p| p.len() >= 2).collect();
                    if parts.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Polyline::with_parts(parts), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                (Some(Geometry::Polygon(polys)), GeometryType::Polygon) => {
                    let mut rings = Vec::new();
                    for poly in polys {
                        let mut it = poly.iter().map(|r| r.iter().filter(finite).map(pt).collect::<Vec<_>>()).filter(|r| r.len() >= 3);
                        // A polygon whose outer ring is gone loses its holes too.
                        let outer_kept = poly.first().is_some_and(|r| r.iter().filter(finite).count() >= 3);
                        if let Some(outer) = it.next().filter(|_| outer_kept) {
                            rings.push(shapefile::PolygonRing::Outer(outer));
                            rings.extend(it.map(shapefile::PolygonRing::Inner));
                        }
                    }
                    if rings.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Polygon::with_rings(rings), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                _ => false,
            };
            if ok {
                written += 1;
            } else {
                left_out += 1;
            }
        }
        if written == 0 {
            let what = if ds.geometry_type == GeometryType::None { "a table without geometry" } else { "a layer with no features to draw" };
            return Err(DataError::Unsupported(format!("{what} as a Shapefile")));
        }
    }
    Ok((
        vec![
            ("shp", shp.into_inner()),
            ("shx", shx.into_inner()),
            ("dbf", dbf.into_inner()),
            ("prj", WGS84_PRJ.as_bytes().to_vec()),
            ("cpg", b"UTF-8".to_vec()),
        ],
        left_out,
    ))
}

fn is_outer<P>(r: &shapefile::PolygonRing<P>) -> bool {
    matches!(r, shapefile::PolygonRing::Outer(_))
}

/// Groups a flat list of rings (outer flag) into polygons: each hole joins the preceding outer ring.
pub(crate) fn group_rings(rings: Vec<(Vec<Coord>, bool)>) -> Geometry {
    let mut polys: Vec<Vec<Vec<Coord>>> = Vec::new();
    for (ring, outer) in rings {
        match polys.last_mut() {
            Some(p) if !outer => p.push(ring),
            _ => polys.push(vec![ring]),
        }
    }
    Geometry::Polygon(polys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topojson_named_json_from_disk() {
        let topo = r#"{"type":"Topology","objects":{"pts":{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[5,6]}]},"more":{"type":"GeometryCollection","geometries":[{"type":"Point","coordinates":[1,2]}]}},"arcs":[]}"#;
        let path = std::env::temp_dir().join(format!("rustgis-topo-{}.json", std::process::id()));
        std::fs::write(&path, topo).unwrap();
        let (ds, report) = read_path(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(ds.features.len(), 1);
        assert!(report.messages.iter().any(|m| m.contains("also holds")), "{:?}", report.messages);
    }

    #[test]
    fn formats_by_contents() {
        let cases: [(&[u8], Option<&str>); 11] = [
            (b"PAR1\0\0", Some("parquet")),
            (b"fgb\x03fgb\0", Some("fgb")),
            (b"II*\0\x08\0", Some("tif")),
            (b"PK\x03\x04", Some("zip")),
            (b"SQLite format 3\0", Some("gpkg")),
            (b"\xef\xbb\xbf  {\"type\":\"FeatureCollection\"}", Some("geojson")),
            (b"<?xml version=\"1.0\"?><kml xmlns=\"\">", Some("kml")),
            (b"name;lon;lat\r\nA;7;45\r\n", Some("csv")),
            (b"name\tlon\tlat\n\nA\t7\t45", Some("csv")),
            // A sentence with a comma isn't a table, nor a header alone.
            (b"Not found, try again later.\n", None),
            (b"name,lon,lat", None),
        ];
        for (bytes, want) in cases {
            assert_eq!(sniff_extension(bytes), want, "{:?}", String::from_utf8_lossy(bytes));
        }
        assert_eq!(sniff_extension(b"a,b\n1,2,3\n"), None);
        assert_eq!(sniff_extension(b"a,b\n1,\x01\n"), None);
    }

    #[test]
    fn geojson_roundtrip() {
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"A","pop":5},"geometry":{"type":"Point","coordinates":[1,2]}},
            {"type":"Feature","properties":{"name":"B","pop":2.5},"geometry":{"type":"Point","coordinates":[3,4]}},
            {"type":"Feature","properties":{"name":"C"},"geometry":{"type":"Point","coordinates":["x",4]}}]}"#;
        let (ds, rep) = read_geojson(text, "t").unwrap();
        assert_eq!(ds.features.len(), 2);
        assert_eq!(rep.skipped, 1);
        assert_eq!(ds.fields[1].ty, FieldType::Double);
        assert_eq!(ds.geometry_type, GeometryType::Point);
        let back = read_geojson(&to_geojson(&ds).to_string(), "t").unwrap().0;
        assert_eq!(back.features.len(), 2);
        assert_eq!(back.value(1, 0), &Value::Text("B".into()));
    }

    #[test]
    fn geojson_null_first_keeps_numeric_type() {
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"v":null,"n":null},"geometry":null},
            {"type":"Feature","properties":{"v":1,"n":null},"geometry":null},
            {"type":"Feature","properties":{"v":2.5},"geometry":null}]}"#;
        let (ds, _) = read_geojson(text, "t").unwrap();
        let (v, n) = (ds.field_index("v").unwrap(), ds.field_index("n").unwrap());
        assert_eq!(ds.fields[v].ty, FieldType::Double);
        assert_eq!(ds.fields[n].ty, FieldType::Text, "all-null columns are Text");
        assert_eq!(ds.value(1, v), &Value::Float(1.0));
    }

    #[test]
    fn write_atomic_replaces_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("rustgis-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp files left behind");
        assert!(write_atomic(Path::new("/"), b"x").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_bytes_dispatches_by_extension() {
        let (ds, _) = read_bytes("pts.CSV", b"lon,lat\n1,2\n").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        assert!(read_bytes("x.shp", b"").is_err());
        assert!(read_bytes("x.geojson", &[0xff, 0xfe]).is_err());
    }

    /// Projected x/y columns with a coordinate system, other separators, WKT and EWKT columns, and
    /// the notes when something can't be placed.
    #[test]
    fn delimited_text_options() {
        use crate::raster::Crs;
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3"), b';');
        assert_eq!(sniff_delimiter("a\tb\n"), b'\t');
        assert_eq!(sniff_delimiter("\"a;b\",c\n"), b',');
        assert_eq!(sniff_delimiter("one\n"), b',');
        // A semicolon in a one-column file's heading doesn't split it once rows show the shape.
        assert_eq!(sniff_delimiter("Name; Surname,id\nAda Lovelace,1\nAlan Turing,2\n"), b',');
        assert_eq!(sniff_delimiter("a;b,c\n1;2,3\n"), b',', "a tie keeps the comma");
        assert_eq!(sniff_delimiter("x;y\n1,5;2\n3,5;4\n"), b';', "decimal commas");
        // Central Park in New York State Plane (US feet), semicolon-separated, columns named.
        let text = "id;E;N\n1;992000;220000\n2;x;1\n";
        let opts =
            CsvOptions { x: Some("e".into()), y: Some("N".into()), crs: Some(Crs::parse("EPSG:2263").unwrap()), ..CsvOptions::default() };
        let (ds, rep) = read_csv_with(text, "t", &opts).unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        let Some(Geometry::Point(c)) = &ds.features[0].geometry else { panic!() };
        assert!((c.x + 73.96).abs() < 0.05 && (c.y - 40.78).abs() < 0.05, "{c:?}");
        assert_eq!(rep.messages, ["1 rows had no usable coordinates"]);
        // Without the coordinate system, the points are said to be out of place.
        let (_, rep) = read_csv_with(text, "t", &CsvOptions { crs: None, ..opts.clone() }).unwrap();
        assert!(rep.messages.iter().any(|m| m.contains("\"crs\"")), "{:?}", rep.messages);
        assert!(read_csv_with(text, "t", &CsvOptions { x: Some("nope".into()), ..opts.clone() }).is_err());
        assert!(read_csv_with(text, "t", &CsvOptions { y: None, ..opts.clone() }).is_err(), "x without y");
        assert!(read_csv_with(text, "t", &CsvOptions { wkt: Some("id".into()), ..opts }).is_err(), "x/y and WKT");
        // A longitude without a latitude doesn't pair with a northing.
        let (ds, _) = read_csv("lon,northing\n1,2\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::None);
        // Easting/northing found by name in a tab-separated file.
        let (ds, _) = read_csv("name\teasting\tnorthing\nA\t500000\t4649776\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        // A WKT column found by name: it is the geometry, not a field; EWKT's SRID places it.
        let text = "name,geometry\nA,\"SRID=32633;LINESTRING (500000 4649776, 500100 4649876)\"\nB,\"SRID=32633;LINESTRING Z (500000 4649776 3, 500200 4649776 4)\"\nC,POINT (1 2)\nD,\nE,nonsense(\n";
        let (ds, rep) = read_csv(text, "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Polyline);
        assert_eq!(ds.fields.len(), 1);
        let e = ds.features[0].geometry.as_ref().unwrap().extent();
        assert!((e.min_x - 15.0).abs() < 0.01 && (e.min_y - 42.0).abs() < 0.05, "{e:?}");
        assert!(ds.features[3].geometry.is_none());
        assert_eq!(
            rep.messages,
            ["1 rows had geometry that isn't readable WKT", "1 rows have another kind of geometry than the layer's polyline"]
        );
        // A named WKT column with a given system.
        let opts = CsvOptions { wkt: Some("g".into()), crs: Some(Crs::WebMercator), ..CsvOptions::default() };
        let (ds, _) =
            read_csv_with("g|v\nPOLYGON ((0 0, 1000 0, 1000 1000, 0 0))|1\n", "t", &CsvOptions { delimiter: Some(b'|'), ..opts }).unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Polygon);
        assert_eq!(ds.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["v"]);
    }

    #[test]
    fn wkt_geometries() {
        use crate::interop::parse_wkt;
        let p = |t: &str| parse_wkt(t).unwrap_or_else(|e| panic!("{t}: {e}")).0;
        assert_eq!(p("POINT (1 2)"), Some(Geometry::Point(Coord::new(1.0, 2.0))));
        assert_eq!(p("point z (1 2 3)"), Some(Geometry::Point(Coord::new(1.0, 2.0))));
        assert_eq!(p("POINTZM(1 2 3 4)"), Some(Geometry::Point(Coord::new(1.0, 2.0))));
        assert_eq!(p("MULTIPOINT ((1 2), (3 4))"), p("MULTIPOINT (1 2, 3 4)"));
        assert!(matches!(p("MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))"), Some(Geometry::Line(l)) if l.len() == 2));
        assert!(
            matches!(p("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5), (5.1 5.1, 5.2 5.1, 5.2 5.2, 5.1 5.1)))"), Some(Geometry::Polygon(v)) if v.len() == 2 && v[1].len() == 2)
        );
        assert_eq!(p("GEOMETRYCOLLECTION (POINT EMPTY, POINT (7 8))"), Some(Geometry::Point(Coord::new(7.0, 8.0))));
        assert_eq!(p("POLYGON EMPTY"), None);
        assert_eq!(parse_wkt("SRID=4326;POINT (1 2)").unwrap().1, Some(4326));
        for bad in [
            "",
            "POINT",
            "POINT (1)",
            "POINT (1 2 3 4 5)",
            "POINT (1 2",
            "POINT (1 2) x",
            "CIRCLE (1 2)",
            "POINT (nan 2)",
            "SRID=x;POINT (1 2)",
            "POINT (1e999 2)",
        ] {
            assert!(parse_wkt(bad).is_err(), "{bad}");
        }
        let deep = format!("{}POINT (1 2){}", "GEOMETRYCOLLECTION (".repeat(50), ")".repeat(50));
        assert!(parse_wkt(&deep).is_err(), "nesting is bounded");
        assert_eq!(parse_wkt("SRID=4326 POINT (1 2)").unwrap_err(), "an SRID= without the ';' before its geometry");
        assert_eq!(crate::raster::Crs::parse("epsg:2263").unwrap().epsg(), 2263);
        assert_eq!(crate::raster::Crs::parse("http://www.opengis.net/def/crs/EPSG/0/3857").unwrap(), crate::raster::Crs::WebMercator);
        assert_eq!(crate::raster::Crs::parse("OGC:CRS84").unwrap(), crate::raster::Crs::LonLat);
        assert!(crate::raster::Crs::parse("EPSG:999999").is_err());
        assert!(crate::raster::Crs::parse("Mercator").is_err());
    }

    /// The columns Add Delimited Text offers, and the ones found as the reader finds them.
    #[test]
    fn delimited_text_columns() {
        let c = csv_columns("\u{feff}name;Lat;Lon\nA;1;2\n", None).unwrap();
        assert_eq!(c.names, ["name", "Lat", "Lon"]);
        assert_eq!((c.x.as_deref(), c.y.as_deref(), c.wkt.as_deref(), c.delimiter), (Some("Lon"), Some("Lat"), None, Some(b';')));
        // Longitude/latitude over easting/northing; easting/northing as a pair only.
        let c = csv_columns("easting,northing,lon,lat\n", None).unwrap();
        assert_eq!((c.x.as_deref(), c.y.as_deref()), (Some("lon"), Some("lat")));
        let c = csv_columns("easting,lat\n1,2\n", None).unwrap();
        assert_eq!((c.x, c.y), (None, None));
        // A WKT column by its name and first value, not by its name alone.
        let c = csv_columns("id,geometry\n1,\n2,POINT (1 2)\n", None).unwrap();
        assert_eq!(c.wkt.as_deref(), Some("geometry"));
        assert_eq!(csv_columns("id,geometry\n1,not wkt\n", None).unwrap().wkt, None);
        // The separator given splits the header.
        let c = csv_columns("a;b,c\n", Some(b',')).unwrap();
        assert_eq!((c.names, c.delimiter), (vec!["a;b".to_string(), "c".to_string()], Some(b',')));
        // Nothing to read is no columns, not a crash.
        assert!(csv_columns("", None).unwrap().names.is_empty());
        let _ = csv_columns("\"unclosed\n", None);
    }

    #[test]
    fn csv_points() {
        let (ds, rep) = read_csv("name,Lat,Lon,n\nA,10,20,1\nB,bad,1,2\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        assert_eq!(rep.skipped, 1);
        assert_eq!(ds.features[0].geometry, Some(Geometry::Point(Coord::new(20.0, 10.0))));
        assert!(to_csv(&ds).unwrap().contains("longitude"));
    }

    #[test]
    fn csv_leading_zeros_and_guard_roundtrip() {
        let (ds, _) = read_csv("zip,n,note\n02134,0,-abc\n10001,0.5,=x\n", "t").unwrap();
        assert_eq!(ds.fields[0].ty, FieldType::Text);
        assert_eq!(ds.value(0, 0), &Value::Text("02134".into()));
        assert_eq!(ds.fields[1].ty, FieldType::Double);
        let once = to_csv(&ds).unwrap();
        let (back, _) = read_csv(&once, "t").unwrap();
        assert_eq!(back.value(0, 2), &Value::Text("-abc".into()));
        assert_eq!(back.value(1, 2), &Value::Text("=x".into()));
        assert_eq!(to_csv(&back).unwrap(), once, "export/import is stable");
    }

    #[test]
    fn csv_bom_injection_and_column_names() {
        let (ds, _) = read_csv("\u{feff}lon,lat,longitude,note\n1,2,9,=SUM(A1)\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point, "BOM must not hide the lon header");
        let out = to_csv(&ds).unwrap();
        assert!(out.contains("'=SUM(A1)"), "{out}");
        assert!(out.lines().next().unwrap().contains("longitude_1"), "{out}");
        let (ds, _) = read_csv("a\nNaN\nx\n", "t").unwrap();
        assert_eq!(ds.fields[0].ty, FieldType::Text);
    }

    #[test]
    fn hostile_geojson_is_an_error_not_a_panic() {
        for t in ["", "{", "[]", "{\"type\":5}", "{\"type\":\"FeatureCollection\",\"features\":7}"] {
            let _ = read_geojson(t, "x");
        }
    }

    #[test]
    fn read_path_dispatches_by_extension() {
        let dir = std::env::temp_dir().join(format!("rustgis-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("square.GeoJSON");
        std::fs::write(&p, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}"#).unwrap();
        let (ds, _) = read_path(&p).unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Polygon);
        assert!(read_path(&dir.join("x.kml")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A projected Shapefile is converted to longitude/latitude by its `.prj` (issue #84).
    #[test]
    fn shapefile_prj_reprojects() {
        use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};
        let dir = std::env::temp_dir().join(format!("rustgis-shp-prj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shp = dir.join("paris.shp");
        {
            let table = TableWriterBuilder::new().add_character_field("name".try_into().unwrap(), 10);
            let mut w = shapefile::Writer::from_path(&shp, table).unwrap();
            let mut r = Record::default();
            r.insert("name".into(), FieldValue::Character(Some("paris".into())));
            // Paris in UTM zone 31N.
            w.write_shape_and_record(&shapefile::Point::new(451_219.33, 5_411_550.20), &r).unwrap();
        }
        let at = |ds: &Dataset| ds.features[0].geometry.as_ref().and_then(Geometry::label_point).unwrap();

        // As ArcGIS and GDAL write it: Esri WKT with no EPSG authority.
        let prj = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        std::fs::write(dir.join("paris.prj"), prj).unwrap();
        let (ds, rep) = read_path(&shp).unwrap();
        let c = at(&ds);
        assert!((c.x - 2.335).abs() < 1e-6 && (c.y - 48.855).abs() < 1e-6, "{c:?}");
        assert!(rep.messages.is_empty(), "{:?}", rep.messages);
        assert_eq!(ds.value(0, 0), &Value::Text("paris".into()));

        // A .prj that isn't understood is said, not passed over.
        std::fs::write(dir.join("paris.prj"), r#"PROJCS["x",PROJECTION["Hotine_Oblique_Mercator"]]"#).unwrap();
        let (ds, rep) = read_path(&shp).unwrap();
        assert_eq!(at(&ds).x, 451_219.33);
        assert!(rep.messages.iter().any(|m| m.contains(".prj") && m.contains("Hotine")), "{:?}", rep.messages);

        // No .prj: read as longitude/latitude, with the out-of-range warning.
        std::fs::remove_file(dir.join("paris.prj")).unwrap();
        let (_, rep) = read_path(&shp).unwrap();
        assert!(rep.messages.iter().any(|m| m.contains("not longitude/latitude")), "{:?}", rep.messages);
        std::fs::remove_dir_all(&dir).ok();
    }
    /// Zipped Shapefiles are read from memory (the web app's way to open them), with their `.prj`,
    /// from folders inside the archive, beside other vector files; truncations are errors.
    #[test]
    fn zipped_shapefiles() {
        use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};
        use std::io::Cursor;
        let (mut shp, mut shx, mut dbf) = (Cursor::new(Vec::new()), Cursor::new(Vec::new()), Cursor::new(Vec::new()));
        {
            let table = TableWriterBuilder::new().add_character_field("name".try_into().unwrap(), 10);
            let mut w = shapefile::Writer::new(shapefile::ShapeWriter::with_shx(&mut shp, &mut shx), table.build_with_dest(&mut dbf));
            let mut r = Record::default();
            r.insert("name".into(), FieldValue::Character(Some("paris".into())));
            w.write_shape_and_record(&shapefile::Point::new(451_219.33, 5_411_550.20), &r).unwrap();
        }
        let prj = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        let geojson = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"n":1},"geometry":{"type":"Point","coordinates":[1,2]}}]}"#;
        let entries = vec![
            ("data/Paris.SHP".to_string(), shp.into_inner()),
            ("data/Paris.shx".to_string(), shx.into_inner()),
            ("data/Paris.dbf".to_string(), dbf.into_inner()),
            ("data/Paris.prj".to_string(), prj.as_bytes().to_vec()),
            ("__MACOSX/data/._Paris.shp".to_string(), vec![0; 10]),
            ("extra/points.geojson".to_string(), geojson.as_bytes().to_vec()),
            ("ne_VERSION.txt".to_string(), b"5.1.1".to_vec()),
        ];
        let zip = crate::zip::write(&entries).unwrap();
        let layers = read_zip("upload.zip", &zip).unwrap();
        assert_eq!(layers.len(), 2);
        let (ds, rep) = &layers[0];
        assert_eq!(ds.name, "Paris");
        let c = ds.features[0].geometry.as_ref().and_then(Geometry::label_point).unwrap();
        assert!((c.x - 2.335).abs() < 1e-6 && (c.y - 48.855).abs() < 1e-6, "{c:?}");
        assert!(rep.messages.is_empty(), "{:?}", rep.messages);
        assert_eq!(ds.value(0, 0), &Value::Text("paris".into()));
        assert_eq!(layers[1].0.name, "points");
        // read_bytes takes the first layer.
        assert_eq!(read_bytes("upload.zip", &zip).unwrap().0.name, "Paris");
        // Without its .dbf the Shapefile is skipped (and said); the rest of the archive is read.
        let no_dbf: Vec<_> = entries.iter().filter(|(n, _)| !n.ends_with(".dbf")).cloned().collect();
        let layers = read_zip("x.zip", &crate::zip::write(&no_dbf).unwrap()).unwrap();
        assert_eq!(layers.len(), 1);
        assert!(layers[0].1.messages.iter().any(|m| m.contains("no .dbf")), "{:?}", layers[0].1.messages);
        // A metadata file that isn't GeoJSON doesn't keep the Shapefile out.
        let mut with_meta = entries.clone();
        with_meta.push(("data/metadata.json".to_string(), br#"{"title": "not GeoJSON"}"#.to_vec()));
        let layers = read_zip("x.zip", &crate::zip::write(&with_meta).unwrap()).unwrap();
        assert_eq!(layers.len(), 2);
        assert!(layers[0].1.messages.iter().any(|m| m.contains("metadata.json")), "{:?}", layers[0].1.messages);
        // A vector tile set's .pbf files aren't read as OpenStreetMap extracts (nor said).
        let mut with_tiles = entries.clone();
        with_tiles.extend((0..3).map(|i| (format!("tiles/3/{i}/1.pbf"), vec![0x1a, 2, 0, 0])));
        let layers = read_zip("x.zip", &crate::zip::write(&with_tiles).unwrap()).unwrap();
        assert_eq!(layers.len(), 2);
        assert!(layers[0].1.messages.is_empty(), "{:?}", layers[0].1.messages);
        // Nothing readable at all is an error.
        let only_shp: Vec<_> = entries.iter().filter(|(n, _)| n.ends_with(".SHP")).cloned().collect();
        assert!(read_zip("x.zip", &crate::zip::write(&only_shp).unwrap()).is_err());
        // Nothing to read.
        assert!(read_zip("x.zip", &crate::zip::write(&[("a.txt.bak".to_string(), vec![1])]).unwrap()).is_err());
        for n in (0..zip.len()).step_by(37) {
            let _ = read_zip("x.zip", &zip[..n]);
        }
        // A .shp alone is refused with a hint to zip it.
        assert!(read_bytes("x.shp", &entries[0].1).is_err());
    }
}
