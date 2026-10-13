//! The GPU map backend: plays a [`rustgis_engine::render::gpu_scene::GpuScene`] inside egui's
//! wgpu render pass (the pattern SpatialOS ships as `VectorCallback`). This module starts
//! with the pure camera/marker math; pipelines and the paint callback land next.
//!
//! `view_proj` maps origin-relative map-unit vertices to clip space and is golden-tested
//! against `Camera::to_screen` — the CPU path's own transform — to 1/100 px.

use rustgis_engine::map::Camera;

/// Clip-space transform for one draw command: vertices are f32 offsets from the command's
/// `origin` (map units), so the f64 `(origin - center)` subtraction happens on the CPU where
/// the precision still is. Matches `Camera::to_screen` exactly:
/// `clip.x = (v.x + origin.x - center.x) * 2/(res*w)`, `clip.y = (v.y + origin.y - center.y) * 2/(res*h)`.
pub(crate) fn view_proj(cam: &Camera, w: f64, h: f64, origin: [f64; 2]) -> [[f32; 4]; 4] {
    let sx = 2.0 / (cam.res * w);
    let sy = 2.0 / (cam.res * h);
    [
        [sx as f32, 0.0, 0.0, 0.0],
        [0.0, sy as f32, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [((origin[0] - cam.center.x) * sx) as f32, ((origin[1] - cam.center.y) * sy) as f32, 0.0, 1.0],
    ]
}

/// Host-side mirror of the marker vertex shader's displacement: a unit-space template point
/// becomes a pixel offset, then a clip offset. Templates are defined y-down (screen), clip y
/// is up — hence the negated y.
pub(crate) fn marker_displace(template: [f32; 2], radius_px: f32, viewport_px: [f32; 2]) -> [f32; 2] {
    [template[0] * radius_px * 2.0 / viewport_px[0], -template[1] * radius_px * 2.0 / viewport_px[1]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_engine::geom::Coord;

    fn assert_matches_to_screen(cam: Camera, w: f64, h: f64, p: Coord, origin: [f64; 2]) {
        let m = view_proj(&cam, w, h, origin);
        let (vx, vy) = ((p.x - origin[0]) as f32, (p.y - origin[1]) as f32);
        let cx = f64::from(m[0][0] * vx + m[3][0]);
        let cy = f64::from(m[1][1] * vy + m[3][1]);
        let (gx, gy) = ((cx + 1.0) * 0.5 * w, (1.0 - cy) * 0.5 * h);
        let (ex, ey) = cam.to_screen(p, w, h);
        assert!((gx - ex).abs() < 0.01 && (gy - ey).abs() < 0.01, "gpu ({gx},{gy}) vs cpu ({ex},{ey})");
    }

    #[test]
    fn matches_to_screen() {
        // World view, a city zoom, and a custom-projection's units (e.g. UTM metres).
        assert_matches_to_screen(Camera::world(), 1280.0, 720.0, Coord::new(2_611_702.0, 8_565_456.0), [0.0, 2_000_000.0]);
        let city = Camera { center: Coord::new(2_000_000.0, 6_000_000.0), res: 150.0 };
        assert_matches_to_screen(city, 1280.0, 720.0, Coord::new(2_013_400.0, 5_991_100.0), [2_000_000.0, 6_000_000.0]);
        let utm = Camera { center: Coord::new(500_000.0, 4_600_000.0), res: 25.0 };
        assert_matches_to_screen(utm, 800.0, 600.0, Coord::new(500_312.0, 4_599_511.0), [500_000.0, 4_600_000.0]);
    }

    #[test]
    fn origin_off_center_shifts_clip() {
        // A vertex 100 units right of the origin, origin 1000 units right of the camera:
        // clip.x = (100 + 1000) * 2/(res*w) = 1100 * 0.002.
        let cam = Camera { center: Coord::new(0.0, 0.0), res: 10.0 };
        let m = view_proj(&cam, 100.0, 100.0, [1000.0, 0.0]);
        let clip_x = f64::from(m[0][0] * 100.0 + m[3][0]);
        assert!((clip_x - 2.2).abs() < 1e-5);
    }

    #[test]
    fn marker_displacement_is_pixels_to_clip() {
        // 4 px radius at a 1280x720 viewport: 8 px of NDC width-2 → 8/1280*2.
        let [dx, dy] = marker_displace([1.0, -1.0], 4.0, [1280.0, 720.0]);
        assert!((dx - 0.00625).abs() < 1e-6);
        assert!(dy > 0.0, "template y is screen-down, clip y is up");
    }
}
