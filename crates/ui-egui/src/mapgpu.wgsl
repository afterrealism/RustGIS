//! GPU map shaders: polygon/line meshes with screen-constant stroke width, instanced marker
//! shapes, and full-view CPU-image compositing. Premultiplied alpha throughout (the CPU
//! renderer and egui agree on that). All textures and the egui target are linear
//! (`Rgba8Unorm`): values pass through with no conversion, matching the CPU path byte for byte.

struct CmdUniforms {
    view_proj: mat4x4f,
    /// Map units per physical pixel: scales stroke normals.
    map_per_px: f32,
    _pad: f32,
    /// Physical pixel size of the map view: scales marker pixel offsets.
    viewport: vec2f,
};

struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) color: vec4f,
};

// ---- mesh (polygon fills and line/ring strokes) ----

struct MeshIn {
    @location(0) position: vec2f,
    @location(1) normal: vec2f,
    @location(2) color: vec4f,
    @location(3) stroke_px: f32,
};

@group(0) @binding(0) var<uniform> u_mesh: CmdUniforms;

@vertex
fn vs_mesh(in: MeshIn) -> VOut {
    // Fill vertices have normal = (0,0) and stroke_px = 0: no displacement.
    let world = in.position + in.normal * (in.stroke_px * u_mesh.map_per_px * 0.5);
    var out: VOut;
    out.clip = u_mesh.view_proj * vec4f(world, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_mesh(in: VOut) -> @location(0) vec4f {
    return in.color;
}

// ---- markers (instanced unit-space shape templates) ----

// Five shapes padded to 72 vertices each (the circle's 24 fan triangles). Padding points sit
// at the shape centre: zero-area triangles, discarded by the rasteriser.
struct TemplateBank {
    verts: array<vec4f, 360>,
};

@group(0) @binding(0) var<uniform> u_mark: CmdUniforms;
@group(0) @binding(1) var<uniform> u_tpl: TemplateBank;

struct MarkIn {
    @builtin(vertex_index) vi: u32,
    @location(0) inst_pos: vec2f,
    @location(1) inst_color: vec4f,
    @location(2) inst_radius: f32,
    @location(3) inst_shape: u32,
};

@vertex
fn vs_marker(in: MarkIn) -> VOut {
    let tpl = u_tpl.verts[in.inst_shape * 72u + in.vi].xy;
    let center = u_mark.view_proj * vec4f(in.inst_pos, 0.0, 1.0);
    // Templates are y-down (screen), clip is y-up; view_proj is orthographic (w = 1).
    let offset = vec2f(tpl.x, -tpl.y) * in.inst_radius * 2.0 / u_mark.viewport;
    var out: VOut;
    out.clip = center + vec4f(offset, 0.0, 0.0);
    out.color = in.inst_color;
    return out;
}

@fragment
fn fs_marker(in: VOut) -> @location(0) vec4f {
    return in.color;
}

// ---- image (a CPU-rasterized run, composited covering the whole map view) ----

struct ImgOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_image(@builtin(vertex_index) vi: u32) -> ImgOut {
    var pos = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0),
    );
    let p = pos[vi];
    var out: ImgOut;
    out.clip = vec4f(p, 0.0, 1.0);
    // Pixmap row 0 is the top: clip y=+1 maps to texture v=0.
    out.uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@group(0) @binding(0) var t_img: texture_2d<f32>;
@group(0) @binding(1) var s_img: sampler;

@fragment
fn fs_image(in: ImgOut) -> @location(0) vec4f {
    return textureSample(t_img, s_img, in.uv);
}
