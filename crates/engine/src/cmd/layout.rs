//! Data › Project (New Layout, Import Layout), Publish › Map Frames, Map Surrounds and Text, and
//! Publish › Output (Export Layout) and Save As (Layout File): page layouts.

use std::path::Path;

use rustgis_geom::{Coord, Extent};
use rustgis_map::Rgba;
use rustgis_map::layout::{ElementKind, FrameShape, Grid, Layout, MIN_ELEMENT_MM, MapSeries, PAGES, fit_scale};
use serde_json::{Value, json};

use super::{b, f, numbered, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// Most compressed page data a PDF export holds (less in the browser).
const PDF_BUDGET: usize = if cfg!(target_arch = "wasm32") { 256 << 20 } else { 1 << 30 };

/// Layout files: JSON with this extension.
pub const LAYOUT_EXTENSION: &str = "rgislayout";
/// Largest layout file read (64 MiB, pictures included).
const MAX_LAYOUT: u64 = 64 << 20;

pub fn commands() -> Vec<C> {
    vec![
        C::new("layout.new", "New Layout", "Data › Project", new_layout).params(
            r#"{"name"?: string, "page"?: "letter"|"legal"|"tabloid"|"a5"|"a4"|"a3"|"a2"|"a1"|"a0", "orientation"?: "landscape"|"portrait", "width"?: mm, "height"?: mm, "template"?: "map" (frame, title, legend, north arrow, scale bar; the default)|"blank"}"#,
        ),
        C::new("layout.activate", "Open Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            s.active_layout = Some(i);
            Ok(json!({"layout": i}))
        })
        .view()
        .params(r#"{"name": string | "layout": name|index}"#),
        C::new("layout.close", "Show Map", "Browser", |s, _| {
            s.active_layout = None;
            Ok(Value::Null)
        })
        .view(),
        C::new("layout.remove", "Delete Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            s.project.layouts.remove(i);
            // Deleting another layout keeps the one open (a layout tab's close button).
            s.active_layout = match s.active_layout {
                Some(a) if a > i => Some(a - 1),
                Some(a) if a < i => Some(a),
                _ => None,
            };
            Ok(Value::Null)
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout)}"#),
        C::new("layout.rename", "Rename Layout", "Browser", |s, p| {
            let name = ps(p, "newName").map(str::trim).filter(|n| !n.is_empty()).ok_or("newName is required")?;
            let i = index(s, &by_name(p))?;
            // Unique among the other layouts, so names keep finding one layout.
            let unique = numbered(name, |n| s.project.layouts.iter().enumerate().any(|(j, l)| j != i && l.name.eq_ignore_ascii_case(n)));
            if let Some(l) = s.project.layouts.get_mut(i) {
                l.name = unique.clone();
            }
            Ok(json!({"name": unique}))
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout), "newName": string}"#),
        // A copy of a layout with all its elements (#288), right after it and open. Element ids
        // are per layout, so legends and scale bars stay on their frames, which show the same maps.
        C::new("layout.duplicate", "Duplicate Layout", "Browser", |s, p| {
            let i = index(s, &by_name(p))?;
            let mut l = s.project.layouts.get(i).ok_or("open a layout first")?.clone();
            l.name = super::copy_name(ps(p, "newName"), &l.name, |n| s.project.layouts.iter().any(|x| x.name.eq_ignore_ascii_case(n)));
            let name = l.name.clone();
            s.project.layouts.insert(i + 1, l);
            s.active_layout = Some(i + 1);
            Ok(json!({"name": name, "layout": i + 1}))
        })
        .params(r#"{"name"?: string, "layout"?: name|index (instead of name; default: the open layout), "newName"?: string (default: "<layout> copy")}"#),
        C::new("layout.page", "Page Size", "Publish › Page Setup", page).params(
            r#"{"layout"?, "page"?: page id, "orientation"?: "landscape"|"portrait", "width"?: mm, "height"?: mm}"#,
        ),
        C::new("layout.addFrame", "Map Frame", "Publish › Map Frames", add_frame).params(
            r#"{"layout"?, "shape"?: "rectangle"|"circle"|"ellipse"|"polygon"|"lasso", "points"?: [[u, v], …] (polygon/lasso, 0..1 across the frame), "map"?: name, "x"?, "y"?, "w"?, "h"?: mm}"#,
        ),
        C::new("layout.addElement", "Add Element", "Publish › Map Surrounds", add_element).params(
            r##"{"layout"?, "kind": "northArrow"|"scaleBar"|"legend"|"text"|"dynamicText"|"picture"|"table"|"chart"|"rectangle"|"extentIndicator"|"grid", "frame"?: element id (default: the first map frame), "of"?: frame id (extentIndicator), "text"?, "size"?, "color"?: "#RRGGBB"|"#RRGGBBAA", "bold"?: bool (text), "title"?: legend title (default "Legend"), "allClasses"?: bool (legend: every class, not only those the frame draws), "hideOther"?: bool (legend: leave out <all other values>), "fill"?: "#RRGGBBAA" (rectangle), "width"?: outline width (rectangle), "image"?: base64, "path"?: picture file, "layer"?, "field"?, "label"?, "rows"?, "step"?: degrees, "labels"?: bool, "off"?: bool (grid), "x"?, "y"?, "w"?, "h"?: mm}"##,
        ),
        C::new("layout.update", "Element Properties", "Publish › Element", update).params(
            r##"{"layout"?, "element": id, "x"?, "y"?, "w"?, "h"?: mm, "name"?, "visible"?, "text"?, "size"?, "color"?, "bold"?, "title"?, "allClasses"?: bool (legend), "hideOther"?: bool (legend), "map"?, "scale"?, "center"?: [lon, lat], "layer"?, "field"?, "rows"?}"##,
        ),
        C::new("layout.removeElement", "Delete Element", "Publish › Element", |s, p| {
            let id = element_id(p)?;
            let l = active(s, p)?;
            if l.remove(id) { Ok(Value::Null) } else { Err(format!("no element {id}")) }
        })
        .params(r#"{"layout"?, "element": id}"#),
        C::new("layout.frameZoom", "Zoom Frame", "Publish › Map Frame", frame_zoom).params(
            r#"{"layout"?, "element"?: frame id, "frame"?: (as element), "to"?: "view" (the map view's extent, the default)|"full"|"selection"|"layer", "layer"?}"#,
        ),
        C::new("layout.series", "Map Series", "Publish › Map Frames", series).params(
            r#"{"layout"?, "layer": index layer, "nameField"?: string, "frame"?: id, "margin"?: 0..1, "off"?: bool}"#,
        ),
        C::new("layout.seriesPage", "Map Series Page", "Publish › Map Frames", series_page).params(r#"{"layout"?, "page": number|"next"|"previous"}"#),
        C::new("layout.seriesRefresh", "Refresh Map Series", "Publish › Map Frames", |s, p| series_page(s, &json!({"layout": p.get("layout"), "page": "current"})))
            .params(r#"{"layout"?}"#),
        C::new("layout.export", "Export Layout", "Publish › Output", export)
            .view()
            .params(r#"{"layout"?, "path": "*.png|*.pdf", "dpi"?: 72..600, "allPages"?: bool (default true: every map series page, up to 500; false: the current page; PDF)}"#),
        C::new("layout.save", "Save Layout File", "Publish › Save As", |s, p| {
            let path = ps(p, "path").ok_or("path is required")?;
            let l = active(s, p)?.clone();
            let text = serde_json::to_string_pretty(&l).map_err(|e| e.to_string())?;
            rustgis_data::io::write_atomic(Path::new(path), text.as_bytes()).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({"path": path}))
        })
        .view()
        .params(r#"{"layout"?, "path": "*.rgislayout"}"#),
        C::new("layout.import", "Import Layout", "Data › Project", |s, p| {
            let text = match (ps(p, "text"), ps(p, "path")) {
                (Some(t), _) => t.to_string(),
                (None, Some(path)) => {
                    use std::io::Read;
                    // Read at most one byte past the limit, so a file growing meanwhile is still refused.
                    let mut text = String::new();
                    std::fs::File::open(path)
                        .and_then(|f| f.take(MAX_LAYOUT + 1).read_to_string(&mut text))
                        .map_err(|e| format!("{path}: {e}"))?;
                    if text.len() as u64 > MAX_LAYOUT {
                        return Err("the layout file is too large".into());
                    }
                    text
                }
                (None, None) => return Err("path or text is required".into()),
            };
            import(s, &text)
        })
        .params(r#"{"path"?: "*.rgislayout", "text"?: layout JSON}"#),
    ]
}

/// The layout named or numbered by `layout`, else the active one, else the only one.
fn index(s: &Session, p: &Value) -> Result<usize, String> {
    let ls = &s.project.layouts;
    // A `layout` that is neither a name nor an index is an error, never the open layout.
    match p.get("layout") {
        None | Some(Value::Null) => {}
        Some(Value::String(n)) => {
            return ls.iter().position(|l| l.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no layout '{n}'"));
        }
        Some(_) => {
            let i = super::uint(p, "layout").ok_or("layout must be a layout's name or index")?;
            return usize::try_from(i).ok().filter(|i| *i < ls.len()).ok_or_else(|| format!("no layout {i}"));
        }
    }
    s.active_layout
        .filter(|i| *i < ls.len())
        .or(if ls.len() == 1 { Some(0) } else { None })
        .ok_or_else(|| "open a layout first".to_string())
}

/// Parameters where `name` names the layout (Open, Delete and Rename Layout) as `layout`.
fn by_name(p: &Value) -> Value {
    match (ps(p, "name"), p.get("layout")) {
        (Some(n), None) => json!({"layout": n}),
        _ => p.clone(),
    }
}

fn active<'a>(s: &'a mut Session, p: &Value) -> Result<&'a mut Layout, String> {
    let i = index(s, p)?;
    s.project.layouts.get_mut(i).ok_or_else(|| "open a layout first".to_string())
}

fn element_id(p: &Value) -> Result<u32, String> {
    p.get("element").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| "element is required".to_string())
}

fn unique_name(s: &Session, base: &str) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "Layout" } else { base };
    numbered(base, |n| s.project.layouts.iter().any(|l| l.name.eq_ignore_ascii_case(n)))
}

/// (width, height) in mm from page/orientation/width/height parameters.
pub(crate) fn page_size(p: &Value, current: Option<(f64, f64)>) -> Result<(f64, f64), String> {
    let (mut w, mut h) = match ps(p, "page") {
        Some(id) => PAGES
            .iter()
            .find(|(pid, label, ..)| pid.eq_ignore_ascii_case(id) || label.eq_ignore_ascii_case(id))
            .map(|(_, _, w, h)| (*w, *h))
            .ok_or_else(|| format!("unknown page size '{id}'"))?,
        None => current.unwrap_or((215.9, 279.4)),
    };
    if let Some(v) = f(p, "width") {
        w = v;
    }
    if let Some(v) = f(p, "height") {
        h = v;
    }
    match ps(p, "orientation") {
        Some("landscape") if h > w => std::mem::swap(&mut w, &mut h),
        Some("portrait") if w > h => std::mem::swap(&mut w, &mut h),
        Some("landscape" | "portrait") | None => {}
        Some(o) => return Err(format!("unknown orientation '{o}'")),
    }
    if !(20.0..=5000.0).contains(&w) || !(20.0..=5000.0).contains(&h) {
        return Err("pages are 20 to 5000 mm on a side".into());
    }
    Ok((w, h))
}

/// The active map's view as a frame centre and scale for a frame `w_mm` wide.
fn view_frame(s: &Session, w_mm: f64) -> (Coord, f64) {
    let m = s.map();
    let ground_w = s.view_size.0 * m.ground_res();
    (m.center_lonlat(), (ground_w / (w_mm / 1000.0)).clamp(100.0, 1e10))
}

fn frame_kind(s: &Session, map: &str, w: f64, shape: FrameShape) -> ElementKind {
    let (center, scale) = view_frame(s, w);
    ElementKind::MapFrame { map: map.to_string(), center, scale, shape, grid: None, extent_of: None }
}

pub(crate) fn new_layout(s: &mut Session, p: &Value) -> CmdResult {
    let (w, h) = page_size(
        &json!({"page": ps(p, "page").unwrap_or("letter"), "orientation": ps(p, "orientation").unwrap_or("landscape"), "width": p.get("width"), "height": p.get("height")}),
        None,
    )?;
    let name = unique_name(s, ps(p, "name").unwrap_or("Layout"));
    let mut l = Layout::new(name.clone(), w, h);
    match ps(p, "template").unwrap_or("map") {
        "blank" => {}
        "map" => {
            let map = s.map().name.clone();
            let m = 10.0;
            let fw = (w - 2.0 * m) * 0.72;
            let fh = h - 2.0 * m - 18.0;
            let frame = l.add("Map Frame", (m, m + 16.0, fw, fh), frame_kind(s, &map, fw, FrameShape::Rectangle));
            let side = m + fw + 6.0;
            let sw = w - side - m;
            l.add(
                "Title",
                (m, m, w - 2.0 * m, 14.0),
                ElementKind::Text { text: map.clone(), size: 22.0, color: Rgba::rgb(0x1F, 0x29, 0x37), bold: true },
            );
            l.add(
                "Legend",
                (side, m + 16.0, sw, fh * 0.6),
                ElementKind::Legend { frame, title: "Legend".into(), all_classes: false, hide_other: false },
            );
            l.add("North Arrow", (side + sw / 2.0 - 8.0, m + 16.0 + fh * 0.62, 16.0, 24.0), ElementKind::NorthArrow { frame });
            l.add("Scale Bar", (side, m + 16.0 + fh * 0.62 + 28.0, sw, 12.0), ElementKind::ScaleBar { frame });
            l.add(
                "Credits",
                (side, h - m - 10.0, sw, 10.0),
                ElementKind::Text { text: "{scale} · {date}".into(), size: 8.0, color: Rgba::rgb(0x55, 0x5B, 0x66), bold: false },
            );
        }
        t => return Err(format!("unknown template '{t}'")),
    }
    s.project.layouts.push(l);
    let i = s.project.layouts.len() - 1;
    s.active_layout = Some(i);
    Ok(json!({"layout": i, "name": name}))
}

fn page(s: &mut Session, p: &Value) -> CmdResult {
    let i = index(s, p)?;
    let cur = s.project.layouts.get(i).map(|l| (l.width, l.height));
    let (w, h) = page_size(p, cur)?;
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    l.width = w;
    l.height = h;
    Ok(json!({"width": w, "height": h}))
}

/// The element's box: given values, else a default inside the page.
fn place(p: &Value, l: &Layout, (dx, dy, dw, dh): (f64, f64, f64, f64)) -> Result<(f64, f64, f64, f64), String> {
    let v = |k: &str, d: f64| f(p, k).unwrap_or(d);
    let (x, y, w, h) = (v("x", dx), v("y", dy), v("w", dw.min(l.width - 10.0).max(5.0)), v("h", dh.min(l.height - 10.0).max(5.0)));
    if w <= 0.0 || h <= 0.0 || w > 5000.0 || h > 5000.0 || x.abs() > 5000.0 || y.abs() > 5000.0 {
        return Err("element sizes are more than 0 and up to 5000 mm".into());
    }
    Ok((x, y, w, h))
}

fn add_frame(s: &mut Session, p: &Value) -> CmdResult {
    let shape = match ps(p, "shape").unwrap_or("rectangle") {
        "rectangle" => FrameShape::Rectangle,
        "circle" => FrameShape::Circle,
        "ellipse" => FrameShape::Ellipse,
        "polygon" | "lasso" => {
            let pts: Vec<(f64, f64)> = p
                .get("points")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .take(10_000)
                        .filter_map(|v| Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)))
                        .filter(|(u, v)| u.is_finite() && v.is_finite())
                        .map(|(u, v)| (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)))
                        .collect()
                })
                .unwrap_or_else(|| vec![(0.5, 0.0), (1.0, 0.38), (0.81, 1.0), (0.19, 1.0), (0.0, 0.38)]);
            if pts.len() < 3 {
                return Err("a polygon frame needs at least three points".into());
            }
            FrameShape::Polygon(pts)
        }
        o => return Err(format!("unknown frame shape '{o}'")),
    };
    let map = match ps(p, "map") {
        Some(n) => {
            s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(n)).map(|m| m.name.clone()).ok_or_else(|| format!("no map '{n}'"))?
        }
        None => s.map().name.clone(),
    };
    let i = index(s, p)?;
    let l = s.project.layouts.get(i).ok_or("no layout")?;
    let n = l.elements.len() as f64;
    let rect = place(p, l, (15.0 + n * 5.0, 25.0 + n * 5.0, l.width * 0.5, l.height * 0.5))?;
    let kind = frame_kind(s, &map, rect.2, shape);
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    let id = l.add("Map Frame", rect, kind);
    Ok(json!({"element": id}))
}

fn frame_of(l: &Layout, p: &Value) -> Result<u32, String> {
    match p.get("frame").and_then(Value::as_u64) {
        Some(id) => {
            let id = u32::try_from(id).map_err(|_| "no such frame")?;
            match l.element(id).map(|e| &e.kind) {
                Some(ElementKind::MapFrame { .. }) => Ok(id),
                _ => Err(format!("element {id} isn't a map frame")),
            }
        }
        None => l.first_frame().ok_or_else(|| "add a map frame first".to_string()),
    }
}

pub(crate) fn color(p: &Value, k: &str, d: Rgba) -> Result<Rgba, String> {
    match ps(p, k) {
        Some(c) => Rgba::from_hex(c).ok_or_else(|| format!("{k} must be #RRGGBB or #RRGGBBAA")),
        None => Ok(d),
    }
}

fn add_element(s: &mut Session, p: &Value) -> CmdResult {
    let kind = ps(p, "kind").ok_or("kind is required")?;
    let i = index(s, p)?;
    let image = if kind == "picture" { Some(super::graphics::picture(p)?) } else { None };
    let layer_name = |s: &Session, l: &Layout, frame: u32| -> Result<String, String> {
        let map = match l.element(frame).map(|e| &e.kind) {
            Some(ElementKind::MapFrame { map, .. }) => map.clone(),
            _ => return Err("no such frame".into()),
        };
        let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
        let found = match ps(p, "layer") {
            Some(n) => m.layer_by_name(n),
            None => m.layers.iter().find(|l| !l.is_raster() && !l.graphics),
        };
        found.map(|l| l.name.clone()).ok_or_else(|| "no such layer in the frame's map".to_string())
    };
    let l = s.project.layouts.get(i).ok_or("no layout")?;
    let (pw, ph) = (l.width, l.height);
    let (name, rect, k) = match kind {
        "northArrow" => {
            let frame = frame_of(l, p)?;
            ("North Arrow", place(p, l, (pw - 30.0, 15.0, 16.0, 24.0))?, ElementKind::NorthArrow { frame })
        }
        "scaleBar" => {
            let frame = frame_of(l, p)?;
            ("Scale Bar", place(p, l, (15.0, ph - 25.0, 70.0, 12.0))?, ElementKind::ScaleBar { frame })
        }
        "legend" => {
            let frame = frame_of(l, p)?;
            let title = ps(p, "title").unwrap_or("Legend").chars().take(200).collect();
            let all_classes = b(p, "allClasses").unwrap_or(false);
            let hide_other = b(p, "hideOther").unwrap_or(false);
            ("Legend", place(p, l, (pw - 70.0, 45.0, 60.0, 80.0))?, ElementKind::Legend { frame, title, all_classes, hide_other })
        }
        "text" | "dynamicText" => {
            let default = if kind == "text" { "Text" } else { "{map} · {scale} · {date}" };
            let text: String = ps(p, "text").unwrap_or(default).chars().take(5000).collect();
            let size = f(p, "size").unwrap_or(12.0).clamp(2.0, 400.0);
            let k = ElementKind::Text {
                text,
                size,
                color: color(p, "color", Rgba::rgb(0x1F, 0x29, 0x37))?,
                bold: b(p, "bold").unwrap_or(false),
            };
            (if kind == "text" { "Text" } else { "Dynamic Text" }, place(p, l, (15.0, 12.0, 120.0, (size * 0.5).max(8.0)))?, k)
        }
        "picture" => {
            ("Picture", place(p, l, (pw - 60.0, ph - 50.0, 45.0, 35.0))?, ElementKind::Picture { image: image.unwrap_or_default() })
        }
        "table" => {
            let frame = frame_of(l, p)?;
            let layer = layer_name(s, l, frame)?;
            let rows = p.get("rows").and_then(Value::as_u64).unwrap_or(12).clamp(1, 500) as usize;
            ("Table Frame", place(p, l, (15.0, ph * 0.6, pw * 0.5, ph * 0.35))?, ElementKind::Table { frame, layer, rows })
        }
        "chart" => {
            let frame = frame_of(l, p)?;
            let layer = layer_name(s, l, frame)?;
            let m = s.project.maps.iter().find_map(|m| m.layer_by_name(&layer)).ok_or("no such layer")?;
            let field = match ps(p, "field") {
                Some(fl) => m
                    .dataset
                    .field_index(fl)
                    .and_then(|i| m.dataset.fields.get(i))
                    .map(|f| f.name.clone())
                    .ok_or_else(|| format!("no field '{fl}'"))?,
                None => {
                    m.dataset.fields.iter().find(|f| f.ty.is_numeric()).map(|f| f.name.clone()).ok_or("the layer has no numeric field")?
                }
            };
            let label = ps(p, "label").and_then(|n| m.dataset.field_index(n)).and_then(|i| m.dataset.fields.get(i)).map(|f| f.name.clone());
            ("Chart Frame", place(p, l, (pw * 0.55, ph * 0.6, pw * 0.4, ph * 0.35))?, ElementKind::Chart { frame, layer, field, label })
        }
        "rectangle" => {
            let k = ElementKind::Rectangle {
                fill: color(p, "fill", Rgba::new(0, 0, 0, 0))?,
                outline: color(p, "color", Rgba::rgb(0x22, 0x22, 0x22))?,
                width: f(p, "width").unwrap_or(1.0).clamp(0.0, 50.0),
            };
            ("Neatline", place(p, l, (5.0, 5.0, pw - 10.0, ph - 10.0))?, k)
        }
        "extentIndicator" => {
            let frame = frame_of(l, p)?;
            let of =
                p.get("of").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or("of (the frame to outline) is required")?;
            if of == frame || !matches!(l.element(of).map(|e| &e.kind), Some(ElementKind::MapFrame { .. })) {
                return Err("of must be another map frame".into());
            }
            let l = active(s, p)?;
            if let Some(ElementKind::MapFrame { extent_of, .. }) = l.element_mut(frame).map(|e| &mut e.kind) {
                *extent_of = Some(of);
            }
            return Ok(json!({"element": frame}));
        }
        "grid" => {
            let frame = frame_of(l, p)?;
            let step = f(p, "step").unwrap_or(0.0).clamp(0.0, 90.0);
            let c = color(p, "color", Rgba::new(0x40, 0x48, 0x55, 150))?;
            let l = active(s, p)?;
            if let Some(ElementKind::MapFrame { grid, .. }) = l.element_mut(frame).map(|e| &mut e.kind) {
                *grid =
                    if b(p, "off").unwrap_or(false) { None } else { Some(Grid { step, color: c, labels: b(p, "labels").unwrap_or(true) }) };
            }
            return Ok(json!({"element": frame}));
        }
        o => return Err(format!("unknown element kind '{o}'")),
    };
    let l = s.project.layouts.get_mut(i).ok_or("no layout")?;
    let id = l.add(name, rect, k);
    Ok(json!({"element": id}))
}

fn update(s: &mut Session, p: &Value) -> CmdResult {
    let id = element_id(p)?;
    // Checked before anything changes, so a bad value leaves the element as it was.
    let new_center = match p.get("center") {
        Some(c) => {
            let a = c.as_array().ok_or("center is [lon, lat]")?;
            match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(Coord::new(x.clamp(-540.0, 540.0), y.clamp(-85.0, 85.0))),
                _ => return Err("center is [lon, lat]".into()),
            }
        }
        None => None,
    };
    // A new layer (tables and charts) or field (charts) must exist in the frame's map.
    let li = index(s, p)?;
    let checked = {
        let l = s.project.layouts.get(li).ok_or("no layout")?;
        let e = l.element(id).ok_or_else(|| format!("no element {id}"))?;
        match (&e.kind, ps(p, "layer"), ps(p, "field")) {
            (ElementKind::Table { frame, layer, .. } | ElementKind::Chart { frame, layer, .. }, nl, nf) if nl.is_some() || nf.is_some() => {
                let map = match l.element(*frame).map(|f| &f.kind) {
                    Some(ElementKind::MapFrame { map, .. }) => map.clone(),
                    _ => return Err("the element's map frame is gone".into()),
                };
                let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
                let target = m
                    .layer_by_name(nl.unwrap_or(layer))
                    .filter(|l| !l.is_raster())
                    .ok_or_else(|| format!("no feature layer '{}' in {map}", nl.unwrap_or(layer)))?;
                // Only charts have a field.
                let field = match nf.filter(|_| matches!(e.kind, ElementKind::Chart { .. })) {
                    Some(fl) => Some(
                        target
                            .dataset
                            .field_index(fl)
                            .and_then(|i| target.dataset.fields.get(i))
                            .filter(|f| f.ty.is_numeric())
                            .map(|f| f.name.clone())
                            .ok_or_else(|| format!("no numeric field '{fl}' in {}", target.name))?,
                    ),
                    // A chart moved to another layer keeps its field only if that layer has it as a
                    // number; otherwise it charts the layer's first numeric field.
                    None => match &e.kind {
                        ElementKind::Chart { field, .. } if nl.is_some() => {
                            let numeric = |f: &&rustgis_data::Field| f.ty.is_numeric();
                            let keep = target.dataset.field_index(field).and_then(|i| target.dataset.fields.get(i)).filter(numeric);
                            let f = keep
                                .or_else(|| target.dataset.fields.iter().find(numeric))
                                .ok_or_else(|| format!("{} has no numeric field to chart", target.name))?;
                            Some(f.name.clone())
                        }
                        _ => None,
                    },
                };
                Some((target.name.clone(), field))
            }
            _ => None,
        }
    };
    let map_name = match ps(p, "map") {
        Some(n) => Some(
            s.project
                .maps
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(n))
                .map(|m| m.name.clone())
                .ok_or_else(|| format!("no map '{n}'"))?,
        ),
        None => None,
    };
    let col = match ps(p, "color") {
        Some(c) => Some(Rgba::from_hex(c).ok_or("color must be #RRGGBB or #RRGGBBAA")?),
        None => None,
    };
    let l = active(s, p)?;
    let e = l.element_mut(id).ok_or_else(|| format!("no element {id}"))?;
    let num = |k: &str, lo: f64, hi: f64| f(p, k).map(|v| v.clamp(lo, hi));
    if let Some(v) = num("x", -5000.0, 5000.0) {
        e.x = v;
    }
    if let Some(v) = num("y", -5000.0, 5000.0) {
        e.y = v;
    }
    if let Some(v) = num("w", MIN_ELEMENT_MM, 5000.0) {
        e.w = v;
    }
    if let Some(v) = num("h", MIN_ELEMENT_MM, 5000.0) {
        e.h = v;
    }
    if let Some(n) = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        e.name = n.chars().take(200).collect();
    }
    if let Some(v) = b(p, "visible") {
        e.visible = v;
    }
    match &mut e.kind {
        ElementKind::Text { text, size, color, bold } => {
            if let Some(t) = ps(p, "text") {
                *text = t.chars().take(5000).collect();
            }
            if let Some(v) = num("size", 2.0, 400.0) {
                *size = v;
            }
            if let Some(c) = col {
                *color = c;
            }
            if let Some(v) = b(p, "bold") {
                *bold = v;
            }
        }
        ElementKind::Legend { title, all_classes, hide_other, .. } => {
            if let Some(t) = ps(p, "title") {
                *title = t.chars().take(200).collect();
            }
            if let Some(v) = b(p, "allClasses") {
                *all_classes = v;
            }
            if let Some(v) = b(p, "hideOther") {
                *hide_other = v;
            }
        }
        ElementKind::MapFrame { map, center, scale, .. } => {
            if let Some(m) = map_name {
                *map = m;
            }
            if let Some(v) = num("scale", 100.0, 1e10) {
                *scale = v;
            }
            if let Some(c) = new_center {
                *center = c;
            }
        }
        ElementKind::Table { rows, layer, .. } => {
            if let Some(v) = p.get("rows").and_then(Value::as_u64) {
                *rows = v.clamp(1, 500) as usize;
            }
            if let Some((l, _)) = &checked {
                *layer = l.clone();
            }
        }
        ElementKind::Chart { field, layer, .. } => {
            if let Some((l, f)) = &checked {
                *layer = l.clone();
                if let Some(f) = f {
                    *field = f.clone();
                }
            }
        }
        _ => {}
    }
    Ok(Value::Null)
}

/// Points a frame at an extent.
fn set_frame(s: &mut Session, li: usize, frame: u32, e: &Extent) -> Result<(), String> {
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let el = l.element(frame).ok_or("no such frame")?;
    let (w, h) = (el.w, el.h);
    let ElementKind::MapFrame { map, .. } = &el.kind else { return Err("that isn't a map frame".into()) };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)).ok_or_else(|| format!("no map '{map}'"))?;
    let (c, sc) = fit_scale(m, e, w, h).ok_or("nothing to zoom to")?;
    if let Some(ElementKind::MapFrame { center, scale, .. }) =
        s.project.layouts.get_mut(li).and_then(|l| l.element_mut(frame)).map(|e| &mut e.kind)
    {
        *center = c;
        *scale = sc;
    }
    Ok(())
}

fn frame_zoom(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let frame = match p.get("element") {
        Some(_) => frame_of(l, &json!({"frame": p.get("element")}))?,
        None => frame_of(l, p)?,
    };
    let map = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("no such frame".into()),
    };
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
    match ps(p, "to").unwrap_or("view") {
        "view" => {
            let w = l.element(frame).map(|e| e.w).unwrap_or(100.0);
            let ground_w = s.view_size.0 * m.ground_res();
            let (c, sc) = (m.center_lonlat(), (ground_w / (w / 1000.0)).clamp(100.0, 1e10));
            if let Some(ElementKind::MapFrame { center, scale, .. }) =
                s.project.layouts.get_mut(li).and_then(|l| l.element_mut(frame)).map(|e| &mut e.kind)
            {
                *center = c;
                *scale = sc;
            }
        }
        "full" => {
            let e = m.shown_extent();
            set_frame(s, li, frame, &e)?;
        }
        "selection" => {
            let e = m.selection_extent();
            set_frame(s, li, frame, &e)?;
        }
        "layer" => {
            let n = ps(p, "layer").ok_or("layer is required")?;
            let e = m.layer_by_name(n).ok_or_else(|| format!("no layer '{n}'"))?.shown_extent();
            set_frame(s, li, frame, &e)?;
        }
        o => return Err(format!("unknown zoom '{o}'")),
    }
    Ok(json!({"element": frame}))
}

fn series(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    if b(p, "off").unwrap_or(false) {
        if let Some(l) = s.project.layouts.get_mut(li) {
            l.series = None;
        }
        return Ok(Value::Null);
    }
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let frame = frame_of(l, p)?;
    let map = match l.element(frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("no such frame".into()),
    };
    let n = ps(p, "layer").ok_or("layer is required")?;
    let m = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(&map)).ok_or_else(|| format!("no map '{map}'"))?;
    let layer = m.layer_by_name(n).filter(|l| !l.is_raster()).ok_or_else(|| format!("no feature layer '{n}' in {map}"))?;
    if layer.dataset.features.is_empty() {
        return Err("the index layer has no features".into());
    }
    let name_field = match ps(p, "nameField") {
        Some(fl) => layer
            .dataset
            .field_index(fl)
            .and_then(|i| layer.dataset.fields.get(i))
            .map(|f| f.name.clone())
            .ok_or_else(|| format!("no field '{fl}'"))?,
        None => layer.labels.field.clone(),
    };
    let layer = layer.name.clone();
    let margin = f(p, "margin").unwrap_or(0.1).clamp(0.0, 5.0);
    if let Some(l) = s.project.layouts.get_mut(li) {
        l.series = Some(MapSeries { frame, layer, name_field, margin, page: 0 });
    }
    series_page(s, &json!({"layout": li, "page": 0}))
}

/// The pages of a layout's map series: (count, page name of each row).
pub fn series_pages(s: &Session, li: usize) -> Option<Vec<String>> {
    let (layer, fi) = series_layer(s, li)?;
    Some((0..layer.dataset.features.len()).map(|r| page_name(layer, fi, r)).collect())
}

/// A map series' page count and the name of `page`, without naming every page.
pub fn series_info(s: &Session, li: usize, page: usize) -> Option<(usize, String)> {
    let (layer, fi) = series_layer(s, li)?;
    let n = layer.dataset.features.len();
    Some((n, if page < n { page_name(layer, fi, page) } else { String::new() }))
}

/// A map series' index layer and its name field.
fn series_layer(s: &Session, li: usize) -> Option<(&rustgis_map::Layer, Option<usize>)> {
    let l = s.project.layouts.get(li)?;
    let sr = l.series.as_ref()?;
    let map = match l.element(sr.frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map,
        _ => return None,
    };
    let layer = s.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map))?.layer_by_name(&sr.layer)?;
    Some((layer, layer.dataset.field_index(&sr.name_field)))
}

fn page_name(layer: &rustgis_map::Layer, fi: Option<usize>, row: usize) -> String {
    fi.map(|i| layer.dataset.value(row, i).to_string()).unwrap_or_else(|| (row + 1).to_string())
}

fn series_page(s: &mut Session, p: &Value) -> CmdResult {
    let li = index(s, p)?;
    let (n, _) = series_info(s, li, 0).ok_or("the layout has no map series")?;
    let l = s.project.layouts.get(li).ok_or("no layout")?;
    let sr = l.series.clone().ok_or("the layout has no map series")?;
    let page = match p.get("page") {
        Some(Value::String(t)) if t == "next" => (sr.page + 1).min(n.saturating_sub(1)),
        Some(Value::String(t)) if t == "previous" => sr.page.saturating_sub(1),
        Some(Value::String(t)) if t == "current" => sr.page.min(n.saturating_sub(1)),
        Some(v) => v
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v < n)
            .ok_or_else(|| format!("page must be 0 to {}", n.saturating_sub(1)))?,
        None => return Err("page is required".into()),
    };
    // The page's feature extent, with the series margin.
    let map = match l.element(sr.frame).map(|e| &e.kind) {
        Some(ElementKind::MapFrame { map, .. }) => map.clone(),
        _ => return Err("the map series frame is gone".into()),
    };
    let layer = s
        .project
        .maps
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&map))
        .and_then(|m| m.layer_by_name(&sr.layer))
        .ok_or("the index layer is gone")?;
    // A feature without geometry (common in CSV and GeoJSON) leaves the frame where it was.
    let e = layer.dataset.features.get(page).and_then(|f| f.geometry.as_ref()).map(|g| g.extent()).filter(|e| !e.is_empty());
    if let Some(e) = e {
        set_frame(s, li, sr.frame, &e.padded(sr.margin, 0.01))?;
    }
    if let Some(sr) = s.project.layouts.get_mut(li).and_then(|l| l.series.as_mut()) {
        sr.page = page;
    }
    let name = series_info(s, li, page).map(|x| x.1).unwrap_or_default();
    Ok(json!({"page": page, "pages": n, "name": name}))
}

/// Renders a layout to PNG or PDF bytes. `all` renders every map series page (PDF only).
pub fn layout_bytes(s: &mut Session, li: usize, pdf: bool, dpi: f64, all: bool) -> Result<Vec<u8>, String> {
    let saved = s.project.layouts.get(li).ok_or("no layout")?.clone();
    let dpi = if dpi.is_finite() { dpi.clamp(36.0, 600.0) } else { 150.0 };
    let current = saved.series.as_ref().map_or(0, |sr| sr.page);
    // Only a whole-series PDF names every page.
    let info = series_info(s, li, current);
    let names = if all && pdf { series_pages(s, li) } else { None };
    let total = info.as_ref().map_or(1, |i| i.0);
    // Every page of a map series (PDF), or just the layout as it is now.
    let pages: Vec<usize> = match (&names, all && pdf) {
        (Some(n), true) => {
            if n.len() > 500 {
                s.notify(format!("The map series has {} pages; the PDF holds the first 500", n.len()), false);
            }
            (0..n.len().min(500)).collect()
        }
        _ => vec![current],
    };
    let stepping = names.is_some() && pages.len() > 1;
    let mut render_pages = || -> Result<(Vec<rustgis_render::pdf::Page>, Option<Vec<u8>>), String> {
        let mut out = Vec::new();
        let mut png = None;
        let mut held = 0usize;
        for page in &pages {
            if stepping {
                series_page(s, &json!({"layout": li, "page": page}))?;
            }
            let layout = s.project.layouts.get(li).ok_or("no layout")?;
            let info = rustgis_render::layout::PageInfo {
                page: page + 1,
                pages: total,
                page_name: match &names {
                    Some(n) => n.get(*page).cloned().unwrap_or_default(),
                    None => info.as_ref().map(|i| i.1.clone()).unwrap_or_default(),
                },
            };
            let img = rustgis_render::layout::render(layout, &mut s.project.maps, dpi, &info).ok_or("the page is too large to draw")?;
            // Each page is compressed as soon as it's drawn, within a memory budget.
            if pdf {
                out.push(rustgis_render::pdf::Page::new(&img, layout.width, layout.height));
                held += out.last().map_or(0, rustgis_render::pdf::Page::len);
                if held > PDF_BUDGET {
                    return Err(format!(
                        "the PDF passed {} MB at page {}; export at a lower dpi, or fewer pages",
                        PDF_BUDGET >> 20,
                        page + 1
                    ));
                }
            } else {
                png = Some(img.encode_png().map_err(|e| e.to_string())?);
            }
        }
        Ok((out, png))
    };
    let result = render_pages();
    // Stepping through the series moved the frame; put the layout back however that went.
    if stepping && let Some(cur) = s.project.layouts.get_mut(li) {
        *cur = saved.clone();
    }
    let (pages, png) = result?;
    if pdf { Ok(rustgis_render::pdf::write(&pages, &saved.name)) } else { png.ok_or_else(|| "nothing to draw".to_string()) }
}

fn export(s: &mut Session, p: &Value) -> CmdResult {
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    let li = index(s, p)?;
    let lower = path.to_ascii_lowercase();
    let pdf = lower.ends_with(".pdf");
    if !pdf && !lower.ends_with(".png") {
        return Err("export to a .png or .pdf file".into());
    }
    let bytes = layout_bytes(s, li, pdf, f(p, "dpi").unwrap_or(150.0), b(p, "allPages").unwrap_or(true))?;
    rustgis_data::io::write_atomic(Path::new(&path), &bytes).map_err(|e| format!("{path}: {e}"))?;
    s.notify(format!("Exported {path}"), false);
    Ok(json!({"path": path, "bytes": bytes.len()}))
}

pub(crate) fn import(s: &mut Session, text: &str) -> CmdResult {
    if text.len() as u64 > MAX_LAYOUT {
        return Err("the layout file is too large".into());
    }
    let mut l: Layout = serde_json::from_str(text).map_err(|e| format!("not a RustGIS layout: {e}"))?;
    l.clamp();
    l.name = unique_name(s, &l.name);
    // Frames showing maps this project lacks show the active map instead.
    let fallback = s.map().name.clone();
    let names: Vec<String> = s.project.maps.iter().map(|m| m.name.to_ascii_lowercase()).collect();
    for e in &mut l.elements {
        if let ElementKind::MapFrame { map, .. } = &mut e.kind
            && !names.contains(&map.to_ascii_lowercase())
        {
            *map = fallback.clone();
        }
    }
    let name = l.name.clone();
    s.project.layouts.push(l);
    let i = s.project.layouts.len() - 1;
    s.active_layout = Some(i);
    Ok(json!({"layout": i, "name": name}))
}
