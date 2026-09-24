//! Editing a scene, without any interface: what is under the mouse, painting terrain, placing,
//! moving and removing things, and undo. The interface calls these; tests do too.

use std::collections::HashSet;

use dark_assets::{
    ColliderDef, EnemyPlacement, ExitDef, FillDef, InnDef, LineDef, NpcDef, PlacedProp, SceneDef,
};
use dark_physics::{Cell, Shape};
use dark_sprite::Facing;
use glam::Vec2;

/// Something in the scene the editor can select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thing {
    Prop(usize),
    Npc(usize),
    Enemy(usize),
    Exit(usize),
    Inn(usize),
    /// A town, camp or ruin stamped on this map (docs/PLAN.md §24.5).
    Place(usize),
    PlayerStart,
}

/// How close (world pixels) a click must be to a point-like thing to pick it.
pub const PICK_RADIUS: f32 = 12.0;

/// How tall a person is, for picking (world pixels above the feet).
const BODY_HEIGHT: f32 = 32.0;

/// What is at `at`: the nearest person (or the player's start) standing there, else the
/// frontmost prop drawn there (`drawn` gives a prop's picture as x, y, width, height, if known),
/// else a prop whose foot is near, else an inn or exit.
pub fn pick(
    scene: &SceneDef,
    at: Vec2,
    drawn: impl Fn(&PlacedProp) -> Option<(f32, f32, f32, f32)>,
    sizes: impl Fn(&str) -> Option<(f32, f32)>,
) -> Option<Thing> {
    let near = |p: (f32, f32)| Vec2::from(p).distance(at);
    // A person is picked anywhere from the feet (where they stand) up to the head.
    let body = |p: (f32, f32)| {
        let feet = Vec2::from(p);
        let up = (feet.y - at.y).clamp(0.0, BODY_HEIGHT);
        (feet - Vec2::new(0.0, up)).distance(at)
    };
    let nearest = |points: Vec<(f32, Thing)>| {
        points
            .into_iter()
            .filter(|(d, _)| *d <= PICK_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, t)| t)
    };
    let mut people: Vec<(f32, Thing)> = Vec::new();
    if let Some(player) = &scene.player {
        people.push((body(player.spawn), Thing::PlayerStart));
    }
    people.extend(
        scene
            .npcs
            .iter()
            .enumerate()
            .map(|(i, n)| (body(n.position), Thing::Npc(i))),
    );
    people.extend(
        scene
            .enemies
            .iter()
            .enumerate()
            .map(|(i, e)| (body(e.position), Thing::Enemy(i))),
    );
    let inside =
        |(x, y, w, h): (f32, f32, f32, f32)| at.x >= x && at.x < x + w && at.y >= y && at.y < y + h;
    // The frontmost picture is the one drawn last: the one standing furthest south.
    let covering = scene
        .props
        .iter()
        .enumerate()
        .filter(|(_, p)| drawn(p).is_some_and(inside))
        .max_by(|a, b| a.1.position.1.total_cmp(&b.1.position.1))
        .map(|(i, _)| Thing::Prop(i));
    nearest(people)
        .or(covering)
        .or_else(|| {
            nearest(
                scene
                    .props
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (near(p.position), Thing::Prop(i)))
                    .collect(),
            )
        })
        .or_else(|| {
            scene
                .inns
                .iter()
                .position(|i| inside(i.area))
                .map(Thing::Inn)
        })
        .or_else(|| {
            scene
                .exits
                .iter()
                .position(|e| inside(e.area))
                .map(Thing::Exit)
        })
        .or_else(|| {
            // A place last of all: everything it holds is drawn inside it, so clicking a villager
            // in a town picks the villager, and clicking the town's ground picks the town. The
            // smallest one holding the click wins, so a quarter can be picked inside its town.
            let mut held: Vec<(f32, usize)> = scene
                .places
                .iter()
                .enumerate()
                .filter_map(|(i, place)| {
                    let (w, h) = sizes(&place.scene)?;
                    inside((place.at.0, place.at.1, w, h)).then_some((w * h, i))
                })
                .collect();
            held.sort_by(|a, b| a.0.total_cmp(&b.0));
            held.first().map(|(_, i)| Thing::Place(*i))
        })
}

/// Where a thing stands (an area's top-left corner), for dragging it.
pub fn position_of(scene: &SceneDef, thing: Thing) -> Option<Vec2> {
    Some(Vec2::from(match thing {
        Thing::Prop(i) => scene.props.get(i)?.position,
        Thing::Npc(i) => scene.npcs.get(i)?.position,
        Thing::Enemy(i) => scene.enemies.get(i)?.position,
        Thing::Exit(i) => {
            let (x, y, ..) = scene.exits.get(i)?.area;
            (x, y)
        }
        Thing::Inn(i) => {
            let (x, y, ..) = scene.inns.get(i)?.area;
            (x, y)
        }
        Thing::Place(i) => scene.places.get(i)?.at,
        Thing::PlayerStart => scene.player.as_ref()?.spawn,
    }))
}

/// Moves a thing to `to` (an area by its top-left corner; an inn's bed goes with it).
pub fn move_to(scene: &mut SceneDef, thing: Thing, to: Vec2) {
    let to = (to.x.round(), to.y.round());
    match thing {
        Thing::Prop(i) => {
            if let Some(p) = scene.props.get_mut(i) {
                p.position = to;
            }
        }
        Thing::Npc(i) => {
            if let Some(n) = scene.npcs.get_mut(i) {
                n.position = to;
            }
        }
        Thing::Enemy(i) => {
            if let Some(e) = scene.enemies.get_mut(i) {
                e.position = to;
            }
        }
        Thing::Exit(i) => {
            if let Some(e) = scene.exits.get_mut(i) {
                e.area.0 = to.0;
                e.area.1 = to.1;
            }
        }
        Thing::Inn(i) => {
            if let Some(inn) = scene.inns.get_mut(i) {
                let shift = (to.0 - inn.area.0, to.1 - inn.area.1);
                inn.area.0 = to.0;
                inn.area.1 = to.1;
                inn.bed = (inn.bed.0 + shift.0, inn.bed.1 + shift.1);
            }
        }
        Thing::Place(i) => {
            if let Some(place) = scene.places.get_mut(i) {
                place.at = to;
            }
        }
        Thing::PlayerStart => {
            if let Some(p) = &mut scene.player {
                p.spawn = to;
            }
        }
    }
}

/// Removes a thing (the player's start cannot be removed).
pub fn remove(scene: &mut SceneDef, thing: Thing) -> bool {
    let gone = |len: usize, i: usize| i < len;
    match thing {
        Thing::Prop(i) if gone(scene.props.len(), i) => {
            scene.props.remove(i);
        }
        Thing::Npc(i) if gone(scene.npcs.len(), i) => {
            scene.npcs.remove(i);
        }
        Thing::Enemy(i) if gone(scene.enemies.len(), i) => {
            scene.enemies.remove(i);
        }
        Thing::Exit(i) if gone(scene.exits.len(), i) => {
            scene.exits.remove(i);
        }
        Thing::Inn(i) if gone(scene.inns.len(), i) => {
            scene.inns.remove(i);
        }
        Thing::Place(i) if gone(scene.places.len(), i) => {
            scene.places.remove(i);
        }
        _ => return false,
    }
    true
}

/// A prop as a click places it: solid (a round footprint a third of its width) or not.
pub fn new_prop(sheet: &str, frame: u32, at: Vec2, width: u32, solid: bool) -> PlacedProp {
    let radius = (width as f32 / 3.0).round().max(2.0);
    PlacedProp {
        sheet: sheet.to_owned(),
        frame,
        position: (at.x.round(), at.y.round()),
        colliders: if solid {
            vec![ColliderDef {
                shape: Shape::Circle { radius },
                offset: (0.0, -radius * 0.5),
                height: 1000.0,
            }]
        } else {
            Vec::new()
        },
    }
}

/// A new villager saying one line (`line` is its string key).
pub fn new_npc(sheet: &str, at: Vec2, line: String) -> NpcDef {
    NpcDef {
        sheet: sheet.to_owned(),
        attack: None,
        face: None,
        name: None,
        moveset: None,
        position: (at.x.round(), at.y.round()),
        facing: Facing::Down,
        lines: vec![LineDef::Says(line)],
        actor: None,
        // A new villager stands where it was put until someone writes it a day (§21).
        day: Vec::new(),
    }
}

pub fn new_enemy(kind: &str, at: Vec2) -> EnemyPlacement {
    EnemyPlacement {
        kind: kind.to_owned(),
        position: (at.x.round(), at.y.round()),
        facing: Facing::Down,
    }
}

/// An area dragged out from `a` to `b`, at least a tile across.
pub fn area(a: Vec2, b: Vec2, tile: f32) -> (f32, f32, f32, f32) {
    let (lo, hi) = (a.min(b).round(), a.max(b).round());
    let size = (hi - lo).max(Vec2::splat(tile));
    (lo.x, lo.y, size.x, size.y)
}

pub fn new_exit(area: (f32, f32, f32, f32), to: &str, spawn: (f32, f32)) -> ExitDef {
    ExitDef {
        area,
        to: to.to_owned(),
        spawn,
    }
}

pub fn new_inn(area: (f32, f32, f32, f32)) -> InnDef {
    let (x, y, w, h) = area;
    InnDef {
        area,
        bed: ((x + w / 2.0).round(), (y + h / 2.0).round()),
    }
}

/// The scene's terrain as a grid of cells, `cols` by `rows` tiles.
pub fn terrain_grid(scene: &SceneDef, tile: u32) -> (u32, u32, Vec<Cell>) {
    let cols = (scene.size.0 / tile as f32).ceil().max(1.0) as u32;
    let rows = (scene.size.1 / tile as f32).ceil().max(1.0) as u32;
    let mut cells = vec![Cell::Floor; (cols * rows) as usize];
    for fill in &scene.terrain.fill {
        let (x, y, w, h) = fill.tiles;
        for row in y..(y + h).min(rows) {
            for col in x..(x + w).min(cols) {
                cells[(row * cols + col) as usize] = fill.cell;
            }
        }
    }
    (cols, rows, cells)
}

/// Paints a square of `wide` tiles, middled on the tile under `at`. One tile is a pencil; more is
/// a brush, which is what painting a hillside wants.
pub fn paint_wide(scene: &mut SceneDef, tile: u32, at: Vec2, cell: Cell, wide: u32) -> bool {
    let wide = wide.max(1) as i64;
    let half = (wide - 1) / 2;
    let (col, row) = (
        (at.x / tile as f32).floor() as i64,
        (at.y / tile as f32).floor() as i64,
    );
    let mut changed = false;
    for down in 0..wide {
        for across in 0..wide {
            let (c, r) = (col + across - half, row + down - half);
            if c < 0 || r < 0 {
                continue;
            }
            changed |= paint_one(scene, tile, c as u32, r as u32, cell);
        }
    }
    changed
}

/// Floods every tile alike and touching the one under `at` — the bucket. What "alike" means is
/// what that tile is now, so filling a lake fills the lake and stops at its shore.
///
/// A map is at most a few million tiles and a fill can reach all of them, so this walks rather
/// than recurses, and says how many it painted.
pub fn fill_from(scene: &mut SceneDef, tile: u32, at: Vec2, cell: Cell) -> usize {
    let (cols, rows, cells) = terrain_grid(scene, tile);
    let (col, row) = (
        (at.x / tile as f32).floor() as i64,
        (at.y / tile as f32).floor() as i64,
    );
    if col < 0 || row < 0 || col as u32 >= cols || row as u32 >= rows {
        return 0;
    }
    let was = cells[(row as u32 * cols + col as u32) as usize];
    if was == cell {
        return 0;
    }
    let mut cells = cells;
    let mut queue = vec![(col as u32, row as u32)];
    let mut painted = 0;
    while let Some((col, row)) = queue.pop() {
        let nth = (row * cols + col) as usize;
        if cells[nth] != was {
            continue;
        }
        cells[nth] = cell;
        painted += 1;
        for (across, down) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let (c, r) = (col as i64 + across, row as i64 + down);
            if c >= 0 && r >= 0 && (c as u32) < cols && (r as u32) < rows {
                queue.push((c as u32, r as u32));
            }
        }
    }
    // Written as one drawing of the whole map rather than as a fill a tile: a flood covers
    // thousands of them, and the scene file would be a list of every one.
    if painted > 0 {
        scene.terrain.fill.clear();
        let mut runs: Vec<FillDef> = Vec::new();
        for row in 0..rows {
            let mut col = 0;
            while col < cols {
                let here = cells[(row * cols + col) as usize];
                let mut wide = 1;
                while col + wide < cols && cells[(row * cols + col + wide) as usize] == here {
                    wide += 1;
                }
                if here != Cell::Floor {
                    runs.push(FillDef {
                        tiles: (col, row, wide, 1),
                        cell: here,
                    });
                }
                col += wide;
            }
        }
        scene.terrain.fill = runs;
    }
    painted
}

/// Paints one tile, if it is not already what it should be.
fn paint_one(scene: &mut SceneDef, tile: u32, col: u32, row: u32, cell: Cell) -> bool {
    let (cols, rows, cells) = terrain_grid(scene, tile);
    if col >= cols || row >= rows {
        return false;
    }
    if cells[(row * cols + col) as usize] == cell {
        return false;
    }
    scene.terrain.fill.push(FillDef {
        tiles: (col, row, 1, 1),
        cell,
    });
    true
}

/// Paints `cell` on every tile the line from `from` to `to` crosses (a pointer moving fast
/// between frames leaves no gaps), with a brush `wide` tiles across. True if it changed anything.
pub fn paint_line_wide(
    scene: &mut SceneDef,
    tile: u32,
    from: Vec2,
    to: Vec2,
    cell: Cell,
    wide: u32,
) -> bool {
    let step = tile as f32 / 2.0;
    let steps = (from.distance(to) / step).ceil().max(1.0) as u32;
    let mut changed = false;
    for i in 0..=steps {
        changed |= paint_wide(
            scene,
            tile,
            from.lerp(to, i as f32 / steps as f32),
            cell,
            wide,
        );
    }
    changed
}

/// Rewrites the terrain's fills as few rectangles as a simple sweep finds (the same grid):
/// painting tile by tile would otherwise leave thousands.
pub fn compact_terrain(scene: &mut SceneDef, tile: u32) {
    let (cols, rows, mut cells) = terrain_grid(scene, tile);
    let mut fills = Vec::new();
    for row in 0..rows {
        let mut col = 0;
        while col < cols {
            let cell = cells[(row * cols + col) as usize];
            if cell == Cell::Floor {
                col += 1;
                continue;
            }
            // As wide as the run goes, then as far down as every row matches.
            let mut w = 1;
            while col + w < cols && cells[(row * cols + col + w) as usize] == cell {
                w += 1;
            }
            let mut h = 1;
            while row + h < rows
                && (col..col + w).all(|c| cells[((row + h) * cols + c) as usize] == cell)
            {
                h += 1;
            }
            for r in row..row + h {
                for c in col..col + w {
                    cells[(r * cols + c) as usize] = Cell::Floor;
                }
            }
            fills.push(FillDef {
                tiles: (col, row, w, h),
                cell,
            });
            col += w;
        }
    }
    scene.terrain.fill = fills;
}

/// Scene snapshots to step back and forth through.
#[derive(Default)]
pub struct History {
    undo: Vec<SceneDef>,
    redo: Vec<SceneDef>,
}

/// Steps kept to undo.
const HISTORY: usize = 200;

impl History {
    /// Call before an edit, with the scene as it was.
    pub fn record(&mut self, before: &SceneDef) {
        self.undo.push(before.clone());
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, scene: &mut SceneDef) -> bool {
        match self.undo.pop() {
            Some(before) => {
                self.redo.push(std::mem::replace(scene, before));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, scene: &mut SceneDef) -> bool {
        match self.redo.pop() {
            Some(after) => {
                self.undo.push(std::mem::replace(scene, after));
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// Every string key the scene names: its villagers' names and lines.
pub fn keys_in(scene: &SceneDef) -> HashSet<String> {
    scene
        .npcs
        .iter()
        .flat_map(|n| {
            n.name
                .iter()
                .cloned()
                .chain(n.lines.iter().map(|l| l.key().to_owned()))
        })
        .collect()
}

/// A fresh scene: an empty field of `ground` frame 0, `cols` by `rows` tiles.
pub fn blank_scene(ground_sheet: &str, cols: u32, rows: u32, tile: u32) -> SceneDef {
    let text = format!(
        r#"(size: ({w}, {h}), ground: (sheet: "{ground_sheet}", frame: 0))"#,
        w = cols * tile,
        h = rows * tile
    );
    ron::from_str(&text).expect("a blank scene always parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> SceneDef {
        let mut s = blank_scene("g", 10, 8, 16);
        s.player = Some(ron::from_str(r#"(sheet: "p", spawn: (40, 40))"#).expect("a player"));
        s
    }

    #[test]
    fn painting_then_compacting_keeps_the_same_ground_in_few_rectangles() {
        let mut s = scene();
        for col in 2..6 {
            for row in 1..4 {
                paint_wide(
                    &mut s,
                    16,
                    Vec2::new(col as f32 * 16.0 + 3.0, row as f32 * 16.0 + 3.0),
                    Cell::Level(1),
                    1,
                );
            }
        }
        assert!(paint_wide(
            &mut s,
            16,
            Vec2::new(8.0 * 16.0, 5.0 * 16.0),
            Cell::Wall,
            1
        ));
        assert!(
            !paint_wide(&mut s, 16, Vec2::new(8.0 * 16.0, 5.0 * 16.0), Cell::Wall, 1),
            "no change"
        );
        assert!(
            !paint_wide(&mut s, 16, Vec2::new(-5.0, 5.0), Cell::Wall, 1),
            "off the map"
        );
        let before = terrain_grid(&s, 16);
        assert_eq!(s.terrain.fill.len(), 13);
        compact_terrain(&mut s, 16);
        assert_eq!(terrain_grid(&s, 16), before, "the same ground");
        assert_eq!(s.terrain.fill.len(), 2, "one block and one wall");
        // Painting floor back over it clears it.
        paint_wide(
            &mut s,
            16,
            Vec2::new(8.0 * 16.0, 5.0 * 16.0),
            Cell::Floor,
            1,
        );
        compact_terrain(&mut s, 16);
        assert_eq!(s.terrain.fill.len(), 1);
    }

    /// The bucket floods what is alike and touching, and stops at whatever is not: a lake fills
    /// to its shore, and the far side of the shore is left alone.
    #[test]
    fn the_bucket_fills_what_is_alike_and_stops_at_what_is_not() {
        let mut s = scene();
        let tile = 16;
        let (cols, rows, _) = terrain_grid(&s, tile);
        assert_eq!((cols, rows), (10, 8), "the scene these tests use");
        // A wall down the middle, dividing the map in two.
        for row in 0..rows {
            paint_wide(
                &mut s,
                tile,
                Vec2::new(5.0 * 16.0 + 8.0, row as f32 * 16.0 + 8.0),
                Cell::Wall,
                1,
            );
        }
        let painted = fill_from(&mut s, tile, Vec2::new(8.0, 8.0), Cell::Level(2));
        let (cols, rows, cells) = terrain_grid(&s, tile);
        let at = |col: u32, row: u32| cells[(row * cols + col) as usize];
        assert_eq!(
            painted as u32,
            5 * rows,
            "the five columns west of the wall, every row"
        );
        assert_eq!(at(0, 0), Cell::Level(2), "the side that was filled");
        assert_eq!(at(4, rows - 1), Cell::Level(2), "and the far corner of it");
        assert_eq!(at(5, 3), Cell::Wall, "the wall itself is untouched");
        assert_eq!(at(6, 3), Cell::Floor, "and so is the other side");
        // Filling what is already that is nothing at all.
        assert_eq!(
            fill_from(&mut s, tile, Vec2::new(8.0, 8.0), Cell::Level(2)),
            0
        );
    }

    #[test]
    fn a_fast_stroke_paints_every_tile_it_crosses() {
        let mut s = scene();
        assert!(paint_line_wide(
            &mut s,
            16,
            Vec2::new(8.0, 8.0),
            Vec2::new(152.0, 8.0),
            Cell::Wall,
            1,
        ));
        let (cols, _, cells) = terrain_grid(&s, 16);
        assert!((0..10).all(|c| cells[c as usize] == Cell::Wall));
        assert_eq!(
            cells[cols as usize],
            Cell::Floor,
            "the row below is untouched"
        );
        assert!(!paint_line_wide(
            &mut s,
            16,
            Vec2::new(8.0, 8.0),
            Vec2::new(152.0, 8.0),
            Cell::Wall,
            1,
        ));
    }

    #[test]
    fn picking_finds_the_nearest_thing_then_areas_and_things_move_and_go() {
        let mut s = scene();
        s.npcs
            .push(new_npc("n", Vec2::new(100.0, 100.0), "k".into()));
        s.props
            .push(new_prop("p", 3, Vec2::new(124.0, 100.0), 30, true));
        s.inns.push(new_inn(area(
            Vec2::new(0.0, 80.0),
            Vec2::new(40.0, 120.0),
            16.0,
        )));
        let unknown = |_: &PlacedProp| None;
        // No places in this scene, so nothing has a size to be picked by.
        let sizes = |_: &str| None;
        assert_eq!(
            pick(&s, Vec2::new(101.0, 100.0), unknown, sizes),
            Some(Thing::Npc(0))
        );
        assert_eq!(
            pick(&s, Vec2::new(125.0, 101.0), unknown, sizes),
            Some(Thing::Prop(0))
        );
        assert_eq!(
            pick(&s, Vec2::new(40.0, 42.0), unknown, sizes),
            Some(Thing::PlayerStart)
        );
        assert_eq!(
            pick(&s, Vec2::new(10.0, 110.0), unknown, sizes),
            Some(Thing::Inn(0))
        );
        assert_eq!(pick(&s, Vec2::new(150.0, 20.0), unknown, sizes), None);
        // A tall picture is picked anywhere on it, not only at its foot.
        let tree = |p: &PlacedProp| Some((p.position.0 - 30.0, p.position.1 - 80.0, 60.0, 80.0));
        assert_eq!(
            pick(&s, Vec2::new(130.0, 30.0), tree, |_: &str| None),
            Some(Thing::Prop(0))
        );
        assert_eq!(
            pick(&s, Vec2::new(101.0, 100.0), tree, |_: &str| None),
            Some(Thing::Npc(0)),
            "people first"
        );
        assert_eq!(
            pick(&s, Vec2::new(102.0, 72.0), tree, |_: &str| None),
            Some(Thing::Npc(0)),
            "a person is picked by the head too"
        );
        // The inn's bed moves with it.
        move_to(&mut s, Thing::Inn(0), Vec2::new(10.0, 90.0));
        assert_eq!(s.inns[0].bed, (30.0, 110.0));
        move_to(&mut s, Thing::Npc(0), Vec2::new(120.4, 99.6));
        assert_eq!(s.npcs[0].position, (120.0, 100.0), "whole pixels");
        assert!(remove(&mut s, Thing::Prop(0)));
        assert!(!remove(&mut s, Thing::PlayerStart));
        assert!(s.props.is_empty());
        // A solid prop has a round footprint under it.
        let p = new_prop("p", 0, Vec2::ZERO, 30, true);
        assert!(matches!(p.colliders[0].shape, Shape::Circle { radius } if radius == 10.0));
    }

    #[test]
    fn the_keys_a_scene_names_are_its_villagers_names_and_lines() {
        let mut s = scene();
        let mut npc = new_npc("n", Vec2::ZERO, "npc.x.1".into());
        npc.name = Some("name.x.1".into());
        npc.lines.push(LineDef::Reply {
            reply: "npc.x.2".into(),
        });
        s.npcs.push(npc);
        let keys = keys_in(&s);
        assert_eq!(keys.len(), 3);
        assert!(
            ["name.x.1", "npc.x.1", "npc.x.2"]
                .iter()
                .all(|k| keys.contains(*k))
        );
    }

    #[test]
    fn edits_undo_and_redo() {
        let mut s = scene();
        let mut h = History::default();
        h.record(&s);
        s.enemies.push(new_enemy("grunt", Vec2::new(5.0, 5.0)));
        h.record(&s);
        s.enemies.clear();
        assert!(h.undo(&mut s));
        assert_eq!(s.enemies.len(), 1);
        assert!(h.undo(&mut s));
        assert!(s.enemies.is_empty());
        assert!(!h.undo(&mut s));
        assert!(h.redo(&mut s));
        assert_eq!(s.enemies.len(), 1);
        // A new edit forgets what could be redone.
        h.record(&s);
        assert!(!h.can_redo());
    }
}
