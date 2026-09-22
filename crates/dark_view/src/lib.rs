//! What a map looks like (docs/PLAN.md §9, §10): its ground, raised terrain, cliff faces and
//! props as sprites, and a debug overlay of collision, levels and exits. Shared by the game and
//! the editor, so what the editor shows is what the game draws.

use std::collections::HashMap;

use dark_assets::LoadedSheet;
use dark_physics::{Cell, Shape};
use dark_render::{Outline, Sprite, TextureId, layer};
use dark_sprite::Rect;
use dark_world::Map;
use glam::Vec2;

/// Static sprites and debug overlay of one map.
pub struct MapView {
    pub size: Vec2,
    /// Ground, terrain and props, sorted by the renderer each frame.
    pub statics: Vec<Sprite>,
    /// Collision, levels and exits, for a debug overlay.
    pub overlay: Vec<Sprite>,
}

impl MapView {
    pub fn build(
        map: &Map,
        sheets: &HashMap<String, LoadedSheet>,
        textures: &HashMap<String, TextureId>,
        white: TextureId,
        jump_apex: f32,
    ) -> Self {
        let def = &map.def;
        let terrain = &map.collision.terrain;
        let size = Vec2::from(def.size);
        let mut statics = Vec::new();
        let mut overlay = Vec::new();

        let ground_sheet = &sheets[&def.ground.sheet].sheet;
        let ground_tex = textures[&def.ground.sheet];
        let frame_rect = |index: u32| ground_sheet.frames.get(index as usize).map(|f| f.rect);
        if let Some(rect) = frame_rect(def.ground.frame) {
            let mut sprite = Sprite::new(ground_tex, rect, Vec2::ZERO, Vec2::ZERO);
            sprite.repeat = size / Vec2::new(rect.w as f32, rect.h as f32);
            sprite.layer = layer::GROUND;
            statics.push(sprite);
        }

        // Raised tiles. A top is a floor: drawn in the terrain layer (per level) below every
        // character and prop, so whatever stands on it is always visible. Two pieces are drawn in
        // the world layer to hide things correctly:
        //  - a north cap, the strip of top that can overlap someone standing just behind (north
        //    of) the plateau, sorted by the plateau's north edge;
        //  - a cliff face down to the tile in front, sorted by its south edge, so anything
        //    standing in front draws after it.
        let top = frame_rect(def.terrain.top_frame.unwrap_or(def.ground.frame));
        let face = frame_rect(def.terrain.face_frame.unwrap_or(def.ground.frame));
        let (tile, lh) = (terrain.tile(), terrain.level_height());
        let rim = [0.0, 0.0, 0.0, 0.45];
        for row in 0..i64::from(terrain.rows()) {
            for col in 0..i64::from(terrain.cols()) {
                let cell = terrain.cell(col, row).unwrap_or_default();
                let origin = Vec2::new(col as f32, row as f32) * tile;
                if cell == Cell::Wall {
                    overlay.push(tint(
                        white,
                        origin,
                        Vec2::splat(tile),
                        [0.9, 0.1, 0.1, 0.35],
                        0.0,
                    ));
                    continue;
                }
                let level = cell.level().unwrap_or(0);
                if level == 0 {
                    continue;
                }
                let height = f32::from(level) * lh;
                let lifted = origin - Vec2::new(0.0, height);
                // Clamped so absurdly high levels never reach the world layer.
                let floor_layer = layer::TERRAIN + i32::from(level.min(40));
                overlay.push(tint(
                    white,
                    lifted,
                    Vec2::splat(tile),
                    [0.2, 0.4, 1.0, 0.12 * f32::from(level)],
                    0.0,
                ));
                let neighbour =
                    |dc: i64, dr: i64| terrain.cell(col + dc, row + dr).and_then(Cell::level);
                let lower = |dc: i64, dr: i64| neighbour(dc, dr).is_some_and(|n| n < level);
                let on_floor = |mut s: Sprite| {
                    s.layer = floor_layer;
                    s.sort_y = origin.y;
                    s
                };
                if let Some(rect) = top {
                    statics.push(on_floor(Sprite::new(
                        ground_tex,
                        sub_rect(rect, origin, Vec2::splat(tile)),
                        lifted,
                        Vec2::ZERO,
                    )));
                }
                // Darkened rims where the neighbour is lower, so plateaus read at a glance.
                if lower(-1, 0) {
                    statics.push(on_floor(tint(
                        white,
                        lifted,
                        Vec2::new(1.0, tile),
                        rim,
                        0.0,
                    )));
                }
                if lower(1, 0) {
                    statics.push(on_floor(tint(
                        white,
                        lifted + Vec2::new(tile - 1.0, 0.0),
                        Vec2::new(1.0, tile),
                        rim,
                        0.0,
                    )));
                }
                if let Some(north) = neighbour(0, -1)
                    && north < level
                {
                    // Anyone standing north of the edge overlaps at most the height difference
                    // of this top (clipped to one tile).
                    let cap = (f32::from(level - north) * lh).min(tile);
                    if let Some(rect) = top {
                        let mut s = Sprite::new(
                            ground_tex,
                            sub_rect(rect, origin, Vec2::new(tile, cap)),
                            lifted,
                            Vec2::ZERO,
                        );
                        s.sort_y = origin.y;
                        statics.push(s);
                    }
                    statics.push(tint(white, lifted, Vec2::new(tile, 1.0), rim, origin.y));
                }
                let front = neighbour(0, 1);
                if let (Some(rect), Some(front)) = (face, front)
                    && front < level
                {
                    let south = origin.y + tile;
                    for step in front..level {
                        let y = south - f32::from(step + 1) * lh;
                        let src = sub_rect(rect, Vec2::new(origin.x, y), Vec2::new(tile, lh));
                        let mut s =
                            Sprite::new(ground_tex, src, Vec2::new(origin.x, y), Vec2::ZERO);
                        s.color = [0.75, 0.75, 0.75, 1.0];
                        s.sort_y = south;
                        statics.push(s);
                    }
                }
            }
        }

        for prop in &map.props {
            let Some(frame) = sheets[&prop.sheet].sheet.frames.get(prop.frame as usize) else {
                tracing::warn!("{}: no frame {}", prop.sheet, prop.frame);
                continue;
            };
            let at = Vec2::from(prop.position);
            let ground = map.collision.ground_under(at, 0.5);
            let ground = if ground.is_finite() { ground } else { 0.0 };
            let mut sprite = Sprite::new(
                textures[&prop.sheet],
                frame.rect,
                at - Vec2::new(0.0, ground),
                frame.pivot,
            );
            sprite.sort_y = at.y;
            statics.push(sprite);
        }
        for c in &map.collision.colliders {
            let (half, outline) = match c.shape {
                Shape::Circle { radius } => (Vec2::splat(radius), Outline::Circle),
                Shape::Rect { half } => (half, Outline::Rect),
            };
            // Orange: blocks at any height. Cyan: low enough to jump over or stand on.
            let color = if c.height <= jump_apex {
                [0.3, 0.9, 1.0, 0.9]
            } else {
                [1.0, 0.6, 0.0, 0.9]
            };
            overlay.push(Sprite::outline(
                white,
                outline,
                c.center - half - Vec2::new(0.0, c.base),
                half * 2.0,
                color,
            ));
        }
        for exit in &map.exits {
            overlay.push(tint(
                white,
                exit.min,
                exit.max - exit.min,
                [1.0, 1.0, 0.2, 0.35],
                0.0,
            ));
        }
        tracing::debug!("{}: {} static sprites", map.name, statics.len());
        Self {
            size,
            statics,
            overlay,
        }
    }
}

/// Southmost ground-plane y of a prop's footprint.
pub fn south_edge(prop: &dark_physics::Collider) -> f32 {
    match prop.shape {
        Shape::Circle { radius } => prop.center.y + radius,
        Shape::Rect { half } => prop.center.y + half.y,
    }
}

/// A flat coloured rectangle, sorted at `sort_y`.
pub fn tint(white: TextureId, top_left: Vec2, size: Vec2, color: [f32; 4], sort_y: f32) -> Sprite {
    let mut s = Sprite::fill(white, top_left, size, color);
    s.sort_y = sort_y;
    s
}

/// The `size` part of a seamless texture that lines up with world position `at`. Clipped to
/// the texture: a level taller than the face texture would show a gap (none in current data).
pub fn sub_rect(texture: Rect, at: Vec2, size: Vec2) -> Rect {
    let (w, h) = (
        (size.x as u32).min(texture.w),
        (size.y as u32).min(texture.h),
    );
    let wrap = |v: f32, span: u32, len: u32| (v.max(0.0) as u32 % span).min(span - len);
    Rect::new(
        texture.x + wrap(at.x, texture.w, w),
        texture.y + wrap(at.y, texture.h, h),
        w,
        h,
    )
}
