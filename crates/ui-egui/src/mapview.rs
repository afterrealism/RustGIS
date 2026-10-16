//! The map view: the rendered map as a texture (re-rendered when the map or camera changes;
//! during pan/zoom the last frame is shifted and scaled for smoothness), the interactive tools,
//! sketches, labels, the identify pop-up, scale bar and north arrow, and the map view tabs.

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use rustgis_engine::Tool;
use rustgis_engine::geom::{Coord, Extent, geodesic_length_m, geodesic_ring_area_m2};
use rustgis_engine::map::{Camera, Globe, Projection};
use rustgis_engine::render::{PlacedLabel, View};
use serde_json::{Value, json};

use crate::theme::{Tokens, c32, regular, semibold};
use crate::{RustGisApp, icons};

#[derive(Clone, Copy, PartialEq)]
struct Key {
    rev: u64,
    cam: (i64, i64, i64),
    size: [usize; 2],
    /// Rendered at reduced resolution while the globe was moving.
    draft: bool,
    /// The tile cache's generation (0 for a map without tile layers).
    tiles: u64,
    /// Streamed layers' reads finished so far: a new one draws the map again.
    sources: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Pan,
    Box,
    /// Drawing a graphic by dragging (rectangle, circle, ellipse, freehand, lasso, picture).
    Sketch,
}

/// Graphic shapes drawn by dragging rather than clicking.
fn drag_shape(tool: Tool) -> Option<&'static str> {
    match tool {
        Tool::Graphic(s) if matches!(s, "rectangle" | "circle" | "ellipse" | "freehand" | "lasso" | "picture") => Some(s),
        _ => None,
    }
}

/// Graphic shapes drawn vertex by vertex, finished with a double-click.
fn vertex_shape(tool: Tool) -> bool {
    matches!(tool, Tool::Graphic("line" | "curve" | "polygon"))
}

pub struct Popup {
    pub info: Value,
    pub at: Coord,
    /// Which of the identified features is shown (Visible Layers identifies several).
    pub index: usize,
}

/// A notice shown as a toast.
pub struct Toast {
    pub text: String,
    pub error: bool,
    /// When it came, or was last hovered (it stays while the pointer is on it).
    pub at: f64,
    /// Its number, for a steady id.
    pub id: u64,
}

pub struct MapView {
    texture: Option<egui::TextureHandle>,
    key: Option<Key>,
    tex_camera: Camera,
    labels: Vec<PlacedLabel>,
    last_interaction: f64,
    drag: Option<(Pos2, DragKind)>,
    sketch: Vec<Coord>,
    /// A finished measurement stays on screen until the next click.
    sketch_done: bool,
    /// The next click splits the selected lines there (Editing pane › Split).
    pub split_mode: bool,
    /// The magnifying lens follows the pointer.
    pub magnifier: bool,
    pub popup: Option<Popup>,
    /// A map or layout waiting for the user to confirm its deletion.
    pub deleting: Option<Deletion>,
    pub pointer_lonlat: Option<Coord>,
    /// Where the map's context menu was opened: the pointer leaves the map for the menu.
    pub menu_lonlat: Option<Coord>,
    pub dms: bool,
    pub render_ms: u64,
    pub notices_seen: usize,
    pub toasts: Vec<Toast>,
    /// Toasts made so far, numbering each for a steady id.
    pub toasts_made: u64,
    camera_moved: bool,
    /// The last snap: its inputs (pointer, camera, undo depth) and where it snapped, if anywhere,
    /// so snapping runs only when one of them changes rather than every frame.
    snap: Option<([f64; 5], Option<Coord>)>,
    /// The side-by-side map's last render (texture and labels) and what it was drawn for, so it
    /// redraws only when that map, the project or the view changes.
    split: Option<([f64; 8], egui::TextureHandle, Vec<PlacedLabel>)>,
    /// The picture an identified feature carries (a photo's thumbnail, a `data:image/…` value):
    /// a hash of the value, and its texture (or why it couldn't be read).
    picture: Option<(u64, Result<egui::TextureHandle, String>)>,
    /// The tile layers' credits, from the last render.
    attribution: Vec<String>,
    /// Tile layer errors already reported.
    tile_errors: std::collections::HashSet<String>,
    /// When the map last rendered (seconds).
    last_render: f64,
    /// The map drawn flat and laid over the globe while it moves.
    drape: crate::globe_drape::GlobeDrape,
    /// Shooting stars around the globe.
    comets: crate::comets::Comets,
    /// GPU map backend: pipelines (built lazily from the render state), the uploaded scene,
    /// and egui's wgpu render state handed over by the host app.
    pub(crate) gpu_renderer: Option<std::sync::Arc<crate::mapgpu::GpuMapRenderer>>,
    pub(crate) gpu_frame: Option<std::sync::Arc<crate::mapgpu::SceneBuffers>>,
    /// The GPU scene's offscreen MSAA target, shared with the paint callback across frames.
    pub(crate) gpu_target: std::sync::Arc<std::sync::Mutex<Option<crate::mapgpu::SceneTarget>>>,
    /// The last side-by-side map's GPU scene (its key, buffers and labels), when the second
    /// map rendered on the GPU.
    pub(crate) split_gpu: Option<([f64; 8], std::sync::Arc<crate::mapgpu::SceneBuffers>, Vec<PlacedLabel>)>,
    /// The split map's offscreen MSAA target (its own slot — main and split differ in size).
    pub(crate) gpu_target_split: std::sync::Arc<std::sync::Mutex<Option<crate::mapgpu::SceneTarget>>>,
    pub(crate) render_state: Option<egui_wgpu::RenderState>,
    /// The GPU map backend, for now gated on the RUSTGIS_GPU environment variable.
    pub gpu_enabled: bool,
    /// Rolling map-frame timestamps (for the status-bar FPS readout).
    frame_times: std::collections::VecDeque<f64>,
}

impl Default for MapView {
    fn default() -> Self {
        MapView {
            texture: None,
            key: None,
            tex_camera: Camera::world(),
            labels: Vec::new(),
            last_interaction: -10.0,
            drag: None,
            sketch: Vec::new(),
            sketch_done: false,
            split_mode: false,
            magnifier: false,
            popup: None,
            deleting: None,
            pointer_lonlat: None,
            menu_lonlat: None,
            dms: false,
            render_ms: 0,
            notices_seen: 0,
            toasts: Vec::new(),
            toasts_made: 0,
            camera_moved: false,
            snap: None,
            split: None,
            picture: None,
            attribution: Vec::new(),
            tile_errors: std::collections::HashSet::new(),
            last_render: -10.0,
            drape: Default::default(),
            comets: Default::default(),
            gpu_renderer: None,
            gpu_frame: None,
            gpu_target: std::sync::Arc::new(std::sync::Mutex::new(None)),
            split_gpu: None,
            gpu_target_split: std::sync::Arc::new(std::sync::Mutex::new(None)),
            render_state: None,
            gpu_enabled: std::env::var_os("RUSTGIS_GPU").is_some(),
            frame_times: std::collections::VecDeque::new(),
        }
    }
}

impl MapView {
    /// egui's wgpu render state from the host (`frame.wgpu_render_state()`), for the GPU map
    /// backend. Cheap to call every frame: the state is Arc-backed and pipelines build lazily.
    pub fn set_render_state(&mut self, rs: Option<egui_wgpu::RenderState>) {
        self.render_state = rs;
    }

    /// Records one drawn map frame (the status bar's FPS readout reads the rolling window).
    fn record_frame(&mut self, now: f64) {
        self.frame_times.push_back(now);
        while self.frame_times.len() > 240 {
            self.frame_times.pop_front();
        }
        while self.frame_times.front().is_some_and(|t| *t < now - 2.0) {
            self.frame_times.pop_front();
        }
    }

    /// Frames drawn in the last second; `None` when the map is idle (egui repaints on demand,
    /// so no frames means nothing is moving — distinct from "0 fps" when moving).
    pub(crate) fn fps(&self, now: f64) -> Option<usize> {
        let live = self.frame_times.back().is_some_and(|t| *t > now - 0.5);
        live.then(|| self.frame_times.iter().filter(|t| **t > now - 1.0).count())
    }
    /// Forces a re-render on the next frame; also drops the GPU frame (the backend toggle
    /// relies on it).
    pub fn invalidate(&mut self) {
        self.key = None;
        self.gpu_frame = None;
    }
    /// The camera the last full render was drawn with (diagnostics and tests).
    pub fn full_frame_camera(&self) -> Camera {
        self.tex_camera
    }

    /// How long the last map draw took, in milliseconds.
    pub fn render_ms(&self) -> u64 {
        self.render_ms
    }

    pub fn cancel_sketch(&mut self) {
        self.sketch.clear();
        self.sketch_done = false;
    }
    pub fn sketch_len(&self) -> usize {
        self.sketch.len()
    }
}

fn now(ui: &Ui) -> f64 {
    ui.input(|i| i.time)
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    view_tabs(app, ui, &t);
    if app.session.active_layout.is_some() {
        crate::layoutview::show(app, ui);
        return;
    }
    if app.ws.catalog_view {
        egui::Frame::NONE.inner_margin(egui::Margin::same(16)).show(ui, |ui| crate::workspacepanes::catalog_view(app, ui, &t));
        return;
    }
    animate(app, ui);
    // Side by side: the other map on the right, following this one when views are linked.
    if let Some(other) = split_map(app).filter(|i| *i != app.session.project.active_map) {
        let full = ui.available_rect_before_wrap();
        let right = Rect::from_min_max(pos2(full.center().x + 1.0, full.min.y), full.max);
        split_view(app, ui, other, right, &t);
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(full.min, pos2(full.center().x - 1.0, full.max.y))));
        main_view(app, &mut left, &t);
        ui.allocate_rect(full, Sense::hover());
        return;
    }
    main_view(app, ui, &t);
}

/// Plays the map's animation: the camera moves through the keyframes in real time.
fn animate(app: &mut RustGisApp, ui: &mut Ui) {
    let Some((t, k)) = app.ws.playing else { return };
    let keys = app.session.map().animation.clone();
    let (Some(a), Some(b)) = (keys.get(k), keys.get(k + 1)) else {
        app.ws.playing = None;
        return;
    };
    let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
    let t = t + dt / b.seconds.max(0.1);
    let e = t.min(1.0);
    let s = e * e * (3.0 - 2.0 * e);
    let c = Coord::new(a.center.x + (b.center.x - a.center.x) * s, a.center.y + (b.center.y - a.center.y) * s);
    let sc = (a.scale.max(1.0).ln() + (b.scale.max(1.0).ln() - a.scale.max(1.0).ln()) * s).exp();
    let m = app.session.map_mut();
    if let Some(mc) = m.to_map(c) {
        m.camera.center = mc;
    }
    m.set_scale(sc);
    app.ws.playing = if t >= 1.0 { Some((0.0, k + 1)) } else { Some((t, k)) };
    ui.ctx().request_repaint();
}

/// The second map of a side-by-side view: drawn read-only at its own (or the linked) camera.
fn split_view(app: &mut RustGisApp, ui: &mut Ui, other: usize, rect: Rect, t: &Tokens) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    if w < 4.0 || h < 4.0 {
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let revision = app.session.revision;
    let Some(m) = app.session.project.maps.get_mut(other) else { return };
    let view = View { width: w, height: h, pixels_per_point: ppp as f64, camera: m.camera };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, c32(m.basemap.palette().0));
    let key = [other as f64, revision as f64, m.camera.center.x, m.camera.center.y, m.camera.res, w, h, f64::from(ppp)];
    let gpu_ok = app.map.gpu_enabled
        && !m.projection.is_globe()
        && m.view.effects.is_none()
        && m.view.color_vision == rustgis_engine::map::view::ColorVision::Normal;
    // Fresh GPU scene? Keep it. Stale? Rebuild. Either way `rendered_gpu` tracks whether the
    // split map draws from `split_gpu` this frame.
    let mut rendered_gpu = gpu_ok && app.map.split_gpu.as_ref().is_some_and(|(k, _, _)| *k == key);
    if gpu_ok && !rendered_gpu {
        if app.map.gpu_renderer.is_none()
            && let Some(rs) = &app.map.render_state
        {
            app.map.gpu_renderer = Some(std::sync::Arc::new(crate::mapgpu::GpuMapRenderer::new(rs)));
        }
        if let (Some(r), Some(rs)) = (&app.map.gpu_renderer, &app.map.render_state)
            && let Some(rustgis_engine::render::RenderOutput::Gpu(scene)) =
                rustgis_engine::render::render_backend(m, &view, rustgis_engine::render::Backend::Gpu)
        {
            // None = the scene exceeds the device's limits: the CPU path below renders it.
            if let Some(bufs) = crate::mapgpu::SceneBuffers::from_scene(&rs.device, &rs.queue, r, &scene, m.camera, [w, h], ppp) {
                app.map.split_gpu = Some((key, std::sync::Arc::new(bufs), scene.labels));
                rendered_gpu = true;
            }
        }
    }
        }
    }
    if !rendered_gpu {
        app.map.split_gpu = None;
        if app.map.split.as_ref().is_none_or(|(k, _, _)| *k != key)
            && let Some(r) = rustgis_engine::render::render(m, &view)
        {
            let img = egui::ColorImage::from_rgba_premultiplied([r.pixmap.width() as usize, r.pixmap.height() as usize], r.pixmap.data());
            match app.map.split.take() {
                Some((_, mut tex, _)) => {
                    tex.set(img, egui::TextureOptions::LINEAR);
                    app.map.split = Some((key, tex, r.labels));
                }
                None => app.map.split = Some((key, ui.ctx().load_texture("split_map", img, egui::TextureOptions::LINEAR), r.labels)),
            }
        }
    }
    if let (Some((_, bufs, labels)), Some(gr)) = (&app.map.split_gpu, &app.map.gpu_renderer) {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            crate::mapgpu::SceneCallback { renderer: gr.clone(), buffers: bufs.clone(), target: app.map.gpu_target_split.clone() },
        ));
        for l in labels {
            draw_label(&painter, l, pos2(rect.min.x + l.x, rect.min.y + l.y));
        }
    } else if let Some((_, tex, labels)) = &app.map.split {
        painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        for l in labels {
            draw_label(&painter, l, pos2(rect.min.x + l.x, rect.min.y + l.y));
        }
    }
    painter.line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(2.0, t.border_strong));
    let name = app.session.project.maps.get(other).map(|m| m.name.clone()).unwrap_or_default();
    painter.text(rect.left_top() + vec2(10.0, 10.0), Align2::LEFT_TOP, name, semibold(12.0), t.text);
    // Linked cursors: the pointer's place on the main map, shown on this one.
    if app.ws.link_cursors
        && let Some(c) = app.map.pointer_lonlat
        && let Some(m) = app.session.project.maps.get(other)
        && let Some(mc) = m.to_map(c)
    {
        let (x, y) = m.camera.to_screen(mc, w, h);
        let p = pos2(rect.min.x + x as f32, rect.min.y + y as f32);
        if rect.contains(p) {
            let red = Color32::from_rgb(0xE0, 0x30, 0x30);
            painter.line_segment([p - vec2(10.0, 0.0), p + vec2(10.0, 0.0)], Stroke::new(1.5, red));
            painter.line_segment([p - vec2(0.0, 10.0), p + vec2(0.0, 10.0)], Stroke::new(1.5, red));
        }
    }
}

fn main_view(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let t = *t;
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    if w < 4.0 || h < 4.0 {
        return;
    }
    app.session.view_size = (w, h);
    // The start extent, fitted to the view's real size.
    if let Some(e) = app.ui.start_view.take() {
        let _ = app.run("map.zoomToExtent", json!({"extent": e}));
    }
    let ppp = ui.ctx().pixels_per_point();
    let time = now(ui);
    app.map.record_frame(time);

    interact(app, ui, &resp, rect);

    // Render when the map changed and we're not in the middle of a pan/zoom: meanwhile the flat
    // map's last frame is shifted and scaled. The globe can't be faked that way (turning and
    // zooming in perspective aren't shifts and scales): while it moves, the map drawn flat is
    // draped over the sphere (see `globe_drape`), or failing that it redraws every frame at
    // reduced resolution; it is drawn exactly once it settles.
    let cam = app.session.map().camera;
    let globe_view = app.session.map().projection == Projection::Globe;
    let dragging = app.map.drag.is_some_and(|(_, k)| k == DragKind::Pan);
    let moving = dragging || time - app.map.last_interaction < 0.18;
    let draped = if globe_view && moving {
        let key = crate::globe_drape::DrapeKey { rev: app.session.revision, sources: rustgis_engine::map::source::generation() };
        let tiles = if app.session.map().tiles.is_empty() { 0 } else { rustgis_engine::render::rustgis_tiles::cache::generation() };
        app.map.drape.prepare(ui.ctx(), app.session.map_mut(), key, tiles, w, h, ppp, time)
    } else {
        if !globe_view {
            app.map.drape.clear();
        }
        false
    };
    let draft = globe_view && moving && !draped;
    let render_ppp = if draft { (ppp * 0.6).max(0.5) } else { ppp };
    let key = Key {
        rev: app.session.revision,
        cam: ((cam.center.x * 100.0) as i64, (cam.center.y * 100.0) as i64, (cam.res * 1000.0) as i64),
        size: [(w as f32 * ppp).round() as usize, (h as f32 * ppp).round() as usize],
        draft,
        tiles: 0,
        sources: rustgis_engine::map::source::generation(),
    };
    // Tiles arriving draw the map again, at most a few times a second.
    static WAKER: std::sync::Once = std::sync::Once::new();
    WAKER.call_once(|| {
        let ctx = ui.ctx().clone();
        rustgis_engine::render::rustgis_tiles::cache::set_waker(move || ctx.request_repaint());
        let ctx = ui.ctx().clone();
        rustgis_engine::map::source::set_waker(move || ctx.request_repaint());
    });
    let mut key = key;
    if !app.session.map().tiles.is_empty() {
        key.tiles = rustgis_engine::render::rustgis_tiles::cache::generation();
        if let Some(old) = app.map.key
            && old.tiles != key.tiles
            && (Key { tiles: old.tiles, ..key }) == old
            && time - app.map.last_render < 0.25
        {
            key.tiles = old.tiles;
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
    let interacting = moving && (!globe_view || draped);
    if globe_view && moving {
        // Wake up once the motion has settled, for the full-resolution frame.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    if globe_view && !moving && app.map.key.is_some_and(|k| k.draft) {
        ui.ctx().request_repaint();
    }
    // The GPU backend draws flat, effect-free frames; it re-renders on every change (no
    // texture-shift trick — meshes re-project in the vertex shader).
    let gpu_ok = app.map.gpu_enabled
        && !globe_view
        && app.session.map().view.effects.is_none()
        && app.session.map().view.color_vision == rustgis_engine::map::view::ColorVision::Normal;
    // GPU fast path: during pan/zoom (or a camera animation) with an all-GPU scene (no CPU
    // image runs — rasters and tiles don't re-project), only the camera changes, so rewrite
    // the per-draw uniforms instead of re-tessellating. `key` and `tex_camera` stay at the
    // last full render: the scene's contents (extent culling, the 0.35 px decimation, label
    // placement) are baked for that camera, so once the motion settles the still-stale key
    // makes the branch below do one full re-render.
    let stale = app.map.key != Some(key);
    let cam_only = app.map.key.is_some_and(|k| (Key { cam: k.cam, ..key }) == k);
    let mut fast_engaged = false;
    if stale
        && gpu_ok
        && (interacting || app.ws.playing.is_some())
        && cam_only
        && let (Some(gf), Some(rs)) = (&app.map.gpu_frame, &app.map.render_state)
        && !gf.has_images
    {
        gf.set_camera(&rs.queue, &cam);
        fast_engaged = true;
    }
    let stale = app.map.key != Some(key);
    if stale
        && !fast_engaged
        && (gpu_ok || !interacting || app.map.texture.is_none() || app.map.key.is_some_and(|k| k.rev != key.rev || k.size != key.size))
    {
        let t0 = web_time::Instant::now();
        let view = View { width: w, height: h, pixels_per_point: render_ppp as f64, camera: cam };
        let mut rendered_gpu = false;
        if gpu_ok {
            if app.map.gpu_renderer.is_none()
                && let Some(rs) = &app.map.render_state
            {
                app.map.gpu_renderer = Some(std::sync::Arc::new(crate::mapgpu::GpuMapRenderer::new(rs)));
            }
            if let (Some(r), Some(rs)) = (&app.map.gpu_renderer, &app.map.render_state)
                && let Some(rustgis_engine::render::RenderOutput::Gpu(scene)) =
                    rustgis_engine::render::render_backend(app.session.map_mut(), &view, rustgis_engine::render::Backend::Gpu)
            {
                if let Some(bufs) = crate::mapgpu::SceneBuffers::from_scene(&rs.device, &rs.queue, r, &scene, cam, [w, h], render_ppp) {
                    app.map.gpu_frame = Some(std::sync::Arc::new(bufs));
                    app.map.labels = scene.labels;
                    app.map.labels.splice(0..0, scene.unplaced);
                    app.map.attribution = scene.attribution;
                    app.map.last_render = time;
                    for e in scene.tile_errors {
                        if app.map.tile_errors.insert(e.clone()) {
                            app.session.notify(e, true);
                        }
                    }
                    app.map.tex_camera = cam;
                    app.map.key = Some(key);
                    rendered_gpu = true;
                }
                // The scene exceeds the device's limits (huge buffers, oversized images): the
                // CPU block below renders the frame instead.
            }
        }
        // The CPU path, unchanged — also the fallback when the GPU backend declines a frame
        // (render_backend's own compatibility guard, or a scene over the device's limits), or
        // no render state arrived yet.
        if !rendered_gpu {
            app.map.gpu_frame = None;
            if let Some(r) = rustgis_engine::render::render(app.session.map_mut(), &view) {
                let size = [r.pixmap.width() as usize, r.pixmap.height() as usize];
                let img = egui::ColorImage::from_rgba_premultiplied(size, r.pixmap.data());
                match &mut app.map.texture {
                    Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                    None => app.map.texture = Some(ui.ctx().load_texture("map", img, egui::TextureOptions::LINEAR)),
                }
                app.map.labels = r.labels;
                app.map.labels.splice(0..0, r.unplaced);
                app.map.attribution = r.attribution;
                app.map.last_render = time;
                for e in r.tile_errors {
                    if app.map.tile_errors.insert(e.clone()) {
                        app.session.notify(e, true);
                    }
                }
                app.map.tex_camera = cam;
                app.map.key = Some(key);
            }
        }
        app.map.render_ms = t0.elapsed().as_millis() as u64;
        if app.map.camera_moved {
            app.session.record_extent();
            app.map.camera_moved = false;
        }
    } else if stale {
        ui.ctx().request_repaint();
    }

    // Draw the (possibly shifted) map texture.
    let painter = ui.painter_at(rect);
    // Behind the (possibly shifted or shrunk) last frame: the map's own background, so zooming
    // out never shows a band of window colour around it.
    let basemap = app.session.map().basemap;
    if draped {
        app.map.drape.paint(ui.ctx(), &painter, rect, app.session.map(), ppp);
    } else if globe_view {
        app.map.drape.paint_space(ui.ctx(), &painter, rect, app.session.map(), ppp);
    } else {
        painter.rect_filled(rect, 0.0, c32(basemap.palette().0));
    }
    let old = app.map.tex_camera;
    let e0 = old.extent(w, h);
    let to_screen = |c: &Camera, x: f64, y: f64| {
        let (sx, sy) = c.to_screen(Coord::new(x, y), w, h);
        pos2(rect.min.x + sx as f32, rect.min.y + sy as f32)
    };
    let tex_rect = if globe_view {
        // Where the old frame's centre now is, at the new zoom.
        let (old_g, new_g) = (Globe::new(&old, w, h), Globe::new(&cam, w, h));
        let s = (new_g.radius() / old_g.radius()) as f32;
        match new_g.forward(old.center_lonlat()) {
            Some((x, y)) if s.is_finite() => Rect::from_center_size(pos2(rect.min.x + x as f32, rect.min.y + y as f32), rect.size() * s),
            _ => rect,
        }
    } else {
        Rect::from_min_max(to_screen(&cam, e0.min_x, e0.max_y), to_screen(&cam, e0.max_x, e0.min_y))
    };
    // A fresh GPU frame (re-rendered whenever the key changed, so its camera is the current
    // one: tex_rect == rect and labels sit unshifted) draws inside egui's wgpu pass. The GPU
    // backend only serves flat maps, so the globe's drape never meets it.
    if let (Some(gf), Some(gr)) = (&app.map.gpu_frame, &app.map.gpu_renderer) {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            crate::mapgpu::SceneCallback { renderer: gr.clone(), buffers: gf.clone(), target: app.map.gpu_target.clone() },
        ));
    } else if let Some(tex) = app.map.texture.as_ref().filter(|_| !draped) {
        painter.image(tex.id(), tex_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    // Shooting stars, while space shows around the globe.
    let g = Globe::new(&cam, w, h);
    let (gx, gy) = g.center();
    let space = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().any(|&(x, y)| (x - gx).hypot(y - gy) > g.disc_radius());
    if globe_view && space && app.session.map().view.effects.stars {
        let wait = app.map.comets.paint(&painter, rect, &g, time);
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(wait.clamp(0.0, 60.0)));
    } else {
        app.map.comets.clear();
    }
    // Labels (moved with the texture; hidden while zooming, and while the globe moves, where
    // re-placing them every frame would make them flicker).
    let zooming = (old.res / cam.res - 1.0).abs() > 0.002;
    let hide_labels = zooming || (globe_view && moving);
    if !hide_labels {
        let dx = tex_rect.min.x - rect.min.x;
        let dy = tex_rect.min.y - rect.min.y;
        for l in &app.map.labels {
            draw_label(&painter, l, pos2(rect.min.x + l.x + dx, rect.min.y + l.y + dy));
        }
    }
    overlays(app, ui, &painter, rect, &t);
    popup(app, ui, rect);
}

fn draw_label(p: &egui::Painter, l: &PlacedLabel, at: Pos2) {
    let font = if l.bold { semibold(l.size * 1.33) } else { regular(l.size * 1.33) };
    // Text set beside an icon is lined up by its edge, with egui's own measure of its width.
    let align = if l.align < -0.25 {
        Align2::LEFT_CENTER
    } else if l.align > 0.25 {
        Align2::RIGHT_CENTER
    } else {
        Align2::CENTER_CENTER
    };
    if let Some(h) = l.halo {
        let hc = c32(h);
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.8, -0.8), (0.8, 0.8), (-0.8, 0.8), (0.8, -0.8)] {
            p.text(at + vec2(dx, dy), align, &l.text, font.clone(), hc);
        }
    }
    p.text(at, align, &l.text, font, c32(l.color));
}

/// The globe as seen in this view, when the map is a globe.
fn globe(app: &RustGisApp, rect: Rect) -> Option<Globe> {
    let m = app.session.map();
    (m.projection == Projection::Globe).then(|| Globe::new(&m.camera, rect.width() as f64, rect.height() as f64))
}

/// The longitude/latitude under a screen point (`None` off the globe).
fn screen_to_lonlat(app: &RustGisApp, rect: Rect, p: Pos2) -> Option<Coord> {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let (x, y) = ((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
    match globe(app, rect) {
        Some(g) => g.inverse(x, y),
        None => app.session.map().to_lonlat(app.session.map().camera.to_map(x, y, w, h)),
    }
}

/// The screen point of a longitude/latitude (`None` behind the globe).
fn lonlat_to_screen(app: &RustGisApp, rect: Rect, c: Coord) -> Option<Pos2> {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let (x, y) = match globe(app, rect) {
        Some(g) => g.forward(c)?,
        None => app.session.map().camera.to_screen(app.session.map().to_map(c)?, w, h),
    };
    Some(pos2(rect.min.x + x as f32, rect.min.y + y as f32))
}

/// Pans the flat map, or turns the globe, by a screen offset.
fn pan(app: &mut RustGisApp, dx: f64, dy: f64, w: f64, h: f64) {
    let m = app.session.map_mut();
    if m.projection == Projection::Globe {
        Globe::drag(&mut m.camera, dx, dy, w, h);
    } else {
        m.camera.pan_px(dx, dy);
    }
}

/// Zooms by `factor` about a screen point.
fn zoom(app: &mut RustGisApp, factor: f64, x: f64, y: f64, w: f64, h: f64) {
    let m = app.session.map_mut();
    if m.projection == Projection::Globe {
        Globe::zoom_at(&mut m.camera, factor, x, y, w, h);
    } else {
        m.camera.zoom_at(factor, x, y, w, h);
    }
}

fn interact(app: &mut RustGisApp, ui: &mut Ui, resp: &egui::Response, rect: Rect) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let time = now(ui);
    let hover = resp.hover_pos();
    app.map.pointer_lonlat = hover.and_then(|p| screen_to_lonlat(app, rect, p));
    let tool = app.session.tool;

    // Wheel zoom about the pointer.
    if let Some(p) = hover {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
        if scroll.abs() > 0.1 {
            let f = (scroll as f64 * 0.0028).exp();
            zoom(app, f, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
            app.map.popup = None;
        }
    }
    // First-person navigation: W A S D walk, Q and E fly up and down.
    if app.session.map().view.first_person && !ui.ctx().egui_wants_keyboard_input() {
        let (mut fw, mut rt, mut up) = (0.0, 0.0, 0.0);
        ui.input(|i| {
            // Shortcuts (Ctrl+S, Ctrl+E…) aren't steps.
            if !i.modifiers.is_none() {
                return;
            }
            // Nine steps a second, whatever the frame rate.
            let step = (f64::from(i.stable_dt) * 9.0).clamp(0.0, 1.0);
            if i.key_down(egui::Key::W) {
                fw += step;
            }
            if i.key_down(egui::Key::S) {
                fw -= step;
            }
            if i.key_down(egui::Key::D) {
                rt += step;
            }
            if i.key_down(egui::Key::A) {
                rt -= step;
            }
            if i.key_down(egui::Key::E) {
                up += step;
            }
            if i.key_down(egui::Key::Q) {
                up -= step;
            }
        });
        if fw != 0.0 || rt != 0.0 || up != 0.0 {
            let _ = app.session.run("map.walk", &json!({"forward": fw, "right": rt, "up": up}));
            app.map.last_interaction = time;
            app.map.camera_moved = true;
            ui.ctx().request_repaint();
        }
    }
    // Arrow keys pan.
    if !ui.ctx().egui_wants_keyboard_input() {
        let (mut dx, mut dy) = (0.0, 0.0);
        ui.input(|i| {
            if i.key_down(egui::Key::ArrowLeft) {
                dx += 12.0;
            }
            if i.key_down(egui::Key::ArrowRight) {
                dx -= 12.0;
            }
            if i.key_down(egui::Key::ArrowUp) {
                dy += 12.0;
            }
            if i.key_down(egui::Key::ArrowDown) {
                dy -= 12.0;
            }
        });
        if dx != 0.0 || dy != 0.0 {
            pan(app, dx, dy, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
        }
    }

    let shift = ui.input(|i| i.modifiers.shift);
    let middle = ui.input(|i| i.pointer.middle_down());
    // Drag start.
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let kind = if middle {
            DragKind::Pan
        } else {
            match tool {
                Tool::Explore if shift => DragKind::Box,
                Tool::Explore => DragKind::Pan,
                Tool::Select | Tool::ZoomIn => DragKind::Box,
                t if drag_shape(t).is_some() => DragKind::Sketch,
                _ => DragKind::Pan,
            }
        };
        app.map.drag = Some((p, kind));
        app.map.popup = None;
        if kind == DragKind::Sketch {
            app.map.cancel_sketch();
            if let Some(c) = screen_to_lonlat(app, rect, p) {
                app.map.sketch.push(c);
            }
        }
    }
    // A dragged graphic follows the pointer: freehand shapes keep every point, the others their
    // two ends.
    if let (Some((_, DragKind::Sketch)), Some(p)) = (app.map.drag, resp.interact_pointer_pos())
        && let Some(c) = screen_to_lonlat(app, rect, p)
    {
        let freehand = matches!(tool, Tool::Graphic("freehand" | "lasso"));
        let far = app.map.sketch.last().and_then(|l| lonlat_to_screen(app, rect, *l)).is_none_or(|s| (s - p).length() > 3.0);
        if freehand {
            // Capped well below what a graphic may hold, so a very long drag still draws.
            if far && app.map.sketch.len() < 20_000 {
                app.map.sketch.push(c);
            }
        } else {
            app.map.sketch.truncate(1);
            app.map.sketch.push(c);
        }
    }
    if let Some((_, DragKind::Pan)) = app.map.drag {
        let d = resp.drag_delta();
        if d != egui::Vec2::ZERO {
            match (globe(app, rect), resp.interact_pointer_pos()) {
                // The globe turns so the grabbed point stays under the pointer.
                (Some(_), Some(p)) => {
                    let (x, y) = ((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
                    let m = app.session.map_mut();
                    Globe::drag_point(&mut m.camera, (x - d.x as f64, y - d.y as f64), (x, y), w, h);
                }
                _ => pan(app, d.x as f64, d.y as f64, w, h),
            }
            app.map.camera_moved = true;
            app.map.last_interaction = time;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    if resp.drag_stopped()
        && let Some((_, DragKind::Sketch)) = app.map.drag
    {
        app.map.drag = None;
        if let Some(shape) = drag_shape(tool) {
            let pts = std::mem::take(&mut app.map.sketch);
            if shape == "picture" {
                app.ui.pending_picture = Some(pts);
                let _ = app.run("ui.addPicture", Value::Null);
            } else {
                finish_graphic(app, shape, pts);
            }
        }
        return;
    }
    if resp.drag_stopped() {
        if let (Some((start, DragKind::Box)), Some(end)) = (app.map.drag, resp.interact_pointer_pos()) {
            // The box's extent from a grid of points inside it, so a box reaching off the globe (even
            // one whose corners all miss it) uses the part that is on it.
            let r = Rect::from_two_pos(start, end);
            let mut e = Extent::EMPTY;
            let n = 12;
            for i in 0..=n {
                for j in 0..=n {
                    let q = pos2(r.min.x + r.width() * i as f32 / n as f32, r.min.y + r.height() * j as f32 / n as f32);
                    if let Some(c) = screen_to_lonlat(app, rect, q) {
                        e.expand(c);
                    }
                }
            }
            if e.is_empty() {
                // Nothing on the globe.
            } else if tool == Tool::Select {
                let mode = ui.input(|i| {
                    if i.modifiers.shift {
                        "add"
                    } else if i.modifiers.command {
                        "remove"
                    } else {
                        "new"
                    }
                });
                let _ = app.run(
                    "select.byRect",
                    json!({"minLon": e.min_x, "minLat": e.min_y, "maxLon": e.max_x, "maxLat": e.max_y, "mode": mode}),
                );
            } else if (start - end).length() > 6.0 {
                app.session.zoom_to(&e);
                app.map.camera_moved = true;
            }
        }
        app.map.drag = None;
        app.map.last_interaction = time - 1.0;
    }

    // Clicks.
    if resp.double_clicked() {
        if matches!(tool, Tool::CreateLine | Tool::CreatePolygon | Tool::Measure | Tool::MeasureArea) || vertex_shape(tool) {
            finish_sketch(app);
        } else if let Some(p) = resp.interact_pointer_pos() {
            zoom(app, 2.0, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
            app.map.last_interaction = time;
            app.map.camera_moved = true;
            app.map.popup = None;
        }
        return;
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let Some(c) = screen_to_lonlat(app, rect, p) else {
            // A click off the globe.
            app.map.popup = None;
            return;
        };
        // A workflow waiting for map points takes the click.
        if let Some(pick) = &mut app.workflow.pick
            && pick.points.len() < pick.needed
        {
            pick.points.push(c);
            return;
        }
        if app.map.split_mode {
            app.map.split_mode = false;
            let _ = app.run("edit.split", json!({"lon": c.x, "lat": c.y}));
            return;
        }
        // Sketch vertices snap to features when snapping is on.
        let c = if tool.creates() || matches!(tool, Tool::Graphic(_) | Tool::Measure | Tool::MeasureArea) { snapped(app, c) } else { c };
        match tool {
            Tool::Explore => {
                let r = app.session.run("identify", &json!({"lon": c.x, "lat": c.y, "tolerancePx": 5}));
                app.map.popup = match r {
                    Ok(v) if !v.is_null() => {
                        if let (Some(id), Some(row)) = (v["layerId"].as_u64(), v["row"].as_u64()) {
                            let _ = app.session.run("select.rows", &json!({"layer": id, "rows": [row]}));
                        }
                        Some(Popup { info: v, at: c, index: 0 })
                    }
                    _ => {
                        let _ = app.session.run("select.clear", &Value::Null);
                        None
                    }
                };
            }
            Tool::Select => {
                let mode = ui.input(|i| {
                    if i.modifiers.shift {
                        "add"
                    } else if i.modifiers.command {
                        "remove"
                    } else {
                        "new"
                    }
                });
                let _ = app.run("select.atPoint", json!({"lon": c.x, "lat": c.y, "tolerancePx": 5, "mode": mode}));
            }
            Tool::ZoomIn => {
                zoom(app, 2.0, (p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h);
                app.map.last_interaction = time;
                app.map.camera_moved = true;
            }
            Tool::CreatePoint => {
                let _ = app.run("edit.addFeature", with_template(app, json!({"geometry": {"type": "Point", "coordinates": [c.x, c.y]}})));
            }
            Tool::Graphic("point") => finish_graphic(app, "point", vec![c]),
            Tool::Graphic("text") => {
                let _ = app.run("ui.dialog", json!({"name": "graphicText", "lon": c.x, "lat": c.y}));
            }
            // Dragged shapes ignore plain clicks.
            Tool::Graphic(s) if !vertex_shape(Tool::Graphic(s)) => {}
            Tool::Measure | Tool::MeasureArea | Tool::CreateLine | Tool::CreatePolygon | Tool::Graphic(_) => {
                if app.map.sketch_done {
                    app.map.cancel_sketch();
                }
                // A double-click arrives as two clicks first; ignore the duplicate vertex.
                let dup = app.map.sketch.last().and_then(|l| lonlat_to_screen(app, rect, *l)).is_some_and(|s| (s - p).length() < 3.0);
                if !dup {
                    app.map.sketch.push(c);
                }
            }
        }
    }
    if resp.secondary_clicked() {
        app.map.menu_lonlat = app.map.pointer_lonlat;
    }
    resp.context_menu(|ui| {
        let c = app.map.menu_lonlat;
        if ui.button(tl!("Zoom In")).clicked() {
            let _ = app.run("map.zoomIn", Value::Null);
            ui.close();
        }
        if ui.button(tl!("Zoom Out")).clicked() {
            let _ = app.run("map.zoomOut", Value::Null);
            ui.close();
        }
        if ui.button(tl!("Full Extent")).clicked() {
            let _ = app.run("map.zoomFull", Value::Null);
            ui.close();
        }
        ui.separator();
        if let Some(c) = c
            && ui.button(tl!("Copy Coordinates")).clicked()
        {
            ui.ctx().copy_text(format!("{:.6}, {:.6}", c.x, c.y));
            ui.close();
        }
        if ui.add_enabled(app.session.map().selection_count() > 0, egui::Button::new(tl!("Clear Selection"))).clicked() {
            let _ = app.run("select.clear", Value::Null);
            ui.close();
        }
    });
    if hover.is_some()
        && matches!(tool, Tool::Measure | Tool::MeasureArea | Tool::CreateLine | Tool::CreatePolygon)
        && !app.map.sketch.is_empty()
    {
        ui.ctx().request_repaint();
    }
}

fn finish_sketch(app: &mut RustGisApp) {
    let tool = app.session.tool;
    let pts = app.map.sketch.clone();
    match tool {
        Tool::Measure | Tool::MeasureArea => app.map.sketch_done = true,
        Tool::CreateLine if pts.len() >= 2 => {
            let coords: Vec<[f64; 2]> = pts.iter().map(|c| [c.x, c.y]).collect();
            let _ = app.run("edit.addFeature", with_template(app, json!({"geometry": {"type": "LineString", "coordinates": coords}})));
            app.map.cancel_sketch();
        }
        Tool::CreatePolygon if pts.len() >= 3 => {
            let mut coords: Vec<[f64; 2]> = pts.iter().map(|c| [c.x, c.y]).collect();
            if let Some(f) = coords.first().copied() {
                coords.push(f);
            }
            let _ = app.run("edit.addFeature", with_template(app, json!({"geometry": {"type": "Polygon", "coordinates": [coords]}})));
            app.map.cancel_sketch();
        }
        Tool::Graphic(shape) => {
            let min = if shape == "polygon" { 3 } else { 2 };
            if pts.len() >= min {
                finish_graphic(app, shape, pts);
            }
        }
        _ => {}
    }
}

/// A point snapped to nearby features (when snapping is on).
fn snapped(app: &mut RustGisApp, c: Coord) -> Coord {
    if !app.session.map().editing.snapping.enabled {
        return c;
    }
    match app.session.run("edit.snap", &json!({"lon": c.x, "lat": c.y})) {
        Ok(v) => Coord::new(v["lon"].as_f64().unwrap_or(c.x), v["lat"].as_f64().unwrap_or(c.y)),
        Err(_) => c,
    }
}

/// Feature creation parameters with the Editing pane's chosen template, if it belongs to the layer.
fn with_template(app: &RustGisApp, mut p: Value) -> Value {
    let t = &app.forms.template;
    let has = app
        .session
        .active_layer
        .and_then(|id| app.session.map().layer(id))
        .is_some_and(|l| l.design.templates.iter().any(|x| x.name == *t));
    if has {
        p["template"] = json!(t);
    }
    p
}

/// Adds a graphic drawn with a tool, then returns to Explore unless the tool is kept.
pub fn finish_graphic(app: &mut RustGisApp, shape: &str, pts: Vec<Coord>) {
    let coords: Vec<[f64; 2]> = pts.iter().map(|c| [c.x, c.y]).collect();
    let ok = app.run("graphics.add", json!({"shape": shape, "points": coords})).is_ok();
    app.map.cancel_sketch();
    // A failed graphic (a click without a drag) keeps the tool; the error is in the notices.
    if ok && !app.session.keep_tool {
        app.session.tool = Tool::Explore;
    }
}

fn overlays(app: &mut RustGisApp, ui: &mut Ui, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let accent = t.accent;
    // Box drag.
    if let (Some((start, DragKind::Box)), Some(cur)) = (app.map.drag, ui.input(|i| i.pointer.latest_pos())) {
        let r = Rect::from_two_pos(start, cur);
        p.rect_filled(r, 0.0, accent.gamma_multiply(0.12));
        p.rect_stroke(r, 0.0, Stroke::new(1.5, accent), egui::StrokeKind::Middle);
    }
    // Vertices of the selected features while editing.
    let editing = &app.session.map().editing;
    if editing.settings.enabled
        && editing.settings.show_vertices
        && (app.session.tool.creates() || app.ui.right == Some(crate::RightPane::Editing))
    {
        let mut verts: Vec<Coord> = Vec::new();
        for l in app.session.map().layers.iter().filter(|l| !l.selection.is_empty() && !l.is_raster() && !l.graphics) {
            for r in &l.selection {
                if let Some(g) = l.dataset.features.get(*r).and_then(|f| f.geometry.as_ref()) {
                    g.for_each_coord(|c| {
                        if verts.len() < 4000 {
                            verts.push(c);
                        }
                    });
                }
            }
        }
        for v in verts {
            if let Some(sp) = lonlat_to_screen(app, rect, v) {
                let r = Rect::from_center_size(sp, vec2(6.0, 6.0));
                p.rect_filled(r, 0.0, Color32::WHITE);
                p.rect_stroke(r, 0.0, Stroke::new(1.2, Color32::from_rgb(0x16, 0x9A, 0x5A)), egui::StrokeKind::Middle);
            }
        }
    }
    // The navigator: a compass, zoom buttons and a pan pad in the top-left corner.
    if app.session.map().view.navigator {
        navigator(app, ui, p, rect, t);
    }
    // First-person crosshair.
    if app.session.map().view.first_person {
        let c = rect.center();
        for (a, b) in [
            (vec2(-12.0, 0.0), vec2(-4.0, 0.0)),
            (vec2(4.0, 0.0), vec2(12.0, 0.0)),
            (vec2(0.0, -12.0), vec2(0.0, -4.0)),
            (vec2(0.0, 4.0), vec2(0.0, 12.0)),
        ] {
            p.line_segment([c + a, c + b], Stroke::new(2.0, Color32::from_black_alpha(160)));
        }
    }
    // The magnifier: twice the map under the pointer, in a lens beside it. It samples the CPU
    // texture, which GPU frames don't update — hide the lens while the GPU backend is drawing.
    if app.map.magnifier
        && app.map.gpu_frame.is_none()
        && let (Some(tex), Some(ptr)) = (&app.map.texture, ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p)))
    {
        let r = 70.0;
        let centre = pos2((ptr.x + r + 24.0).min(rect.max.x - r - 4.0), (ptr.y - r - 24.0).max(rect.min.y + r + 4.0));
        let uv_c = vec2((ptr.x - rect.min.x) / rect.width(), (ptr.y - rect.min.y) / rect.height());
        let half = vec2(r / 2.0 / rect.width(), r / 2.0 / rect.height());
        let mut mesh = egui::Mesh::with_texture(tex.id());
        let n = 48;
        let centre_uv = pos2(uv_c.x, uv_c.y);
        mesh.vertices.push(egui::epaint::Vertex { pos: centre, uv: centre_uv, color: Color32::WHITE });
        for k in 0..=n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            let (cs, sn) = (a.cos(), a.sin());
            mesh.vertices.push(egui::epaint::Vertex {
                pos: centre + vec2(cs, sn) * r,
                uv: pos2(uv_c.x + cs * half.x, uv_c.y + sn * half.y),
                color: Color32::WHITE,
            });
        }
        for k in 1..=n as u32 {
            mesh.indices.extend_from_slice(&[0, k, k + 1]);
        }
        p.add(egui::Shape::mesh(mesh));
        p.circle_stroke(centre, r, Stroke::new(2.5, t.accent));
        p.circle_stroke(ptr, 6.0, Stroke::new(1.5, t.accent));
    }
    // Points picked for a workflow.
    if let Some(pick) = &app.workflow.pick {
        let pts = pick.points.clone();
        for (i, c) in pts.iter().enumerate() {
            if let Some(sp) = lonlat_to_screen(app, rect, *c) {
                p.circle_filled(sp, 6.0, Color32::from_rgb(0xE5, 0x6A, 0x1A));
                p.circle_stroke(sp, 6.0, Stroke::new(1.5, Color32::WHITE));
                p.text(sp + vec2(9.0, -9.0), Align2::LEFT_BOTTOM, (i + 1).to_string(), semibold(11.0), Color32::from_rgb(0xE5, 0x6A, 0x1A));
            }
        }
    }
    // Where a click would snap to.
    if app.session.map().editing.snapping.enabled
        && (app.session.tool.creates() || matches!(app.session.tool, Tool::Graphic(_) | Tool::Measure | Tool::MeasureArea))
        && let Some(c) = app.map.pointer_lonlat
        && let Some(at) = {
            let cam = app.session.map().camera;
            let key = [c.x, c.y, cam.center.x + cam.center.y * 1e-3, cam.res, app.session.revision as f64];
            match &app.map.snap {
                Some((k, at)) if *k == key => *at,
                _ => {
                    // Asked directly, not as a command: commands mark the map changed.
                    let at = rustgis_engine::cmd::design::snap_at(&mut app.session, c).map(|(at, _)| at);
                    app.map.snap = Some((key, at));
                    at
                }
            }
        }
        && let Some(sp) = lonlat_to_screen(app, rect, at)
    {
        let magenta = Color32::from_rgb(0xD0, 0x2A, 0xB0);
        p.circle_stroke(sp, 7.0, Stroke::new(2.0, magenta));
        p.circle_filled(sp, 2.5, magenta);
    }
    // A dragged rectangle, circle or ellipse previews as its outline.
    let tool = app.session.tool;
    let mut previewed = false;
    if !app.map.sketch.is_empty()
        && let Some(shape) = drag_shape(tool).filter(|s| !matches!(*s, "freehand" | "lasso"))
    {
        // Rectangles, circles and ellipses preview as their outline between the two points.
        if let (Some(a), Some(b)) = (
            app.map.sketch.first().and_then(|c| lonlat_to_screen(app, rect, *c)),
            app.map.sketch.last().and_then(|c| lonlat_to_screen(app, rect, *c)),
        ) {
            let green = Color32::from_rgb(0x16, 0x9A, 0x5A);
            let stroke = Stroke::new(2.0, green);
            match shape {
                "circle" => {
                    p.circle_stroke(a, (b - a).length(), stroke);
                }
                "ellipse" => {
                    let r = Rect::from_two_pos(a, b);
                    let pts: Vec<Pos2> = (0..72)
                        .map(|i| {
                            let t = i as f32 / 72.0 * std::f32::consts::TAU;
                            pos2(r.center().x + r.width() / 2.0 * t.cos(), r.center().y + r.height() / 2.0 * t.sin())
                        })
                        .collect();
                    p.add(egui::Shape::closed_line(pts, stroke));
                }
                _ => {
                    p.rect_stroke(Rect::from_two_pos(a, b), 0.0, stroke, egui::StrokeKind::Middle);
                }
            }
        }
        previewed = true;
    }
    // Sketch.
    if !app.map.sketch.is_empty() && !previewed {
        let mut pts: Vec<Coord> = app.map.sketch.clone();
        if !app.map.sketch_done
            && let Some(c) = app.map.pointer_lonlat
        {
            pts.push(c);
        }
        let closed = matches!(tool, Tool::MeasureArea | Tool::CreatePolygon | Tool::Graphic("polygon" | "lasso"));
        let placed: Vec<Option<Pos2>> = pts.iter().map(|c| lonlat_to_screen(app, rect, *c)).collect();
        let screen: Vec<Pos2> = placed.iter().flatten().copied().collect();
        let color = if tool.creates() { Color32::from_rgb(0x16, 0x9A, 0x5A) } else { Color32::from_rgb(0xE5, 0x6A, 0x1A) };
        let all_visible = screen.len() == placed.len();
        if closed && all_visible && screen.len() >= 3 {
            p.add(egui::Shape::closed_line(screen.clone(), Stroke::new(2.0, color)));
        } else {
            // On the globe a vertex behind the horizon breaks the outline rather than joining its
            // neighbours across the disc.
            for run in placed.split(|s| s.is_none()) {
                let run: Vec<Pos2> = run.iter().flatten().copied().collect();
                if run.len() >= 2 {
                    p.add(egui::Shape::line(run, Stroke::new(2.0, color)));
                }
            }
        }
        for s in &screen {
            p.circle_filled(*s, 3.5, Color32::WHITE);
            p.circle_stroke(*s, 3.5, Stroke::new(1.5, color));
        }
        if matches!(tool, Tool::Measure | Tool::MeasureArea) && pts.len() >= 2 {
            let text = if tool == Tool::Measure {
                let seg = pts.len().checked_sub(2).and_then(|i| pts.get(i..)).map(geodesic_length_m).unwrap_or(0.0);
                crate::i18n::fmt(
                    "Total {total}   ·   Segment {segment}",
                    &[
                        ("total", &rustgis_engine::geom::format_distance(geodesic_length_m(&pts))),
                        ("segment", &rustgis_engine::geom::format_distance(seg)),
                    ],
                )
            } else {
                let mut ring = pts.clone();
                if let Some(f) = ring.first().copied() {
                    ring.push(f);
                }
                crate::i18n::fmt(
                    "Area {area}   ·   Perimeter {perimeter}",
                    &[
                        ("area", &rustgis_engine::geom::format_area(geodesic_ring_area_m2(&ring))),
                        ("perimeter", &rustgis_engine::geom::format_distance(geodesic_length_m(&ring))),
                    ],
                )
            };
            let anchor = pos2(rect.center().x, rect.min.y + 22.0);
            let galley = p.layout_no_wrap(text, semibold(12.5), t.text);
            let bg = Rect::from_center_size(anchor, galley.size() + vec2(24.0, 12.0));
            p.rect_filled(bg, CornerRadius::same(14), t.menu);
            p.rect_stroke(bg, CornerRadius::same(14), Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
            p.galley(bg.center() - galley.size() / 2.0, galley, t.text);
        }
    }
    // Hint for the active tool.
    let hint = match app.session.tool {
        Tool::Measure => Some("Click to add points · double-click to finish · Esc to stop"),
        Tool::MeasureArea => Some("Click the corners of the area · double-click to finish"),
        Tool::CreatePoint => Some("Click the map to add a point"),
        Tool::CreateLine => Some("Click to add vertices · double-click to finish the line"),
        Tool::CreatePolygon => Some("Click to add vertices · double-click to close the polygon"),
        Tool::Select => Some("Click or drag a box · Shift adds · Ctrl removes"),
        Tool::ZoomIn => Some("Drag a box or click to zoom in"),
        Tool::Graphic("point") => Some("Click the map to place a point"),
        Tool::Graphic("text") => Some("Click the map to place text"),
        Tool::Graphic("line" | "curve") => Some("Click to add vertices · double-click to finish"),
        Tool::Graphic("polygon") => Some("Click to add vertices · double-click to close"),
        Tool::Graphic("freehand" | "lasso") => Some("Drag to draw"),
        Tool::Graphic("circle") => Some("Drag from the centre out"),
        Tool::Graphic(_) => Some("Drag a box"),
        Tool::Explore => None,
    };
    if let Some(h) = hint
        && (app.map.sketch.is_empty() || !matches!(app.session.tool, Tool::Measure | Tool::MeasureArea))
    {
        let g = p.layout_no_wrap(tl!(h).to_string(), regular(12.0), Color32::WHITE);
        let bg = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 26.0), g.size() + vec2(22.0, 10.0));
        p.rect_filled(bg, CornerRadius::same(12), Color32::from_black_alpha(160));
        p.galley(bg.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    scale_bar(app, p, rect, t);
    north_arrow(p, rect, t);
    // Attribution.
    let mut att = String::from("Natural Earth");
    for a in &app.map.attribution {
        att.push_str(" | ");
        att.push_str(a);
    }
    let g = p.layout(att, regular(10.5), t.text_dim, (rect.width() - 240.0).max(120.0));
    let r = Rect::from_min_size(pos2(rect.max.x - g.size().x - 14.0, rect.max.y - g.size().y - 6.0), g.size() + vec2(10.0, 4.0));
    p.rect_filled(r, CornerRadius::same(3), t.ribbon.gamma_multiply(0.75));
    p.galley(r.min + vec2(5.0, 2.0), g, t.text_dim);
    // Navigation buttons (zoom in/out/full) in the top-right corner.
    let mut y = rect.min.y + 12.0;
    for (icon, id, tip) in
        [("zoom_in", "map.zoomIn", "Zoom in"), ("zoom_out", "map.zoomOut", "Zoom out"), ("zoom_full", "map.zoomFull", "Full extent")]
    {
        let r = Rect::from_min_size(pos2(rect.max.x - 44.0, y), vec2(32.0, 32.0));
        let resp = ui.interact(r, egui::Id::new(("nav", id)), Sense::click());
        p.rect_filled(r, CornerRadius::same(8), if resp.hovered() { t.hover } else { t.ribbon });
        p.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        icons::paint(p, r.shrink(7.0), icon, t.icon, t.accent);
        if resp.on_hover_text(tl!(tip)).clicked() {
            let _ = app.run(id, Value::Null);
            app.map.popup = None;
        }
        y += 38.0;
    }
}

fn scale_bar(app: &RustGisApp, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let m_per_pt = app.session.map().ground_res();
    let d = rustgis_engine::geom::nice_distance(m_per_pt * 130.0);
    if d <= 0.0 {
        return;
    }
    let len = (d / m_per_pt) as f32;
    let x0 = rect.min.x + 18.0;
    let y0 = rect.max.y - 22.0;
    let bg = Rect::from_min_max(pos2(x0 - 8.0, y0 - 20.0), pos2(x0 + len + 10.0, y0 + 10.0));
    p.rect_filled(bg, CornerRadius::same(5), t.ribbon.gamma_multiply(0.85));
    for i in 0..4 {
        let seg = Rect::from_min_size(pos2(x0 + len * i as f32 / 4.0, y0), vec2(len / 4.0, 5.0));
        p.rect_filled(seg, 0.0, if i % 2 == 0 { t.text } else { t.ribbon });
    }
    p.rect_stroke(Rect::from_min_size(pos2(x0, y0), vec2(len, 5.0)), 0.0, Stroke::new(1.0, t.text), egui::StrokeKind::Middle);
    p.text(pos2(x0, y0 - 3.0), Align2::LEFT_BOTTOM, "0", regular(10.5), t.text);
    p.text(pos2(x0 + len, y0 - 3.0), Align2::RIGHT_BOTTOM, rustgis_engine::geom::format_distance(d), regular(10.5), t.text);
}

fn north_arrow(p: &egui::Painter, rect: Rect, t: &Tokens) {
    let c = pos2(rect.min.x + 30.0, rect.min.y + 34.0);
    p.circle_filled(c, 17.0, t.ribbon.gamma_multiply(0.85));
    p.circle_stroke(c, 17.0, Stroke::new(1.0, t.border_strong));
    p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -12.0), c + vec2(6.0, 6.0), c + vec2(0.0, 2.5)], t.text, Stroke::NONE));
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(0.0, -12.0), c + vec2(0.0, 2.5), c + vec2(-6.0, 6.0)],
        t.text.gamma_multiply(0.4),
        Stroke::NONE,
    ));
    p.text(c + vec2(0.0, 11.5), Align2::CENTER_CENTER, "N", semibold(8.5), t.text);
}

fn popup(app: &mut RustGisApp, ui: &mut Ui, rect: Rect) {
    let Some(pp) = &app.map.popup else { return };
    let t = Tokens::get(ui.ctx());
    let Some(anchor) = lonlat_to_screen(app, rect, pp.at) else { return };
    if !rect.contains(anchor) {
        return;
    }
    let results = pp.info["results"].as_array().map_or(0, Vec::len);
    let index = pp.index.min(results.saturating_sub(1));
    let info = if results > 1 { pp.info["results"][index].clone() } else { pp.info.clone() };
    let mut step: Option<usize> = None;
    let mut zoom = false;
    // The feature shown, as its layer id and feature id (a raster pixel has neither).
    let shown = info["layerId"].as_u64().and_then(|id| u32::try_from(id).ok()).zip(info["fid"].as_u64());
    let layer = info["layer"].as_str().unwrap_or("").to_string();
    let mut attrs = info["attributes"].as_object().cloned().unwrap_or_default();
    // A picture value (a photo's thumbnail) shows as the picture, not as its text.
    let picture_key = attrs.iter().find(|(_, v)| v.as_str().is_some_and(is_picture)).map(|(k, _)| k.clone());
    let picture = picture_key.and_then(|k| attrs.remove(&k)).and_then(|v| v.as_str().map(|s| picture_texture(app, ui.ctx(), s)));
    let title = if info["raster"].as_bool() == Some(true) {
        tl!("Pixel").to_string()
    } else {
        {
            // A field called "name" first, then any name-like one (adm0name, NAME_EN…).
            let named = |exact: bool| {
                attrs.iter().find(|(k, _)| if exact { k.eq_ignore_ascii_case("name") } else { k.to_lowercase().contains("name") })
            };
            named(true).or_else(|| named(false)).and_then(|(_, v)| v.as_str()).unwrap_or(tl!("Feature")).to_string()
        }
    };
    let p = ui.painter();
    p.circle_filled(anchor, 6.0, Color32::from_rgb(0x00, 0xE5, 0xFF).gamma_multiply(0.5));
    p.circle_stroke(anchor, 6.0, Stroke::new(2.0, Color32::from_rgb(0x00, 0xA8, 0xC0)));
    // The popup is as tall as its rows, up to a limit; the corner grip sets the width and that
    // limit, and both are remembered for the next popup.
    let limit_id = egui::Id::new("identify_popup_limit");
    let shown_id = egui::Id::new("identify_popup_shown");
    let space = vec2((rect.width() - 24.0).max(240.0), (rect.height() - 24.0).max(140.0));
    let limit = ui.ctx().data(|d| d.get_temp::<egui::Vec2>(limit_id)).unwrap_or(vec2(310.0, 360.0)).min(space);
    let last_h = ui.ctx().data(|d| d.get_temp::<f32>(shown_id)).unwrap_or(limit.y);
    let pos = pos2(
        (anchor.x + 14.0).min(rect.max.x - limit.x - 12.0).max(rect.min.x + 8.0),
        (anchor.y - 40.0).clamp(rect.min.y + 8.0, (rect.max.y - last_h - 12.0).max(rect.min.y + 8.0)),
    );
    let mut close = false;
    let mut new_limit = None;
    // Keyed by the feature: egui remembers an area's (and a scroll area's and grid's) size, and
    // the next popup mustn't start from the last one's.
    let key = (&layer, &title, attrs.len());
    egui::Area::new(egui::Id::new(("identify_popup", key))).fixed_pos(pos).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        let frame = egui::Frame::NONE
            .fill(t.menu)
            .stroke(Stroke::new(1.0, t.border_strong))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: t.shadow })
            .show(ui, |ui| {
                ui.set_width(limit.x);
                let header = egui::Frame::NONE
                    .fill(t.pane_header)
                    .corner_radius(CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 })
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&title).font(semibold(14.0)));
                                ui.label(egui::RichText::new(&layer).size(11.0).color(t.text_dim));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                if ui.add(egui::Button::new(tl!("×")).frame(false)).clicked() {
                                    close = true;
                                }
                                // A feature (not a raster pixel): zoom to it.
                                if shown.is_some() {
                                    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                                    let ink = if resp.hovered() { t.text } else { t.icon };
                                    crate::icons::paint(ui.painter(), r, "zoom_sel", ink, t.accent);
                                    if resp.on_hover_text(tl!("Zoom To")).clicked() {
                                        zoom = true;
                                    }
                                }
                                // Several features identified: step through them.
                                if results > 1 {
                                    if ui.add_enabled(index + 1 < results, egui::Button::new("›").frame(false)).clicked() {
                                        step = Some(index + 1);
                                    }
                                    let n = (index + 1).to_string();
                                    let total = results.to_string();
                                    ui.label(
                                        egui::RichText::new(crate::i18n::fmt("{n} of {total}", &[("n", &n), ("total", &total)]))
                                            .size(11.0)
                                            .color(t.text_dim),
                                    );
                                    if ui.add_enabled(index > 0, egui::Button::new("‹").frame(false)).clicked() {
                                        step = Some(index.saturating_sub(1));
                                    }
                                }
                            });
                        });
                    });
                if let Some(picture) = &picture {
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| match picture {
                        Ok(tex) => {
                            let size = tex.size_vec2();
                            let scale = (ui.available_width() / size.x.max(1.0)).min(1.5);
                            ui.add(egui::Image::new((tex.id(), size * scale)).corner_radius(CornerRadius::same(6)));
                        }
                        Err(e) => {
                            ui.label(egui::RichText::new(e.as_str()).size(11.0).color(t.text_dim));
                        }
                    });
                }
                // A solid scroll bar in its own gutter, shown when the rows are taller than the
                // limit (a floating one is invisible until hovered).
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                // Full width, so the scrollbar sits on the right edge; as tall as the rows, up to
                // the limit.
                egui::ScrollArea::vertical()
                    .id_salt(("popup_rows", key))
                    .auto_shrink([false, true])
                    .max_height((limit.y - header.response.rect.height()).max(40.0))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .show(ui, |ui| {
                        egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 14, top: 6, bottom: 12 }).show(ui, |ui| {
                            // The values take the width the names leave, so the stripes reach the scrollbar.
                            let name_w = attrs
                                .keys()
                                .map(|k| crate::widgets::text_w(ui, k, regular(11.5)))
                                .fold(0.0_f32, f32::max)
                                .min(ui.available_width() * 0.5);
                            let value_w = (ui.available_width() - name_w - 12.0).max(80.0);
                            egui::Grid::new(("popup_grid", key)).num_columns(2).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                                for (k, v) in &attrs {
                                    ui.label(egui::RichText::new(k).size(11.5).color(t.text_dim));
                                    ui.scope(|ui| {
                                        // A fixed column: long values wrap, and the stripes span it.
                                        ui.set_min_width(value_w);
                                        ui.set_max_width(value_w);
                                        ui.add(egui::Label::new(egui::RichText::new(v.as_str().unwrap_or("")).size(11.5)).wrap());
                                    });
                                    ui.end_row();
                                }
                            });
                        });
                    });
            });
        // The corner grip: dragging it sets the width and the height limit.
        let r = frame.response.rect;
        let grip = Rect::from_min_max(r.max - vec2(16.0, 16.0), r.max);
        let resp = ui.interact(grip, egui::Id::new("identify_popup_grip"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeNwSe);
        let color = if resp.hovered() || resp.dragged() { t.text } else { t.text_dim };
        for k in [4.0, 8.0, 12.0] {
            ui.painter()
                .line_segment([pos2(r.max.x - 3.0, r.max.y - 3.0 - k), pos2(r.max.x - 3.0 - k, r.max.y - 3.0)], Stroke::new(1.0, color));
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            new_limit = Some(vec2(limit.x + d.x, r.height() + d.y).clamp(vec2(240.0, 120.0), space));
        }
        ui.ctx().data_mut(|d| d.insert_temp(shown_id, r.height()));
    });
    if let Some(l) = new_limit {
        ui.ctx().data_mut(|d| d.insert_temp(limit_id, l));
    }
    // Zooms to the feature shown, found by its id, as Zoom To Selection does: the selection and
    // the undo history are left as they are.
    if let (true, Some((id, fid))) = (zoom, shown) {
        let extent = app
            .session
            .map()
            .layer(id)
            .and_then(|l| l.dataset.features.iter().find(|f| f.id == fid))
            .and_then(|f| f.geometry.as_ref())
            .map(|g| g.extent());
        if let Some(e) = extent.filter(|e| !e.is_empty()) {
            app.session.zoom_to(&e);
        }
    }
    if close {
        app.map.popup = None;
    } else if let (Some(i), Some(pp)) = (step, &mut app.map.popup) {
        pp.index = i;
        // The feature shown is the one selected, as after the click. Not through `select.rows`:
        // browsing a pop-up mustn't fill the undo history; the new revision redraws the map.
        // Found again by its feature id: a streamed layer may have read another window since the
        // click, moving the feature to another row (or out of the layer: then nothing is selected).
        let shown = pp.info["results"].get(i).and_then(|r| Some((u32::try_from(r["layerId"].as_u64()?).ok()?, r["fid"].as_u64()?)));
        let found = shown.and_then(|(id, fid)| {
            let row = app.session.map().layer(id)?.dataset.features.iter().position(|f| f.id == fid)?;
            Some((id, row))
        });
        if let Some((id, row)) = found {
            rustgis_engine::cmd::select::set(&mut app.session, id, vec![row], rustgis_engine::cmd::select::Mode::New);
        } else {
            app.session.map_mut().clear_selection();
        }
        app.session.revision = app.session.revision.wrapping_add(1);
    }
}

/// Whether an attribute value is a picture (a `data:image/png` or `jpeg` address, as a photo's
/// thumbnail is kept).
pub(crate) fn is_picture(v: &str) -> bool {
    v.starts_with("data:image/jpeg;base64,") || v.starts_with("data:image/png;base64,")
}

/// The texture of a picture value, decoded once and kept while the same value is shown.
fn picture_texture(app: &mut RustGisApp, ctx: &egui::Context, value: &str) -> Result<egui::TextureHandle, String> {
    use base64::Engine;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    let key = h.finish();
    if let Some((k, t)) = &app.map.picture
        && *k == key
    {
        return t.clone();
    }
    // Pictures kept with features are small (thumbnails); anything large isn't decoded here.
    let decoded = (|| {
        let data = value.split_once(",").map(|(_, d)| d).ok_or("not a picture")?;
        if data.len() > 4 << 20 {
            return Err("the picture is too large to show".to_string());
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
        let img = decode_picture(&bytes)?;
        let (w, h) = (img.width() as usize, img.height() as usize);
        let color = egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw());
        Ok(ctx.load_texture("identify_picture", color, egui::TextureOptions::LINEAR))
    })();
    app.map.picture = Some((key, decoded.clone()));
    decoded
}

/// A picture kept with a feature, decoded for the identify popup. A project's value is hostile:
/// its size is checked before anything is decoded, so a small compressed picture can't unpack
/// into a huge one.
fn decode_picture(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 << 20);
    reader.limits(limits);
    Ok(reader.decode().map_err(|_| "the picture is too large to show, or isn't one".to_string())?.to_rgba8())
}

/// The on-screen navigator: a north compass, zoom in and out, and four pan arrows.
fn navigator(app: &mut RustGisApp, ui: &mut Ui, p: &egui::Painter, rect: Rect, t: &Tokens) {
    let c = pos2(rect.min.x + 52.0, rect.min.y + 112.0);
    p.circle_filled(c, 34.0, t.ribbon.gamma_multiply(0.92));
    p.circle_stroke(c, 34.0, Stroke::new(1.0, t.border_strong));
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    // Each button is an egui widget above the map, so its clicks are its own (not the map's
    // identify, select or sketch) and popups over it get theirs first.
    let button = |ui: &mut Ui, r: Rect, id: &str| ui.interact(r, ui.id().with(("navigator", id)), Sense::click()).clicked();
    for (dir, (dx, dy)) in [("n", (0.0, -1.0)), ("s", (0.0, 1.0)), ("e", (1.0, 0.0)), ("w", (-1.0, 0.0))] {
        let tip = c + vec2(dx, dy) * 28.0;
        let base = c + vec2(dx, dy) * 18.0;
        let side = vec2(-dy, dx) * 6.0;
        let col = if dir == "n" { Color32::from_rgb(0xD1, 0x3C, 0x3C) } else { t.text_dim };
        p.add(egui::Shape::convex_polygon(vec![tip, base + side, base - side], col, Stroke::NONE));
        if button(ui, Rect::from_center_size(c + vec2(dx, dy) * 23.0, vec2(18.0, 18.0)), dir) {
            pan(app, -dx as f64 * w * 0.25, -dy as f64 * h * 0.25, w, h);
            app.session.record_extent();
        }
    }
    p.text(c, Align2::CENTER_CENTER, "N", semibold(11.0), t.text);
    for (k, (label, f)) in [("+", 2.0), ("−", 0.5)].into_iter().enumerate() {
        let r = Rect::from_center_size(pos2(c.x, c.y + 52.0 + k as f32 * 26.0), vec2(24.0, 22.0));
        p.rect_filled(r, CornerRadius::same(5), t.ribbon.gamma_multiply(0.92));
        p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, label, semibold(14.0), t.text);
        if button(ui, r, label) {
            zoom(app, f, w / 2.0, h / 2.0, w, h);
            app.session.record_extent();
        }
    }
}

/// View tabs above the map, one per map in the project, like document tabs.
fn view_tabs(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.tab_strip);
    ui.painter().line_segment([pos2(r.min.x, r.max.y - 0.5), pos2(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, t.border));
    let mut x = r.min.x + 6.0;
    let names: Vec<String> = app.session.project.maps.iter().map(|m| m.name.clone()).collect();
    let active = if app.session.active_layout.is_some() { usize::MAX } else { app.session.project.active_map };
    // A tab's close button asks to delete its map or layout.
    let mut close: Option<Deletion> = None;
    for (i, n) in names.iter().enumerate() {
        let w = crate::widgets::text_w(ui, n, regular(12.0)) + 46.0 + TAB_CLOSE_W;
        let tr = Rect::from_min_size(pos2(x, r.min.y + 4.0), vec2(w, r.height() - 4.0));
        let resp = ui.interact(tr, egui::Id::new(("maptab", i)), Sense::click());
        let is = i == active;
        // A project keeps at least one map (`map.remove` refuses the last).
        let last = names.len() < 2;
        let tip = if last { tl!("A project keeps at least one map") } else { tl!("Delete Map") };
        if is {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.ribbon);
            ui.painter().rect_filled(
                Rect::from_min_max(tr.min, pos2(tr.max.x, tr.min.y + 2.0)),
                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                t.accent,
            );
        } else if resp.contains_pointer() {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.hover);
        }
        icons::paint(ui.painter(), Rect::from_center_size(pos2(tr.min.x + 15.0, tr.center().y), vec2(15.0, 15.0)), "map", t.icon, t.accent);
        ui.painter().text(
            pos2(tr.min.x + 28.0, tr.center().y),
            Align2::LEFT_CENTER,
            n,
            if is { semibold(12.0) } else { regular(12.0) },
            if is { t.text } else { t.text_dim },
        );
        // Over the tab's fill, so painted after it.
        if tab_close(ui, t, tr, egui::Id::new(("maptab_close", i)), is, !last, tip) {
            close = Some(Deletion::Map(n.clone()));
        }
        if resp.clicked() && !is {
            let _ = app.run("layout.close", Value::Null);
            let _ = app.run("map.activate", json!({"name": n}));
            app.map.popup = None;
        }
        x += w + 2.0;
    }
    // Layouts, after the maps.
    let layouts: Vec<String> = app.session.project.layouts.iter().map(|l| l.name.clone()).collect();
    for (i, n) in layouts.iter().enumerate() {
        let w = crate::widgets::text_w(ui, n, regular(12.0)) + 46.0 + TAB_CLOSE_W;
        let tr = Rect::from_min_size(pos2(x, r.min.y + 4.0), vec2(w, r.height() - 4.0));
        let resp = ui.interact(tr, egui::Id::new(("layouttab", i)), Sense::click());
        let is = app.session.active_layout == Some(i);
        if is {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.ribbon);
            ui.painter().rect_filled(
                Rect::from_min_max(tr.min, pos2(tr.max.x, tr.min.y + 2.0)),
                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                t.accent,
            );
        } else if resp.contains_pointer() {
            ui.painter().rect_filled(tr, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, t.hover);
        }
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(tr.min.x + 15.0, tr.center().y), vec2(15.0, 15.0)),
            "new_layout",
            t.icon,
            t.accent,
        );
        ui.painter().text(
            pos2(tr.min.x + 28.0, tr.center().y),
            Align2::LEFT_CENTER,
            n,
            if is { semibold(12.0) } else { regular(12.0) },
            if is { t.text } else { t.text_dim },
        );
        // Over the tab's fill, so painted after it.
        if tab_close(ui, t, tr, egui::Id::new(("layouttab_close", i)), is, true, tl!("Delete Layout")) {
            close = Some(Deletion::Layout(n.clone()));
        }
        if resp.clicked() && !is {
            let _ = app.run("layout.activate", json!({"name": n}));
            app.ui.tab = "Publish".into();
        }
        x += w + 2.0;
    }
    let pr = Rect::from_min_size(pos2(x + 2.0, r.min.y + 6.0), vec2(22.0, 20.0));
    let resp = ui.interact(pr, egui::Id::new("maptab_new"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(pr, CornerRadius::same(4), t.hover);
    }
    ui.painter().text(pr.center(), Align2::CENTER_CENTER, "+", regular(15.0), t.text_dim);
    if resp.on_hover_text(tl!("New Map")).clicked() {
        let _ = app.run("map.new", Value::Null);
    }
    if close.is_some() {
        app.map.deleting = close;
    }
}

/// A map or a layout, by name, to delete once the user confirms ([`confirm_delete`]).
#[derive(Clone)]
pub enum Deletion {
    Map(String),
    Layout(String),
}

/// Asks before deleting the map or layout in `app.map.deleting` (tab close buttons, the
/// Browser): a whole map or layout goes in one click. The commands themselves never ask.
pub fn confirm_delete(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(d) = app.map.deleting.clone() else { return };
    // Gone meanwhile (a command, the assistant), or the last map now: nothing left to ask.
    let p = &app.session.project;
    let possible = match &d {
        Deletion::Map(n) => p.maps.len() > 1 && p.maps.iter().any(|m| m.name.eq_ignore_ascii_case(n)),
        Deletion::Layout(n) => p.layouts.iter().any(|l| l.name.eq_ignore_ascii_case(n)),
    };
    if !possible {
        app.map.deleting = None;
        return;
    }
    let t = Tokens::get(ctx);
    let title = match &d {
        Deletion::Map(n) => crate::i18n::fmt("Delete the map {name}?", &[("name", n)]),
        Deletion::Layout(n) => crate::i18n::fmt("Delete the layout {name}?", &[("name", n)]),
    };
    let undo = crate::widgets::shortcut_text(app, "edit.undo");
    let mut decision = None;
    egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(egui::RichText::new(title).font(semibold(15.0)));
        if !undo.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(crate::i18n::fmt("You can undo this with {shortcut}.", &[("shortcut", &undo)])).color(t.text_dim));
        }
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            if crate::widgets::dialog_button(ui, tl!("Delete"), Some(t.red), vec2(84.0, 28.0)).clicked() {
                decision = Some(true);
            }
            if crate::widgets::dialog_button(ui, tl!("Cancel"), None, vec2(84.0, 28.0)).clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                decision = Some(false);
            }
        });
    });
    match decision {
        Some(true) => {
            app.map.deleting = None;
            match d {
                Deletion::Map(n) => delete_map(app, &n),
                Deletion::Layout(n) => {
                    let _ = app.run("layout.remove", json!({"name": n}));
                }
            }
        }
        Some(false) => app.map.deleting = None,
        None => {}
    }
}

/// The map shown side by side, while its name finds one (a deleted or renamed map's doesn't).
pub(crate) fn split_map(app: &RustGisApp) -> Option<usize> {
    let n = app.ws.split.as_deref()?;
    app.session.project.maps.iter().position(|m| m.name.eq_ignore_ascii_case(n))
}

/// Deletes a map (`map.remove`); the popup goes with the active map, even behind an open layout.
pub(crate) fn delete_map(app: &mut RustGisApp, name: &str) {
    let p = &app.session.project;
    let was_active = p.maps.get(p.active_map).is_some_and(|m| m.name.eq_ignore_ascii_case(name));
    if app.run("map.remove", json!({"name": name})).is_ok() && was_active {
        app.map.popup = None;
    }
}

/// Room a view tab keeps at its right for the close button.
const TAB_CLOSE_W: f32 = 18.0;

/// A view tab's close button (×), shown on the tab in view and on the one under the pointer;
/// `enabled: false` dims it and only explains why (`tip`). True when clicked.
fn tab_close(ui: &mut Ui, t: &Tokens, tab: Rect, id: egui::Id, in_view: bool, enabled: bool, tip: &str) -> bool {
    // Hidden, it takes no clicks either: they open the tab (a touch screen has no hover).
    if !(in_view || ui.rect_contains_pointer(tab)) {
        return false;
    }
    let cr = Rect::from_center_size(pos2(tab.max.x - 13.0, tab.center().y), vec2(16.0, 16.0));
    // Added after the tab, so it sits on top: its click (even dimmed) doesn't also open the tab.
    let resp = ui.interact(cr, id, Sense::click());
    if enabled && resp.hovered() {
        ui.painter().rect_filled(cr, CornerRadius::same(3), t.pressed);
    }
    let ink = if !enabled {
        t.text_disabled
    } else if resp.hovered() {
        t.text
    } else {
        t.text_dim
    };
    icons::paint(ui.painter(), cr.shrink(3.0), "close", ink, t.accent);
    let resp = resp.on_hover_text(tip);
    enabled && resp.clicked()
}

#[cfg(test)]
mod picture_tests {
    #[test]
    fn large_pictures_are_refused_before_decoding() {
        let png = |w: u32, h: u32| {
            let mut out = Vec::new();
            image::RgbaImage::new(w, h).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            out
        };
        assert!(super::decode_picture(&png(160, 120)).is_ok());
        // A blank picture compresses to a few KB, but would unpack into far more than shown.
        let bomb = png(4000, 4000);
        assert!(bomb.len() < 4 << 20, "{}", bomb.len());
        assert!(super::decode_picture(&bomb).is_err());
        assert!(super::decode_picture(b"not a picture").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_counts_the_last_second_and_idle_is_none() {
        let mut mv = MapView::default();
        assert_eq!(mv.fps(0.0), None, "no frames yet");
        for i in 0..10 {
            mv.record_frame(i as f64 * 0.05);
        }
        assert_eq!(mv.fps(0.46), Some(10), "all ten frames inside the window");
        // A second burst later: the first burst has aged out of the 1 s window.
        for i in 0..10 {
            mv.record_frame(1.2 + i as f64 * 0.05);
        }
        assert_eq!(mv.fps(1.7), Some(10), "only the new burst counts");
        assert_eq!(mv.fps(2.3), None, "idle (no frame in 500 ms)");
    }
}
