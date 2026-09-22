//! CPU side of sprite drawing: what a sprite is, ordering, batching and pixel-perfect layout.

use dark_sprite::Rect;
use glam::Vec2;

/// Handle to a texture created by [`crate::Renderer::create_texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub(crate) u32);

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

/// A run of consecutive instances sharing one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    pub texture: TextureId,
    pub start: u32,
    pub end: u32,
}

/// Everything the GPU passes need for one frame, all in one instance buffer.
#[derive(Debug, Default)]
pub(crate) struct FrameBatches {
    pub instances: Vec<Instance>,
    /// Main pass, back to front.
    pub main: Vec<Batch>,
    /// Sprites drawn after, and overlapping, a character: they write the occlusion mask.
    pub occluders: Vec<Batch>,
    /// Character bodies, drawn again as silhouettes where the mask says they are covered.
    pub silhouettes: Vec<Batch>,
}

/// Sorts sprites back-to-front and turns them into instances plus texture batches.
/// Top-left corners are snapped to whole pixels so art never shimmers between texels.
pub(crate) fn build_batches(
    sprites: &mut [Sprite],
    texture_size: impl Fn(TextureId) -> (u32, u32),
) -> FrameBatches {
    // Stable: equal keys keep submission order. total_cmp, because a NaN position must not
    // break the total order sort_by requires.
    sprites.sort_by(|a, b| a.layer.cmp(&b.layer).then(a.sort_y.total_cmp(&b.sort_y)));
    let mut frame = FrameBatches::default();
    // (instance index, layer) of everything that can hide a character, and the bodies.
    let mut can_occlude = Vec::new();
    let mut bodies = Vec::new();
    for sprite in sprites.iter() {
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
        let mut push = |pos: Vec2, mode: u32| {
            let index = frame.instances.len() as u32;
            frame.instances.push(Instance {
                pos: pos.to_array(),
                size: size.to_array(),
                uv_min,
                uv_size,
                repeat: sprite.repeat.to_array(),
                color: sprite.color,
                order: index as f32 + 1.0,
                mode,
            });
            append(&mut frame.main, sprite.texture, index);
            index
        };
        match sprite.kind {
            SpriteKind::Plain => {
                let i = push(lifted, mode::PLAIN);
                can_occlude.push((i, sprite.layer));
            }
            SpriteKind::Character => {
                // Characters do not occlude each other: a ghost painted over a character in
                // front would hide the one the player can already see.
                let i = push(lifted, mode::BODY);
                bodies.push((i, sprite.texture));
            }
            SpriteKind::Blob => {
                push(lifted, mode::BLOB);
            }
            SpriteKind::Outline(Outline::Circle) => {
                push(lifted, mode::CIRCLE);
            }
            SpriteKind::Outline(Outline::Rect) => {
                push(lifted, mode::RECT);
            }
        }
    }

    // Occluders: drawn after some character body they overlap, and not interface or debug.
    let rect = |i: u32| {
        let inst = &frame.instances[i as usize];
        (
            Vec2::from(inst.pos),
            Vec2::from(inst.pos) + Vec2::from(inst.size),
        )
    };
    let overlaps = |a: u32, b: u32| {
        let ((a0, a1), (b0, b1)) = (rect(a), rect(b));
        a0.x < b1.x && b0.x < a1.x && a0.y < b1.y && b0.y < a1.y
    };
    let occluders: Vec<u32> = can_occlude
        .iter()
        .filter(|&&(i, layer)| {
            layer < layer::UI && bodies.iter().any(|&(b, _)| b < i && overlaps(b, i))
        })
        .map(|&(i, _)| i)
        .collect();
    // Only bodies something covers need a silhouette; the mask decides exactly where.
    let covered: Vec<(u32, TextureId)> = bodies
        .iter()
        .copied()
        .filter(|&(b, _)| occluders.iter().any(|&o| b < o && overlaps(b, o)))
        .collect();
    // The mask is 16-bit float, exact for whole numbers only up to 2048, so the copies carry a
    // compact rank among just these sprites instead of their draw order in the whole frame.
    let mut ranked: Vec<u32> = occluders
        .iter()
        .copied()
        .chain(covered.iter().map(|&(b, _)| b))
        .collect();
    ranked.sort_unstable();
    ranked.dedup();
    let rank = |i: u32| ranked.binary_search(&i).map_or(0.0, |r| r as f32 + 1.0);
    for i in occluders {
        let mut instance = frame.instances[i as usize];
        instance.order = rank(i);
        let index = frame.instances.len() as u32;
        frame.instances.push(instance);
        append(&mut frame.occluders, texture_of(&frame.main, i), index);
    }
    for (b, texture) in covered {
        let mut instance = frame.instances[b as usize];
        instance.order = rank(b);
        let index = frame.instances.len() as u32;
        frame.instances.push(instance);
        append(&mut frame.silhouettes, texture, index);
    }
    frame
}

fn append(batches: &mut Vec<Batch>, texture: TextureId, index: u32) {
    match batches.last_mut() {
        Some(batch) if batch.texture == texture && batch.end == index => batch.end = index + 1,
        _ => batches.push(Batch {
            texture,
            start: index,
            end: index + 1,
        }),
    }
}

fn texture_of(batches: &[Batch], index: u32) -> TextureId {
    batches
        .iter()
        .find(|b| (b.start..b.end).contains(&index))
        .map(|b| b.texture)
        .expect("every main instance is in a batch")
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
        let frame = build_batches(&mut sprites, |_| (16, 16));
        let ys: Vec<f32> = frame.instances.iter().map(|i| i.pos[1]).collect();
        assert_eq!(ys, vec![0.0, 10.0, 20.0, 50.0]);
        assert_eq!(
            frame.main,
            vec![
                Batch {
                    texture: TextureId(0),
                    start: 0,
                    end: 3
                },
                Batch {
                    texture: TextureId(1),
                    start: 3,
                    end: 4
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
        let frame = build_batches(&mut [s], |_| (64, 32));
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
        let frame = build_batches(&mut [s], |_| (400, 400));
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
        let frame = build_batches(&mut [hero], |_| (16, 16));
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
        let frame = build_batches(&mut [hero, behind, tree, far, debug], |_| (64, 64));
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
        let frame = build_batches(&mut [character(50.0), character(55.0)], |_| (16, 16));
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
}
