//! GPU-ready scene types: a `GpuScene` is what `render_backend(.., Backend::Gpu)` returns —
//! an ordered list of draw commands (painter's algorithm, no depth) that `ui-egui`'s `mapgpu`
//! module uploads to wgpu buffers and draws inside egui's render pass.
//!
//! World positions are f32 offsets from each command's `origin` (map units): the f64
//! subtraction happens on the CPU so city-zoom precision survives the f32 upload.

use crate::PlacedLabel;

/// A finished GPU scene: draw commands in bottom-first order plus the same side channels
/// (labels, attribution, pending flags) the CPU render returns in [`crate::Rendered`].
pub struct GpuScene {
    pub cmds: Vec<DrawCmd>,
    pub labels: Vec<PlacedLabel>,
    /// Labels that didn't fit, when the map shows unplaced labels (drawn in red).
    pub unplaced: Vec<PlacedLabel>,
    /// The credits the map's tile layers ask for.
    pub attribution: Vec<String>,
    /// Tiles are still on their way: drawing again later shows more.
    pub tiles_pending: bool,
    /// Streamed layers are still reading the view: drawing again later shows them.
    pub data_pending: bool,
    /// Tile layers that couldn't be drawn, and why.
    pub tile_errors: Vec<String>,
}

/// One draw command. `Image` is a CPU-rasterized run (basemap, tiles, rasters, and layers
/// that need tiny-skia: masks, blend modes, transparency, aggregation, extrusion) at its
/// exact z-position; `Mesh`/`Points` are lyon-tessellated vector layers.
pub enum DrawCmd {
    /// Screen-sized premultiplied RGBA, drawn covering the whole map view.
    Image { rgba: Vec<u8>, size: [u32; 2] },
    /// Tessellated polygon fills and line/ring strokes of one layer run.
    Mesh { origin: [f64; 2], vertices: Vec<MeshVertex>, indices: Vec<u32> },
    /// Point markers: instances of a small set of unit-space shape templates.
    Points { origin: [f64; 2], instances: Vec<PointInstance> },
}

/// Vertex for polygon fills and line strokes. Position is origin-relative map units.
/// Fill vertices carry `normal = [0, 0]` and `stroke_px = 0`; stroke vertices carry the
/// centerline position, the tessellator's unit normal, and the stroke width in physical
/// pixels, so the vertex shader keeps stroke width screen-constant at every zoom.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 2],
    pub normal: [f32; 2],
    /// Premultiplied RGBA, 0..1.
    pub color: [f32; 4],
    /// Stroke width in physical pixels (0 for fill vertices).
    pub stroke_px: f32,
    pub _pad: f32,
}

/// One marker draw: an instance of a unit-space shape template scaled to `radius_px` screen
/// pixels by the vertex shader. A CPU marker with an outline becomes two instances (stroke
/// under, fill over; the outer half of a centered stroke is what shows) and a selection halo
/// is a third, largest, first — painter's order within the layer.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointInstance {
    /// Origin-relative map units.
    pub position: [f32; 2],
    /// Premultiplied 0..1.
    pub color: [f32; 4],
    /// Shape radius in physical pixels.
    pub radius_px: f32,
    /// `rustgis_map::Marker` as u32 (0 Circle, 1 Square, 2 Triangle, 3 Diamond, 4 Star).
    pub shape: u32,
}
