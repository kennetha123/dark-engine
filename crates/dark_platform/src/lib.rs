//! Window and OS event loop (winit).

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

pub use winit;
pub use winit::keyboard::KeyCode;

/// Returned from [`Game::frame`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit,
}

#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Dark Engine".into(),
            width: 1280,
            height: 720,
        }
    }
}

/// Callbacks from the platform loop into the game.
pub trait Game {
    /// The window exists; create GPU resources here.
    fn init(&mut self, window: Arc<Window>);
    fn resized(&mut self, width: u32, height: u32);
    /// One rendered frame; `dt` is real time since the previous frame.
    fn frame(&mut self, dt: Duration) -> Flow;
    /// A physical key changed state. Auto-repeat is filtered out.
    fn key(&mut self, _key: KeyCode, _pressed: bool) {}
    /// The window lost focus; held keys will not report their release.
    fn focus_lost(&mut self) {}
    /// The window is closing. Last chance for a graceful network quit.
    fn exiting(&mut self);
}

pub fn run(config: WindowConfig, game: impl Game) -> Result<(), winit::error::EventLoopError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = Runner {
        config,
        game,
        window: None,
        last_frame: Instant::now(),
    };
    event_loop.run_app(&mut runner)
}

struct Runner<G> {
    config: WindowConfig,
    game: G,
    window: Option<Arc<Window>>,
    last_frame: Instant,
}

impl<G: Game> ApplicationHandler for Runner<G> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height));
        match event_loop.create_window(attributes) {
            Ok(window) => {
                let window = Arc::new(window);
                self.game.init(window.clone());
                self.window = Some(window);
                self.last_frame = Instant::now();
            }
            Err(err) => {
                tracing::error!("failed to create window: {err}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.game.exiting();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.game.resized(size.width, size.height),
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = now - self.last_frame;
                self.last_frame = now;
                if self.game.frame(dt) == Flow::Exit {
                    self.game.exiting();
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && !event.repeat
                {
                    self.game.key(code, event.state.is_pressed());
                }
            }
            WindowEvent::Focused(false) => self.game.focus_lost(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
