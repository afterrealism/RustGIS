//! L3: the CPU map renderer.
//!
//! [`render`] draws a [`Map`] (basemap, graticule, layers, selection) into a premultiplied RGBA
//! `tiny_skia::Pixmap` and places labels without collisions. The desktop app shows the pixmap
//! as a texture and draws the placed labels with its own text engine; [`render_png`] rasterises
//! the labels too, for export and the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod effects;
pub mod fonts;
pub mod gpu_scene;
pub mod layout;
pub mod logo;
mod mesh;
pub mod pdf;
mod picture;
pub mod post;
mod raster;
pub mod report;
mod text;
mod tiles;

pub use rustgis_tiles;
pub use tiles::spec as tile_spec;

/// Natural Earth 1:110m sample data (public domain), fetched by `build.rs` from rustgis-assets.
pub mod sample_data {
    pub const COUNTRIES: &str = include_str!(concat!(env!("OUT_DIR"), "/countries.geojson"));
    /// The countries as China sees its borders (Natural Earth's China point of view).
    pub const COUNTRIES_CHN: &str = include_str!(concat!(env!("OUT_DIR"), "/countries_chn.geojson"));
    pub const CITIES: &str = include_str!(concat!(env!("OUT_DIR"), "/cities.geojson"));
    pub const RIVERS: &str = include_str!(concat!(env!("OUT_DIR"), "/rivers.geojson"));
    pub const LAKES: &str = include_str!(concat!(env!("OUT_DIR"), "/lakes.geojson"));

    use std::sync::atomic::{AtomicU8, Ordering};

    /// Whose view of disputed borders the built-in countries and world outline show.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Worldview {
        /// Natural Earth's de facto borders.
        #[default]
        Default,
        /// China's view (required for maps shown in China).
        China,
    }

    impl Worldview {
        pub fn code(self) -> &'static str {
            match self {
                Worldview::Default => "default",
                Worldview::China => "china",
            }
        }

        pub fn from_code(code: &str) -> Option<Worldview> {
            match code.trim().to_ascii_lowercase().as_str() {
                "default" => Some(Worldview::Default),
                "china" | "chn" | "cn" => Some(Worldview::China),
                _ => None,
            }
        }

        /// The worldview for a system in time zone `tz` (IANA, such as `Asia/Shanghai`) whose
        /// preferred locales are `locales` (such as `zh-CN`, `zh_CN.UTF-8`): China when the time
        /// zone is one of mainland China's or the first locale is mainland Chinese.
        pub fn detect<'a>(tz: Option<&str>, locales: impl IntoIterator<Item = &'a str>) -> Worldview {
            let china_tz = tz.is_some_and(|tz| {
                matches!(
                    tz.trim(),
                    "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Chungking" | "Asia/Harbin" | "Asia/Urumqi" | "Asia/Kashgar" | "PRC"
                )
            });
            let china_locale = locales.into_iter().next().is_some_and(|tag| {
                let base = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
                let mut parts = base.split('-');
                parts.next() == Some("zh") && parts.any(|p| p == "cn")
            });
            if china_tz || china_locale { Worldview::China } else { Worldview::Default }
        }
    }

    static WORLDVIEW: AtomicU8 = AtomicU8::new(0);

    /// Sets the worldview of the countries sample and the basemap's world outline.
    pub fn set_worldview(w: Worldview) {
        WORLDVIEW.store(w as u8, Ordering::Relaxed);
    }

    pub fn worldview() -> Worldview {
        if WORLDVIEW.load(Ordering::Relaxed) == Worldview::China as u8 { Worldview::China } else { Worldview::Default }
    }

    /// The countries GeoJSON in the current worldview.
    pub fn countries() -> &'static str {
        match worldview() {
            Worldview::Default => COUNTRIES,
            Worldview::China => COUNTRIES_CHN,
        }
    }
}

use std::sync::OnceLock;

use rustgis_data::{Dataset, Geometry, GeometryType};
use rustgis_geom::{Coord, Extent, extent_to_merc, lonlat_to_merc};
use rustgis_map::globe::{V3, densify};
use rustgis_map::{Camera, Globe, Map, Marker, Rgba, Symbol};
/// The image type pages and maps are drawn into.
pub use tiny_skia::Pixmap;
use tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, Paint, PathBuilder, PixmapPaint, Point, RadialGradient, SpreadMode, Stroke, Transform,
};

/// Selection highlight (cyan, as desktop GIS users expect).
pub const SELECTION: Rgba = Rgba::rgb(0x00, 0xE5, 0xFF);

/// Largest raster rendered (pixels per side).
pub const MAX_SIDE: u32 = 8192;

/// What to draw and at which size.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Size in logical points.
    pub width: f64,
    pub height: f64,
    /// Physical pixels per logical point.
    pub pixels_per_point: f64,
    pub camera: Camera,
}

impl View {
    pub fn pixel_size(&self) -> (u32, u32) {
        let w = (self.width * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        let h = (self.height * self.pixels_per_point).round().clamp(1.0, MAX_SIDE as f64) as u32;
        (w, h)
    }
    /// Logical screen position of a Web Mercator point.
    pub fn to_screen(&self, p: Coord) -> (f64, f64) {
        self.camera.to_screen(p, self.width, self.height)
    }
}

/// A placed label in logical points (centre of the text).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLabel {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Rgba,
    pub halo: Option<Rgba>,
    pub bold: bool,
    /// An icon placed with the label (a tile style's `icon-image`), drawn into the map.
    pub icon: Option<PlacedIcon>,
    /// Which part of the text is at `x`: 0 its centre, -0.5 its left edge, 0.5 its right edge.
    /// Each renderer lines the text up with its own measure of the width, so text set beside an
    /// icon keeps clear of it whatever the font.
    pub align: f32,
}

impl PlacedLabel {
    /// The centre of the text, by this crate's measure of its width.
    pub fn center_x(&self) -> f32 {
        self.x - self.align * text::measure(&self.text, self.size, self.bold)
    }
}

/// An icon in logical points: its picture and centre.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedIcon {
    pub image: std::sync::Arc<Pixmap>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct Rendered {
    pub pixmap: Pixmap,
    pub labels: Vec<PlacedLabel>,
    /// Labels that didn't fit, when the map shows unplaced labels (drawn in red).
    pub unplaced: Vec<PlacedLabel>,
    /// The credits the map's tile layers ask for.
    pub attribution: Vec<String>,
    /// Tiles are still on their way: drawing again later shows more.
    pub tiles_pending: bool,
    /// Streamed layers are still reading the view: drawing again later shows them.
    pub data_pending: bool,
    /// Tile layers that couldn't be drawn, and why.
    pub tile_errors: Vec<String>,
}

/// Which rasteriser [`render_backend`] targets: the tiny-skia CPU pixmap (the default, and the
/// only path for exports, the globe and effect-heavy frames) or a lyon-tessellated GPU scene
/// drawn by the UI inside egui's wgpu pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Cpu,
    Gpu,
}

/// A finished render in either backend.
pub enum RenderOutput {
    Cpu(Rendered),
    Gpu(gpu_scene::GpuScene),
}

/// Renders the map for the screen in the chosen backend. `Backend::Cpu` is exactly
/// [`render`]; `Backend::Gpu` draws what it can as tessellated meshes and everything else
/// (basemap, tiles, rasters, masked/blended/transparent layers) as CPU-rasterized images in
/// the same z-order.
pub fn render_backend(map: &mut Map, view: &View, backend: Backend) -> Option<RenderOutput> {
    render_with_backend(map, view, false, backend)
}

/// Decodes PNG bytes into a pixmap.
pub fn decode_png(png: &[u8]) -> Option<Pixmap> {
    Pixmap::decode_png(png).ok()
}

/// The pixel size of a base64 PNG or JPEG picture, `None` when it can't be decoded.
pub fn picture_size(b64: &str) -> Option<(u32, u32)> {
    picture::size(b64)
}

/// The colour unplaced labels are drawn in.
pub const UNPLACED: Rgba = Rgba::rgb(0xD0, 0x1C, 0x1C);

/// The distance (metres) a viewer of a flat map at a ground resolution would see it from: the
/// view's height seen through a 60° field of view.
pub fn viewing_distance(ground_res: f64, view_height: f64) -> f64 {
    ground_res * view_height * 0.866
}

/// The built-in world outline used by every basemap (Natural Earth 1:110m, public domain), in the
/// current worldview.
fn world() -> &'static Vec<Geometry> {
    static W: [OnceLock<Vec<Geometry>>; 2] = [OnceLock::new(), OnceLock::new()];
    let wv = sample_data::worldview();
    let cell = &W[wv as usize];
    cell.get_or_init(|| match rustgis_data::io::read_geojson(sample_data::countries(), "world") {
        Ok((ds, _)) => ds.features.into_iter().filter_map(|f| f.geometry).map(|g| g.map_coords(&lonlat_to_merc)).collect(),
        Err(e) => {
            log::warn!("built-in world outline unreadable: {e}");
            Vec::new()
        }
    })
}

/// The built-in world outline in longitude/latitude, for the globe.
fn world_lonlat() -> &'static Vec<Geometry> {
    static W: [OnceLock<Vec<Geometry>>; 2] = [OnceLock::new(), OnceLock::new()];
    let cell = &W[sample_data::worldview() as usize];
    cell.get_or_init(|| world().iter().map(|g| g.map_coords(&rustgis_geom::merc_to_lonlat)).collect())
}

thread_local! {
    /// The world outline in each custom projection used so far.
    static WORLD_IN: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<Vec<Geometry>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The built-in world outline in a custom projection's coordinates (kept per projection).
fn world_in(flat: &rustgis_map::projection::Flat, key: &str) -> std::rc::Rc<Vec<Geometry>> {
    WORLD_IN.with(|cell| {
        cell.borrow_mut()
            .entry(format!("{}:{key}", sample_data::worldview().code()))
            .or_insert_with(|| std::rc::Rc::new(world_lonlat().iter().map(|g| flat.project(g)).collect()))
            .clone()
    })
}

/// Largest angle between globe vertices before edges are bent along great circles (2°).
const GLOBE_STEP: f64 = 0.035;

/// The built-in world dataset (lon/lat), for the sample project.
pub fn world_dataset(name: &str, text: &str) -> Option<Dataset> {
    rustgis_data::io::read_geojson(text, name).ok().map(|(d, _)| d)
}

thread_local! {
    /// Antialiasing for the map being drawn (off in the fast display type).
    static ANTIALIAS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Sets antialiasing for one render and puts it back on when dropped, however the render ends.
struct Antialias;

impl Antialias {
    fn set(on: bool) -> Antialias {
        ANTIALIAS.with(|a| a.set(on));
        Antialias
    }
}

impl Drop for Antialias {
    fn drop(&mut self) {
        ANTIALIAS.with(|a| a.set(true));
    }
}

fn paint(c: Rgba, opacity: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (c.a() as f32 * opacity).round().clamp(0.0, 255.0) as u8;
    p.set_color_rgba8(c.r(), c.g(), c.b(), a);
    p.anti_alias = ANTIALIAS.with(|a| a.get());
    p
}

struct Ctx<'a> {
    view: &'a View,
    ppp: f32,
    /// Polygons drawn as outlines only.
    wireframe: bool,
    /// Visible Mercator extent with a margin, for culling.
    cull: Extent,
    /// Set when drawing the globe; coordinates are then longitude/latitude, not Web Mercator.
    globe: Option<Globe>,
}

impl Ctx<'_> {
    /// Logical screen position of a coordinate, `None` when it is behind the globe.
    fn screen(&self, c: Coord) -> Option<(f64, f64)> {
        match &self.globe {
            Some(g) => g.forward(c),
            None => Some(self.view.to_screen(c)),
        }
    }

    fn px(&self, c: Coord) -> (f32, f32) {
        match self.screen(c) {
            Some((x, y)) => ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32),
            None => (f32::NAN, f32::NAN),
        }
    }

    fn vpx(&self, g: &Globe, v: V3) -> (f32, f32) {
        let (x, y) = g.project(v);
        ((x * self.view.pixels_per_point) as f32, (y * self.view.pixels_per_point) as f32)
    }

    /// View vectors of a ring or line, with long edges bent along great circles.
    fn globe_vectors(g: &Globe, pts: &[Coord]) -> Vec<V3> {
        let mut out: Vec<V3> = Vec::with_capacity(pts.len());
        for c in pts {
            if !c.is_finite() {
                continue;
            }
            let v = g.to_view(*c);
            match out.last().copied() {
                Some(prev) => out.extend(densify(prev, v, GLOBE_STEP)),
                None => out.push(v),
            }
        }
        out
    }

    /// A polygon ring on the globe. Hidden vertices are moved radially onto the horizon circle at
    /// their own bearing, so the hidden part of the ring runs along the limb in its true order and
    /// encloses no area; a ring that is wholly hidden is skipped.
    fn globe_ring(&self, g: &Globe, pb: &mut PathBuilder, ring: &[Coord]) {
        let vs = Self::globe_vectors(g, ring);
        let h = g.horizon();
        if vs.len() < 3 || vs.iter().all(|v| v[2] <= h) {
            return;
        }
        let r = (1.0 - h * h).sqrt();
        let limb = |v: V3| {
            let a = v[1].atan2(v[0]);
            [r * a.cos(), r * a.sin(), h]
        };
        let mut out: Vec<V3> = Vec::with_capacity(vs.len() + 8);
        let push_limb = |out: &mut Vec<V3>, p: V3| {
            // Follow the limb between hidden points that are far apart in bearing.
            if let Some(last) = out.last().copied()
                && last[2] <= h + 1e-9
            {
                out.extend(g.horizon_arc(last, p));
            }
            out.push(p);
        };
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            if let Some(p) = prev
                && (p[2] > h) != vis
            {
                push_limb(&mut out, g.horizon_crossing(p, v));
            }
            if vis {
                out.push(v)
            } else {
                push_limb(&mut out, limb(v))
            }
            prev = Some(v);
        }
        // A ring that ends and starts behind the horizon closes along the limb, not across it.
        if let (Some(first), Some(last)) = (out.first().copied(), out.last().copied())
            && first[2] <= h + 1e-9
            && last[2] <= h + 1e-9
        {
            out.extend(g.horizon_arc(last, first));
        }
        for (i, v) in out.iter().enumerate() {
            let (x, y) = self.vpx(g, *v);
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        if !out.is_empty() {
            pb.close();
        }
    }

    /// A line on the globe, broken where it goes behind the horizon.
    fn globe_line(&self, g: &Globe, pb: &mut PathBuilder, part: &[Coord]) {
        let vs = Self::globe_vectors(g, part);
        let h = g.horizon();
        let mut prev: Option<V3> = None;
        for v in vs {
            let vis = v[2] > h;
            match prev {
                Some(p) if p[2] > h && vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                Some(p) if p[2] > h => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.line_to(x, y);
                }
                Some(p) if vis => {
                    let (x, y) = self.vpx(g, g.horizon_crossing(p, v));
                    pb.move_to(x, y);
                    let (x, y) = self.vpx(g, v);
                    pb.line_to(x, y);
                }
                None if vis => {
                    let (x, y) = self.vpx(g, v);
                    pb.move_to(x, y);
                }
                _ => {}
            }
            prev = Some(v);
        }
    }

    fn ring_path(&self, pb: &mut PathBuilder, ring: &[Coord], close: bool) {
        if let Some(g) = &self.globe {
            if close {
                self.globe_ring(g, pb, ring)
            } else {
                self.globe_line(g, pb, ring)
            }
            return;
        }
        // Drop vertices closer than ~0.35 px to the previous one: big speed-up when zoomed out.
        let mut last: Option<(f32, f32)> = None;
        let n = ring.len();
        for (i, c) in ring.iter().enumerate() {
            let (x, y) = self.px(*c);
            if !x.is_finite() || !y.is_finite() {
                // A point the projection can't show breaks the line rather than being bridged.
                if close && last.is_some() {
                    pb.close();
                }
                last = None;
                continue;
            }
            match last {
                None => pb.move_to(x, y),
                Some((lx, ly)) => {
                    if (x - lx).abs() + (y - ly).abs() < 0.35 && i + 1 != n {
                        continue;
                    }
                    pb.line_to(x, y)
                }
            }
            last = Some((x, y));
        }
        if close && last.is_some() {
            pb.close();
        }
    }

    fn polygon(&self, pm: &mut Pixmap, polys: &[Vec<Vec<Coord>>], fill: Option<Paint>, stroke: Option<(Paint, f32)>) {
        let mut pb = PathBuilder::new();
        for rings in polys {
            for r in rings {
                self.ring_path(&mut pb, r, true);
            }
        }
        let Some(path) = pb.finish() else { return };
        if let Some(f) = fill {
            pm.fill_path(&path, &f, FillRule::EvenOdd, Transform::identity(), None);
        }
        if let Some((s, w)) = stroke
            && w > 0.0
        {
            let st = Stroke { width: w * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &s, &st, Transform::identity(), None);
        }
    }

    fn line(&self, pm: &mut Pixmap, parts: &[Vec<Coord>], p: &Paint, w: f32) {
        let mut pb = PathBuilder::new();
        for part in parts {
            self.ring_path(&mut pb, part, false);
        }
        let Some(path) = pb.finish() else { return };
        let st = Stroke { width: (w * self.ppp).max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, p, &st, Transform::identity(), None);
    }

    fn marker(&self, pm: &mut Pixmap, c: Coord, s: &Symbol, opacity: f32, halo: Option<Rgba>) {
        let (x, y) = self.px(c);
        if !x.is_finite() || !y.is_finite() {
            return; // behind the globe
        }
        let r = (s.size.max(1.0) * self.ppp) / 2.0;
        let mut pb = PathBuilder::new();
        match s.marker {
            Marker::Circle => pb.push_circle(x, y, r),
            Marker::Square => {
                if let Some(rect) = tiny_skia::Rect::from_xywh(x - r * 0.9, y - r * 0.9, r * 1.8, r * 1.8) {
                    pb.push_rect(rect)
                }
            }
            Marker::Triangle => {
                pb.move_to(x, y - r * 1.1);
                pb.line_to(x + r, y + r * 0.75);
                pb.line_to(x - r, y + r * 0.75);
                pb.close();
            }
            Marker::Diamond => {
                pb.move_to(x, y - r * 1.15);
                pb.line_to(x + r * 0.9, y);
                pb.line_to(x, y + r * 1.15);
                pb.line_to(x - r * 0.9, y);
                pb.close();
            }
            Marker::Star => {
                for i in 0..10 {
                    let a = std::f32::consts::PI * (i as f32) / 5.0 - std::f32::consts::FRAC_PI_2;
                    let rr = if i % 2 == 0 { r * 1.2 } else { r * 0.5 };
                    let (px, py) = (x + rr * a.cos(), y + rr * a.sin());
                    if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
                }
                pb.close();
            }
        }
        let Some(path) = pb.finish() else { return };
        if let Some(h) = halo {
            let st = Stroke { width: 3.5 * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &paint(h, 1.0), &st, Transform::identity(), None);
        }
        pm.fill_path(&path, &paint(s.fill, opacity), FillRule::Winding, Transform::identity(), None);
        if s.stroke_width > 0.0 && s.stroke.a() > 0 {
            let st = Stroke { width: s.stroke_width * self.ppp, line_join: LineJoin::Round, ..Stroke::default() };
            pm.stroke_path(&path, &paint(s.stroke, opacity), &st, Transform::identity(), None);
        }
    }

    fn feature(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol) {
        match g {
            Geometry::Point(c) => self.marker(pm, *c, s, 1.0, None),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.marker(pm, *c, s, 1.0, None)),
            Geometry::Line(parts) => self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width),
            Geometry::Polygon(polys) if self.wireframe => {
                self.polygon(pm, polys, None, Some((paint(s.stroke.with_alpha(255), 1.0), s.stroke_width.max(1.0))))
            }
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(s.fill, 1.0)), Some((paint(s.stroke, 1.0), s.stroke_width))),
        }
    }

    fn highlight(&self, pm: &mut Pixmap, g: &Geometry, s: &Symbol) {
        let cyan = paint(SELECTION, 1.0);
        match g {
            Geometry::Point(c) => self.marker(pm, *c, s, 1.0, Some(SELECTION)),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| self.marker(pm, *c, s, 1.0, Some(SELECTION))),
            Geometry::Line(parts) => {
                self.line(pm, parts, &cyan, s.stroke_width + 3.0);
                self.line(pm, parts, &paint(s.stroke, 1.0), s.stroke_width);
            }
            Geometry::Polygon(polys) => self.polygon(pm, polys, Some(paint(SELECTION.with_alpha(40), 1.0)), Some((cyan, 2.0))),
        }
    }
}

/// The longitude/latitude extent a view shows (the whole world on the globe, or when the
/// projection can't say).
pub fn view_lonlat(map: &Map, view: &View) -> Extent {
    if map.projection.is_globe() {
        return Extent::WORLD_LONLAT;
    }
    let flat = map.flat();
    let e = view.camera.extent(view.width, view.height);
    let mut out = Extent::EMPTY;
    const N: usize = 8;
    for i in 0..=N {
        let t = i as f64 / N as f64;
        let (x, y) = (e.min_x + t * e.width(), e.min_y + t * e.height());
        for c in [Coord::new(x, e.min_y), Coord::new(x, e.max_y), Coord::new(e.min_x, y), Coord::new(e.max_x, y)] {
            match flat.inverse(c) {
                Some(p) if p.is_finite() => out.expand(p),
                _ => return Extent::WORLD_LONLAT,
            }
        }
    }
    // A pole inside the view lies beyond every edge sample (a polar projection): reach it.
    for pole in [Coord::new(0.0, 90.0), Coord::new(0.0, -90.0)] {
        if let Some(p) = flat.forward(pole)
            && p.is_finite()
            && e.contains(p)
        {
            out = Extent::new(-180.0, out.min_y.min(pole.y), 180.0, out.max_y.max(pole.y));
        }
    }
    if out.is_empty() { Extent::WORLD_LONLAT } else { out }
}

/// Draws the map for the screen. Layers are drawn bottom-up (the last layer in the list first).
/// On high-density screens rasters are computed at logical-pixel resolution, for speed.
pub fn render(map: &mut Map, view: &View) -> Option<Rendered> {
    render_with(map, view, false)
}

fn render_with(map: &mut Map, view: &View, full_res_rasters: bool) -> Option<Rendered> {
    match render_with_backend(map, view, full_res_rasters, Backend::Cpu) {
        Some(RenderOutput::Cpu(r)) => Some(r),
        // The Cpu backend never produces RenderOutput::Gpu.
        Some(RenderOutput::Gpu(_)) | None => None,
    }
}

/// Flushes the GPU pass's CPU accumulator pixmap as an Image draw command and resets it to
/// transparent. Basemap, tiles, rasters and tiny-skia-only layers draw into the accumulator
/// between GPU mesh commands, preserving the exact painter's-algorithm order.
fn flush_acc(pm: &mut Pixmap, cmds: &mut Vec<gpu_scene::DrawCmd>, dirty: &mut bool) -> Option<()> {
    if *dirty {
        let (w, h) = (pm.width(), pm.height());
        cmds.push(gpu_scene::DrawCmd::Image { rgba: pm.data().to_vec(), size: [w, h] });
        *pm = Pixmap::new(w, h)?;
        *dirty = false;
    }
    Some(())
}

/// The shared render body behind [`render`], [`render_backend`] and the export paths.
/// `full_res_rasters` renders rasters at full resolution (exports); `backend` selects the CPU
/// pixmap or a GPU scene (see [`Backend`]).
fn render_with_backend(map: &mut Map, view: &View, full_res_rasters: bool, backend: Backend) -> Option<RenderOutput> {
    let (w, h) = view.pixel_size();
    let mut pm = Pixmap::new(w, h)?;
    // Streamed layers read the view: in the background for the screen, in place for an export
    // (the full-resolution render), which waits for them.
    let data_pending = map.sync_sources(&view_lonlat(map, view), full_res_rasters);
    let globe = map.projection.is_globe().then(|| Globe::new(&view.camera, view.width, view.height));
    let (flat, key) = (map.flat(), map.projection.code());
    let custom = matches!(flat, rustgis_map::projection::Flat::Custom(_)) && globe.is_none();
    let _antialias = Antialias::set(!map.view.fast);
    let ctx = Ctx {
        view,
        ppp: view.pixels_per_point as f32,
        wireframe: map.view.wireframe,
        cull: view.camera.extent(view.width, view.height).padded(0.05, 0.0),
        globe,
    };
    let scale = map.ground_res_at(&view.camera) / rustgis_geom::PIXEL_M;
    // The Gpu backend draws flat, effect-free frames; anything else stays on the CPU pixmap.
    let gpu = matches!(backend, Backend::Gpu)
        && globe.is_none()
        && map.view.effects.is_none()
        && map.view.color_vision == rustgis_map::view::ColorVision::Normal;
    // The effective backend: the Gpu arm at the end only runs when the frame is compatible.
    let backend = if gpu { Backend::Gpu } else { Backend::Cpu };
    // GPU draw commands; between them, tiny-skia-only content accumulates in `pm` and is
    // flushed as ordered Image commands (`flush_acc`, gated by `cpu_dirty`).
    let mut cmds: Vec<gpu_scene::DrawCmd> = Vec::new();
    // Map units per physical pixel: the decimation threshold scale (`Ctx::ring_path`'s 0.35 px
    // rule is applied in screen space) and the stroke shader's width scale.
    let map_per_px = view.camera.res / view.pixels_per_point;
    // On the globe nothing is culled by the flat extent: the horizon clips instead.
    let visible = |e: &Extent| match &globe {
        Some(g) => g.may_see(e),
        None => e.intersects(&ctx.cull),
    };

    // Basemap.
    let (water, land, outline, grat) = map.basemap.palette();
    match &globe {
        Some(g) => globe_backdrop(&mut pm, g, water, map.view.effects.stars, view.pixels_per_point as f32),
        None if !gpu => pm.fill(tiny_skia::Color::from_rgba8(water.r(), water.g(), water.b(), 255)),
        // The Gpu scene carries the water backdrop as its clear colour instead — an
        // always-full-screen Image here would poison `has_images` and kill the camera
        // fast path.
        None => {}
    }
    // Set when tiny-skia actually rasterizes into the accumulator.
    let mut cpu_dirty = false;
    if land != water {
        let fill = paint(land, 1.0);
        let stroke = (outline.a() > 0).then(|| (paint(outline, 1.0), 0.6));
        let custom_world = custom.then(|| world_in(&flat, &key));
        let outlines: &Vec<Geometry> = match (&globe, &custom_world) {
            (Some(_), _) => world_lonlat(),
            (None, Some(w)) => w,
            (None, None) => world(),
        };
        for g in outlines {
            if let Geometry::Polygon(polys) = g
                && visible(&g.extent())
            {
                let mut tessellated = false;
                if gpu {
                    flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
                    let e = g.extent();
                    tessellated =
                        mesh::collect_basemap(&mut cmds, polys, [e.center().x, e.center().y], land, outline, ctx.ppp, map_per_px).is_ok();
                }
                if gpu && !tessellated {
                    // Lyon refused: tiny-skia still draws the outline.
                    ctx.polygon(&mut pm, polys, Some(fill.clone()), stroke.clone());
                    cpu_dirty = true;
                } else if !gpu {
                    ctx.polygon(&mut pm, polys, Some(fill.clone()), stroke.clone());
                }
            }
        }
    }
    if map.graticule && grat.a() > 0 {
        let p = paint(grat, 1.0);
        let step = if view.camera.res > 8000.0 {
            30.0
        } else if view.camera.res > 2000.0 {
            15.0
        } else if view.camera.res > 400.0 {
            5.0
        } else {
            1.0
        };
        let (lat_max, lat_min) = if globe.is_some() { (90.0, -90.0) } else { (85.0, -85.0) };
        let mut lines = Vec::new();
        let mut lon = -180.0;
        while lon <= 180.0 {
            lines.push((0..=36).map(|i| Coord::new(lon, lat_min + (lat_max - lat_min) * i as f64 / 36.0)).collect::<Vec<_>>());
            lon += step;
        }
        let mut lat = -90.0 + step;
        while lat < 90.0 {
            lines.push((0..=72).map(|i| Coord::new(-180.0 + i as f64 * 5.0, lat)).collect::<Vec<_>>());
            lat += step;
        }
        let lines = if globe.is_some() {
            lines
        } else if custom {
            // Parallels are cut at the map's edge like any line (#209).
            match flat.project(&Geometry::Line(lines)) {
                Geometry::Line(parts) => parts,
                _ => Vec::new(),
            }
        } else {
            lines.into_iter().map(|l| l.into_iter().map(lonlat_to_merc).collect()).collect()
        };
        if gpu {
            let mut e = Extent::EMPTY;
            for l in &lines {
                for c in l {
                    e.expand(*c);
                }
            }
            flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
            let tessellated = !e.is_empty()
                && mesh::collect_graticule(&mut cmds, &lines, [e.center().x, e.center().y], grat, ctx.ppp, map_per_px).is_ok();
            if !tessellated {
                ctx.line(&mut pm, &lines, &p, 0.6);
                cpu_dirty = true;
            }
        } else {
            ctx.line(&mut pm, &lines, &p, 0.6);
        }
    }

    // Web tile layers, over the built-in basemap and its graticule.
    let tile_place = match &globe {
        Some(g) => raster::Place::Globe(g),
        None => raster::Place::Flat(&flat, &key),
    };
    let tile_out = tiles::draw(&mut pm, map, view, tile_place, !map.label_display.paused && map.label_lock.is_none());
    cpu_dirty |= !map.tiles.is_empty();

    // Layers, bottom first.
    let mut label_candidates: Vec<(PlacedLabel, f32, (u32, usize))> = Vec::new();
    let display = map.label_display;
    if !display.locked {
        map.label_lock = None;
    }
    let lock: Option<std::collections::HashSet<(u32, usize)>> = map.label_lock.as_ref().map(|v| v.iter().copied().collect());
    let distance = viewing_distance(map.ground_res_at(&view.camera), view.height);
    // Graphics text: always drawn, and other labels keep clear of it.
    let mut fixed_labels: Vec<PlacedLabel> = Vec::new();
    let tokens = rustgis_map::graphics::TokenValues {
        map: map.name.clone(),
        scale,
        crs: map.projection.label(),
        layers: map.layers.iter().filter(|l| !l.graphics).count(),
        center: map.center_lonlat(),
        now: web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    let n_layers = map.layers.len();
    // Masks: layers shown only inside another layer's polygons, or left of the swipe divider.
    let masks = layer_masks(map, &ctx, w, h, &flat, &key);
    // Bottom first; a higher depth priority draws later (on top) whatever the order.
    let mut order: Vec<usize> = (0..n_layers).rev().collect();
    order.sort_by_key(|i| map.layers.get(*i).map_or(0, |l| l.priority));
    let clip = map.view.clip_distance.map(|d| (view.camera.center_lonlat(), d));
    for li in order {
        let Some(layer) = map.layers.get_mut(li) else { continue };
        if !layer.visible || !layer.in_scale_range(scale) {
            continue;
        }
        let opacity = layer.opacity();
        let mask = masks.iter().find(|(id, _)| *id == layer.id).map(|(_, m)| m);
        let blend = blend_mode(layer.blend);
        // Drawn on its own and composited when it is see-through, masked or blended.
        let offscreen = opacity < 0.999 || mask.is_some() || layer.blend != rustgis_map::effects::Blend::Normal;
        let composite = |pm: &mut Pixmap, target: &Pixmap| {
            pm.draw_pixmap(
                0,
                0,
                target.as_ref(),
                &PixmapPaint { opacity, blend_mode: blend, ..PixmapPaint::default() },
                Transform::identity(),
                mask,
            );
        };
        if let Some(rl) = &layer.raster {
            if let Some(data) = &rl.data {
                let place = match &globe {
                    Some(g) => raster::Place::Globe(g),
                    None => raster::Place::Flat(&flat, &key),
                };
                if offscreen {
                    let mut target = Pixmap::new(w, h)?;
                    raster::draw(&mut target, data, &rl.style, view, 1.0, full_res_rasters, place);
                    composite(&mut pm, &target);
                } else {
                    raster::draw(&mut pm, data, &rl.style, view, opacity, full_res_rasters, place);
                }
                cpu_dirty = true;
            }
            continue;
        }
        // The definition query and the display filters that apply at this scale (a feature draws
        // only if it matches all of them).
        let mut rows = layer.drawn_rows(scale);
        // View clipping: features entirely beyond the distance from the view's centre are left
        // out; one reaching into it (its box's nearest point within the distance) still draws.
        if let Some((centre, d)) = clip {
            for (r, keep) in rows.iter_mut().enumerate() {
                if *keep {
                    let e = layer.dataset.features.get(r).and_then(|f| f.geometry.as_ref()).map(|g| g.extent()).filter(|e| !e.is_empty());
                    *keep = e.is_some_and(|e| {
                        let near = Coord::new(centre.x.clamp(e.min_x, e.max_x), centre.y.clamp(e.min_y, e.max_y));
                        rustgis_geom::haversine_m(centre, near) <= d
                    });
                }
            }
        }
        if globe.is_none() {
            layer.projected_in(&flat, &key); // refresh the projected cache
        }
        // Everything below only reads the layer: borrow rather than copy its data each frame.
        let layer: &rustgis_map::Layer = layer;
        let (labels, selection, ds_ptr) = (&layer.labels, &layer.selection, &layer.dataset);
        // The globe draws the lon/lat geometry itself; the flat map its Web Mercator copy.
        let geoms: Vec<Option<&Geometry>> = if globe.is_some() {
            ds_ptr.features.iter().map(|f| f.geometry.as_ref()).collect()
        } else {
            layer.projected_cache().iter().map(Option::as_ref).collect()
        };
        let mut target = if offscreen { Pixmap::new(w, h)? } else { Pixmap::new(1, 1)? };
        let direct = !offscreen;
        // GPU-eligible: a plain vector layer drawn at full opacity with no mask, blend,
        // aggregation, extrusion or per-row graphics (an inert aggregation/extrusion on the
        // "wrong" geometry type still falls back — conservative and correct).
        let gpu_layer = gpu && !offscreen && layer.aggregation.is_none() && layer.extrusion.is_none() && !layer.graphics;
        // Lyon refuses degenerate geometry tiny-skia draws fine: collect_layer reports it,
        // and the layer runs the CPU branch below like any ineligible one.
        let mut gpu_done = false;
        if gpu_layer {
            flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
            gpu_done = mesh::collect_layer(&mut cmds, layer, &geoms, &rows, &visible, ctx.wireframe, ctx.ppp, map_per_px).is_ok();
        }
        if !gpu_done {
            let dest = if direct { &mut pm } else { &mut target };
            let aggregate = layer.aggregation.filter(|_| layer.geometry_type() == GeometryType::Point);
            let extrude = layer.extrusion.as_ref().filter(|_| layer.geometry_type() == GeometryType::Polygon && globe.is_none());
            if let Some(a) = aggregate {
                effects::aggregate(dest, &ctx, &geoms, &rows, a, &layer.symbol(0));
            } else if let Some(x) = extrude {
                let px_per_m = view.pixels_per_point / map_ground_res(view, &flat);
                effects::extrude(dest, &ctx, ds_ptr, &geoms, &rows, x, px_per_m, |r| layer.symbol(r));
            } else {
                for (row, g) in geoms.iter().enumerate() {
                    let Some(g) = g else { continue };
                    if rows.get(row) != Some(&true) || !visible(&g.extent()) {
                        continue;
                    }
                    // A picture draws in its row's turn, so graphics keep their order. (On the globe
                    // the geometry and `px` are both longitude/latitude, so the corners land right.)
                    if layer.graphics && rustgis_map::graphics::kind(ds_ptr, row) == "picture" {
                        let e = g.extent();
                        let (x0, y0) = ctx.px(Coord::new(e.min_x, e.max_y));
                        let (x1, y1) = ctx.px(Coord::new(e.max_x, e.min_y));
                        let (pw, ph) = (dest.width() as f32, dest.height() as f32);
                        if x0.max(x1) >= 0.0 && x0.min(x1) <= pw && y0.max(y1) >= 0.0 && y0.min(y1) <= ph {
                            picture::draw(dest, ds_ptr, row, (x0, y0, x1, y1), if direct { opacity } else { 1.0 });
                        }
                        continue;
                    }
                    ctx.feature(dest, g, &layer.symbol(row));
                }
            }
            cpu_dirty = true;
        }
        if !direct {
            composite(&mut pm, &target);
        }
        for row in selection {
            if let Some(Some(g)) = geoms.get(*row)
                && rows.get(*row) == Some(&true)
            {
                // Follow the layer's own backend: when it fell back to the CPU pixmap, the
                // highlight must too — tessellated here, the layer's later-flushed Image would
                // draw over it (the CPU path draws highlights over the layer).
                if gpu_done && mesh::collect_highlight(&mut cmds, g, &layer.symbol(*row), ctx.ppp, map_per_px).is_ok() {
                    // highlight tessellated into the scene, right after its layer
                } else {
                    ctx.highlight(&mut pm, g, &layer.symbol(*row));
                    cpu_dirty = true;
                }
            }
        }
        if layer.graphics {
            for (row, g) in geoms.iter().enumerate() {
                let Some(g) = g else { continue };
                if rows.get(row) != Some(&true) {
                    continue;
                }
                if let (Some(t), Some(c)) = (rustgis_map::graphics::label(ds_ptr, row), g.label_point())
                    && opacity > 0.0
                    && let Some((x, y)) = ctx.screen(c)
                {
                    let base = rustgis_map::graphics::text_color(ds_ptr, row);
                    let color = base.with_alpha((base.a() as f32 * opacity).round() as u8);
                    fixed_labels.push(PlacedLabel {
                        text: rustgis_map::graphics::expand(t, &tokens),
                        x: x as f32,
                        y: y as f32,
                        size: rustgis_map::graphics::font_size(ds_ptr, row),
                        color,
                        halo: Some(halo_for(color)),
                        bold: false,
                        icon: None,
                        align: 0.0,
                    });
                }
            }
        }
        if labels.enabled
            && !display.paused
            && labels.in_range(scale, distance)
            && let Some(fi) = ds_ptr.field_index(&labels.field)
        {
            let lid = layer.id;
            for (row, g) in geoms.iter().enumerate() {
                let Some(g) = g else { continue };
                if rows.get(row) != Some(&true) || lock.as_ref().is_some_and(|k| !k.contains(&(lid, row))) {
                    continue;
                }
                let v = ds_ptr.value(row, fi);
                if v.is_null() {
                    continue;
                }
                let Some(c) = g.label_point() else { continue };
                if globe.is_none() && !ctx.cull.contains(c) {
                    continue;
                }
                let Some((x, y)) = ctx.screen(c) else { continue };
                let text = v.to_string();
                let is_point = g.kind() == GeometryType::Point;
                let marker = if is_point { layer.symbol(row).size } else { 0.0 };
                let (dx, dy) =
                    labels.placement.offset(text::measure(&text, labels.size, labels.bold), labels.size * 1.25, marker, is_point);
                // Bigger polygons get priority.
                let pri = match g {
                    Geometry::Polygon(_) => {
                        let e = g.extent();
                        (e.width() * e.height()).sqrt() as f32
                    }
                    _ => 0.0,
                } + (n_layers - li) as f32 * 1e9;
                label_candidates.push((
                    PlacedLabel {
                        text,
                        x: x as f32 + dx,
                        y: y as f32 + dy,
                        size: labels.size,
                        color: labels.color,
                        halo: labels.halo.then_some(halo_for(labels.color)),
                        bold: labels.bold,
                        icon: None,
                        align: 0.0,
                    },
                    pri,
                    (lid, row),
                ));
            }
        }
    }
    if let Some(g) = &globe {
        globe_shading(&mut pm, g, water, map.view.effects.stars, view.pixels_per_point as f32);
    }
    // Tile labels, after every layer's own.
    if lock.is_none() {
        for (i, (l, pri)) in tile_out.labels.into_iter().enumerate() {
            label_candidates.push((l, pri - 1e10, (u32::MAX, i)));
        }
    }
    let placed = place_labels(label_candidates, fixed_labels, view.width as f32, view.height as f32, lock.is_some());
    if display.locked && map.label_lock.is_none() {
        map.label_lock = Some(placed.keys);
    }
    let unplaced = if display.show_unplaced { placed.unplaced } else { Vec::new() };
    // The placed labels' icons go into the map itself; their text is drawn over it later.
    for l in &placed.labels {
        if let Some(icon) = &l.icon {
            draw_icon(&mut pm, icon, view.pixels_per_point as f32);
            cpu_dirty = true;
        }
    }
    // Effects, and (on screen only) the colour vision simulation.
    let vision = if full_res_rasters { rustgis_map::view::ColorVision::Normal } else { map.view.color_vision };
    if !map.view.effects.is_none() || vision != rustgis_map::view::ColorVision::Normal {
        let ppp = view.pixels_per_point;
        let lonlat = |x: f64, y: f64| -> Option<Coord> {
            let (sx, sy) = (x / ppp, y / ppp);
            match &globe {
                Some(g) => g.inverse(sx, sy),
                None => flat.inverse(view.camera.to_map(sx, sy, view.width, view.height)),
            }
        };
        post::apply(&mut pm, &map.view.effects, vision, &lonlat);
    }
    let labels = placed.labels;
    let attribution = tile_out.attribution;
    let tiles_pending = tile_out.pending;
    let tile_errors = tile_out.errors;
    match backend {
        Backend::Cpu => {
            Some(RenderOutput::Cpu(Rendered { pixmap: pm, labels, unplaced, attribution, tiles_pending, data_pending, tile_errors }))
        }
        Backend::Gpu => {
            flush_acc(&mut pm, &mut cmds, &mut cpu_dirty)?;
            Some(RenderOutput::Gpu(gpu_scene::GpuScene {
                clear: mesh::premul(water),
                cmds,
                labels,
                unplaced,
                attribution,
                tiles_pending,
                data_pending,
                tile_errors,
            }))
        }
    }
}

/// Draws an icon, scaled to its size in logical points.
fn draw_icon(pm: &mut Pixmap, icon: &PlacedIcon, ppp: f32) {
    let (iw, ih) = (icon.image.width().max(1) as f32, icon.image.height().max(1) as f32);
    let (sx, sy) = (icon.w * ppp / iw, icon.h * ppp / ih);
    let (tx, ty) = ((icon.x - icon.w / 2.0) * ppp, (icon.y - icon.h / 2.0) * ppp);
    if !(sx.is_finite() && sy.is_finite() && tx.is_finite() && ty.is_finite()) {
        return;
    }
    let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    pm.draw_pixmap(0, 0, Pixmap::as_ref(&icon.image), &paint, Transform::from_row(sx, 0.0, 0.0, sy, tx, ty), None);
}

/// Metres on the ground per logical point at the view's centre.
fn map_ground_res(view: &View, flat: &rustgis_map::projection::Flat) -> f64 {
    (view.camera.res * flat.ground_per_unit(view.camera.center)).max(1e-9)
}

fn blend_mode(b: rustgis_map::effects::Blend) -> tiny_skia::BlendMode {
    use rustgis_map::effects::Blend;
    match b {
        Blend::Normal => tiny_skia::BlendMode::SourceOver,
        Blend::Multiply => tiny_skia::BlendMode::Multiply,
        Blend::Screen => tiny_skia::BlendMode::Screen,
        Blend::Overlay => tiny_skia::BlendMode::Overlay,
        Blend::Darken => tiny_skia::BlendMode::Darken,
        Blend::Lighten => tiny_skia::BlendMode::Lighten,
        Blend::Difference => tiny_skia::BlendMode::Difference,
    }
}

/// The mask of every masked or swiped layer: inside its mask layer's polygons and left of the
/// swipe divider.
fn layer_masks(map: &mut Map, ctx: &Ctx, w: u32, h: u32, flat: &rustgis_map::projection::Flat, key: &str) -> Vec<(u32, tiny_skia::Mask)> {
    let wanted: Vec<(u32, Option<String>, Option<f64>)> = map
        .layers
        .iter()
        .filter_map(|l| {
            let swipe = map.swipe.filter(|(id, _)| *id == l.id).map(|(_, x)| x.clamp(0.0, 1.0));
            (l.mask.is_some() || swipe.is_some()).then(|| (l.id, l.mask.clone(), swipe))
        })
        .collect();
    let mut out = Vec::new();
    for (id, by, swipe) in wanted {
        let Some(mut m) = tiny_skia::Mask::new(w, h) else { continue };
        match by
            .as_deref()
            .and_then(|n| map.layers.iter().position(|l| l.name.eq_ignore_ascii_case(n) && l.geometry_type() == GeometryType::Polygon))
        {
            Some(mi) => {
                let mut pb = PathBuilder::new();
                // The mask's shapes go straight into the path, without copying them.
                let mut add = |g: &Geometry| {
                    if let Geometry::Polygon(polys) = g {
                        for r in polys.iter().flatten() {
                            ctx.ring_path(&mut pb, r, true);
                        }
                    }
                };
                if let Some(ml) = map.layers.get_mut(mi) {
                    if ctx.globe.is_some() {
                        ml.dataset.features.iter().filter_map(|f| f.geometry.as_ref()).for_each(&mut add);
                    } else {
                        ml.projected_in(flat, key).iter().flatten().for_each(&mut add);
                    }
                }
                if let Some(p) = pb.finish() {
                    m.fill_path(&p, FillRule::EvenOdd, true, Transform::identity());
                }
            }
            // No (or no usable) mask layer: everything shows.
            None => {
                if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
                    m.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
                }
            }
        }
        if let Some(x) = swipe
            && let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, (w as f64 * x) as f32, h as f32)
        {
            m.intersect_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
        }
        out.push((id, m));
    }
    out
}

/// The colour of the space around the globe.
pub fn globe_space() -> Rgba {
    Rgba::rgb(0x08, 0x0B, 0x10)
}

/// Side of the starfield tile in logical points: the stars repeat this far apart.
pub const STAR_TILE: f32 = 512.0;

/// The stars of the space around the globe: a square `STAR_TILE` points a side at `ppp` pixels
/// per point, transparent between the stars, to be laid edge to edge from the view's top left
/// corner. The sky is the same every time, and the stars stay put while the globe turns.
pub fn starfield(ppp: f32) -> Option<Pixmap> {
    let ppp = if ppp.is_finite() { ppp.clamp(0.25, 4.0) } else { 1.0 };
    let side = (STAR_TILE * ppp).round().max(1.0);
    let mut pm = Pixmap::new(side as u32, side as u32)?;
    let ppp = side / STAR_TILE;
    // A small fixed-seed generator (SplitMix64), so every view gets the same sky.
    let mut seed: u64 = 0x5EED_57A2_F1E1_D000;
    let mut next = || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    };
    // Bluish, white and warm stars; most of them faint.
    let tints = [(0xCF, 0xDD, 0xFF), (0xFF, 0xFF, 0xFF), (0xFF, 0xEA, 0xCC)];
    for _ in 0..180 {
        let (x, y) = (2.0 + next() * (STAR_TILE - 4.0), 2.0 + next() * (STAR_TILE - 4.0));
        let bright = next().powi(3);
        let tint = tints.get((next() * 3.0) as usize).copied().unwrap_or((0xFF, 0xFF, 0xFF));
        let (cx, cy) = (x * ppp, y * ppp);
        let r = (0.3 + 0.8 * bright) * ppp;
        if bright > 0.6
            && let Some(path) = PathBuilder::from_circle(cx, cy, r * 2.5)
        {
            // A faint halo around the brightest.
            let halo = paint(Rgba::rgb(tint.0, tint.1, tint.2), 0.12);
            pm.fill_path(&path, &halo, FillRule::Winding, Transform::identity(), None);
        }
        if let Some(path) = PathBuilder::from_circle(cx, cy, r.max(0.35)) {
            let p = paint(Rgba::rgb(tint.0, tint.1, tint.2), 0.3 + 0.7 * bright);
            pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
        }
    }
    Some(pm)
}

thread_local! {
    /// The starfield tile last drawn, and its pixels per point.
    static STARS: std::cell::RefCell<Option<(u32, Pixmap)>> = const { std::cell::RefCell::new(None) };
}

/// Fills `pm` with the starfield, laid edge to edge from its top left corner.
fn draw_stars(pm: &mut Pixmap, ppp: f32) {
    STARS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.0 != ppp.to_bits()) {
            *cache = starfield(ppp).map(|t| (ppp.to_bits(), t));
        }
        let Some((_, tile)) = cache.as_ref() else { return };
        let shader =
            tiny_skia::Pattern::new(tile.as_ref(), SpreadMode::Repeat, tiny_skia::FilterQuality::Nearest, 1.0, Transform::identity());
        let p = Paint { shader, ..Paint::default() };
        if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, pm.width() as f32, pm.height() as f32) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    });
}

/// The globe's backdrop and lighting depend only on the view size, zoom, ocean and stars, not on
/// the rotation, so they are drawn once and reused while the globe turns.
struct GlobeLayers {
    key: (u32, u32, u32, u32, u32, u32, [u8; 4], bool),
    backdrop: Pixmap,
    shading: Pixmap,
}

thread_local! {
    static GLOBE_LAYERS: std::cell::RefCell<Option<GlobeLayers>> = const { std::cell::RefCell::new(None) };
}

fn with_globe_layers(pm: &mut Pixmap, g: &Globe, water: Rgba, stars: bool, ppp: f32, f: impl FnOnce(&mut Pixmap, &GlobeLayers)) {
    let (cx, cy) = g.center();
    let key = (
        pm.width(),
        pm.height(),
        ppp.to_bits(),
        (cx as f32).to_bits(),
        (cy as f32).to_bits(),
        (g.disc_radius() as f32).to_bits(),
        water.0,
        stars,
    );
    GLOBE_LAYERS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.key != key) {
            let (Some(mut backdrop), Some(mut shading)) = (Pixmap::new(pm.width(), pm.height()), Pixmap::new(pm.width(), pm.height()))
            else {
                return;
            };
            draw_globe_backdrop(&mut backdrop, g, water, stars, ppp);
            draw_globe_shading(&mut shading, g, ppp);
            *cache = Some(GlobeLayers { key, backdrop, shading });
        }
        if let Some(c) = cache.as_ref() {
            f(pm, c);
        }
    });
}

fn globe_backdrop(pm: &mut Pixmap, g: &Globe, water: Rgba, stars: bool, ppp: f32) {
    with_globe_layers(pm, g, water, stars, ppp, |pm, c| pm.data_mut().copy_from_slice(c.backdrop.data()));
}

fn globe_shading(pm: &mut Pixmap, g: &Globe, water: Rgba, stars: bool, ppp: f32) {
    with_globe_layers(pm, g, water, stars, ppp, |pm, c| {
        pm.draw_pixmap(0, 0, c.shading.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    });
}

/// The globe's surroundings and ocean: space (starry, when `stars`), an atmosphere glow, and the
/// ocean disc.
fn draw_globe_backdrop(pm: &mut Pixmap, g: &Globe, water: Rgba, stars: bool, ppp: f32) {
    let bg = globe_space();
    pm.fill(tiny_skia::Color::from_rgba8(bg.r(), bg.g(), bg.b(), 255));
    if stars {
        draw_stars(pm, ppp);
    }
    let (cx, cy) = g.center();
    draw_globe_disc(pm, cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp, water);
}

/// The atmosphere glow and the ocean disc, for a disc of radius `r` pixels at (`cx`, `cy`).
fn draw_globe_disc(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, water: Rgba) {
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    // Atmosphere: a glow fading out just beyond the limb.
    let glow_r = r * 1.07;
    let glow = (0x5A, 0x9B, 0xF0, 150);
    if let Some(shader) = RadialGradient::new(
        Point::from_xy(cx, cy),
        0.0,
        Point::from_xy(cx, cy),
        glow_r,
        vec![
            GradientStop::new(0.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
            GradientStop::new(r / glow_r, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, glow.3)),
            GradientStop::new(1.0, tiny_skia::Color::from_rgba8(glow.0, glow.1, glow.2, 0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(path) = PathBuilder::from_circle(cx, cy, glow_r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        pm.fill_path(&path, &paint(water, 1.0), FillRule::Winding, Transform::identity(), None);
    }
}

/// Light from the upper left: a soft highlight and a darker limb, so the globe reads as a sphere.
fn draw_globe_shading(pm: &mut Pixmap, g: &Globe, ppp: f32) {
    let (cx, cy) = g.center();
    draw_globe_light(pm, cx as f32 * ppp, cy as f32 * ppp, g.disc_radius() as f32 * ppp);
}

/// The lighting of a disc of radius `r` pixels at (`cx`, `cy`).
fn draw_globe_light(pm: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    if !(r.is_finite() && r > 0.5 && cx.is_finite() && cy.is_finite()) {
        return;
    }
    let light = Point::from_xy(cx - r * 0.35, cy - r * 0.4);
    if let Some(shader) = RadialGradient::new(
        light,
        0.0,
        Point::from_xy(cx, cy),
        r * 1.02,
        vec![
            GradientStop::new(0.0, tiny_skia::Color::from_rgba8(255, 255, 255, 46)),
            GradientStop::new(0.45, tiny_skia::Color::from_rgba8(255, 255, 255, 0)),
            GradientStop::new(0.8, tiny_skia::Color::from_rgba8(0, 0, 0, 18)),
            GradientStop::new(1.0, tiny_skia::Color::from_rgba8(0, 0, 0, 70)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(path) = PathBuilder::from_circle(cx, cy, r)
    {
        let p = Paint { shader, anti_alias: true, ..Paint::default() };
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

/// How far the atmosphere glow reaches, in disc radii.
pub const GLOBE_GLOW: f32 = 1.07;

/// The globe's surroundings as two square sprites for a disc of radius `r` pixels, centred in
/// them (side `2 × GLOBE_GLOW × r`): the atmosphere and ocean drawn under the map, and the
/// lighting drawn over it. Neither changes as the globe turns, so a view can scale them while it
/// moves instead of drawing them again.
pub fn globe_sprites(basemap: rustgis_map::Basemap, r: f32) -> Option<(Pixmap, Pixmap)> {
    if !(r.is_finite() && r >= 1.0) {
        return None;
    }
    // A whole number of pixels a side, with the disc exactly in proportion to it.
    let side = (2.0 * GLOBE_GLOW * r.min(2048.0)).round().max(2.0);
    let (r, c) = (side / (2.0 * GLOBE_GLOW), side / 2.0);
    let (mut under, mut over) = (Pixmap::new(side as u32, side as u32)?, Pixmap::new(side as u32, side as u32)?);
    draw_globe_disc(&mut under, c, c, r, basemap.palette().0);
    draw_globe_light(&mut over, c, c, r);
    Some((under, over))
}

/// A part of the map drawn flat, in Web Mercator, to lay over the globe while it turns.
pub struct DrapePatch {
    pub pixmap: Pixmap,
    /// The Web Mercator box the picture shows.
    pub merc: Extent,
}

/// Draws the longitude/latitude `boxes` (from `Globe::drape_extents`) of a globe map flat, at
/// the view's scale, each picture at most `max_side` pixels a side (coarser beyond that). The
/// globe view then drapes them over the sphere ([`rustgis_map::Globe::drape_mesh`]) while it
/// turns, with no drawing per frame. Labels are left out (the view hides them while it moves).
pub fn render_globe_drape(map: &mut Map, view: &View, boxes: &[Extent], max_side: u32) -> Vec<DrapePatch> {
    let ppp = if view.pixels_per_point.is_finite() { view.pixels_per_point.clamp(0.25, 4.0) } else { 1.0 };
    let res = view.camera.res;
    if !(res.is_finite() && res > 0.0) {
        return Vec::new();
    }
    let max_side = f64::from(max_side.clamp(64, 8192));
    let projection = std::mem::replace(&mut map.projection, rustgis_map::Projection::WebMercator);
    let (display, lock) = (map.label_display, map.label_lock.take());
    map.label_display.paused = true;
    let mut out = Vec::new();
    for b in boxes {
        let merc = extent_to_merc(b);
        if merc.is_empty() || !(merc.width() > 0.0 && merc.height() > 0.0) {
            continue;
        }
        // Metres per point: the view's, or coarser to fit the size limit.
        let res = res.max(merc.width().max(merc.height()) * ppp / max_side);
        // Whole points, rounded down so the picture never passes the size limit.
        let (w, h) = ((merc.width() / res).floor().max(1.0), (merc.height() / res).floor().max(1.0));
        if w * ppp < 2.0 || h * ppp < 2.0 {
            continue;
        }
        let camera = Camera { center: merc.center(), res };
        let v = View { width: w, height: h, pixels_per_point: ppp, camera };
        // The picture is given the box asked for, though rounding to whole points made it show
        // up to a point less: stretched by under a point, its edges stay exactly where asked, so
        // one at the Web Mercator limit is still stretched on to the pole (drape_mesh).
        if let Some(r) = render(map, &v) {
            out.push(DrapePatch { pixmap: r.pixmap, merc });
        }
    }
    map.projection = projection;
    map.label_display = display;
    map.label_lock = lock;
    out
}

/// A halo that contrasts with the text colour.
fn halo_for(text: Rgba) -> Rgba {
    let lum = 0.299 * text.r() as f32 + 0.587 * text.g() as f32 + 0.114 * text.b() as f32;
    if lum < 140.0 { Rgba::new(255, 255, 255, 225) } else { Rgba::new(20, 22, 26, 210) }
}

struct Placement {
    labels: Vec<PlacedLabel>,
    unplaced: Vec<PlacedLabel>,
    /// (layer id, row) of each placed label.
    keys: Vec<(u32, usize)>,
}

/// Greedy collision-free placement, highest priority first. Locked labels are all placed (they
/// were placed without collisions when the lock was taken).
fn place_labels(mut cands: Vec<(PlacedLabel, f32, (u32, usize))>, fixed: Vec<PlacedLabel>, w: f32, h: f32, locked: bool) -> Placement {
    cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    // The text's box and its icon's, together.
    let bbox = |l: &PlacedLabel| {
        let tw = text::measure(&l.text, l.size, l.bold);
        let th = l.size * 1.25;
        let cx = l.center_x();
        let text = (!l.text.is_empty()).then(|| [cx - tw / 2.0 - 2.0, l.y - th / 2.0, cx + tw / 2.0 + 2.0, l.y + th / 2.0]);
        let icon = l.icon.as_ref().map(|i| [i.x - i.w / 2.0, i.y - i.h / 2.0, i.x + i.w / 2.0, i.y + i.h / 2.0]);
        match (text, icon) {
            (Some(t), Some(i)) => [t[0].min(i[0]), t[1].min(i[1]), t[2].max(i[2]), t[3].max(i[3])],
            (Some(b), None) | (None, Some(b)) => b,
            (None, None) => [l.x, l.y, l.x, l.y],
        }
    };
    let mut boxes: Vec<[f32; 4]> = fixed.iter().map(bbox).collect();
    let mut out = Placement { labels: fixed, unplaced: Vec::new(), keys: Vec::new() };
    for (l, _, key) in cands.into_iter().take(5000) {
        let b = bbox(&l);
        let off_screen = b[2] < 0.0 || b[3] < 0.0 || b[0] > w || b[1] > h;
        let clipped = b[0] < 0.0 || b[1] < 0.0 || b[2] > w || b[3] > h;
        if off_screen || (!locked && clipped) {
            continue;
        }
        if !locked && boxes.iter().any(|o| b[0] < o[2] && o[0] < b[2] && b[1] < o[3] && o[1] < b[3]) {
            if out.unplaced.len() < 2000 {
                out.unplaced.push(PlacedLabel { color: UNPLACED, halo: Some(halo_for(UNPLACED)), icon: None, ..l });
            }
            continue;
        }
        boxes.push(b);
        out.labels.push(l);
        out.keys.push(key);
    }
    out
}

/// Renders at full resolution once the map's tiles have arrived: each pass fetches what it is
/// missing (a style, then its sources, then their tiles) and waits for it, at most a minute in
/// all. In the browser, which can't wait, it draws what has arrived.
pub(crate) fn render_complete(map: &mut Map, view: &View) -> Option<Rendered> {
    let mut r = render_with(map, view, true)?;
    let start = web_time::Instant::now();
    let budget = web_time::Duration::from_secs(60);
    let mut passes = 0;
    while r.tiles_pending && passes < 6 && start.elapsed() < budget && cfg!(not(target_arch = "wasm32")) {
        // Each wait within what is left of the minute.
        let left = budget.saturating_sub(start.elapsed());
        rustgis_tiles::cache::wait(left.min(web_time::Duration::from_secs(30)));
        r = render_with(map, view, true)?;
        passes += 1;
    }
    Some(r)
}

/// Renders a map with its labels, a scale bar and a north arrow to PNG bytes.
pub fn render_png(map: &mut Map, view: &View) -> Result<Vec<u8>, String> {
    let mut r = render_complete(map, view).ok_or("could not allocate the image")?;
    for l in r.unplaced.iter().chain(&r.labels) {
        text::draw(&mut r.pixmap, l, view.pixels_per_point as f32);
    }
    text::scale_bar(&mut r.pixmap, view, map.ground_res_at(&view.camera));
    draw_attribution(&mut r.pixmap, &r.attribution, view.pixels_per_point as f32);
    r.pixmap.encode_png().map_err(|e| e.to_string())
}

/// The tile layers' credits, small, in the bottom-right corner.
pub(crate) fn draw_attribution(pm: &mut Pixmap, attribution: &[String], ppp: f32) {
    if attribution.is_empty() {
        return;
    }
    let text = attribution.join(" | ");
    let size = 9.0;
    let (w, h) = (pm.width() as f32 / ppp, pm.height() as f32 / ppp);
    let tw = text::measure(&text, size, false).min(w);
    let label = PlacedLabel {
        text,
        x: w - tw / 2.0 - 4.0,
        y: h - size * 0.9,
        size,
        color: Rgba::rgb(0x33, 0x33, 0x33),
        halo: Some(Rgba::rgb(0xFF, 0xFF, 0xFF)),
        bold: false,
        icon: None,
        align: 0.0,
    };
    text::draw(pm, &label, ppp);
}

#[cfg(test)]
mod tests {

    fn label(text: &str, x: f32, icon: Option<PlacedIcon>, align: f32) -> PlacedLabel {
        PlacedLabel { text: text.into(), x, y: 20.0, size: 10.0, color: Rgba::rgb(0, 0, 0), halo: None, bold: false, icon, align }
    }

    #[test]
    fn places_icons_with_their_labels() {
        let icon = |x: f32| {
            let mut p = Pixmap::new(4, 4).unwrap();
            p.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
            PlacedIcon { image: std::sync::Arc::new(p), x, y: 20.0, w: 10.0, h: 10.0 }
        };
        // Two icons on the same spot collide even with no text; one far away doesn't.
        let cands = vec![
            (label("", 50.0, Some(icon(50.0)), 0.0), 2.0, (u32::MAX, 0)),
            (label("", 52.0, Some(icon(52.0)), 0.0), 1.0, (u32::MAX, 1)),
            (label("", 150.0, Some(icon(150.0)), 0.0), 0.0, (u32::MAX, 2)),
        ];
        let placed = place_labels(cands, Vec::new(), 200.0, 40.0, false);
        assert_eq!(placed.labels.iter().map(|l| l.x).collect::<Vec<_>>(), vec![50.0, 150.0]);
        // Left-aligned text starts at x.
        let l = label("Station", 100.0, None, -0.5);
        assert!((l.center_x() - (100.0 + text::measure("Station", 10.0, false) / 2.0)).abs() < 1e-3);
        // An icon is drawn into the map at its size.
        let mut pm = Pixmap::new(200, 40).unwrap();
        draw_icon(&mut pm, &icon(50.0), 1.0);
        let px = pm.pixel(50, 20).unwrap();
        assert_eq!((px.blue(), px.alpha()), (255, 255));
        assert_eq!(pm.pixel(60, 20).unwrap().alpha(), 0);
    }

    use super::*;
    use rustgis_data::{Field, FieldType, Value};

    #[test]
    fn worldview_detection_and_data() {
        use sample_data::Worldview::{China, Default};
        let detect = |tz: Option<&str>, locales: &[&str]| sample_data::Worldview::detect(tz, locales.iter().copied());
        assert_eq!(detect(Some("Asia/Shanghai"), &["en-US"]), China);
        assert_eq!(detect(Some("Asia/Urumqi"), &[]), China);
        assert_eq!(detect(Some("Europe/Berlin"), &["zh-CN", "en"]), China);
        assert_eq!(detect(None, &["zh_CN.UTF-8"]), China);
        assert_eq!(detect(None, &["zh-Hans-CN"]), China);
        assert_eq!(detect(Some("Asia/Taipei"), &["zh-TW"]), Default);
        assert_eq!(detect(Some("Asia/Hong_Kong"), &["zh-HK"]), Default);
        assert_eq!(detect(Some("America/New_York"), &["en-US", "zh-CN"]), Default);
        assert_eq!(detect(None, &[]), Default);
        assert_eq!(sample_data::Worldview::from_code(" China "), Some(China));
        assert_eq!(sample_data::Worldview::from_code("default"), Some(Default));
        assert_eq!(sample_data::Worldview::from_code("auto"), None);
        // Both files have the same fields; in China's view Taiwan is part of China.
        let read = |text| rustgis_data::io::read_geojson(text, "Countries").unwrap().0;
        let (def, chn) = (read(sample_data::COUNTRIES), read(sample_data::COUNTRIES_CHN));
        let fields = |d: &Dataset| d.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(fields(&def), fields(&chn));
        let names = |d: &Dataset| {
            let i = d.fields.iter().position(|f| f.name == "name").unwrap();
            d.features
                .iter()
                .filter_map(|f| if let Some(Value::Text(t)) = f.attrs.get(i) { Some(t.clone()) } else { None })
                .collect::<Vec<_>>()
        };
        assert!(names(&def).iter().any(|n| n == "Taiwan"));
        assert!(!names(&chn).iter().any(|n| n == "Taiwan"));
        assert!(names(&chn).iter().any(|n| n == "China"));
    }

    #[test]
    fn the_globe_drape_is_the_flat_map_of_its_box() {
        let mut m = Map::new("Map");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
        m.add_dataset(d);
        m.projection = rustgis_map::Projection::Globe;
        let paris = lonlat_to_merc(Coord::new(2.35, 48.85));
        let camera = Camera { center: paris, res: 2_000.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera };
        let boxes = Globe::new(&camera, 400.0, 300.0).drape_extents(400.0, 300.0, 0.25);
        let patches = render_globe_drape(&mut m, &view, &boxes, 4096);
        assert_eq!(patches.len(), 1);
        // The map is left as it was.
        assert!(m.projection.is_globe() && m.label_lock.is_none() && !m.label_display.paused);
        let p = &patches[0];
        assert!(p.merc.contains(paris) && p.merc.contains(lonlat_to_merc(boxes[0].center())));
        // Paris's marker is where the box puts it, as the flat map draws it.
        let px = |pm: &Pixmap, x: f64, y: f64| pm.pixel(x as u32, y as u32).map(|c| (c.red(), c.green(), c.blue()));
        let res = p.merc.width() / f64::from(p.pixmap.width());
        let at = px(&p.pixmap, (paris.x - p.merc.min_x) / res, (p.merc.max_y - paris.y) / res);
        m.projection = rustgis_map::Projection::WebMercator;
        let flat = render(&mut m, &View { camera: Camera { center: paris, res }, ..view }).unwrap();
        assert_eq!(at, px(&flat.pixmap, 200.0, 150.0));
        // Too big a box is drawn coarser, within the limit; junk draws nothing.
        let big = render_globe_drape(&mut m, &view, &[Extent::new(-180.0, -80.0, 180.0, 80.0)], 512);
        assert!(big.iter().all(|p| p.pixmap.width() <= 512 && p.pixmap.height() <= 512));
        // A box at the Web Mercator limit stays exactly there however coarse the picture, so its
        // mesh still reaches the pole.
        let lim = rustgis_geom::MAX_LAT;
        // (30 km points leave a remainder: the picture is 1335 points tall, 0.8 of a point short.)
        let coarse = View { camera: Camera { center: paris, res: 30_000.0 }, ..view };
        let world = render_globe_drape(&mut m, &coarse, &[Extent::new(-180.0, -lim, 180.0, lim)], 4096);
        let edge = std::f64::consts::PI * rustgis_geom::MERCATOR_R * 0.9999;
        assert!(
            world.iter().all(|p| p.merc.max_y >= edge && p.merc.min_y <= -edge),
            "{:?}",
            world.iter().map(|p| p.merc).collect::<Vec<_>>()
        );
        assert!(render_globe_drape(&mut m, &View { camera: Camera { center: paris, res: f64::NAN }, ..view }, &boxes, 4096).is_empty());
        assert!(render_globe_drape(&mut m, &view, &[Extent::EMPTY], 4096).is_empty());
        // The sprites: a square around the disc, or nothing for a junk radius.
        let (under, over) = globe_sprites(rustgis_map::Basemap::Paper, 100.0).unwrap();
        assert_eq!((under.width(), over.height()), (214, 214));
        assert_eq!(under.pixel(107, 107).map(|c| c.alpha()), Some(255));
        assert!(globe_sprites(rustgis_map::Basemap::Paper, f32::NAN).is_none());
    }

    #[test]
    fn the_starfield_is_the_same_sky_every_time() {
        let a = starfield(2.0).unwrap();
        assert_eq!((a.width(), a.height()), (1024, 1024));
        assert_eq!(a.data(), starfield(2.0).unwrap().data());
        // Stars on a transparent sky.
        let lit = a.pixels().iter().filter(|p| p.alpha() > 0).count();
        assert!(lit > 100 && lit < a.pixels().len() / 20, "{lit}");
        // Junk scales still give a tile.
        assert!(starfield(f32::NAN).is_some_and(|t| t.width() == 512));
        assert!(starfield(1e9).is_some_and(|t| t.width() == 2048));
    }

    #[test]
    fn renders_world_and_layers() {
        let mut m = Map::new("Map");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris".into())]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.labels.enabled = true;
            l.selection = vec![0];
        }
        let camera = Camera { center: lonlat_to_merc(Coord::new(2.35, 48.85)), res: 5000.0 };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera };
        let r = render(&mut m, &view).unwrap();
        assert_eq!(r.labels.len(), 1);
        assert_eq!(r.labels[0].text, "Paris");
        let png = render_png(&mut m, &view).unwrap();
        assert!(png.len() > 1000);
        assert!(!world().is_empty());
    }

    #[test]
    fn degenerate_views_do_not_panic() {
        let mut m = Map::new("Map");
        for (w, h) in [(0.0, 0.0), (1e9, 1e9), (f64::NAN, 3.0)] {
            let view = View { width: w, height: h, pixels_per_point: 1.0, camera: Camera::world() };
            let _ = render(&mut m, &view);
        }
    }

    /// The world sample the golden pixmap test renders.
    fn sample_world() -> Map {
        let mut m = Map::new("Map");
        for (name, text) in [
            ("Countries", sample_data::COUNTRIES),
            ("Rivers", sample_data::RIVERS),
            ("Lakes", sample_data::LAKES),
            ("Cities", sample_data::CITIES),
        ] {
            if let Some(d) = world_dataset(name, text) {
                m.add_dataset(d);
            }
        }
        m
    }

    fn fnv1a(data: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in data {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Golden hash of the sample world render: the GPU-backend refactor (Backend/Dest
    /// plumbing) must not change a single pixel of the CPU path.
    #[test]
    fn world_atlas_pixmap_is_byte_stable() {
        let mut m = sample_world();
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world() };
        let r = render(&mut m, &view).expect("render");
        assert_eq!(r.pixmap.data().len(), 800 * 500 * 4);
        let h = fnv1a(r.pixmap.data());
        assert_eq!(h, 0x8caa_2097_9bae_31b7, "golden pixmap hash changed: {h:#x}");
    }

    fn square_layer(m: &mut Map, name: &str, x: f64, y: f64, s: f64) -> u32 {
        let mut d = Dataset::new(name, GeometryType::Polygon, vec![]);
        d.push(
            Some(Geometry::Polygon(vec![vec![vec![
                Coord::new(x, y),
                Coord::new(x + s, y),
                Coord::new(x + s, y + s),
                Coord::new(x, y + s),
            ]]])),
            vec![],
        );
        m.add_dataset(d)
    }

    fn cmd_kinds(scene: &gpu_scene::GpuScene) -> Vec<&'static str> {
        scene
            .cmds
            .iter()
            .map(|c| match c {
                gpu_scene::DrawCmd::Mesh { .. } => "mesh",
                gpu_scene::DrawCmd::Points { .. } => "points",
                gpu_scene::DrawCmd::Image { .. } => "image",
            })
            .collect()
    }

    #[test]
    fn gpu_backend_builds_scene_for_flat_world() {
        let mut m = sample_world();
        if let Some(l) = m.layers.iter_mut().find(|l| l.name == "Cities") {
            l.labels.enabled = true;
        }
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world() };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        assert!(kinds.iter().filter(|k| **k == "mesh").count() >= 2, "countries/lakes/rivers as meshes: {kinds:?}");
        assert!(kinds.contains(&"points"), "cities as marker instances: {kinds:?}");
        // The label channel is identical to the CPU render's.
        let mut m2 = sample_world();
        if let Some(l) = m2.layers.iter_mut().find(|l| l.name == "Cities") {
            l.labels.enabled = true;
        }
        let cpu = render(&mut m2, &view).expect("cpu");
        assert_eq!(scene.labels.len(), cpu.labels.len());
        assert_eq!(scene.labels.first().map(|l| l.text.as_str()), cpu.labels.first().map(|l| l.text.as_str()));
    }

    #[test]
    fn gpu_backend_interleaves_cpu_image_for_blended_layer() {
        let mut m = Map::new("Map");
        // Layers draw bottom-up: the last one in the list first.
        square_layer(&mut m, "a", 0.0, 0.0, 10.0);
        let b = square_layer(&mut m, "b", 20.0, 0.0, 10.0);
        square_layer(&mut m, "c", 40.0, 0.0, 10.0);
        if let Some(l) = m.layer_mut(b) {
            l.blend = rustgis_map::effects::Blend::Multiply;
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world() };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        let last_image = kinds.iter().rposition(|k| *k == "image").expect("the blended layer's cpu image");
        assert!(kinds[..last_image].contains(&"mesh"), "a mesh below the blended layer: {kinds:?}");
        assert!(kinds[last_image + 1..].contains(&"mesh"), "a mesh above the blended layer: {kinds:?}");
    }

    #[test]
    fn gpu_backend_appends_selection_highlight() {
        let mut m = Map::new("Map");
        let id = square_layer(&mut m, "s", 0.0, 0.0, 10.0);
        if let Some(l) = m.layer_mut(id) {
            l.selection = vec![0];
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world() };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        let kinds = cmd_kinds(&scene);
        assert_eq!(kinds.last(), Some(&"mesh"), "the highlight mesh follows the layer: {kinds:?}");
        assert!(kinds.iter().filter(|k| **k == "mesh").count() >= 2, "layer mesh + highlight mesh: {kinds:?}");
    }

    #[test]
    fn gpu_scene_carries_backdrop_as_clear_color_and_no_image_for_pure_vectors() {
        let mut m = sample_world();
        let view = View { width: 800.0, height: 500.0, pixels_per_point: 1.0, camera: Camera::world() };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        assert!(
            !scene.cmds.iter().any(|c| matches!(c, gpu_scene::DrawCmd::Image { .. })),
            "a pure-vector scene must carry no CPU image (it would poison the camera fast path): {} cmds",
            scene.cmds.len()
        );
        let (water, ..) = m.basemap.palette();
        assert_eq!(scene.clear, mesh::premul(water), "the clear colour is the basemap's water");
    }

    #[test]
    fn gpu_backend_skips_non_finite_stroke_widths() {
        // A hostile symbol state (NaN stroke width) must not put NaN vertices on the GPU:
        // the line's stroke is skipped, the rest of the scene is unaffected.
        let mut m = Map::new("Map");
        let mut d = Dataset::new("l", GeometryType::Polyline, vec![]);
        d.push(Some(Geometry::Line(vec![vec![Coord::new(-50.0, 0.0), Coord::new(50.0, 0.0)]])), vec![]);
        let id = m.add_dataset(d);
        if let Some(l) = m.layer_mut(id) {
            l.renderer = rustgis_map::Renderer::Single(rustgis_map::Symbol {
                fill: rustgis_map::Rgba::rgb(0, 0, 200),
                stroke: rustgis_map::Rgba::rgb(0, 0, 200),
                stroke_width: f32::NAN,
                size: 0.0,
                marker: rustgis_map::Marker::Circle,
            });
        }
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world() };
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut m, &view, Backend::Gpu) else { panic!("gpu scene") };
        for c in &scene.cmds {
            if let gpu_scene::DrawCmd::Mesh { vertices, .. } = c {
                assert!(vertices.iter().all(|v| v.position[0].is_finite() && v.position[1].is_finite() && v.stroke_px.is_finite()));
            }
        }
    }

    #[test]
    fn gpu_backend_handles_non_finite_geometry_without_panicking() {
        // A polygon with a NaN vertex (hostile file): the GPU drops the point (lyon's path
        // builder panics on NaN — never feed it), the healthy layer still tessellates, and
        // the frame completes like the CPU's.
        let build = || {
            let mut m = Map::new("Map");
            let mut d = Dataset::new("bad", GeometryType::Polygon, vec![]);
            d.push(
                Some(Geometry::Polygon(vec![vec![vec![
                    Coord::new(0.0, 0.0),
                    Coord::new(10.0, f64::NAN),
                    Coord::new(10.0, 10.0),
                    Coord::new(0.0, 10.0),
                ]]])),
                vec![],
            );
            m.add_dataset(d);
            square_layer(&mut m, "good", 30.0, 0.0, 10.0);
            m
        };
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world() };
        let mut gm = build();
        let Some(RenderOutput::Gpu(scene)) = render_backend(&mut gm, &view, Backend::Gpu) else { panic!("gpu scene") };
        assert!(cmd_kinds(&scene).contains(&"mesh"), "the healthy layer tessellated");
        let mut cm = build();
        assert!(render(&mut cm, &view).is_some(), "cpu frame completes too");
    }

    #[test]
    fn gpu_backend_falls_back_for_globe_and_effects() {
        let mut m = sample_world();
        let view = View { width: 400.0, height: 300.0, pixels_per_point: 1.0, camera: Camera::world() };
        m.projection = rustgis_map::Projection::Globe;
        assert!(matches!(render_backend(&mut m, &view, Backend::Gpu), Some(RenderOutput::Cpu(_))), "globe stays on the CPU renderer");
        m.projection = rustgis_map::Projection::WebMercator;
        m.view.effects.vignette = true;
        assert!(matches!(render_backend(&mut m, &view, Backend::Gpu), Some(RenderOutput::Cpu(_))), "view effects stay on the CPU renderer");
    }
}
