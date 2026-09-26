//! wgpu renderer for pixel art.
//!
//! Sprites are drawn into a low-resolution target at the project's internal resolution, then
//! blitted to the window at the largest whole-number scale that fits (letterboxed). The camera
//! and every sprite corner are snapped to whole pixels.

mod model;
mod sprite;
mod text;

use std::sync::Arc;

use glam::Vec2;
use winit::window::Window;

pub use model::{Light, MAX_BONES, ModelDraw, ModelVertex};
pub use sprite::{
    Mesh, MeshVertex, Outline, Sprite, SpriteKind, TextureId, Viewport, fit_viewport, layer,
};
pub use text::{FontError, GlyphQuad, TextLayout, TextSystem};

/// Sprite textures and the internal target share this format; blending happens in linear space.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// How many models the bone buffer and the geometry buffers start out able to hold. They grow.
const MIN_PALETTES: usize = 8;
const MIN_MODEL_VERTICES: usize = 4096;
/// Depth for the mesh pass. Plain 32-bit float: no stencil is wanted, and every backend has it.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// One palette's room in the bone buffer. A dynamic offset must be aligned, and 256 is the
/// alignment every backend asks for at most.
const BONE_STRIDE: u64 = (model::MAX_BONES * size_of::<glam::Mat4>()) as u64;
/// A dynamic offset must be a multiple of `min_uniform_buffer_offset_alignment`, which is 256
/// at worst. Cutting `MAX_BONES` to something not divisible by four would break every draw
/// after the first — and only after the first, so a one-model preview would not notice.
const _: () = assert!(
    BONE_STRIDE.is_multiple_of(256),
    "MAX_BONES must be a multiple of 4"
);
/// Occlusion mask format. Needs render + blend support, which Vulkan, DX12 and Metal have but
/// OpenGL only with a half-float extension; without it silhouettes are turned off. (32-bit float
/// would need the optional FLOAT32_BLENDABLE feature everywhere.)
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const MIN_INSTANCE_CAPACITY: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("failed to create surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("no suitable GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to create device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("surface is not supported by the adapter")]
    UnsupportedSurface,
    #[error("texture {label} is {width}x{height}; it must be 1..={max} on each side")]
    TextureSize {
        label: String,
        width: u32,
        height: u32,
        max: u32,
    },
    #[error("texture {label}: expected {expected} bytes of RGBA, got {actual}")]
    TextureData {
        label: String,
        expected: usize,
        actual: usize,
    },
}

/// RGBA8 (sRGB) pixels read back from the internal target.
/// Everything one frame draws. Sprites and meshes are laid out flat by the 2D camera; models
/// are placed by `model_camera`, which is world to clip with near at 0.
pub struct Scene<'a> {
    /// Where the 2D camera looks, in world pixels.
    pub camera: Vec2,
    pub clear: [f64; 3],
    pub sprites: &'a mut [Sprite],
    pub meshes: &'a [Mesh],
    pub models: &'a [ModelDraw<'a>],
    pub model_camera: glam::Mat4,
    pub light: Light,
}

pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Silhouettes {
    mask_pipeline: wgpu::RenderPipeline,
    silhouette_pipeline: wgpu::RenderPipeline,
    silhouette_mesh_pipeline: wgpu::RenderPipeline,
    mask_view: wgpu::TextureView,
    mask_bind_group: wgpu::BindGroup,
    mask_layout: wgpu::BindGroupLayout,
}

/// Everything the skinned-mesh pass owns.
struct Models {
    /// Back faces culled, for closed bodies.
    pipeline: wgpu::RenderPipeline,
    /// Nothing culled, for the single-layer cards a material marks double-sided.
    both_sides: wgpu::RenderPipeline,
    depth: wgpu::TextureView,
    camera: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    bones: wgpu::Buffer,
    bones_bind_group: wgpu::BindGroup,
    bones_layout: wgpu::BindGroupLayout,
    /// How many palettes the bone buffer holds.
    bone_capacity: usize,
    vertices: wgpu::Buffer,
    vertex_capacity: usize,
    indices: wgpu::Buffer,
    index_capacity: usize,
}

struct GpuTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    origin: [f32; 2],
    size: [f32; 2],
}

pub struct Renderer {
    /// The window it shows in; `None` offscreen (the editor shows the target itself).
    output: Option<Output>,
    device: wgpu::Device,
    queue: wgpu::Queue,

    internal_size: (u32, u32),
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    /// The target can also be viewed as plain (not sRGB) bytes: see [`Renderer::shared_view`].
    shared: bool,

    sampler: wgpu::Sampler,
    /// For painted art drawn smaller or turned (skeletons): see [`Renderer::create_texture_smooth`].
    smooth_sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    textures: Vec<GpuTexture>,

    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    mesh_vertices: wgpu::Buffer,
    mesh_capacity: usize,
    main_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    /// The mesh pass: skinned models drawn against a depth buffer (`model.rs`).
    models: Models,
    /// `None` where the GPU cannot blend into the occlusion mask.
    silhouettes: Option<Silhouettes>,

    blit_bind_group: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
}

/// Where a windowed renderer shows its frames.
struct Output {
    instance: wgpu::Instance,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
}

impl Renderer {
    /// `internal_size` is the project's render resolution (docs/PLAN.md §1).
    pub fn new(window: Arc<Window>, internal_size: (u32, u32)) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window, internal_size))
    }

    /// A renderer that draws only into its internal target, on someone else's device (the
    /// editor, which shows the target in its own interface). See [`Renderer::shared_view`].
    pub fn offscreen(
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        internal_size: (u32, u32),
    ) -> Self {
        let masks = masks_supported(adapter);
        let mut renderer = Self::build(device, queue, internal_size, None, masks);
        renderer.shared = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::VIEW_FORMATS);
        if !renderer.shared {
            tracing::warn!("this GPU cannot share the target as plain bytes: colours show dark");
        }
        renderer.replace_target();
        renderer
    }

    /// The internal target as last rendered, read as plain gamma-encoded bytes: how interfaces
    /// such as egui sample an image (they would darken an sRGB view). Made anew on each call,
    /// and after [`Renderer::set_internal_size`] the old one shows the old target. Where the GPU
    /// cannot view the target so (GLES), it is the sRGB view.
    pub fn shared_view(&self) -> wgpu::TextureView {
        let format = self.shared.then(|| TARGET_FORMAT.remove_srgb_suffix());
        self.target.create_view(&wgpu::TextureViewDescriptor {
            format,
            ..Default::default()
        })
    }

    /// Draws at a new internal size from the next frame on (an editor's viewport resized).
    pub fn set_internal_size(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.internal_size {
            return;
        }
        self.internal_size = size;
        self.replace_target();
    }

    /// Makes the internal target (and what reads it) anew at the internal size.
    fn replace_target(&mut self) {
        let size = self.internal_size;
        let (target, target_view) = create_target(&self.device, size, self.shared);
        self.blit_bind_group = texture_bind_group(
            &self.device,
            &self.texture_layout,
            &target_view,
            &self.sampler,
            "blit",
        );
        self.target = target;
        self.target_view = target_view;
        self.models.depth = create_depth(&self.device, size);
        if let Some(sil) = &mut self.silhouettes {
            sil.mask_view = create_mask(&self.device, size);
            sil.mask_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("occlusion mask"),
                layout: &sil.mask_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sil.mask_view),
                }],
            });
        }
    }

    async fn new_async(
        window: Arc<Window>,
        internal_size: (u32, u32),
    ) -> Result<Self, RenderError> {
        let size = window.inner_size();
        // The display handle is only used by the GL backend (Linux fallback), but costs nothing to pass.
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let info = adapter.get_info();
        tracing::info!("GPU: {} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("dark_render"),
                // Ask for what the adapter has, not wgpu's desktop defaults, which GL and older
                // GPUs (the Linux fallback) often cannot meet. Nothing here needs more.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or(RenderError::UnsupportedSurface)?;
        // An sRGB swapchain keeps colours identical between the internal target and the window.
        if let Some(srgb) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|f| f.is_srgb())
        {
            config.format = srgb;
        }
        // Without an sRGB swapchain format, render through an sRGB view of it so colours still match.
        let view_format = config.format.add_srgb_suffix();
        if view_format != config.format {
            config.view_formats.push(view_format);
        }
        surface.configure(&device, &config);
        let output = Output {
            instance,
            window,
            surface,
            config,
            view_format,
        };
        let masks = masks_supported(&adapter);
        Ok(Self::build(
            device,
            queue,
            internal_size,
            Some(output),
            masks,
        ))
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        internal_size: (u32, u32),
        output: Option<Output>,
        mask_supported: bool,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let smooth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
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
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let (target, target_view) = create_target(&device, internal_size, false);
        let blit_bind_group =
            texture_bind_group(&device, &texture_layout, &target_view, &sampler, "blit");

        let sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        // Occlusion mask: the frontmost occluder's rank per pixel, for character silhouettes.
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("occlusion mask"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        if !mask_supported {
            tracing::warn!(
                "{MASK_FORMAT:?} cannot be blended on this GPU; character silhouettes are off"
            );
        }

        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2,
            5 => Float32x4, 6 => Float32, 7 => Uint32
        ];
        // The three sprite passes share the vertex stage and differ in fragment entry and target;
        // meshes use the same attributes per vertex instead of per instance.
        // A pipeline must agree with its pass about depth. The main pass carries one for the
        // models; everything drawn there has to declare it, and the sprites declare it in the
        // only way that leaves them alone — never write, never fail.
        let ignores_depth = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let pipeline = |label: &str,
                        vertex_entry: &str,
                        step_mode: wgpu::VertexStepMode,
                        entry: &str,
                        layouts: &[Option<&wgpu::BindGroupLayout>],
                        format: wgpu::TextureFormat,
                        blend: wgpu::BlendState,
                        depth: bool| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &sprite_shader,
                    entry_point: Some(vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<sprite::Instance>() as u64,
                        step_mode,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth.then(|| ignores_depth.clone()),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &sprite_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sprite_pipeline = |label: &str,
                               entry: &str,
                               layouts: &[Option<&wgpu::BindGroupLayout>],
                               format: wgpu::TextureFormat,
                               blend: wgpu::BlendState,
                               depth: bool| {
            pipeline(
                label,
                "vs_sprite",
                wgpu::VertexStepMode::Instance,
                entry,
                layouts,
                format,
                blend,
                depth,
            )
        };
        let base_layouts = [Some(&globals_layout), Some(&texture_layout)];
        let max = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let main_pipeline = sprite_pipeline(
            "sprite",
            "fs_sprite",
            &base_layouts,
            TARGET_FORMAT,
            wgpu::BlendState::ALPHA_BLENDING,
            true,
        );
        let mesh_pipeline = pipeline(
            "mesh",
            "vs_mesh",
            wgpu::VertexStepMode::Vertex,
            "fs_sprite",
            &base_layouts,
            TARGET_FORMAT,
            wgpu::BlendState::ALPHA_BLENDING,
            true,
        );
        let models = build_models(&device, internal_size, &texture_layout);
        let silhouettes = mask_supported.then(|| {
            let mask_view = create_mask(&device, internal_size);
            let mask_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("occlusion mask"),
                layout: &mask_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                }],
            });
            Silhouettes {
                mask_pipeline: sprite_pipeline(
                    "occlusion mask",
                    "fs_mask",
                    &base_layouts,
                    MASK_FORMAT,
                    wgpu::BlendState {
                        color: max,
                        alpha: max,
                    },
                    false,
                ),
                silhouette_pipeline: sprite_pipeline(
                    "silhouette",
                    "fs_silhouette",
                    &[
                        Some(&globals_layout),
                        Some(&texture_layout),
                        Some(&mask_layout),
                    ],
                    TARGET_FORMAT,
                    wgpu::BlendState::ALPHA_BLENDING,
                    false,
                ),
                silhouette_mesh_pipeline: pipeline(
                    "mesh silhouette",
                    "vs_mesh",
                    wgpu::VertexStepMode::Vertex,
                    "fs_silhouette",
                    &[
                        Some(&globals_layout),
                        Some(&texture_layout),
                        Some(&mask_layout),
                    ],
                    TARGET_FORMAT,
                    wgpu::BlendState::ALPHA_BLENDING,
                    false,
                ),
                mask_view,
                mask_bind_group,
                mask_layout,
            }
        });
        let blit_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(&texture_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &blit_shader,
                    entry_point: Some("vs_blit"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &blit_shader,
                    entry_point: Some("fs_blit"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        // Offscreen there is no window; the pipeline is never used then.
                        format: output.as_ref().map_or(TARGET_FORMAT, |o| o.view_format),
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let instances = create_instance_buffer(&device, MIN_INSTANCE_CAPACITY);
        let mesh_vertices = create_instance_buffer(&device, MIN_INSTANCE_CAPACITY);

        Self {
            output,
            device,
            queue,
            internal_size,
            target,
            target_view,
            shared: false,
            sampler,
            smooth_sampler,
            texture_layout,
            textures: Vec::new(),
            globals,
            globals_bind_group,
            instances,
            instance_capacity: MIN_INSTANCE_CAPACITY,
            mesh_vertices,
            mesh_capacity: MIN_INSTANCE_CAPACITY,
            main_pipeline,
            mesh_pipeline,
            models,
            silhouettes,
            blit_bind_group,
            blit_pipeline,
        }
    }

    /// Draws models alone, clearing first: previews and tests, where there is no world around
    /// them. It is [`Renderer::render_scene`] with nothing else in the frame, so the models take
    /// the same path they take in the game.
    pub fn render_models(
        &mut self,
        camera: glam::Mat4,
        light: Light,
        clear: Option<[f64; 3]>,
        draws: &[ModelDraw],
    ) {
        self.render_scene(Scene {
            camera: Vec2::ZERO,
            clear: clear.unwrap_or([0.0; 3]),
            sprites: &mut [],
            meshes: &[],
            models: draws,
            model_camera: camera,
            light,
        });
    }

    pub fn internal_size(&self) -> (u32, u32) {
        self.internal_size
    }

    /// Uploads RGBA8 (sRGB) pixels, sampled nearest: pixel art.
    pub fn create_texture(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<TextureId, RenderError> {
        self.upload(label, width, height, rgba, false)
    }

    /// Like [`Renderer::create_texture`], sampled smoothly: painted art that is drawn turned or
    /// not texel for pixel (skeleton atlases).
    pub fn create_texture_smooth(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<TextureId, RenderError> {
        self.upload(label, width, height, rgba, true)
    }

    fn upload(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        smooth: bool,
    ) -> Result<TextureId, RenderError> {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(RenderError::TextureSize {
                label: label.into(),
                width,
                height,
                max,
            });
        }
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(RenderError::TextureData {
                label: label.into(),
                expected,
                actual: rgba.len(),
            });
        }
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: None,
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = if smooth {
            &self.smooth_sampler
        } else {
            &self.sampler
        };
        let bind_group =
            texture_bind_group(&self.device, &self.texture_layout, &view, sampler, label);
        self.textures.push(GpuTexture {
            texture,
            bind_group,
            size: (width, height),
        });
        Ok(TextureId(self.textures.len() as u32 - 1))
    }

    /// Replaces all of a texture's pixels; `rgba` must match its size.
    pub fn update_texture(&mut self, id: TextureId, rgba: &[u8]) -> Result<(), RenderError> {
        let gpu = &self.textures[id.0 as usize];
        let (width, height) = gpu.size;
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(RenderError::TextureData {
                label: format!("#{}", id.0),
                expected,
                actual: rgba.len(),
            });
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub fn texture_size(&self, id: TextureId) -> (u32, u32) {
        self.textures[id.0 as usize].size
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let Some(out) = &mut self.output else {
            return;
        };
        if width == 0 || height == 0 {
            return;
        }
        out.config.width = width;
        out.config.height = height;
        out.surface.configure(&self.device, &out.config);
    }

    /// Draws one frame. `camera` is the world position at the centre of the view; `clear` is
    /// linear RGB. Sprites are sorted in place.
    pub fn render(&mut self, camera: Vec2, clear: [f64; 3], sprites: &mut [Sprite]) {
        self.render_with(camera, clear, sprites, &[]);
    }

    /// Like [`Renderer::render`], with meshes drawn among the sprites in sort order.
    pub fn render_with(
        &mut self,
        camera: Vec2,
        clear: [f64; 3],
        sprites: &mut [Sprite],
        meshes: &[Mesh],
    ) {
        self.render_scene(Scene {
            camera,
            clear,
            sprites,
            meshes,
            models: &[],
            model_camera: glam::Mat4::IDENTITY,
            light: Light::default(),
        });
    }

    /// Draws one frame: sprites, posed 2D skeletons and skinned models, in one sorted order.
    ///
    /// The world is still sorted back to front by layer and by feet — a model takes its place in
    /// that order like anything else. What a model gets on top of that is a **depth buffer**, so
    /// that its own surfaces sort against each other; sprites neither read nor write it.
    pub fn render_scene(&mut self, scene: Scene<'_>) {
        let Scene {
            camera,
            clear,
            sprites,
            meshes,
            models,
            model_camera,
            light,
        } = scene;
        let textures = &self.textures;
        let frame_batches =
            sprite::build_batches(sprites, meshes, models, |id| textures[id.0 as usize].size);
        let instances = &frame_batches.instances;
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instances = create_instance_buffer(&self.device, self.instance_capacity);
        }
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
        let vertices = &frame_batches.vertices;
        if vertices.len() > self.mesh_capacity {
            self.mesh_capacity = vertices.len().next_power_of_two();
            self.mesh_vertices = create_instance_buffer(&self.device, self.mesh_capacity);
        }
        if !vertices.is_empty() {
            self.queue
                .write_buffer(&self.mesh_vertices, 0, bytemuck::cast_slice(vertices));
        }
        // The models' own buffers, and the frame's bone palettes end to end.
        let model_vertices = &frame_batches.model_vertices;
        if model_vertices.len() > self.models.vertex_capacity {
            self.models.vertex_capacity = model_vertices.len().next_power_of_two();
            self.models.vertices = create_model_buffer(
                &self.device,
                self.models.vertex_capacity,
                wgpu::BufferUsages::VERTEX,
            );
        }
        let model_indices = &frame_batches.model_indices;
        if model_indices.len() > self.models.index_capacity {
            self.models.index_capacity = model_indices.len().next_power_of_two();
            self.models.indices = create_model_buffer(
                &self.device,
                self.models.index_capacity,
                wgpu::BufferUsages::INDEX,
            );
        }
        let palettes = frame_batches.bones.len() / model::MAX_BONES;
        if palettes > self.models.bone_capacity {
            self.models.bone_capacity = palettes.next_power_of_two();
            self.models.bones = create_bone_buffer(&self.device, self.models.bone_capacity);
            self.models.bones_bind_group =
                bone_bind_group(&self.device, &self.models.bones_layout, &self.models.bones);
        }
        if !model_vertices.is_empty() {
            self.queue.write_buffer(
                &self.models.vertices,
                0,
                bytemuck::cast_slice(model_vertices),
            );
            self.queue
                .write_buffer(&self.models.indices, 0, bytemuck::cast_slice(model_indices));
            let bones: Vec<[[f32; 4]; 4]> = frame_batches
                .bones
                .iter()
                .map(glam::Mat4::to_cols_array_2d)
                .collect();
            self.queue
                .write_buffer(&self.models.bones, 0, bytemuck::cast_slice(&bones));
            self.queue.write_buffer(
                &self.models.camera,
                0,
                bytemuck::bytes_of(&model::ModelCamera::new(model_camera, light)),
            );
        }

        let internal = Vec2::new(self.internal_size.0 as f32, self.internal_size.1 as f32);
        let globals = Globals {
            origin: (camera - internal / 2.0).round().to_array(),
            size: internal.to_array(),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let [r, g, b] = clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprites"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                // Only the models read or write this; the sprite pipelines are built to ignore
                // it, so the flat world keeps the painter's order it has always had.
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.models.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.draw_mixed(
                &mut pass,
                &frame_batches.main,
                &self.main_pipeline,
                &self.mesh_pipeline,
            );
        }
        if let Some(sil) = &self.silhouettes
            && !frame_batches.silhouettes.is_empty()
        {
            // Occluders write their rank; then covered bodies show through where a later rank won.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("occlusion mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &sil.mask_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&sil.mask_pipeline);
            self.draw_batches(&mut pass, &frame_batches.occluders);
            drop(pass);

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("silhouettes"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_bind_group(2, &sil.mask_bind_group, &[]);
            self.draw_mixed(
                &mut pass,
                &frame_batches.silhouettes,
                &sil.silhouette_pipeline,
                &sil.silhouette_mesh_pipeline,
            );
        }
        // The interface last: nothing shows through it.
        if !frame_batches.overlay.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                // It draws with the main pass's pipelines, so it must offer what they declare.
                // Nothing here reads or writes it.
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.models.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            self.draw_mixed(
                &mut pass,
                &frame_batches.overlay,
                &self.main_pipeline,
                &self.mesh_pipeline,
            );
        }

        let frame = self.acquire();
        if let (Some((frame, _)), Some(out)) = (&frame, &self.output) {
            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(out.view_format),
                ..Default::default()
            });
            let viewport = fit_viewport(self.internal_size, (out.config.width, out.config.height));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                viewport.x,
                viewport.y,
                viewport.width,
                viewport.height,
                0.0,
                1.0,
            );
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        if let Some((frame, suboptimal)) = frame {
            self.queue.present(frame);
            // Reconfigure only after the acquired frame is released.
            if suboptimal && let Some(out) = &self.output {
                out.surface.configure(&self.device, &out.config);
            }
        }
    }

    fn acquire(&mut self) -> Option<(wgpu::SurfaceTexture, bool)> {
        let out = self.output.as_ref()?;
        match out.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some((frame, false)),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some((frame, true)),
            wgpu::CurrentSurfaceTexture::Outdated => {
                out.surface.configure(&self.device, &out.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.recreate_surface();
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::warn!("surface validation error; skipping frame");
                None
            }
        }
    }

    /// A lost surface cannot be reconfigured; it must be created again from the window.
    fn recreate_surface(&mut self) {
        let Some(out) = &mut self.output else {
            return;
        };
        match out.instance.create_surface(out.window.clone()) {
            Ok(surface) => {
                surface.configure(&self.device, &out.config);
                out.surface = surface;
            }
            Err(err) => tracing::error!("failed to recreate lost surface: {err}"),
        }
    }

    /// Reads back the internal image of the last rendered frame, at internal resolution.
    pub fn capture(&self) -> Capture {
        let (width, height) = self.internal_size;
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| {
            if let Err(err) = result {
                tracing::error!("capture map failed: {err}");
            }
        });
        if let Err(err) = self.device.poll(wgpu::PollType::wait_indefinitely()) {
            tracing::error!("capture poll failed: {err}");
        }
        let mapped = slice
            .get_mapped_range()
            .expect("capture buffer was just mapped");
        let mut rgba = Vec::with_capacity((unpadded * height) as usize);
        for row in mapped.chunks(padded as usize) {
            rgba.extend_from_slice(&row[..unpadded as usize]);
        }
        Capture {
            width,
            height,
            rgba,
        }
    }
}

/// Whether the GPU can blend into the occlusion mask (character silhouettes).
fn masks_supported(adapter: &wgpu::Adapter) -> bool {
    let features = adapter.get_texture_format_features(MASK_FORMAT);
    features
        .allowed_usages
        .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING)
        && features
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
}

/// The internal target; `shared`, it can also be viewed as plain bytes (not on GLES).
fn create_target(
    device: &wgpu::Device,
    (width, height): (u32, u32),
    shared: bool,
) -> (wgpu::Texture, wgpu::TextureView) {
    let plain = [TARGET_FORMAT.remove_srgb_suffix()];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("internal target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: if shared { &plain } else { &[] },
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Binds globals and instances, then draws each batch with its texture. The pipeline is set.
impl Renderer {
    /// Sprite runs and mesh runs in order, each with its pipeline (the main and silhouette
    /// passes).
    fn draw_mixed(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        batches: &[sprite::Batch],
        quads: &wgpu::RenderPipeline,
        triangles: &wgpu::RenderPipeline,
    ) {
        // What the pass is set up for at the moment. A model uses a different pipeline layout
        // and a different camera, so switching to or from one rebinds group 0.
        let mut set = None;
        for batch in batches {
            let kind = batch.kind;
            if set != Some(kind_of(kind)) {
                set = Some(kind_of(kind));
                match kind {
                    sprite::BatchKind::Sprites => {
                        pass.set_pipeline(quads);
                        pass.set_bind_group(0, &self.globals_bind_group, &[]);
                        pass.set_vertex_buffer(0, self.instances.slice(..));
                    }
                    sprite::BatchKind::Triangles => {
                        pass.set_pipeline(triangles);
                        pass.set_bind_group(0, &self.globals_bind_group, &[]);
                        pass.set_vertex_buffer(0, self.mesh_vertices.slice(..));
                    }
                    sprite::BatchKind::Model { double_sided, .. } => {
                        pass.set_pipeline(if double_sided {
                            &self.models.both_sides
                        } else {
                            &self.models.pipeline
                        });
                        pass.set_bind_group(0, &self.models.camera_bind_group, &[]);
                        pass.set_vertex_buffer(0, self.models.vertices.slice(..));
                        pass.set_index_buffer(
                            self.models.indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                    }
                }
            }
            let Some(gpu) = self.textures.get(batch.texture.0 as usize) else {
                tracing::warn!(
                    "a batch names texture {}, which is not there",
                    batch.texture.0
                );
                continue;
            };
            pass.set_bind_group(1, &gpu.bind_group, &[]);
            match kind {
                sprite::BatchKind::Sprites => pass.draw(0..6, batch.start..batch.end),
                sprite::BatchKind::Triangles => pass.draw(batch.start..batch.end, 0..1),
                sprite::BatchKind::Model { base, palette, .. } => {
                    pass.set_bind_group(2, &self.models.bones_bind_group, &[palette]);
                    pass.draw_indexed(batch.start..batch.end, base, 0..1);
                }
            }
        }
    }

    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, batches: &[sprite::Batch]) {
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        for batch in batches {
            pass.set_bind_group(1, &self.textures[batch.texture.0 as usize].bind_group, &[]);
            pass.draw(0..6, batch.start..batch.end);
        }
    }
}

fn create_mask(device: &wgpu::Device, (width, height): (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("occlusion mask"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// The depth buffer the mesh pass tests against, at the target's size.
/// What the pass has to be set up for. Two batches of the same kind run on without rebinding;
/// models of different sidedness do not, because they are different pipelines.
fn kind_of(kind: sprite::BatchKind) -> (u8, bool) {
    match kind {
        sprite::BatchKind::Sprites => (0, false),
        sprite::BatchKind::Triangles => (1, false),
        sprite::BatchKind::Model { double_sided, .. } => (2, double_sided),
    }
}

fn create_depth(device: &wgpu::Device, size: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("model depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_bone_buffer(device: &wgpu::Device, palettes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bone palettes"),
        size: BONE_STRIDE * palettes.max(1) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bone_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bone palettes"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                // One palette at a time; the dynamic offset picks which.
                size: std::num::NonZeroU64::new(BONE_STRIDE),
            }),
        }],
    })
}

fn build_models(
    device: &wgpu::Device,
    size: (u32, u32),
    texture_layout: &wgpu::BindGroupLayout,
) -> Models {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("model"),
        source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
    });
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model camera"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bone palettes"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                // Every draw shares one buffer and picks its palette by offset.
                has_dynamic_offset: true,
                min_binding_size: std::num::NonZeroU64::new(BONE_STRIDE),
            },
            count: None,
        }],
    });
    let camera = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model camera"),
        size: size_of::<model::ModelCamera>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model camera"),
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera.as_entire_binding(),
        }],
    });
    let bones = create_bone_buffer(device, MIN_PALETTES);
    let bones_bind_group = bone_bind_group(device, &bones_layout, &bones);

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("model"),
        bind_group_layouts: &[
            Some(&camera_layout),
            Some(texture_layout),
            Some(&bones_layout),
        ],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint16x4, 4 => Float32x4
    ];
    let describe = |label: &str, cull: Option<wgpu::Face>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<model::ModelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                // Nearer wins. The projection puts near at 0, as wgpu wants.
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_model"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    // Cut out in the shader rather than blended: see model.wgsl.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    // A character is solid and closed, so the inside of it is not worth drawing — unless the
    // material says otherwise, which is what the second pipeline is for.
    let pipeline = describe("model", Some(wgpu::Face::Back));
    let both_sides = describe("model, both sides", None);

    Models {
        pipeline,
        both_sides,
        depth: create_depth(device, size),
        camera,
        camera_bind_group,
        bones,
        bones_bind_group,
        bones_layout,
        bone_capacity: MIN_PALETTES,
        vertices: create_model_buffer(device, MIN_MODEL_VERTICES, wgpu::BufferUsages::VERTEX),
        vertex_capacity: MIN_MODEL_VERTICES,
        indices: create_model_buffer(device, MIN_MODEL_VERTICES, wgpu::BufferUsages::INDEX),
        index_capacity: MIN_MODEL_VERTICES,
    }
}

/// A vertex or index buffer for the mesh pass, sized in elements of four or more bytes.
fn create_model_buffer(
    device: &wgpu::Device,
    capacity: usize,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let stride = if usage.contains(wgpu::BufferUsages::INDEX) {
        size_of::<u32>()
    } else {
        size_of::<model::ModelVertex>()
    };
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model geometry"),
        size: (capacity.max(1) * stride) as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sprite instances"),
        size: (capacity * size_of::<sprite::Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod shader_tests {
    fn validate(name: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }

    #[test]
    fn shaders_are_valid_wgsl() {
        validate("sprite.wgsl", include_str!("sprite.wgsl"));
        validate("blit.wgsl", include_str!("blit.wgsl"));
    }
}
