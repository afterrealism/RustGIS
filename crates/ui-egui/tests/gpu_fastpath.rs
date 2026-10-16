//! The GPU camera fast path must yield one full re-render once the camera settles:
//! the scene's contents depend on the render-time camera (extent culling, the 0.35 px
//! decimation, label placement). Regression test for the PR #309 review (coderabbit +
//! github-actions, both flagging the missing settle render).

use rustgis_ui_egui::RustGisApp;
use serde_json::{Value, json};

/// A wgpu harness sharing one render state with the app (the ui_shot pattern), so the GPU
/// backend engages. `None` on hosts without an adapter.
fn harness() -> Option<egui_kittest::Harness<'static, RustGisApp>> {
    let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
    let instance = pollster::block_on(setup.new_instance());
    let rs = pollster::block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
        &instance,
        None,
        egui_wgpu::RendererOptions::PREDICTABLE,
    ))
    .ok()?;
    let mut app = RustGisApp::with_sample();
    app.set_render_state(Some(rs.clone()));
    app.run("ui.map.gpu", json!({"on": true})).expect("gpu on");
    Some(
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 550.0))
            .with_pixels_per_point(1.0)
            .with_max_steps(10_000)
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs))
            .build_ui_state(
                |ui, app: &mut RustGisApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            ),
    )
}

#[test]
fn settling_after_a_camera_animation_re_renders_fully() {
    let Some(mut h) = harness() else { return };
    for _ in 0..6 {
        h.step();
    }
    let app = h.state_mut();
    app.run("animation.add", Value::Null).expect("keyframe 1");
    app.run("map.setScale", json!({"scale": 20_000_000.0})).expect("zoom keyframe");
    app.run("animation.add", Value::Null).expect("keyframe 2");
    app.run("ui.animationPlay", Value::Null).expect("play");
    // Pump frames until the animation settles (playing is None again), then a few more.
    let mut settled = false;
    for _ in 0..9_000 {
        h.step();
        if h.state().ws.playing.is_none() {
            settled = true;
            break;
        }
    }
    assert!(settled, "the animation finished");
    for _ in 0..5 {
        h.step();
    }
    let app = h.state();
    let (cam, tex_cam) = (app.session.map().camera, app.map.full_frame_camera());
    assert_eq!((tex_cam.center, tex_cam.res), (cam.center, cam.res), "a full re-render followed the settled camera (tex_camera refreshed)");
}
