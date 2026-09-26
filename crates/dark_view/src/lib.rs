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
use glam::{Mat4, Vec2, Vec3, Vec4};

/// How wide and tall a piece of a map is, in pixels. Small enough that little outside the screen
/// is copied with the pieces that meet it; large enough that a map is not mostly bookkeeping.
/// A screenful of the first game is 640×360, so a view meets three or four pieces a side.
const PIECE: f32 = 256.0;

/// How many places in the drawing order one piece's land may take. A piece is sixteen tiles a
/// side and a tile draws a handful of things, so this is room to spare.
const PER_PIECE: u64 = 1 << 20;
/// Where the things standing on the land begin, after every piece's land.
const PROPS_FROM: u64 = 1 << 50;
/// And where what *grew* on it begins, after the props a scene placed by hand. Its own range,
/// because a grown thing is numbered by where it stands rather than by its place in a list.
const GROWN_FROM: u64 = 1 << 53;
/// And where the ways out begin, after those.
const EXITS_FROM: u64 = 1 << 55;

/// How many pieces a sprite may cross before it is simply drawn wherever the camera looks. The
/// ground covers every piece there is, and listing it against each of them would be the very
/// cost this is all about.
const SPREAD: usize = 64;

/// How many pieces are kept made at once. A screenful reaches a dozen, so this is a good deal of
/// walking about before the piece made longest ago is let go.
const KEPT: usize = 512;

/// What a piece of a map is made from: the map itself and the pictures its things are drawn with.
/// Held by whoever is drawing, and lent to the view when a piece has to be made.
pub struct Scenery<'a> {
    pub map: &'a Map,
    pub sheets: &'a HashMap<String, LoadedSheet>,
    pub textures: &'a HashMap<String, TextureId>,
}

/// What it takes to make a piece, worked out once when the map is opened.
struct Recipe {
    ground_tex: TextureId,
    white: TextureId,
    top: Option<Rect>,
    face: Option<Rect>,
    /// What to draw where the land made water, if it made any.
    water: Option<[f32; 4]>,
    jump_apex: f32,
    /// Which props, colliders and ways out belong to which piece, sorted by piece so a piece can
    /// find its own in a moment. Only the things are listed; the land itself is walked.
    props: Vec<(u32, u32)>,
    colliders: Vec<(u32, u32)>,
    exits: Vec<(u32, u32)>,
}

impl Recipe {
    /// What one piece holds, out of a list sorted by piece.
    fn belonging(of: &[(u32, u32)], piece: u32) -> &[(u32, u32)] {
        let first = of.partition_point(|(p, _)| *p < piece);
        let last = of.partition_point(|(p, _)| *p <= piece);
        &of[first..last]
    }
}

/// One piece of a map: the sprites standing in it, each with the place it had when the map was
/// one list, and what they cover once drawn.
struct Piece {
    /// What the sprites held here cover — more than the piece itself, since a tree hangs above
    /// its own feet. Empty until something is put here.
    min: Vec2,
    max: Vec2,
    statics: Vec<(u64, Sprite)>,
    overlay: Vec<(u64, Sprite)>,
    /// Whether the land under this piece had been made when it was made (docs/PLAN.md §24.4).
    /// A piece made over land that was shaped afterwards would show level grass over a hill for
    /// ever, and one made over land since let go of would show a hill that is no longer there.
    land_was_made: bool,
}

impl Default for Piece {
    fn default() -> Self {
        Self {
            // Nothing is here yet, so anything put here decides both corners.
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
            statics: Vec::new(),
            overlay: Vec::new(),
            land_was_made: false,
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

    fn take(&mut self, nth: u64, sprite: Sprite, overlay: bool) {
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

/// Whether the land under a piece whose top-left corner is at `corner` has all been made.
///
/// A piece is 256 px and a patch of land is 64 tiles, so at any tile size the game uses a piece
/// lies inside one patch; the four corners are asked all the same, in case a project's tiles are
/// so small that a patch is narrower than a piece. Ground beyond the map counts as made: there is
/// nothing there to shape.
fn land_under(terrain: &dark_physics::Terrain, corner: Vec2) -> bool {
    let far = corner + Vec2::splat(PIECE - 0.5);
    [
        (corner.x, corner.y),
        (far.x, corner.y),
        (corner.x, far.y),
        (far.x, far.y),
    ]
    .into_iter()
    .all(|(x, y)| {
        let (col, row) = terrain.tile_of(Vec2::new(x, y));
        match (u32::try_from(col), u32::try_from(row)) {
            (Ok(col), Ok(row)) => {
                col >= terrain.cols() || row >= terrain.rows() || terrain.is_made(col, row)
            }
            _ => true,
        }
    })
}

/// Whether a sprite is drawn inside `min..max`.
fn drawn_in(sprite: &Sprite, min: Vec2, max: Vec2) -> bool {
    let (a, b) = sprite.covers();
    a.x < max.x && min.x < b.x && a.y < max.y && min.y < b.y
}

/// Static sprites and debug overlay of one map, in pieces a fraction of a screen across.
///
/// A piece is made the first time the camera reaches it and let go once a great many others have
/// been made since, so what a map costs is what is being looked at rather than how much of it
/// there is (docs/PLAN.md §24.4).
pub struct MapView {
    pub size: Vec2,
    /// One place per piece of the map, holding the piece once it has been made. A map nobody has
    /// looked at costs a pointer a piece and nothing more.
    pieces: Vec<Option<Box<Piece>>>,
    /// The pieces made so far, in the order they were made, so the oldest can be let go.
    made: Vec<usize>,
    /// What to make a piece from. None in a view built from ready-made sprites, which is how the
    /// tests make one.
    recipe: Option<Recipe>,
    /// Sprites larger than a piece — the ground, a great tree — held once here, with the place
    /// each had when the map was one list.
    wide: Vec<(u64, Sprite, bool)>,
    /// Which of those each piece crosses. Kept by the map, because a piece one crosses may not
    /// have been made yet.
    crossing: HashMap<usize, Vec<u32>>,
    /// And those that cross so much of the map that it is cheaper to look at them every time.
    everywhere: Vec<u32>,
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
    pub fn seen(&mut self, world: &Scenery, min: Vec2, max: Vec2, out: &mut Vec<Sprite>) {
        self.ready(world, min, max);
        self.gather(min, max, false, out, &|s| s);
    }

    /// The same for the debug overlay, each sprite passed through `each` on the way (the editor
    /// dims it).
    pub fn seen_overlay(
        &mut self,
        world: &Scenery,
        min: Vec2,
        max: Vec2,
        out: &mut Vec<Sprite>,
        each: impl Fn(Sprite) -> Sprite,
    ) {
        self.ready(world, min, max);
        self.gather(min, max, true, out, &each);
    }

    /// Makes every piece this view reaches that has not been made yet, and lets go of the ones
    /// made longest ago once there are more than [`KEPT`] of them.
    fn ready(&mut self, world: &Scenery, min: Vec2, max: Vec2) {
        if self.recipe.is_none() {
            return;
        }
        let terrain = &world.map.collision.terrain;
        // What grows in a patch is worked out once for all the pieces that stand on it: a patch
        // is 64 tiles and a piece 16, so sixteen pieces would otherwise ask the same question and
        // throw away fifteen sixteenths of every answer (§24.5).
        let mut grown: HashMap<(i64, i64), Vec<dark_world::Grown>> = HashMap::new();
        for nth in self.reach(min - Vec2::ONE, max + Vec2::ONE) {
            // A piece whose land has been made — or let go of — since it was made is made again;
            // the rest are left alone, however much of the map has been shaped elsewhere.
            let corner = Vec2::new((nth % self.across) as f32, (nth / self.across) as f32) * PIECE;
            let made = land_under(terrain, corner);
            if self
                .pieces
                .get(nth)
                .and_then(|piece| piece.as_ref())
                .is_some_and(|piece| piece.land_was_made != made)
            {
                self.pieces[nth] = None;
                self.made.retain(|held| *held != nth);
            }
            if matches!(self.pieces.get(nth), Some(None)) {
                self.make(nth, world, &mut grown);
            }
        }
        while self.made.len() > KEPT {
            let oldest = self.made.remove(0);
            self.pieces[oldest] = None;
        }
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
        let mut gathered: Vec<(u64, Sprite)> = Vec::new();
        let mut wide: Vec<u32> = self.everywhere.clone();
        for (nth, piece) in self.around(min, max) {
            let held = if overlay {
                &piece.overlay
            } else {
                &piece.statics
            };
            gathered.extend(held.iter().filter(|(_, s)| drawn_in(s, min, max)));
            if let Some(crossing) = self.crossing.get(&nth) {
                wide.extend_from_slice(crossing);
            }
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

    /// The pieces a view of `min..max` reaches — the ones that must be made for it, whether or
    /// not they turn out to hold anything. However large the map, this is a handful.
    fn reach(&self, min: Vec2, max: Vec2) -> Vec<usize> {
        let first = ((min - self.after) / PIECE).floor();
        let last = ((max + self.before) / PIECE).floor();
        let cols = span(first.x, last.x, self.across);
        let rows = span(first.y, last.y, self.down);
        rows.flat_map(|row| cols.clone().map(move |col| row * self.across + col))
            .collect()
    }

    /// The pieces that show inside `min..max`. Only the pieces whose own square is near the view
    /// are asked — never all of them — so this costs the size of the view and not of the map.
    fn around(&self, min: Vec2, max: Vec2) -> impl Iterator<Item = (usize, &Piece)> {
        let first = ((min - self.after) / PIECE).floor();
        let last = ((max + self.before) / PIECE).floor();
        let cols = span(first.x, last.x, self.across);
        let rows = span(first.y, last.y, self.down);
        rows.flat_map(move |row| cols.clone().map(move |col| row * self.across + col))
            .filter_map(|nth| Some((nth, self.pieces.get(nth)?.as_deref()?)))
            .filter(move |(_, piece)| piece.seen_in(min, max))
    }

    /// How many pieces have been made. For the tests: a view of a great map that has only been
    /// looked at in one place should have made only the pieces around that place.
    pub fn made(&self) -> usize {
        self.made.len()
    }

    /// How many static sprites the whole map holds. For logs and tests; a frame never asks.
    pub fn sprites(&self) -> usize {
        let wide = self.wide.iter().filter(|(_, _, overlay)| !overlay).count();
        wide + self
            .pieces
            .iter()
            .flatten()
            .map(|p| p.statics.len())
            .sum::<usize>()
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
        let ground_sheet = &sheets[&def.ground.sheet].sheet;
        let ground_tex = textures[&def.ground.sheet];
        let frame_rect = |index: u32| ground_sheet.frames.get(index as usize).map(|f| f.rect);
        let across = ((size.x / PIECE).ceil() as usize).max(1);
        let down = ((size.y / PIECE).ceil() as usize).max(1);
        let mut view = Self {
            size,
            pieces: (0..across * down).map(|_| None).collect(),
            made: Vec::new(),
            recipe: Some(Recipe {
                ground_tex,
                white,
                top: frame_rect(def.terrain.top_frame.unwrap_or(def.ground.frame)),
                face: frame_rect(def.terrain.face_frame.unwrap_or(def.ground.frame)),
                water: def.land.as_ref().map(|_| [0.10, 0.22, 0.42, 1.0]),
                jump_apex,
                props: Vec::new(),
                colliders: Vec::new(),
                exits: Vec::new(),
            }),
            wide: Vec::new(),
            crossing: HashMap::new(),
            everywhere: Vec::new(),
            across,
            down,
            before: Vec2::ZERO,
            after: Vec2::ZERO,
        };

        // The ground is one sprite repeated over the whole map: too big for any piece, so the
        // map holds it and every piece it crosses points at it.
        if let Some(rect) = frame_rect(def.ground.frame) {
            let mut sprite = Sprite::new(ground_tex, rect, Vec2::ZERO, Vec2::ZERO);
            sprite.repeat = size / Vec2::new(rect.w as f32, rect.h as f32);
            sprite.layer = layer::GROUND;
            view.lay_wide(0, sprite, false);
        }
        // Floors laid over it, flat, on the surface under their middle: the ground, or the top of
        // a rise a town was levelled onto, which is drawn lifted by its height in a layer of its
        // own — so the floor is too, after every top in that layer. Floors are meant for ground a
        // place has levelled; one spanning several levels is drawn at its middle's. They draw in
        // the order they are written, so a pier laid after the water is on the water.
        let tile = terrain.tile();
        for (nth, floor) in def.floors.iter().enumerate() {
            let Some(rect) = frame_rect(floor.frame) else {
                tracing::warn!(
                    "{}: a floor names frame {} of the ground sheet, which has no such frame",
                    map.name,
                    floor.frame
                );
                continue;
            };
            let (col, row, w, h) = floor.tiles;
            let middle = (
                i64::from(col) + i64::from(w / 2),
                i64::from(row) + i64::from(h / 2),
            );
            let level = terrain
                .cell(middle.0, middle.1)
                .and_then(Cell::level)
                .unwrap_or(0);
            let at = Vec2::new(col as f32, row as f32) * tile
                - Vec2::new(0.0, f32::from(level) * terrain.level_height());
            let area = Vec2::new(w as f32, h as f32) * tile;
            let mut sprite = Sprite::new(ground_tex, rect, at, Vec2::ZERO);
            sprite.repeat = area / Vec2::new(rect.w as f32, rect.h as f32);
            if level == 0 {
                sprite.layer = layer::GROUND;
                sprite.sort_y = 0.0;
            } else {
                sprite.layer = layer::TERRAIN + i32::from(level.min(40));
                // After every top in the layer, which sort by the rows they stand on.
                sprite.sort_y = size.y + tile;
            }
            sprite.sub = 1 + i16::try_from(nth).unwrap_or(i16::MAX - 1);
            view.lay_wide(0, sprite, false);
        }

        // Which piece everything standing on the land belongs to, and how far each hangs beyond
        // it — which is what a view must look past itself to find them. The land is not made
        // here; it is made when the camera reaches it.
        let world = Scenery {
            map,
            sheets,
            textures,
        };
        let (mut props, mut colliders, mut exits) = (Vec::new(), Vec::new(), Vec::new());
        for nth in 0..map.props.len() {
            if let Some(sprite) = prop_sprite(&world, nth) {
                view.belongs(&mut props, nth, &sprite, PROPS_FROM, false);
            }
        }
        // Only the colliders the map was built with. What grows on made land comes and goes,
        // and its number is given back and used again (§24.5), so a picture that held one would
        // later draw a footprint belonging to something a kilometre away.
        for nth in 0..map.built_colliders as usize {
            let sprite = view
                .recipe
                .as_ref()
                .and_then(|recipe| collider_sprite(&world, recipe, nth));
            if let Some(sprite) = sprite {
                view.belongs(&mut colliders, nth, &sprite, PROPS_FROM, true);
            }
        }
        for (nth, exit) in map.exits.iter().enumerate() {
            let sprite = tint(
                white,
                exit.min,
                exit.max - exit.min,
                [1.0, 1.0, 0.2, 0.35],
                0.0,
            );
            view.belongs(&mut exits, nth, &sprite, EXITS_FROM, true);
        }

        // A top is drawn its own height above the tile it covers, so the tallest level the scene
        // asks for says how far the land reaches beyond the piece holding it. That is known
        // without walking a single tile.
        let drawn = def
            .terrain
            .fill
            .iter()
            .filter_map(|fill| fill.cell.level())
            .max()
            .unwrap_or(0);
        // Made land can raise a tile as high as the land makes them, and the view has not seen
        // any of it yet, so it must allow for the highest there is (docs/PLAN.md §24.4).
        let tallest = if def.land.is_some() {
            drawn.max(dark_land::HIGHEST)
        } else {
            drawn
        };
        view.before.y = view
            .before
            .y
            .max(f32::from(tallest) * terrain.level_height());

        if let Some(recipe) = &mut view.recipe {
            props.sort_unstable();
            colliders.sort_unstable();
            exits.sort_unstable();
            (recipe.props, recipe.colliders, recipe.exits) = (props, colliders, exits);
        }
        tracing::debug!(
            "{}: {} pieces, made as they are reached",
            map.name,
            across * down
        );
        view
    }

    /// Makes one piece: the land in it, and the things standing on that land.
    fn make(
        &mut self,
        nth: usize,
        world: &Scenery,
        grown: &mut HashMap<(i64, i64), Vec<dark_world::Grown>>,
    ) {
        let Some(recipe) = &self.recipe else {
            return;
        };
        let mut piece = Piece::default();
        let corner = Vec2::new((nth % self.across) as f32, (nth / self.across) as f32) * PIECE;
        let terrain = &world.map.collision.terrain;
        piece.land_was_made = land_under(terrain, corner);
        let (from_col, from_row) = terrain.tile_of(corner);
        let (to_col, to_row) = terrain.tile_of(corner + Vec2::splat(PIECE - 0.5));
        // Every sprite carries where it would have stood in one list of the whole map, so what is
        // drawn never depends on when a piece happened to be made.
        let mut place = 1 + (nth as u64) * PER_PIECE;
        for row in from_row..=to_row {
            for col in from_col..=to_col {
                let Some(cell) = terrain.cell(col, row) else {
                    continue;
                };
                tiles(recipe, terrain, col, row, cell, &mut |sprite, overlay| {
                    piece.take(place, sprite, overlay);
                    place += 1;
                });
            }
        }
        // What grows on made land is not in the map's list of props: it is worked out from the
        // seed, the same way the ground it stands on is (docs/PLAN.md §24.5). A piece asks for
        // what grows in the patches it covers, and keeps only what shows here.
        if world.map.land.is_some() {
            let patch = i64::from(dark_physics::Terrain::PATCH);
            let (first, last) = (
                (from_col.div_euclid(patch), from_row.div_euclid(patch)),
                (to_col.div_euclid(patch), to_row.div_euclid(patch)),
            );
            for patch_row in first.1..=last.1 {
                for patch_col in first.0..=last.0 {
                    // Nothing is drawn on land that has not been made: its footprints are not in
                    // the collision world yet, and a wood a body can walk through is worse than
                    // bare ground. The piece is made again when its land is (§24.4).
                    let (Ok(at_col), Ok(at_row)) = (
                        u32::try_from(patch_col * patch),
                        u32::try_from(patch_row * patch),
                    ) else {
                        continue;
                    };
                    if !terrain.is_made(at_col, at_row) {
                        continue;
                    }
                    let here = grown
                        .entry((patch_col * patch, patch_row * patch))
                        .or_insert_with(|| {
                            world
                                .map
                                .grown_in_patch(patch_col * patch, patch_row * patch)
                        })
                        .clone();
                    for one in here {
                        let (col, row) = terrain.tile_of(one.at);
                        if !(from_col..=to_col).contains(&col)
                            || !(from_row..=to_row).contains(&row)
                        {
                            continue;
                        }
                        // Where it stands is its place in the drawing order, so what is drawn
                        // never depends on when a piece happened to be made.
                        let place = grown_place(&one);
                        if let Some(sprite) = grown_sprite(world, &one) {
                            piece.take(place, sprite, false);
                        }
                        // Its footprint for the overlay, worked out here rather than looked up:
                        // what grows is not in the map's built list of colliders, and a piece
                        // must show what a body will walk into.
                        if let Some(sprite) = grown_overlay(world, recipe, &one) {
                            piece.take(place, sprite, true);
                        }
                    }
                }
            }
        }
        let which = nth as u32;
        for &(_, prop) in Recipe::belonging(&recipe.props, which) {
            if let Some(sprite) = prop_sprite(world, prop as usize) {
                piece.take(PROPS_FROM + u64::from(prop), sprite, false);
            }
        }
        for &(_, collider) in Recipe::belonging(&recipe.colliders, which) {
            if let Some(sprite) = collider_sprite(world, recipe, collider as usize) {
                piece.take(PROPS_FROM + u64::from(collider), sprite, true);
            }
        }
        for &(_, exit) in Recipe::belonging(&recipe.exits, which) {
            if let Some(area) = world.map.exits.get(exit as usize) {
                let sprite = tint(
                    recipe.white,
                    area.min,
                    area.max - area.min,
                    [1.0, 1.0, 0.2, 0.35],
                    0.0,
                );
                piece.take(EXITS_FROM + u64::from(exit), sprite, true);
            }
        }
        self.pieces[nth] = Some(Box::new(piece));
        self.made.push(nth);
    }

    /// Notes which piece a thing belongs to, and how far its picture hangs beyond that piece.
    /// Anything too big for a piece is held by the map itself instead.
    fn belongs(
        &mut self,
        into: &mut Vec<(u32, u32)>,
        nth: usize,
        sprite: &Sprite,
        from: u64,
        overlay: bool,
    ) {
        let (min, max) = sprite.covers();
        if (max - min).max_element() > PIECE {
            self.lay_wide(from + nth as u64, *sprite, overlay);
            return;
        }
        let piece = self.piece_of(sprite.position);
        let corner = Vec2::new((piece % self.across) as f32, (piece / self.across) as f32) * PIECE;
        self.before = self.before.max((corner - min).max(Vec2::ZERO));
        self.after = self.after.max((max - (corner + PIECE)).max(Vec2::ZERO));
        into.push((piece as u32, nth as u32));
    }

    /// Holds a sprite too big for one piece. One that crosses a few pieces is pointed at by
    /// each of them; one that covers half the map — the ground — is simply looked at every time,
    /// because listing it against a million pieces would cost more than the map itself.
    fn lay_wide(&mut self, place: u64, sprite: Sprite, overlay: bool) {
        let (min, max) = sprite.covers();
        let nth = self.wide.len() as u32;
        self.wide.push((place, sprite, overlay));
        let crossed = self.crossed_by(min, max);
        if crossed.len() > SPREAD {
            self.everywhere.push(nth);
            return;
        }
        for piece in crossed {
            self.crossing.entry(piece).or_default().push(nth);
        }
    }

    /// Lays ready-made sprites out in pieces by where each one stands, remembering the place
    /// each had in the list they came in. Only the tests make a view this way; a map's own view
    /// keeps a recipe and makes its pieces as the camera reaches them.
    #[cfg(test)]
    fn place(size: Vec2, sprites: impl Iterator<Item = (Sprite, bool)>) -> Self {
        let across = ((size.x / PIECE).ceil() as usize).max(1);
        let down = ((size.y / PIECE).ceil() as usize).max(1);
        let mut view = Self {
            size,
            pieces: (0..across * down).map(|_| None).collect(),
            made: Vec::new(),
            recipe: None,
            wide: Vec::new(),
            crossing: HashMap::new(),
            everywhere: Vec::new(),
            across,
            down,
            before: Vec2::ZERO,
            after: Vec2::ZERO,
        };
        for (place, (sprite, overlay)) in sprites.enumerate() {
            let place = place as u64;
            let (min, max) = sprite.covers();
            // Larger than a piece — the ground, a great tree — so no one piece can hold it: the
            // map holds it and every piece it crosses points at it. Holding it where it stands
            // would make every view ask far beyond itself to be sure of finding it.
            if (max - min).max_element() > PIECE {
                view.lay_wide(place, sprite, overlay);
                for piece in view.crossed_by(min, max) {
                    view.pieces[piece]
                        .get_or_insert_default()
                        .covering(min, max);
                }
                continue;
            }
            let nth = view.piece_of(sprite.position);
            // How far it hangs beyond the piece holding it, each way: what a view must look
            // beyond itself to find it.
            let corner = Vec2::new((nth % across) as f32, (nth / across) as f32) * PIECE;
            view.before = view.before.max((corner - min).max(Vec2::ZERO));
            view.after = view.after.max((max - (corner + PIECE)).max(Vec2::ZERO));
            view.pieces[nth]
                .get_or_insert_default()
                .take(place, sprite, overlay);
        }
        view
    }
}

/// The sprites of one tile of land: its top, its rims, the cap that hides anyone standing just
/// behind it, and the cliff face below it. Each is handed to `land` with whether it belongs to
/// the debug overlay.
///
/// A top is a floor: drawn in the terrain layer (per level) below every character and prop, so
/// whatever stands on it is always visible. Two pieces are drawn in the world layer to hide
/// things correctly: a north cap, the strip of top that can overlap someone standing just behind
/// (north of) the plateau, sorted by the plateau's north edge; and a cliff face down to the tile
/// in front, sorted by its south edge, so anything standing in front draws after it.
fn tiles(
    recipe: &Recipe,
    terrain: &dark_physics::Terrain,
    col: i64,
    row: i64,
    cell: Cell,
    land: &mut impl FnMut(Sprite, bool),
) {
    let (tile, lh, white) = (terrain.tile(), terrain.level_height(), recipe.white);
    let rim = [0.0, 0.0, 0.0, 0.45];
    let origin = Vec2::new(col as f32, row as f32) * tile;
    if cell == Cell::Wall {
        land(
            tint(white, origin, Vec2::splat(tile), [0.9, 0.1, 0.1, 0.35], 0.0),
            true,
        );
        // A wall a scene drew is hidden under its own art; one the land made has none, so it is
        // drawn as what it is — water — rather than left as grass nobody may walk on. The terrain
        // knows which is which, so an authored cliff on a made map is not painted blue.
        if let Some(water) = recipe.water.filter(|_| !terrain.drawn_by_hand(col, row)) {
            let mut s = tint(white, origin, Vec2::splat(tile), water, 0.0);
            s.layer = layer::TERRAIN;
            land(s, false);
        }
        return;
    }
    let level = cell.level().unwrap_or(0);
    if level == 0 {
        return;
    }
    let height = f32::from(level) * lh;
    let lifted = origin - Vec2::new(0.0, height);
    // Clamped so absurdly high levels never reach the world layer.
    let floor_layer = layer::TERRAIN + i32::from(level.min(40));
    land(
        tint(
            white,
            lifted,
            Vec2::splat(tile),
            [0.2, 0.4, 1.0, 0.12 * f32::from(level)],
            0.0,
        ),
        true,
    );
    let neighbour = |dc: i64, dr: i64| terrain.cell(col + dc, row + dr).and_then(Cell::level);
    let lower = |dc: i64, dr: i64| neighbour(dc, dr).is_some_and(|n| n < level);
    let on_floor = |mut s: Sprite| {
        s.layer = floor_layer;
        s.sort_y = origin.y;
        s
    };
    if let Some(rect) = recipe.top {
        land(
            on_floor(Sprite::new(
                recipe.ground_tex,
                sub_rect(rect, origin, Vec2::splat(tile)),
                lifted,
                Vec2::ZERO,
            )),
            false,
        );
    }
    // Darkened rims where the neighbour is lower, so plateaus read at a glance.
    if lower(-1, 0) {
        land(
            on_floor(tint(white, lifted, Vec2::new(1.0, tile), rim, 0.0)),
            false,
        );
    }
    if lower(1, 0) {
        land(
            on_floor(tint(
                white,
                lifted + Vec2::new(tile - 1.0, 0.0),
                Vec2::new(1.0, tile),
                rim,
                0.0,
            )),
            false,
        );
    }
    if let Some(north) = neighbour(0, -1)
        && north < level
    {
        // Anyone standing north of the edge overlaps at most the height difference of this top
        // (clipped to one tile).
        let cap = (f32::from(level - north) * lh).min(tile);
        if let Some(rect) = recipe.top {
            let mut s = Sprite::new(
                recipe.ground_tex,
                sub_rect(rect, origin, Vec2::new(tile, cap)),
                lifted,
                Vec2::ZERO,
            );
            s.sort_y = origin.y;
            land(s, false);
        }
        land(
            tint(white, lifted, Vec2::new(tile, 1.0), rim, origin.y),
            false,
        );
    }
    let front = neighbour(0, 1);
    if let (Some(rect), Some(front)) = (recipe.face, front)
        && front < level
    {
        let south = origin.y + tile;
        for step in front..level {
            let y = south - f32::from(step + 1) * lh;
            let src = sub_rect(rect, Vec2::new(origin.x, y), Vec2::new(tile, lh));
            let mut s = Sprite::new(recipe.ground_tex, src, Vec2::new(origin.x, y), Vec2::ZERO);
            s.color = [0.75, 0.75, 0.75, 1.0];
            s.sort_y = south;
            land(s, false);
        }
    }
}

/// A prop standing on the land, drawn with its feet where it stands.
fn prop_sprite(world: &Scenery, nth: usize) -> Option<Sprite> {
    let prop = world.map.props.get(nth)?;
    let Some(frame) = world.sheets[&prop.sheet]
        .sheet
        .frames
        .get(prop.frame as usize)
    else {
        tracing::warn!("{}: no frame {}", prop.sheet, prop.frame);
        return None;
    };
    let at = Vec2::from(prop.position);
    let ground = world.map.collision.ground_under(at, 0.5);
    let ground = if ground.is_finite() { ground } else { 0.0 };
    let mut sprite = Sprite::new(
        world.textures[&prop.sheet],
        frame.rect,
        at - Vec2::new(0.0, ground),
        frame.pivot,
    );
    sprite.sort_y = at.y;
    Some(sprite)
}

/// Where something grown stands in the drawing order: its own place in the world, so every
/// piece and every machine gives it the same one. Ties by `sort_y` are broken the same way
/// everywhere, which is all this number is for.
fn grown_place(grown: &dark_world::Grown) -> u64 {
    let part = |n: f32| (n as i64).rem_euclid(1 << 21) as u64;
    GROWN_FROM + (part(grown.at.y) << 21) + part(grown.at.x)
}

/// What grew on made land, drawn as the props beside it are: standing on the ground, sorted by
/// where its feet are.
fn grown_sprite(world: &Scenery, grown: &dark_world::Grown) -> Option<Sprite> {
    let (sheet, frame) = world.map.grown_look(grown)?;
    let looks = world.sheets.get(sheet)?;
    let Some(frame) = looks.sheet.frames.get(frame as usize) else {
        tracing::warn!("{sheet}: no frame {}", grown.frame);
        return None;
    };
    let ground = world.map.ground_of(grown.at);
    let mut sprite = Sprite::new(
        *world.textures.get(sheet)?,
        frame.rect,
        grown.at - Vec2::new(0.0, ground),
        frame.pivot,
    );
    sprite.sort_y = grown.at.y;
    Some(sprite)
}

/// The footprint of something that grew on made land, for the debug overlay, drawn as the props
/// beside it are.
fn grown_overlay(world: &Scenery, recipe: &Recipe, grown: &dark_world::Grown) -> Option<Sprite> {
    let group = world.map.def.scatter.get(grown.group)?;
    let collider = grown.collider(group, world.map.ground_of(grown.at))?;
    Some(footprint_sprite(&collider, recipe))
}

/// A prop's footprint, for the debug overlay. Orange blocks at any height; cyan is low enough to
/// jump over or stand on.
fn collider_sprite(world: &Scenery, recipe: &Recipe, nth: usize) -> Option<Sprite> {
    let c = world.map.collision.collider(nth as u32)?;
    Some(footprint_sprite(c, recipe))
}

/// One footprint drawn: orange blocks at any height, cyan is low enough to jump over or stand on.
fn footprint_sprite(c: &dark_physics::Collider, recipe: &Recipe) -> Sprite {
    let (half, outline) = match c.shape {
        Shape::Circle { radius } => (Vec2::splat(radius), Outline::Circle),
        Shape::Rect { half } => (half, Outline::Rect),
    };
    let color = if c.height <= recipe.jump_apex {
        [0.3, 0.9, 1.0, 0.9]
    } else {
        [1.0, 0.6, 0.0, 0.9]
    };
    Sprite::outline(
        recipe.white,
        outline,
        c.center - half - Vec2::new(0.0, c.base),
        half * 2.0,
        color,
    )
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

    /// What a view hands over for a camera. A view of ready-made sprites has nothing to make
    /// first, so it is asked directly.
    fn seen(view: &MapView, min: Vec2, max: Vec2) -> Vec<Sprite> {
        let mut out = Vec::new();
        view.gather(min, max, false, &mut out, &|s| s);
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

    /// However large the map, a screenful reaches a handful of pieces — and only those are ever
    /// made. This is what keeps a great map from costing anything until it is walked through
    /// (docs/PLAN.md §24.4).
    #[test]
    fn a_screenful_reaches_a_handful_of_pieces_however_large_the_map() {
        // 16 km at 16 px to the metre: a million pieces.
        let world = Vec2::splat(16_000.0 * 16.0);
        let view = view_of(world, vec![standing(1000.0, 1000.0, 48, 96)]);
        assert_eq!(view.pieces.len(), 1_000_000, "a thousand pieces a side");
        let middle = world / 2.0;
        let reached = view.reach(middle, middle + Vec2::new(640.0, 360.0));
        assert!(
            reached.len() <= 20,
            "a screenful reached {} pieces of {}",
            reached.len(),
            view.pieces.len()
        );
        assert_eq!(
            view.made(),
            0,
            "a view nobody has looked at has made nothing"
        );
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

/// Where a model stands in the world it is drawn in: `X` runs east, `Y` stands up out of the
/// ground, and `Z` runs south. The ground plane `XZ` is the map's own, pixel for pixel.
///
/// glTF is Y-up, so a model loads into this frame already (`dark_model::Model::UP`).
pub fn stand_at(feet: Vec2, elevation: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(feet.x, elevation, feet.y))
        * Mat4::from_scale(Vec3::splat(scale))
}

/// The camera a model is drawn through, agreeing pixel for pixel with the flat one the sprites
/// use.
///
/// **It is not a tilted camera.** Turn a camera down to fifty degrees and a step north moves you
/// `sin 50°` of a pixel up the screen, while the sprite world moves a whole one; the ground would
/// disagree with itself. What a top-down game actually looks through is an **oblique**
/// projection: the ground is one to one with world pixels, and height shears straight up the
/// screen. That is the projection the art already assumes — a character's feet are at their
/// world place and the sprite rises from there — so a model and a sprite of the same person
/// stand in the same spot.
///
/// A step north and a rise of the same size therefore move a point the same way, which is what
/// lets height read as height.
///
/// `camera` and `internal` are what [`dark_render::Renderer::render_scene`] is given, and the
/// origin is rounded exactly as it rounds it, or models would sit half a pixel off the ground.
pub fn model_camera(camera: Vec2, internal: (u32, u32)) -> Mat4 {
    let size = Vec2::new(internal.0.max(1) as f32, internal.1.max(1) as f32);
    let origin = (camera - size / 2.0).round();

    // Depth: what is further south, and what is higher, is nearer the viewer. Measured from the
    // middle of the screen rather than from the world's corner, so a map kilometres across keeps
    // its precision where the player is.
    let middle = origin.y + size.y / 2.0;
    // How far either way the depth buffer reaches. It cannot scale with the view alone: a tall
    // thing near the edge of a small viewport is further out in depth than that viewport is
    // wide. Half of this is the budget, and it buys two screens of ground plus four thousand
    // pixels of height in each direction — far more than a frame can hold, and a 32-bit depth
    // buffer still resolves a thousandth of a pixel across it.
    let span = size.y * 4.0 + 8192.0;

    Mat4::from_cols(
        // X: east, one world pixel to one screen pixel.
        Vec4::new(2.0 / size.x, 0.0, 0.0, 0.0),
        // Y: up the screen, and nearer.
        Vec4::new(0.0, 2.0 / size.y, -1.0 / span, 0.0),
        // Z: south is down the screen, and nearer.
        Vec4::new(0.0, -2.0 / size.y, -1.0 / span, 0.0),
        Vec4::new(
            -2.0 * origin.x / size.x - 1.0,
            1.0 + 2.0 * origin.y / size.y,
            0.5 + middle / span,
            1.0,
        ),
    )
}

#[cfg(test)]
mod camera_tests {
    use super::*;

    const SIZE: (u32, u32) = (640, 360);

    /// Where a point lands on the screen, in pixels from the top-left.
    fn on_screen(camera: Vec2, at: Vec3) -> Vec2 {
        let clip = model_camera(camera, SIZE) * at.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        Vec2::new(
            (ndc.x * 0.5 + 0.5) * SIZE.0 as f32,
            (0.5 - ndc.y * 0.5) * SIZE.1 as f32,
        )
    }

    fn depth(camera: Vec2, at: Vec3) -> f32 {
        let clip = model_camera(camera, SIZE) * at.extend(1.0);
        clip.z / clip.w
    }

    /// The ground is the map's own, pixel for pixel: a model standing where a sprite stands is
    /// drawn where that sprite is drawn. This is the whole reason the projection is oblique.
    #[test]
    fn the_ground_agrees_with_the_flat_camera_pixel_for_pixel() {
        let camera = Vec2::new(1000.0, 800.0);
        // How `render_scene` places a sprite: world minus the rounded origin.
        let origin = (camera - Vec2::new(SIZE.0 as f32, SIZE.1 as f32) / 2.0).round();
        for world in [
            Vec2::new(1000.0, 800.0),
            Vec2::new(1017.0, 743.0),
            Vec2::new(812.5, 931.25),
        ] {
            let flat = world - origin;
            let mesh = on_screen(camera, Vec3::new(world.x, 0.0, world.y));
            assert!(
                mesh.abs_diff_eq(flat, 1e-3),
                "a model at {world} lands at {mesh}, a sprite at {flat}"
            );
        }
    }

    /// A step north and a rise of the same size move a point the same way. That is what makes
    /// height read as height rather than as walking away.
    #[test]
    fn rising_looks_the_same_as_stepping_north() {
        let camera = Vec2::new(0.0, 0.0);
        let foot = Vec3::new(10.0, 0.0, 20.0);
        let risen = on_screen(camera, foot + Vec3::new(0.0, 32.0, 0.0));
        let north = on_screen(camera, foot - Vec3::new(0.0, 0.0, 32.0));
        assert!(
            risen.abs_diff_eq(north, 1e-3),
            "rising put it at {risen}, stepping north at {north}"
        );
    }

    /// What is further south, and what is higher, is nearer the viewer — so a character's front
    /// hides their back, and they stand in front of what is behind them.
    #[test]
    fn what_is_south_and_what_is_high_is_nearer() {
        let camera = Vec2::new(0.0, 0.0);
        let here = Vec3::new(0.0, 0.0, 0.0);
        assert!(
            depth(camera, here + Vec3::new(0.0, 0.0, 40.0)) < depth(camera, here),
            "further south is nearer"
        );
        assert!(
            depth(camera, here + Vec3::new(0.0, 40.0, 0.0)) < depth(camera, here),
            "higher is nearer"
        );
        assert_eq!(
            depth(camera, here + Vec3::new(50.0, 0.0, 0.0)),
            depth(camera, here),
            "east and west are the same distance away"
        );
    }

    /// Everything a frame can see has to land inside the depth buffer's range, or it is clipped
    /// away and simply never drawn.
    #[test]
    fn a_screenful_stays_inside_the_depth_range() {
        let camera = Vec2::new(12_000.0, 9_000.0);
        let (w, h) = (SIZE.0 as f32, SIZE.1 as f32);
        // Twice as far out as the view reaches, and taller than anything that stands in it: a
        // model is submitted before it is culled, and a tree is not a person's height.
        for corner in [-1.0f32, 1.0] {
            for high in [0.0f32, 400.0, 1800.0] {
                let at = Vec3::new(camera.x + corner * w, high, camera.y + corner * h);
                let z = depth(camera, at);
                assert!(
                    (0.0..=1.0).contains(&z),
                    "a corner of the view at {at} has depth {z}"
                );
            }
        }
    }

    /// A model stands where its feet are put, at the scale it is given.
    #[test]
    fn standing_puts_the_feet_where_they_were_asked_for() {
        let at = stand_at(Vec2::new(300.0, 200.0), 0.0, 20.0);
        assert_eq!(
            at.transform_point3(Vec3::ZERO),
            Vec3::new(300.0, 0.0, 200.0)
        );
        // A model one unit tall stands twenty pixels tall.
        assert_eq!(at.transform_point3(Vec3::Y), Vec3::new(300.0, 20.0, 200.0));
    }
}
