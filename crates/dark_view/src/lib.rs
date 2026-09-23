//! What a map looks like (docs/PLAN.md §9, §10): its ground, raised terrain, cliff faces and
//! props as sprites, and a debug overlay of collision, levels and exits. Shared by the game and
//! the editor, so what the editor shows is what the game draws.
//!
//! A map is held in **pieces** (§24.2), not as one list: a frame copies and sorts the pieces the
//! camera can see, so what a frame costs is the size of the screen and not the size of the map.
//! A piece knows what it covers once drawn, which is more than the ground it stands on — a tall
//! tree hangs above its feet, and would otherwise vanish a moment before it left the screen.

use std::collections::HashMap;

use dark_assets::LoadedSheet;
use dark_physics::{Cell, Shape};
use dark_render::{Outline, Sprite, TextureId, layer};
use dark_sprite::Rect;
use dark_world::Map;
use glam::Vec2;

/// How wide and tall a piece of a map is, in pixels. Small enough that little outside the screen
/// is copied with the pieces that meet it; large enough that a map is not mostly bookkeeping.
/// A screenful of the first game is 640×360, so a view meets three or four pieces a side.
const PIECE: f32 = 256.0;

/// One piece of a map: the sprites standing in it, each with the place it had when the map was
/// one list, and what they cover once drawn.
struct Piece {
    /// What the sprites held here cover — more than the piece itself, since a tree hangs above
    /// its own feet. Empty until something is put here.
    min: Vec2,
    max: Vec2,
    statics: Vec<(u32, Sprite)>,
    overlay: Vec<(u32, Sprite)>,
    /// Sprites too wide or tall for one piece, held once by the map: where each sits in
    /// [`MapView::wide`].
    wide: Vec<u32>,
}

impl Default for Piece {
    fn default() -> Self {
        Self {
            // Nothing is here yet, so anything put here decides both corners.
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
            statics: Vec::new(),
            overlay: Vec::new(),
            wide: Vec::new(),
        }
    }
}

impl Piece {
    /// Remembers that something drawn in `min..max` shows here, whether it is held here or
    /// merely crosses.
    fn covering(&mut self, min: Vec2, max: Vec2) {
        self.min = self.min.min(min);
        self.max = self.max.max(max);
    }

    fn take(&mut self, nth: u32, sprite: Sprite, overlay: bool) {
        let (min, max) = sprite.covers();
        self.covering(min, max);
        if overlay {
            self.overlay.push((nth, sprite));
        } else {
            self.statics.push((nth, sprite));
        }
    }

    /// Whether anything held here is drawn inside `min..max`.
    fn seen_in(&self, min: Vec2, max: Vec2) -> bool {
        self.min.x < max.x && min.x < self.max.x && self.min.y < max.y && min.y < self.max.y
    }
}

/// Whether a sprite is drawn inside `min..max`.
fn drawn_in(sprite: &Sprite, min: Vec2, max: Vec2) -> bool {
    let (a, b) = sprite.covers();
    a.x < max.x && min.x < b.x && a.y < max.y && min.y < b.y
}

/// Static sprites and debug overlay of one map, in pieces a fraction of a screen across.
pub struct MapView {
    pub size: Vec2,
    pieces: Vec<Piece>,
    /// Sprites larger than a piece — the ground, a great tree — held once here and pointed at by
    /// every piece they cross, with the place each had when the map was one list.
    wide: Vec<(u32, Sprite, bool)>,
    /// Pieces across, and down; a sprite's piece is its position divided by [`PIECE`].
    across: usize,
    down: usize,
    /// How far sprites hang beyond the piece holding them: `before` up and to the left, `after`
    /// down and to the right. A tree stands in one piece and is drawn into the one above, so a
    /// view asks pieces beyond itself on both sides — widening the near edge by `after` and the
    /// far edge by `before` — or a tall thing vanishes while the top of it still shows.
    before: Vec2,
    after: Vec2,
}

impl MapView {
    /// Copies the static sprites drawn inside the world rectangle `min..max` — the camera's own
    /// rectangle — onto the end of `out`, in the order the whole map would have given them.
    pub fn seen(&self, min: Vec2, max: Vec2, out: &mut Vec<Sprite>) {
        self.gather(min, max, false, out, &|s| s);
    }

    /// The same for the debug overlay, each sprite passed through `each` on the way (the editor
    /// dims it).
    pub fn seen_overlay(
        &self,
        min: Vec2,
        max: Vec2,
        out: &mut Vec<Sprite>,
        each: impl Fn(Sprite) -> Sprite,
    ) {
        self.gather(min, max, true, out, &each);
    }

    /// Everything of one sort drawn in the view, put back into the order the map was built in.
    ///
    /// The order matters: the renderer's sort is stable, so sprites that tie keep the order they
    /// are given in, and the ground, the land and what stands on it tie by design. Each sprite
    /// carries the place it had when a map was one list, and they are handed over by it, so what
    /// a frame draws cannot depend on which piece a thing happened to fall in.
    fn gather(
        &self,
        min: Vec2,
        max: Vec2,
        overlay: bool,
        out: &mut Vec<Sprite>,
        each: &impl Fn(Sprite) -> Sprite,
    ) {
        // A pixel of grace at the edge, once and for both the pieces and the sprites: the
        // renderer rounds a sprite's corner to whole pixels and this does not, so something
        // whose rounded corner would just reach the screen is kept rather than dropped.
        let (min, max) = (min - Vec2::ONE, max + Vec2::ONE);
        let mut gathered: Vec<(u32, Sprite)> = Vec::new();
        let mut wide: Vec<u32> = Vec::new();
        for piece in self.around(min, max) {
            let held = if overlay {
                &piece.overlay
            } else {
                &piece.statics
            };
            gathered.extend(held.iter().filter(|(_, s)| drawn_in(s, min, max)));
            wide.extend_from_slice(&piece.wide);
        }
        // A wide sprite crosses several pieces, and the view may see more than one of them.
        wide.sort_unstable();
        wide.dedup();
        gathered.extend(wide.into_iter().filter_map(|nth| {
            let (place, sprite, its_overlay) = self.wide[nth as usize];
            (its_overlay == overlay && drawn_in(&sprite, min, max)).then_some((place, sprite))
        }));
        gathered.sort_unstable_by_key(|(place, _)| *place);
        out.extend(gathered.into_iter().map(|(_, s)| each(s)));
    }

    /// The pieces that show inside `min..max`. Only the pieces whose own square is near the view
    /// are asked — never all of them — so this costs the size of the view and not of the map.
    fn around(&self, min: Vec2, max: Vec2) -> impl Iterator<Item = &Piece> {
        let first = ((min - self.after) / PIECE).floor();
        let last = ((max + self.before) / PIECE).floor();
        let cols = span(first.x, last.x, self.across);
        let rows = span(first.y, last.y, self.down);
        rows.flat_map(move |row| cols.clone().map(move |col| row * self.across + col))
            .filter_map(|nth| self.pieces.get(nth))
            .filter(move |piece| piece.seen_in(min, max))
    }

    /// How many static sprites the whole map holds. For logs and tests; a frame never asks.
    pub fn sprites(&self) -> usize {
        let wide = self.wide.iter().filter(|(_, _, overlay)| !overlay).count();
        wide + self.pieces.iter().map(|p| p.statics.len()).sum::<usize>()
    }

    /// The piece a sprite stands in. A sprite off the map belongs to the nearest piece, so
    /// nothing authored outside the edges is lost.
    fn piece_of(&self, at: Vec2) -> usize {
        let col = ((at.x.max(0.0) / PIECE) as usize).min(self.across - 1);
        let row = ((at.y.max(0.0) / PIECE) as usize).min(self.down - 1);
        row * self.across + col
    }

    /// Every piece a rectangle touches, clamped to the map as [`MapView::piece_of`] is.
    fn crossed_by(&self, min: Vec2, max: Vec2) -> Vec<usize> {
        let (first, last) = (self.piece_of(min), self.piece_of(max));
        let (cols, rows) = (
            (first % self.across)..=(last % self.across),
            (first / self.across)..=(last / self.across),
        );
        rows.flat_map(|row| cols.clone().map(move |col| row * self.across + col))
            .collect()
    }
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
        let mut props = Vec::new();
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
            props.push(sprite);
        }
        for c in map.collision.colliders() {
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
        // In the order a map used to be one list: the ground and the land, then what stands on
        // it, then the overlay.
        let view = Self::place(
            size,
            statics
                .into_iter()
                .chain(props)
                .map(|s| (s, false))
                .chain(overlay.into_iter().map(|s| (s, true))),
        );
        tracing::debug!(
            "{}: {} static sprites in {} pieces",
            map.name,
            view.sprites(),
            view.pieces.len()
        );
        view
    }

    /// Lays sprites out in pieces by where each one stands, remembering the place each had in
    /// the list they came in.
    fn place(size: Vec2, sprites: impl Iterator<Item = (Sprite, bool)>) -> Self {
        let across = ((size.x / PIECE).ceil() as usize).max(1);
        let down = ((size.y / PIECE).ceil() as usize).max(1);
        let mut view = Self {
            size,
            pieces: (0..across * down).map(|_| Piece::default()).collect(),
            wide: Vec::new(),
            across,
            down,
            before: Vec2::ZERO,
            after: Vec2::ZERO,
        };
        for (place, (sprite, overlay)) in sprites.enumerate() {
            let place = place as u32;
            let (min, max) = sprite.covers();
            // Larger than a piece — the ground, a great tree — so no one piece can hold it: the
            // map holds it and every piece it crosses points at it. Holding it where it stands
            // would make every view ask far beyond itself to be sure of finding it.
            if (max - min).max_element() > PIECE {
                let nth = view.wide.len() as u32;
                view.wide.push((place, sprite, overlay));
                for piece in view.crossed_by(min, max) {
                    view.pieces[piece].wide.push(nth);
                    view.pieces[piece].covering(min, max);
                }
                continue;
            }
            let nth = view.piece_of(sprite.position);
            // How far it hangs beyond the piece holding it, each way: what a view must look
            // beyond itself to find it.
            let corner = Vec2::new((nth % across) as f32, (nth / across) as f32) * PIECE;
            view.before = view.before.max((corner - min).max(Vec2::ZERO));
            view.after = view.after.max((max - (corner + PIECE)).max(Vec2::ZERO));
            view.pieces[nth].take(place, sprite, overlay);
        }
        view
    }
}

/// The pieces between two grid lines, inside a map that has `many` of them.
fn span(first: f32, last: f32, many: usize) -> std::ops::Range<usize> {
    // Both ends are held inside the map, because the edge pieces hold what was authored beyond
    // them: a view off the side of a map must still ask the nearest column, or a prop standing
    // outside the edge is never drawn.
    let end = many.saturating_sub(1);
    let first = (first.max(0.0) as usize).min(end);
    let last = (last.max(0.0) as usize).clamp(first, end);
    first..last + 1
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

#[cfg(test)]
mod tests {
    use super::*;
    use dark_sprite::Rect;

    fn white() -> TextureId {
        TextureId::FIRST
    }

    /// A 16×16 sprite standing at `at`.
    fn at(x: f32, y: f32) -> Sprite {
        Sprite::new(
            white(),
            Rect::new(0, 0, 16, 16),
            Vec2::new(x, y),
            Vec2::ZERO,
        )
    }

    /// A thing that stands on its feet, `w` by `h`, as a tree or a character does.
    fn standing(x: f32, y: f32, w: u32, h: u32) -> Sprite {
        let mut s = Sprite::new(
            white(),
            Rect::new(0, 0, w, h),
            Vec2::new(x, y),
            Vec2::new(w as f32 / 2.0, h as f32),
        );
        s.sort_y = y;
        s
    }

    fn view_of(size: Vec2, sprites: Vec<Sprite>) -> MapView {
        MapView::place(size, sprites.into_iter().map(|s| (s, false)))
    }

    fn seen(view: &MapView, min: Vec2, max: Vec2) -> Vec<Sprite> {
        let mut out = Vec::new();
        view.seen(min, max, &mut out);
        out
    }

    /// A field where sprites hang beyond the piece holding them every way: trees standing on
    /// their feet (drawn above where they stand) and tiles drawn down and to the right. The
    /// spacing shares no factor with a piece, so plenty of both straddle a piece's edge.
    fn field() -> (MapView, Vec<Sprite>, Vec2) {
        let size = Vec2::splat(2048.0);
        let mut sprites = Vec::new();
        let mut y = 5.0;
        while y < size.y {
            let mut x = 11.0;
            while x < size.x {
                sprites.push(standing(x, y, 48, 96));
                let mut tile = at(x + 30.0, y + 18.0);
                tile.repeat = Vec2::new(5.0, 4.0);
                sprites.push(tile);
                x += 70.0;
            }
            y += 66.0;
        }
        let view = view_of(size, sprites.clone());
        // The field really does hang beyond its pieces, every way; without this the sweep can
        // pass while half of the looking is broken.
        assert!(
            view.before.min_element() > 0.0 && view.after.min_element() > 0.0,
            "before {} after {}",
            view.before,
            view.after
        );
        (view, sprites, size)
    }

    /// Walks a screenful over the whole map and counts what was drawn in it but not copied.
    fn missed(view: &MapView, sprites: &[Sprite], size: Vec2) -> usize {
        // A quarter of a piece: fine enough that a view's edge falls inside the overhang of
        // something in the row beyond it, which is where a one-sided look goes wrong.
        let step = PIECE / 4.0;
        let (mut missed, mut steps) = (0, 0);
        let mut y = -100.0;
        while y < size.y {
            let mut x = -100.0;
            while x < size.x {
                let (min, max) = (Vec2::new(x, y), Vec2::new(x + 640.0, y + 360.0));
                let copied = seen(view, min, max);
                missed += sprites
                    .iter()
                    .filter(|s| drawn_in(s, min, max) && !copied.contains(s))
                    .count();
                steps += 1;
                x += step;
            }
            y += step;
        }
        assert!(steps > 1000, "only {steps} views tried");
        missed
    }

    /// Nothing drawn inside the view is missed. This is the one that matters: a sprite quietly
    /// dropped is a tree that blinks out as the player walks past it.
    #[test]
    fn nothing_drawn_in_the_view_is_missed() {
        let (view, sprites, size) = field();
        assert_eq!(missed(&view, &sprites, size), 0);
    }

    /// And the sweep can see it when the looking is wrong — both ways round. A view is widened
    /// at the near edge by how far sprites hang down and right, and at the far edge by how far
    /// they hang up and left; take either away and the sweep must notice. Without this, the
    /// sweep above can pass by the arithmetic of where its steps happen to land.
    #[test]
    fn the_sweep_sees_a_one_sided_look() {
        for blinded in ["before", "after"] {
            let (mut view, sprites, size) = field();
            if blinded == "before" {
                view.before = Vec2::ZERO;
            } else {
                view.after = Vec2::ZERO;
            }
            assert!(
                missed(&view, &sprites, size) > 0,
                "the sweep did not notice a view that never looks {blinded} itself"
            );
        }
    }

    /// The point of §24.2: what a frame copies is the size of the screen, not of the map.
    #[test]
    fn only_what_the_camera_can_see_is_copied() {
        let size = Vec2::splat(8192.0);
        let every: Vec<Sprite> = (0..64)
            .flat_map(|row| (0..64).map(move |col| at(col as f32 * 128.0, row as f32 * 128.0)))
            .collect();
        let all = every.len();
        let view = view_of(size, every);
        assert_eq!(view.sprites(), all, "every sprite is held somewhere");
        let (min, max) = (
            Vec2::splat(4000.0),
            Vec2::splat(4000.0) + Vec2::new(640.0, 360.0),
        );
        let copied = seen(&view, min, max).len();
        assert!(
            copied < all / 32,
            "copied {copied} of {all} for a screenful"
        );
    }

    /// A tree is drawn where it stands but covers the ground above its feet: it must not vanish
    /// while the top of it still shows, nor be copied when it is far away.
    #[test]
    fn a_tall_sprite_shows_while_any_of_it_does() {
        // Feet at y 900, 200 tall: drawn from y 700. Its piece is the one at y 768..1024.
        let view = view_of(Vec2::splat(2048.0), vec![standing(300.0, 900.0, 32, 200)]);
        // A view that ends at 760: above the piece holding the tree, over the top of the tree.
        assert_eq!(
            seen(&view, Vec2::new(200.0, 650.0), Vec2::new(840.0, 760.0)).len(),
            1,
            "the top of the tree is in view, but its feet are in the piece below"
        );
        // Above the tree altogether, and away to the side: not copied.
        assert!(seen(&view, Vec2::new(200.0, 400.0), Vec2::new(840.0, 690.0)).is_empty());
        assert!(seen(&view, Vec2::new(1400.0, 700.0), Vec2::new(2040.0, 900.0)).is_empty());
    }

    /// The ground is one sprite repeated over the whole map, so it is drawn wherever the camera
    /// looks — and it does not drag its neighbours along with it.
    #[test]
    fn the_ground_is_drawn_wherever_the_camera_looks() {
        let size = Vec2::splat(4096.0);
        let mut ground = Sprite::new(white(), Rect::new(0, 0, 16, 16), Vec2::ZERO, Vec2::ZERO);
        ground.repeat = size / 16.0;
        let view = view_of(size, vec![ground, at(8.0, 8.0), at(3000.0, 3000.0)]);
        let copied = seen(&view, Vec2::new(2000.0, 2000.0), Vec2::new(2640.0, 2360.0));
        assert_eq!(
            copied.len(),
            1,
            "the ground, and nothing standing elsewhere"
        );
        assert_eq!(copied[0].repeat, size / 16.0);
    }

    /// Sprites are handed over in the order the map was built in, whichever pieces they fell in:
    /// the renderer's sort is stable, so what ties keeps the order it is given in, and the land,
    /// what stands on it and a sprite too wide for a piece all tie by design.
    #[test]
    fn the_order_the_map_was_built_in_is_kept() {
        // Built in this order, and deliberately scattered across pieces back to front, with a
        // wide one in the middle of the list.
        let cliff = at(700.0, 700.0);
        let great = standing(500.0, 500.0, 512, 512);
        let tree = at(100.0, 100.0);
        let view = MapView::place(
            Vec2::splat(1024.0),
            vec![(cliff, false), (great, false), (tree, false)].into_iter(),
        );
        assert_eq!(
            seen(&view, Vec2::ZERO, Vec2::splat(1024.0)),
            vec![cliff, great, tree],
            "the list's own order, not the pieces'"
        );
    }

    /// A wide sprite is still found when the pieces it crosses also hold sprites of their own —
    /// a great tree standing in a wood does not blink out because a shrub shares its piece.
    #[test]
    fn a_wide_sprite_survives_its_pieces_being_used() {
        let great = standing(1000.0, 1000.0, 512, 600);
        let (min, max) = great.covers();
        // A small sprite in the corner of every piece the tree crosses, added after it.
        let mut sprites = vec![great];
        let mut y = (min.y / PIECE).floor() * PIECE;
        while y <= max.y {
            let mut x = (min.x / PIECE).floor() * PIECE;
            while x <= max.x {
                sprites.push(at(x + 1.0, y + 1.0));
                x += PIECE;
            }
            y += PIECE;
        }
        let view = view_of(Vec2::splat(4096.0), sprites);
        // A view inside the tree — it is drawn from (744, 400) — and over none of the small ones.
        let copied = seen(&view, Vec2::new(800.0, 600.0), Vec2::new(900.0, 700.0));
        assert_eq!(copied, vec![great], "the tree the view is standing in");
    }

    /// A wide sprite authored off the edge of the map is still held by the nearest pieces, as a
    /// small one is, rather than pointed at by nobody and never drawn.
    #[test]
    fn a_wide_sprite_off_the_map_is_still_held() {
        let great = standing(5000.0, 5000.0, 512, 512);
        let view = view_of(Vec2::splat(1024.0), vec![great]);
        assert_eq!(view.sprites(), 1);
        let (min, max) = great.covers();
        assert_eq!(seen(&view, min, max), vec![great]);
    }

    /// Two pieces sharing a wide sprite draw it once, and a view elsewhere does not draw it.
    #[test]
    fn a_sprite_larger_than_a_piece_is_drawn_once_only() {
        let great_tree = standing(1000.0, 1000.0, 512, 600);
        let view = view_of(Vec2::splat(4096.0), vec![great_tree, at(1000.0, 1500.0)]);
        let copied = seen(&view, Vec2::new(800.0, 700.0), Vec2::new(1440.0, 1060.0));
        assert_eq!(
            copied.iter().filter(|s| **s == great_tree).count(),
            1,
            "the tree crosses several pieces the view can see"
        );
        assert!(seen(&view, Vec2::new(3000.0, 3000.0), Vec2::new(3640.0, 3360.0)).is_empty());
    }

    /// The promise of §24.2, measured: with the same thing under the camera, a frame copies the
    /// same sprites whether the map is four screens across or two hundred. That is what "no
    /// slower as the world grows" means, and it fails the moment anything walks the whole map.
    #[test]
    fn a_frame_costs_the_same_however_big_the_map_is() {
        let counted = |across: f32| {
            let size = Vec2::splat(across);
            // One sprite every 64 px, everywhere, so density does not change with size.
            let every: usize = (across / 64.0) as usize;
            let sprites: Vec<Sprite> = (0..every)
                .flat_map(|row| (0..every).map(move |col| at(col as f32 * 64.0, row as f32 * 64.0)))
                .collect();
            let held = sprites.len();
            let view = view_of(size, sprites);
            // The same screenful, in the same place, whatever the map around it.
            let (min, max) = (
                Vec2::splat(1000.0),
                Vec2::splat(1000.0) + Vec2::new(640.0, 360.0),
            );
            (seen(&view, min, max).len(), held)
        };
        // Both maps cover the view; only what is around it differs.
        let (small, held_small) = counted(2_560.0);
        let (large, held_large) = counted(128_000.0);
        assert_eq!(
            small, large,
            "a screenful copied {small} sprites in a map holding {held_small}, \
             and {large} in one fifty times wider holding {held_large}"
        );
        assert!(
            held_large > held_small * 1000,
            "the large map really is larger"
        );
        assert!(large < 200, "a screenful is {large} sprites");
    }

    /// A sprite outside the map is held by the nearest piece rather than lost or panicking.
    #[test]
    fn a_sprite_off_the_map_is_still_held() {
        let view = view_of(
            Vec2::splat(1024.0),
            vec![at(-40.0, -40.0), at(5000.0, 5000.0)],
        );
        assert_eq!(view.sprites(), 2);
        assert_eq!(
            seen(&view, Vec2::new(-200.0, -200.0), Vec2::new(200.0, 200.0)).len(),
            1,
            "the one just off the top-left corner"
        );
    }
}
