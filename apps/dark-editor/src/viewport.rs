//! The map as the game draws it: the scene built as the game builds it (`Map::preview`), laid
//! out by `dark_view` like the game lays it out, rendered by `dark_render` offscreen, and shown
//! as an image in the interface.

use std::collections::HashMap;

use dark_assets::{LoadedSheet, Project, SceneDef};
use dark_render::{Renderer, Sprite, SpriteKind, TextureId, layer};
use dark_sprite::Facing;
use dark_view::MapView;
use dark_world::Map;
use glam::Vec2;

/// Where the view looks and how close.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// World pixel at the middle of the viewport.
    pub center: Vec2,
    /// Screen pixels per world pixel.
    pub zoom: u32,
}

pub const MAX_ZOOM: u32 = 6;

/// The viewport's mapping between the interface (points) and the world (pixels), for one frame.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    /// The viewport's top-left, in points.
    pub min: egui::Pos2,
    /// World pixel at the viewport's top-left.
    pub origin: Vec2,
    /// Points per world pixel.
    pub scale: f32,
}

impl Mapping {
    /// `rect` in points, `ppp` pixels per point.
    pub fn new(rect: egui::Rect, ppp: f32, view: View) -> (Self, (u32, u32)) {
        let zoom = view.zoom.max(1) as f32;
        let size = (
            ((rect.width() * ppp) / zoom).ceil().max(1.0) as u32,
            ((rect.height() * ppp) / zoom).ceil().max(1.0) as u32,
        );
        let internal = Vec2::new(size.0 as f32, size.1 as f32);
        // The renderer rounds its origin the same way, so what is drawn lines up with this.
        let origin = (view.center - internal / 2.0).round();
        let mapping = Self {
            min: rect.min,
            origin,
            scale: zoom / ppp,
        };
        (mapping, size)
    }

    pub fn to_world(self, p: egui::Pos2) -> Vec2 {
        self.origin + Vec2::new(p.x - self.min.x, p.y - self.min.y) / self.scale
    }

    pub fn to_screen(self, w: Vec2) -> egui::Pos2 {
        let v = (w - self.origin) * self.scale;
        egui::pos2(self.min.x + v.x, self.min.y + v.y)
    }

    pub fn rect(self, (x, y, w, h): (f32, f32, f32, f32)) -> egui::Rect {
        egui::Rect::from_min_max(
            self.to_screen(Vec2::new(x, y)),
            self.to_screen(Vec2::new(x + w, y + h)),
        )
    }
}

/// A character drawn standing where the scene puts it.
pub struct Figure<'a> {
    pub sheet: &'a str,
    pub at: (f32, f32),
    pub facing: Facing,
}

pub struct Viewport {
    renderer: Renderer,
    device: wgpu::Device,
    /// Every sheet loaded so far, by path.
    pub sheets: HashMap<String, LoadedSheet>,
    textures: HashMap<String, TextureId>,
    white: TextureId,
    /// The scene as built, and its sprites; `None` until a scene builds.
    pub map: Option<Map>,
    view: Option<MapView>,
    /// The rendered target, as egui knows it.
    pub image: egui::TextureId,
    size: (u32, u32),
    jump_apex: f32,
}

impl Viewport {
    pub fn new(
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        egui: &mut egui_wgpu::Renderer,
    ) -> Self {
        let size = (64, 64);
        let mut renderer = Renderer::offscreen(adapter, device.clone(), queue, size);
        let white = renderer
            .create_texture("white", 1, 1, &[255; 4])
            .expect("a 1x1 texture always fits");
        let image = egui.register_native_texture(
            &device,
            &renderer.shared_view(),
            wgpu::FilterMode::Nearest,
        );
        let params = dark_physics::MoveParams::default();
        Self {
            renderer,
            device,
            sheets: HashMap::new(),
            textures: HashMap::new(),
            white,
            map: None,
            view: None,
            image,
            size,
            jump_apex: params.jump_speed * params.jump_speed / (2.0 * params.gravity),
        }
    }

    /// Loads sheet `path` (and uploads its image) if it is not loaded yet.
    pub fn load_sheet(&mut self, project: &Project, path: &str) -> Result<&LoadedSheet, String> {
        if !self.sheets.contains_key(path) {
            let loaded = project.load_sheet(path).map_err(|e| e.to_string())?;
            let image = &loaded.image;
            let id = self
                .renderer
                .create_texture(path, image.width, image.height, &image.rgba)
                .map_err(|e| e.to_string())?;
            self.textures.insert(path.to_owned(), id);
            self.sheets.insert(path.to_owned(), loaded);
        }
        Ok(&self.sheets[path])
    }

    /// Builds scene `path` (`def`) as the game would, with other scenes' exits landing at
    /// `landings`. On failure the last good build stays on screen.
    pub fn rebuild(
        &mut self,
        project: &Project,
        path: &str,
        def: &SceneDef,
        tile: u32,
        landings: &[(f32, f32)],
    ) -> Result<(), String> {
        let sheets = std::iter::once(def.ground.sheet.as_str())
            .chain(def.props.iter().map(|p| p.sheet.as_str()))
            .chain(def.scatter.iter().map(|s| s.sheet.as_str()));
        for sheet in sheets.collect::<Vec<_>>() {
            self.load_sheet(project, sheet)?;
        }
        let map = Map::preview(path, def.clone(), tile, landings)?;
        // Props naming frames their sheet does not have are skipped by the view (and warned).
        self.view = Some(MapView::build(
            &map,
            &self.sheets,
            &self.textures,
            self.white,
            self.jump_apex,
        ));
        self.map = Some(map);
        Ok(())
    }

    /// Ground height under `at` in the built scene.
    pub fn ground(&self, at: Vec2) -> f32 {
        self.map
            .as_ref()
            .map(|m| m.collision.ground_under(at, 0.5))
            .filter(|g| g.is_finite())
            .unwrap_or(0.0)
    }

    /// Renders the scene at `size` world pixels around `origin`, with its characters standing
    /// and, if asked, the collision overlay; then points egui at the result.
    pub fn render(
        &mut self,
        egui: &mut egui_wgpu::Renderer,
        size: (u32, u32),
        origin: Vec2,
        figures: &[Figure],
        overlay: bool,
    ) {
        if size != self.size {
            self.size = size;
            self.renderer.set_internal_size(size);
            egui.update_egui_texture_from_wgpu_texture(
                &self.device,
                &self.renderer.shared_view(),
                wgpu::FilterMode::Nearest,
                self.image,
            );
        }
        let mut sprites: Vec<Sprite> = Vec::new();
        if let Some(view) = &self.view {
            sprites.extend(view.statics.iter().cloned());
            if overlay {
                sprites.extend(view.overlay.iter().cloned().map(|mut s| {
                    s.layer = layer::DEBUG;
                    s
                }));
            }
        }
        for figure in figures {
            if let Some(sprite) = self.figure(figure) {
                sprites.push(sprite);
            }
        }
        let half = Vec2::new(size.0 as f32, size.1 as f32) / 2.0;
        self.renderer
            .render(origin + half, [0.02, 0.02, 0.025], &mut sprites);
    }

    fn figure(&self, figure: &Figure) -> Option<Sprite> {
        let loaded = self.sheets.get(figure.sheet)?;
        // Skeletons have no still frames to show; the interface marks where they stand.
        if loaded.spine.is_some() {
            return None;
        }
        let sheet = &loaded.sheet;
        let clip = sheet
            .clip_id(&format!("idle_{}", figure.facing.cardinal().name()))
            .or_else(|| sheet.clip_id("idle_down"))
            .map(|id| &sheet.clips[id.0 as usize]);
        let (index, flip) = clip
            .and_then(|c| c.frames.first().map(|f| (*f, c.flip_x)))
            .unwrap_or((0, false));
        let frame = sheet.frames.get(index as usize)?;
        let at = Vec2::from(figure.at);
        let mut sprite = Sprite::new(
            self.textures[figure.sheet],
            frame.rect,
            at - Vec2::new(0.0, self.ground(at)),
            frame.pivot,
        );
        sprite.flip_x = flip;
        sprite.sort_y = at.y;
        // Shown through what stands in front, as the game shows them.
        sprite.kind = SpriteKind::Character;
        sprite.layer = layer::WORLD;
        Some(sprite)
    }
}
