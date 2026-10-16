//! Runs frames of the whole UI (no GPU) through every tab, pane, dialog and theme.

use egui_kittest::kittest::Queryable;
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

fn harness() -> egui_kittest::Harness<'static, RustGisApp> {
    egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        RustGisApp::with_sample(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn mcp_relay_early_start_wakes_queued_request_and_updates_ui() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    let mut app = RustGisApp::with_sample();
    app.run("ui.mcpRelay", json!({"port": 0})).expect("start relay before first UI context");
    let port = app.relay.as_ref().expect("relay started").port();
    assert!(app.run("ui.mcpRelay", json!({"port": port})).is_ok());
    assert!(app.run("ui.mcpRelay", json!({})).is_ok());
    let different = if port == 1 { 2 } else { 1 };
    let err = app.run("ui.mcpRelay", json!({"port": different})).expect_err("different port must be refused");
    assert!(err.contains("stop it first"), "{err}");

    let token = rustgis_ui_egui::rustgis_assistant::relay::read_token(port).expect("relay token");
    let mut client = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).expect("connect to relay");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("bounded reply poll");
    writeln!(client, "{}", json!({"jsonrpc":"2.0","method":"rustgis/attach","params":{"access":false,"token":token}})).expect("attach");
    writeln!(client, "{}", json!({"jsonrpc":"2.0","id":1,"method":"ping"})).expect("queue ping");
    client.flush().expect("flush request");

    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    let mut input = BufReader::new(client.try_clone().expect("clone stream"));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut reply = String::new();
    loop {
        h.step();
        match input.read_line(&mut reply) {
            Ok(0) => panic!("relay closed before replying"),
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                assert!(Instant::now() < deadline, "queued request was not executed");
                reply.clear();
            }
            Err(e) => panic!("reading relay reply: {e}"),
        }
    }
    let value: serde_json::Value = serde_json::from_str(&reply).expect("JSON-RPC reply");
    assert_eq!(value["id"], 1);
    assert_eq!(value["result"], json!({}));

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.step();
        let attached = h.state().relay.as_ref().map(|r| r.attached_clients());
        if attached == Some(1) && h.query_by_label("MCP relay · 1 attached").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "attached state {attached:?} was not rendered as 1 attached");
        std::thread::yield_now();
    }
    assert_eq!(h.state().relay.as_ref().map(|r| r.attached_clients()), Some(1));
    assert!(h.query_by_label("MCP relay · 1 attached").is_some(), "authenticated-client indicator rendered");

    client.shutdown(std::net::Shutdown::Both).expect("disconnect attached client");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.step();
        let attached = h.state().relay.as_ref().map(|r| r.attached_clients());
        if attached == Some(0) && h.query_by_label("MCP relay · 0 attached").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "disconnected state {attached:?} was not rendered as 0 attached");
        std::thread::yield_now();
    }
    assert_eq!(h.state().relay.as_ref().map(|r| r.attached_clients()), Some(0));
    assert!(h.query_by_label("MCP relay · 0 attached").is_some(), "disconnect repaints the count");
    h.get_by_label("MCP relay · 0 attached").click();
    h.step();
    assert!(h.state().relay.is_none(), "clicking indicator stops relay");
    h.step();
    assert!(h.query_by_label("MCP relay · 0 attached").is_none());
}
#[test]
fn every_tab_pane_and_dialog_renders() {
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    for tab in ["Home", "Data", "Style", "Edit", "Analyze", "View", "Publish", "Plugins", "Settings", "Help"] {
        h.state_mut().ui.tab = tab.into();
        h.step();
        h.step();
    }
    for (id, p) in [
        ("ui.pane.symbology", json!(null)),
        ("ui.pane.geoprocessing", json!(null)),
        ("ui.gp.open", json!({"tool": "buffer"})),
        ("ui.pane.history", json!(null)),
        ("ui.pane.details", json!(null)),
        ("ui.pane.catalog", json!(null)),
        ("table.open", json!(null)),
        ("ui.dialog", json!({"name": "selectByAttributes"})),
        ("ui.dialog", json!({"name": "goToXY"})),
        ("ui.dialog", json!({"name": "calculateField"})),
        ("ui.commandSearch", json!(null)),
        ("ui.about", json!(null)),
        ("ui.shortcuts", json!(null)),
        ("ui.theme", json!({"dark": true})),
        ("ui.backstage", json!({"open": true})),
    ] {
        h.state_mut().run(id, p).unwrap_or_else(|e| panic!("{id}: {e}"));
        h.step();
        h.step();
    }
    h.state_mut().ui.backstage = false;
    // Every language, the collapsed and peeked ribbon.
    for l in rustgis_ui_egui::i18n::Lang::all() {
        h.state_mut().run("ui.language", json!({"code": l.code()})).unwrap();
        h.step();
        h.step();
    }
    h.state_mut().run("ui.language", json!({"code": "auto"})).unwrap();
    assert!(h.state_mut().run("ui.language", json!({"code": "xx"})).is_err());
    h.state_mut().run("ui.ribbon", json!({"minimized": true})).unwrap();
    h.state_mut().ui.ribbon_peek = true;
    h.step();
    h.state_mut().run("ui.ribbon", json!({"minimized": false})).unwrap();
    h.step();
    for kind in ["unique", "graduated", "single"] {
        h.state_mut().run("layer.symbology", json!({"layer": "Countries", "type": kind, "field": "continent"})).ok();
        h.step();
    }
    h.state_mut().run("layer.select", json!({"layer": "Cities"})).unwrap();
    h.state_mut().run("layer.symbology", json!({"type": "proportional", "field": "pop_max"})).unwrap();
    h.state_mut().run("project.new", json!(null)).unwrap();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.state().session.map().layers.is_empty());
}

/// Plugins are loaded for the whole process, so the tests that look at them take turns.
static PLUGINS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_ribbon_wider_than_the_window_scrolls_with_arrow_buttons() {
    let mut h = harness();
    h.set_size(egui::vec2(900.0, 600.0));
    h.state_mut().ui.tab = "View".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the ribbon left").is_none());
    h.get_by_label("Scroll the ribbon right").click();
    for _ in 0..4 {
        h.step();
    }
    let _ = h.get_by_label("Scroll the ribbon left");
    // A window wide enough for every group shows no arrows.
    h.set_size(egui::vec2(3000.0, 600.0));
    h.state_mut().ui.tab = "Help".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the ribbon right").is_none());
    assert!(h.query_by_label("Scroll the tabs right").is_none());
}

#[test]
fn a_tab_strip_wider_than_the_window_scrolls_with_arrow_buttons() {
    let mut h = harness();
    h.set_size(egui::vec2(560.0, 500.0));
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs left").is_none());
    h.get_by_label("Scroll the tabs right").click();
    h.step();
    h.step();
    let _ = h.get_by_label("Scroll the tabs left");
    // Scrolling to the end reaches the last tab.
    for _ in 0..20 {
        let Some(right) = h.query_by_label("Scroll the tabs right") else { break };
        right.click();
        h.step();
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs right").is_none());
    h.get_by_label("Help").click();
    h.step();
    h.step();
    assert_eq!(h.state().ui.tab, "Help");
    // A tab opened off screen scrolls into view, so the left arrow goes away.
    h.state_mut().ui.tab = "Home".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs left").is_none());
}

#[test]
fn plugins_install_from_a_file_after_a_prompt() {
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rustgis-ui-plugins-{}", std::process::id()));
    rustgis_ui_egui::rustgis_engine::plugins::store::set_dir(Some(dir.join("installed")));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ui-test.rgplugin");
    std::fs::write(&file, rustgis_ui_egui::rustgis_engine::plugins::testing::package("ui-test", &["edit"])).unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().run("ui.installPlugin", json!({"path": file.display().to_string()})).unwrap();
    h.step();
    h.step();
    assert!(h.state().plugins.confirm.is_some(), "asks first");
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    h.get_by_label("Install").click();
    h.step();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_some());
    // Its commands are on the Plugins tab, and run from there.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    h.get_by_label("Hello").click();
    h.step();
    // Its tools: a button on the Plugins tab, and listed in the Tools pane.
    h.get_by_label("Make Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-test/m"));
    h.state_mut().gp.tool = None;
    h.step();
    h.step();
    // On the ribbon and in the pane's Plugins section.
    assert_eq!(h.query_all_by_label("Select Two").count(), 2);
    for tab in ["installed", "browse", "settings"] {
        // An unreachable registry is an error in the window, not a crash.
        h.state_mut().run("ui.plugins", json!({"open": true, "tab": tab, "registry": "http://localhost:9/registry.json"})).unwrap();
        h.step();
        h.step();
    }
    // The Tools pane has an Uninstall of its own when a GeoLibre runner is installed on this
    // machine: closed, the only one is the plugin's.
    h.state_mut().ui.right = None;
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "installed"})).unwrap();
    h.step();
    h.get_by_label("Uninstall").click();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plugins_can_have_a_tab_of_their_own() {
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    use rustgis_ui_egui::rustgis_engine::plugins as rp;
    let mut m: serde_json::Value = serde_json::from_str(&rp::testing::manifest("ui-tab", &[])).unwrap();
    // Its own names, so the other tests (which share the loaded plugins) don't see them.
    m["commands"][0]["label"] = json!("Tab Hello");
    m["tools"][0]["name"] = json!("Tab Points");
    m["tools"][1]["name"] = json!("Tab Select");
    m["tab"] = json!({"label": "Hello Tab", "groups": [
        {"label": "Things", "items": ["h", "tool:m", {"menu": "More", "items": ["e", "tool:x"]}]}
    ]});
    let pkg = rp::testing::zip_of(&[("plugin.json", m.to_string().as_bytes()), ("plugin.wasm", &rp::testing::wasm())]);
    rp::load(rp::Package::from_zip(&pkg).unwrap(), "test").unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    // Its tab is in the tab strip; its buttons are there, not on the Plugins tab.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    assert_eq!(h.query_all_by_label("Tab Points").count(), 0);
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    h.step();
    h.step();
    h.get_by_label("Tab Hello").click();
    h.step();
    h.get_by_label("Tab Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/m"));
    h.get_by_label("More").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Tab Select").click();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/x"));
    // Hidden, then shown again, from Settings › Customize › Ribbon.
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    h.step();
    assert!(h.state().ui.hidden_tabs.contains(&"plugin:ui-tab".to_string()));
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    // A disabled plugin's tab goes, and the ribbon falls back to the Plugins tab.
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    rp::set_enabled("ui-tab", false).unwrap();
    h.step();
    h.step();
    assert_eq!(h.state().ui.tab, "Plugins");
    rp::unload("ui-tab");
}

#[test]
fn browse_lists_thousands_of_plugins_quickly() {
    use rustgis_ui_egui::rustgis_engine::plugins::registry;
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let sha = "a".repeat(64);
    let list: Vec<serde_json::Value> = (0..5000)
        .map(|i| {
            let cat = ["Vector", "Raster", "Terrain"][i % 3];
            json!({"id": format!("fake-{i:04}"), "name": format!("Plugin {i}"), "version": "1.0.0",
                "description": format!("Does many things with data for plugin number {i}. ").repeat(3), "author": format!("Author {}", i % 97),
                "categories": [cat, "Utilities"], "package": format!("https://packages.example.com/fake-{i}.rgplugin"),
                "sha256": sha, "size": 123_456, "permissions": ["edit"]})
        })
        .collect();
    let entries = registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(entries.len(), 5000);
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().plugins.set_registry(Some(Ok(entries)));
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "browse"})).unwrap();
    h.step();
    let _ = h.get_by_label("5000 plugins");
    // Only the visible rows are laid out, so a frame stays cheap with thousands of plugins
    // (about 1 ms in a release build; it was over 100 ms when every card was drawn). The bound
    // is loose for debug builds on busy machines.
    let t0 = std::time::Instant::now();
    for _ in 0..10 {
        h.step();
    }
    let per_frame = t0.elapsed() / 10;
    assert!(per_frame < std::time::Duration::from_millis(100), "{per_frame:?} per frame");
    // Search and the category filter narrow the list.
    h.state_mut().plugins.filter = "plugin 4999".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1 of 5000 plugins");
    h.state_mut().plugins.filter = String::new();
    h.state_mut().plugins.category = "Terrain".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1666 of 5000 plugins");
    // A new registry of the same size replaces what is listed (the cache follows the registry,
    // not its allocation).
    h.state_mut().plugins.category = String::new();
    let two = |a: &str, b: &str| {
        let list: Vec<serde_json::Value> = [a, b]
            .iter()
            .map(|n| json!({"id": n.to_lowercase(), "name": n, "version": "1.0.0", "package": "https://packages.example.com/x.rgplugin", "sha256": sha}))
            .collect();
        registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap()
    };
    h.state_mut().plugins.set_registry(Some(Ok(two("Alpha", "Beta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Alpha");
    h.state_mut().plugins.set_registry(Some(Ok(two("Gamma", "Delta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Gamma");
    assert!(h.query_by_label("Alpha").is_none());
    h.state_mut().plugins.set_registry(None);
    h.state_mut().plugins.open = false;
}

/// Switching to a CJK language keeps drawing the previous one for the frame its fonts are
/// installed in (`set_fonts` takes effect a frame late), so no frame shows missing-glyph boxes.
#[test]
fn language_switch_waits_for_its_font() {
    let mut h = harness();
    h.state_mut().run("ui.language", json!({"code": "en"})).unwrap();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().lang().code(), "en");
    h.state_mut().run("ui.language", json!({"code": "zh"})).unwrap();
    assert_eq!(h.state().chosen_lang().code(), "zh");
    h.step();
    assert_eq!(h.state().lang().code(), "en", "fonts installed this frame aren't active yet");
    h.step();
    assert_eq!(h.state().lang().code(), "zh");
    // Back to an alphabetic language: nothing to wait for.
    h.state_mut().run("ui.language", json!({"code": "de"})).unwrap();
    h.step();
    assert_eq!(h.state().lang().code(), "de");
}

/// Settings › Start-up (discussion #81): the preferences are saved, a later start follows them,
/// a shell's overrides win, and the start is neither an edit to undo nor unsaved work.
#[test]
fn start_up_preferences() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    let store: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let services = || {
        let (load, save) = (store.clone(), store.clone());
        rustgis_ui_egui::Services {
            load_pref: Some(Box::new(move |k| load.lock().unwrap().get(k).cloned())),
            save_pref: Some(Box::new(move |k, v| {
                save.lock().unwrap().insert(k.to_string(), v.to_string());
            })),
            ..Default::default()
        }
    };
    let mut app = RustGisApp::with_sample().with_services(services());
    assert!(app.run("ui.startup", json!({"projection": "nope"})).is_err());
    let r = app.run("ui.startup", json!({"blank": true, "projection": "epsg:3010"})).unwrap();
    assert_eq!(r, json!({"blank": true, "projection": "EPSG:3010", "extent": null}));
    assert_eq!(store.lock().unwrap().get("start_project").map(String::as_str), Some("blank"));

    let mut app = RustGisApp::with_sample().with_services(services());
    app.start(&Default::default()).unwrap();
    assert!(app.session.map().layers.is_empty());
    assert_eq!(app.session.map().projection.code(), "EPSG:3010");
    assert!(!app.session.dirty && !app.session.can_undo());

    // The web app's `?projection=robinson` without `?blank`: the sample still goes when the
    // preference says blank; an explicit `Some(false)` keeps it.
    let mut app = RustGisApp::with_sample().with_services(services());
    let start = |blank, projection: &str, extent: Option<&str>| rustgis_ui_egui::Start {
        blank,
        projection: Some(projection.into()),
        extent: extent.map(String::from),
    };
    app.start(&start(Some(false), "robinson", None)).unwrap();
    assert!(!app.session.map().layers.is_empty());
    assert_eq!(app.session.map().projection.code(), "robinson");
    assert!(app.start(&start(Some(false), "EPSG:4326", None)).is_err(), "degrees are refused");

    // A start extent: saved from the view, or given by the shell (`?extent=`).
    let mut app = RustGisApp::with_sample().with_services(services());
    app.start(&start(Some(true), "EPSG:3010", Some("10.5,55,24.5,69.5"))).unwrap();
    let view = app.session.run("map.extent", &json!(null)).unwrap();
    let e = rustgis_ui_egui::parse_extent(&view["extent"].as_array().unwrap().iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
        .unwrap();
    assert!(e[0] < 11.0 && e[2] > 24.0 && e[1] < 55.5 && e[3] > 69.0, "Sweden is in view: {e:?}");
    assert!(e[2] - e[0] < 90.0 && e[3] - e[1] < 25.0, "and not the world: {e:?}");
    assert!(app.start(&start(None, "EPSG:3010", Some("10,60,5,50"))).is_err(), "west after east");
    app.run("ui.startup", json!({"extent": "view"})).unwrap();
    assert!(store.lock().unwrap().get("start_extent").is_some_and(|v| v.split(',').count() == 4));
    let mut again = RustGisApp::with_sample().with_services(services());
    assert!(again.ui.start_extent.is_some());
    again.start(&Default::default()).unwrap();
    assert!(again.run("ui.startup", json!({"extent": [1, 2]})).is_err());
    again.run("ui.startup", json!({"extent": null})).unwrap();
    assert_eq!(store.lock().unwrap().get("start_extent").map(String::as_str), Some(""));

    // Web Mercator is the default, stored as nothing.
    app.run("ui.startup", json!({"blank": false, "projection": "mercator"})).unwrap();
    assert_eq!(store.lock().unwrap().get("start_projection").map(String::as_str), Some(""));
}

#[test]
fn old_and_unknown_tab_names_open_a_current_tab() {
    let mut h = harness();
    for (asked, opened) in [("Imagery", "Analyze"), ("Feature Layer", "Style"), ("Share", "Publish"), ("Map", "Home"), ("nope", "Home")] {
        let _ = h.state_mut().run("ui.tab", json!({"tab": asked}));
        assert_eq!(h.state().ui.tab, opened, "{asked}");
        h.step();
    }
}

/// Settings holds Customize › Ribbon, so it (like Help) can't be hidden.
#[test]
fn settings_and_help_tabs_always_show() {
    let mut h = harness();
    for tab in ["Settings", "Help"] {
        assert!(h.state_mut().run("ui.customizeRibbon", json!({"tab": tab})).is_err(), "{tab}");
    }
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "Edit"})).unwrap();
    assert_eq!(h.state().ui.hidden_tabs, vec!["Edit".to_string()]);
    h.state_mut().ui.tab = "Settings".into();
    h.step();
    h.step();
}

/// Add Delimited Text in the web app with a script's options: they go with the file picked next
/// (and only that one), so a projected CSV lands in place.
#[test]
fn delimited_text_options_go_with_the_picked_file() {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    let services = rustgis_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str| log.lock().unwrap().push(purpose.to_string()))),
        ..Default::default()
    };
    let mut app = RustGisApp::with_sample().with_services(services);
    app.run("ui.addDelimited", json!(null)).unwrap();
    app.run("ui.addDelimited", json!({"x": "east", "y": "north", "crs": "EPSG:2263", "delimiter": ";"})).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), ["text", "text"]);
    // A drop (or another picker's file) doesn't take the options.
    let r = app.open_picked("dropped.csv", b"lon,lat\n3,4\n").unwrap();
    let e = app.session.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().extent();
    assert_eq!((e.min_x, e.min_y), (3.0, 4.0));
    let picked = format!("{}parks.csv", rustgis_ui_egui::DELIMITED_PICK);
    let r = app.open_picked(&picked, b"name;east;north\nCentral Park;992000;220000\n").unwrap();
    let id = r["layer"].as_u64().unwrap() as u32;
    let e = app.session.map().layer(id).unwrap().extent();
    assert!((e.min_x + 73.96).abs() < 0.05 && (e.min_y - 40.78).abs() < 0.05, "{e:?}");
    // The options were used up: the next pick is read as it is.
    let picked = format!("{}plain.csv", rustgis_ui_egui::DELIMITED_PICK);
    let r = app.open_picked(&picked, b"lon,lat\n1,2\n").unwrap();
    let e = app.session.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().extent();
    assert_eq!((e.min_x, e.min_y), (1.0, 2.0));
}

/// Add Delimited Text in the web app: the file first, then the dialog lists its columns (the ones
/// found by name picked), and the ones picked there place it.
#[test]
fn delimited_text_dialog_lists_the_picked_files_columns() {
    let services = rustgis_ui_egui::Services { open_async: Some(Box::new(|_: &str| {})), ..Default::default() };
    let app = RustGisApp::with_sample().with_services(services);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.state_mut().run("ui.addDelimited", json!(null)).unwrap();
    let picked = format!("{}parks.csv", rustgis_ui_egui::DELIMITED_PICK);
    h.state_mut().open_picked(&picked, b"name;east;north\nCentral Park;992000;220000\n").unwrap();
    assert_eq!(h.state().dialogs.open, Some("addCsv"));
    assert_eq!(h.state().dialogs.list, ["name", "east", "north"]);
    assert_eq!(h.state().dialogs.mode, ";", "the separator found");
    assert!(h.state().dialogs.a.is_empty(), "east isn't a name RustGIS finds by itself");
    {
        let d = &mut h.state_mut().dialogs;
        (d.a, d.b, d.text) = ("east".into(), "north".into(), "EPSG:2263".into());
    }
    h.step();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add").click();
    h.step();
    h.step();
    assert_eq!(h.state().dialogs.open, None);
    let e = h.state().session.map().layer_by_name("parks").unwrap().extent();
    assert!((e.min_x + 73.96).abs() < 0.05 && (e.min_y - 40.78).abs() < 0.05, "{e:?}");
    // A file whose columns are found: picked for you.
    h.state_mut().run("ui.addDelimited", json!(null)).unwrap();
    let picked = format!("{}stops.csv", rustgis_ui_egui::DELIMITED_PICK);
    h.state_mut().open_picked(&picked, b"stop,Lat,Lon\nA,1,2\n").unwrap();
    assert_eq!((h.state().dialogs.a.as_str(), h.state().dialogs.b.as_str()), ("Lon", "Lat"));
}

/// Add Data From URL downloads off the frame and adds the file when it lands; a failed download
/// is said.
#[test]
fn data_from_a_web_address_lands_later() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let body = br#"{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [1, 2]}}"#;
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(body);
        }
    });
    let mut h = harness();
    h.step();
    let before = h.state().session.map().layers.len();
    h.state_mut().run("ui.addUrl", json!(null)).unwrap();
    h.step();
    h.state_mut().run("ui.addUrl", json!({"url": format!("http://{addr}/pin"), "name": "Pin"})).unwrap();
    h.state_mut().run("ui.addUrl", json!({"url": "http://169.254.169.254/x.geojson"})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.map().layer_by_name("Pin").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.map().layers.len(), before + 1);
    assert!(h.state().session.map().layer_by_name("Pin").is_some());
    assert!(h.state().session.notices.iter().any(|n| n.error && n.text.contains("never fetched")));
    assert!(h.state().ui.downloads.is_empty());
}

/// A project by its web address (a `?project=` link) downloads off the frame and replaces the
/// open project when it lands, its layers and view with it; a failed download is said and keeps
/// the open project.
#[test]
fn a_project_from_a_web_address_opens_when_it_lands() {
    use std::io::{Read, Write};
    let mut h = harness();
    h.step();
    let mut shared = h.state().session.project.clone();
    shared.name = "Shared map".into();
    let layers = shared.map().layers.len();
    let body = serde_json::to_string(&shared).unwrap().into_bytes();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    h.state_mut().run("project.new", json!({})).unwrap();
    assert!(h.state_mut().run("ui.openUrl", json!({})).is_err());
    // An API's address, without the extension, is still read as a project.
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/maps/7/project?v=2")})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.project.name == "Shared map" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.project.name, "Shared map");
    assert_eq!(h.state().session.map().layers.len(), layers);
    assert!(h.state().ui.project_download.is_none());
    h.state_mut().run("ui.openUrl", json!({"url": "http://169.254.169.254/a.rgis"})).unwrap();
    h.step();
    assert!(h.state().session.notices.iter().any(|n| n.error && n.text.contains("never fetched")));
    assert_eq!(h.state().session.project.name, "Shared map");
}

/// A project still downloading when another project is opened doesn't replace it; a pending item
/// picker isn't taken by the download either.
#[test]
fn a_late_project_download_leaves_the_newer_project_and_the_picker_alone() {
    use std::io::{Read, Write};
    let mut h = harness();
    h.step();
    let mut shared = h.state().session.project.clone();
    shared.name = "Late map".into();
    let body = serde_json::to_string(&shared).unwrap().into_bytes();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            // the first answer waits until the test has opened another project
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/late.rgis")})).unwrap();
    h.state_mut().run("project.new", json!({})).unwrap();
    let newer = h.state().session.project.name.clone();
    go.send(()).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().ui.project_download.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(h.state().ui.project_download.is_none());
    assert_eq!(h.state().session.project.name, newer);
    assert!(h.state().session.notices.iter().any(|n| n.text.contains("after another project was opened")));
    // With an item picker waiting, a downloaded project opens as a project and the picker keeps waiting.
    h.state_mut().ui.pending_open = Some("style.add");
    let _ = go.send(());
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/late.rgis")})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.project.name == "late" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.project.name, "late");
    assert_eq!(h.state().ui.pending_open, Some("style.add"));
}

/// Add From Service connects in the background and lists the service's layers in the dialog.
#[test]
fn a_service_is_listed_when_its_capabilities_land() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let body = br#"<WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink"><Service><Title>Local</Title></Service><Capability><Request><GetMap><Format>image/png</Format><DCPType><HTTP><Get><OnlineResource xlink:href="http://127.0.0.1/wms?"/></Get></HTTP></DCPType></GetMap></Request><Layer><CRS>EPSG:3857</CRS><Layer><Name>a</Name><Title>A</Title></Layer></Layer></Capability></WMS_Capabilities>"#;
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(body);
        }
    });
    let mut h = harness();
    h.state_mut().run("ui.addService", json!(null)).unwrap();
    h.step();
    h.state_mut().run("ui.addService", json!({"url": format!("http://{addr}/wms"), "type": "wms"})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().ui.service.as_ref().is_some_and(|c| c.listing.is_some() || c.error.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let conn = h.state().ui.service.as_ref().unwrap();
    assert_eq!(conn.error, None);
    assert_eq!(conn.listing.as_ref().unwrap()["layers"][0]["name"], "a");
    h.step();
}

/// Save in the web app (issue #197): the browser has no save dialog, so the first Save and every
/// Save As ask for the file name, and Save then downloads under it; an opened file keeps its name.
#[test]
fn web_save_asks_for_the_file_name() {
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    let log = got.clone();
    let services = rustgis_ui_egui::Services {
        download: Some(Box::new(move |name: &str, bytes: Vec<u8>| log.lock().unwrap().push((name.to_string(), bytes)))),
        ..Default::default()
    };
    let app = RustGisApp::with_sample().with_services(services);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    let names = |got: &std::sync::Mutex<Vec<(String, Vec<u8>)>>| got.lock().unwrap().iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    // A project never saved: Save asks first, with the project's name.
    h.state_mut().run("layer.color", json!({"layer": "Countries", "color": "#336699"})).unwrap();
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(h.state().dialogs.text, "World Atlas.rgis");
    assert!(names(&got).is_empty());
    h.step();
    h.state_mut().dialogs.text = "  ".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    assert_eq!(h.state().dialogs.open, Some("saveProject"), "an empty name is refused");
    assert!(h.state().dialogs.error.is_some() && names(&got).is_empty());
    h.state_mut().dialogs.text = "Trip plan".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    h.step();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got), ["Trip plan.rgis"]);
    assert_eq!(h.state().session.project.name, "Trip plan");
    assert!(!h.state().session.dirty);
    // Saved once: Save downloads under that name again without asking; Save As asks.
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got), ["Trip plan.rgis", "Trip plan.rgis"]);
    h.state_mut().run("ui.saveAs", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(h.state().dialogs.text, "Trip plan.rgis");
    h.state_mut().dialogs.open = None;
    // An opened file names the project, and Save keeps its name.
    let bytes = got.lock().unwrap()[0].1.clone();
    h.state_mut().open_picked("test.rgis", &bytes).unwrap();
    assert_eq!(h.state().session.project.name, "test");
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got).last().map(String::as_str), Some("test.rgis"));
    // Another project in its place hasn't been saved: Save asks again, even under the same name.
    let n = names(&got).len();
    h.state_mut().run("project.new", json!({"name": "test"})).unwrap();
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(names(&got).len(), n);
    // A name that can't be a file name is offered as one that can.
    h.state_mut().run("project.new", json!({"name": "Roads 2024/25"})).unwrap();
    h.state_mut().run("ui.saveAs", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.text, "Roads 2024-25.rgis");
}

/// Without a browser's downloads, Save Project As (opened by `ui.dialog`) saves nothing and
/// leaves the project as it was.
#[test]
fn save_project_dialog_needs_downloads() {
    let mut h = harness();
    h.state_mut().run("layer.color", json!({"layer": "Countries", "color": "#336699"})).unwrap();
    h.state_mut().run("ui.dialog", json!({"name": "saveProject"})).unwrap();
    h.step();
    h.state_mut().dialogs.text = "Elsewhere".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    assert!(h.state().dialogs.error.is_some());
    assert_eq!(h.state().session.project.name, "World Atlas");
    assert!(h.state().session.dirty);
}

#[test]
fn map_context_menu_copies_the_clicked_coordinates() {
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    // Right-click the middle of the map.
    let at = egui::pos2(700.0, 500.0);
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        });
    }
    for _ in 0..4 {
        h.step();
    }
    // Moving onto the menu leaves the map: the item must stay.
    h.get_by_label("Copy Coordinates").hover();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Copy Coordinates").click();
    h.step();
    let copied = h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t.contains(", ")));
    assert!(copied, "nothing copied");
}

#[test]
fn bookmarks_menu_removes_bookmarks() {
    let mut h = harness();
    h.state_mut().ui.tab = "Home".into();
    for _ in 0..4 {
        h.step();
    }
    let before = h.state().session.map().bookmarks.len();
    h.get_by_label("Bookmarks").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Remove Europe").click();
    for _ in 0..4 {
        h.step();
    }
    let map = h.state().session.map();
    assert_eq!(map.bookmarks.len(), before - 1);
    assert!(!map.bookmarks.iter().any(|b| b.name == "Europe"));
}

#[test]
fn dialogs_can_be_moved() {
    let mut h = harness();
    // A tab without a New Bookmark button: only the dialog's title matches.
    h.state_mut().ui.tab = "Help".into();
    let _ = h.state_mut().run("ui.dialog", json!({"name": "bookmark"}));
    for _ in 0..4 {
        h.step();
    }
    let before = h.get_by_label("New Bookmark").rect();
    // Drag the title bar.
    let from = before.left_top() + egui::vec2(150.0, 12.0);
    let to = from + egui::vec2(-200.0, 120.0);
    let press =
        |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(press(from, true));
    h.step();
    for i in 1..=5 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 5.0)));
        h.step();
    }
    h.input_mut().events.push(press(to, false));
    for _ in 0..4 {
        h.step();
    }
    let after = h.get_by_label("New Bookmark").rect();
    assert!((after.left_top() - (before.left_top() + (to - from))).length() < 2.0, "{before:?} → {after:?}");
}
