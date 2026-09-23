//! What grows on made land (docs/PLAN.md §24.5): the trees, rocks and tufts standing on ground
//! that was itself made from a seed (§24.4).
//!
//! A drawn map's props are placed once, when it is loaded, and written down. A made world cannot
//! be: sixteen kilometres of it is a hundred million tiles, and nobody is going to store a tree
//! for each of them. So nothing is stored. What grows at a tile is *worked out* from the world's
//! seed and that tile, in whole numbers, the same way the ground under it is — so the host and
//! every client grow the same wood without a word between them, and a player who walks away and
//! comes back finds the same tree.
//!
//! The scene says *what* may grow (its `scatter` groups: which sheet, which frames, how dense,
//! what footprint, which levels it likes); the seed says *where*. On a made map a group's `count`
//! is how many it wants in a patch of land — 64 tiles a side — because a made map has no end to
//! fill.
//!
//! Spacing is kept by construction, not by searching: the world is cut into cells as wide as the
//! group's `min_spacing`, and at most one thing grows in each. That is what makes this answerable
//! a tile at a time, by anyone, without knowing what grew anywhere else.

use dark_assets::{ColliderDef, ScatterDef};
use dark_physics::{Cell, Collider, Shape};
use glam::Vec2;

use crate::maps::Map;

/// One thing standing on made ground, worked out rather than remembered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grown {
    /// Which of the scene's scatter groups this grew from.
    pub group: usize,
    /// Which of that group's frames it wears.
    pub frame: u32,
    /// Where it stands, in world pixels.
    pub at: Vec2,
}

impl Grown {
    /// Its footprint, if its group gives one.
    pub fn collider(&self, group: &ScatterDef, base: f32) -> Option<Collider> {
        group.collider.map(|c: ColliderDef| Collider {
            center: self.at + Vec2::from(c.offset),
            shape: c.shape,
            base,
            height: c.height,
        })
    }
}

/// Stirs a number so that neighbouring ones come out nothing alike (splitmix64's finisher). The
/// same as `dark_land`'s, and for the same reason: every machine must agree.
fn stir(mut n: u64) -> u64 {
    n = (n ^ (n >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    n ^ (n >> 31)
}

/// One number from a seed and a place, for asking the same question of the same tile everywhere.
///
/// The group's place in the scene is stirred in beside its seed: a scene whose two groups carry
/// the same seed would otherwise grow one on top of the other, everywhere, for ever.
fn roll(seed: u64, group: usize, of: &ScatterDef, col: i64, row: i64) -> u64 {
    let mut n = seed ^ of.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    n = stir(n ^ (group as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    for part in [col as u64, row as u64] {
        n = stir(n ^ part.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    }
    stir(n)
}

/// How many tiles a patch of land holds each way. What grows is asked for a patch at a time,
/// because that is how the ground under it is made.
pub const PATCH: i64 = dark_physics::Terrain::PATCH as i64;

impl Map {
    /// What grows in the cell of the spacing grid at `(cell_col, cell_row)` for one group, if
    /// anything does. Pure: the same seed and the same cell give the same answer on every
    /// machine, for ever.
    fn grown_in_cell(&self, group: usize, cell_col: i64, cell_row: i64) -> Option<Grown> {
        let land = self.land?;
        let def = self.def.scatter.get(group)?;
        if def.frames.is_empty() || def.count == 0 {
            return None;
        }
        let tile = self.collision.terrain.tile();
        let wide = cell_tiles(def, tile);
        let n = roll(land.seed(), group, def, cell_col, cell_row);

        // Whether anything grows here at all. The group's `count` is how many it wants in a patch
        // of land, and a patch holds `(PATCH / wide)²` cells — which is rarely a whole number, so
        // it is worked out as a chance in a million rather than by counting cells. Rounding it to
        // whole cells is what made a group asking for four trees a patch grow eight.
        let chance = u64::from(def.count) * (wide as u64) * (wide as u64) * A_MILLION
            / ((PATCH as u64) * (PATCH as u64));
        if n % A_MILLION >= chance {
            return None;
        }

        // Where in its cell it stands: never nearer the edge than half the spacing, so two things
        // in neighbouring cells are always at least `min_spacing` apart. A pixel is taken off the
        // room to wander in, because a position is rounded to whole pixels afterwards and two
        // roundings could otherwise close the gap by one.
        let span = (wide as f32 * tile - def.min_spacing - 1.0).max(0.0);
        let jitter = |n: u64| ((n % 1_024) as f32 / 1_024.0 - 0.5) * span;
        let middle = (Vec2::new(cell_col as f32, cell_row as f32) + 0.5) * (wide as f32 * tile);
        let at = (middle + Vec2::new(jitter(n >> 8), jitter(n >> 24))).round();

        let grown = Grown {
            group,
            frame: def.frames[(n >> 40) as usize % def.frames.len()],
            at,
        };
        self.may_grow(def, &grown).then_some(grown)
    }

    /// Whether this thing may stand where it wants to: on ground of a level its group allows,
    /// clear of the map's edge, of the ways out, and of everywhere the scene puts somebody.
    fn may_grow(&self, def: &ScatterDef, grown: &Grown) -> bool {
        let Some(land) = self.land else {
            return false;
        };
        let terrain = &self.collision.terrain;
        let (col, row) = terrain.tile_of(grown.at);
        if terrain.cell(col, row).is_none() {
            return false;
        }
        // Nothing grows on ground somebody drew. A place stamped on the world is levelled into
        // it by hand (§24.5), and a town square with saplings coming up through it is not a town
        // square; the same goes for a hillside a designer put there on purpose.
        if terrain.drawn_by_hand(col, row) {
            return false;
        }
        // The land as the seed makes it, whether or not it has been made yet: a tree must not
        // appear or vanish depending on who has walked past.
        let cell = land.cell(col, row);
        let Some(level) = cell.level() else {
            return false;
        };
        if def.levels.as_ref().is_some_and(|l| !l.contains(&level)) {
            return false;
        }
        // Nothing grows where a scene puts somebody, or in a doorway. A great tree is kept
        // further off than a tuft of grass: what must stay clear is the clearing *plus* whatever
        // the thing itself would put there, or a player would start the game inside a trunk.
        let reach = def.collider.map_or(0.0, |c| footprint(c.shape));
        let clearing = crate::SPAWN_CLEARING + reach;
        let clear_of =
            |place: (f32, f32)| grown.at.distance_squared(Vec2::from(place)) >= clearing * clearing;
        let mut places = self
            .def
            .player
            .iter()
            .map(|player| player.spawn)
            .chain(self.def.npcs.iter().map(|npc| npc.position))
            .chain(
                self.def
                    .npcs
                    .iter()
                    .flat_map(|npc| npc.day.iter().map(|entry| entry.at)),
            )
            .chain(self.def.enemies.iter().map(|enemy| enemy.position))
            .chain(self.def.inns.iter().map(|inn| inn.bed))
            .chain(self.arrivals.iter().copied());
        places.all(clear_of)
            // A way out is kept clear by the whole footprint, not only by the middle of it: half
            // a trunk across a doorway is still across the doorway.
            && !self.exits.iter().any(|exit| {
                exit.min.x - reach < grown.at.x
                    && grown.at.x < exit.max.x + reach
                    && exit.min.y - reach < grown.at.y
                    && grown.at.y < exit.max.y + reach
            })
            // And not on top of anything the scene put there by hand.
            && !self.props.iter().any(|prop| {
                grown.at.distance_squared(Vec2::from(prop.position)) < clearing * clearing
            })
            && !self.near_an_earlier_group(def, grown)
    }

    /// Whether something already grew too near this, from a group written before it.
    ///
    /// Each group keeps its own spacing by its own grid of cells, which says nothing about the
    /// other groups: a rock would grow inside a tree. A drawn map's scatter keeps `min_spacing`
    /// from *every* prop already placed, including earlier groups, and this keeps the same
    /// promise — by asking the earlier groups' cells that reach this far, which is a handful.
    fn near_an_earlier_group(&self, def: &ScatterDef, grown: &Grown) -> bool {
        let tile = self.collision.terrain.tile();
        for earlier in 0..grown.group {
            let Some(before) = self.def.scatter.get(earlier) else {
                continue;
            };
            let wide = cell_tiles(before, tile) as f32 * tile;
            let reach = (def.min_spacing / wide).ceil() as i64;
            let (cell_col, cell_row) = (
                (grown.at.x / wide).floor() as i64,
                (grown.at.y / wide).floor() as i64,
            );
            for down in -reach..=reach {
                for across in -reach..=reach {
                    if let Some(one) =
                        self.grown_in_cell(earlier, cell_col + across, cell_row + down)
                        && one.at.distance(grown.at) < def.min_spacing
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Everything growing in the patch of land holding tile `(col, row)`.
    pub fn grown_in_patch(&self, col: i64, row: i64) -> Vec<Grown> {
        let tile = self.collision.terrain.tile();
        let (from_col, from_row) = (col.div_euclid(PATCH) * PATCH, row.div_euclid(PATCH) * PATCH);
        let mut grown = Vec::new();
        for (group, def) in self.def.scatter.iter().enumerate() {
            let wide = cell_tiles(def, tile);
            // The cells of this group's spacing grid that the patch covers. A group whose things
            // stand further apart than a patch is wide has its cells asked about once each.
            let first = (from_col.div_euclid(wide), from_row.div_euclid(wide));
            let last = (
                (from_col + PATCH - 1).div_euclid(wide),
                (from_row + PATCH - 1).div_euclid(wide),
            );
            for cell_row in first.1..=last.1 {
                for cell_col in first.0..=last.0 {
                    let Some(one) = self.grown_in_cell(group, cell_col, cell_row) else {
                        continue;
                    };
                    // A cell on the patch's edge belongs to whichever patch its middle is in, so
                    // that no tree is grown twice or left to two patches to let go of.
                    let (at_col, at_row) = self.collision.terrain.tile_of(one.at);
                    if at_col.div_euclid(PATCH) * PATCH == from_col
                        && at_row.div_euclid(PATCH) * PATCH == from_row
                    {
                        grown.push(one);
                    }
                }
            }
        }
        grown
    }

    /// How high the ground stands where something grew, from the land itself rather than from
    /// whatever has been made so far.
    ///
    /// Asking the terrain would be asking a question whose answer changes: a footprint on a tile
    /// boundary also reads the tile before it, which is level ground while its patch is unmade
    /// and a hillside once it is. A host walking east and a client walking west would then put
    /// the same tree at two heights.
    pub fn ground_of(&self, at: Vec2) -> f32 {
        let terrain = &self.collision.terrain;
        let (col, row) = terrain.tile_of(at);
        let cell = match self.land {
            Some(land) if !terrain.drawn_by_hand(col, row) => land.cell(col, row),
            _ => terrain.cell(col, row).unwrap_or(Cell::Floor),
        };
        cell.level()
            .map_or(0.0, |level| f32::from(level) * terrain.level_height())
    }

    /// The sheet and frame a grown thing is drawn with.
    pub fn grown_look(&self, grown: &Grown) -> Option<(&str, u32)> {
        let def = self.def.scatter.get(grown.group)?;
        Some((def.sheet.as_str(), grown.frame))
    }

    /// Puts the footprints of everything growing in a patch into the collision world, and says
    /// where they were put so they can be taken away with the patch.
    pub fn grow_patch(&mut self, col: i64, row: i64) -> Vec<u32> {
        let grown = self.grown_in_patch(col, row);
        let mut held = Vec::new();
        for one in grown {
            let Some(def) = self.def.scatter.get(one.group) else {
                continue;
            };
            if let Some(collider) = one.collider(def, self.ground_of(one.at)) {
                held.push(self.collision.add(collider));
            }
        }
        held
    }
}

/// How wide a group's spacing cells are, in tiles: at least one, and enough to hold the spacing
/// it asks for. A spacing that is not a sensible number of pixels — nothing, or wider than the
/// world — is held to the width of a patch, so one slip of a scene file cannot send a group's
/// whole crop to the corner of the map.
fn cell_tiles(def: &ScatterDef, tile: f32) -> i64 {
    if !def.min_spacing.is_finite() || def.min_spacing <= 0.0 {
        return 1;
    }
    ((def.min_spacing / tile).ceil() as i64).clamp(1, PATCH)
}

/// The denominator the chance of something growing is worked out against. Big enough that a
/// group asking for one thing in a patch of wide cells is still counted.
const A_MILLION: u64 = 1_000_000;

/// The width of a footprint, for keeping grown things off the things a scene drew.
pub fn footprint(shape: Shape) -> f32 {
    match shape {
        Shape::Circle { radius } => radius,
        Shape::Rect { half } => half.max_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::Maps;
    use dark_assets::Project;

    /// A made map with three things that may grow on it: great trees on the level ground only,
    /// rocks anywhere, and tufts of grass thick on the ground.
    fn a_made_map(name: &str, seed: u64) -> (std::path::PathBuf, Maps) {
        let dir = std::env::temp_dir().join(format!("dark_grow_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/made.ron"),
            format!(
                r#"(
                size: (32000, 32000),
                land: (seed: {seed}),
                ground: (sheet: "g", frame: 0),
                player: (sheet: "p", spawn: (9000, 9000)),
                scatter: [
                    (sheet: "trees", frames: [1, 2], count: 4, min_spacing: 360, seed: 11,
                     collider: (shape: Circle(radius: 36), offset: (0, -40), height: 1000),
                     levels: [0]),
                    (sheet: "rocks", frames: [0], count: 10, min_spacing: 110, seed: 13,
                     collider: (shape: Circle(radius: 6), offset: (0, -6), height: 1000)),
                    (sheet: "grass", frames: [15, 24, 25], count: 40, min_spacing: 60, seed: 14),
                ],
            )"#
            ),
        )
        .unwrap();
        let project = Project::open(dir.clone()).unwrap();
        let maps = Maps::load(&project, "scenes/made.ron").unwrap();
        (dir, maps)
    }

    /// A place stamped on a made world: everything it holds belongs to the map it is stamped on,
    /// moved to where it was put down; the ground under it is levelled and walks back out to the
    /// land around it; and nothing grows on top of it.
    ///
    /// A stamped town is how a made world gets anywhere worth walking to (§24.5), and the whole
    /// bargain is that nothing downstream can tell it from a town drawn where it stands.
    #[test]
    fn a_place_stamped_on_the_world_is_part_of_the_map() {
        use dark_assets::Project;

        let dir = std::env::temp_dir().join(format!("dark_place_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        // A camp: a hut with a footprint, a villager who walks to the fire, an inn to sleep in,
        // a way out to somewhere else, and a patch of raised ground drawn by hand.
        std::fs::write(
            dir.join("scenes/camp.ron"),
            r#"(
                size: (400, 400),
                ground: (sheet: "g", frame: 0),
                terrain: (fill: [(tiles: (2, 2, 4, 4), cell: Level(2))]),
                props: [(sheet: "hut", frame: 0, position: (100, 100),
                         colliders: [(shape: Circle(radius: 20))])],
                npcs: [(sheet: "n", position: (200, 200), lines: [],
                        day: [(from: 8, at: (240, 200))])],
                inns: [(area: (300, 300, 80, 80), bed: (340, 340))],
                exits: [(area: (0, 380, 400, 20), to: "scenes/away.ron", spawn: (50, 50))],
            )"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/away.ron"),
            r#"(size: (600, 600), ground: (sheet: "g", frame: 0))"#,
        )
        .unwrap();
        // The world it is stamped on: made land, with the camp put down well away from the start.
        std::fs::write(
            dir.join("scenes/world.ron"),
            r#"(
                size: (32000, 32000),
                land: (seed: 20260923),
                ground: (sheet: "g", frame: 0),
                player: (sheet: "p", spawn: (9000, 9000)),
                places: [(scene: "scenes/camp.ron", at: (12000, 12000))],
                scatter: [
                    (sheet: "grass", frames: [15], count: 40, min_spacing: 60, seed: 14),
                ],
            )"#,
        )
        .unwrap();
        let project = Project::open(dir.clone()).unwrap();
        let mut maps = Maps::load(&project, "scenes/world.ron").unwrap();
        let at = Vec2::new(12_000.0, 12_000.0);

        // Everything the camp holds is in the world's own map, moved to where it was put down —
        // and the scene it leads to is a map of the world, reached through the camp's own door.
        let map = &mut maps.maps[0];
        assert_eq!(map.def.npcs.len(), 1);
        assert_eq!(map.def.npcs[0].position, (12_200.0, 12_200.0));
        assert_eq!(map.def.npcs[0].day[0].at, (12_240.0, 12_200.0));
        assert_eq!(map.def.inns[0].bed, (12_340.0, 12_340.0));
        assert_eq!(map.props[0].position, (12_100.0, 12_100.0));
        assert_eq!(map.exits.len(), 1, "the camp's way out is the world's");
        assert!(map.exits[0].contains(at + Vec2::new(200.0, 390.0)));
        assert_eq!(maps.maps.len(), 2, "and it leads somewhere");

        // What the camp drew is where it was put down, and the land did not take it back.
        let map = &mut maps.maps[0];
        map.make_around(at);
        let tile = map.collision.terrain.tile();
        let (col, row) = map.collision.terrain.tile_of(at + Vec2::splat(3.0 * tile));
        assert_eq!(map.collision.terrain.cell(col, row), Some(Cell::Level(2)));

        let (from_col, from_row) = map.collision.terrain.tile_of(at);

        // And nothing grew on the camp: it is drawn ground, and a town square is not a meadow.
        let grown = map.grown_in_patch(from_col, from_row);
        for one in &grown {
            let in_camp = one.at.x >= at.x
                && one.at.x < at.x + 400.0
                && one.at.y >= at.y
                && one.at.y < at.y + 400.0;
            assert!(!in_camp, "{:?} grew inside the camp", one.at);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The ground under a stamped place is levelled, and walks back out to the land without
    /// leaving a wall a walker cannot climb or a pit they cannot get out of — including where a
    /// place is stamped inside another, which is the case that leaves a wall if a place is
    /// levelled without looking at what the place around it already did.
    ///
    /// It is run twice, on ground found by looking rather than hoped for: once on a shore, where
    /// the land put water under the place, and once on a hillside, where it put two levels. On
    /// the level plain this land mostly is, the levelling has nothing to do and a test of it
    /// would pass with the levelling deleted.
    #[test]
    fn a_place_is_levelled_into_the_land_and_leaves_no_wall() {
        let land = dark_land::Land::new(20_260_923);
        let side = 30i64;
        let kinds_in = |at: (i64, i64)| {
            let mut kinds = std::collections::BTreeSet::new();
            for row in (0..side).step_by(3) {
                for col in (0..side).step_by(3) {
                    kinds.insert(land.cell(at.0 + col, at.1 + row).level());
                }
            }
            kinds
        };
        let look_for = |wanted: fn(&std::collections::BTreeSet<Option<u8>>) -> bool| {
            (0..90)
                .flat_map(|down| {
                    (0..90).map(move |across| (1_000 + across * 61, 1_000 + down * 67))
                })
                .find(|at| wanted(&kinds_in(*at)))
        };
        // A shore: water under part of the place. And a hillside: two levels of dry ground.
        let shore = look_for(|kinds| kinds.len() >= 2 && kinds.contains(&None))
            .expect("this land has a shore somewhere");
        let hillside = look_for(|kinds| {
            kinds.len() >= 2 && kinds.iter().filter(|kind| kind.is_some()).count() >= 2
        })
        .expect("this land has a hillside somewhere");
        assert_ne!(shore, hillside, "one spot cannot stand for both");
        // A shore proves the water under a place is made ground; a hillside proves the ground
        // steps out to the land. Neither proves the other, so both are asked for.
        levelled_at("a shore", shore, side, &land, Wanted::WaterMadeGround);
        levelled_at("a hillside", hillside, side, &land, Wanted::GroundThatSteps);
    }

    /// What a levelling is expected to have done where it was asked for.
    #[derive(Clone, Copy, PartialEq)]
    enum Wanted {
        /// Water under the place became ground somebody can stand on.
        WaterMadeGround,
        /// The ground walked out from the place to the land, a step at a time.
        GroundThatSteps,
    }

    /// Stamps a hall with a quarter inside it at `spot`, and asks what the ground did.
    fn levelled_at(
        what: &str,
        spot: (i64, i64),
        side: i64,
        land: &dark_land::Land,
        wanted: Wanted,
    ) {
        use dark_assets::Project;

        let tile = 16i64;
        let at = (spot.0 * tile, spot.1 * tile);
        let dir = std::env::temp_dir().join(format!(
            "dark_level_{}_{}",
            std::process::id(),
            what.replace(' ', "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/quarter.ron"),
            format!(
                r#"(size: ({}, {}), ground: (sheet: "g", frame: 0))"#,
                8 * tile,
                8 * tile
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/hall.ron"),
            format!(
                r#"(size: ({}, {}), ground: (sheet: "g", frame: 0),
                    places: [(scene: "scenes/quarter.ron", at: ({}, {}))])"#,
                side * tile,
                side * tile,
                4 * tile,
                4 * tile
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/world.ron"),
            format!(
                r#"(
                    size: (128000, 128000),
                    land: (seed: 20260923),
                    ground: (sheet: "g", frame: 0),
                    player: (sheet: "p", spawn: (200, 200)),
                    places: [(scene: "scenes/hall.ron", at: ({}, {}))],
                )"#,
                at.0, at.1
            ),
        )
        .unwrap();
        let project = Project::open(dir.clone()).unwrap();
        let maps = Maps::load(&project, "scenes/world.ron").unwrap();
        let map = &maps.maps[0];
        let level_at = |col: i64, row: i64| {
            map.collision
                .terrain
                .cell(col, row)
                .and_then(dark_physics::Cell::level)
        };

        // Under the hall the ground is one level, whatever the land put there — water included,
        // or a camp would be half in a lake.
        let sits = level_at(spot.0 + side / 2, spot.1 + side / 2)
            .unwrap_or_else(|| panic!("{what}: the hall is not dry ground"));
        let (mut changed, mut was_water) = (0, 0);
        for row in 0..side {
            for col in 0..side {
                let (col, row) = (spot.0 + col, spot.1 + row);
                assert_eq!(
                    level_at(col, row),
                    Some(sits),
                    "{what}: the hall is not level"
                );
                let under = land.cell(col, row).level();
                changed += usize::from(under != Some(sits));
                was_water += usize::from(under.is_none());
            }
        }
        assert!(
            changed > 0,
            "{what}: the land was already level here, so the levelling is untested"
        );
        if wanted == Wanted::WaterMadeGround {
            assert!(
                was_water > 0,
                "{what}: no water was under the hall, so that half is untested"
            );
        }

        // And nowhere from well inside it to well outside does the ground go more than one step
        // between neighbouring tiles.
        let apron = 6;
        let mut stepped = 0;
        for row in (spot.1 - apron)..(spot.1 + side + apron) {
            for col in (spot.0 - apron)..(spot.0 + side + apron) {
                let Some(here) = level_at(col, row) else {
                    continue;
                };
                for (across, down) in [(1, 0), (0, 1)] {
                    let Some(next) = level_at(col + across, row + down) else {
                        continue;
                    };
                    assert!(
                        here.abs_diff(next) <= 1,
                        "{what}: the ground goes {} steps at once between ({col}, {row}) and ({}, {})",
                        here.abs_diff(next),
                        col + across,
                        row + down
                    );
                    stepped += usize::from(here != next);
                }
            }
        }
        if wanted == Wanted::GroundThatSteps {
            assert!(
                stepped > 0,
                "{what}: the ground never changed level at all, so the stepping is untested"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What grows is worked out, not remembered: the same seed and the same patch give the same
    /// wood, on a second asking and on a second machine.
    ///
    /// This is the whole bargain of a made world. A host and a client that grew different woods
    /// would have players walking through trees that are not there.
    #[test]
    fn the_same_seed_grows_the_same_wood_everywhere() {
        let (dir, host) = a_made_map("same", 20_260_923);
        let (dir2, client) = a_made_map("same2", 20_260_923);
        let (map, other) = (&host.maps[0], &client.maps[0]);
        let mut found = 0;
        for patch in 0..8 {
            let (col, row) = (9_000 / 16 + patch * PATCH, 9_000 / 16 + patch * PATCH);
            let grown = map.grown_in_patch(col, row);
            assert_eq!(grown, map.grown_in_patch(col, row), "asked twice");
            assert_eq!(grown, other.grown_in_patch(col, row), "on another machine");
            found += grown.len();
        }
        assert!(found > 20, "only {found} things grew in eight patches");
        for dir in [dir, dir2] {
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// Nothing grows in water, nothing grows on ground its group does not like, and nothing grows
    /// where the scene puts somebody — with room for its own footprint, or a player would begin
    /// the game inside a tree trunk.
    #[test]
    fn nothing_grows_in_water_or_where_people_stand() {
        let (dir, maps) = a_made_map("where", 20_260_923);
        let map = &maps.maps[0];
        let land = map.land.unwrap();
        let spawn = Vec2::new(9_000.0, 9_000.0);
        let mut looked = 0;
        // A good sweep of the country around the start, patches at a time.
        for down in -6..6 {
            for across in -6..6 {
                let (col, row) = (
                    (spawn.x / 16.0) as i64 + across * PATCH,
                    (spawn.y / 16.0) as i64 + down * PATCH,
                );
                for one in map.grown_in_patch(col, row) {
                    looked += 1;
                    let (col, row) = map.collision.terrain.tile_of(one.at);
                    let cell = land.cell(col, row);
                    assert!(cell.level().is_some(), "{:?} grew in water", one.at);
                    let def = &map.def.scatter[one.group];
                    if let Some(levels) = &def.levels {
                        assert!(
                            levels.contains(&cell.level().unwrap()),
                            "group {} grew on level {:?}",
                            one.group,
                            cell.level()
                        );
                    }
                    let reach = def.collider.map_or(0.0, |c| footprint(c.shape));
                    assert!(
                        one.at.distance(spawn) >= crate::SPAWN_CLEARING + reach,
                        "{:?} grew {} px from the start, too near for its {reach} px footprint",
                        one.at,
                        one.at.distance(spawn)
                    );
                }
            }
        }
        assert!(looked > 100, "only {looked} things to look at");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two things of a group never stand closer than the spacing it asks for. Spacing is kept by
    /// cutting the world into cells and growing at most one thing in each, so this is what says
    /// that the cutting is right.
    #[test]
    fn what_grows_keeps_its_distance() {
        let (dir, maps) = a_made_map("spacing", 20_260_923);
        let map = &maps.maps[0];
        let mut standing: Vec<Vec<Vec2>> = vec![Vec::new(); map.def.scatter.len()];
        // Around the middle of the map, where this seed makes dry ground: a corner that is all
        // water would grow nothing and prove nothing.
        for down in 8..12 {
            for across in 8..12 {
                for one in map.grown_in_patch(across * PATCH, down * PATCH) {
                    standing[one.group].push(one.at);
                }
            }
        }
        for (group, def) in map.def.scatter.iter().enumerate() {
            let places = &standing[group];
            assert!(!places.is_empty(), "nothing grew in group {group}");
            for (n, a) in places.iter().enumerate() {
                for b in &places[n + 1..] {
                    assert!(
                        a.distance(*b) >= def.min_spacing,
                        "group {group}: {a} and {b} are {} apart, closer than {}",
                        a.distance(*b),
                        def.min_spacing
                    );
                }
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What grows comes and goes with the land it grows on: its footprints are in the collision
    /// world while the patch is, gone when the patch is let go of, and the same again when a
    /// player walks back.
    #[test]
    fn a_wood_comes_and_goes_with_its_land() {
        let (dir, mut maps) = a_made_map("coming", 20_260_923);
        let map = &mut maps.maps[0];
        let at = Vec2::new(9_000.0, 9_000.0);
        map.make_around(at);
        let planted = map.collision.colliders().count();
        assert!(planted > 0, "nothing grew where the player stands");
        let patches = map.grown.len();
        assert!(patches > 0, "no patch remembers what grew on it");

        // Walking far enough away that the land behind is let go of, through the same door the
        // game uses — a map that has walked and come back must end up where a map that never
        // left already is.
        let far = at + Vec2::new(200_000.0, 0.0);
        map.make_around(far);
        map.forget_far_from(&[far]);
        assert!(
            map.grown.len() < patches + 25,
            "the wood behind is still held: {} patches",
            map.grown.len()
        );
        assert!(
            map.collision.colliders().count() < planted + 25,
            "the footprints behind are still held"
        );

        // And walking back grows the same wood again, footprint for footprint, as a map that had
        // been standing there all along.
        map.make_around(at);
        let (dir2, mut fresh) = a_made_map("coming2", 20_260_923);
        let never_left = &mut fresh.maps[0];
        never_left.make_around(at);
        let footprints = |map: &crate::maps::Map| {
            let mut all: Vec<_> = map
                .collision
                .colliders()
                .map(|(_, c)| (c.center.x.to_bits(), c.center.y.to_bits(), c.base.to_bits()))
                .collect();
            all.sort_unstable();
            all
        };
        assert_eq!(
            footprints(map),
            footprints(never_left),
            "the wood came back different from the one that was never left"
        );
        for dir in [dir, dir2] {
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// Everything drawn on made land has a footprint in the collision world, and nothing else
    /// does: the two are worked out by different code — a pure function for the drawing, a side
    /// effect for the collision — and the moment they disagree a player walks through a tree.
    ///
    /// This is the assertion that would have caught the shore search making land it never grew
    /// on, which left every moved start inside a wood with no collision in it.
    #[test]
    fn what_is_drawn_on_made_land_is_what_can_be_walked_into() {
        // A seed and a start that land in water, so the start is moved and the ground around the
        // shore is made by a different path from the walking.
        let (dir, mut maps) = a_made_map("agree", 20_260_923);
        let map = &mut maps.maps[0];
        let start = Vec2::from(map.def.player.as_ref().unwrap().spawn);
        map.make_around(start + Vec2::new(3_000.0, 1_000.0));
        map.make_around(start);

        // Every patch that has been made: what grows there, against what is held there.
        let tile = map.collision.terrain.tile();
        let mut looked = 0;
        for (patch, held) in map.grown.clone() {
            let grown = map.grown_in_patch(patch.0, patch.1);
            let with_footprints = grown
                .iter()
                .filter(|one| map.def.scatter[one.group].collider.is_some())
                .count();
            assert_eq!(
                held.len(),
                with_footprints,
                "patch {patch:?} holds {} footprints for {with_footprints} things",
                held.len()
            );
            for one in &grown {
                let Some(def) = map.def.scatter.get(one.group) else {
                    continue;
                };
                let Some(should) = one.collider(def, map.ground_of(one.at)) else {
                    continue;
                };
                assert!(
                    held.iter()
                        .any(|nth| map.collision.collider(*nth) == Some(&should)),
                    "nothing can be walked into where {:?} is drawn",
                    one.at
                );
                looked += 1;
            }
        }
        assert!(looked > 50, "only {looked} things to look at");

        // And every patch made has an entry: a patch made by any path at all grows.
        let (cols, rows) = (
            map.collision.terrain.cols() as i64,
            map.collision.terrain.rows() as i64,
        );
        let mut made = 0;
        for row in (0..rows).step_by(PATCH as usize) {
            for col in (0..cols).step_by(PATCH as usize) {
                if !map.collision.terrain.is_made(col as u32, row as u32) {
                    continue;
                }
                made += 1;
                let grown = map.grown_in_patch(col, row);
                let wants: Vec<_> = grown
                    .iter()
                    .filter(|one| map.def.scatter[one.group].collider.is_some())
                    .collect();
                assert_eq!(
                    map.grown.get(&(col, row)).map_or(0, Vec::len),
                    wants.len(),
                    "patch ({col}, {row}) is made but {} of its wood is not held",
                    wants.len()
                );
            }
        }
        assert!(made > 20, "only {made} patches were made");
        let _ = tile;
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
