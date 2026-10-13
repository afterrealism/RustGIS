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
/// is up — hence the negated y. Test-only: the shader is the production side of the mirror.
#[cfg(test)]
pub(crate) fn marker_displace(template: [f32; 2], radius_px: f32, viewport_px: [f32; 2]) -> [f32; 2] {
    [template[0] * radius_px * 2.0 / viewport_px[0], -template[1] * radius_px * 2.0 / viewport_px[1]]
}

// ---- wgpu playback of a GpuScene (the SpatialOS `VectorCallback` pattern) ----

use std::sync::Arc;

use egui_wgpu::{CallbackResources, CallbackTrait, RenderState, wgpu};
use wgpu::util::DeviceExt;

use rustgis_engine::render::gpu_scene::{DrawCmd, GpuScene};

/// Vertices per marker shape slot in the template bank (the circle's 24 fan triangles fill it
/// exactly; smaller shapes pad with centre points — zero-area triangles are discarded).
const TEMPLATE_VERTS: usize = 72;
const SHAPES: usize = 5;

/// The five marker templates as triangle-fan lists (unit space, y-down like the CPU path's
/// tiny-skia marker shapes at `render::lib.rs` `Ctx::marker`).
fn marker_templates() -> [[f32; 4]; TEMPLATE_VERTS * SHAPES] {
    let mut out = [[0.0; 4]; TEMPLATE_VERTS * SHAPES];
    fn fan(outline: &[(f32, f32)], dst: &mut [[f32; 4]]) {
        for (i, &a) in outline.iter().enumerate() {
            let b = outline[(i + 1) % outline.len()];
            for (j, p) in [(0.0f32, 0.0f32), a, b].into_iter().enumerate() {
                if let Some(slot) = dst.get_mut(i * 3 + j) {
                    *slot = [p.0, p.1, 0.0, 0.0];
                }
            }
        }
    }
    // Circle: a 24-gon.
    let circle: Vec<(f32, f32)> = (0..24)
        .map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            (a.cos(), a.sin())
        })
        .collect();
    fan(&circle, &mut out[0..TEMPLATE_VERTS]);
    fan(&[(-0.9, -0.9), (0.9, -0.9), (0.9, 0.9), (-0.9, 0.9)], &mut out[TEMPLATE_VERTS..2 * TEMPLATE_VERTS]);
    fan(&[(0.0, -1.1), (1.0, 0.75), (-1.0, 0.75)], &mut out[2 * TEMPLATE_VERTS..3 * TEMPLATE_VERTS]);
    fan(&[(0.0, -1.15), (0.9, 0.0), (0.0, 1.15), (-0.9, 0.0)], &mut out[3 * TEMPLATE_VERTS..4 * TEMPLATE_VERTS]);
    let star: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 5.0 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { 1.2 } else { 0.5 };
            (r * a.cos(), r * a.sin())
        })
        .collect();
    fan(&star, &mut out[4 * TEMPLATE_VERTS..5 * TEMPLATE_VERTS]);
    out
}

/// Per-draw-call uniforms (WGSL `CmdUniforms`: 80 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CmdUniforms {
    view_proj: [[f32; 4]; 4],
    map_per_px: f32,
    _pad: f32,
    viewport: [f32; 2],
}

impl CmdUniforms {
    fn new(cam: &Camera, logical_size: [f64; 2], ppp: f32, origin: [f64; 2]) -> Self {
        CmdUniforms {
            view_proj: view_proj(cam, logical_size[0], logical_size[1], origin),
            map_per_px: (cam.res / f64::from(ppp)) as f32,
            _pad: 0.0,
            viewport: [(logical_size[0] * f64::from(ppp)) as f32, (logical_size[1] * f64::from(ppp)) as f32],
        }
    }
}

/// Premultiplied-alpha blending, the same arithmetic tiny-skia composites with.
const PREMULTIPLIED_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Pipelines, layouts and shared resources, created once per wgpu device.
pub(crate) struct GpuMapRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    cmd_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    templates: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

impl GpuMapRenderer {
    /// Builds the pipelines against egui's own target format (linear, 1× MSAA like egui's
    /// default — paint callbacks share egui's render pass and must match its attachment).
    pub(crate) fn new(rs: &RenderState) -> Self {
        let device = &rs.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mapgpu"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mapgpu.wgsl").into()),
        });
        let uniform_buf = |visibility: wgpu::ShaderStages, binding: u32, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let cmd_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapgpu_cmd"),
            entries: &[
                uniform_buf(wgpu::ShaderStages::VERTEX, 0, std::mem::size_of::<CmdUniforms>() as u64),
                uniform_buf(wgpu::ShaderStages::VERTEX, 1, (TEMPLATE_VERTS * SHAPES * 16) as u64),
            ],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mapgpu_image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let templates = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mapgpu_templates"),
            contents: bytemuck::cast_slice(&marker_templates()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mapgpu_image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let target = [Some(wgpu::ColorTargetState {
            format: rs.target_format,
            blend: Some(PREMULTIPLIED_BLEND),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = |label: &str, vs: &str, fs: &str, buffers: &[wgpu::VertexBufferLayout<'_>], layouts: &[&wgpu::BindGroupLayout]| {
            let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
            let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = buffers.iter().map(|b| Some(b.clone())).collect();
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &target,
                }),
                primitive: wgpu::PrimitiveState::default(), // triangle list, no culling
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(), // 1×, matching egui's target
                multiview_mask: None,
                cache: None,
            })
        };
        let mesh_buffers = [wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 32, shader_location: 3 },
            ],
        }];
        let point_buffers = [wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 28, shader_location: 3 },
            ],
        }];
        let mesh_pipeline = pipeline("mapgpu_mesh", "vs_mesh", "fs_mesh", &mesh_buffers, &[&cmd_layout]);
        let point_pipeline = pipeline("mapgpu_points", "vs_marker", "fs_marker", &point_buffers, &[&cmd_layout]);
        let image_pipeline = pipeline("mapgpu_image", "vs_image", "fs_image", &[], &[&image_layout]);
        GpuMapRenderer { mesh_pipeline, point_pipeline, image_pipeline, cmd_layout, image_layout, templates, sampler }
    }
}

/// One uploaded draw command.
pub(crate) enum SceneDraw {
    Mesh {
        vertices: wgpu::Buffer,
        indices: wgpu::Buffer,
        index_count: u32,
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
        origin: [f64; 2],
    },
    Points {
        instances: wgpu::Buffer,
        count: u32,
        uniforms: wgpu::Buffer,
        bind: wgpu::BindGroup,
        origin: [f64; 2],
    },
    Image {
        bind: wgpu::BindGroup,
        _texture: wgpu::Texture,
    },
}

/// A whole scene on the GPU, plus the view parameters needed to recompute per-draw uniforms
/// for a moved camera (`set_camera`).
pub(crate) struct SceneBuffers {
    pub draws: Vec<SceneDraw>,
    pub size: [f64; 2],
    pub ppp: f32,
    /// Scenes containing CPU-rasterized image runs can't fast-path a camera move (images are
    /// screen-space bitmaps of the old view) — they take the full re-render instead.
    pub has_images: bool,
}

impl SceneBuffers {
    pub(crate) fn from_scene(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        r: &GpuMapRenderer,
        scene: &GpuScene,
        camera: Camera,
        size: [f64; 2],
        ppp: f32,
    ) -> SceneBuffers {
        let mut draws = Vec::with_capacity(scene.cmds.len());
        let mut has_images = false;
        for cmd in &scene.cmds {
            match cmd {
                DrawCmd::Mesh { origin, vertices, indices } => {
                    if indices.is_empty() {
                        continue;
                    }
                    let uniforms = CmdUniforms::new(&camera, size, ppp, *origin);
                    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("mapgpu_cmd_uniforms"),
                        contents: bytemuck::bytes_of(&uniforms),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_mesh_bind"),
                        layout: &r.cmd_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: r.templates.as_entire_binding() },
                        ],
                    });
                    draws.push(SceneDraw::Mesh {
                        vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_mesh_vertices"),
                            contents: bytemuck::cast_slice(vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_mesh_indices"),
                            contents: bytemuck::cast_slice(indices),
                            usage: wgpu::BufferUsages::INDEX,
                        }),
                        index_count: indices.len() as u32,
                        uniforms,
                        bind,
                        origin: *origin,
                    });
                }
                DrawCmd::Points { origin, instances } => {
                    if instances.is_empty() {
                        continue;
                    }
                    let uniforms = CmdUniforms::new(&camera, size, ppp, *origin);
                    let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("mapgpu_cmd_uniforms"),
                        contents: bytemuck::bytes_of(&uniforms),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_points_bind"),
                        layout: &r.cmd_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: r.templates.as_entire_binding() },
                        ],
                    });
                    draws.push(SceneDraw::Points {
                        instances: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("mapgpu_point_instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        count: instances.len() as u32,
                        uniforms,
                        bind,
                        origin: *origin,
                    });
                }
                DrawCmd::Image { rgba, size: [w, h] } => {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("mapgpu_image"),
                        size: wgpu::Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        // Linear, like every egui texture: values pass through unconverted.
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        rgba,
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(*h) },
                        wgpu::Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
                    );
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("mapgpu_image_bind"),
                        layout: &r.image_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&r.sampler) },
                        ],
                    });
                    draws.push(SceneDraw::Image { bind, _texture: texture });
                    has_images = true;
                }
            }
        }
        SceneBuffers { draws, size, ppp, has_images }
    }

    /// Rewrites every command's camera uniforms for a moved camera — pan/zoom without
    /// re-tessellation (meshes re-project in the vertex shader). wgpu buffer writes go through
    /// the queue, so `&self` suffices. Callers check `has_images` first: image runs don't
    /// re-project and need a full re-render.
    pub(crate) fn set_camera(&self, queue: &wgpu::Queue, cam: &Camera) {
        for d in &self.draws {
            let (uniforms, origin) = match d {
                SceneDraw::Mesh { uniforms, origin, .. } | SceneDraw::Points { uniforms, origin, .. } => (uniforms, origin),
                SceneDraw::Image { .. } => continue,
            };
            let u = CmdUniforms::new(cam, self.size, self.ppp, *origin);
            queue.write_buffer(uniforms, 0, bytemuck::bytes_of(&u));
        }
    }
}

/// Draws the scene inside egui's render pass (painter's algorithm; no depth).
pub(crate) struct SceneCallback {
    pub renderer: Arc<GpuMapRenderer>,
    pub buffers: Arc<SceneBuffers>,
}

impl CallbackTrait for SceneCallback {
    fn paint(&self, _info: egui::PaintCallbackInfo, rp: &mut wgpu::RenderPass<'static>, _res: &CallbackResources) {
        for d in &self.buffers.draws {
            match d {
                SceneDraw::Mesh { vertices, indices, index_count, bind, .. } => {
                    rp.set_pipeline(&self.renderer.mesh_pipeline);
                    rp.set_bind_group(0, bind, &[]);
                    rp.set_vertex_buffer(0, vertices.slice(..));
                    rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..*index_count, 0, 0..1);
                }
                SceneDraw::Points { instances, count, bind, .. } => {
                    rp.set_pipeline(&self.renderer.point_pipeline);
                    rp.set_bind_group(0, bind, &[]);
                    rp.set_vertex_buffer(0, instances.slice(..));
                    rp.draw(0..TEMPLATE_VERTS as u32, 0..*count);
                }
                SceneDraw::Image { bind, .. } => {
                    rp.set_pipeline(&self.renderer.image_pipeline);
                    rp.set_bind_group(0, bind, &[]);
                    rp.draw(0..6, 0..1);
                }
            }
        }
    }
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

    #[test]
    fn marker_templates_match_cpu_shapes() {
        let t = marker_templates();
        // Circle: 24 fan triangles = 72 vertices filling the slot, on the unit ring or centre.
        assert!(t[1][0].abs() + t[1][1].abs() > 0.9, "circle starts on the unit ring");
        // Square: 4 triangles = 12 vertices, then centre padding.
        let sq = &t[TEMPLATE_VERTS..TEMPLATE_VERTS + 12];
        assert!(sq.iter().any(|p| p[0] == 0.9 && p[1] == 0.9));
        assert!(t[TEMPLATE_VERTS + 12..2 * TEMPLATE_VERTS].iter().all(|p| *p == [0.0; 4]), "padding stays at the centre");
        // Star's top point (y-down): (0, -1.2).
        let star = &t[4 * TEMPLATE_VERTS..5 * TEMPLATE_VERTS];
        assert!(star.iter().any(|p| (p[1] + 1.2).abs() < 1e-6), "star top point");
    }

    /// A headless wgpu render state (the egui_kittest recipe). `None` on hosts without any
    /// adapter — CI without a GPU skips these tests.
    fn headless_render_state() -> Option<RenderState> {
        let setup = egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew::without_display_handle());
        let instance = pollster::block_on(setup.new_instance());
        pollster::block_on(egui_wgpu::RenderState::create(
            &egui_wgpu::WgpuConfiguration { wgpu_setup: setup, ..Default::default() },
            &instance,
            None,
            egui_wgpu::RendererOptions::PREDICTABLE,
        ))
        .ok()
    }

    #[test]
    fn gpu_renderer_constructs_and_uploads_headless() {
        let Some(rs) = headless_render_state() else { return };
        let r = GpuMapRenderer::new(&rs);
        // A tiny scene uploads: one triangle, one marker instance, one CPU image.
        use rustgis_engine::render::gpu_scene::{MeshVertex, PointInstance};
        let v =
            |x: f32, y: f32| MeshVertex { position: [x, y], normal: [0.0, 0.0], color: [1.0, 0.0, 0.0, 1.0], stroke_px: 0.0, _pad: 0.0 };
        let scene = GpuScene {
            cmds: vec![
                DrawCmd::Mesh { origin: [0.0, 0.0], vertices: vec![v(0.0, 0.0), v(100.0, 0.0), v(0.0, 100.0)], indices: vec![0, 1, 2] },
                DrawCmd::Points {
                    origin: [0.0, 0.0],
                    instances: vec![PointInstance { position: [10.0, 10.0], color: [0.0, 1.0, 0.0, 1.0], radius_px: 4.0, shape: 0 }],
                },
                DrawCmd::Image { rgba: vec![128; 4], size: [1, 1] },
            ],
            labels: vec![],
            unplaced: vec![],
            attribution: vec![],
            tiles_pending: false,
            data_pending: false,
            tile_errors: vec![],
        };
        let sb = SceneBuffers::from_scene(&rs.device, &rs.queue, &r, &scene, Camera::world(), [400.0, 300.0], 1.0);
        assert_eq!(sb.draws.len(), 3);
    }
}
