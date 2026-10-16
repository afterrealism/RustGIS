//! The layout view: the page with its elements, shown instead of the map when a layout is open.
//! Click an element to select it, drag it to move, drag its handles to resize, Shift-drag a map
//! frame to pan its map; the wheel zooms the page. The properties panel edits the selection.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::map::layout::{ElementKind, Layout, PT_PER_MM, frame_camera};
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, regular, semibold};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Moving an element: its id and the pointer's page position (mm) at the start.
    Move(u32, (f64, f64)),
    /// Resizing from a handle (0..8 clockwise from top-left).
    Resize(u32, usize, (f64, f64)),
    /// Panning a map frame's map.
    PanFrame(u32, (f64, f64)),
    /// Panning the page view.
    Pan,
    /// Drawing a lasso frame.
    Lasso,
}

#[derive(Default)]
pub struct LayoutView {
    texture: Option<egui::TextureHandle>,
    key: Option<(u64, usize, [usize; 2], i64, i64, i64)>,
    /// Page zoom (1 = fit) and pan, in screen points.
    zoom: f32,
    pan: egui::Vec2,
    pub selected: Option<u32>,
    drag: Option<Drag>,
    /// A frame being drawn: "polygon" (clicked) or "lasso" (dragged), and its points (mm).
    pub tool: Option<&'static str>,
    sketch: Vec<(f64, f64)>,
    /// Text being typed into the element properties, by (layout, element, field): kept here so
    /// keystrokes survive the frame, and saved when the field loses focus or stops being shown.
    typing: std::collections::HashMap<(usize, u32, &'static str), String>,
    /// The layout shown, shared between frames until the project changes (layouts can hold
    /// megabytes of pictures, too much to copy every frame), keyed by layout and revision.
    shown: Option<(usize, u64, std::rc::Rc<Layout>)>,
    /// The map series page count and the current page's name, by the same key.
    series: Option<(usize, u64, usize, String)>,
}

impl LayoutView {
    /// Stops drawing a polygon or lasso frame (Esc).
    pub fn cancel_tool(&mut self) {
        self.tool = None;
        self.sketch.clear();
        if self.drag == Some(Drag::Lasso) {
            self.drag = None;
        }
    }
}

/// Page geometry on screen: the page rectangle and points per millimetre.
fn page_rect(lv: &LayoutView, area: Rect, l: &Layout) -> (Rect, f32) {
    let fit = ((area.width() - 48.0) / l.width as f32).min((area.height() - 48.0) / l.height as f32).max(0.05);
    let s = fit * if lv.zoom > 0.0 { lv.zoom } else { 1.0 };
    let size = vec2(l.width as f32 * s, l.height as f32 * s);
    (Rect::from_center_size(area.center() + lv.pan, size), s)
}

fn to_mm(page: Rect, s: f32, p: Pos2) -> (f64, f64) {
    (((p.x - page.min.x) / s) as f64, ((p.y - page.min.y) / s) as f64)
}

fn to_screen(page: Rect, s: f32, (x, y): (f64, f64)) -> Pos2 {
    pos2(page.min.x + x as f32 * s, page.min.y + y as f32 * s)
}

fn handles(r: Rect) -> [Pos2; 8] {
    let (c, m) = (r.center(), r);
    [
        m.left_top(),
        pos2(c.x, m.top()),
        m.right_top(),
        pos2(m.right(), c.y),
        m.right_bottom(),
        pos2(c.x, m.bottom()),
        m.left_bottom(),
        pos2(m.left(), c.y),
    ]
}

/// A rectangle after dragging handle `h` by (dx, dy) mm. A side the handle doesn't move keeps its
/// size, so widening a hairline rule leaves it a hairline.
fn resized((x, y, w, h): (f64, f64, f64, f64), handle: usize, dx: f64, dy: f64) -> (f64, f64, f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
    match handle {
        0 => (x0, y0) = (x0 + dx, y0 + dy),
        1 => y0 += dy,
        2 => (x1, y0) = (x1 + dx, y0 + dy),
        3 => x1 += dx,
        4 => (x1, y1) = (x1 + dx, y1 + dy),
        5 => y1 += dy,
        6 => (x0, y1) = (x0 + dx, y1 + dy),
        _ => x0 += dx,
    }
    let (sizes_x, sizes_y) = (!matches!(handle, 1 | 5), !matches!(handle, 3 | 7));
    let w = if sizes_x { (x1 - x0).abs().max(2.0) } else { w };
    let h = if sizes_y { (y1 - y0).abs().max(2.0) } else { h };
    (x0.min(x1), y0.min(y1), w, h)
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let Some(li) = app.session.active_layout else { return };
    let rev = app.session.revision;
    let layout = match &app.layout.shown {
        Some((i, r, l)) if *i == li && *r == rev => l.clone(),
        _ => {
            let Some(l) = app.session.project.layouts.get(li).cloned() else { return };
            let l = std::rc::Rc::new(l);
            app.layout.shown = Some((li, rev, l.clone()));
            l
        }
    };
    let t = Tokens::get(ui.ctx());
    if app.layout.selected.is_some_and(|id| layout.element(id).is_none()) {
        app.layout.selected = None;
    }
    save_drafts(app, li);
    if app.layout.selected.is_some() {
        egui::Panel::right("layout_props")
            .resizable(false)
            .exact_size(270.0)
            .show_separator_line(true)
            .show(ui, |ui| properties(app, ui, &layout, li));
    }
    let area = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(area, Sense::click_and_drag());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, if t.dark { Color32::from_gray(38) } else { Color32::from_rgb(0xD9, 0xDD, 0xE3) });
    let (page, s) = page_rect(&app.layout, area, &layout);
    interact(app, ui, &resp, &layout, li, page, s);
    let (page, s) = page_rect(&app.layout, area, &layout);

    // Render the page at screen resolution when it changes.
    let ppp = ui.ctx().pixels_per_point();
    let px_per_mm = s * ppp;
    // The texture can't be larger than the GPU allows: zoomed far in, the page draws coarser.
    let max_side = ui.ctx().input(|i| i.max_texture_side) as f64;
    let dpi = (px_per_mm as f64 * 25.4).min(max_side / layout.width.max(layout.height) * 25.4).clamp(20.0, 300.0);
    let size = [(page.width() * ppp) as usize, (page.height() * ppp) as usize];
    let key = (app.session.revision, li, size, (dpi * 10.0) as i64, layout.width as i64, layout.height as i64);
    if app.layout.key != Some(key) && size[0] > 2 && size[1] > 2 {
        let info = page_info(app, li);
        if let Some(pm) = rustgis_engine::render::layout::render(&layout, &mut app.session.project.maps, dpi, &info) {
            let img = egui::ColorImage::from_rgba_premultiplied([pm.width() as usize, pm.height() as usize], pm.data());
            match &mut app.layout.texture {
                Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                None => app.layout.texture = Some(ui.ctx().load_texture("layout", img, egui::TextureOptions::LINEAR)),
            }
            app.layout.key = Some(key);
        }
    }
    // Shadow, page, elements.
    painter.rect_filled(page.translate(vec2(3.0, 4.0)), 2.0, Color32::from_black_alpha(50));
    painter.rect_filled(page, 0.0, Color32::WHITE);
    if let Some(tex) = &app.layout.texture {
        painter.image(tex.id(), page, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let accent = t.accent;
    let pointer = ui.input(|i| i.pointer.latest_pos());
    // The dragged element's outline where it will land.
    if let (Some(Drag::Move(id, start) | Drag::Resize(id, _, start)), Some(p)) = (app.layout.drag, pointer)
        && let Some(e) = layout.element(id)
    {
        let (mx, my) = to_mm(page, s, p);
        let (dx, dy) = (mx - start.0, my - start.1);
        let r = match app.layout.drag {
            Some(Drag::Resize(_, h, _)) => resized((e.x, e.y, e.w, e.h), h, dx, dy),
            _ => (e.x + dx, e.y + dy, e.w, e.h),
        };
        let rr = Rect::from_min_max(to_screen(page, s, (r.0, r.1)), to_screen(page, s, (r.0 + r.2, r.1 + r.3)));
        painter.rect_stroke(rr, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
    }
    if let Some(e) = app.layout.selected.and_then(|id| layout.element(id)) {
        let r = Rect::from_min_max(to_screen(page, s, (e.x, e.y)), to_screen(page, s, (e.x + e.w, e.y + e.h)));
        painter.extend(egui::Shape::dashed_line(
            &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
            Stroke::new(1.2, accent),
            5.0,
            3.0,
        ));
        for h in handles(r) {
            painter.rect_filled(Rect::from_center_size(h, vec2(7.0, 7.0)), 1.0, Color32::WHITE);
            painter.rect_stroke(Rect::from_center_size(h, vec2(7.0, 7.0)), 1.0, Stroke::new(1.2, accent), egui::StrokeKind::Middle);
        }
        painter.text(r.left_top() + vec2(0.0, -4.0), Align2::LEFT_BOTTOM, &e.name, regular(11.0), accent);
    }
    // A frame being drawn.
    if app.layout.tool.is_some() && !app.layout.sketch.is_empty() {
        let mut pts: Vec<Pos2> = app.layout.sketch.iter().map(|p| to_screen(page, s, *p)).collect();
        if let (Some("polygon"), Some(p)) = (app.layout.tool, pointer) {
            pts.push(p);
        }
        painter.add(egui::Shape::closed_line(pts, Stroke::new(2.0, Color32::from_rgb(0x16, 0x9A, 0x5A))));
    }
    // Status: page size, zoom and hint.
    let hint = match app.layout.tool {
        Some("polygon") => tl!("Click the frame's corners · double-click to finish · Esc to stop").to_string(),
        Some(_) => tl!("Drag the frame's outline · Esc to stop").to_string(),
        None => crate::i18n::fmt(
            "{w} × {h} mm · {zoom}% · drag to move, Shift-drag a map frame to pan it, wheel to zoom",
            &[
                ("w", &format!("{:.0}", layout.width)),
                ("h", &format!("{:.0}", layout.height)),
                ("zoom", &format!("{:.0}", (if app.layout.zoom > 0.0 { app.layout.zoom } else { 1.0 }) * 100.0)),
            ],
        ),
    };
    let g = painter.layout_no_wrap(hint, regular(11.5), t.text_dim);
    let bg = Rect::from_min_size(pos2(area.min.x + 10.0, area.max.y - g.size().y - 14.0), g.size() + vec2(14.0, 8.0));
    painter.rect_filled(bg, CornerRadius::same(10), t.ribbon.gamma_multiply(0.9));
    painter.galley(bg.min + vec2(7.0, 4.0), g, t.text_dim);
    if layout.series.is_some()
        && let Some((total, name)) = series_label(app, li)
    {
        let page_no = layout.series.as_ref().map_or(0, |s| s.page);
        let label = crate::i18n::fmt(
            "Page {n} of {total}: {name}",
            &[("n", &(page_no + 1).to_string()), ("total", &total.to_string()), ("name", &name)],
        );
        let g = painter.layout_no_wrap(label, semibold(12.0), t.text);
        let bg = Rect::from_min_size(pos2(area.min.x + 10.0, area.min.y + 10.0), g.size() + vec2(16.0, 10.0));
        painter.rect_filled(bg, CornerRadius::same(10), t.ribbon);
        painter.galley(bg.min + vec2(8.0, 5.0), g, t.text);
    }
}

/// The map series page count and the current page's name, worked out once per project change.
fn series_label(app: &mut RustGisApp, li: usize) -> Option<(usize, String)> {
    let rev = app.session.revision;
    if let Some((i, r, n, name)) = &app.layout.series
        && *i == li
        && *r == rev
    {
        return Some((*n, name.clone()));
    }
    let page = app.session.project.layouts.get(li).and_then(|l| l.series.as_ref()).map_or(0, |s| s.page);
    let (n, name) = rustgis_engine::cmd::layout::series_info(&app.session, li, page)?;
    app.layout.series = Some((li, rev, n, name.clone()));
    Some((n, name))
}

/// Page values for dynamic text in the view.
pub fn page_info(app: &mut RustGisApp, li: usize) -> rustgis_engine::render::layout::PageInfo {
    let page = app.session.project.layouts.get(li).and_then(|l| l.series.as_ref()).map_or(0, |s| s.page);
    let (pages, page_name) = series_label(app, li).unwrap_or((1, String::new()));
    rustgis_engine::render::layout::PageInfo { page: page + 1, pages, page_name }
}

fn interact(app: &mut RustGisApp, ui: &mut Ui, resp: &egui::Response, layout: &Layout, li: usize, page: Rect, s: f32) {
    // Wheel zooms the page about the pointer.
    if let Some(p) = resp.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.1 {
            let old = if app.layout.zoom > 0.0 { app.layout.zoom } else { 1.0 };
            let new = (old * (scroll * 0.0028).exp()).clamp(0.2, 12.0);
            // Keep the page point under the pointer fixed.
            let centre = resp.rect.center() + app.layout.pan;
            app.layout.pan += (p - centre) * (1.0 - new / old);
            app.layout.zoom = new;
        }
    }
    let pointer_mm = resp.interact_pointer_pos().map(|p| to_mm(page, s, p));
    // Drawing a polygon or lasso frame.
    if let Some(tool) = app.layout.tool {
        if tool == "polygon" {
            if resp.double_clicked() {
                finish_frame(app, li, "polygon");
            } else if resp.clicked()
                && let Some(p) = pointer_mm
            {
                app.layout.sketch.push(p);
            }
        } else {
            if resp.drag_started() {
                app.layout.sketch.clear();
                app.layout.drag = Some(Drag::Lasso);
            }
            if app.layout.drag == Some(Drag::Lasso)
                && let Some(p) = pointer_mm
                && app.layout.sketch.last().is_none_or(|l| ((l.0 - p.0).powi(2) + (l.1 - p.1).powi(2)).sqrt() > 1.0)
            {
                app.layout.sketch.push(p);
            }
            if resp.drag_stopped() {
                app.layout.drag = None;
                finish_frame(app, li, "lasso");
            }
        }
        return;
    }
    if resp.clicked()
        && let Some(p) = pointer_mm
    {
        app.layout.selected = layout.hit(p.0, p.1);
    }
    if resp.double_clicked()
        && let Some(id) = app.layout.selected
        && matches!(layout.element(id).map(|e| &e.kind), Some(ElementKind::MapFrame { .. }))
    {
        // Double-clicking a map frame zooms it to the map view.
        let _ = app.run("layout.frameZoom", json!({"layout": li, "element": id, "to": "view"}));
    }
    if resp.drag_started()
        && let (Some(p), Some(pos)) = (pointer_mm, resp.interact_pointer_pos())
    {
        let shift = ui.input(|i| i.modifiers.shift);
        let sel = app.layout.selected.and_then(|id| layout.element(id));
        let handle = sel.and_then(|e| {
            let r = Rect::from_min_max(to_screen(page, s, (e.x, e.y)), to_screen(page, s, (e.x + e.w, e.y + e.h)));
            handles(r).iter().position(|h| (*h - pos).length() < 7.0).map(|h| (e.id, h))
        });
        app.layout.drag = Some(match (handle, layout.hit(p.0, p.1)) {
            (Some((id, h)), _) => Drag::Resize(id, h, p),
            (None, Some(id)) if shift && matches!(layout.element(id).map(|e| &e.kind), Some(ElementKind::MapFrame { .. })) => {
                app.layout.selected = Some(id);
                Drag::PanFrame(id, p)
            }
            (None, Some(id)) => {
                app.layout.selected = Some(id);
                Drag::Move(id, p)
            }
            (None, None) => Drag::Pan,
        });
    }
    if app.layout.drag == Some(Drag::Pan) {
        app.layout.pan += resp.drag_delta();
    }
    if resp.drag_stopped() {
        let end = pointer_mm.or_else(|| ui.input(|i| i.pointer.latest_pos()).map(|p| to_mm(page, s, p)));
        match (app.layout.drag, end) {
            (Some(Drag::Move(id, start)), Some(end)) => {
                if let Some(e) = layout.element(id) {
                    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
                    if dx.abs() > 0.2 || dy.abs() > 0.2 {
                        let _ = app.run("layout.update", json!({"layout": li, "element": id, "x": e.x + dx, "y": e.y + dy}));
                    }
                }
            }
            (Some(Drag::Resize(id, h, start)), Some(end)) => {
                if let Some(e) = layout.element(id) {
                    let (x, y, w, hh) = resized((e.x, e.y, e.w, e.h), h, end.0 - start.0, end.1 - start.1);
                    let _ = app.run("layout.update", json!({"layout": li, "element": id, "x": x, "y": y, "w": w, "h": hh}));
                }
            }
            (Some(Drag::PanFrame(id, start)), Some(end)) => pan_frame(app, layout, li, id, end.0 - start.0, end.1 - start.1),
            _ => {}
        }
        app.layout.drag = None;
    }
}

/// Moves a map frame's map by a page offset (mm).
fn pan_frame(app: &mut RustGisApp, layout: &Layout, li: usize, id: u32, dx: f64, dy: f64) {
    let Some(ElementKind::MapFrame { map, center, scale, .. }) = layout.element(id).map(|e| &e.kind) else { return };
    let Some(m) = app.session.project.maps.iter().find(|m| m.name.eq_ignore_ascii_case(map)) else { return };
    let cam = frame_camera(m, *center, *scale);
    let c = rustgis_engine::geom::Coord::new(cam.center.x - dx * PT_PER_MM * cam.res, cam.center.y + dy * PT_PER_MM * cam.res);
    if let Some(ll) = m.to_lonlat(c) {
        let _ = app.run("layout.update", json!({"layout": li, "element": id, "center": [ll.x, ll.y]}));
    }
}

/// Adds a polygon or lasso frame from the sketch.
fn finish_frame(app: &mut RustGisApp, li: usize, shape: &str) {
    let pts = std::mem::take(&mut app.layout.sketch);
    app.layout.tool = None;
    if pts.len() < 3 {
        return;
    }
    let (x0, y0) = pts.iter().fold((f64::MAX, f64::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let (x1, y1) = pts.iter().fold((f64::MIN, f64::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let unit: Vec<[f64; 2]> = pts.iter().map(|p| [(p.0 - x0) / w, (p.1 - y0) / h]).collect();
    if let Ok(v) = app.run("layout.addFrame", json!({"layout": li, "shape": shape, "points": unit, "x": x0, "y": y0, "w": w, "h": h})) {
        app.layout.selected = v["element"].as_u64().map(|v| v as u32);
    }
}

/// Saves the text typed into fields that are no longer shown: the selection or the layout changed
/// before the field could notice it lost focus.
fn save_drafts(app: &mut RustGisApp, li: usize) {
    let shown = app.layout.selected.map(|id| (li, id));
    let stale: Vec<_> = app.layout.typing.keys().filter(|(l, id, _)| Some((*l, *id)) != shown).copied().collect();
    for key in stale {
        let Some(text) = app.layout.typing.remove(&key) else { continue };
        let (l, id, field) = key;
        let current = app.session.project.layouts.get(l).and_then(|x| x.element(id)).and_then(|e| match (field, &e.kind) {
            ("name", _) => Some(e.name.clone()),
            ("text", ElementKind::Text { text, .. }) => Some(text.clone()),
            ("title", ElementKind::Legend { title, .. }) => Some(title.clone()),
            _ => None,
        });
        if current.is_some_and(|c| c != text) {
            let _ = app.run("layout.update", json!({"layout": l, "element": id, field: text}));
        }
    }
}

/// A text field whose typing is kept in the view's state while it has focus. Returns the new text
/// when the field loses focus with a change.
fn text_field(app: &mut RustGisApp, ui: &mut Ui, key: (usize, u32, &'static str), current: &str, multiline: bool) -> Option<String> {
    let mut buf = app.layout.typing.remove(&key).unwrap_or_else(|| current.to_string());
    let edit = if multiline {
        egui::TextEdit::multiline(&mut buf).desired_rows(3).desired_width(240.0)
    } else {
        egui::TextEdit::singleline(&mut buf)
    };
    let r = ui.add(edit);
    if r.lost_focus() {
        return (buf != current).then_some(buf);
    }
    if r.has_focus() {
        app.layout.typing.insert(key, buf);
    }
    None
}

/// The selected element's properties.
fn properties(app: &mut RustGisApp, ui: &mut Ui, layout: &Layout, li: usize) {
    let Some(e) = app.layout.selected.and_then(|id| layout.element(id)).cloned() else { return };
    let id = e.id;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!(e.kind.label())).font(semibold(14.0)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::icon_button(ui, "close", tl!("Close")).clicked() {
                app.layout.selected = None;
            }
        });
    });
    ui.separator();
    let update = |app: &mut RustGisApp, p: Value| {
        let mut p = p;
        p["layout"] = json!(li);
        p["element"] = json!(id);
        let _ = app.run("layout.update", p);
    };
    egui::Grid::new(("layout_props", id)).num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Name"));
        if let Some(name) = text_field(app, ui, (li, id, "name"), &e.name, false) {
            update(app, json!({"name": name}));
        }
        ui.end_row();
        for (label, key, v) in [("X (mm)", "x", e.x), ("Y (mm)", "y", e.y), ("Width (mm)", "w", e.w), ("Height (mm)", "h", e.h)] {
            ui.label(tl!(label));
            let mut val = v;
            let r = ui.add(egui::DragValue::new(&mut val).speed(0.5).max_decimals(1));
            if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (val - v).abs() > 1e-6 {
                update(app, json!({key: val}));
            }
            ui.end_row();
        }
        ui.label(tl!("Visible"));
        let mut vis = e.visible;
        if ui.checkbox(&mut vis, "").changed() {
            update(app, json!({"visible": vis}));
        }
        ui.end_row();
        match &e.kind {
            ElementKind::Text { text, size, color, bold } => {
                ui.label(tl!("Size"));
                let mut sz = *size;
                let r = ui.add(egui::DragValue::new(&mut sz).range(2.0..=400.0).speed(0.2).suffix(" pt"));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (sz - size).abs() > 1e-6 {
                    update(app, json!({"size": sz}));
                }
                ui.end_row();
                ui.label(tl!("Color"));
                let mut c = crate::theme::c32(*color);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    update(app, json!({"color": crate::theme::rgba(c).hex()}));
                }
                ui.end_row();
                ui.label(tl!("Bold"));
                let mut bb = *bold;
                if ui.checkbox(&mut bb, "").changed() {
                    update(app, json!({"bold": bb}));
                }
                ui.end_row();
                ui.label(tl!("Text"));
                ui.end_row();
                if let Some(tx) = text_field(app, ui, (li, id, "text"), text, true) {
                    update(app, json!({"text": tx}));
                }
                ui.end_row();
            }
            ElementKind::Legend { title, all_classes, hide_other, .. } => {
                ui.label(tl!("Title"));
                if let Some(tt) = text_field(app, ui, (li, id, "title"), title, false) {
                    update(app, json!({"title": tt}));
                }
                ui.end_row();
                // Off, the legend lists only what the frame draws (#207).
                ui.label(tl!("All classes"));
                let mut all = *all_classes;
                if ui.checkbox(&mut all, "").changed() {
                    update(app, json!({"allClasses": all}));
                }
                ui.end_row();
                ui.label(tl!("Hide other values"));
                let mut hide = *hide_other;
                if ui.checkbox(&mut hide, "").changed() {
                    update(app, json!({"hideOther": hide}));
                }
                ui.end_row();
            }
            ElementKind::MapFrame { map, scale, .. } => {
                ui.label(tl!("Map"));
                let maps: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
                egui::ComboBox::from_id_salt(("frame_map", id)).selected_text(map.as_str()).show_ui(ui, |ui| {
                    for m in maps {
                        if ui.selectable_label(m == *map, m.as_str()).clicked() {
                            update(app, json!({"map": m}));
                        }
                    }
                });
                ui.end_row();
                ui.label(tl!("Scale 1:"));
                let mut sc = *scale;
                let speed = sc * 0.01;
                let r = ui.add(egui::DragValue::new(&mut sc).range(100.0..=1e10).speed(speed).max_decimals(0));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && (sc - scale).abs() > 0.5 {
                    update(app, json!({"scale": sc}));
                }
                ui.end_row();
            }
            ElementKind::Table { rows, .. } => {
                ui.label(tl!("Rows"));
                let mut n = *rows as u64;
                let r = ui.add(egui::DragValue::new(&mut n).range(1..=500));
                if (r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged())) && n as usize != *rows {
                    update(app, json!({"rows": n}));
                }
                ui.end_row();
            }
            _ => {}
        }
    });
    ui.add_space(8.0);
    if let ElementKind::MapFrame { .. } = e.kind {
        ui.horizontal_wrapped(|ui| {
            for (label, to) in [("Zoom To Map View", "view"), ("Full Extent", "full"), ("Selection", "selection")] {
                if ui.button(tl!(label)).clicked() {
                    let _ = app.run("layout.frameZoom", json!({"layout": li, "element": id, "to": to}));
                }
            }
        });
    }
    ui.add_space(8.0);
    if ui.button(tl!("Delete Element")).clicked() {
        let _ = app.run("layout.removeElement", json!({"layout": li, "element": id}));
        app.layout.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::resized;

    #[test]
    fn widening_a_hairline_keeps_it_thin() {
        let rule = (10.0, 30.0, 100.0, 0.15);
        assert_eq!(resized(rule, 3, 20.0, 5.0), (10.0, 30.0, 120.0, 0.15));
        assert_eq!(resized(rule, 7, -10.0, 0.0), (0.0, 30.0, 110.0, 0.15));
        // Corner and top/bottom handles size the height, at least 2 mm.
        assert_eq!(resized(rule, 4, 0.0, 0.5), (10.0, 30.0, 100.0, 2.0));
        assert_eq!(resized(rule, 5, 0.0, 0.5), (10.0, 30.0, 100.0, 2.0));
    }
}
