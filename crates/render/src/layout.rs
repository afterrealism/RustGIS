//! Draws a page layout: the page, its map frames (each a map rendered at the frame's scale and
//! clipped to its shape, with grids and extent indicators), and the surrounds.

use rustgis_data::GeometryType;
use rustgis_geom::{Coord, Extent};
use rustgis_map::layout::{ElementKind, FrameShape, Layout, PT_PER_MM, frame_camera};
use rustgis_map::symbology::LegendEntry;
use rustgis_map::{Map, Rgba};
use tiny_skia::{FillRule, Mask, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use crate::{View, paint, text};

/// Page values for dynamic text.
#[derive(Clone, Debug, Default)]
pub struct PageInfo {
    /// 1-based page number and count (a map series has many).
    pub page: usize,
    pub pages: usize,
    /// The map series page's name.
    pub page_name: String,
}

/// Largest page drawn, in pixels per side and in all (less in the browser, where memory is
/// tighter): a larger page is drawn at a lower resolution.
const MAX_SIDE: f64 = 12_000.0;
const MAX_PIXELS: f64 = if cfg!(target_arch = "wasm32") { 25_000_000.0 } else { 60_000_000.0 };

/// Renders a layout at `dpi` (dots per inch). `maps` are the project's maps, found by name.
pub fn render(layout: &Layout, maps: &mut [Map], dpi: f64, info: &PageInfo) -> Option<Pixmap> {
    let ppp = (if dpi.is_finite() { dpi } else { 96.0 } / 96.0).clamp(0.25, 8.0);
    let k = PT_PER_MM * ppp; // pixels per millimetre
    let k = k.min((MAX_PIXELS / (layout.width * layout.height).max(1.0)).sqrt());
    let (w, h) = ((layout.width * k).clamp(1.0, MAX_SIDE), (layout.height * k).clamp(1.0, MAX_SIDE));
    let k = k.min(w / layout.width).min(h / layout.height);
    // A page too large for the cap draws at a lower resolution throughout.
    let ppp = k / PT_PER_MM;
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    pm.fill(tiny_skia::Color::WHITE);
    // Frame cameras first: surrounds and extent indicators read them.
    let frames: Vec<(u32, usize, rustgis_map::Camera, f64, f64)> = layout
        .elements
        .iter()
        .filter_map(|e| match &e.kind {
            ElementKind::MapFrame { map, center, scale, .. } => {
                let mi = maps.iter().position(|m| m.name.eq_ignore_ascii_case(map))?;
                let m = maps.get(mi)?;
                Some((e.id, mi, frame_camera(m, *center, *scale), e.w * PT_PER_MM, e.h * PT_PER_MM))
            }
            _ => None,
        })
        .collect();
    let frame = |id: u32| frames.iter().find(|f| f.0 == id);
    // What each legend's frame draws, worked out before any element is drawn: its map is synced
    // to that frame's view first, so a streamed layer holds that frame's features whichever order
    // the frames and legends come in, and whichever other frame showed the map last.
    let mut shown: Vec<(u32, Vec<(u32, Vec<LegendEntry>)>)> = Vec::new();
    for e in layout.elements.iter().filter(|e| e.visible) {
        if let ElementKind::Legend { frame: f, all_classes: false, .. } = &e.kind
            && !shown.iter().any(|(id, _)| id == f)
            && let Some(&(_, mi, cam, fw, fh)) = frame(*f)
            && let Some(map) = maps.get_mut(mi)
        {
            let view = View { width: fw, height: fh, pixels_per_point: 1.0, camera: cam, dark: false };
            let lonlat = crate::view_lonlat(map, &view);
            map.sync_sources(&lonlat, true);
            shown.push((*f, shown_legends(map, map.ground_res_at(&cam) / rustgis_geom::PIXEL_M)));
        }
    }
    for e in layout.elements.iter().filter(|e| e.visible) {
        let (x, y, ew, eh) = ((e.x * k) as f32, (e.y * k) as f32, (e.w * k) as f32, (e.h * k) as f32);
        let Some(r) = Rect::from_xywh(x, y, ew.max(1.0), eh.max(1.0)) else { continue };
        match &e.kind {
            ElementKind::MapFrame { shape, grid, extent_of, .. } => {
                let Some(&(_, mi, cam, fw, fh)) = frame(e.id) else {
                    placeholder(&mut pm, r, "Missing map", ppp);
                    continue;
                };
                let Some(map) = maps.get_mut(mi) else { continue };
                // A frame larger than the page budget is drawn at a lower resolution and scaled up.
                let fppp = ppp.min((MAX_PIXELS / (fw * fh).max(1.0)).sqrt()).max(0.01);
                let view = View { width: fw, height: fh, pixels_per_point: fppp, camera: cam, dark: false };
                let Some(mut img) = crate::render_complete(map, &view) else { continue };
                for l in img.unplaced.iter().chain(&img.labels) {
                    text::draw(&mut img.pixmap, l, fppp as f32);
                }
                crate::draw_attribution(&mut img.pixmap, &img.attribution, fppp as f32);
                let mut frame_pm = img.pixmap;
                if let Some(g) = grid {
                    draw_grid(&mut frame_pm, map, &view, g);
                }
                if let Some(other) = extent_of.and_then(frame)
                    && let Some(om) = maps.get(other.1)
                {
                    let ring = frame_outline_lonlat(om, &other.2, other.3, other.4);
                    if let Some(map) = maps.get(mi) {
                        outline_lonlat(&mut frame_pm, map, &view, &ring, Rgba::rgb(0xD0, 0x1C, 0x1C), 2.0);
                    }
                }
                // Whole pixels, so the clipped image and its outline line up.
                let (x0, y0) = (x.round(), y.round());
                let r = Rect::from_xywh(x0, y0, ew.max(1.0), eh.max(1.0)).unwrap_or(r);
                let path = shape_path(shape, r);
                let up = (ppp / fppp) as f32;
                // Shaped frames are clipped by a mask of the frame's own size.
                if !matches!(shape, FrameShape::Rectangle)
                    && let Some(p) =
                        path.as_ref().and_then(|p| p.clone().transform(Transform::from_translate(-x0, -y0).post_scale(1.0 / up, 1.0 / up)))
                    && let Some(mut m) = Mask::new(frame_pm.width(), frame_pm.height())
                {
                    m.fill_path(&p, FillRule::Winding, true, Transform::identity());
                    frame_pm.apply_mask(&m);
                }
                let place = Transform::from_row(up, 0.0, 0.0, up, x0, y0);
                pm.draw_pixmap(
                    0,
                    0,
                    frame_pm.as_ref(),
                    &PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() },
                    place,
                    None,
                );
                if let Some(p) = &path {
                    pm.stroke_path(
                        p,
                        &paint(Rgba::rgb(0x30, 0x30, 0x30), 1.0),
                        &Stroke { width: 0.8 * ppp as f32, ..Stroke::default() },
                        Transform::identity(),
                        None,
                    );
                }
            }
            ElementKind::NorthArrow { .. } => north_arrow(&mut pm, r, ppp),
            ElementKind::ScaleBar { frame: f } => {
                if let Some(&(_, mi, cam, _, _)) = frame(*f)
                    && let Some(map) = maps.get(mi)
                {
                    scale_bar(&mut pm, r, map.ground_res_at(&cam), ppp);
                }
            }
            ElementKind::Legend { frame: f, title, all_classes, hide_other } => {
                if let Some(&(_, mi, ..)) = frame(*f)
                    && let Some(map) = maps.get(mi)
                {
                    let drawn = (!*all_classes).then(|| shown.iter().find(|(id, _)| id == f).map(|(_, v)| v.as_slice()).unwrap_or(&[]));
                    legend(&mut pm, r, map, title, drawn, *hide_other, ppp);
                }
            }
            ElementKind::Text { text: t, size, color, bold } => {
                let first = layout.first_frame().and_then(frame);
                let s = expand(t, layout, info, first.and_then(|f| maps.get(f.1).map(|m| (m, f.2))));
                text_block(&mut pm, r, &s, *size as f32, *color, *bold, ppp);
            }
            ElementKind::Picture { image } => {
                if let Some(img) = crate::picture::decode(image) {
                    let (iw, ih) = (img.width() as f32, img.height() as f32);
                    let s = (ew / iw).min(eh / ih);
                    let (dw, dh) = (iw * s, ih * s);
                    let t = Transform::from_row(s, 0.0, 0.0, s, x + (ew - dw) / 2.0, y + (eh - dh) / 2.0);
                    pm.draw_pixmap(
                        0,
                        0,
                        img.as_ref(),
                        &PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() },
                        t,
                        None,
                    );
                } else {
                    placeholder(&mut pm, r, "Picture", ppp);
                }
            }
            ElementKind::Table { frame: f, layer, rows } => {
                let ds = frame(*f).and_then(|fr| maps.get(fr.1)).and_then(|m| m.layer_by_name(layer)).map(|l| &l.dataset);
                match ds {
                    Some(ds) => table(&mut pm, r, ds, *rows, ppp),
                    None => placeholder(&mut pm, r, "Table", ppp),
                }
            }
            ElementKind::Chart { frame: f, layer, field, label } => {
                let ds = frame(*f).and_then(|fr| maps.get(fr.1)).and_then(|m| m.layer_by_name(layer)).map(|l| &l.dataset);
                match ds {
                    Some(ds) => chart(&mut pm, r, ds, field, label.as_deref(), ppp),
                    None => placeholder(&mut pm, r, "Chart", ppp),
                }
            }
            ElementKind::Rectangle { fill, outline, width } => {
                pm.fill_rect(r, &paint(*fill, 1.0), Transform::identity(), None);
                if *width > 0.0 {
                    let p = PathBuilder::from_rect(r);
                    pm.stroke_path(
                        &p,
                        &paint(*outline, 1.0),
                        &Stroke { width: (*width as f32) * ppp as f32, ..Stroke::default() },
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
    }
    Some(pm)
}

/// Expands layout text tokens: the page ones here, the map ones from the first frame.
pub fn expand(t: &str, layout: &Layout, info: &PageInfo, frame: Option<(&Map, rustgis_map::Camera)>) -> String {
    let t = t
        .replace("{page}", &info.page.max(1).to_string())
        .replace("{pages}", &info.pages.max(1).to_string())
        .replace("{pageName}", &info.page_name)
        .replace("{layout}", &layout.name);
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let v = match frame {
        Some((m, cam)) => rustgis_map::graphics::TokenValues {
            map: m.name.clone(),
            scale: m.ground_res_at(&cam) / rustgis_map::layout::PAPER_M_PER_PT,
            crs: m.projection.label(),
            layers: m.layers.iter().filter(|l| !l.graphics).count(),
            center: m.to_lonlat(cam.center).unwrap_or_default(),
            now,
        },
        None => rustgis_map::graphics::TokenValues {
            map: String::new(),
            scale: 0.0,
            crs: String::new(),
            layers: 0,
            center: Coord::default(),
            now,
        },
    };
    rustgis_map::graphics::expand(&t, &v)
}

fn shape_path(shape: &FrameShape, r: Rect) -> Option<tiny_skia::Path> {
    match shape {
        FrameShape::Rectangle => Some(PathBuilder::from_rect(r)),
        FrameShape::Circle => {
            let d = r.width().min(r.height());
            PathBuilder::from_oval(Rect::from_xywh(r.x() + (r.width() - d) / 2.0, r.y() + (r.height() - d) / 2.0, d, d)?)
        }
        FrameShape::Ellipse => PathBuilder::from_oval(r),
        FrameShape::Polygon(_) => {
            let mut pb = PathBuilder::new();
            for (i, (u, v)) in shape.outline().into_iter().enumerate() {
                let (px, py) = (r.x() + u as f32 * r.width(), r.y() + v as f32 * r.height());
                if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
            }
            pb.close();
            pb.finish()
        }
    }
}

/// The lon/lat outline of what a frame shows.
fn frame_outline_lonlat(map: &Map, cam: &rustgis_map::Camera, w: f64, h: f64) -> Vec<Coord> {
    let mut ring = Vec::new();
    let n = 16;
    let edge = |t: f64, side: usize| match side {
        0 => (t * w, 0.0),
        1 => (w, t * h),
        2 => ((1.0 - t) * w, h),
        _ => (0.0, (1.0 - t) * h),
    };
    for side in 0..4 {
        for i in 0..n {
            let (sx, sy) = edge(i as f64 / n as f64, side);
            if let Some(c) = map.to_lonlat(cam.to_map(sx, sy, w, h)) {
                ring.push(c);
            }
        }
    }
    if let Some(f) = ring.first().copied() {
        ring.push(f);
    }
    ring
}

fn outline_lonlat(pm: &mut Pixmap, map: &Map, view: &View, ring: &[Coord], color: Rgba, width: f32) {
    let ppp = view.pixels_per_point as f32;
    let mut pb = PathBuilder::new();
    let mut first = true;
    for c in ring {
        let Some(m) = map.to_map(*c) else { continue };
        let (x, y) = view.to_screen(m);
        let (x, y) = (x as f32 * ppp, y as f32 * ppp);
        if first {
            pb.move_to(x, y);
            first = false;
        } else {
            pb.line_to(x, y);
        }
    }
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint(color, 1.0), &Stroke { width: width * ppp, ..Stroke::default() }, Transform::identity(), None);
    }
}

/// Formats a graticule meridian or parallel label.
///
/// The Equator and prime meridian are `0°` (neither hemisphere). The antimeridian
/// is `180°` without an E/W suffix. Other values keep the usual hemisphere letter.
fn grid_degree_label(v: f64, pos: char, neg: char, step: f64) -> String {
    let d = if step < 1.0 { format!("{:.2}", v.abs()) } else { format!("{:.0}", v.abs()) };
    // The stepped loop accumulates `+= step`, so 0 and ±180 can be off by rounding error.
    let eps = step.abs() * 1e-6;
    if v.abs() < eps || (v.abs() - 180.0).abs() < eps { format!("{d}°") } else { format!("{d}°{}", if v > 0.0 { pos } else { neg }) }
}

/// A graticule over a frame, labelled along its bottom and left edges.
fn draw_grid(pm: &mut Pixmap, map: &Map, view: &View, g: &rustgis_map::layout::Grid) {
    let ppp = view.pixels_per_point as f32;
    let (w, h) = (view.width, view.height);
    let mut ext = Extent::EMPTY;
    for (sx, sy) in [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h), (w / 2.0, 0.0), (w / 2.0, h), (0.0, h / 2.0), (w, h / 2.0)] {
        if let Some(c) = map.to_lonlat(view.camera.to_map(sx, sy, w, h)) {
            ext.expand(c);
        }
    }
    if ext.is_empty() {
        return;
    }
    let span = ext.width().max(ext.height());
    let step = if g.step > 0.0 {
        g.step.max(0.001)
    } else {
        *[0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 45.0].iter().find(|s| span / **s <= 8.0).unwrap_or(&60.0)
    };
    let to_px = |c: Coord| map.to_map(c).map(|m| view.to_screen(m)).map(|(x, y)| (x as f32 * ppp, y as f32 * ppp));
    let stroke = Stroke { width: 0.6 * ppp, ..Stroke::default() };
    let lines = |pm: &mut Pixmap, pts: Vec<Coord>| {
        let mut pb = PathBuilder::new();
        let mut started = false;
        for c in pts {
            match to_px(c) {
                Some((x, y)) if x.is_finite() && y.is_finite() => {
                    if started {
                        pb.line_to(x, y)
                    } else {
                        pb.move_to(x, y)
                    }
                    started = true;
                }
                _ => started = false,
            }
        }
        if let Some(p) = pb.finish() {
            pm.stroke_path(&p, &paint(g.color, 1.0), &stroke, Transform::identity(), None);
        }
    };
    let fmt = |v: f64, pos: char, neg: char| grid_degree_label(v, pos, neg, step);
    let label_color = Rgba::rgb(0x20, 0x20, 0x20);
    let mut lon = (ext.min_x / step).floor() * step;
    let mut n = 0;
    while lon <= ext.max_x && n < 200 {
        let pts: Vec<Coord> = (0..=48).map(|i| Coord::new(lon, ext.min_y + (ext.max_y - ext.min_y) * i as f64 / 48.0)).collect();
        lines(pm, pts);
        if g.labels
            && let Some((x, _)) = to_px(Coord::new(lon, ext.min_y.max(-85.0)))
            && x > 12.0 * ppp
            && x < pm.width() as f32 - 12.0 * ppp
        {
            text::text_at(
                pm,
                &fmt(lon, 'E', 'W'),
                x,
                pm.height() as f32 - 7.0 * ppp,
                8.0 * 1.33 * ppp,
                label_color,
                false,
                text::Align::Center,
                true,
            );
        }
        lon += step;
        n += 1;
    }
    let mut lat = (ext.min_y / step).floor() * step;
    while lat <= ext.max_y && n < 400 {
        let pts: Vec<Coord> = (0..=48).map(|i| Coord::new(ext.min_x + (ext.max_x - ext.min_x) * i as f64 / 48.0, lat)).collect();
        lines(pm, pts);
        if g.labels
            && let Some((_, y)) = to_px(Coord::new(ext.min_x, lat))
            && y > 10.0 * ppp
            && y < pm.height() as f32 - 14.0 * ppp
        {
            text::text_at(pm, &fmt(lat, 'N', 'S'), 3.0 * ppp, y, 8.0 * 1.33 * ppp, label_color, false, text::Align::Left, true);
        }
        lat += step;
        n += 1;
    }
}

fn placeholder(pm: &mut Pixmap, r: Rect, label: &str, ppp: f64) {
    pm.fill_rect(r, &paint(Rgba::rgb(0xF0, 0xF1, 0xF3), 1.0), Transform::identity(), None);
    let p = PathBuilder::from_rect(r);
    pm.stroke_path(
        &p,
        &paint(Rgba::rgb(0xA0, 0xA4, 0xAA), 1.0),
        &Stroke { width: ppp as f32, ..Stroke::default() },
        Transform::identity(),
        None,
    );
    text::text_at(
        pm,
        label,
        r.x() + r.width() / 2.0,
        r.y() + r.height() / 2.0,
        10.0 * 1.33 * ppp as f32,
        Rgba::rgb(0x70, 0x74, 0x7A),
        false,
        text::Align::Center,
        false,
    );
}

fn north_arrow(pm: &mut Pixmap, r: Rect, ppp: f64) {
    let (cx, top) = (r.x() + r.width() / 2.0, r.y());
    let s = (r.width() * 0.5).min(r.height() * 0.32);
    let label_h = (r.height() * 0.25).min(14.0 * ppp as f32);
    let (tip, base) = (top + label_h + 2.0, r.y() + r.height());
    let mid = tip + (base - tip) * 0.68;
    let mut left = PathBuilder::new();
    left.move_to(cx, tip);
    left.line_to(cx - s, base);
    left.line_to(cx, mid);
    left.close();
    let mut right = PathBuilder::new();
    right.move_to(cx, tip);
    right.line_to(cx + s, base);
    right.line_to(cx, mid);
    right.close();
    let dark = paint(Rgba::rgb(0x22, 0x22, 0x22), 1.0);
    let stroke = Stroke { width: 0.8 * ppp as f32, ..Stroke::default() };
    if let Some(p) = left.finish() {
        pm.fill_path(&p, &dark, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(p) = right.finish() {
        pm.fill_path(&p, &paint(Rgba::rgb(0xFF, 0xFF, 0xFF), 1.0), FillRule::Winding, Transform::identity(), None);
        pm.stroke_path(&p, &dark, &stroke, Transform::identity(), None);
    }
    text::text_at(pm, "N", cx, top + label_h / 2.0, label_h * 1.1, Rgba::rgb(0x22, 0x22, 0x22), true, text::Align::Center, false);
}

/// An alternating scale bar filling most of the element's width. `ground_per_pt` is ground metres
/// per logical point of paper.
fn scale_bar(pm: &mut Pixmap, r: Rect, ground_per_pt: f64, ppp: f64) {
    if !(ground_per_pt.is_finite() && ground_per_pt > 0.0) {
        return;
    }
    let width_pt = r.width() as f64 / ppp;
    let target = rustgis_geom::nice_distance(ground_per_pt * width_pt * 0.85);
    if target <= 0.0 {
        return;
    }
    let len = (target / ground_per_pt * ppp) as f32;
    let bar_h = (r.height() * 0.28).min(6.0 * ppp as f32);
    let (x0, y0) = (r.x() + 2.0, r.y() + r.height() * 0.62 - bar_h / 2.0);
    for i in 0..4 {
        let c = if i % 2 == 0 { Rgba::rgb(0x22, 0x22, 0x22) } else { Rgba::rgb(0xFF, 0xFF, 0xFF) };
        if let Some(b) = Rect::from_xywh(x0 + len * i as f32 / 4.0, y0, len / 4.0, bar_h) {
            pm.fill_rect(b, &paint(c, 1.0), Transform::identity(), None);
        }
    }
    if let Some(b) = Rect::from_xywh(x0, y0, len, bar_h) {
        let p = PathBuilder::from_rect(b);
        pm.stroke_path(
            &p,
            &paint(Rgba::rgb(0x22, 0x22, 0x22), 1.0),
            &Stroke { width: 0.6 * ppp as f32, ..Stroke::default() },
            Transform::identity(),
            None,
        );
    }
    let px = 8.0 * 1.33 * ppp as f32;
    let ty = y0 - px * 0.7;
    let dark = Rgba::rgb(0x22, 0x22, 0x22);
    text::text_at(pm, "0", x0, ty, px, dark, false, text::Align::Center, false);
    text::text_at(pm, &rustgis_geom::format_distance(target / 2.0), x0 + len / 2.0, ty, px, dark, false, text::Align::Center, false);
    text::text_at(pm, &rustgis_geom::format_distance(target), x0 + len, ty, px, dark, false, text::Align::Center, false);
}

/// Each visible layer's legend for what it draws at a scale denominator (#207), by layer id; a
/// layer that draws nothing there isn't listed. The scale is the one the renderer draws the frame
/// at (the one its layers' scale ranges and display filters see), not the paper scale `{scale}`
/// prints, so the legend follows what the frame actually shows.
fn shown_legends(map: &Map, scale: f64) -> Vec<(u32, Vec<LegendEntry>)> {
    map.layers
        .iter()
        .filter(|l| l.visible && !l.graphics && !l.is_raster())
        .filter_map(|l| l.shown_legend(scale).map(|entries| (l.id, entries)))
        .collect()
}

/// The legend of a frame's map. With `drawn` (from [`shown_legends`]) it shows what the frame
/// draws: only the layers listed there, with their classes. Without, every class of every visible
/// layer. `hide_other` leaves `<all other values>` out either way.
fn legend(pm: &mut Pixmap, r: Rect, map: &Map, title: &str, drawn: Option<&[(u32, Vec<LegendEntry>)]>, hide_other: bool, ppp: f64) {
    let ppp = ppp as f32;
    let dark = Rgba::rgb(0x22, 0x22, 0x22);
    let mut y = r.y();
    let line = 15.0 * ppp;
    if !title.is_empty() {
        text::text_at(pm, title, r.x(), y + line / 2.0, 12.0 * 1.33 * ppp, dark, true, text::Align::Left, false);
        y += line * 1.3;
    }
    let sw = 16.0 * ppp;
    for l in map.layers.iter().filter(|l| l.visible && !l.graphics && !l.is_raster()) {
        if y + line > r.y() + r.height() {
            break;
        }
        let mut entries = match drawn {
            Some(d) => match d.iter().find(|(id, _)| *id == l.id) {
                Some((_, entries)) => entries.clone(),
                None => continue,
            },
            None => l.renderer.legend(),
        };
        // The fallback row only (a class that happens to bear its name stays).
        if hide_other && entries.iter().any(|e| e.fallback) {
            entries.retain(|e| !e.fallback);
            // Nothing left to show for this layer: leave it out rather than draw its plain swatch.
            if entries.is_empty() {
                continue;
            }
        }
        let gt = l.geometry_type();
        if entries.is_empty() {
            swatch(pm, r.x(), y, sw, line, gt, &l.renderer.symbol_for(&l.dataset, 0), ppp);
            text::text_at(pm, &l.name, r.x() + sw + 6.0 * ppp, y + line / 2.0, 9.0 * 1.33 * ppp, dark, false, text::Align::Left, false);
            y += line;
            continue;
        }
        text::text_at(pm, &l.name, r.x(), y + line / 2.0, 9.0 * 1.33 * ppp, dark, true, text::Align::Left, false);
        y += line;
        for en in entries {
            if y + line > r.y() + r.height() {
                break;
            }
            swatch(pm, r.x() + 6.0 * ppp, y, sw, line, gt, &en.symbol, ppp);
            text::text_at(pm, &en.label, r.x() + sw + 12.0 * ppp, y + line / 2.0, 8.5 * 1.33 * ppp, dark, false, text::Align::Left, false);
            y += line;
        }
    }
}

fn swatch(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, gt: GeometryType, s: &rustgis_map::Symbol, ppp: f32) {
    let cy = y + h / 2.0;
    match gt {
        GeometryType::Point => {
            let rad = (s.size * ppp / 2.0).clamp(1.5 * ppp, h / 2.0 - 1.0);
            if let Some(p) = PathBuilder::from_circle(x + w / 2.0, cy, rad) {
                pm.fill_path(&p, &paint(s.fill, 1.0), FillRule::Winding, Transform::identity(), None);
                pm.stroke_path(
                    &p,
                    &paint(s.stroke, 1.0),
                    &Stroke { width: s.stroke_width.max(0.5) * ppp, ..Stroke::default() },
                    Transform::identity(),
                    None,
                );
            }
        }
        GeometryType::Polyline => {
            let mut pb = PathBuilder::new();
            pb.move_to(x, cy + h * 0.2);
            pb.line_to(x + w * 0.4, cy - h * 0.2);
            pb.line_to(x + w, cy + h * 0.1);
            if let Some(p) = pb.finish() {
                pm.stroke_path(
                    &p,
                    &paint(s.stroke, 1.0),
                    &Stroke { width: s.stroke_width.max(1.0) * ppp, ..Stroke::default() },
                    Transform::identity(),
                    None,
                );
            }
        }
        _ => {
            if let Some(b) = Rect::from_xywh(x, y + h * 0.2, w, h * 0.6) {
                pm.fill_rect(b, &paint(s.fill, 1.0), Transform::identity(), None);
                let p = PathBuilder::from_rect(b);
                pm.stroke_path(
                    &p,
                    &paint(s.stroke, 1.0),
                    &Stroke { width: s.stroke_width.max(0.5) * ppp, ..Stroke::default() },
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// Text in an element: lines split at `\n`, left-aligned from the top.
fn text_block(pm: &mut Pixmap, r: Rect, s: &str, size: f32, color: Rgba, bold: bool, ppp: f64) {
    let px = size.clamp(2.0, 400.0) * 1.33 * ppp as f32;
    let mut y = r.y() + px * 0.6;
    for line in s.lines().take(200) {
        if y > r.y() + r.height() + px {
            break;
        }
        text::text_at(pm, line, r.x(), y, px, color, bold, text::Align::Left, false);
        y += px * 1.25;
    }
}

/// The first `rows` rows of a table, with as many columns as fit.
fn table(pm: &mut Pixmap, r: Rect, ds: &rustgis_data::Dataset, rows: usize, ppp: f64) {
    let ppp = ppp as f32;
    let px = 8.0 * 1.33 * ppp;
    let row_h = px * 1.6;
    let col_w = (90.0 * ppp).min(r.width());
    let cols = ((r.width() / col_w).floor() as usize).clamp(1, ds.fields.len().max(1));
    let n = rows.min(ds.features.len()).min(((r.height() / row_h) as usize).saturating_sub(1));
    let dark = Rgba::rgb(0x22, 0x22, 0x22);
    if let Some(bg) = Rect::from_xywh(r.x(), r.y(), col_w * cols as f32, row_h * (n + 1) as f32) {
        pm.fill_rect(bg, &paint(Rgba::rgb(0xFF, 0xFF, 0xFF), 1.0), Transform::identity(), None);
    }
    if let Some(hdr) = Rect::from_xywh(r.x(), r.y(), col_w * cols as f32, row_h) {
        pm.fill_rect(hdr, &paint(Rgba::rgb(0xE4, 0xE8, 0xEE), 1.0), Transform::identity(), None);
    }
    let fit = |s: &str| -> String {
        let mut t: String = s.chars().take(40).collect();
        while !t.is_empty() && text::measure(&t, 8.0, false) * ppp > col_w - 6.0 * ppp {
            t.pop();
        }
        t
    };
    for (c, f) in ds.fields.iter().take(cols).enumerate() {
        text::text_at(
            pm,
            &fit(f.display()),
            r.x() + c as f32 * col_w + 3.0 * ppp,
            r.y() + row_h / 2.0,
            px,
            dark,
            true,
            text::Align::Left,
            false,
        );
    }
    for row in 0..n {
        let y = r.y() + row_h * (row + 1) as f32;
        if row % 2 == 1
            && let Some(b) = Rect::from_xywh(r.x(), y, col_w * cols as f32, row_h)
        {
            pm.fill_rect(b, &paint(Rgba::rgb(0xF5, 0xF6, 0xF8), 1.0), Transform::identity(), None);
        }
        for c in 0..cols {
            text::text_at(
                pm,
                &fit(&ds.value(row, c).to_string()),
                r.x() + c as f32 * col_w + 3.0 * ppp,
                y + row_h / 2.0,
                px,
                dark,
                false,
                text::Align::Left,
                false,
            );
        }
    }
}

/// A bar chart of a field's values, largest first (up to 20 bars).
fn chart(pm: &mut Pixmap, r: Rect, ds: &rustgis_data::Dataset, field: &str, label: Option<&str>, ppp: f64) {
    let ppp = ppp as f32;
    let Some(fi) = ds.field_index(field) else {
        placeholder(pm, r, "Chart", ppp as f64);
        return;
    };
    let li = label.and_then(|l| ds.field_index(l));
    let mut vals: Vec<(f64, String)> = (0..ds.features.len())
        .filter_map(|row| {
            ds.value(row, fi).as_f64().filter(|v| v.is_finite()).map(|v| (v, li.map(|i| ds.value(row, i).to_string()).unwrap_or_default()))
        })
        .collect();
    vals.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    vals.truncate(20);
    let max = vals.iter().map(|v| v.0).fold(0.0_f64, f64::max);
    if vals.is_empty() || max <= 0.0 {
        placeholder(pm, r, "Chart", ppp as f64);
        return;
    }
    let dark = Rgba::rgb(0x22, 0x22, 0x22);
    let px = 7.5 * 1.33 * ppp;
    let title_h = px * 1.6;
    text::text_at(pm, field, r.x(), r.y() + title_h / 2.0, px * 1.15, dark, true, text::Align::Left, false);
    let label_w = if li.is_some() { r.width() * 0.32 } else { 0.0 };
    let bar_area = r.width() - label_w - 4.0;
    let bh = ((r.height() - title_h) / vals.len() as f32).min(18.0 * ppp);
    for (i, (v, name)) in vals.iter().enumerate() {
        let y = r.y() + title_h + bh * i as f32;
        let w = (bar_area * (*v / max) as f32).max(1.0);
        if let Some(b) = Rect::from_xywh(r.x() + label_w, y + bh * 0.15, w, bh * 0.7) {
            pm.fill_rect(b, &paint(Rgba::rgb(0x3B, 0x82, 0xC4), 1.0), Transform::identity(), None);
        }
        if li.is_some() {
            let mut t: String = name.chars().take(30).collect();
            while !t.is_empty() && text::measure(&t, 7.5, false) * ppp > label_w - 4.0 * ppp {
                t.pop();
            }
            text::text_at(pm, &t, r.x(), y + bh / 2.0, px.min(bh * 0.9), dark, false, text::Align::Left, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legends_leave_out_what_the_frame_does_not_draw() {
        use rustgis_data::{Dataset, Field, FieldType, Geometry, Value};
        // A layer with one feature, of class "a", classed a, b and c; and an empty layer.
        let mut map = Map::new("m");
        let mut d = Dataset::new("pts", GeometryType::Point, vec![Field::new("g", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Text("a".into())]);
        let pts = map.add_dataset(d);
        map.add_dataset(Dataset::new("empty", GeometryType::Point, vec![]));
        let s = |c| rustgis_map::Symbol::default_for(GeometryType::Point, c);
        if let Some(l) = map.layers.iter_mut().find(|l| l.id == pts) {
            let classes = ["a", "b", "c"].iter().zip(0u32..).map(|(k, i)| (k.to_string(), s(i))).collect();
            l.renderer = rustgis_map::Renderer::Unique { field: "g".into(), classes, other: s(3), ramp: rustgis_map::Ramp::Ember };
        }
        // How far down the legend draws, in pixels.
        let depth = |map: &Map, drawn: Option<&[(u32, Vec<LegendEntry>)]>, hide_other: bool| {
            let mut pm = Pixmap::new(300, 400).unwrap();
            pm.fill(tiny_skia::Color::WHITE);
            legend(&mut pm, Rect::from_xywh(0.0, 0.0, 300.0, 400.0).unwrap(), map, "", drawn, hide_other, 1.0);
            (0..pm.height()).filter(|&y| (0..pm.width()).any(|x| pm.pixel(x, y).is_some_and(|p| p.red() < 200))).max().unwrap_or(0)
        };
        let drawn = shown_legends(&map, 1e6);
        assert_eq!(drawn.len(), 1, "the empty layer draws nothing: {drawn:?}");
        let (shown, all) = (depth(&map, Some(&drawn), false), depth(&map, None, false));
        // The drawn legend is two rows (the layer's heading and "a"); every class makes it six
        // (b, c, <all other values> and the empty layer's row too). Compared with each other, so
        // the font and antialiasing don't matter.
        assert!(shown > 0, "the drawn class is listed");
        assert!(all >= 2 * shown, "{shown} px of {all}");
        // Hiding <all other values> takes one row off the full legend, and nothing off the drawn
        // one, which has none.
        assert!(depth(&map, None, true) < all);
        assert_eq!(depth(&map, Some(&drawn), true), shown);
        // A layer whose drawn features all fall in <all other values>, hidden, is left out rather
        // than drawn as a plain swatch.
        if let Some(l) = map.layers.iter_mut().find(|l| l.id == pts)
            && let rustgis_map::Renderer::Unique { classes, .. } = &mut l.renderer
        {
            classes.retain(|(k, _)| k != "a");
        }
        let drawn = shown_legends(&map, 1e6);
        assert!(depth(&map, Some(&drawn), false) > 0, "<all other values> alone is listed");
        assert_eq!(depth(&map, Some(&drawn), true), 0, "and hidden, the layer is left out");
        // A class that bears the fallback's name is a class: hiding the fallback keeps it.
        if let Some(l) = map.layers.iter_mut().find(|l| l.id == pts)
            && let rustgis_map::Renderer::Unique { classes, .. } = &mut l.renderer
        {
            classes.push(("a".into(), s(0)));
            classes.iter_mut().for_each(|(k, _)| {
                if k == "a" {
                    *k = rustgis_map::symbology::OTHER_VALUES.into();
                }
            });
        }
        if let Some(l) = map.layers.iter_mut().find(|l| l.id == pts) {
            l.dataset.features.iter_mut().for_each(|f| f.attrs = vec![Value::Text(rustgis_map::symbology::OTHER_VALUES.into())]);
        }
        let drawn = shown_legends(&map, 1e6);
        assert!(depth(&map, Some(&drawn), true) > 0, "the class named like the fallback is still listed");
    }

    #[test]
    fn hairline_rectangles_draw_thin() {
        // A filled 0.15 mm rule at 300 dpi is under two pixels tall, not a 1 mm bar (#211).
        let mut l = Layout::new("Layout", 50.0, 20.0);
        l.add(
            "Rule",
            (5.0, 10.0, 40.0, 0.15),
            ElementKind::Rectangle { fill: Rgba::rgb(0, 0, 0), outline: Rgba::rgb(0, 0, 0), width: 0.0 },
        );
        let pm = render(&l, &mut [], 300.0, &PageInfo::default()).unwrap();
        let x = pm.width() / 2;
        let inked = (0..pm.height()).filter(|&y| pm.pixel(x, y).is_some_and(|p| p.red() < 250)).count();
        assert!((1..=3).contains(&inked), "{inked} rows inked");
    }

    #[test]
    fn grid_degree_label_omits_hemisphere_on_zero_and_antimeridian() {
        // #210: Equator / prime meridian are 0°, antimeridian is 180°, not 0°N/0°E/180°E.
        assert_eq!(grid_degree_label(0.0, 'N', 'S', 10.0), "0°");
        assert_eq!(grid_degree_label(-0.0, 'N', 'S', 10.0), "0°");
        assert_eq!(grid_degree_label(0.0, 'E', 'W', 10.0), "0°");
        assert_eq!(grid_degree_label(180.0, 'E', 'W', 10.0), "180°");
        assert_eq!(grid_degree_label(-180.0, 'E', 'W', 10.0), "180°");
        assert_eq!(grid_degree_label(10.0, 'N', 'S', 10.0), "10°N");
        assert_eq!(grid_degree_label(-10.0, 'N', 'S', 10.0), "10°S");
        assert_eq!(grid_degree_label(20.0, 'E', 'W', 10.0), "20°E");
        assert_eq!(grid_degree_label(-20.0, 'E', 'W', 10.0), "20°W");
        assert_eq!(grid_degree_label(0.5, 'N', 'S', 0.25), "0.50°N");
        assert_eq!(grid_degree_label(0.0, 'N', 'S', 0.25), "0.00°");
        // Accumulated decimal steps miss 0 and 180 by rounding error.
        let mut v = -0.5;
        for _ in 0..5 {
            v += 0.1;
        }
        assert_ne!(v, 0.0);
        assert_eq!(grid_degree_label(v, 'N', 'S', 0.1), "0.00°");
        let mut v = 179.5;
        for _ in 0..5 {
            v += 0.1;
        }
        assert_eq!(grid_degree_label(v, 'E', 'W', 0.1), "180.00°");
    }
}
