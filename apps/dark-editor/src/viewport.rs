//! The map as the game draws it: the scene built as the game builds it (`Map::preview`), laid
//! out by `dark_view` like the game lays it out, rendered by `dark_render` offscreen, and shown
//! as an image in the interface.

use std::collections::HashMap;

use dark_assets::{LoadedSheet, Project, SceneDef};
use dark_render::{Mesh, MeshVertex, Renderer, Sprite, SpriteKind, TextureId, layer};
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
    /// Skeletons of Spine sheets, drawn standing in their idle pose.
    skeletons: HashMap<String, Skeleton>,
}

/// A Spine sheet's skeleton, its pages uploaded, and which animation plays each clip.
struct Skeleton {
    rig: dark_spine::Rig,
    pages: Vec<TextureId>,
    clips: HashMap<String, (String, bool)>,
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
            skeletons: HashMap::new(),
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
            // A skeleton draws from its own pages (painted art, so sampled smoothly).
            if let Some(spine) = &loaded.spine {
                match self.skeleton_of(project, path, spine) {
                    Ok(skeleton) => {
                        self.skeletons.insert(path.to_owned(), skeleton);
                    }
                    // The sheet still works (its bake is what the game plays); it is only not drawn.
                    Err(e) => tracing::warn!("{path}: the skeleton cannot be drawn: {e}"),
                }
            }
            self.sheets.insert(path.to_owned(), loaded);
        }
        Ok(&self.sheets[path])
    }

    /// A Spine sheet's rig, with its pages uploaded.
    fn skeleton_of(
        &mut self,
        project: &Project,
        path: &str,
        spine: &dark_assets::SpineSheet,
    ) -> Result<Skeleton, String> {
        let rig = dark_spine::Rig::load(project, &spine.def).map_err(|e| e.to_string())?;
        let pages = rig
            .pages
            .iter()
            .map(|page| {
                let image = &page.image;
                self.renderer.create_texture_smooth(
                    &format!("{path} {}", page.name),
                    image.width,
                    image.height,
                    &image.rgba,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let clips = spine
            .bake
            .clips
            .iter()
            .map(|(name, c)| (name.clone(), (c.animation.clone(), c.looping)))
            .collect();
        Ok(Skeleton { rig, pages, clips })
    }

    /// Whether figures wearing `sheet` are drawn (a skeleton that did not load is not).
    pub fn draws(&self, sheet: &str) -> bool {
        self.sheets
            .get(sheet)
            .is_some_and(|s| s.spine.is_none() || self.skeletons.contains_key(sheet))
    }

    /// Drops sheet `path`, so the next use loads it again (it was changed and saved).
    pub fn forget_sheet(&mut self, path: &str) {
        self.sheets.remove(path);
        self.textures.remove(path);
        self.skeletons.remove(path);
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
        // And the pictures the places stamped on it are drawn with, which are named in their own
        // scenes rather than in this one.
        let mut stamped = def.clone();
        if stamped.stamp_places(project, tile, 0).is_ok() {
            for sheet in stamped
                .props
                .iter()
                .map(|p| p.sheet.clone())
                .collect::<Vec<_>>()
            {
                self.load_sheet(project, &sheet)?;
            }
        }
        let map = Map::preview(project, path, def.clone(), tile, landings)?;
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
        // Only the pieces of the map this view can see (docs/PLAN.md §24.2), so what a frame
        // costs here is the size of the panel and not the size of the map.
        let seen = origin + Vec2::new(size.0 as f32, size.1 as f32);
        let mut sprites: Vec<Sprite> = Vec::new();
        if let (Some(view), Some(map)) = (&mut self.view, &self.map) {
            // A piece of the map is made when the panel reaches it (docs/PLAN.md §24.4).
            let scenery = dark_view::Scenery {
                map,
                sheets: &self.sheets,
                textures: &self.textures,
            };
            view.seen(&scenery, origin, seen, &mut sprites);
            if overlay {
                view.seen_overlay(&scenery, origin, seen, &mut sprites, |mut s| {
                    s.layer = layer::DEBUG;
                    s
                });
            }
        }
        let mut meshes = Vec::new();
        for figure in figures {
            if let Some(sprite) = self.figure(figure) {
                sprites.push(sprite);
            }
            meshes.extend(self.skeleton(figure));
        }
        let half = Vec2::new(size.0 as f32, size.1 as f32) / 2.0;
        self.renderer
            .render_with(origin + half, [0.02, 0.02, 0.025], &mut sprites, &meshes);
    }

    /// A skeletal figure, posed as its idle clip begins, as meshes standing at its feet.
    fn skeleton(&self, figure: &Figure) -> Vec<Mesh> {
        let Some(skeleton) = self.skeletons.get(figure.sheet) else {
            return Vec::new();
        };
        let idle = format!("idle_{}", figure.facing.cardinal().name());
        let Some((animation, looping)) = skeleton
            .clips
            .get(&idle)
            .or_else(|| skeleton.clips.get("idle_down"))
            .or_else(|| skeleton.clips.values().next())
        else {
            return Vec::new();
        };
        let mut pose = dark_spine::Pose::new(&skeleton.rig);
        pose.pose(animation, *looping, 0.0);
        let at = Vec2::from(figure.at);
        let feet = at - Vec2::new(0.0, self.ground(at));
        pose.meshes(&skeleton.rig)
            .into_iter()
            .map(|mesh| Mesh {
                texture: skeleton.pages[mesh.page],
                vertices: mesh
                    .vertices
                    .iter()
                    .map(|v| MeshVertex {
                        position: feet + v.position,
                        uv: v.uv,
                        color: v.color,
                    })
                    .collect(),
                layer: layer::WORLD,
                sort_y: at.y,
                body: true,
            })
            .collect()
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
