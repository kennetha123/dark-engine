//! Spine skeletons (docs/PLAN.md §16): posed and turned into triangles for the game's view, and
//! baked once for the host, which never runs Spine.
//!
//! The runtime is `rusty_spine`, spine-c transpiled to Rust, pinned to Spine 4.1 (the editor
//! the studio exports from). A [`Rig`] is one loaded skeleton (atlas pages as images, skeleton
//! data scaled to world pixels); a [`Pose`] is one character's skeleton on it. Posing is exact:
//! the view asks for an animation at a time, as the host's replicated clip and frame say, so what
//! is drawn is what the host simulated.

mod raw;

use std::path::Path;
use std::sync::Arc;

use dark_assets::{AssetError, BakedClip, Image, Project, SpineBake, SpineDef, skeleton_hash};
use glam::Vec2;
use rusty_spine::{
    AnimationEvent, AnimationState, AnimationStateData, Atlas, Skeleton, SkeletonData,
    SkeletonJson,
    draw::{ColorSpace, CullDirection, SimpleDrawer},
};

/// Bounding boxes whose slot or attachment name contains this are baked as hitboxes; `hurtbox`
/// likewise. Draw them where the blow lands (or the body stands) on the ground.
pub const HITBOX: &str = "hitbox";
pub const HURTBOX: &str = "hurtbox";
/// Simulation ticks per second, as `dark_core` runs.
const TICK_RATE: f32 = 60.0;

#[derive(Debug, thiserror::Error)]
pub enum SpineError {
    #[error(transparent)]
    Asset(#[from] AssetError),
    #[error("{path}: {message}")]
    Spine { path: String, message: String },
}

fn spine_error(path: &Path, err: impl std::fmt::Display) -> SpineError {
    SpineError::Spine {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// An atlas page, ready for the GPU: straight alpha, shrunk to the size it is drawn at.
pub struct Page {
    pub name: String,
    pub image: Image,
}

/// One loaded skeleton: its data and atlas pages.
pub struct Rig {
    data: Arc<SkeletonData>,
    state_data: Arc<AnimationStateData>,
    /// The atlas owns the regions the skeleton's attachments point into.
    _atlas: Arc<Atlas>,
    pub pages: Vec<Page>,
    hash: String,
    /// The skeleton's size in the export is not scaled by the reader.
    scale: f32,
}

/// The atlas pages `def`'s skeleton draws from, named as they are beside the atlas. Packaging
/// a build ships these; loading names them the same way.
pub fn atlas_pages(project: &Project, def: &SpineDef) -> Result<Vec<String>, SpineError> {
    raw::name_pages();
    let path = project.path(&def.atlas);
    let atlas = Atlas::new_from_file(&path).map_err(|e| spine_error(&path, e))?;
    Ok(atlas.pages().map(|page| page.name().to_owned()).collect())
}

impl Rig {
    /// Loads `def`'s skeleton and atlas from `project`, scaled to world pixels.
    pub fn load(project: &Project, def: &SpineDef) -> Result<Self, SpineError> {
        raw::name_pages();
        let atlas_path = project.path(&def.atlas);
        let atlas_text =
            std::fs::read_to_string(&atlas_path).map_err(|e| spine_error(&atlas_path, e))?;
        let atlas =
            Arc::new(Atlas::new_from_file(&atlas_path).map_err(|e| spine_error(&atlas_path, e))?);
        let skeleton_path = project.path(&def.skeleton);
        let json =
            std::fs::read_to_string(&skeleton_path).map_err(|e| spine_error(&skeleton_path, e))?;
        let mut reader = SkeletonJson::new(atlas.clone());
        reader.set_scale(def.scale);
        let data = Arc::new(
            reader
                .read_skeleton_data(json.as_bytes())
                .map_err(|e| spine_error(&skeleton_path, e))?,
        );
        let state_data = Arc::new(AnimationStateData::new(data.clone()));
        // Pages beside the atlas, shrunk as the skeleton is (painted art drawn small), straight
        // alpha for the renderer's blending.
        let dir = atlas_path.parent().unwrap_or(Path::new("."));
        let premultiplied = atlas_text.lines().any(|l| l.trim() == "pma:true");
        let mut pages = Vec::new();
        for page in atlas.pages() {
            let name = page.name().to_owned();
            let image = dark_assets::load_image(&dir.join(&name))?;
            pages.push(Page {
                name,
                image: prepare(image, def.texture_scale.unwrap_or(def.scale), premultiplied),
            });
        }
        Ok(Self {
            data,
            state_data,
            _atlas: atlas,
            pages,
            hash: skeleton_hash(&json).unwrap_or_default(),
            scale: def.scale,
        })
    }

    /// Height of the skeleton's setup-pose bounds, in world pixels.
    pub fn height(&self) -> f32 {
        self.data.height() * self.scale
    }

    /// The `hash` of the skeleton export as loaded.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn has_animation(&self, name: &str) -> bool {
        self.data.animations().any(|a| a.name() == name)
    }
}

/// `image` at `scale`, straight alpha. Shrinking averages premultiplied colour, so edges do not
/// darken; only then is alpha divided out.
fn prepare(image: Image, scale: f32, premultiplied: bool) -> Image {
    let mut rgba = image::RgbaImage::from_raw(image.width, image.height, image.rgba)
        .expect("a decoded image matches its size");
    if !premultiplied {
        for p in rgba.pixels_mut() {
            let a = u16::from(p[3]);
            for c in 0..3 {
                p[c] = ((u16::from(p[c]) * a + 127) / 255) as u8;
            }
        }
    }
    if scale < 1.0 {
        let w = ((image.width as f32 * scale).round() as u32).max(1);
        let h = ((image.height as f32 * scale).round() as u32).max(1);
        rgba = image::imageops::resize(&rgba, w, h, image::imageops::FilterType::CatmullRom);
    }
    for p in rgba.pixels_mut() {
        let a = u16::from(p[3]);
        for c in 0..3 {
            if let Some(v) = (u16::from(p[c]) * 255 + a / 2).checked_div(a) {
                p[c] = v.min(255) as u8;
            }
        }
    }
    bleed(&mut rgba);
    Image {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }
}

/// Gives fully transparent texels next to art that art's colour (alpha stays 0), a couple of
/// texels out: smooth filtering blends neighbours, and straight-alpha black there would draw
/// dark fringes round every edge.
fn bleed(rgba: &mut image::RgbaImage) {
    let (w, h) = rgba.dimensions();
    for _ in 0..2 {
        let before = rgba.clone();
        for y in 0..h {
            for x in 0..w {
                if before.get_pixel(x, y)[3] != 0 {
                    continue;
                }
                let (mut sum, mut n) = ([0u32; 3], 0u32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let q = before.get_pixel(nx as u32, ny as u32);
                    // Art, or a texel already given colour by the previous round.
                    if q[3] != 0 || q[0] != 0 || q[1] != 0 || q[2] != 0 {
                        for c in 0..3 {
                            sum[c] += u32::from(q[c]);
                        }
                        n += 1;
                    }
                }
                let p = rgba.get_pixel_mut(x, y);
                for c in 0..3 {
                    if let Some(average) = sum[c].checked_div(n) {
                        p[c] = average as u8;
                    }
                }
            }
        }
    }
}

/// Triangles on one atlas page, three vertices each; positions in world pixels from the feet
/// (y down).
pub struct SpineMesh {
    pub page: usize,
    pub vertices: Vec<SpineVertex>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpineVertex {
    pub position: Vec2,
    pub uv: Vec2,
    pub color: [f32; 4],
}

/// One character's skeleton, posed on request.
pub struct Pose {
    skeleton: Skeleton,
    state: AnimationState,
    playing: Option<(String, bool)>,
    time: f32,
}

impl Pose {
    pub fn new(rig: &Rig) -> Self {
        let mut skeleton = Skeleton::new(rig.data.clone());
        skeleton.set_to_setup_pose();
        Self {
            skeleton,
            state: AnimationState::new(rig.state_data.clone()),
            playing: None,
            time: 0.0,
        }
    }

    /// Poses the skeleton `time` seconds into `animation` (no blending: exactly that moment).
    pub fn pose(&mut self, animation: &str, looping: bool, time: f32) {
        let same = self
            .playing
            .as_ref()
            .is_some_and(|(a, l)| a == animation && *l == looping);
        if !same || time < self.time {
            self.skeleton.set_to_setup_pose();
            if self
                .state
                .set_animation_by_name(0, animation, looping)
                .is_err()
            {
                tracing::warn!("no Spine animation {animation}");
            }
            self.playing = Some((animation.to_owned(), looping));
            self.time = 0.0;
        }
        self.state.update(time - self.time);
        self.time = time;
        self.state.apply(&mut self.skeleton);
        self.skeleton.update_world_transform();
    }

    /// The posed skeleton as triangles, back to front.
    pub fn meshes(&mut self, rig: &Rig) -> Vec<SpineMesh> {
        let drawer = SimpleDrawer {
            cull_direction: CullDirection::Clockwise,
            premultiplied_alpha: false,
            color_space: ColorSpace::Linear,
        };
        let mut out: Vec<SpineMesh> = Vec::new();
        for r in drawer.draw(&mut self.skeleton, None) {
            let Some(page) = raw::page_name(r.attachment_renderer_object)
                .and_then(|name| rig.pages.iter().position(|p| p.name == name))
            else {
                continue;
            };
            let color = [r.color.r, r.color.g, r.color.b, r.color.a];
            let vertex = |i: u16| {
                let i = usize::from(i);
                let [x, y] = r.vertices[i];
                SpineVertex {
                    position: Vec2::new(x, -y),
                    uv: Vec2::from(r.uvs[i]),
                    color,
                }
            };
            let vertices = r.indices.iter().map(|&i| vertex(i));
            match out.last_mut() {
                Some(mesh) if mesh.page == page => mesh.vertices.extend(vertices),
                _ => out.push(SpineMesh {
                    page,
                    vertices: vertices.collect(),
                }),
            }
        }
        out
    }

    /// Circles around the bounding boxes whose slot or attachment name contains `name`, on the
    /// ground from the feet.
    fn boxes(&self, name: &str) -> Option<(f32, f32, f32)> {
        let polygons = raw::bounding_boxes(&self.skeleton, name);
        let points: Vec<Vec2> = polygons
            .iter()
            .flatten()
            .map(|&[x, y]| Vec2::new(x, -y))
            .collect();
        if points.is_empty() {
            return None;
        }
        let centre = points.iter().copied().sum::<Vec2>() / points.len() as f32;
        let radius = points
            .iter()
            .map(|p| p.distance(centre))
            .fold(0.0, f32::max);
        Some((round2(centre.x), round2(centre.y), round2(radius)))
    }
}

/// Two decimals: baked numbers stay short and the same everywhere.
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// Measures every clip `def` names, a tick at a time: length, events, hitboxes and hurtboxes.
pub fn bake(project: &Project, def: &SpineDef) -> Result<SpineBake, SpineError> {
    let rig = Rig::load(project, def)?;
    let skeleton_path = project.path(&def.skeleton);
    let mut clips = std::collections::BTreeMap::new();
    for (name, animation, looping) in def.clips() {
        let Some(duration) = rig
            .data
            .animations()
            .find(|a| a.name() == animation)
            .map(|a| a.duration())
        else {
            return Err(spine_error(
                &skeleton_path,
                format!("no animation {animation} (for clip {name})"),
            ));
        };
        // A clip that plays once shows its last key too (frames 0 to the duration); one that
        // loops comes back round to its first instead.
        let span = duration * TICK_RATE;
        let ticks = if looping {
            (span.round() as u32).max(1)
        } else {
            span.floor() as u32 + 1
        };
        let mut pose = Pose::new(&rig);
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = events.clone();
        let strike = def.strike_event.clone();
        pose.state.set_listener(move |_, event| {
            if let AnimationEvent::Event { name, time, .. } = event {
                // Spine fires a key on the first update at or past it; a hair's allowance for
                // times exported as 0.3333.
                let tick = ((time * TICK_RATE) - 1e-3).ceil().max(0.0) as u32;
                let named = if strike.as_deref() == Some(name) {
                    "strike".to_owned()
                } else {
                    name.to_owned()
                };
                if let Ok(mut events) = heard.lock() {
                    events.push((named, tick));
                }
            }
        });
        let (mut hitboxes, mut hurtboxes) = (Vec::new(), Vec::new());
        for tick in 0..ticks {
            pose.pose(&animation, false, tick as f32 / TICK_RATE);
            hitboxes.push(pose.boxes(HITBOX));
            hurtboxes.push(pose.boxes(HURTBOX));
        }
        // Past the last frame, so a key on it fires too.
        pose.pose(&animation, false, duration + 1.0 / TICK_RATE);
        let mut events = events.lock().map(|e| e.clone()).unwrap_or_default();
        for (_, tick) in &mut events {
            *tick = (*tick).min(ticks - 1);
        }
        events.sort_by_key(|(_, tick)| *tick);
        events.dedup();
        let keep = |boxes: Vec<Option<(f32, f32, f32)>>| {
            if boxes.iter().all(Option::is_none) {
                Vec::new()
            } else {
                boxes
            }
        };
        clips.insert(
            name,
            BakedClip {
                animation,
                ticks,
                looping,
                events,
                hitboxes: keep(hitboxes),
                hurtboxes: keep(hurtboxes),
            },
        );
    }
    Ok(SpineBake {
        skeleton_hash: rig.hash.clone(),
        height: round2(rig.height()),
        clips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A skeleton with no art: a `hitbox` bounding box (a 10-unit square centred 20 units up) on
    /// a bone that moves 30 units right over half a second, and a `swing` event at 0.25 s.
    const SKELETON: &str = r#"{
        "skeleton": {"hash": "test", "spine": "4.1.24", "x": 0, "y": 0, "width": 40, "height": 40},
        "bones": [{"name": "root"}, {"name": "arm", "parent": "root"}],
        "slots": [{"name": "hitbox", "bone": "arm", "attachment": "hitbox"}],
        "skins": [{"name": "default", "attachments": {"hitbox": {"hitbox": {
            "type": "boundingbox", "vertexCount": 4,
            "vertices": [-5, 15, 5, 15, 5, 25, -5, 25]
        }}}}],
        "events": {"swing": {}},
        "animations": {"S_atk": {
            "bones": {"arm": {"translate": [{"time": 0, "x": 0}, {"time": 0.5, "x": 30}]}},
            "events": [{"time": 0.25, "name": "swing"}]
        }}
    }"#;

    #[test]
    fn a_hitbox_is_baked_on_the_ground_each_tick_and_the_strike_is_named() {
        let dir = std::env::temp_dir().join(format!("dark_spine_bake_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        std::fs::write(dir.join("s.json"), SKELETON).unwrap();
        // One page and no regions: the skeleton has no art.
        std::fs::write(
            dir.join("s.atlas"),
            "s.png
size:1,1
filter:Linear,Linear
",
        )
        .unwrap();
        image::RgbaImage::new(1, 1).save(dir.join("s.png")).unwrap();
        let project = Project::open(&dir).unwrap();
        let def: SpineDef = ron::from_str(
            r#"(skeleton: "s.json", atlas: "s.atlas", baked: "s.baked.ron", scale: 0.5,
                actions: {"attack": "atk"}, directions: {"down": "S"}, strike_event: Some("swing"))"#,
        )
        .unwrap();
        let bake = bake(&project, &def).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bake.skeleton_hash, "test");
        assert_eq!(bake.height, 20.0, "40 units at half scale");
        let clip = &bake.clips["attack_down"];
        assert_eq!(clip.ticks, 31, "half a second, both ends shown");
        assert_eq!(clip.events, vec![("strike".to_owned(), 15)]);
        assert!(clip.hurtboxes.is_empty());
        // Half scale: centred 10 px up the screen (y down), radius half the diagonal of a 5 px
        // square; the bone carries it 15 px right over the clip.
        let (x0, y0, r0) = clip.hitboxes[0].unwrap();
        assert_eq!((x0, y0), (0.0, -10.0));
        assert!((r0 - 3.54).abs() < 0.01, "{r0}");
        let (x_end, ..) = clip.hitboxes[30].unwrap();
        assert!((x_end - 15.0).abs() < 0.01, "{x_end}");
    }
}
