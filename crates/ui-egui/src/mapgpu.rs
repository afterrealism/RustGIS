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

/// Pipelines, layouts and shared resources, created once per wgpu device. The scene draws
/// into an offscreen 4× MSAA target (anti-aliased like the CPU renderer); its resolved
/// texture is then blitted into egui's 1× pass — paint callbacks share egui's attachment and
/// must match its sample count, so the multisampling happens in our own pass.
pub(crate) struct GpuMapRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    cmd_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    templates: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// MSAA sample count of the offscreen scene target.
    samples: u32,
}

impl GpuMapRenderer {
    /// Builds the pipelines: the scene pipelines against the offscreen `Rgba8Unorm` MSAA
    /// target, the blit pipeline against egui's own (linear, 1×) target format.
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
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[wgpu::VertexBufferLayout<'_>],
                        layouts: &[&wgpu::BindGroupLayout],
                        format: wgpu::TextureFormat,
                        samples: u32| {
            let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
            let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = buffers.iter().map(|b| Some(b.clone())).collect();
            let target = [Some(wgpu::ColorTargetState { format, blend: Some(PREMULTIPLIED_BLEND), write_mask: wgpu::ColorWrites::ALL })];
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
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
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
        let samples = 4;
        let scene_format = wgpu::TextureFormat::Rgba8Unorm;
        let mesh_pipeline = pipeline("mapgpu_mesh", "vs_mesh", "fs_mesh", &mesh_buffers, &[&cmd_layout], scene_format, samples);
        let point_pipeline = pipeline("mapgpu_points", "vs_marker", "fs_marker", &point_buffers, &[&cmd_layout], scene_format, samples);
        let image_pipeline = pipeline("mapgpu_image", "vs_image", "fs_image", &[], &[&image_layout], scene_format, samples);
        // The blit pipeline draws the resolved scene texture into egui's pass (egui's target
        // is 1×, so a separate pipeline; the scene's own pipelines render into the 4× MSAA
        // offscreen target).
        let blit_pipeline = pipeline("mapgpu_blit", "vs_image", "fs_image", &[], &[&image_layout], rs.target_format, 1);
        GpuMapRenderer {
            mesh_pipeline,
            point_pipeline,
            image_pipeline,
            blit_pipeline,
            cmd_layout,
            image_layout,
            templates,
            sampler,
            samples,
        }
    }
}

/// The offscreen scene target: a 4× MSAA color attachment plus the plain texture it resolves
/// to (the blit source in egui's pass). Recreated when the map view's physical size changes.
pub(crate) struct SceneTarget {
    msaa_view: wgpu::TextureView,
    resolve_tex: wgpu::Texture,
    pub resolve_bind: wgpu::BindGroup,
    pub size: [u32; 2],
}

impl SceneTarget {
    pub(crate) fn new(device: &wgpu::Device, r: &GpuMapRenderer, size: [u32; 2]) -> Option<SceneTarget> {
        let [w, h] = size;
        if w == 0 || h == 0 {
            return None;
        }
        let msaa_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mapgpu_msaa"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: r.samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let resolve_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mapgpu_resolve"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let resolve_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mapgpu_resolve_bind"),
            layout: &r.image_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolve_tex.create_view(&wgpu::TextureViewDescriptor::default())),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&r.sampler) },
            ],
        });
        Some(SceneTarget { msaa_view: msaa_tex.create_view(&wgpu::TextureViewDescriptor::default()), resolve_tex, resolve_bind, size })
    }
}

/// Records the scene's MSAA render pass into `enc` (resolves into `tgt.resolve_tex`).
fn record_scene(r: &GpuMapRenderer, draws: &[SceneDraw], tgt: &SceneTarget, enc: &mut wgpu::CommandEncoder) {
    let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("mapgpu_scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &tgt.msaa_view,
            depth_slice: None,
            resolve_target: Some(&tgt.resolve_tex.create_view(&wgpu::TextureViewDescriptor::default())),
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
        })],
        ..Default::default()
    });
    paint_draws(r, draws, &mut rp);
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
    /// The offscreen MSAA target, persistent across frames (recreated on size changes).
    pub target: Arc<std::sync::Mutex<Option<SceneTarget>>>,
}

/// The draw loop, shared by the egui paint callback and the offscreen parity harness.
fn paint_draws(renderer: &GpuMapRenderer, draws: &[SceneDraw], rp: &mut wgpu::RenderPass<'_>) {
    for d in draws {
        match d {
            SceneDraw::Mesh { vertices, indices, index_count, bind, .. } => {
                rp.set_pipeline(&renderer.mesh_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.set_vertex_buffer(0, vertices.slice(..));
                rp.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..*index_count, 0, 0..1);
            }
            SceneDraw::Points { instances, count, bind, .. } => {
                rp.set_pipeline(&renderer.point_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.set_vertex_buffer(0, instances.slice(..));
                rp.draw(0..TEMPLATE_VERTS as u32, 0..*count);
            }
            SceneDraw::Image { bind, .. } => {
                rp.set_pipeline(&renderer.image_pipeline);
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..6, 0..1);
            }
        }
    }
}

impl CallbackTrait for SceneCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let w = (self.buffers.size[0] * f64::from(self.buffers.ppp)).round() as u32;
        let h = (self.buffers.size[1] * f64::from(self.buffers.ppp)).round() as u32;
        let mut guard = self.target.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|t| t.size) != Some([w, h]) {
            *guard = SceneTarget::new(device, &self.renderer, [w, h]);
        }
        if let Some(tgt) = guard.as_ref() {
            record_scene(&self.renderer, &self.buffers.draws, tgt, egui_encoder);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, rp: &mut wgpu::RenderPass<'static>, _res: &CallbackResources) {
        let guard = self.target.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tgt) = guard.as_ref() {
            rp.set_pipeline(&self.renderer.blit_pipeline);
            rp.set_bind_group(0, &tgt.resolve_bind, &[]);
            rp.draw(0..6, 0..1);
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

    // ---- CPU↔GPU pixel parity: render the same map through both backends and compare ----

    use rustgis_engine::data::{Dataset, Geometry, GeometryType};
    use rustgis_engine::map::{Map, Marker, Renderer, Rgba, Symbol};
    use rustgis_engine::render::{self, Backend, Rendered2, View};

    fn world_view() -> View {
        View { width: 640.0, height: 400.0, pixels_per_point: 1.0, camera: Camera::world(), dark: false }
    }

    /// Renders the GpuScene into an offscreen texture and reads it back (premultiplied RGBA,
    /// the same bytes the CPU pixmap holds). `None` on hosts without a GPU adapter.
    fn gpu_render_offscreen(map: &mut Map, view: &View) -> Option<Vec<u8>> {
        let rs = headless_render_state()?;
        let r = GpuMapRenderer::new(&rs);
        let Some(Rendered2::Gpu(scene)) = render::render_backend(map, view, Backend::Gpu) else { panic!("gpu-compatible scene") };
        let bufs = SceneBuffers::from_scene(
            &rs.device,
            &rs.queue,
            &r,
            &scene,
            view.camera,
            [view.width, view.height],
            view.pixels_per_point as f32,
        );
        let (w, h) = view.pixel_size();
        if w == 0 || h == 0 {
            return None;
        }
        // The production path: MSAA offscreen pass, resolved to a plain texture.
        let tgt = SceneTarget::new(&rs.device, &r, [w, h])?;
        let mut enc = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("parity") });
        record_scene(&r, &bufs.draws, &tgt, &mut enc);
        // Buffer copies need 256-aligned rows; pad and strip on readback.
        let row_bytes = (w * 4).div_ceil(256) * 256;
        let buf = rs.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("parity"),
            size: u64::from(row_bytes * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tgt.resolve_tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        rs.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        rs.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok()?;
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let src = y * row_bytes as usize;
            out[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(data.get(src..src + w as usize * 4)?);
        }
        drop(data);
        buf.unmap();
        Some(out)
    }

    /// Pixels differing by >16 whose 3×3 neighbourhood in the CPU render is FLAT (max − min
    /// < 4 per channel). Edge pixels legitimately differ between analytic AA (CPU) and 4× MSAA
    /// (GPU) — worst on 1-2 px features at world zoom; a mismatch in a flat area is a
    /// structural bug (wrong colour, missing content, bad z-order).
    fn structural_outliers(cpu: &[u8], gpu: &[u8], w: usize, h: usize) -> usize {
        let mut out = 0;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let worst = (0..4).map(|c| cpu[i + c].abs_diff(gpu[i + c])).max().unwrap_or(0);
                if worst <= 16 {
                    continue;
                }
                let mut lo = [255u8; 4];
                let mut hi = [0u8; 4];
                for dy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for dx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        let j = (dy * w + dx) * 4;
                        for c in 0..4 {
                            lo[c] = lo[c].min(cpu[j + c]);
                            hi[c] = hi[c].max(cpu[j + c]);
                        }
                    }
                }
                if (0..4).all(|c| hi[c] - lo[c] < 4) {
                    out += 1;
                }
            }
        }
        out
    }

    /// Mean per-byte absolute difference and the fraction of pixels whose worst channel
    /// differs by more than 16.
    fn diff_stats(a: &[u8], b: &[u8]) -> (f64, f64) {
        assert_eq!(a.len(), b.len());
        let mut sum = 0u64;
        let mut over = 0usize;
        for (px, py) in a.chunks(4).zip(b.chunks(4)) {
            let mut worst = 0u8;
            for c in 0..4 {
                let d = px[c].abs_diff(py[c]);
                sum += u64::from(d);
                worst = worst.max(d);
            }
            if worst > 16 {
                over += 1;
            }
        }
        (sum as f64 / a.len() as f64, over as f64 / (a.len() / 4) as f64)
    }

    fn assert_parity(name: &str, build: impl Fn() -> Map, view: &View) {
        let mut gm = build();
        let Some(gpu) = gpu_render_offscreen(&mut gm, view) else { return }; // no adapter: skip
        let mut cm = build();
        let cpu = render::render(&mut cm, view).expect("cpu render");
        // PARITY_DUMP=/tmp/dir writes {name}-{cpu,gpu,diff}.png for eyeballing failures.
        if let Some(dir) = std::env::var_os("PARITY_DUMP") {
            let (w, h) = view.pixel_size();
            let dump = |suffix: &str, data: &[u8]| {
                let p = std::path::Path::new(&dir).join(format!("{}-{suffix}.png", name.replace(' ', "_")));
                let _ = image::save_buffer(&p, data, w, h, image::ColorType::Rgba8);
            };
            dump("cpu", cpu.pixmap.data());
            dump("gpu", &gpu);
            let mut diff: Vec<u8> = cpu.pixmap.data().iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b).saturating_mul(8)).collect();
            for px in diff.chunks_mut(4) {
                px[3] = 255; // opaque, or the PNG previews as blank
            }
            dump("diff", &diff);
        }
        // Raw mean: same image overall. Structural: no mismatches away from edges — the CPU
        // anti-aliases analytically, the GPU via 4× MSAA, and on 1-2 px features their
        // coverage quantization legitimately differs (edge pixels are excluded by the
        // flatness test); wrong colours, missing content and bad z-order fail here.
        let (mean_raw, _) = diff_stats(cpu.pixmap.data(), &gpu);
        let (w, h) = view.pixel_size();
        let structural = structural_outliers(cpu.pixmap.data(), &gpu, w as usize, h as usize);
        assert!(mean_raw < 1.5 && structural == 0, "{name}: raw mean {mean_raw:.2}/255, {structural} structural outliers");
    }

    fn world_map() -> Map {
        let mut m = Map::new("Map");
        for (name, text) in [
            ("Countries", render::sample_data::COUNTRIES),
            ("Rivers", render::sample_data::RIVERS),
            ("Lakes", render::sample_data::LAKES),
            ("Cities", render::sample_data::CITIES),
        ] {
            if let Some(d) = render::world_dataset(name, text) {
                m.add_dataset(d);
            }
        }
        m
    }

    #[test]
    fn parity_world_sample() {
        assert_parity("world sample", world_map, &world_view());
    }

    #[test]
    fn parity_dark_world_sample() {
        assert_parity("dark world sample", world_map, &View { dark: true, ..world_view() });
    }

    #[test]
    fn parity_polygon_with_hole() {
        assert_parity(
            "polygon with hole",
            || {
                let mut m = Map::new("Map");
                let mut d = Dataset::new("holes", GeometryType::Polygon, vec![]);
                let ring = |x0: f64, y0: f64, x1: f64, y1: f64| {
                    vec![Coord::new(x0, y0), Coord::new(x1, y0), Coord::new(x1, y1), Coord::new(x0, y1)]
                };
                d.push(Some(Geometry::Polygon(vec![vec![ring(-60.0, -20.0, 0.0, 40.0), ring(-40.0, 0.0, -20.0, 20.0)]])), vec![]);
                m.add_dataset(d);
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_markers_all_shapes() {
        assert_parity(
            "all marker shapes",
            || {
                let mut m = Map::new("Map");
                for (i, marker) in Marker::ALL.into_iter().enumerate() {
                    let mut d = Dataset::new(format!("p{i}"), GeometryType::Point, vec![]);
                    d.push(Some(Geometry::Point(Coord::new(i as f64 * 25.0 - 50.0, 20.0))), vec![]);
                    let id = m.add_dataset(d);
                    if let Some(l) = m.layer_mut(id) {
                        l.renderer = Renderer::Single(Symbol {
                            fill: Rgba::rgb(0xE0, 0x7B, 0x39),
                            stroke: Rgba::rgb(255, 255, 255),
                            stroke_width: 1.5,
                            size: 12.0,
                            marker,
                        });
                    }
                }
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_lines_and_selection() {
        assert_parity(
            "lines and selection",
            || {
                let mut m = Map::new("Map");
                let mut d = Dataset::new("rivers", GeometryType::Polyline, vec![]);
                d.push(Some(Geometry::Line(vec![vec![Coord::new(-100.0, 0.0), Coord::new(-50.0, 20.0), Coord::new(0.0, 0.0)]])), vec![]);
                d.push(Some(Geometry::Line(vec![vec![Coord::new(0.0, -20.0), Coord::new(50.0, -10.0)]])), vec![]);
                let id = m.add_dataset(d);
                if let Some(l) = m.layer_mut(id) {
                    l.selection = vec![0];
                }
                m
            },
            &world_view(),
        );
    }

    #[test]
    fn parity_transparent_layer_interleaves() {
        assert_parity(
            "transparent layer interleave",
            || {
                let mut m = Map::new("Map");
                // Drawn bottom-up: last in the list first.
                let ring = |x0: f64| {
                    vec![vec![vec![Coord::new(x0, 0.0), Coord::new(x0 + 30.0, 0.0), Coord::new(x0 + 30.0, 30.0), Coord::new(x0, 30.0)]]]
                };
                let mut d = Dataset::new("a", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(-60.0))), vec![]);
                m.add_dataset(d);
                let mut d = Dataset::new("b", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(-20.0))), vec![]);
                let id = m.add_dataset(d);
                if let Some(l) = m.layer_mut(id) {
                    l.transparency = 50.0; // offscreen-composited → a CPU image command
                }
                let mut d = Dataset::new("c", GeometryType::Polygon, vec![]);
                d.push(Some(Geometry::Polygon(ring(20.0))), vec![]);
                m.add_dataset(d);
                m
            },
            &world_view(),
        );
    }
}
