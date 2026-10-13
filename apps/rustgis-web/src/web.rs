//! The browser shell: file picker, downloads, drag-and-drop, the unsaved-changes guard and the
//! eframe web runner.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use rustgis_ui_egui::{Inbox, RustGisApp, Services};

use crate::parts::Notes;
use wasm_bindgen::JsCast as _;

const DATA_EXTS: &[&str] = &[
    "geojson",
    // Geotagged photos (points in the Photos layer).
    "jpg",
    "jpeg",
    "json",
    "csv",
    "tsv",
    "txt",
    "xlsx",
    "xlsm",
    "xlsb",
    "xls",
    "ods",
    "rss",
    "atom",
    "georss",
    "xml",
    "landxml",
    "zip",
    "kml",
    "kmz",
    "gpx",
    "gpkg",
    "parquet",
    "geoparquet",
    "fgb",
    "mbtiles",
    "gml",
    "dxf",
    "mif",
    "pbf",
    "topojson",
    "las",
    "laz",
    "tif",
    "tiff",
    "asc",
];
const CANVAS_ID: &str = "rustgis_canvas";
const LOADING_ID: &str = "rustgis_loading";

pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let dirty = Rc::new(Cell::new(false));
        // Inside another page (`?embed`) the page holds the work, so leaving it doesn't ask.
        let embedded = flag(&query(), "embed");
        if !embedded && let Err(e) = guard_unload(dirty.clone()) {
            log::error!("no unsaved-changes guard: {e}");
        }
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    let inbox: Inbox = Inbox::default();
                    let font_inbox: Inbox = Inbox::default();
                    let notes = Notes::default();
                    let q = query();
                    let project = params(&q, "project", 1).into_iter().next();
                    let mut app = RustGisApp::with_sample().with_services(services(
                        inbox.clone(),
                        font_inbox,
                        notes.clone(),
                        cc.egui_ctx.clone(),
                        embedded,
                    ));
                    // Settings › Start-up, unless `?blank`, `?projection=EPSG:3010` (a preset id,
                    // `mercator`, `globe`) or `?extent=minLon,minLat,maxLon,maxLat` says otherwise
                    // for this visit.
                    let start = rustgis_ui_egui::Start {
                        blank: (embedded || project.is_some() || flag(&q, "blank")).then_some(true),
                        projection: param(&q, "projection").filter(|c| !c.is_empty()),
                        extent: param(&q, "extent").filter(|c| !c.is_empty()),
                    };
                    if let Err(e) = app.start(&start) {
                        log::warn!("start-up: {e}");
                    }
                    // `?lang=de` picks the interface language for this visit.
                    if let Some(code) = q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("lang=")) {
                        match rustgis_ui_egui::i18n::Lang::from_code(code) {
                            Some(l) => app.ui.language = l.code().to_string(),
                            None => log::warn!("?lang={code}: no such interface language; following the browser's"),
                        }
                    }
                    // `?project=<url>` opens a project (`.rgis`) by its web address: a whole map,
                    // its layers, symbology and view, shared as one link. It replaces the project
                    // when it lands, so the links that add to a project are left out beside it.
                    if let Some(url) = &project {
                        if let Err(e) = app.run("ui.openUrl", serde_json::json!({"url": url})) {
                            app.session.notify(format!("?project={url}: {e}"), true);
                        }
                        let skipped: Vec<&str> =
                            ["data", "stac", "style", "tiles", "zarr"].into_iter().filter(|k| !params(&q, k, 1).is_empty()).collect();
                        if !skipped.is_empty() {
                            let list = skipped.iter().map(|k| format!("?{k}=")).collect::<Vec<_>>().join(", ");
                            app.session.notify(format!("?project= opens a whole project, so {list} links were left out"), false);
                        }
                    }
                    let links = if project.is_some() { "" } else { q.as_str() };
                    // `?data=<url>` (repeatable) adds data by web address, downloaded in the
                    // background, the map zoomed to the first file that lands with something to
                    // show; `?style=<url>` adds a MapLibre style; `?tiles=<template>` a tile
                    // layer. Links that share a map. What fails is said in a notice.
                    for url in params(links, "data", 16) {
                        if let Err(e) = app.run("ui.addUrl", serde_json::json!({"url": url, "zoom": true})) {
                            app.session.notify(format!("?data={url}: {e}"), true);
                        }
                    }
                    // `?stac=<url>`: a STAC item (its default asset is added, signed when it is a
                    // Planetary Computer one) or a collection (STAC Search opens on it).
                    // One link: the dialog follows one document at a time, so others are said to
                    // be skipped rather than lost.
                    let stac = params(links, "stac", 4);
                    if let Some(url) = stac.first()
                        && let Err(e) = app.run("ui.stacLink", serde_json::json!({"url": url}))
                    {
                        app.session.notify(format!("?stac={url}: {e}"), true);
                    }
                    if stac.len() > 1 {
                        app.session.notify(format!("only the first ?stac= link is opened ({} more were left out)", stac.len() - 1), false);
                    }
                    for (key, kind) in [("style", Some("style")), ("tiles", None)] {
                        for url in params(links, key, 8) {
                            let mut p = serde_json::json!({"url": url});
                            if let Some(kind) = kind {
                                p["kind"] = serde_json::json!(kind);
                            }
                            if let Err(e) = app.run("map.addTiles", p) {
                                app.session.notify(format!("?{key}={url}: {e}"), true);
                            }
                        }
                    }
                    // `?zarr=<url>` opens Add Zarr Variable on a store, its variables listed.
                    // Read as `?data=` is, so a `+` in the address stays a `+`.
                    if let Some(url) = params(links, "zarr", 1).into_iter().next()
                        && let Err(e) = app.run("ui.addZarr", serde_json::json!({"address": url}))
                    {
                        app.session.notify(format!("?zarr={url}: {e}"), true);
                    }
                    // `?registry=<url>` opens the Plugin Manager on another plugin registry for this
                    // visit (a pull request's preview, say); installing still asks as usual.
                    if let Some(url) = param(&q, "registry").filter(|u| !u.is_empty())
                        && let Err(e) = app.run("ui.plugins", serde_json::json!({"open": true, "registry": url, "tab": "browse"}))
                    {
                        log::warn!("?registry={url}: {e}");
                    }
                    let embed = if embedded {
                        match Embed::listen(cc.egui_ctx.clone()) {
                            Ok(e) => Some(e),
                            Err(e) => {
                                log::error!("?embed: {e}");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    Ok(Box::new(WebShell { app, inbox, notes, dirty, embed }))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    el.set_inner_html(&format!("<p>RustGIS failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                }
            }
        }
    });
}

/// Up to `max` values of a query parameter that may repeat (`?data=a&data=b`), percent-decoded.
/// A `+` stays a `+` (it is one in a web address): a value holding `&`, `#` or a space must be
/// percent-encoded.
fn params(q: &str, key: &str, max: usize) -> Vec<String> {
    q.trim_start_matches('?')
        .split('&')
        .filter_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .filter(|raw| !raw.trim().is_empty())
        .take(max)
        .filter_map(|raw| js_sys::decode_uri_component(raw).ok().and_then(|s| s.as_string()))
        .filter(|v| !v.trim().is_empty())
        .collect()
}

/// A query parameter's value, percent-decoded.
fn param(q: &str, key: &str) -> Option<String> {
    let raw = q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?;
    js_sys::decode_uri_component(&raw.replace('+', " ")).ok().and_then(|s| s.as_string())
}

/// Whether a query flag is set (`?blank`, `?embed`).
fn flag(q: &str, key: &str) -> bool {
    q.trim_start_matches('?').split('&').any(|kv| kv == key)
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

/// Reads dropped files asynchronously (browsers can't read them synchronously) into the inbox,
/// and mirrors the unsaved-changes flag for the `beforeunload` guard.
struct WebShell {
    app: RustGisApp,
    inbox: Inbox,
    /// Notices from files read off the frame.
    notes: Notes,
    dirty: Rc<Cell<bool>>,
    embed: Option<Embed>,
}

/// The link to the page that embeds the app (`?embed`; see `embed.rs`).
struct Embed {
    parent: web_sys::Window,
    /// Messages from the parent, as JSON, waiting for the next frame.
    queue: Rc<RefCell<Vec<serde_json::Value>>>,
    /// The origin of the parent's first message: the only one answered.
    origin: Rc<RefCell<Option<String>>>,
    changes: crate::embed::Changes,
}

impl Embed {
    /// Takes messages from the parent window and tells it the app is ready.
    fn listen(ctx: egui::Context) -> Result<Embed, String> {
        let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
        let window = web_sys::window().ok_or("no window")?;
        let parent = window.parent().map_err(err)?.ok_or("no parent window")?;
        let (queue, origin) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(None::<String>)));
        let (q, o, from) = (queue.clone(), origin.clone(), parent.clone());
        let on_message = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            if !e.source().is_some_and(|s| js_sys::Object::is(&s, &from)) {
                return;
            }
            let mut origin = o.borrow_mut();
            if origin.as_deref().is_some_and(|known| known != e.origin()) {
                return;
            }
            let Some(text) = js_sys::JSON::stringify(&e.data()).ok().and_then(|t| t.as_string()) else {
                return;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return;
            };
            if crate::embed::parse(&value).is_some() {
                origin.get_or_insert_with(|| e.origin());
                q.borrow_mut().push(value);
                ctx.request_repaint();
            }
        });
        window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).map_err(err)?;
        on_message.forget();
        let embed = Embed { parent, queue, origin, changes: crate::embed::Changes::default() };
        // Nothing to hide in it, so it goes to whichever page embeds the app.
        embed.post(&crate::embed::ready(), "*");
        Ok(embed)
    }

    fn post(&self, msg: &serde_json::Value, origin: &str) {
        match js_sys::JSON::parse(&msg.to_string()) {
            Ok(v) => {
                if let Err(e) = self.parent.post_message(&v, origin) {
                    log::error!("couldn't post to the embedding page: {e:?}");
                }
            }
            Err(e) => log::error!("couldn't post to the embedding page: {e:?}"),
        }
    }

    /// Posts to the page that sent the first message; nothing before it has.
    fn answer(&self, msg: &serde_json::Value) {
        if let Some(origin) = self.origin.borrow().clone() {
            self.post(msg, &origin);
        }
    }

    /// Runs the page's messages, then posts the project once a change here has settled.
    fn exchange(&mut self, app: &mut RustGisApp, ctx: &egui::Context) {
        use crate::embed::{Msg, key, parse};
        let queued = std::mem::take(&mut *self.queue.borrow_mut());
        for value in queued {
            match parse(&value) {
                Some(Msg::Project(text)) => match app.run("project.openText", serde_json::json!({"text": text})) {
                    Ok(_) => self.changes.seen(key(&app.session)),
                    Err(e) => app.session.notify(e, true),
                },
                Some(Msg::Run { id, command, params }) => {
                    let outcome = if crate::embed::runnable(&command) {
                        app.run(&command, params)
                    } else {
                        Err(format!("{command}: only the app's own commands (ui.*) run from the embedding page"))
                    };
                    self.answer(&crate::embed::result(id, outcome));
                }
                None => {}
            }
        }
        match self.changes.due(key(&app.session), web_time::Instant::now()) {
            (true, _) => match serde_json::to_string(&app.session.project) {
                Ok(text) => self.answer(&crate::embed::project(&text)),
                Err(e) => log::error!("couldn't send the project: {e}"),
            },
            (false, Some(wait)) => ctx.request_repaint_after(wait),
            (false, None) => {}
        }
    }
}

impl eframe::App for WebShell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // The GPU map backend draws inside egui's wgpu pass; hand over the render state.
        self.app.set_render_state(frame.wgpu_render_state().cloned());
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for f in dropped {
            let (inbox, notes) = (self.inbox.clone(), self.notes.clone());
            let ctx = ctx.clone();
            let Some(file) = f.web_file().cloned() else {
                log::error!("a dropped file without its browser file");
                continue;
            };
            wasm_bindgen_futures::spawn_local(async move {
                let name = file.name();
                if let Some(bytes) = crate::parts::read_file(file, notes).await {
                    inbox.lock().unwrap_or_else(|e| e.into_inner()).push((name, bytes));
                }
                ctx.request_repaint();
            });
        }
        let notes = std::mem::take(&mut *self.notes.lock().unwrap_or_else(|e| e.into_inner()));
        for (text, error) in notes {
            self.app.session.notify(text, error);
        }
        if let Some(embed) = &mut self.embed {
            embed.exchange(&mut self.app, ctx);
        }
        self.app.logic(ctx);
        self.dirty.set(self.app.session.dirty);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Downloads `url`, or the byte range it asks for (a PMTiles archive's part: see
/// `rustgis_tiles::cache::split_range`).
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let window = web_sys::window().ok_or("no window")?;
    use rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache;
    // A data download may be up to MAX_DOWNLOAD; anything else is at most 64 MB.
    let (url, download) = cache::split_download(url);
    let max = if download { cache::MAX_DOWNLOAD as u64 } else { 64 << 20 };
    let (url, tail) = cache::split_tail(url);
    let (url, range) = cache::split_range(url);
    // An `s3://` address becomes its web address, and a request to an S3 connection's endpoint
    // is signed (the bucket's CORS rules must allow the Authorization and x-amz-* headers).
    let prepared = rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::s3::prepare(url)?;
    let url = prepared.url.as_str();
    let asked = match (range, tail) {
        (Some((a, b)), _) => Some(format!("bytes={a}-{b}")),
        (None, Some(n)) => Some(format!("bytes=-{n}")),
        (None, None) => None,
    };
    let promise = if asked.is_some() || !prepared.headers.is_empty() {
        let init = web_sys::RequestInit::new();
        let headers = web_sys::Headers::new().map_err(err)?;
        if let Some(r) = &asked {
            headers.set("Range", r).map_err(err)?;
        }
        for (k, v) in &prepared.headers {
            headers.set(k, v).map_err(err)?;
        }
        init.set_headers(&headers);
        // A signed request goes only where its credentials are for.
        if prepared.signed {
            init.set_redirect(web_sys::RequestRedirect::Error);
        }
        window.fetch_with_str_and_init(url, &init)
    } else {
        window.fetch_with_str(url)
    };
    // A refused fetch is an opaque TypeError ("Failed to fetch") by design: the browser doesn't
    // say why, so the likely reasons are named, with the browser's own message but not its stack.
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| {
            let why = e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())).unwrap_or_else(|| err(e));
            format!(
                "the browser couldn't reach it (offline, blocked by a browser extension or the network, or the server doesn't accept requests from web pages): {why}"
            )
        })?
        .dyn_into()
        .map_err(err)?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    // The last bytes asked for and the whole file answered: fine when it is no longer.
    let whole_tail = tail.filter(|_| resp.status() != 206);
    let max = match whole_tail {
        Some(n) => n.saturating_add(1),
        None => max,
    };
    // A range asked for and the whole file answered (200): fine for a range from the start that
    // holds the whole file (a small archive), checked once it's read; else no ranges.
    let whole = range.is_some() && resp.status() != 206;
    if range.is_some_and(|(a, _)| whole && a > 0) {
        return Err(format!("{url}: {}", cache::NO_RANGES));
    }
    // Fonts are at most about 10 MB and plugins 64 MB; refuse anything larger than allowed, by
    // header when there is one and by the downloaded size otherwise.
    if let Ok(Some(len)) = resp.headers().get("content-length").map(|v| v.and_then(|s| s.parse::<u64>().ok()))
        && len > max
    {
        // The last bytes asked for and a longer file answered: no ranges, so it is downloaded whole.
        if whole_tail.is_some() {
            return Err(format!("{url}: {}", cache::NO_RANGES));
        }
        return Err(format!("{len} bytes is too large"));
    }
    // Read a chunk at a time, so a body larger than allowed (whatever its header said) is cut
    // off as soon as it is, never held whole.
    // Sized by the header when there is one (checked above), so the chunks don't reallocate.
    let len = resp.headers().get("content-length").ok().flatten().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(len.min(max)).unwrap_or(0));
    if let Some(body) = resp.body() {
        let reader: web_sys::ReadableStreamDefaultReader =
            body.get_reader().dyn_into().map_err(|_| "the response has no readable body".to_string())?;
        loop {
            let chunk = wasm_bindgen_futures::JsFuture::from(reader.read()).await.map_err(err)?;
            if js_sys::Reflect::get(&chunk, &"done".into()).ok().and_then(|d| d.as_bool()).unwrap_or(true) {
                break;
            }
            let value = js_sys::Uint8Array::new(&js_sys::Reflect::get(&chunk, &"value".into()).map_err(err)?);
            let n = value.length() as usize;
            if (bytes.len() + n) as u64 > max {
                let _ = reader.cancel();
                if whole_tail.is_some() {
                    return Err(format!("{url}: {}", cache::NO_RANGES));
                }
                return Err(format!("larger than {} MB", max >> 20));
            }
            let at = bytes.len();
            bytes.resize(at + n, 0);
            value.copy_to(bytes.get_mut(at..).unwrap_or_default());
        }
    }
    if let Some((a, b)) = range.filter(|_| whole)
        && bytes.len() as u64 > b - a + 1
    {
        return Err(format!("{url}: {}", cache::NO_RANGES));
    }
    if whole_tail.is_some_and(|n| bytes.len() as u64 > n) {
        return Err(format!("{url}: {}", cache::NO_RANGES));
    }
    Ok(bytes)
}

/// Makes an HTTP request (the AI assistant's calls to its model), returning the response of any
/// status.
async fn http(req: rustgis_ui_egui::rustgis_assistant::HttpRequest) -> Result<rustgis_ui_egui::rustgis_assistant::HttpResponse, String> {
    use rustgis_ui_egui::rustgis_assistant::provider::{allowed_url, reply_limit};
    let limit = reply_limit(&req);
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    if !allowed_url(&req.url) {
        return Err(format!("{}: only HTTPS addresses, or plain HTTP on this computer or network, are allowed", req.url));
    }
    let window = web_sys::window().ok_or("no window")?;
    let init = web_sys::RequestInit::new();
    init.set_method(req.method);
    // The API key is never carried on to another address.
    init.set_redirect(web_sys::RequestRedirect::Error);
    let headers = web_sys::Headers::new().map_err(err)?;
    for (k, v) in &req.headers {
        headers.set(k, v).map_err(err)?;
    }
    init.set_headers(&headers);
    if let Some(body) = &req.body {
        init.set_body(&wasm_bindgen::JsValue::from_str(body));
    }
    // A failed fetch here is usually the provider refusing browser requests (CORS) or no network.
    let resp: web_sys::Response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str_and_init(&req.url, &init))
        .await
        .map_err(|_| "couldn't reach the provider (offline, or it doesn't accept requests from web pages)".to_string())?
        .dyn_into()
        .map_err(err)?;
    let status = resp.status();
    let request_id = resp.headers().get("x-request-id").ok().flatten();
    // Too large by its header: refused before it is read.
    if let Ok(Some(len)) = resp.headers().get("content-length").map(|v| v.and_then(|s| s.parse::<u64>().ok()))
        && len > limit as u64
    {
        return Err(format!("{len} bytes is too large"));
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(err)?).await.map_err(err)?;
    let bytes = js_sys::Uint8Array::new(&buf);
    if bytes.length() as usize > limit {
        return Err(format!("{} bytes is too large", bytes.length()));
    }
    Ok(rustgis_ui_egui::rustgis_assistant::HttpResponse { status, body: bytes.to_vec(), request_id })
}

/// The browser's services. Embedded, settings live in memory for the visit: the page that embeds
/// the app must not read what the user saved on the site.
fn services(inbox: Inbox, font_inbox: Inbox, notes: Notes, ctx: egui::Context, embedded: bool) -> Services {
    // Map tiles come through the browser's fetch (the app repaints when they arrive).
    rustgis_ui_egui::rustgis_engine::render::rustgis_tiles::cache::set_fetcher(|url, done| {
        wasm_bindgen_futures::spawn_local(async move { done(fetch_bytes(&url).await) });
    });
    let open_inbox = inbox.clone();
    let font_ctx = ctx.clone();
    let (gl_list_ctx, gl_run_ctx) = (ctx.clone(), ctx.clone());
    let font_sink = font_inbox.clone();
    let fetch_ctx = ctx.clone();
    let http_ctx = ctx.clone();
    let (load_pref, save_pref): (Box<dyn Fn(&str) -> Option<String>>, Box<dyn Fn(&str, &str)>) = if embedded {
        let prefs = Rc::new(RefCell::new(HashMap::<String, String>::new()));
        let saved = prefs.clone();
        (
            Box::new(move |key: &str| prefs.borrow().get(key).cloned()),
            Box::new(move |key: &str, value: &str| {
                saved.borrow_mut().insert(key.to_string(), value.to_string());
            }),
        )
    } else {
        (
            Box::new(|key: &str| storage().and_then(|s| s.get_item(&format!("rustgis.{key}")).ok().flatten())),
            Box::new(|key: &str, value: &str| {
                if let Some(s) = storage() {
                    let _ = s.set_item(&format!("rustgis.{key}"), value);
                }
            }),
        )
    };
    Services {
        load_pref: Some(load_pref),
        save_pref: Some(save_pref),
        fetch_font: Some(Box::new(move |url: &str| {
            let (url, inbox, ctx) = (url.to_string(), font_sink.clone(), font_ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_bytes(&url).await {
                    Ok(bytes) => {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((url, bytes));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        log::error!("couldn't load font {url}: {e}");
                        // Empty bytes tell the app the download failed.
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((url, Vec::new()));
                        ctx.request_repaint();
                    }
                }
            });
        })),
        font_inbox: Some(font_inbox),
        open_async: Some(Box::new(move |purpose: &str| {
            let inbox = open_inbox.clone();
            let notes = notes.clone();
            let ctx = ctx.clone();
            // A file picked for Add Delimited Text carries its dialog's options.
            // So does a file picked for a tool's file parameter, which goes to the Tools pane.
            let tool_exts = purpose.strip_prefix(rustgis_ui_egui::TOOL_FILE_PURPOSE);
            let mark = match (purpose, tool_exts) {
                ("text", _) => rustgis_ui_egui::DELIMITED_PICK,
                (_, Some(_)) => rustgis_ui_egui::TOOL_FILE_PICK,
                _ => "",
            };
            let dialog = if let Some(exts) = tool_exts {
                let exts: Vec<&str> = exts.split(',').filter(|e| !e.is_empty()).collect();
                if exts.is_empty() { rfd::AsyncFileDialog::new() } else { rfd::AsyncFileDialog::new().add_filter(exts.join(", "), &exts) }
            } else if purpose == "project" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS project", &["rgis", "rgisx"])
            } else if purpose == "item" {
                rfd::AsyncFileDialog::new().add_filter(
                    "RustGIS items",
                    &[
                        "rgismap",
                        "rgismpk",
                        "rgispkg",
                        "rgislyr",
                        "rgisstyle",
                        "rgisramp",
                        "rgistemplate",
                        "rgistask",
                        "rgisbookmarks",
                        "rgis",
                        "rgisx",
                    ],
                )
            } else if purpose == "ramp" {
                rfd::AsyncFileDialog::new().add_filter("Colour ramps", &["rgisramp", "xml", "qml", "txt", "clr", "cpt"])
            } else if purpose == "layout" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS layout", &["rgislayout"])
            } else if purpose == "plugin" {
                rfd::AsyncFileDialog::new().add_filter("RustGIS plugin", &["rgplugin"])
            } else if purpose == "text" {
                rfd::AsyncFileDialog::new()
                    .add_filter("Delimited text and spreadsheets", &["csv", "tsv", "txt", "xlsx", "xlsm", "xlsb", "xls", "ods"])
            } else if purpose == "picture" {
                rfd::AsyncFileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg"])
            } else {
                rfd::AsyncFileDialog::new().add_filter("Vector and raster data", DATA_EXTS)
            };
            // Data may come several files at once (photos gather in one layer).
            let many = purpose == "data";
            wasm_bindgen_futures::spawn_local(async move {
                let files: Vec<rfd::FileHandle> =
                    if many { dialog.pick_files().await.unwrap_or_default() } else { dialog.pick_file().await.into_iter().collect() };
                for file in files {
                    let name = file.file_name();
                    if let Some(bytes) = crate::parts::read_file(file.inner().clone(), notes.clone()).await {
                        inbox.lock().unwrap_or_else(|e| e.into_inner()).push((format!("{mark}{name}"), bytes));
                    }
                }
                ctx.request_repaint();
            });
        })),
        download: Some(Box::new(|name: &str, bytes: Vec<u8>| {
            if let Err(e) = download(name, &bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
        geolibre_manifests: Some(Box::new(move |slot| crate::geolibre::manifests(slot, gl_list_ctx.clone()))),
        geolibre_run: Some(Box::new(move |job, slot| crate::geolibre::run(job, slot, gl_run_ctx.clone()))),
        http: Some(Box::new(move |req, slot| {
            let ctx = http_ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let r = http(req).await.map_err(Into::into);
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                ctx.request_repaint();
            });
        })),
        fetch: Some(Box::new(move |url: &str, slot| {
            let (url, ctx) = (url.to_string(), fetch_ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let r = fetch_bytes(&url).await.map_err(|e| format!("{url}: {e}"));
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                ctx.request_repaint();
            });
        })),
    }
}

/// Saves bytes through a temporary `<a download>` link.
fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
    let err = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let document = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let props = web_sys::BlobPropertyBag::new();
    props.set_type("application/octet-stream");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(err)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(err)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor".to_string())?;
    a.set_href(&url);
    a.set_download(name);
    // Some browsers only follow links that are in the document.
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(err)?;
    a.click();
    a.remove();
    // Browsers may start the download after `click` returns, so free the URL a little later.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let window = web_sys::window().ok_or("no window")?;
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(err)?;
    Ok(())
}

/// Closing, reloading or leaving the tab with unsaved changes asks first.
fn guard_unload(dirty: Rc<Cell<bool>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let on_unload = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
        if dirty.get() {
            e.prevent_default();
            e.set_return_value("unsaved");
        }
    });
    window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref()).map_err(|e| format!("{e:?}"))?;
    on_unload.forget();
    Ok(())
}
