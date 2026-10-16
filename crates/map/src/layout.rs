//! Page layouts: a page (in millimetres) holding map frames and the surrounds that explain them
//! (north arrows, scale bars, legends, text, pictures, tables, charts), exported as images or PDF.
//!
//! Element positions are in millimetres from the page's top-left corner. A map frame shows a map
//! of the project at its own centre and scale, independent of the map view.

use rustgis_geom::Coord;
use serde::{Deserialize, Serialize};

use crate::{Camera, Map, Rgba};

/// Logical points per millimetre (96 points per inch).
pub const PT_PER_MM: f64 = 96.0 / 25.4;
/// Metres of paper per logical point.
pub const PAPER_M_PER_PT: f64 = 0.0254 / 96.0;

/// The smallest element side, in mm: thin enough for a hairline rule (a 0.05 mm rectangle).
pub const MIN_ELEMENT_MM: f64 = 0.05;

/// The largest element id a layout keeps (imported ones above it are renumbered).
const MAX_ID: u32 = 1_000_000;

/// Page sizes: (id, label, width mm, height mm), portrait.
pub const PAGES: &[(&str, &str, f64, f64)] = &[
    ("letter", "Letter", 215.9, 279.4),
    ("legal", "Legal", 215.9, 355.6),
    ("tabloid", "Tabloid", 279.4, 431.8),
    ("a5", "A5", 148.0, 210.0),
    ("a4", "A4", 210.0, 297.0),
    ("a3", "A3", 297.0, 420.0),
    ("a2", "A2", 420.0, 594.0),
    ("a1", "A1", 594.0, 841.0),
    ("a0", "A0", 841.0, 1189.0),
];

/// The outline of a map frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum FrameShape {
    #[default]
    Rectangle,
    Circle,
    Ellipse,
    /// A polygon in the frame's unit square (0..1 from its top-left), drawn by Polygon or Lasso.
    Polygon(Vec<(f64, f64)>),
}

impl FrameShape {
    /// The outline as points in the frame's unit square.
    pub fn outline(&self) -> Vec<(f64, f64)> {
        match self {
            FrameShape::Rectangle => vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            FrameShape::Circle | FrameShape::Ellipse => (0..96)
                .map(|i| {
                    let t = i as f64 / 96.0 * std::f64::consts::TAU;
                    (0.5 + 0.5 * t.cos(), 0.5 + 0.5 * t.sin())
                })
                .collect(),
            FrameShape::Polygon(p) => p.clone(),
        }
    }
}

/// A graticule drawn over a map frame, labelled at the frame's edges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// Degrees between lines; 0 picks a step from the frame's extent.
    #[serde(default)]
    pub step: f64,
    pub color: Rgba,
    #[serde(default = "yes")]
    pub labels: bool,
}

fn yes() -> bool {
    true
}

/// What an element is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ElementKind {
    MapFrame {
        /// The map shown (by name).
        map: String,
        /// Centre (longitude/latitude) and scale denominator on paper.
        center: Coord,
        scale: f64,
        #[serde(default)]
        shape: FrameShape,
        #[serde(default)]
        grid: Option<Grid>,
        /// Another frame whose extent this one outlines (an extent indicator).
        #[serde(default)]
        extent_of: Option<u32>,
    },
    NorthArrow {
        frame: u32,
    },
    ScaleBar {
        frame: u32,
    },
    Legend {
        frame: u32,
        title: String,
        /// List every class of every visible layer. Off (the default), the legend shows what the
        /// frame draws: classes some drawn feature has, `<all other values>` only when used, and
        /// no layer that draws nothing at the frame's scale (#207).
        #[serde(default)]
        all_classes: bool,
        /// Leave `<all other values>` out of the legend, in either mode. Only the legend changes:
        /// the features it stands for are drawn as before.
        #[serde(default)]
        hide_other: bool,
    },
    /// Text; dynamic tokens (`{page}`, `{date}`, `{scale}`…) expand when drawn.
    Text {
        text: String,
        size: f64,
        color: Rgba,
        #[serde(default)]
        bold: bool,
    },
    Picture {
        /// Base64 PNG or JPEG.
        image: String,
    },
    /// The attribute table of a layer in a frame's map.
    Table {
        frame: u32,
        layer: String,
        #[serde(default = "rows")]
        rows: usize,
    },
    /// A bar chart of a numeric field of a layer in a frame's map, one bar per feature.
    Chart {
        frame: u32,
        layer: String,
        field: String,
        /// Bars are labelled with this field, if set.
        #[serde(default)]
        label: Option<String>,
    },
    /// A rectangle drawn on the page (a neatline or a backdrop).
    Rectangle {
        fill: Rgba,
        outline: Rgba,
        width: f64,
    },
}

fn rows() -> usize {
    12
}

impl ElementKind {
    pub fn code(&self) -> &'static str {
        match self {
            ElementKind::MapFrame { .. } => "mapFrame",
            ElementKind::NorthArrow { .. } => "northArrow",
            ElementKind::ScaleBar { .. } => "scaleBar",
            ElementKind::Legend { .. } => "legend",
            ElementKind::Text { .. } => "text",
            ElementKind::Picture { .. } => "picture",
            ElementKind::Table { .. } => "table",
            ElementKind::Chart { .. } => "chart",
            ElementKind::Rectangle { .. } => "rectangle",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            ElementKind::MapFrame { .. } => "Map Frame",
            ElementKind::NorthArrow { .. } => "North Arrow",
            ElementKind::ScaleBar { .. } => "Scale Bar",
            ElementKind::Legend { .. } => "Legend",
            ElementKind::Text { .. } => "Text",
            ElementKind::Picture { .. } => "Picture",
            ElementKind::Table { .. } => "Table Frame",
            ElementKind::Chart { .. } => "Chart Frame",
            ElementKind::Rectangle { .. } => "Rectangle",
        }
    }
    /// The map frame this element belongs to.
    pub fn frame(&self) -> Option<u32> {
        match self {
            ElementKind::NorthArrow { frame }
            | ElementKind::ScaleBar { frame }
            | ElementKind::Legend { frame, .. }
            | ElementKind::Table { frame, .. }
            | ElementKind::Chart { frame, .. } => Some(*frame),
            _ => None,
        }
    }
}

/// An element on the page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: u32,
    pub name: String,
    /// Top-left corner and size, in millimetres.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default = "yes")]
    pub visible: bool,
    pub kind: ElementKind,
}

impl Element {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// A map series: one page per feature of an index layer, the main frame zoomed to each.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapSeries {
    /// The frame that follows the pages.
    pub frame: u32,
    /// The index layer, in the frame's map.
    pub layer: String,
    /// The field naming each page (`{pageName}`); the row number when empty.
    #[serde(default)]
    pub name_field: String,
    /// Space around each page's feature, as a fraction of its extent.
    #[serde(default = "margin")]
    pub margin: f64,
    /// The page shown (0-based row of the index layer).
    #[serde(default)]
    pub page: usize,
}

fn margin() -> f64 {
    0.1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub elements: Vec<Element>,
    #[serde(default)]
    pub series: Option<MapSeries>,
    #[serde(default)]
    next_id: u32,
}

impl Layout {
    pub fn new(name: impl Into<String>, width: f64, height: f64) -> Self {
        Layout { name: name.into(), width, height, elements: Vec::new(), series: None, next_id: 1 }
    }

    /// Adds an element and returns its id.
    pub fn add(&mut self, name: impl Into<String>, (x, y, w, h): (f64, f64, f64, f64), kind: ElementKind) -> u32 {
        let id = self.next_id.max(self.elements.iter().map(|e| e.id).max().unwrap_or(0).saturating_add(1));
        self.next_id = id.saturating_add(1);
        // Templates on tiny pages can compute sizes near zero.
        let (w, h) = (finite_or(w, 1.0).clamp(MIN_ELEMENT_MM, 5000.0), finite_or(h, 1.0).clamp(MIN_ELEMENT_MM, 5000.0));
        self.elements.push(Element { id, name: name.into(), x, y, w, h, visible: true, kind });
        id
    }

    pub fn element(&self, id: u32) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }
    pub fn element_mut(&mut self, id: u32) -> Option<&mut Element> {
        self.elements.iter_mut().find(|e| e.id == id)
    }

    /// The first map frame.
    pub fn first_frame(&self) -> Option<u32> {
        self.elements.iter().find(|e| matches!(e.kind, ElementKind::MapFrame { .. })).map(|e| e.id)
    }

    /// The topmost element at a page point (mm).
    pub fn hit(&self, x: f64, y: f64) -> Option<u32> {
        self.elements.iter().rev().find(|e| e.visible && e.contains(x, y)).map(|e| e.id)
    }

    /// Removes an element and the surrounds that belong to it.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.elements.len();
        self.elements.retain(|e| e.id != id && e.kind.frame() != Some(id));
        for e in &mut self.elements {
            if let ElementKind::MapFrame { extent_of, .. } = &mut e.kind
                && *extent_of == Some(id)
            {
                *extent_of = None;
            }
        }
        if self.series.as_ref().is_some_and(|s| s.frame == id) {
            self.series = None;
        }
        self.elements.len() != before
    }

    /// Keeps the page and elements sane after loading or editing (layout files are untrusted):
    /// sizes in range, unique element ids, and no references to missing frames.
    pub fn clamp(&mut self) {
        self.width = finite_or(self.width, 210.0).clamp(20.0, 5000.0);
        self.height = finite_or(self.height, 297.0).clamp(20.0, 5000.0);
        self.elements.truncate(500);
        let mut seen = std::collections::HashSet::new();
        let mut next = self.elements.iter().map(|e| e.id).filter(|id| *id <= MAX_ID).max().unwrap_or(0).saturating_add(1);
        for e in &mut self.elements {
            // Ids stay well below u32::MAX so new elements never run out of numbers. A repeated id
            // (only a hand-edited file has one) is renumbered; references to it keep meaning the
            // first element with that id, as they did when the file was written.
            if e.id == 0 || e.id > MAX_ID || !seen.insert(e.id) {
                e.id = next;
                seen.insert(next);
                next = next.saturating_add(1);
            }
            e.w = finite_or(e.w, 20.0).clamp(MIN_ELEMENT_MM, 5000.0);
            e.h = finite_or(e.h, 20.0).clamp(MIN_ELEMENT_MM, 5000.0);
            e.x = finite_or(e.x, 0.0).clamp(-5000.0, 5000.0);
            e.y = finite_or(e.y, 0.0).clamp(-5000.0, 5000.0);
            e.name = e.name.chars().take(200).collect();
            match &mut e.kind {
                ElementKind::MapFrame { center, scale, shape, grid, .. } => {
                    *scale = finite_or(*scale, 50_000_000.0).clamp(100.0, 1e10);
                    if !center.is_finite() {
                        *center = Coord::new(0.0, 0.0);
                    }
                    center.x = center.x.clamp(-540.0, 540.0);
                    center.y = center.y.clamp(-85.0, 85.0);
                    if let FrameShape::Polygon(p) = shape {
                        p.retain(|(u, v)| u.is_finite() && v.is_finite());
                        p.truncate(10_000);
                        p.iter_mut().for_each(|(u, v)| (*u, *v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)));
                        if p.len() < 3 {
                            *shape = FrameShape::Rectangle;
                        }
                    }
                    if let Some(g) = grid {
                        g.step = finite_or(g.step, 0.0).clamp(0.0, 90.0);
                    }
                }
                ElementKind::Text { text, size, .. } => {
                    *size = finite_or(*size, 12.0).clamp(2.0, 400.0);
                    text.truncate(text.char_indices().nth(5000).map_or(text.len(), |(i, _)| i));
                }
                ElementKind::Table { rows, .. } => *rows = (*rows).clamp(1, 500),
                ElementKind::Rectangle { width, .. } => *width = finite_or(*width, 1.0).clamp(0.0, 50.0),
                _ => {}
            }
        }
        self.next_id = next;
        // References must point at map frames that exist.
        let frames: std::collections::HashSet<u32> =
            self.elements.iter().filter(|e| matches!(e.kind, ElementKind::MapFrame { .. })).map(|e| e.id).collect();
        self.elements.retain(|e| e.kind.frame().is_none_or(|f| frames.contains(&f)));
        for e in &mut self.elements {
            if let ElementKind::MapFrame { extent_of, .. } = &mut e.kind
                && extent_of.is_some_and(|o| !frames.contains(&o) || o == e.id)
            {
                *extent_of = None;
            }
        }
        if let Some(s) = &mut self.series {
            if !frames.contains(&s.frame) {
                self.series = None;
            } else {
                s.margin = finite_or(s.margin, 0.1).clamp(0.0, 5.0);
            }
        }
    }
}

fn finite_or(v: f64, d: f64) -> f64 {
    if v.is_finite() { v } else { d }
}

/// The camera that shows a frame's centre at its paper scale in `map`'s projection, for a frame
/// `w`×`h` logical points. The globe takes its camera in Web Mercator, whose ground scale at the
/// centre it matches, so it is handled like the flat map.
pub fn frame_camera(map: &Map, center: Coord, scale: f64) -> Camera {
    let mut cam = map.camera;
    if let Some(c) = map.to_map(center) {
        cam.center = c;
    }
    let per = map.flat().ground_per_unit(cam.center).max(1e-9);
    cam.res = scale * PAPER_M_PER_PT / per;
    cam.clamp();
    cam
}

/// The scale that fits a lon/lat extent into a frame `w_mm`×`h_mm` with a margin, and its centre.
pub fn fit_scale(map: &Map, e: &rustgis_geom::Extent, w_mm: f64, h_mm: f64) -> Option<(Coord, f64)> {
    if e.is_empty() || w_mm <= 0.0 || h_mm <= 0.0 {
        return None;
    }
    let flat = map.flat();
    let mut ext = rustgis_geom::Extent::EMPTY;
    for c in
        [Coord::new(e.min_x, e.min_y), Coord::new(e.max_x, e.min_y), Coord::new(e.max_x, e.max_y), Coord::new(e.min_x, e.max_y), e.center()]
    {
        if let Some(p) = flat.forward_checked(c) {
            ext.expand(p);
        }
    }
    if ext.is_empty() {
        return None;
    }
    let center = flat.inverse(ext.center())?;
    let per = flat.ground_per_unit(ext.center()).max(1e-9);
    let (gw, gh) = (ext.width().max(1.0) * per * 1.1, ext.height().max(1.0) * per * 1.1);
    let scale = (gw / (w_mm / 1000.0)).max(gh / (h_mm / 1000.0));
    Some((center, scale.clamp(100.0, 1e10)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_add_hit_and_remove_with_surrounds() {
        let mut l = Layout::new("Layout", 297.0, 210.0);
        let f = l.add(
            "Map Frame",
            (10.0, 10.0, 200.0, 150.0),
            ElementKind::MapFrame {
                map: "Map".into(),
                center: Coord::new(0.0, 0.0),
                scale: 1e8,
                shape: FrameShape::Rectangle,
                grid: None,
                extent_of: None,
            },
        );
        let n = l.add("North Arrow", (215.0, 10.0, 15.0, 20.0), ElementKind::NorthArrow { frame: f });
        let t = l.add(
            "Title",
            (20.0, 165.0, 100.0, 12.0),
            ElementKind::Text { text: "World".into(), size: 18.0, color: Rgba::rgb(0, 0, 0), bold: true },
        );
        assert_eq!(l.hit(50.0, 50.0), Some(f));
        assert_eq!(l.hit(220.0, 15.0), Some(n));
        assert_eq!(l.hit(290.0, 200.0), None);
        assert!(l.remove(f));
        assert!(l.element(n).is_none(), "the frame's north arrow goes with it");
        assert!(l.element(t).is_some());
    }

    #[test]
    fn hostile_layouts_are_repaired() {
        let text = r#"{"name":"x","width":1e300,"height":-5,"elements":[
            {"id":3,"name":"a","x":0,"y":0,"w":1e9,"h":10,"kind":{"MapFrame":{"map":"M","center":{"x":1e300,"y":200},"scale":-1,"shape":{"Polygon":[[0,0],[1,1]]},"extent_of":3}}},
            {"id":3,"name":"b","x":0,"y":0,"w":10,"h":10,"kind":{"Text":{"text":"t","size":1e9,"color":[0,0,0,255]}}},
            {"id":9,"name":"c","x":0,"y":0,"w":10,"h":10,"kind":{"Legend":{"frame":42,"title":"L"}}}],
            "series":{"frame":77,"layer":"x"}}"#;
        let mut l: Layout = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}"));
        l.clamp();
        let ids: Vec<u32> = l.elements.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 2, "the legend of a missing frame goes");
        assert_ne!(ids[0], ids[1], "duplicate ids are renumbered");
        assert!(l.width <= 5000.0 && l.height >= 20.0 && l.series.is_none());
        match &l.elements[0].kind {
            ElementKind::MapFrame { scale, shape, extent_of, center, .. } => {
                assert!(*scale >= 100.0 && *shape == FrameShape::Rectangle && extent_of.is_none() && center.is_finite());
            }
            _ => panic!("frame expected"),
        }
        assert!(matches!(&l.elements[1].kind, ElementKind::Text { size, .. } if *size <= 400.0));
        let id = l.add("n", (0.0, 0.0, 1.0, 1.0), ElementKind::NorthArrow { frame: ids[0] });
        assert!(!ids.contains(&id));
        // An id at u32::MAX is renumbered, and adding after it doesn't overflow.
        let mut big: Layout = serde_json::from_str(r#"{"name":"b","width":100,"height":100,"elements":[{"id":4294967295,"name":"t","x":0,"y":0,"w":5,"h":5,"kind":{"Text":{"text":"t","size":10,"color":[0,0,0,255]}}}]}"#).unwrap_or_else(|e| panic!("{e}"));
        big.clamp();
        assert!(big.elements[0].id <= MAX_ID);
        let a = big.add(
            "a",
            (0.0, 0.0, 1.0, 1.0),
            ElementKind::Rectangle { fill: Rgba::rgb(0, 0, 0), outline: Rgba::rgb(0, 0, 0), width: 1.0 },
        );
        assert!(a != big.elements[0].id && a <= MAX_ID + 1);
    }

    #[test]
    fn hairline_rectangles_keep_their_height() {
        let mut l = Layout::new("Layout", 210.0, 297.0);
        let rule = ElementKind::Rectangle { fill: Rgba::rgb(0, 0, 0), outline: Rgba::rgb(0, 0, 0), width: 0.0 };
        let id = l.add("Rule", (14.0, 33.0, 182.0, 0.15), rule.clone());
        assert_eq!(l.element(id).map(|e| e.h), Some(0.15));
        // A zero height becomes the thinnest element, not a 1 mm box.
        let zero = l.add("Zero", (14.0, 40.0, 182.0, 0.0), rule);
        assert_eq!(l.element(zero).map(|e| e.h), Some(MIN_ELEMENT_MM));
        // So does a loaded one.
        let text = r#"{"name":"x","width":210,"height":297,"elements":[
            {"id":1,"name":"a","x":14,"y":33,"w":182,"h":0.15,"kind":{"Rectangle":{"fill":[0,0,0,255],"outline":[0,0,0,0],"width":0}}},
            {"id":2,"name":"b","x":14,"y":40,"w":182,"h":0,"kind":{"Rectangle":{"fill":[0,0,0,0],"outline":[0,0,0,255],"width":0.4}}}]}"#;
        let mut l: Layout = serde_json::from_str(text).unwrap();
        l.clamp();
        assert_eq!(l.element(1).map(|e| e.h), Some(0.15));
        assert_eq!(l.element(2).map(|e| e.h), Some(MIN_ELEMENT_MM));
    }

    #[test]
    fn frame_camera_keeps_paper_scale() {
        let map = Map::new("Map");
        let cam = frame_camera(&map, Coord::new(0.0, 0.0), 1_000_000.0);
        // At the equator one logical point of paper (0.2646 mm) covers 264.6 m at 1:1,000,000.
        assert!((cam.res - 264.583).abs() < 0.5, "{}", cam.res);
        let (c, s) = fit_scale(&map, &rustgis_geom::Extent::new(-10.0, -10.0, 10.0, 10.0), 200.0, 200.0).unwrap_or_default();
        assert!(c.x.abs() < 1e-6 && s > 1e7 && s < 2e7, "{c:?} {s}");
    }

    #[test]
    fn frame_camera_follows_the_frame_on_the_globe() {
        let mut map = Map::new("Map");
        map.projection = crate::Projection::Globe;
        let near = frame_camera(&map, Coord::new(10.0, 20.0), 60_000_000.0);
        let far = frame_camera(&map, Coord::new(10.0, 20.0), 300_000_000.0);
        assert!((far.res / near.res - 5.0).abs() < 1e-6, "{} {}", near.res, far.res);
        let c = map.to_lonlat(near.center).unwrap_or_default();
        assert!((c.x - 10.0).abs() < 1e-6 && (c.y - 20.0).abs() < 1e-6, "{c:?}");
        // The globe's ground scale at the centre is the frame's paper scale.
        let ground = near.res * 20f64.to_radians().cos();
        assert!((ground - 60_000_000.0 * PAPER_M_PER_PT).abs() < 1e-3, "{ground}");
    }

    #[test]
    fn repeated_ids_keep_references_on_the_first_element() {
        let text = r#"{"name":"x","width":200,"height":100,"elements":[
            {"id":1,"name":"frame","x":0,"y":0,"w":100,"h":80,"kind":{"MapFrame":{"map":"M","center":{"x":0,"y":0},"scale":1e7}}},
            {"id":1,"name":"box","x":0,"y":0,"w":10,"h":10,"kind":{"Rectangle":{"fill":[0,0,0,255],"outline":[0,0,0,255],"width":1}}},
            {"id":2,"name":"arrow","x":110,"y":0,"w":10,"h":10,"kind":{"NorthArrow":{"frame":1}}}]}"#;
        let mut l: Layout = serde_json::from_str(text).unwrap();
        l.clamp();
        assert_eq!(l.element(1).map(|e| e.name.as_str()), Some("frame"));
        let ids: std::collections::HashSet<u32> = l.elements.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), 3, "the box got a fresh id");
        assert!(matches!(l.element(2).map(|e| &e.kind), Some(ElementKind::NorthArrow { frame: 1 })));
    }
}
