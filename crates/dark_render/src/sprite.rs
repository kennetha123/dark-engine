//! CPU side of sprite drawing: what a sprite is, ordering, batching and pixel-perfect layout.

use dark_sprite::Rect;
use glam::Vec2;

/// Handle to a texture created by [`crate::Renderer::create_texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub(crate) u32);

impl TextureId {
    /// The first texture a renderer makes. Laying sprites out and sorting them does not care
    /// which texture it is; anything actually drawn wants an id the renderer gave out.
    pub const FIRST: Self = Self(0);
}

/// How a sprite is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpriteKind {
    #[default]
    Plain,
    /// A character: only its opaque pixels are drawn (a soft shadow baked into the art is dropped;
    /// draw a [`SpriteKind::Blob`] on the surface below instead, which stays down when it jumps).
    /// Wherever something drawn later covers the body, a silhouette shows through it.
    Character,
    /// A filled ellipse over the sprite's quad in its colour: blob shadows.
    Blob,
    /// A 1 px outline of the sprite's quad in its colour (debug shapes).
    Outline(Outline),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outline {
    Circle,
    Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub texture: TextureId,
    /// Source rectangle in texels.
    pub src: Rect,
    /// World position (pixels) of the pivot.
    pub position: Vec2,
    /// Pivot in texels from `src`'s top-left.
    pub pivot: Vec2,
    /// Draw the source this many times across and down (seamless ground). `(1, 1)` normally.
    pub repeat: Vec2,
    pub flip_x: bool,
    /// Linear RGBA multiplier.
    pub color: [f32; 4],
    /// Coarse ordering: ground < world < overhead.
    pub layer: i32,
    /// Fine ordering within a layer: larger draws later (in front). Usually the feet's world y.
    pub sort_y: f32,
    /// Pixels to draw above `position` (height off the ground, e.g. a jump).
    pub lift: f32,
    pub kind: SpriteKind,
}

impl Sprite {
    /// A standing sprite, Y-sorted by its pivot.
    pub fn new(texture: TextureId, src: Rect, position: Vec2, pivot: Vec2) -> Self {
        Self {
            texture,
            src,
            position,
            pivot,
            repeat: Vec2::ONE,
            flip_x: false,
            color: [1.0; 4],
            layer: 0,
            sort_y: position.y,
            lift: 0.0,
            kind: SpriteKind::Plain,
        }
    }

    /// The rectangle this sprite covers once drawn, in world pixels: where it is *drawn*, not
    /// where it stands, so a tall tree covers the ground above its feet and a jumping character
    /// covers where it is in the air. What decides whether it is worth drawing at all.
    pub fn covers(&self) -> (Vec2, Vec2) {
        let size = Vec2::new(self.src.w as f32, self.src.h as f32) * self.repeat;
        let min = self.position - self.pivot - Vec2::new(0.0, self.lift);
        (min, min + size)
    }

    /// A flat rectangle of `color`, from a 1×1 white texture.
    pub fn fill(white: TextureId, top_left: Vec2, size: Vec2, color: [f32; 4]) -> Self {
        let mut s = Self::new(white, Rect::new(0, 0, 1, 1), top_left, Vec2::ZERO);
        s.repeat = size;
        s.color = color;
        s
    }

    /// A 1 px outline of a circle or rectangle, from a 1×1 white texture.
    pub fn outline(
        white: TextureId,
        shape: Outline,
        top_left: Vec2,
        size: Vec2,
        color: [f32; 4],
    ) -> Self {
        Self {
            kind: SpriteKind::Outline(shape),
            ..Self::fill(white, top_left, size, color)
        }
    }
}

/// Textured triangles in world pixels (a posed skeleton), sorted with sprites by `layer` and
/// `sort_y`. Meshes never hide anything; a `body` (a character) shows as a silhouette where a
/// sprite drawn after it covers it, like a [`SpriteKind::Character`].
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub texture: TextureId,
    /// Three per triangle.
    pub vertices: Vec<MeshVertex>,
    pub layer: i32,
    pub sort_y: f32,
    pub body: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshVertex {
    pub position: Vec2,
    /// Texture coordinates, 0 to 1 across the texture.
    pub uv: Vec2,
    /// Linear RGBA multiplier.
    pub color: [f32; 4],
}

pub mod layer {
    pub const GROUND: i32 = -100;
    /// Raised terrain tops, `TERRAIN + level`: floors below everything standing on them.
    pub const TERRAIN: i32 = -50;
    pub const WORLD: i32 = 0;
    pub const OVERHEAD: i32 = 100;
    /// Speech bubbles, prompts and other interface drawn in the world. From here up nothing hides
    /// a character (no silhouettes behind a bubble).
    pub const UI: i32 = 500;
    pub const DEBUG: i32 = 1000;
}

/// Instance draw modes; must match `sprite.wgsl`.
pub(crate) mod mode {
    pub const PLAIN: u32 = 0;
    pub const BLOB: u32 = 1;
    /// Only the opaque pixels (a character's body).
    pub const BODY: u32 = 2;
    pub const CIRCLE: u32 = 3;
    pub const RECT: u32 = 4;
}

/// GPU instance data; layout matches `sprite.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Instance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub repeat: [f32; 2],
    pub color: [f32; 4],
    /// 1-based draw order, compared in the silhouette pass.
    pub order: f32,
    pub mode: u32,
}

/// A run of consecutive instances (or, for `triangles`, mesh vertices) sharing one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    pub texture: TextureId,
    pub start: u32,
    pub end: u32,
    pub triangles: bool,
}

/// Everything the GPU passes need for one frame: sprites in one instance buffer, meshes in one
/// vertex buffer (the same layout, one entry per vertex).
#[derive(Debug, Default)]
pub(crate) struct FrameBatches {
    pub instances: Vec<Instance>,
    pub vertices: Vec<Instance>,
    /// Main pass, back to front: the world.
    pub main: Vec<Batch>,
    /// Interface and debug ([`layer::UI`] and up), drawn after silhouettes so none shows through.
    pub overlay: Vec<Batch>,
    /// Sprites drawn after, and overlapping, a character: they write the occlusion mask.
    pub occluders: Vec<Batch>,
    /// Character bodies, drawn again as silhouettes where the mask says they are covered.
    pub silhouettes: Vec<Batch>,
}

/// Something drawn, as the silhouette logic sees it: its place in the draw order, its screen
/// bounds, and where its GPU data is.
#[derive(Clone, Copy)]
struct Drawn {
    /// 1-based position in the main pass, across sprites and meshes.
    seq: u32,
    min: Vec2,
    max: Vec2,
    part: Part,
    texture: TextureId,
}

#[derive(Clone, Copy)]
enum Part {
    /// An instance.
    Sprite(u32),
    /// A run of mesh vertices.
    Mesh(u32, u32),
}

impl Drawn {
    fn overlaps(&self, other: &Drawn) -> bool {
        self.min.x < other.max.x
            && other.min.x < self.max.x
            && self.min.y < other.max.y
            && other.min.y < self.max.y
    }
}

/// Sorts sprites back-to-front and turns them into instances plus texture batches, with meshes
/// drawn among them in the same order. Top-left corners of sprites are snapped to whole pixels
/// so art never shimmers between texels.
pub(crate) fn build_batches(
    sprites: &mut [Sprite],
    meshes: &[Mesh],
    texture_size: impl Fn(TextureId) -> (u32, u32),
) -> FrameBatches {
    // Stable: equal keys keep submission order. total_cmp, because a NaN position must not
    // break the total order sort_by requires.
    let key = |layer: i32, y: f32| (layer, y);
    let before = |a: (i32, f32), b: (i32, f32)| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1));
    sprites.sort_by(|a, b| before(key(a.layer, a.sort_y), key(b.layer, b.sort_y)));
    let mut mesh_order: Vec<usize> = (0..meshes.len()).collect();
    mesh_order.sort_by(|&a, &b| {
        let (a, b) = (&meshes[a], &meshes[b]);
        before(key(a.layer, a.sort_y), key(b.layer, b.sort_y))
    });
    let mut frame = FrameBatches::default();
    let mut seq = 0u32;
    // Everything that can hide a character (with its layer), and the characters.
    let mut can_occlude: Vec<(Drawn, i32)> = Vec::new();
    let mut bodies: Vec<Drawn> = Vec::new();
    let mut next_mesh = mesh_order.iter().peekable();
    let mut draw_meshes_before = |frame: &mut FrameBatches,
                                  seq: &mut u32,
                                  bodies: &mut Vec<Drawn>,
                                  until: Option<(i32, f32)>| {
        // Meshes that sort before `until` (all of them when `None`); a sprite wins ties.
        while let Some(&&m) = next_mesh.peek() {
            let mesh = &meshes[m];
            let due = until.is_none_or(|k| before(key(mesh.layer, mesh.sort_y), k).is_lt());
            if !due {
                break;
            }
            next_mesh.next();
            let start = frame.vertices.len() as u32;
            frame
                .vertices
                .extend(mesh.vertices.iter().map(|v| Instance {
                    pos: v.position.to_array(),
                    size: [0.0; 2],
                    uv_min: v.uv.to_array(),
                    uv_size: [0.0; 2],
                    repeat: [1.0; 2],
                    color: v.color,
                    order: 0.0,
                    mode: mode::PLAIN,
                }));
            let end = frame.vertices.len() as u32;
            if end == start {
                continue;
            }
            append_triangles(&mut frame.main, mesh.texture, start, end);
            *seq += 1;
            if mesh.body {
                let (min, max) = mesh.vertices.iter().fold(
                    (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
                    |(lo, hi), v| (lo.min(v.position), hi.max(v.position)),
                );
                bodies.push(Drawn {
                    seq: *seq,
                    min,
                    max,
                    part: Part::Mesh(start, end),
                    texture: mesh.texture,
                });
            }
        }
    };
    for sprite in sprites.iter() {
        draw_meshes_before(
            &mut frame,
            &mut seq,
            &mut bodies,
            Some(key(sprite.layer, sprite.sort_y)),
        );
        let (tw, th) = texture_size(sprite.texture);
        let size = Vec2::new(sprite.src.w as f32, sprite.src.h as f32) * sprite.repeat;
        let lifted = (sprite.position - sprite.pivot - Vec2::new(0.0, sprite.lift)).round();
        let mut uv_min = [
            sprite.src.x as f32 / tw as f32,
            sprite.src.y as f32 / th as f32,
        ];
        let mut uv_size = [
            sprite.src.w as f32 / tw as f32,
            sprite.src.h as f32 / th as f32,
        ];
        if sprite.flip_x {
            uv_min[0] += uv_size[0];
            uv_size[0] = -uv_size[0];
        }
        let mode = match sprite.kind {
            SpriteKind::Plain => mode::PLAIN,
            SpriteKind::Character => mode::BODY,
            SpriteKind::Blob => mode::BLOB,
            SpriteKind::Outline(Outline::Circle) => mode::CIRCLE,
            SpriteKind::Outline(Outline::Rect) => mode::RECT,
        };
        let index = frame.instances.len() as u32;
        seq += 1;
        frame.instances.push(Instance {
            pos: lifted.to_array(),
            size: size.to_array(),
            uv_min,
            uv_size,
            repeat: sprite.repeat.to_array(),
            color: sprite.color,
            order: seq as f32,
            mode,
        });
        if sprite.layer >= layer::UI {
            append(&mut frame.overlay, sprite.texture, index);
        } else {
            append(&mut frame.main, sprite.texture, index);
        }
        let drawn = Drawn {
            seq,
            min: lifted,
            max: lifted + size,
            part: Part::Sprite(index),
            texture: sprite.texture,
        };
        match sprite.kind {
            SpriteKind::Plain => can_occlude.push((drawn, sprite.layer)),
            // Characters do not occlude each other: a ghost painted over a character in front
            // would hide the one the player can already see.
            SpriteKind::Character => bodies.push(drawn),
            SpriteKind::Blob | SpriteKind::Outline(_) => {}
        }
    }
    draw_meshes_before(&mut frame, &mut seq, &mut bodies, None);

    // Occluders: drawn after some character body they overlap, and not interface or debug.
    let occluders: Vec<Drawn> = can_occlude
        .iter()
        .filter(|(o, layer)| {
            *layer < layer::UI && bodies.iter().any(|b| b.seq < o.seq && b.overlaps(o))
        })
        .map(|&(o, _)| o)
        .collect();
    // Only bodies something covers need a silhouette; the mask decides exactly where.
    let covered: Vec<Drawn> = bodies
        .iter()
        .copied()
        .filter(|b| occluders.iter().any(|o| b.seq < o.seq && b.overlaps(o)))
        .collect();
    // The mask is 16-bit float, exact for whole numbers only up to 2048, so the copies carry a
    // compact rank among just these instead of their draw order in the whole frame.
    let mut ranked: Vec<u32> = occluders.iter().chain(&covered).map(|d| d.seq).collect();
    ranked.sort_unstable();
    ranked.dedup();
    let rank = |seq: u32| ranked.binary_search(&seq).map_or(0.0, |r| r as f32 + 1.0);
    for o in &occluders {
        let Part::Sprite(i) = o.part else { continue };
        let mut instance = frame.instances[i as usize];
        instance.order = rank(o.seq);
        let index = frame.instances.len() as u32;
        frame.instances.push(instance);
        append(&mut frame.occluders, o.texture, index);
    }
    for b in &covered {
        match b.part {
            Part::Sprite(i) => {
                let mut instance = frame.instances[i as usize];
                instance.order = rank(b.seq);
                let index = frame.instances.len() as u32;
                frame.instances.push(instance);
                append(&mut frame.silhouettes, b.texture, index);
            }
            Part::Mesh(start, end) => {
                let first = frame.vertices.len() as u32;
                for v in start..end {
                    let mut vertex = frame.vertices[v as usize];
                    vertex.order = rank(b.seq);
                    frame.vertices.push(vertex);
                }
                let last = frame.vertices.len() as u32;
                append_triangles(&mut frame.silhouettes, b.texture, first, last);
            }
        }
    }
    frame
}

fn append(batches: &mut Vec<Batch>, texture: TextureId, index: u32) {
    match batches.last_mut() {
        Some(batch) if !batch.triangles && batch.texture == texture && batch.end == index => {
            batch.end = index + 1
        }
        _ => batches.push(Batch {
            texture,
            start: index,
            end: index + 1,
            triangles: false,
        }),
    }
}

fn append_triangles(batches: &mut Vec<Batch>, texture: TextureId, start: u32, end: u32) {
    match batches.last_mut() {
        Some(batch) if batch.triangles && batch.texture == texture && batch.end == start => {
            batch.end = end
        }
        _ => batches.push(Batch {
            texture,
            start,
            end,
            triangles: true,
        }),
    }
}

/// Where the internal image lands in the window: integer scale when it fits, letterboxed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

pub fn fit_viewport(internal: (u32, u32), window: (u32, u32)) -> Viewport {
    let (iw, ih) = (internal.0 as f32, internal.1 as f32);
    let (ww, wh) = (window.0 as f32, window.1 as f32);
    let exact = (ww / iw).min(wh / ih);
    // Whole-number scale keeps pixels square; only a window smaller than the art shrinks it.
    let scale = if exact >= 1.0 { exact.floor() } else { exact };
    let (width, height) = (iw * scale, ih * scale);
    Viewport {
        x: ((ww - width) / 2.0).floor(),
        y: ((wh - height) / 2.0).floor(),
        width,
        height,
        scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far from the origin the world may go, in one number (docs/PLAN.md §24.0).
    ///
    /// Fine ordering is a world `y` held in an `f32`, and the game settles ties by nudging it a
    /// hundredth of a pixel — a blob shadow drawn behind its owner's feet. Far enough out, an
    /// `f32` can no longer hold a hundredth, the two sprites tie, and the shadow draws on the
    /// feet. That happens just past 262 144 px: **16.38 km** at 16 px to the metre, which is why
    /// §24.0 says a world of ten to sixteen kilometres needs no new coordinates and a larger one
    /// does. When the nudge is replaced by a whole-number sub-layer, this test is what says the
    /// limit has moved.
    #[test]
    fn the_sort_nudge_says_how_far_the_world_may_reach() {
        let behind = |y: f32| (y - 0.01) < y;
        // 4 km, 10 km, 16 km: a shadow still sorts behind the feet it belongs to.
        for y in [64_000.0, 160_000.0, 256_000.0, 262_144.0] {
            assert!(
                behind(y),
                "the nudge is lost at {y} px, sooner than expected"
            );
        }
        // Past that it is lost, and only the stable sort keeps the order by luck of submission.
        for y in [300_000.0, 1_600_000.0] {
            assert!(
                !behind(y),
                "the nudge outlived {y} px; §24.0 may be too careful"
            );
        }
    }

    fn sprite(texture: u32, y: f32, layer: i32) -> Sprite {
        let mut s = Sprite::new(
            TextureId(texture),
            Rect::new(0, 0, 16, 16),
            Vec2::new(0.0, y),
            Vec2::ZERO,
        );
        s.layer = layer;
        s
    }

    #[test]
    fn sorts_by_layer_then_y_and_batches_runs() {
        let mut sprites = vec![
            sprite(1, 50.0, 0),
            sprite(0, 10.0, 0),
            sprite(0, 0.0, -100),
            sprite(0, 20.0, 0),
        ];
        let frame = build_batches(&mut sprites, &[], |_| (16, 16));
        let ys: Vec<f32> = frame.instances.iter().map(|i| i.pos[1]).collect();
        assert_eq!(ys, vec![0.0, 10.0, 20.0, 50.0]);
        assert_eq!(
            frame.main,
            vec![
                Batch {
                    texture: TextureId(0),
                    start: 0,
                    end: 3,
                    triangles: false,
                },
                Batch {
                    texture: TextureId(1),
                    start: 3,
                    end: 4,
                    triangles: false,
                },
            ]
        );
        assert!(frame.occluders.is_empty() && frame.silhouettes.is_empty());
    }

    #[test]
    fn snaps_to_pixels_and_flips() {
        let mut s = Sprite::new(
            TextureId(0),
            Rect::new(16, 0, 16, 32),
            Vec2::new(10.6, 20.4),
            Vec2::new(8.0, 32.0),
        );
        s.flip_x = true;
        let frame = build_batches(&mut [s], &[], |_| (64, 32));
        let i = frame.instances[0];
        assert_eq!(i.pos, [3.0, -12.0]);
        assert_eq!(i.uv_min, [0.5, 0.0]);
        assert_eq!(i.uv_size, [-0.25, 1.0]);
    }

    #[test]
    fn repeat_scales_draw_size() {
        let mut s = Sprite::new(
            TextureId(0),
            Rect::new(0, 0, 80, 80),
            Vec2::ZERO,
            Vec2::ZERO,
        );
        s.repeat = Vec2::new(3.0, 2.0);
        let frame = build_batches(&mut [s], &[], |_| (400, 400));
        assert_eq!(frame.instances[0].size, [240.0, 160.0]);
    }

    #[test]
    fn character_draws_only_its_body_lifted() {
        let mut hero = Sprite::new(
            TextureId(0),
            Rect::new(0, 0, 16, 16),
            Vec2::new(50.0, 50.0),
            Vec2::new(8.0, 16.0),
        );
        hero.kind = SpriteKind::Character;
        hero.lift = 12.0;
        let frame = build_batches(&mut [hero], &[], |_| (16, 16));
        assert_eq!(frame.instances.len(), 1);
        assert_eq!(
            (frame.instances[0].mode, frame.instances[0].pos),
            (mode::BODY, [42.0, 22.0])
        );
    }

    #[test]
    fn only_later_overlapping_sprites_occlude_a_character() {
        let mut hero = Sprite::new(
            TextureId(0),
            Rect::new(0, 0, 16, 16),
            Vec2::new(50.0, 50.0),
            Vec2::new(8.0, 16.0),
        );
        hero.kind = SpriteKind::Character;
        let behind = sprite(1, 40.0, 0); // drawn before the hero
        let mut tree = Sprite::new(
            TextureId(2),
            Rect::new(0, 0, 32, 64),
            Vec2::new(50.0, 60.0),
            Vec2::new(16.0, 64.0),
        );
        tree.sort_y = 60.0; // in front, overlapping
        let far = sprite(3, 90.0, 0); // in front, not overlapping
        let mut debug = sprite(1, 0.0, layer::DEBUG);
        debug.position = Vec2::new(45.0, 40.0);
        let frame = build_batches(&mut [hero, behind, tree, far, debug], &[], |_| (64, 64));
        let occluder_textures: Vec<_> = frame.occluders.iter().map(|b| b.texture).collect();
        assert_eq!(occluder_textures, vec![TextureId(2)]);
        assert_eq!(frame.silhouettes.len(), 1);
        let body = frame.instances[frame.silhouettes[0].start as usize];
        assert_eq!(body.mode, mode::BODY);
        let tree_inst = frame.instances[frame.occluders[0].start as usize];
        assert!(tree_inst.order > body.order, "the mask compares draw order");
    }

    #[test]
    fn characters_do_not_occlude_each_other() {
        let character = |y: f32| {
            let mut s = Sprite::new(
                TextureId(0),
                Rect::new(0, 0, 16, 16),
                Vec2::new(50.0, y),
                Vec2::new(8.0, 16.0),
            );
            s.kind = SpriteKind::Character;
            s
        };
        let frame = build_batches(&mut [character(50.0), character(55.0)], &[], |_| (16, 16));
        assert!(frame.occluders.is_empty() && frame.silhouettes.is_empty());
    }

    #[test]
    fn viewport_uses_integer_scale_and_letterbox() {
        assert_eq!(
            fit_viewport((640, 360), (1280, 720)),
            Viewport {
                x: 0.0,
                y: 0.0,
                width: 1280.0,
                height: 720.0,
                scale: 2.0
            }
        );
        let v = fit_viewport((640, 360), (1500, 900));
        assert_eq!(
            (v.scale, v.width, v.height, v.x, v.y),
            (2.0, 1280.0, 720.0, 110.0, 90.0)
        );
        let small = fit_viewport((640, 360), (320, 180));
        assert_eq!(small.scale, 0.5);
    }

    #[test]
    fn meshes_draw_between_the_sprites_they_sort_between() {
        let texture = TextureId(0);
        let sprite = |y: f32| {
            Sprite::new(
                texture,
                Rect::new(0, 0, 4, 4),
                Vec2::new(0.0, y),
                Vec2::ZERO,
            )
        };
        let triangle = |y: f32| Mesh {
            texture: TextureId(1),
            vertices: vec![
                MeshVertex {
                    position: Vec2::ZERO,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                };
                3
            ],
            layer: layer::WORLD,
            sort_y: y,
            body: false,
        };
        let frame = build_batches(
            &mut [sprite(30.0), sprite(10.0)],
            &[triangle(20.0), triangle(40.0), triangle(20.0)],
            |_| (4, 4),
        );
        let kinds: Vec<(bool, u32, u32)> = frame
            .main
            .iter()
            .map(|b| (b.triangles, b.start, b.end))
            .collect();
        // The sprite at 10, both meshes at 20 (one batch), the sprite at 30, the mesh at 40.
        assert_eq!(
            kinds,
            vec![(false, 0, 1), (true, 0, 6), (false, 1, 2), (true, 6, 9)]
        );
        assert_eq!(frame.vertices.len(), 9);
        assert!(frame.occluders.is_empty(), "meshes never hide anyone");
    }

    #[test]
    fn a_character_mesh_behind_a_sprite_gets_a_silhouette() {
        let body = Mesh {
            texture: TextureId(2),
            vertices: [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]
                .map(|(x, y)| MeshVertex {
                    position: Vec2::new(x, y),
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                })
                .to_vec(),
            layer: layer::WORLD,
            sort_y: 10.0,
            body: true,
        };
        // A tree drawn after it (sorted lower down) over the same pixels.
        let tree = Sprite::new(
            TextureId(0),
            Rect::new(0, 0, 16, 16),
            Vec2::new(0.0, 20.0),
            Vec2::new(0.0, 16.0),
        );
        let frame = build_batches(&mut [tree], std::slice::from_ref(&body), |_| (16, 16));
        assert_eq!(frame.occluders.len(), 1, "the tree hides it");
        assert!(matches!(
            frame.silhouettes[..],
            [Batch {
                triangles: true,
                ..
            }]
        ));
        // The silhouette copies carry their rank, after the tree's.
        let copy = frame.silhouettes[0];
        let order = frame.vertices[copy.start as usize].order;
        let tree_rank = frame.instances[frame.occluders[0].start as usize].order;
        assert!(order < tree_rank, "{order} < {tree_rank}");
        // Not a body: no silhouette.
        let plain = Mesh {
            body: false,
            ..body
        };
        let tree = Sprite::new(
            TextureId(0),
            Rect::new(0, 0, 16, 16),
            Vec2::new(0.0, 20.0),
            Vec2::new(0.0, 16.0),
        );
        let frame = build_batches(&mut [tree], &[plain], |_| (16, 16));
        assert!(frame.silhouettes.is_empty() && frame.occluders.is_empty());
    }

    #[test]
    fn the_interface_is_drawn_apart_after_everything() {
        let mut bubble = Sprite::new(TextureId(0), Rect::new(0, 0, 4, 4), Vec2::ZERO, Vec2::ZERO);
        bubble.layer = layer::UI;
        let tree = Sprite::new(TextureId(0), Rect::new(0, 0, 4, 4), Vec2::ZERO, Vec2::ZERO);
        let frame = build_batches(&mut [bubble, tree], &[], |_| (4, 4));
        assert_eq!(frame.main.len(), 1, "the tree");
        assert_eq!(frame.overlay.len(), 1, "the bubble");
    }
}
