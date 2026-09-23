//! Dark Editor: makes a game's maps by pointing and clicking (docs/PLAN.md §18).
//!
//!   dark-editor [--project <dir>] [--scene <file>] [--script <steps>]
//!               [--screenshot <png> --frames <n>]
//!
//! Without `--project` it asks for the project folder. `--screenshot` saves the whole window
//! after `--frames` frames (and the `--script`, see `script.rs`) and quits: checking the editor
//! without a person at the screen.

mod catalog;
mod database;
mod editor;
mod minimap;
mod scene_ops;
mod script;
mod sheets;
mod story;
mod strings;
mod viewport;
mod widgets;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use dark_assets::Project;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::editor::Editor;
use crate::script::Script;
use crate::viewport::Viewport;

#[derive(Default)]
struct Args {
    project: Option<PathBuf>,
    scene: Option<String>,
    screenshot: Option<PathBuf>,
    frames: u32,
    script: Option<Script>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        frames: 30,
        ..Default::default()
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--project" => args.project = Some(value()?.into()),
            "--scene" => args.scene = Some(value()?),
            "--screenshot" => args.screenshot = Some(value()?.into()),
            "--script" => args.script = Some(Script::parse(&value()?)?),
            "--frames" => {
                args.frames = value()?
                    .parse()
                    .map_err(|_| "--frames needs a number".to_owned())?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .init();
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("dark-editor: {err}");
            std::process::exit(2);
        }
    };
    let root = match args.project.clone().or_else(|| {
        rfd::FileDialog::new()
            .set_title("Open a Dark Engine project (the folder with project.ron)")
            .pick_folder()
    }) {
        Some(root) => root,
        None => return,
    };
    let project = match Project::open(&root) {
        Ok(project) => project,
        Err(err) => {
            rfd::MessageDialog::new()
                .set_title("Dark Editor")
                .set_description(format!("Cannot open the project: {err}"))
                .show();
            std::process::exit(1);
        }
    };
    let event_loop = EventLoop::new().expect("an event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        args,
        project: Some(project),
        running: None,
    };
    if let Err(err) = event_loop.run_app(&mut app) {
        tracing::error!("{err}");
    }
}

struct App {
    args: Args,
    project: Option<Project>,
    running: Option<Running>,
}

struct Running {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    editor: Editor,
    frame: u32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        let Some(project) = self.project.take() else {
            return;
        };
        let attributes = Window::default_attributes()
            .with_title("Dark Editor")
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        let window = Arc::new(event_loop.create_window(attributes).expect("a window"));
        match pollster::block_on(start(window, project, &self.args)) {
            Ok(running) => self.running = Some(running),
            Err(err) => {
                tracing::error!("cannot start: {err}");
                event_loop.exit();
            }
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, Some(r)) = (cause, &self.running) {
            r.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.running else {
            return;
        };
        let response = r.egui_state.on_window_event(&r.window, &event);
        if response.repaint {
            r.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                r.editor.request_quit();
                r.window.request_redraw();
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    r.config.width = size.width;
                    r.config.height = size.height;
                    r.surface.configure(&r.device, &r.config);
                }
                r.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let script = &mut self.args.script;
                let input = script.as_mut().and_then(Script::next);
                let scripted = script.as_ref().is_some_and(|s| !s.is_done());
                let shot = self.args.screenshot.clone();
                let last = shot.is_some() && !scripted && r.frame + 1 >= self.args.frames;
                let delay = r.draw(input, last.then_some(shot).flatten());
                r.frame += 1;
                if r.editor.quit || last {
                    event_loop.exit();
                    return;
                }
                if self.args.screenshot.is_some() || scripted || delay.is_zero() {
                    r.window.request_redraw();
                } else {
                    // Asleep until something happens, or until egui asks to be woken.
                    let flow = Instant::now()
                        .checked_add(delay)
                        .map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
                    event_loop.set_control_flow(flow);
                }
            }
            _ => {}
        }
    }
}

async fn start(window: Arc<Window>, project: Project, args: &Args) -> Result<Running, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
        Box::new(window.clone()),
    ));
    let surface = instance
        .create_surface(window.clone())
        .map_err(|e| e.to_string())?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("dark-editor"),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    let size = window.inner_size();
    let mut config = surface
        .get_default_config(&adapter, size.width.max(1), size.height.max(1))
        .ok_or("the window cannot be drawn to")?;
    // egui blends in gamma space: it wants a plain (not sRGB) target, 8 bits a channel like
    // the default (screenshots read it back as such).
    let capabilities = surface.get_capabilities(&adapter);
    let plain = config.format.remove_srgb_suffix();
    if capabilities.formats.contains(&plain) {
        config.format = plain;
    }
    if args.screenshot.is_some() && capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC) {
        config.usage |= wgpu::TextureUsages::COPY_SRC;
    }
    surface.configure(&device, &config);

    let egui = egui::Context::default();
    let egui_state = egui_winit::State::new(
        egui.clone(),
        egui::ViewportId::ROOT,
        &window,
        Some(window.scale_factor() as f32),
        None,
        Some(device.limits().max_texture_dimension_2d as usize),
    );
    let mut egui_renderer = egui_wgpu::Renderer::new(
        &device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );
    let viewport = Viewport::new(&adapter, device.clone(), queue.clone(), &mut egui_renderer);
    let editor = Editor::new(project, viewport, &egui, args.scene.clone());
    window.set_title(&editor.title());
    Ok(Running {
        window,
        surface,
        device,
        queue,
        config,
        egui,
        egui_state,
        egui_renderer,
        editor,
        frame: 0,
    })
}

impl Running {
    /// One frame: the interface, then the map it laid out, then the interface on screen. Returns
    /// how long egui can wait before the next frame.
    fn draw(
        &mut self,
        scripted: Option<Vec<egui::Event>>,
        screenshot: Option<PathBuf>,
    ) -> std::time::Duration {
        let mut input = self.egui_state.take_egui_input(&self.window);
        input.events.extend(scripted.into_iter().flatten());
        let editor = &mut self.editor;
        let mut output = self.egui.run_ui(input, |ui| editor.ui(ui));
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);
        self.window.set_title(&self.editor.title());
        self.editor.render(&mut self.egui_renderer);

        let jobs = self.egui.tessellate(output.shapes, output.pixels_per_point);
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            _ => None,
        };
        if let Some(frame) = frame {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("interface"),
                });
            let commands = self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &jobs,
                &screen,
            );
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("interface"),
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
                self.egui_renderer
                    .render(&mut pass.forget_lifetime(), &jobs, &screen);
            }
            let shot = screenshot.and_then(|path| {
                let usable = self.config.usage.contains(wgpu::TextureUsages::COPY_SRC);
                if !usable {
                    tracing::error!("this GPU cannot read the window back for a screenshot");
                }
                usable.then(|| (path, copy_out(&self.device, &mut encoder, &frame.texture)))
            });
            self.queue
                .submit(commands.into_iter().chain([encoder.finish()]));
            if let Some((path, (buffer, padded))) = shot {
                save_capture(&self.device, &buffer, padded, &self.config, &path);
            }
            self.queue.present(frame);
        }
        for id in &output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        output.textures_delta.clear();
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(std::time::Duration::MAX, |v| v.repaint_delay)
    }
}

fn copy_out(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> (wgpu::Buffer, u32) {
    let (width, height) = (texture.width(), texture.height());
    let padded = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
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
    (buffer, padded)
}

fn save_capture(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    padded: u32,
    config: &wgpu::SurfaceConfiguration,
    path: &std::path::Path,
) {
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    if let Err(err) = device.poll(wgpu::PollType::wait_indefinitely()) {
        tracing::error!("screenshot: {err}");
        return;
    }
    let Ok(mapped) = slice.get_mapped_range() else {
        tracing::error!("screenshot: the buffer did not map");
        return;
    };
    let (width, height) = (config.width, config.height);
    let bgra = matches!(
        config.format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in mapped.chunks(padded as usize).take(height as usize) {
        for px in row[..(width * 4) as usize].chunks(4) {
            let (r, b) = if bgra { (px[2], px[0]) } else { (px[0], px[2]) };
            rgba.extend_from_slice(&[r, px[1], b, 255]);
        }
    }
    match image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8) {
        Ok(()) => tracing::info!("saved {}", path.display()),
        Err(err) => tracing::error!("cannot save {}: {err}", path.display()),
    }
}
