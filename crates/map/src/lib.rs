//! L2: the map model — layers with symbology and labels, the camera, colour ramps and basemaps.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod antimeridian;
pub mod camera;
pub mod color;
pub mod cull;
pub mod design;
pub mod effects;
pub mod globe;
pub mod graphics;
pub mod layout;
pub mod lod;
pub mod palette;
pub mod presentation;
pub mod projection;
pub mod raster;
pub mod report;
pub mod source;
pub mod symbology;
pub mod tiles;
pub mod view;
pub mod world;

pub use camera::Camera;
pub use color::{Ramp, Rgba};
pub use globe::{Globe, Projection};
pub use raster::{RasterLayer, RasterRenderer, RasterStyle, Resampling, Stretch};
pub use symbology::{ClassMethod, LabelPlacement, Labeling, Marker, Renderer, Symbol};
pub use tiles::{TileKind, TileLayer};

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent};
use serde::{Deserialize, Serialize};

/// The backdrop drawn under every layer. Each is drawn from the built-in world outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basemap {
    #[default]
    Paper,
    Midnight,
    Ocean,
    Atlas,
    Blank,
}

impl Basemap {
    pub const ALL: [Basemap; 5] = [Basemap::Paper, Basemap::Midnight, Basemap::Ocean, Basemap::Atlas, Basemap::Blank];
    pub fn label(self) -> &'static str {
        match self {
            Basemap::Paper => "Paper",
            Basemap::Midnight => "Midnight",
            Basemap::Ocean => "Ocean",
            Basemap::Atlas => "Atlas",
            Basemap::Blank => "Blank",
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Basemap::Paper => "paper",
            Basemap::Midnight => "midnight",
            Basemap::Ocean => "ocean",
            Basemap::Atlas => "atlas",
            Basemap::Blank => "blank",
        }
    }
    pub fn from_code(s: &str) -> Option<Basemap> {
        Basemap::ALL.into_iter().find(|b| b.code().eq_ignore_ascii_case(s.trim()) || b.label().eq_ignore_ascii_case(s.trim()))
    }
    /// (water, land, land outline, graticule) colours.
    pub fn palette(self) -> (Rgba, Rgba, Rgba, Rgba) {
        match self {
            Basemap::Paper => {
                (Rgba::rgb(0xDD, 0xE4, 0xEA), Rgba::rgb(0xF6, 0xF5, 0xF1), Rgba::rgb(0xC9, 0xC9, 0xC4), Rgba::new(0x9A, 0xA8, 0xB4, 60))
            }
            Basemap::Midnight => {
                (Rgba::rgb(0x1A, 0x1F, 0x26), Rgba::rgb(0x2E, 0x33, 0x3A), Rgba::rgb(0x47, 0x4D, 0x55), Rgba::new(0x80, 0x90, 0xA0, 40))
            }
            Basemap::Ocean => {
                (Rgba::rgb(0x9C, 0xC6, 0xE0), Rgba::rgb(0xE9, 0xE4, 0xD4), Rgba::rgb(0xB8, 0xB0, 0x98), Rgba::new(0xFF, 0xFF, 0xFF, 90))
            }
            Basemap::Atlas => {
                (Rgba::rgb(0xB5, 0xD5, 0xE6), Rgba::rgb(0xDC, 0xE8, 0xC8), Rgba::rgb(0xA8, 0xB4, 0x90), Rgba::new(0x60, 0x80, 0x90, 50))
            }
            Basemap::Blank => (Rgba::rgb(0xFF, 0xFF, 0xFF), Rgba::rgb(0xFF, 0xFF, 0xFF), Rgba::new(0, 0, 0, 0), Rgba::new(0, 0, 0, 0)),
        }
    }
}

/// A layer: a dataset drawn with a renderer and labels.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub dataset: Dataset,
    pub renderer: Renderer,
    pub labels: Labeling,
    /// 0 = opaque, 100 = invisible.
    pub transparency: f32,
    /// Drawn only when zoomed in beyond 1:`min_scale` (out-beyond limit), if set.
    #[serde(default)]
    pub min_scale: Option<f64>,
    /// Drawn only when zoomed out beyond 1:`max_scale` (in-beyond limit), if set.
    #[serde(default)]
    pub max_scale: Option<f64>,
    /// Definition query: only matching features draw.
    #[serde(default)]
    pub definition_query: Option<String>,
    /// Selected rows (indices into `dataset.features`).
    #[serde(default)]
    pub selection: Vec<usize>,
    /// Whether the Layers pane shows the legend classes.
    #[serde(default = "yes")]
    pub expanded: bool,
    #[serde(default = "yes")]
    pub selectable: bool,
    /// A graphics layer: each feature carries its own style (see [`graphics`]).
    #[serde(default)]
    pub graphics: bool,
    /// How the layer blends with what's beneath.
    #[serde(default)]
    pub blend: effects::Blend,
    /// A polygon layer (by name) outside whose shapes this layer is hidden.
    #[serde(default)]
    pub mask: Option<String>,
    /// Scale-dependent feature filters.
    #[serde(default)]
    pub display_filters: Vec<effects::DisplayFilter>,
    /// Points drawn as counted bins.
    #[serde(default)]
    pub aggregation: Option<effects::Aggregation>,
    /// Polygons drawn extruded.
    #[serde(default)]
    pub extrusion: Option<effects::Extrusion>,
    /// Layers with a higher priority draw above lower ones, whatever their order (the scene's
    /// depth priority).
    #[serde(default)]
    pub priority: i32,
    /// Domains, subtypes, attribute rules, contingent values, archive and templates.
    #[serde(default, skip_serializing_if = "design::Design::is_empty")]
    pub design: design::Design,
    /// Set for raster layers (whose `dataset` is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raster: Option<RasterLayer>,
    /// Set for streamed layers, whose `dataset` holds the features of the last view read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<source::SourceLayer>,
    /// Shared, like the features: an undo snapshot or other copy of the layer doesn't copy it.
    #[serde(skip)]
    projected: std::sync::Arc<Vec<Option<Geometry>>>,
    /// Each projected geometry's extent, for culling without walking its vertices.
    #[serde(skip)]
    projected_extents: std::sync::Arc<Vec<Extent>>,
    /// The projected geometry's line and ring vertices (those thinning can drop).
    #[serde(skip)]
    projected_vertices: usize,
    /// Thinned copies of the projected geometry by level, the latest used first (see [`lod`]);
    /// `None` for a level that draws from the full geometry, thinning saving too little there.
    #[serde(skip)]
    thinned: std::sync::Arc<Vec<(i32, Option<std::sync::Arc<Vec<Option<Geometry>>>>)>>,
    /// The cull index over `projected_extents` (see [`cull`]), built eagerly during projection
    /// and dropped with every reprojection. Arc-shared: a `Layer` clone (undo snapshots, the
    /// render worker's map clone) bumps a reference instead of copying ~12 bytes per feature.
    #[serde(skip)]
    cull_index: Option<std::sync::Arc<cull::CullIndex>>,
    #[serde(skip)]
    projected_rev: u64,
    /// The projection the cache is in.
    #[serde(skip)]
    projected_for: String,
    /// Bumped whenever geometry changes so caches rebuild.
    #[serde(default)]
    pub geometry_rev: u64,
}

fn yes() -> bool {
    true
}

impl Layer {
    pub fn new(id: u32, dataset: Dataset) -> Self {
        let renderer = Renderer::Single(Symbol::default_for(dataset.geometry_type, id));
        Layer {
            id,
            name: dataset.name.clone(),
            visible: true,
            labels: Labeling::default_for(&dataset),
            dataset,
            renderer,
            transparency: 0.0,
            min_scale: None,
            max_scale: None,
            definition_query: None,
            selection: Vec::new(),
            expanded: true,
            selectable: true,
            graphics: false,
            blend: effects::Blend::Normal,
            mask: None,
            display_filters: Vec::new(),
            aggregation: None,
            extrusion: None,
            priority: 0,
            design: design::Design::default(),
            raster: None,
            source: None,
            projected: Default::default(),
            projected_extents: Default::default(),
            projected_vertices: 0,
            thinned: Default::default(),
            cull_index: None,
            projected_rev: u64::MAX,
            projected_for: String::new(),
            geometry_rev: 0,
        }
    }

    /// A raster layer.
    pub fn new_raster(id: u32, name: impl Into<String>, raster: RasterLayer) -> Self {
        let name = name.into();
        let mut l = Layer::new(id, Dataset::new(name.clone(), GeometryType::None, Vec::new()));
        l.selectable = false;
        l.labels.enabled = false;
        l.raster = Some(raster);
        l
    }

    /// A graphics layer.
    pub fn new_graphics(id: u32, name: &str) -> Self {
        let mut l = Layer::new(id, graphics::dataset(name));
        l.graphics = true;
        l.labels.enabled = false;
        l
    }

    /// A streamed layer: a large file read in place, a view at a time.
    pub fn new_streamed(id: u32, source: source::SourceLayer) -> Self {
        let (name, ty, fields) = match source.info() {
            Some(i) => (i.name.clone(), i.geometry_type, i.fields.clone()),
            None => ("layer".to_string(), GeometryType::None, Vec::new()),
        };
        let mut ds = Dataset::new(name, ty, fields);
        ds.windowed = true;
        ds.source =
            source.path.clone().or_else(|| source.file_name.clone()).or_else(|| source.postgis.as_ref().map(|p| p.database.clone()));
        let mut l = Layer::new(id, ds);
        l.source = Some(source);
        l
    }

    pub fn is_streamed(&self) -> bool {
        self.source.is_some()
    }

    /// Keeps a streamed layer's features up with the view (longitude/latitude): takes a finished
    /// read, and starts one when the view has moved beyond what was read. With `block`, reads in
    /// place and waits (for exports). Returns whether a read is still running.
    pub fn sync_source(&mut self, view: &Extent, block: bool) -> bool {
        let Some(src) = &mut self.source else { return false };
        if !src.is_open() {
            // Opened once after a project loads; a failure is kept, not retried every frame.
            if src.error.is_some() || src.open().is_err() {
                return false;
            }
            // A source by its web address opens off the frame: an export waits for it.
            if block {
                src.wait_open(std::time::Duration::from_secs(60));
            }
            if !src.is_open() {
                return src.is_pending();
            }
        }
        // A project that lists no fields for a streamed layer (one written by another app, which
        // can't know which columns the reader takes) gets the source's: reads keep the attributes
        // in that order, and a renderer on a field finds it.
        if self.dataset.fields.is_empty()
            && let Some(info) = self.source.as_ref().and_then(|s| s.info())
        {
            self.dataset.fields = info.fields.clone();
        }
        let Some(src) = &mut self.source else { return false };
        // A failed read leaves nothing drawn for it (its error shows in Layers), not the
        // previous view's features.
        match src.poll(block) {
            source::Polled::Done(_, batch) => self.install(batch.features),
            source::Polled::Failed(_) => self.install(Vec::new()),
            source::Polled::Idle | source::Polled::Waiting => {}
        }
        let Some(src) = &mut self.source else { return false };
        if let Some(e) = src.wanted(view) {
            src.start(e, block);
            match src.poll(block) {
                source::Polled::Done(_, batch) => self.install(batch.features),
                source::Polled::Failed(_) => self.install(Vec::new()),
                // A thread that couldn't start leaves an error and no read.
                source::Polled::Idle if src.error.is_some() => self.install(Vec::new()),
                source::Polled::Idle | source::Polled::Waiting => {}
            }
        }
        self.source.as_ref().is_some_and(|s| s.is_pending())
    }

    /// Replaces a streamed layer's features, keeping the selection (by feature id).
    fn install(&mut self, features: Vec<rustgis_data::Feature>) {
        let ids: std::collections::HashSet<u64> =
            self.selection.iter().filter_map(|r| self.dataset.features.get(*r)).map(|f| f.id).collect();
        let fields = self.dataset.fields.len();
        self.dataset.features = features.into();
        for f in &mut self.dataset.features {
            f.attrs.resize(fields, rustgis_data::Value::Null);
        }
        self.dataset.windowed = true;
        if let Some(src) = &mut self.source {
            src.reads = src.reads.wrapping_add(1);
        }
        self.selection = if ids.is_empty() {
            Vec::new()
        } else {
            self.dataset.features.iter().enumerate().filter(|(_, f)| ids.contains(&f.id)).map(|(i, _)| i).collect()
        };
        self.touch_geometry();
    }

    /// Every feature of a streamed layer's source (up to the read limit), for tools and for
    /// loading the layer into memory.
    pub fn load_all(&self) -> Result<Dataset, String> {
        let Some(src) = &self.source else { return Ok(self.dataset.clone()) };
        let mut copy = src.clone();
        if !copy.is_open() {
            copy.open()?;
        }
        let (mut ds, _) = copy.read_all()?;
        // A sample isn't all of them: refuse rather than run a tool on part of the data.
        let max = rustgis_data::source::MAX_LIMIT;
        if let Some(n) = copy.info().and_then(|i| i.count).filter(|n| (ds.features.len() as u64) < *n && *n > max as u64) {
            return Err(format!("the layer has {n} features, more than the {max} read at once"));
        }
        // A count the source doesn't know, and a read that hit the limit: maybe not all of them.
        if copy.info().is_some_and(|i| i.count.is_none()) && ds.features.len() >= max {
            return Err(format!("the layer has more features than the {max} read at once"));
        }
        ds.name = self.dataset.name.clone();
        // (A project that lists no fields keeps the source's, as `sync_source` gives it.)
        if !self.dataset.fields.is_empty() {
            ds.fields = self.dataset.fields.clone();
        }
        ds.source = self.dataset.source.clone();
        Ok(ds)
    }

    /// The symbol a row draws with.
    pub fn symbol(&self, row: usize) -> Symbol {
        if self.graphics { graphics::symbol(&self.dataset, row) } else { self.renderer.symbol_for(&self.dataset, row) }
    }

    pub fn is_raster(&self) -> bool {
        self.raster.is_some()
    }

    pub fn geometry_type(&self) -> GeometryType {
        self.dataset.geometry_type
    }

    /// Copies of every geometry in a map projection's coordinates (`key` names the projection),
    /// rebuilt lazily when the geometry or the projection changes. Points a projection can't show
    /// become NaN, which the renderer skips; features across a world projection's edge are cut
    /// there ([`projection::Flat::project`]). There is one entry per feature, row for row, but its
    /// parts and vertices needn't match the source's (a cut feature has more parts, and a point
    /// the projection can't use breaks a line): hit-testing and snapping use the shape, never a
    /// vertex index, which only the source geometry has.
    pub fn projected_in(&mut self, flat: &projection::Flat, key: &str) -> &[Option<Geometry>] {
        if self.projected_rev != self.geometry_rev || self.projected.len() != self.dataset.features.len() || self.projected_for != key {
            let projected: Vec<Option<Geometry>> =
                self.dataset.features.iter().map(|ft| ft.geometry.as_ref().map(|g| flat.project(g))).collect();
            self.projected_extents =
                std::sync::Arc::new(projected.iter().map(|g| g.as_ref().map_or(Extent::EMPTY, Geometry::extent)).collect());
            self.projected_vertices = projected.iter().flatten().map(lod::thinnable).sum();
            self.projected = std::sync::Arc::new(projected);
            self.thinned = Default::default();
            // Eager cull-index build: piggybacks on the projection walk that already touched
            // every extent. The sort is O(n log n) but runs once, not on every frame. The
            // `>` gate matches the renderer's use exactly (`render` asks for the index only
            // above CULL_INDEX_ABOVE); smaller layers stay linear-scan.
            self.cull_index = if self.dataset.features.len() > cull::CULL_INDEX_ABOVE {
                Some(std::sync::Arc::new(cull::CullIndex::build(&self.projected_extents)))
            } else {
                None
            };
            self.projected_rev = self.geometry_rev;
            self.projected_for = key.to_string();
        }
        &self.projected
    }

    /// Web Mercator copies of every geometry (see [`Layer::projected_in`]).
    pub fn projected(&mut self) -> &[Option<Geometry>] {
        self.projected_in(&projection::Flat::Mercator, "mercator")
    }

    /// The projected geometry (see [`Layer::projected_in`]) as drawn at a tolerance in projection
    /// units: a thinned copy at the level just under it (see [`lod`]), made on first use, or the
    /// geometry itself for a small layer. Read it back with [`Layer::thinned_cache`].
    pub fn thinned_in(&mut self, flat: &projection::Flat, key: &str, tol: f64) -> &[Option<Geometry>] {
        self.projected_in(flat, key);
        let Some(level) = lod::level(tol).filter(|_| self.projected_vertices > lod::THIN_ABOVE) else { return &self.projected };
        let levels = std::sync::Arc::make_mut(&mut self.thinned);
        match levels.iter().position(|(l, _)| *l == level) {
            Some(i) => {
                let hit = levels.remove(i);
                levels.insert(0, hit);
            }
            None => {
                let t = lod::tolerance(level);
                let copy: Vec<Option<Geometry>> = self.projected.iter().map(|g| g.as_ref().map(|g| lod::thin(g, t))).collect();
                // The saving counts against every vertex the copy holds, points included.
                let all: usize = self.projected.iter().flatten().map(lod::vertices).sum();
                let kept: usize = copy.iter().flatten().map(lod::vertices).sum();
                let worth = (kept as f64) <= all as f64 * (1.0 - lod::MIN_SAVING);
                levels.insert(0, (level, worth.then(|| std::sync::Arc::new(copy))));
                levels.truncate(lod::LEVELS_KEPT);
            }
        }
        self.thinned_cache(tol)
    }

    /// The geometry from the last [`Layer::thinned_in`] call at this tolerance, without making
    /// it: the full projected geometry when there is no thinned copy for it.
    pub fn thinned_cache(&self, tol: f64) -> &[Option<Geometry>] {
        let level = lod::level(tol).filter(|_| self.projected_vertices > lod::THIN_ABOVE);
        match level.and_then(|level| self.thinned.iter().find(|(l, _)| *l == level)) {
            Some((_, Some(g))) if g.len() == self.projected.len() => g,
            _ => &self.projected,
        }
    }

    /// Each projected geometry's extent, row for row with [`Layer::projected_cache`].
    pub fn projected_extents(&self) -> &[Extent] {
        &self.projected_extents
    }

    /// The cull index over `projected_extents` (see [`cull`]), built eagerly during projection
    /// for layers above `CULL_INDEX_ABOVE`; `None` for smaller layers (linear scan is faster)
    /// or before the first projection. The renderer falls back to its linear extent scan when
    /// this returns `None`.
    pub fn cull_index_in(&mut self, flat: &projection::Flat, key: &str) -> Option<&cull::CullIndex> {
        self.projected_in(flat, key);
        self.cull_index.as_deref()
    }

    /// The Web Mercator copies from the last [`Layer::projected`] call, without refreshing them:
    /// call `projected` first (it needs `&mut self`) whenever the geometry may have changed.
    pub fn projected_cache(&self) -> &[Option<Geometry>] {
        &self.projected
    }

    /// Drops the projected geometry cache (the feature cache).
    pub fn clear_cache(&mut self) {
        self.projected = Default::default();
        self.projected_extents = Default::default();
        self.projected_vertices = 0;
        self.thinned = Default::default();
        self.projected_rev = u64::MAX;
        self.projected_for.clear();
    }

    /// Whether the projected geometry cache is filled for a projection.
    pub fn is_cached(&self, key: &str) -> bool {
        self.projected_for == key && self.projected_rev == self.geometry_rev && self.projected.len() == self.dataset.features.len()
    }

    /// Marks geometry as changed.
    pub fn touch_geometry(&mut self) {
        self.geometry_rev = self.geometry_rev.wrapping_add(1);
    }

    /// Whether the layer draws at a scale denominator.
    pub fn in_scale_range(&self, scale: f64) -> bool {
        self.min_scale.is_none_or(|m| scale <= m) && self.max_scale.is_none_or(|m| scale >= m)
    }

    /// Rows drawn at a scale denominator: those passing the definition query and every display
    /// filter that applies at that scale (a filter that no longer reads is skipped).
    pub fn drawn_rows(&self, scale: f64) -> Vec<bool> {
        let mut rows = self.visible_rows();
        for f in self.display_filters.iter().filter(|f| f.applies(scale)) {
            if let Ok(e) = rustgis_data::query::parse(&f.where_, &self.dataset) {
                for (r, keep) in rows.iter_mut().enumerate() {
                    *keep = *keep && rustgis_data::query::eval(&e, &self.dataset, r);
                }
            }
        }
        rows
    }

    /// The legend rows for what this layer draws at a scale denominator (#207): the classes of the
    /// features its definition query and display filters keep. `None` when it draws nothing there
    /// (out of its scale range, or no feature kept, as in an empty layer), so a legend can leave it out.
    pub fn shown_legend(&self, scale: f64) -> Option<Vec<symbology::LegendEntry>> {
        if !self.in_scale_range(scale) {
            return None;
        }
        // Nothing to filter: every feature is drawn, and no row list needs building.
        let query = self.definition_query.as_deref().is_some_and(|q| !q.trim().is_empty());
        if !query && self.display_filters.is_empty() {
            return (!self.dataset.features.is_empty()).then(|| self.renderer.legend_for(&self.dataset, &[]));
        }
        let rows = self.drawn_rows(scale);
        rows.contains(&true).then(|| self.renderer.legend_for(&self.dataset, &rows))
    }

    /// Rows passing the definition query (all rows when none or invalid).
    pub fn visible_rows(&self) -> Vec<bool> {
        let n = self.dataset.features.len();
        match self.definition_query.as_deref().filter(|q| !q.trim().is_empty()) {
            Some(q) => match rustgis_data::query::parse(q, &self.dataset) {
                Ok(e) => (0..n).map(|r| rustgis_data::query::eval(&e, &self.dataset, r)).collect(),
                Err(_) => vec![true; n],
            },
            None => vec![true; n],
        }
    }

    /// Extent in longitude/latitude.
    pub fn extent(&self) -> Extent {
        match (&self.raster, &self.source) {
            (Some(r), _) => r.data.as_ref().map(|d| d.extent()).unwrap_or(Extent::EMPTY),
            // A streamed layer's whole extent, not just the part read.
            (None, Some(s)) => s.info().map(|i| i.extent).filter(|e| !e.is_empty()).unwrap_or_else(|| self.dataset.extent()),
            (None, None) => self.dataset.extent(),
        }
    }

    /// Extent in longitude/latitude of the features the definition query lets through (the whole
    /// extent without a query, for rasters and for streamed layers, whose rows are only those read).
    pub fn shown_extent(&self) -> Extent {
        let query = self.definition_query.as_deref().is_some_and(|q| !q.trim().is_empty());
        if !query || self.raster.is_some() || self.source.is_some() {
            return self.extent();
        }
        self.visible_rows()
            .iter()
            .zip(&self.dataset.features)
            .filter(|(v, _)| **v)
            .filter_map(|(_, f)| f.geometry.as_ref())
            .fold(Extent::EMPTY, |e, g| e.union(&g.extent()))
    }

    pub fn selection_extent(&self) -> Extent {
        self.selection
            .iter()
            .filter_map(|r| self.dataset.features.get(*r))
            .filter_map(|f| f.geometry.as_ref())
            .fold(Extent::EMPTY, |e, g| e.union(&g.extent()))
    }

    pub fn opacity(&self) -> f32 {
        (1.0 - self.transparency / 100.0).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub camera: Camera,
}

/// What [`Map::layer_mark`] records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerMark {
    ids: Vec<u32>,
    next: u32,
}

/// A map: an ordered list of layers (index 0 draws on top), a basemap and a camera.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Map {
    pub name: String,
    pub layers: Vec<Layer>,
    pub basemap: Basemap,
    /// Web tile basemaps and overlays over the basemap and under the layers (index 0 on top).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiles: Vec<TileLayer>,
    pub camera: Camera,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default = "yes")]
    pub graticule: bool,
    /// Flat map or 3D globe.
    #[serde(default)]
    pub projection: Projection,
    /// Which layers a click identifies.
    #[serde(default)]
    pub popup_mode: PopupMode,
    /// Map-wide label display switches.
    #[serde(default)]
    pub label_display: LabelDisplay,
    /// The features labelled when labels were locked, as (layer id, row): only they are labelled
    /// while the lock holds, wherever the map moves. Filled by the renderer.
    #[serde(skip)]
    pub label_lock: Option<Vec<(u32, usize)>>,
    /// Relationships, snapping, topology rules and edit settings.
    #[serde(default)]
    pub editing: design::MapEditing,
    /// A layer shown only left of a divider (0–1 across the view): the swipe comparison.
    #[serde(default)]
    pub swipe: Option<(u32, f64)>,
    /// Effects, colour vision, drawing mode, clipping and navigation aids.
    #[serde(default)]
    pub view: view::ViewSettings,
    /// The map's thumbnail (base64 PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// Animation keyframes: views the animation moves through.
    #[serde(default)]
    pub animation: Vec<view::Keyframe>,
    #[serde(default)]
    next_layer_id: u32,
}

/// Which layers a click in the map identifies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PopupMode {
    /// The topmost feature under the pointer.
    #[default]
    Topmost,
    /// Every visible layer's feature under the pointer.
    Visible,
    /// Every selectable layer's features under the pointer.
    Selectable,
    /// Only the layer selected in the Layers pane.
    SelectedInContents,
    /// No pop-ups.
    None,
}

impl PopupMode {
    pub const ALL: [PopupMode; 5] =
        [PopupMode::Topmost, PopupMode::Visible, PopupMode::Selectable, PopupMode::SelectedInContents, PopupMode::None];
    pub fn code(self) -> &'static str {
        match self {
            PopupMode::Topmost => "topmost",
            PopupMode::Visible => "visible",
            PopupMode::Selectable => "selectable",
            PopupMode::SelectedInContents => "selectedInContents",
            PopupMode::None => "none",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PopupMode::Topmost => "Topmost Layer",
            PopupMode::Visible => "Visible Layers",
            PopupMode::Selectable => "Selectable Layers",
            PopupMode::SelectedInContents => "Selected in Layers",
            PopupMode::None => "No Pop-ups",
        }
    }
    pub fn from_code(s: &str) -> Option<PopupMode> {
        PopupMode::ALL.into_iter().find(|m| m.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// Map-wide label switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelDisplay {
    /// No labels are drawn (faster navigation).
    #[serde(default)]
    pub paused: bool,
    /// The labelled features stay fixed as the map moves.
    #[serde(default)]
    pub locked: bool,
    /// Labels that didn't fit are drawn too, in red.
    #[serde(default)]
    pub show_unplaced: bool,
}

impl Map {
    pub fn new(name: impl Into<String>) -> Self {
        Map {
            name: name.into(),
            layers: Vec::new(),
            basemap: Basemap::Paper,
            tiles: Vec::new(),
            camera: Camera::world(),
            bookmarks: Vec::new(),
            graticule: true,
            projection: Projection::WebMercator,
            popup_mode: PopupMode::Topmost,
            label_display: LabelDisplay::default(),
            label_lock: None,
            editing: design::MapEditing::default(),
            swipe: None,
            view: view::ViewSettings::default(),
            thumbnail: None,
            animation: Vec::new(),
            next_layer_id: 1,
        }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_layer_id.max(self.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1);
        self.next_layer_id = id + 1;
        id
    }

    /// The map's layers as they stand (their ids in order, and the next id), cheap to take before
    /// adding several layers that must all go in or none: see [`Map::restore_layers`].
    pub fn layer_mark(&self) -> LayerMark {
        LayerMark { ids: self.layers.iter().map(|l| l.id).collect(), next: self.next_layer_id }
    }

    /// Takes out the layers added since `mark` and puts the others back in its order, with the
    /// layer ids it had (the layers' data is never copied).
    pub fn restore_layers(&mut self, mark: &LayerMark) {
        self.layers.retain(|l| mark.ids.contains(&l.id));
        self.layers.sort_by_key(|l| mark.ids.iter().position(|id| *id == l.id).unwrap_or(usize::MAX));
        self.next_layer_id = mark.next;
    }

    /// Inserts a layer on top of the layers of its kind: points above lines above polygons above
    /// rasters, as GIS apps insert them.
    fn insert_ranked(&mut self, layer: Layer) {
        let rank = |l: &Layer| match (l.is_raster(), l.geometry_type()) {
            (true, _) => 4,
            (_, GeometryType::Point) => 0,
            (_, GeometryType::Polyline) => 1,
            (_, GeometryType::Polygon) => 2,
            (_, GeometryType::None) => 3,
        };
        let r = rank(&layer);
        let pos = self.layers.iter().position(|l| rank(l) >= r).unwrap_or(self.layers.len());
        self.layers.insert(pos, layer);
    }

    /// Adds a dataset as a new layer; returns its id.
    pub fn add_dataset(&mut self, ds: Dataset) -> u32 {
        let id = self.next_id();
        self.insert_ranked(Layer::new(id, ds));
        id
    }

    /// Adds a layer (from a layer file) with a fresh id among the layers of its kind; returns the id.
    pub fn add_layer(&mut self, mut layer: Layer) -> u32 {
        let id = self.next_id();
        layer.id = id;
        layer.selection.clear();
        layer.touch_geometry();
        self.insert_ranked(layer);
        id
    }

    /// Adds an empty graphics layer on top; returns its id.
    pub fn add_graphics(&mut self, name: &str) -> u32 {
        let id = self.next_id();
        self.layers.insert(0, Layer::new_graphics(id, name));
        id
    }

    /// Adds a streamed layer (a large file read in place); returns its id.
    pub fn add_streamed(&mut self, name: impl Into<String>, source: source::SourceLayer) -> u32 {
        let id = self.next_id();
        let mut l = Layer::new_streamed(id, source);
        l.name = name.into();
        l.dataset.name = l.name.clone();
        self.insert_ranked(l);
        id
    }

    /// Keeps streamed layers up with a view (longitude/latitude; see [`Layer::sync_source`]).
    /// Only visible layers read. Returns whether a read is still running.
    pub fn sync_sources(&mut self, view: &Extent, block: bool) -> bool {
        let mut pending = false;
        for l in self.layers.iter_mut().filter(|l| l.is_streamed() && l.visible) {
            pending |= l.sync_source(view, block);
        }
        pending
    }

    /// Adds a raster layer; returns its id.
    pub fn add_raster(&mut self, name: impl Into<String>, raster: RasterLayer) -> u32 {
        let id = self.next_id();
        self.insert_ranked(Layer::new_raster(id, name, raster));
        id
    }

    pub fn layer(&self, id: u32) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: u32) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
    pub fn layer_index(&self, id: u32) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }
    pub fn layer_by_name(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name.eq_ignore_ascii_case(name))
    }

    /// The flat projection of the camera's coordinates (Web Mercator for the globe).
    pub fn flat(&self) -> projection::Flat {
        projection::Flat::of(&self.projection)
    }

    /// Longitude/latitude to the camera's coordinates; `None` outside the projection's area of
    /// use or where it folds.
    pub fn to_map(&self, c: Coord) -> Option<Coord> {
        self.flat().forward_checked(c)
    }

    /// The camera's coordinates to longitude/latitude.
    pub fn to_lonlat(&self, m: Coord) -> Option<Coord> {
        self.flat().inverse(m)
    }

    /// The longitude/latitude at the centre of view.
    pub fn center_lonlat(&self) -> Coord {
        self.to_lonlat(self.camera.center).unwrap_or(Coord::new(0.0, 0.0))
    }

    /// Ground metres per logical pixel at the centre of view.
    pub fn ground_res(&self) -> f64 {
        self.ground_res_at(&self.camera)
    }

    /// Ground metres per logical pixel for another camera on this map (an export's view).
    pub fn ground_res_at(&self, camera: &Camera) -> f64 {
        camera.res * self.flat().ground_per_unit(camera.center)
    }

    /// Map scale denominator at the centre of view.
    pub fn scale(&self) -> f64 {
        self.ground_res() / rustgis_geom::PIXEL_M
    }

    /// Sets the resolution from a scale denominator.
    pub fn set_scale(&mut self, scale: f64) {
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let per = self.flat().ground_per_unit(self.camera.center).max(1e-9);
        self.camera.res = scale * rustgis_geom::PIXEL_M / per;
        self.camera.clamp();
    }

    /// Points the flat map at a longitude/latitude extent with a margin.
    pub fn fit_lonlat(&mut self, e: &Extent, w: f64, h: f64) {
        let flat = self.flat();
        if let projection::Flat::Mercator = flat {
            self.camera.fit_lonlat(e, w, h);
            return;
        }
        if e.is_empty() || w <= 1.0 || h <= 1.0 {
            return;
        }
        let min_pad = if e.width() > 0.0 || e.height() > 0.0 { 1e-6 } else { 0.02 };
        let m = flat.extent(&e.padded(0.06, min_pad));
        if m.is_empty() {
            return;
        }
        self.camera.center = m.center();
        self.camera.res = (m.width() / w).max(m.height() / h).max(camera::MIN_RES * 2.0);
        self.camera.clamp();
    }

    /// Switches projection, keeping the centre and the ground scale.
    /// Bookmarks move with it, so they still show the same place.
    pub fn set_projection(&mut self, p: Projection) {
        let old = self.flat();
        let ground_of = |c: &Camera| (old.inverse(c.center).unwrap_or(Coord::new(0.0, 0.0)), c.res * old.ground_per_unit(c.center));
        let camera = ground_of(&self.camera);
        let marks: Vec<_> = self.bookmarks.iter().map(|b| ground_of(&b.camera)).collect();
        self.projection = p;
        let flat = self.flat();
        let place = |cam: &mut Camera, (center, ground): (Coord, f64)| {
            // A centre the new projection can't show moves to its origin.
            cam.center = flat.forward_checked(center).or_else(|| flat.forward(Coord::new(0.0, 0.0))).unwrap_or(Coord::new(0.0, 0.0));
            cam.res = ground / flat.ground_per_unit(cam.center).max(1e-9);
            cam.clamp();
        };
        place(&mut self.camera, camera);
        for (b, m) in self.bookmarks.iter_mut().zip(marks) {
            place(&mut b.camera, m);
        }
    }

    /// Union of all layers' extents (lon/lat).
    pub fn full_extent(&self) -> Extent {
        self.layers.iter().filter(|l| l.visible).fold(Extent::EMPTY, |e, l| e.union(&l.extent()))
    }

    /// Union of the visible layers' shown extents (lon/lat): what Full Extent zooms to, narrowed by
    /// each layer's definition query.
    pub fn shown_extent(&self) -> Extent {
        self.layers.iter().filter(|l| l.visible).fold(Extent::EMPTY, |e, l| e.union(&l.shown_extent()))
    }

    pub fn selection_count(&self) -> usize {
        self.layers.iter().map(|l| l.selection.len()).sum()
    }

    pub fn selection_extent(&self) -> Extent {
        self.layers.iter().fold(Extent::EMPTY, |e, l| e.union(&l.selection_extent()))
    }

    pub fn clear_selection(&mut self) {
        for l in &mut self.layers {
            l.selection.clear();
        }
    }

    /// The feature drawn on top under a point in the camera's coordinates, among the visible layers
    /// `keep` accepts: (layer id, row). Draw order and the rows drawn are those of [`Map::hits_at`].
    pub fn hit(&mut self, p: Coord, tol: f64, scale: f64, keep: impl Fn(&Layer) -> bool) -> Option<(u32, usize)> {
        self.hits_at(p, tol, scale, keep, true, 1).into_iter().next()
    }

    /// Every feature under a point in the visible layers `keep` accepts: top layer first and, in
    /// a layer, the one drawn on top first. At most `limit` of them; with `first_layer`, only
    /// those of the top layer with any.
    pub fn hits_at(
        &mut self,
        p: Coord,
        tol: f64,
        scale: f64,
        keep: impl Fn(&Layer) -> bool,
        first_layer: bool,
        limit: usize,
    ) -> Vec<(u32, usize)> {
        let (flat, key) = (self.flat(), self.projection.code());
        let mut out = Vec::new();
        // Top first, as drawn: a higher depth priority is above whatever the order.
        let mut order: Vec<usize> = (0..self.layers.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(self.layers.get(*i).map_or(0, |l| l.priority)));
        for i in order {
            let Some(l) = self.layers.get_mut(i) else { continue };
            if out.len() >= limit || (first_layer && !out.is_empty()) {
                break;
            }
            if !l.visible || !l.in_scale_range(scale) || !keep(l) {
                continue;
            }
            // Only features drawn: the definition query and the display filters let them through.
            let rows = l.drawn_rows(scale);
            let id = l.id;
            let proj = l.projected_in(&flat, &key);
            let hits = proj
                .iter()
                .enumerate()
                .rev()
                .filter(|(row, g)| rows.get(*row) == Some(&true) && g.as_ref().is_some_and(|g| g.hit(p, tol)))
                .map(|(row, _)| (id, row));
            out.extend(hits.take(limit - out.len()));
        }
        out
    }

    /// Rows of each selectable, visible layer inside a rectangle in the camera's coordinates.
    pub fn hits_in(&mut self, e: &Extent, scale: f64) -> Vec<(u32, Vec<usize>)> {
        let (flat, key) = (self.flat(), self.projection.code());
        let mut out = Vec::new();
        for l in &mut self.layers {
            if !l.visible || !l.selectable || !l.in_scale_range(scale) {
                continue;
            }
            // Only features drawn: the definition query and the display filters let them through.
            let rows = l.drawn_rows(scale);
            let id = l.id;
            let hits: Vec<usize> = l
                .projected_in(&flat, &key)
                .iter()
                .enumerate()
                .filter(|(r, g)| rows.get(*r) == Some(&true) && g.as_ref().is_some_and(|g| g.intersects_extent(e)))
                .map(|(r, _)| r)
                .collect();
            if !hits.is_empty() {
                out.push((id, hits));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, FieldType, Value};
    use rustgis_geom::lonlat_to_merc;

    fn points() -> Dataset {
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("n", FieldType::Integer)]);
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(1)]);
        d.push(Some(Geometry::Point(Coord::new(10.0, 10.0))), vec![Value::Int(2)]);
        d
    }

    /// A layer of one long line along the equator with `n` vertices, with a gentle wiggle so
    /// shape-aware (area-based) thinning has detail to measure at every level: a perfectly
    /// straight line thins to its ends at any tolerance, which these caching tests don't mean
    /// to exercise.
    fn long_line(n: usize) -> Layer {
        let mut d = Dataset::new("line", GeometryType::Polyline, vec![]);
        let run = (0..n)
            .map(|i| Coord::new(-170.0 + 340.0 * i as f64 / n as f64, 0.005 * (std::f64::consts::TAU * i as f64 / 40.0).sin()))
            .collect();
        d.push(Some(Geometry::Line(vec![run])), vec![]);
        Layer::new(1, d)
    }

    #[test]
    fn a_detailed_layer_draws_from_a_thinned_copy() {
        let flat = projection::Flat::Mercator;
        let mut l = long_line(lod::THIN_ABOVE + 1000);
        let full = lod::vertices(l.projected()[0].as_ref().unwrap());
        // Seen whole (about 40 km a pixel), one vertex every 8 km (the level under 12 km) is enough.
        let thinned = lod::vertices(l.thinned_in(&flat, "mercator", 12_000.0)[0].as_ref().unwrap());
        assert!(thinned < full / 40, "{thinned} of {full} vertices drawn");
        assert_eq!(lod::vertices(l.thinned_cache(12_000.0)[0].as_ref().unwrap()), thinned);
        // Zoomed in past its vertex spacing, every vertex draws.
        assert_eq!(lod::vertices(l.thinned_in(&flat, "mercator", 0.001)[0].as_ref().unwrap()), full);
        // Its extents are cached row for row, and a clone shares the caches.
        assert_eq!(l.projected_extents().len(), 1);
        let copy = l.clone();
        assert_eq!(copy.projected_cache().as_ptr(), l.projected_cache().as_ptr());
        assert_eq!(copy.thinned_cache(12_000.0).as_ptr(), l.thinned_cache(12_000.0).as_ptr());
        // A change of geometry drops the thinned copies with the projection.
        l.touch_geometry();
        l.projected();
        assert_eq!(l.thinned_cache(12_000.0).as_ptr(), l.projected_cache().as_ptr());
    }

    fn many_points(n: usize) -> Layer {
        // n separate point features: the cull gate counts features, not vertices.
        let mut d = Dataset::new("pts", GeometryType::Point, vec![]);
        for i in 0..n {
            d.push(Some(Geometry::Point(Coord::new(i as f64 * 1e-6, 0.0))), vec![]);
        }
        Layer::new(1, d)
    }

    #[test]
    fn a_large_layer_builds_its_cull_index_with_its_projection() {
        let flat = projection::Flat::Mercator;
        // Above the renderer's `> CULL_INDEX_ABOVE` gate: the index is built eagerly with
        // the projection (the first render never pays the sort), and a clone shares it
        // instead of copying ~12 bytes per feature.
        let mut big = many_points(cull::CULL_INDEX_ABOVE + 1);
        assert!(big.cull_index_in(&flat, "mercator").is_some(), "large layer's index is ready with its projection");
        let copy = big.clone();
        match (&copy.cull_index, &big.cull_index) {
            (Some(a), Some(b)) => assert!(std::sync::Arc::ptr_eq(a, b), "a clone shares the index instead of copying it"),
            _ => panic!("both clones carry the index"),
        }
        // At and below the gate (the renderer asks only above it): no index, linear scan.
        let mut edge = many_points(cull::CULL_INDEX_ABOVE);
        edge.projected();
        assert!(edge.cull_index_in(&flat, "mercator").is_none(), "exactly at the gate stays linear-scan");
        let mut small = many_points(10);
        small.projected();
        assert!(small.cull_index_in(&flat, "mercator").is_none(), "small layers never index");
        // A reprojection drops the index with the projection cache.
        big.touch_geometry();
        big.projected();
        assert!(big.cull_index_in(&flat, "mercator").is_some(), "rebuilt with the next projection");
    }

    #[test]
    fn a_level_saving_little_draws_in_full() {
        // Zoomed in past the vertex spacing, thinning drops too little to be worth a copy.
        let flat = projection::Flat::Mercator;
        let mut l = long_line(lod::THIN_ABOVE + 1000);
        l.projected();
        assert_eq!(l.thinned_in(&flat, "mercator", 1.0).as_ptr(), l.projected_cache().as_ptr());
        // Nor for points, however many.
        let mut d = Dataset::new("pts", GeometryType::Point, vec![]);
        let pts = (0..lod::THIN_ABOVE + 10).map(|i| Coord::new(i as f64 * 1e-6, 0.0)).collect();
        d.push(Some(Geometry::MultiPoint(pts)), vec![]);
        let mut p = Layer::new(2, d);
        p.projected();
        assert_eq!(p.thinned_in(&flat, "mercator", 1e6).as_ptr(), p.projected_cache().as_ptr());
    }

    #[test]
    fn a_small_layer_always_draws_in_full() {
        let flat = projection::Flat::Mercator;
        let mut l = long_line(1000);
        let full = l.projected().to_vec();
        assert_eq!(l.thinned_in(&flat, "mercator", 1e6), &full[..]);
    }

    #[test]
    fn layer_order_and_hit() {
        let mut m = Map::new("Map");
        let poly = Dataset::new("poly", GeometryType::Polygon, vec![]);
        m.add_dataset(poly);
        let id = m.add_dataset(points());
        assert_eq!(m.layers[0].id, id, "points go on top of polygons");
        let p = lonlat_to_merc(Coord::new(10.0, 10.0));
        assert_eq!(m.hit(p, 1000.0, 1e6, |_| true), Some((id, 1)));
        let e = Extent::from_points(lonlat_to_merc(Coord::new(-1.0, -1.0)), lonlat_to_merc(Coord::new(1.0, 1.0)));
        assert_eq!(m.hits_in(&e, 1e6), vec![(id, vec![0])]);
        m.layers[0].definition_query = Some("n = 2".into());
        assert_eq!(m.hits_in(&e, 1e6), vec![]);
    }

    #[test]
    fn hits_at_finds_every_overlapping_feature() {
        let mut m = Map::new("Map");
        let mut d = points();
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(3)]);
        let below = m.add_dataset(d);
        let above = m.add_dataset(points());
        let p = lonlat_to_merc(Coord::new(0.0, 0.0));
        // Top layer first; in a layer, the feature drawn last (on top) first.
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, false, 100), vec![(above, 0), (below, 2), (below, 0)]);
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, false, 2), vec![(above, 0), (below, 2)]);
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |l| l.id == below, false, 100), vec![(below, 2), (below, 0)]);
        // Only the top layer with a hit.
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, true, 100), vec![(above, 0)]);
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |l| l.id == below, true, 100), vec![(below, 2), (below, 0)]);
        // A higher depth priority draws on top, whatever the order.
        if let Some(l) = m.layers.iter_mut().find(|l| l.id == below) {
            l.priority = 1;
        }
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, true, 100), vec![(below, 2), (below, 0)]);
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, false, 100), vec![(below, 2), (below, 0), (above, 0)]);
        if let Some(l) = m.layers.iter_mut().find(|l| l.id == below) {
            l.priority = 0;
        }
        // Features the definition query hides aren't hit.
        if let Some(l) = m.layers.iter_mut().find(|l| l.id == below) {
            l.definition_query = Some("n = 1".into());
        }
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |l| l.id == below, false, 100), vec![(below, 0)]);
    }

    #[test]
    fn hits_at_skips_features_display_filters_hide() {
        let mut m = Map::new("Map");
        let mut d = points();
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(3)]);
        let id = m.add_dataset(d);
        let p = lonlat_to_merc(Coord::new(0.0, 0.0));
        let filter = |min_scale| effects::DisplayFilter { name: "big".into(), min_scale, max_scale: None, where_: "n = 3".into() };
        m.layers[0].display_filters = vec![filter(None)];
        assert_eq!(m.layers[0].drawn_rows(1e6), vec![false, false, true]);
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, false, 100), vec![(id, 2)]);
        // A filter that doesn't apply at this scale hides nothing.
        m.layers[0].display_filters = vec![filter(Some(1e5))];
        assert_eq!(m.hits_at(p, 1000.0, 1e6, |_| true, false, 100), vec![(id, 2), (id, 0)]);
    }

    #[test]
    fn select_hits_skip_hidden_features_and_follow_draw_order() {
        let mut m = Map::new("Map");
        let mut d = points();
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(3)]);
        let below = m.add_dataset(d);
        let above = m.add_dataset(points());
        let p = lonlat_to_merc(Coord::new(0.0, 0.0));
        let e = Extent::from_points(lonlat_to_merc(Coord::new(-1.0, -1.0)), lonlat_to_merc(Coord::new(1.0, 1.0)));
        let all = |_: &Layer| true;
        assert_eq!(m.hit(p, 1000.0, 1e6, all), Some((above, 0)));
        // A higher depth priority draws on top, whatever the order (#243).
        m.layers.iter_mut().filter(|l| l.id == below).for_each(|l| l.priority = 1);
        assert_eq!(m.hit(p, 1000.0, 1e6, all), Some((below, 2)));
        m.layers.iter_mut().filter(|l| l.id == below).for_each(|l| l.priority = 0);
        // Features a display filter hides are neither clicked nor boxed (#243).
        let only = |w: &str| vec![effects::DisplayFilter { name: "f".into(), min_scale: None, max_scale: None, where_: w.into() }];
        m.layers.iter_mut().filter(|l| l.id == above).for_each(|l| l.display_filters = only("n = 2"));
        m.layers.iter_mut().filter(|l| l.id == below).for_each(|l| l.display_filters = only("n = 3"));
        assert_eq!(m.hit(p, 1000.0, 1e6, all), Some((below, 2)));
        assert_eq!(m.hits_in(&e, 1e6), vec![(below, vec![2])]);
        // A layer the click doesn't take doesn't hide the ones below it.
        m.layers.iter_mut().for_each(|l| l.display_filters.clear());
        assert_eq!(m.hit(p, 1000.0, 1e6, |l| l.id != above), Some((below, 2)));
    }

    #[test]
    fn shown_extent_follows_the_definition_query() {
        let mut m = Map::new("Map");
        m.add_dataset(points());
        let l = &mut m.layers[0];
        assert_eq!(l.shown_extent(), Extent::new(0.0, 0.0, 10.0, 10.0));
        l.definition_query = Some("n = 2".into());
        assert_eq!(l.shown_extent(), Extent::new(10.0, 10.0, 10.0, 10.0));
        l.definition_query = Some("n = 3".into());
        assert!(l.shown_extent().is_empty());
        // An invalid query hides nothing.
        l.definition_query = Some("n = = 2".into());
        assert_eq!(l.shown_extent(), Extent::new(0.0, 0.0, 10.0, 10.0));
        // The map's shown extent (Full Extent) follows it too; its full extent doesn't.
        m.layers[0].definition_query = Some("n = 2".into());
        assert_eq!(m.shown_extent(), Extent::new(10.0, 10.0, 10.0, 10.0));
        assert_eq!(m.full_extent(), Extent::new(0.0, 0.0, 10.0, 10.0));
    }

    #[test]
    fn the_shown_legend_is_what_the_layer_draws() {
        let mut m = Map::new("Map");
        m.add_dataset(points());
        let l = &mut m.layers[0];
        // Classes for n = 1, 2 and 9: 9 is no feature's value.
        let s = |c| Symbol::default_for(GeometryType::Point, c);
        l.renderer = Renderer::Unique {
            field: "n".into(),
            classes: vec![("1".into(), s(0)), ("2".into(), s(1)), ("9".into(), s(2))],
            other: s(3),
            ramp: Ramp::Ember,
        };
        let labels = |l: &Layer, scale| l.shown_legend(scale).map(|v| v.into_iter().map(|e| e.label).collect::<Vec<_>>());
        assert_eq!(labels(l, 1e6), Some(vec!["1".to_string(), "2".to_string()]), "no 9, no <all other values>");
        // The definition query and display filters narrow it.
        l.definition_query = Some("n = 2".into());
        assert_eq!(labels(l, 1e6), Some(vec!["2".to_string()]));
        l.definition_query = None;
        l.display_filters = vec![effects::DisplayFilter { name: "f".into(), min_scale: None, max_scale: None, where_: "n = 1".into() }];
        assert_eq!(labels(l, 1e6), Some(vec!["1".to_string()]));
        // Nothing drawn: no legend at all.
        l.display_filters.clear();
        l.definition_query = Some("n = 3".into());
        assert_eq!(labels(l, 1e6), None);
        l.definition_query = None;
        l.min_scale = Some(1e5);
        assert_eq!(labels(l, 1e6), None, "out of its scale range");
        assert!(labels(l, 5e4).is_some());
        // The full legend is still there for "all classes".
        assert_eq!(l.renderer.legend().len(), 4);
        // An empty layer, with no query or filter to evaluate, draws nothing either.
        l.min_scale = None;
        l.dataset.features.clear();
        assert_eq!(labels(l, 1e6), None);
    }
}
