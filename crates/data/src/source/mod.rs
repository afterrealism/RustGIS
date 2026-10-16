//! Data providers: vector sources read in place rather than loaded whole.
//!
//! A [`FeatureSource`] answers "the features in this extent, at most this many" from the file's own
//! spatial index (the GeoPackage R-tree, the FlatGeobuf packed Hilbert R-tree, GeoParquet row-group
//! bounding boxes), so a layer can draw a dataset far larger than memory a view at a time.
//! Coordinates come out as WGS 84 longitude/latitude whatever the source's coordinate system.
//!
//! Sources are hostile input: malformed rows are skipped and counted, never a panic.

#[cfg(not(target_arch = "wasm32"))]
pub mod duckdb;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod external;
pub mod fgb;
#[cfg(not(target_arch = "wasm32"))]
pub mod gdal;
#[cfg(not(target_arch = "wasm32"))]
pub mod geojson;
#[cfg(not(target_arch = "wasm32"))]
pub mod gpkg;
mod gpkg_blob;
pub mod gpkg_mem;
pub mod lidar;
pub mod parquet;
pub mod pgconn;
#[cfg(not(target_arch = "wasm32"))]
pub mod postgis;
pub mod remote;
#[cfg(not(target_arch = "wasm32"))]
pub mod shp;

use crate::raster::{Crs, Transformer};
use crate::{DataError, Dataset, Feature, Field, Geometry, GeometryType, ReadReport, Result};
use rustgis_geom::{Coord, Extent};
use std::path::Path;

/// One kind of geometry, for a layer that mixes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Family {
    Points,
    Lines,
    Polygons,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Points, Family::Lines, Family::Polygons];

    pub fn label(self) -> &'static str {
        match self {
            Family::Points => "points",
            Family::Lines => "lines",
            Family::Polygons => "polygons",
        }
    }

    /// An OGR SQL filter on the geometry type.
    pub fn filter(self) -> &'static str {
        match self {
            Family::Points => "OGR_GEOMETRY IN ('POINT', 'MULTIPOINT')",
            Family::Lines => "OGR_GEOMETRY IN ('LINESTRING', 'MULTILINESTRING')",
            Family::Polygons => "OGR_GEOMETRY IN ('POLYGON', 'MULTIPOLYGON')",
        }
    }
}

/// File types read through a provider.
pub const EXTENSIONS: &[&str] = &["gpkg", "parquet", "geoparquet", "fgb", "las", "laz"];

/// The most features one read returns, whatever the request asks for.
pub const MAX_LIMIT: usize = if cfg!(target_arch = "wasm32") { 500_000 } else { 2_000_000 };

/// Whether a file name is read through a provider.
pub fn is_source_name(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

/// What a source holds.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    /// Layer name (the table, or the file name).
    pub name: String,
    /// "GeoPackage", "GeoParquet" or "FlatGeobuf".
    pub format: &'static str,
    pub geometry_type: GeometryType,
    pub fields: Vec<Field>,
    /// Longitude/latitude extent, empty when the file doesn't say.
    pub extent: Extent,
    /// Number of features, when known without reading them.
    pub count: Option<u64>,
    /// The source's coordinate system, as `EPSG:…`.
    pub crs: String,
    /// Whether extent queries use a spatial index rather than a scan.
    pub indexed: bool,
    /// Things worth telling whoever adds the source (an unknown coordinate system read as
    /// longitude/latitude, tables left out).
    pub notes: Vec<String>,
}

/// A read: the features intersecting `extent` (longitude/latitude; `None` = all), at most `limit`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub extent: Option<Extent>,
    pub limit: usize,
}

impl Request {
    /// Every feature, up to [`MAX_LIMIT`].
    pub fn all() -> Self {
        Request { extent: None, limit: MAX_LIMIT }
    }
}

/// The answer to a [`Request`].
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub features: Vec<Feature>,
    /// How many features the request matched, before the limit (an estimate where the index only
    /// narrows to blocks of rows).
    pub matched: u64,
    /// Whether every matching feature is in `features`. When the limit is hit, providers return an
    /// evenly spread sample of the matches rather than the first `limit`, so the map stays covered.
    pub complete: bool,
    /// Rows that couldn't be read.
    pub skipped: usize,
}

/// A vector data source read in place.
pub trait FeatureSource: Send + Sync {
    fn info(&self) -> &SourceInfo;
    /// Reads the features a request asks for. A source read by its web address fails with
    /// [`remote::NEED`] until the bytes the read needs are fetched: run it with
    /// [`remote::drive`] (see [`FeatureSource::remote`]).
    fn read(&self, req: &Request) -> Result<Batch>;
    /// The file a source reads by its web address, a range at a time.
    fn remote(&self) -> Option<std::sync::Arc<remote::Remote>> {
        None
    }
}

/// Opens a file through its provider. `layer` names a table in a GeoPackage (default: the first).
pub fn open_path(path: &Path, layer: Option<&str>) -> Result<Box<dyn FeatureSource>> {
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    match ext(path).as_str() {
        "parquet" | "geoparquet" => Ok(Box::new(parquet::ParquetSource::open_path(path, &name)?)),
        "fgb" => Ok(Box::new(fgb::FgbSource::open_path(path, &name)?)),
        "las" | "laz" => Ok(Box::new(lidar::LidarSource::open_path(path, &name)?)),
        #[cfg(not(target_arch = "wasm32"))]
        "gpkg" => Ok(Box::new(gpkg::GpkgSource::open(path, layer)?)),
        // Only large GeoJSON files are read in place (see `geojson`); `is_source_name` leaves
        // them out.
        #[cfg(not(target_arch = "wasm32"))]
        "geojson" | "json" => Ok(Box::new(geojson::GeoJsonSource::open_path(path, &name)?)),
        // Only large Shapefiles are read in place (see `shp`); `is_source_name` leaves them out.
        #[cfg(not(target_arch = "wasm32"))]
        "shp" => Ok(Box::new(shp::ShpSource::open_path(path, &name)?)),
        other => {
            let _ = layer;
            Err(DataError::Unsupported(other.to_string()))
        }
    }
}

/// Opens a file held in memory (a browser upload).
pub fn open_bytes(file_name: &str, bytes: Vec<u8>, layer: Option<&str>) -> Result<Box<dyn FeatureSource>> {
    open_shared(file_name, std::sync::Arc::new(bytes), layer)
}

/// File bytes shared without copying (a layer keeps them for reopening the source).
#[derive(Clone, Debug)]
pub struct Shared(pub std::sync::Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Opens a file held in memory, sharing its bytes rather than copying them.
pub fn open_shared(file_name: &str, bytes: std::sync::Arc<Vec<u8>>, layer: Option<&str>) -> Result<Box<dyn FeatureSource>> {
    let path = Path::new(file_name);
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    let bytes = Shared(bytes);
    match ext(path).as_str() {
        "parquet" | "geoparquet" => Ok(Box::new(parquet::ParquetSource::open_shared(bytes, &name)?)),
        "fgb" => Ok(Box::new(fgb::FgbSource::open_shared(bytes, &name)?)),
        "las" | "laz" => Ok(Box::new(lidar::LidarSource::open_bytes(bytes.as_ref(), &name)?)),
        // The desktop reads GeoPackages with SQLite (and its R-tree); the browser, which has no
        // SQLite, with the pure-Rust reader.
        #[cfg(not(target_arch = "wasm32"))]
        "gpkg" => Ok(Box::new(gpkg::GpkgSource::open_bytes(bytes.as_ref(), layer)?)),
        #[cfg(target_arch = "wasm32")]
        "gpkg" => Ok(Box::new(gpkg_mem::GpkgMemSource::open_shared(bytes, layer)?)),
        other => {
            let _ = layer;
            Err(DataError::Unsupported(other.to_string()))
        }
    }
}

/// Whether a web address names a file read in place by range: a FlatGeobuf or a GeoParquet.
pub fn is_remote_name(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    // `s3://` too: the app's fetchers check it and read it from its bucket's web address (signed
    // only for a connection that serves the bucket, see rustgis_tiles::s3).
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("s3://"))
        && [".fgb", ".parquet", ".geoparquet"].iter().any(|e| path.ends_with(e))
}

/// A layer name from a web address: its file name without the extension.
fn url_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.rsplit_once('.').map(|(a, _)| a).unwrap_or(file);
    if stem.is_empty() { "layer".to_string() } else { stem.to_string() }
}

/// One attempt at opening a file by its web address from what is fetched of it so far: fails
/// with [`remote::NEED`] until its header (or footer) is in. See [`open_url`].
pub fn open_url_now(url: &str) -> Result<Box<dyn FeatureSource>> {
    let file = remote::file(url);
    let name = url_name(url);
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if path.ends_with(".fgb") {
        Ok(Box::new(fgb::FgbSource::open_remote(file, &name)?))
    } else if path.ends_with(".parquet") || path.ends_with(".geoparquet") {
        Ok(Box::new(parquet::ParquetSource::open_remote(file, &name)?))
    } else {
        Err(DataError::Unsupported(format!("{url}: only FlatGeobuf and GeoParquet are read in place by address")))
    }
}

/// Opens a file by its web address, fetching what it needs (off the caller's thread).
pub fn open_url(url: &str, done: Box<dyn FnOnce(Result<Box<dyn FeatureSource>>) + Send>) {
    let u = url.to_string();
    remote::drive(remote::file(url), std::sync::Arc::new(move || open_url_now(&u)), done);
}

/// [`open_url`], waiting for it (the command line and tools).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_url_blocking(url: &str) -> Result<Box<dyn FeatureSource>> {
    let u = url.to_string();
    remote::drive_blocking(remote::file(url), std::sync::Arc::new(move || open_url_now(&u)))
}

/// Reads a request, fetching what a source read by its web address needs, and waiting for it.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_blocking(src: &std::sync::Arc<dyn FeatureSource>, req: &Request) -> Result<Batch> {
    match src.remote() {
        Some(file) => {
            let (s, r) = (src.clone(), *req);
            remote::drive_blocking(file, std::sync::Arc::new(move || s.read(&r)))
        }
        None => src.read(req),
    }
}

/// A source whose reads wait for the bytes a source by its web address needs (tools and Load
/// All Features on the desktop).
#[cfg(not(target_arch = "wasm32"))]
pub struct Fetching(pub std::sync::Arc<dyn FeatureSource>);

#[cfg(not(target_arch = "wasm32"))]
impl FeatureSource for Fetching {
    fn info(&self) -> &SourceInfo {
        self.0.info()
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        read_blocking(&self.0, req)
    }
}

/// The vector layers in a file: the feature tables of a GeoPackage, or the one layer of others.
pub fn layers(path: &Path) -> Result<Vec<String>> {
    match ext(path).as_str() {
        #[cfg(not(target_arch = "wasm32"))]
        "gpkg" => gpkg::layers(path),
        _ => Ok(vec![open_path(path, None)?.info().name.clone()]),
    }
}

/// Reads a whole source into a dataset.
pub fn materialize(src: &dyn FeatureSource) -> Result<(Dataset, ReadReport)> {
    let info = src.info();
    let batch = src.read(&Request::all())?;
    let mut ds = Dataset::new(info.name.clone(), info.geometry_type, info.fields.clone());
    ds.features = batch.features.into();
    let mut report = ReadReport { skipped: batch.skipped, messages: info.notes.iter().map(|n| format!("{}: {n}", info.name)).collect() };
    if !batch.complete {
        report.messages.push(format!("{}: read a sample of {} of {} features", info.name, ds.features.len(), batch.matched));
    }
    if ds.geometry_type == GeometryType::None
        && let Some(g) = ds.features.iter().find_map(|f| f.geometry.as_ref())
    {
        ds.geometry_type = g.kind();
    }
    Ok((ds, report))
}

/// The geometry type of a source that doesn't declare one, from its first feature.
pub(crate) fn peek_kind(src: &dyn FeatureSource) -> GeometryType {
    src.read(&Request { extent: None, limit: 1 })
        .ok()
        .and_then(|b| b.features.into_iter().find_map(|f| f.geometry))
        .map(|g| g.kind())
        .unwrap_or(GeometryType::None)
}

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// A source's coordinate system: converts its geometry to longitude/latitude and a request's
/// extent to its own coordinates.
#[derive(Clone, Debug)]
pub(crate) struct Reproject {
    crs: Crs,
    /// `None` for longitude/latitude.
    t: Option<Transformer>,
}

impl Reproject {
    /// From an EPSG code; an unknown or missing code is taken as longitude/latitude, with a note.
    pub(crate) fn from_epsg(code: Option<u32>, notes: &mut Vec<String>) -> Reproject {
        let crs = match code {
            None | Some(0) => Crs::LonLat,
            Some(c) => match Crs::from_epsg(c) {
                Some(crs) => crs,
                None => {
                    notes.push(format!("unknown coordinate system EPSG:{c}; read as longitude/latitude"));
                    Crs::LonLat
                }
            },
        };
        Reproject::from_crs(crs, notes)
    }

    /// From a coordinate system; one that can't be converted is taken as longitude/latitude, with
    /// a note.
    pub(crate) fn from_crs(crs: Crs, notes: &mut Vec<String>) -> Reproject {
        let t = match crs {
            Crs::LonLat => None,
            other => match other.transformer() {
                Ok(t) => Some(t),
                Err(e) => {
                    notes.push(format!("{e}; read as longitude/latitude"));
                    None
                }
            },
        };
        Reproject { crs: if t.is_some() { crs } else { Crs::LonLat }, t }
    }

    /// Whether coordinates are taken as they are.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn is_lonlat(&self) -> bool {
        self.t.is_none()
    }

    pub(crate) fn label(&self) -> String {
        self.crs.label()
    }

    /// The geometry in longitude/latitude; `None` when a coordinate can't be converted.
    pub(crate) fn geometry(&self, g: Geometry) -> Option<Geometry> {
        let Some(t) = &self.t else { return Some(g) };
        let mut ok = true;
        let out = g.map_coords(&|c| t.to_lonlat(c).unwrap_or(Coord::new(f64::NAN, f64::NAN)));
        out.for_each_coord(|c| ok &= c.is_finite());
        ok.then_some(out)
    }

    /// A native extent in longitude/latitude.
    pub(crate) fn extent_to_lonlat(&self, e: &Extent) -> Extent {
        match &self.t {
            None => *e,
            Some(t) => around(e, |c| t.to_lonlat(c)),
        }
    }

    /// A longitude/latitude extent in native coordinates.
    pub(crate) fn extent_from_lonlat(&self, e: &Extent) -> Extent {
        match &self.t {
            None => *e,
            Some(t) => {
                // Clamp to the valid range first: projections fail near the poles.
                let c = Extent::new(e.min_x.max(-180.0), e.min_y.max(-89.9), e.max_x.min(180.0), e.max_y.min(89.9));
                around(&c, |p| t.from_lonlat(p))
            }
        }
    }
}

/// The extent of an extent's edges (sampled, since edges curve when reprojected) through `f`.
fn around(e: &Extent, f: impl Fn(Coord) -> Option<Coord>) -> Extent {
    let mut out = Extent::EMPTY;
    if e.is_empty() {
        return out;
    }
    const N: usize = 16;
    for i in 0..=N {
        let t = i as f64 / N as f64;
        let x = e.min_x + t * e.width();
        let y = e.min_y + t * e.height();
        for c in [Coord::new(x, e.min_y), Coord::new(x, e.max_y), Coord::new(e.min_x, y), Coord::new(e.max_x, y)] {
            if let Some(p) = f(c) {
                out.expand(p);
            }
        }
    }
    out
}

/// The step that keeps at most `limit` of `matched` items, evenly spread (1 when they all fit,
/// including when `matched` is 0, which `Sampler::new` relies on).
pub(crate) fn stride(matched: u64, limit: usize) -> u64 {
    if limit == 0 {
        return u64::MAX;
    }
    matched.div_ceil(limit as u64).max(1)
}

/// Keeps an evenly spread sample of at most `limit` items from a stream of unknown length: every
/// `step`-th item, doubling the step (and dropping every other kept item) whenever the sample
/// overflows.
pub(crate) struct Sampler<T> {
    limit: usize,
    step: u64,
    seen: u64,
    pub(crate) items: Vec<T>,
}

impl<T> Sampler<T> {
    pub(crate) fn new(limit: usize) -> Self {
        Self::with_total(limit, 0)
    }

    /// A sampler for a stream of about `total` items: it starts at the step that fits them.
    pub(crate) fn with_total(limit: usize, total: u64) -> Self {
        let limit = limit.max(1);
        Sampler { limit, step: stride(total, limit), seen: 0, items: Vec::new() }
    }

    /// Counts a match; whether it would be kept (so the caller decodes only those).
    pub(crate) fn wants(&mut self) -> bool {
        self.seen += 1;
        (self.seen - 1).is_multiple_of(self.step)
    }

    /// Keeps an item `wants` asked for.
    pub(crate) fn keep(&mut self, item: T) {
        self.items.push(item);
        if self.items.len() > self.limit {
            let mut i = 0;
            self.items.retain(|_| {
                i += 1;
                (i - 1) % 2 == 0
            });
            self.step = self.step.saturating_mul(2);
        }
    }

    /// Matches counted so far.
    pub(crate) fn seen(&self) -> u64 {
        self.seen
    }

    /// Whether every match was kept.
    pub(crate) fn complete(&self) -> bool {
        self.step == 1
    }
}

/// The limit a request may use.
pub(crate) fn clamp_limit(limit: usize) -> usize {
    limit.min(MAX_LIMIT)
}

/// Whether a feature's geometry falls in a request's extent (always true without one).
pub(crate) fn in_request(g: &Geometry, req: &Request) -> bool {
    match &req.extent {
        Some(e) => g.extent().intersects(e),
        None => true,
    }
}

#[cfg(test)]
mod tests;
