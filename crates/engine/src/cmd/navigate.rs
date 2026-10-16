//! Home › Navigate, Add (basemap) and Data › Project (new map), bookmarks.

use rustgis_geom::{Coord, Extent};
use rustgis_map::layout::ElementKind;
use rustgis_map::presentation::PageKind;
use rustgis_map::{Basemap, Bookmark, Map, Projection};
use serde_json::{Value, json};

use super::{f, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("map.zoomFull", "Full Extent", "Home › Navigate", zoom_full).view(),
        C::new("map.zoomIn", "Fixed Zoom In", "Home › Navigate", |s, _| zoom(s, 2.0)).view().key("Mod+="),
        C::new("map.zoomOut", "Fixed Zoom Out", "Home › Navigate", |s, _| zoom(s, 0.5)).view().key("Mod+-"),
        C::new("map.prevExtent", "Previous Extent", "Home › Navigate", |s, _| {
            if s.extent_back() { Ok(Value::Null) } else { Err("no previous extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_back() { None } else { Some("no previous extent") }),
        C::new("map.nextExtent", "Next Extent", "Home › Navigate", |s, _| {
            if s.extent_forward() { Ok(Value::Null) } else { Err("no next extent".into()) }
        })
        .view()
        .when(|s| if s.can_extent_forward() { None } else { Some("no next extent") }),
        C::new("map.zoomWorld", "Zoom To World", "Home › Navigate", |s, _| {
            s.zoom_to(&Extent::new(-170.0, -58.0, 180.0, 78.0));
            Ok(Value::Null)
        })
        .view(),
        C::new("map.extent", "View Extent", "Home › Navigate", |s, _| {
            let e = if s.map().projection.is_globe() { Extent::new(-180.0, -90.0, 180.0, 90.0) } else { super::workspace::view_extent(s) };
            if e.is_empty() {
                return Err("the view shows no part of the Earth".into());
            }
            Ok(json!({"extent": [e.min_x, e.min_y, e.max_x, e.max_y]}))
        })
        .view(),
        C::new("map.zoomToExtent", "Zoom To Extent", "Home › Navigate", zoom_to_extent)
            .view()
            .params(r#"{"extent": [minLon, minLat, maxLon, maxLat]}"#),
        C::new("map.goToXY", "Go To XY", "Home › Navigate", go_to_xy)
            .view()
            .params(r#"{"lon" | "x": number, "lat" | "y": number, "scale"?: number}"#),
        C::new("map.setScale", "Set Scale", "Status Bar", |s, p| {
            let sc = f(p, "scale").ok_or("scale is required")?;
            s.map_mut().set_scale(sc);
            s.record_extent();
            Ok(Value::Null)
        })
        .view()
        .params(r#"{"scale": number}"#),
        C::new("map.basemap", "Basemap", "Home › Add", basemap)
            .params(r#"{"name": "paper"|"midnight"|"ocean"|"atlas"|"blank", "keepTiles"?: bool (keeps the web tile basemaps over it)}"#),
        C::new("map.graticule", "Graticule", "View › Display", |s, p| {
            let m = s.map_mut();
            m.graticule = super::b(p, "on").unwrap_or(!m.graticule);
            Ok(Value::Null)
        })
        .params(r#"{"on"?: bool (toggles when omitted)}"#),
        C::new("map.projection", "Globe", "View › View", |s, p| {
            let m = s.map_mut();
            let next = match ps(p, "type") {
                Some(t) => Projection::from_code(t)?,
                None if m.projection.is_globe() => Projection::WebMercator,
                None => Projection::Globe,
            };
            m.set_projection(next);
            let out = json!({"projection": m.projection.code(), "label": m.projection.label()});
            if s.auto_cache {
                super::workspace::fill_cache(s);
            }
            Ok(out)
        })
        .params(r#"{"type"?: "mercator"|"globe"|preset id|"EPSG:<code>" (toggles the globe when omitted)}"#),
        C::new("map.projections", "Projections", "View › View", |_, _| {
            let mut v = vec![json!({"id": "mercator", "label": "Web Mercator"}), json!({"id": "globe", "label": "Globe"})];
            v.extend(rustgis_map::projection::PRESETS.iter().map(|(id, label, _)| json!({"id": id, "label": label})));
            Ok(Value::Array(v))
        })
        .view(),
        C::new("bookmark.add", "New Bookmark", "Home › Navigate › Bookmarks", add_bookmark).params(r#"{"name"?: string}"#),
        C::new("bookmark.go", "Go To Bookmark", "Home › Navigate › Bookmarks", go_bookmark).view().params(r#"{"name": string}"#),
        C::new("bookmark.remove", "Remove Bookmark", "Home › Navigate › Bookmarks", |s, p| {
            let n = s_name(p)?;
            let m = s.map_mut();
            let before = m.bookmarks.len();
            m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(n));
            if m.bookmarks.len() == before { Err(format!("no bookmark '{n}'")) } else { Ok(Value::Null) }
        })
        .params(r#"{"name": string}"#),
        C::new("bookmark.save", "Export Bookmarks", "Home › Navigate › Bookmarks", |s, p| super::items::save_item(s, "bookmarks", p))
            .view()
            .params(r#"{"path": "*.rgisbookmarks"}"#),
        C::new("bookmark.import", "Import Bookmarks", "Home › Navigate › Bookmarks", import_bookmarks)
            .params(r#"{"path"?: "*.rgisbookmarks", "text"?: bookmarks JSON, "data"?: base64 file}"#),
        C::new("map.new", "New Map", "Data › Project", new_map).params(r#"{"name"?: string}"#),
        C::new("map.activate", "Activate Map", "Browser", |s, p| {
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.active_map = i;
            s.active_layer = s.map().layers.first().map(|l| l.id);
            s.table_layer = None;
            Ok(Value::Null)
        })
        .params(r#"{"name": string}"#)
        .view(),
        C::new("map.rename", "Rename Map", "Layers", |s, p| {
            let n = s_name(p)?.trim();
            if n.is_empty() {
                return Err("name is required".into());
            }
            let i = map_at(s, p)?;
            let old = s.project.maps.get(i).map(|m| m.name.clone()).ok_or("no map")?;
            // Unique among the other maps, so a name finds one map (tabs, Browser, map frames).
            let name = super::numbered(n, |n| s.project.maps.iter().enumerate().any(|(j, m)| j != i && m.name.eq_ignore_ascii_case(n)));
            if let Some(m) = s.project.maps.get_mut(i) {
                m.name = name.clone();
            }
            follow_renamed_map(s, &old, &name);
            Ok(json!({"name": name}))
        })
        .params(r#"{"name": string (the new name; a number is added when another map has it), "map"?: name (default: the active map)}"#),
        C::new("map.duplicate", "Duplicate Map", "Browser", duplicate_map)
            .params(r#"{"map"?: name (default: the active map), "name"?: string (default: "<map> copy"; a number is added when another map has it)}"#),
        C::new("map.remove", "Delete Map", "Browser", |s, p| {
            if s.project.maps.len() < 2 {
                return Err("a project keeps at least one map".into());
            }
            let n = s_name(p)?;
            let i = s.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("no map '{n}'"))?;
            s.project.maps.remove(i);
            // Deleting another map keeps the one in view (a map tab's close button).
            let active = s.project.active_map;
            if i < active {
                s.project.active_map = active - 1;
            } else if i == active {
                s.project.active_map = 0;
                s.active_layer = s.map().layers.first().map(|l| l.id);
                s.table_layer = None;
            }
            Ok(Value::Null)
        })
        .params(r#"{"name": string}"#),
    ]
}

fn s_name(p: &Value) -> Result<&str, String> {
    ps(p, "name").ok_or_else(|| "name is required".to_string())
}

/// The map `map` names (default: the active one). A `map` that isn't a name is an error, never
/// the active map.
fn map_at(s: &Session, p: &Value) -> Result<usize, String> {
    match p.get("map") {
        None | Some(Value::Null) => Ok(s.project.active_map),
        Some(Value::String(m)) => s.project.maps.iter().position(|x| x.name.eq_ignore_ascii_case(m)).ok_or_else(|| format!("no map '{m}'")),
        Some(_) => Err("map must be a map's name".into()),
    }
}

/// A full copy of a map (#288), its data included, right after it and in view. Layer ids are per
/// map, so the copy keeps them; layouts aren't copied and keep showing the original.
fn duplicate_map(s: &mut Session, p: &Value) -> CmdResult {
    let i = map_at(s, p)?;
    let mut m = s.project.maps.get(i).ok_or("no map")?.clone();
    m.name = super::copy_name(ps(p, "name"), &m.name, |n| s.project.maps.iter().any(|x| x.name.eq_ignore_ascii_case(n)));
    m.label_lock = None;
    for l in &mut m.layers {
        l.selection.clear();
    }
    let name = m.name.clone();
    let was_active = i == s.project.active_map;
    s.project.maps.insert(i + 1, m);
    s.project.active_map = i + 1;
    // The copy of the map in view keeps its active layer (same ids); another starts on its first.
    if !was_active {
        s.active_layer = s.map().layers.first().map(|l| l.id);
        s.table_layer = None;
    }
    Ok(json!({"name": name}))
}

/// Points layout map frames, presentation map pages and offline areas at a renamed map (#287),
/// so they keep finding it.
fn follow_renamed_map(s: &mut Session, old: &str, new: &str) {
    let renamed = |m: &mut String| {
        if m.eq_ignore_ascii_case(old) {
            *m = new.to_string();
        }
    };
    for e in s.project.layouts.iter_mut().flat_map(|l| l.elements.iter_mut()) {
        if let ElementKind::MapFrame { map, .. } = &mut e.kind {
            renamed(map);
        }
    }
    for page in s.project.presentations.iter_mut().flat_map(|p| p.pages.iter_mut()) {
        if let PageKind::Map { map, .. } = &mut page.kind {
            renamed(map);
        }
    }
    for a in &mut s.project.offline_areas {
        renamed(&mut a.map);
    }
}

fn zoom_full(s: &mut Session, _: &Value) -> CmdResult {
    let e = s.map().shown_extent();
    let e = if e.is_empty() { Extent::new(-170.0, -58.0, 180.0, 78.0) } else { e };
    s.zoom_to(&e);
    Ok(Value::Null)
}

fn zoom_to_extent(s: &mut Session, p: &Value) -> CmdResult {
    let e = lonlat_extent(p.get("extent")).ok_or("extent is [minLon, minLat, maxLon, maxLat], west before east and south before north")?;
    s.zoom_to(&e);
    Ok(Value::Null)
}

/// A longitude/latitude extent `[minLon, minLat, maxLon, maxLat]` within ±180/±90, not empty.
pub fn lonlat_extent(v: Option<&Value>) -> Option<Extent> {
    let n: Vec<f64> = v?.as_array()?.iter().map(Value::as_f64).collect::<Option<_>>()?;
    let [x0, y0, x1, y1] = n.as_slice() else { return None };
    let ok = |x: f64, lim: f64| x.is_finite() && (-lim..=lim).contains(&x);
    (ok(*x0, 180.0) && ok(*x1, 180.0) && ok(*y0, 90.0) && ok(*y1, 90.0) && x0 < x1 && y0 < y1).then(|| Extent::new(*x0, *y0, *x1, *y1))
}

fn zoom(s: &mut Session, factor: f64) -> CmdResult {
    let (w, h) = s.view_size;
    s.map_mut().camera.zoom_at(factor, w / 2.0, h / 2.0, w, h);
    s.record_extent();
    Ok(Value::Null)
}

pub(crate) fn go_to_xy(s: &mut Session, p: &Value) -> CmdResult {
    let lon = f(p, "lon").or_else(|| f(p, "x")).ok_or("lon is required")?;
    let lat = f(p, "lat").or_else(|| f(p, "y")).ok_or("lat is required")?;
    if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err("longitude must be within ±180 and latitude within ±90".into());
    }
    let m = s.map_mut();
    m.camera.center = m.to_map(Coord::new(lon, lat)).ok_or("that point is outside the map's projection")?;
    if let Some(sc) = f(p, "scale") {
        m.set_scale(sc);
    }
    m.camera.clamp();
    s.record_extent();
    Ok(Value::Null)
}

fn basemap(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    let m = s.map_mut();
    m.basemap = Basemap::from_code(n).ok_or_else(|| format!("unknown basemap '{n}'"))?;
    // A built-in basemap takes the place of the web basemaps (tile overlays stay).
    if !super::b(p, "keepTiles").unwrap_or(false) {
        m.tiles.retain(|t| t.overlay);
    }
    Ok(Value::Null)
}

fn add_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let m = s.map_mut();
    let name = match super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.chars().take(120).collect(),
        None => format!("Bookmark {}", m.bookmarks.len() + 1),
    };
    m.bookmarks.retain(|b| !b.name.eq_ignore_ascii_case(&name));
    m.bookmarks.push(Bookmark { name: name.clone(), camera: m.camera });
    Ok(json!({"name": name}))
}

fn go_bookmark(s: &mut Session, p: &Value) -> CmdResult {
    let n = s_name(p)?;
    let cam =
        s.map().bookmarks.iter().find(|b| b.name.eq_ignore_ascii_case(n)).map(|b| b.camera).ok_or_else(|| format!("no bookmark '{n}'"))?;
    s.map_mut().camera = cam;
    s.record_extent();
    Ok(Value::Null)
}

/// Most bookmarks a map keeps from imported files.
const MAX_BOOKMARKS: usize = 10_000;

/// The map's bookmarks as a bookmarks file. Places are lon/lat and metres per pixel, so they
/// open in the same place on a map in any projection.
pub(crate) fn bookmarks_file(map: &Map) -> Result<Vec<u8>, String> {
    if map.bookmarks.is_empty() {
        return Err("the map has no bookmarks; save one with New Bookmark".into());
    }
    let flat = map.flat();
    let marks: Vec<Value> = map
        .bookmarks
        .iter()
        .filter_map(|b| {
            let c = flat.inverse(b.camera.center)?;
            Some(json!({"name": b.name, "lon": c.x, "lat": c.y, "metresPerPixel": b.camera.res * flat.ground_per_unit(b.camera.center)}))
        })
        .collect();
    if marks.is_empty() {
        return Err("none of the map's bookmarks has a place on the earth".into());
    }
    serde_json::to_vec_pretty(&json!({"type": "rustgisBookmarks", "bookmarks": marks})).map_err(|e| e.to_string())
}

fn import_bookmarks(s: &mut Session, p: &Value) -> CmdResult {
    let text = super::items::read_text(p)?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("not a RustGIS bookmarks file: {e}"))?;
    let list = v.get("bookmarks").and_then(Value::as_array).ok_or("not a RustGIS bookmarks file: no bookmarks")?;
    let m = s.map_mut();
    let flat = m.flat();
    let mut names = Vec::new();
    for b in list.iter().take(MAX_BOOKMARKS) {
        if m.bookmarks.len() >= MAX_BOOKMARKS {
            break;
        }
        let name: String = match super::ps(b, "name").map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.chars().take(120).collect(),
            None => continue,
        };
        let (Some(lon), Some(lat), Some(ground)) = (f(b, "lon"), f(b, "lat"), f(b, "metresPerPixel")) else { continue };
        if !(lon.is_finite() && lat.is_finite() && (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)) {
            continue;
        }
        if !(ground.is_finite() && ground > 0.0) {
            continue;
        }
        let Some(center) = flat.forward_checked(Coord::new(lon, lat)) else { continue };
        let mut camera = m.camera;
        camera.center = center;
        camera.res = ground / flat.ground_per_unit(center).max(1e-9);
        camera.clamp();
        // A bookmark by the same name is replaced where it is in the list.
        match m.bookmarks.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&name)) {
            Some(b) => b.camera = camera,
            None => m.bookmarks.push(Bookmark { name: name.clone(), camera }),
        }
        if !names.iter().any(|n: &String| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Err("the file has no bookmarks this map can show".into());
    }
    Ok(json!({"added": names}))
}

fn new_map(s: &mut Session, p: &Value) -> CmdResult {
    let base = super::ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Map").to_string();
    let mut name = base.clone();
    let mut i = 1;
    while s.project.maps.iter().any(|m| m.name.eq_ignore_ascii_case(&name)) {
        i += 1;
        name = format!("{base}{i}");
    }
    let mut m = Map::new(name.clone());
    m.basemap = s.map().basemap;
    s.project.maps.push(m);
    s.project.active_map = s.project.maps.len() - 1;
    s.active_layer = None;
    s.table_layer = None;
    Ok(json!({"name": name}))
}
