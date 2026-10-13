//! Headless UI screenshots: renders the whole RustGIS window offscreen (wgpu).
//!
//! `cargo run -p rustgis-ui-egui --example ui_shot -- script.jsonl`
//!
//! The script is JSON lines: `{"cmd": id, "params": {...}}` runs a command (ui.* included),
//! `{"shot": "/abs/out.png"}` saves the window, `{"steps": n}` runs frames, `{"sleep": ms}` runs frames
//! for that long, `{"click": label}` clicks the first widget whose label contains the text (with
//! `"exact": true`, whose whole label is the text), `{"dark": true}` switches theme,
//! `{"drag": [x, y, dx, dy]}` presses the pointer at (x, y) and moves it by (dx, dy), holding
//! it (shots then catch the map mid-drag; `{"release": true}` lets go),
//! `{"titlebar": {"left_inset": 80, "height": 34}}`
//! draws the header as a macOS unified title bar (`null` turns it off), and
//! `{"width": w, "height": h}` on the first line sets the size (default
//! 1480×900 at 2×). The window opens the World Atlas sample.

use egui_kittest::kittest::Queryable;
use rustgis_ui_egui::RustGisApp;

fn main() {
    let path = std::env::args().nth(1).expect("usage: ui_shot script.jsonl");
    let script = std::fs::read_to_string(&path).expect("read script");
    let lines: Vec<serde_json::Value> =
        script.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("json line")).collect();
    let w = lines.iter().find_map(|l| l.get("width").and_then(|v| v.as_f64())).unwrap_or(1480.0) as f32;
    let h = lines.iter().find_map(|l| l.get("height").and_then(|v| v.as_f64())).unwrap_or(900.0) as f32;
    let mut app = RustGisApp::with_sample();
    // One shared wgpu render state: the app's GPU map backend and the harness's renderer must
    // live on the same device for the map's paint callback to be valid.
    let rs = {
        let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
        let instance = pollster::block_on(setup.new_instance());
        pollster::block_on(egui_wgpu::RenderState::create(
            &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
            &instance,
            None,
            egui_wgpu::RendererOptions::PREDICTABLE,
        ))
        .expect("render state")
    };
    app.set_render_state(Some(rs.clone()));
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(w, h))
        .with_pixels_per_point(2.0)
        .with_max_steps(10_000)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
        .build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    for _ in 0..6 {
        harness.step();
    }
    for l in lines {
        if let Some(p) = l.get("shot").and_then(|v| v.as_str()) {
            for _ in 0..4 {
                harness.step();
            }
            match harness.render() {
                Ok(img) => {
                    img.save(p).expect("save png");
                    println!("{p}");
                }
                Err(e) => eprintln!("render failed: {e}"),
            }
        } else if let Some(ms) = l.get("sleep").and_then(|v| v.as_u64()) {
            // Real time passing, for work off the frame (downloads, the AI assistant's model).
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(100));
                harness.step();
            }
        } else if let Some(n) = l.get("steps").and_then(|v| v.as_u64()) {
            for _ in 0..n {
                harness.step();
            }
        } else if let Some(id) = l.get("cmd").and_then(|v| v.as_str()) {
            let params = l.get("params").cloned().unwrap_or(serde_json::Value::Null);
            if let Err(e) = harness.state_mut().run(id, params) {
                eprintln!("{id}: {e}");
            }
            for _ in 0..3 {
                harness.step();
            }
        } else if let Some(t) = l.get("titlebar") {
            let num = |k: &str| t.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            harness.state_mut().ui.titlebar = t.is_object().then(|| rustgis_ui_egui::TitlebarMetrics {
                left_inset: num("left_inset"),
                height: num("height"),
                ..rustgis_ui_egui::TitlebarMetrics::FALLBACK
            });
            for _ in 0..3 {
                harness.step();
            }
        } else if let Some(d) = l.get("dark").and_then(|v| v.as_bool()) {
            harness.state_mut().ui.dark = d;
            for _ in 0..3 {
                harness.step();
            }
        } else if let Some(label) = l.get("click").and_then(|v| v.as_str()) {
            // `"exact": true`: the label is the whole of it (an Add button, not Add Data).
            let node = if l.get("exact").and_then(|v| v.as_bool()) == Some(true) {
                harness.query_all_by_label(label).next()
            } else {
                harness.query_all_by_label_contains(label).next()
            };
            match node {
                Some(node) => node.click(),
                None => eprintln!("nothing labelled '{label}'"),
            }
            for _ in 0..3 {
                harness.step();
            }
        } else if let Some(d) = l.get("drag").and_then(|v| v.as_array()) {
            let n: Vec<f32> = d.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect();
            let [x, y, dx, dy] = n[..] else {
                eprintln!("drag takes [x, y, dx, dy]");
                continue;
            };
            let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            harness.event(egui::Event::PointerMoved(egui::pos2(x, y)));
            harness.event(button(egui::pos2(x, y), true));
            harness.step();
            for i in 1..=8 {
                let t = i as f32 / 8.0;
                harness.event(egui::Event::PointerMoved(egui::pos2(x + dx * t, y + dy * t)));
                harness.step();
            }
        } else if l.get("release").is_some() {
            if let Some(pos) = harness.ctx.input(|i| i.pointer.latest_pos()) {
                harness.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            for _ in 0..3 {
                harness.step();
            }
        } else if let Some(t) = l.get("tab").and_then(|v| v.as_str()) {
            harness.state_mut().ui.tab = t.to_string();
            for _ in 0..3 {
                harness.step();
            }
        }
    }
}
