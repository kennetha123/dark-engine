//! wgpu renderer for pixel art.
//!
//! Sprites are drawn into a low-resolution target at the project's internal resolution, then
//! blitted to the window at the largest whole-number scale that fits (letterboxed). The camera
//! and every sprite corner are snapped to whole pixels.

mod sprite;
mod text;

use std::sync::Arc;

use glam::Vec2;
use winit::window::Window;

pub use sprite::{Outline, Sprite, SpriteKind, TextureId, Viewport, fit_viewport, layer};
pub use text::{FontError, GlyphQuad, TextLayout, TextSystem};

/// Sprite textures and the internal target share this format; blending happens in linear space.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
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
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Silhouettes {
    mask_pipeline: wgpu::RenderPipeline,
    silhouette_pipeline: wgpu::RenderPipeline,
    mask_view: wgpu::TextureView,
    mask_bind_group: wgpu::BindGroup,
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
    instance: wgpu::Instance,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,

    internal_size: (u32, u32),
    target: wgpu::Texture,
    target_view: wgpu::TextureView,

    sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    textures: Vec<GpuTexture>,

    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: usize,
    main_pipeline: wgpu::RenderPipeline,
    /// `None` where the GPU cannot blend into the occlusion mask.
    silhouettes: Option<Silhouettes>,

    blit_bind_group: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    /// `internal_size` is the project's render resolution (docs/PLAN.md §1).
    pub fn new(window: Arc<Window>, internal_size: (u32, u32)) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window, internal_size))
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
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

        let (target, target_view) = create_target(&device, internal_size);
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
        let mask_features = adapter.get_texture_format_features(MASK_FORMAT);
        let mask_supported = mask_features.allowed_usages.contains(
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        ) && mask_features
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE);
        if !mask_supported {
            tracing::warn!(
                "{MASK_FORMAT:?} cannot be blended on this GPU; character silhouettes are off"
            );
        }

        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2,
            5 => Float32x4, 6 => Float32, 7 => Uint32
        ];
        // The three sprite passes share the vertex stage and differ in fragment entry and target.
        let sprite_pipeline = |label: &str,
                               entry: &str,
                               layouts: &[Option<&wgpu::BindGroupLayout>],
                               format: wgpu::TextureFormat,
                               blend: wgpu::BlendState| {
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
                    entry_point: Some("vs_sprite"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<sprite::Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
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
        );
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
                ),
                mask_view,
                mask_bind_group,
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
                        format: view_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let instances = create_instance_buffer(&device, MIN_INSTANCE_CAPACITY);

        Ok(Self {
            instance,
            window,
            surface,
            device,
            queue,
            config,
            view_format,
            internal_size,
            target,
            target_view,
            sampler,
            texture_layout,
            textures: Vec::new(),
            globals,
            globals_bind_group,
            instances,
            instance_capacity: MIN_INSTANCE_CAPACITY,
            main_pipeline,
            silhouettes,
            blit_bind_group,
            blit_pipeline,
        })
    }

    pub fn internal_size(&self) -> (u32, u32) {
        self.internal_size
    }

    /// Uploads RGBA8 (sRGB) pixels.
    pub fn create_texture(
        &mut self,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
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
        let bind_group = texture_bind_group(
            &self.device,
            &self.texture_layout,
            &view,
            &self.sampler,
            label,
        );
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
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Draws one frame. `camera` is the world position at the centre of the view; `clear` is
    /// linear RGB. Sprites are sorted in place.
    pub fn render(&mut self, camera: Vec2, clear: [f64; 3], sprites: &mut [Sprite]) {
        let textures = &self.textures;
        let frame_batches = sprite::build_batches(sprites, |id| textures[id.0 as usize].size);
        let instances = &frame_batches.instances;
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instances = create_instance_buffer(&self.device, self.instance_capacity);
        }
        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
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
                ..Default::default()
            });
            pass.set_pipeline(&self.main_pipeline);
            self.draw_batches(&mut pass, &frame_batches.main);
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
            pass.set_pipeline(&sil.silhouette_pipeline);
            pass.set_bind_group(2, &sil.mask_bind_group, &[]);
            self.draw_batches(&mut pass, &frame_batches.silhouettes);
        }

        let frame = self.acquire();
        if let Some((frame, _)) = &frame {
            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.view_format),
                ..Default::default()
            });
            let viewport =
                fit_viewport(self.internal_size, (self.config.width, self.config.height));
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
            if suboptimal {
                self.surface.configure(&self.device, &self.config);
            }
        }
    }

    fn acquire(&mut self) -> Option<(wgpu::SurfaceTexture, bool)> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => Some((frame, false)),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some((frame, true)),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
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
        match self.instance.create_surface(self.window.clone()) {
            Ok(surface) => {
                surface.configure(&self.device, &self.config);
                self.surface = surface;
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

fn create_target(
    device: &wgpu::Device,
    (width, height): (u32, u32),
) -> (wgpu::Texture, wgpu::TextureView) {
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
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Binds globals and instances, then draws each batch with its texture. The pipeline is set.
impl Renderer {
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
