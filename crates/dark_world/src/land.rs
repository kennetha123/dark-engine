//! Shaping the land around the players (docs/PLAN.md §24.4).
//!
//! A world of ten or sixteen kilometres is not written down; it is made from the seed its scene
//! carries, a patch of tiles at a time, as somebody walks towards it. This keeps the patches
//! around every player shaped, and lets go of the ones nobody is near once a map holds more than
//! it keeps — the same seed makes the same patch again, so a player walking back finds the hill
//! they walked over either way.
//!
//! The same patches are made the same way on the host and on every client, because `dark_land`
//! works in whole numbers — a client that disagreed about the land would predict its own player
//! falling through a rock the host says is there.

use bevy_ecs::prelude::*;
use dark_land::Land;
use dark_physics::Terrain;
use glam::Vec2;

use crate::characters::PlayerAvatar;
use crate::maps::{BodyState, MapId, Maps};

/// How far beyond a player the land is shaped, in patches. A patch is 64 tiles, so two patches
/// is 128 tiles: at 16 px to the tile that is 2 048 px, three screenfuls of the first game, and
/// far enough that a piece of the map is never drawn before its land exists.
const AHEAD: i64 = 2;

/// How many made patches a map holds before it starts letting go of the ones nobody is near.
/// A patch is 8 KB, so this is 32 MB of ground a map: more than a session of walking makes, and
/// far less than a long evening of it would (docs/PLAN.md §24.4).
const KEPT: usize = 4_096;

/// How far around a player made land is kept when land is let go of, in patches. Eight patches is
/// 512 tiles, four screenfuls of the first game, so what is let go of is always well out of sight
/// and well beyond where the next few minutes of walking will reach.
const KEEP_NEAR: i64 = 8;

/// Shapes the patches around each player that have not been shaped yet, and lets go of made land
/// that no player is near once a map holds more than [`KEPT`] patches of it. A map drawn by hand
/// has no land of its own and is left alone.
pub fn shape_around_players(
    mut maps: ResMut<Maps>,
    players: Query<(&MapId, &BodyState), With<PlayerAvatar>>,
) {
    for (map, body) in &players {
        let Some(map) = maps.maps.get_mut(map.0 as usize) else {
            continue;
        };
        map.make_around(body.0.position);
    }
    // Then the other way about: made land nobody is near is let go of. The same seed makes the
    // same patch again, so nothing is lost by it.
    let standing: Vec<(u16, Vec2)> = players
        .iter()
        .map(|(map, body)| (map.0, body.0.position))
        .collect();
    for (nth, map) in maps.maps.iter_mut().enumerate() {
        if map.land.is_none() {
            continue;
        }
        let here: Vec<Vec2> = standing
            .iter()
            .filter(|(on, _)| *on as usize == nth)
            .map(|(_, at)| *at)
            .collect();
        map.forget_far_from(&here);
    }
}

impl crate::maps::Map {
    /// Makes the land around a place, and grows what stands on it: the patches within [`AHEAD`]
    /// that have not been made yet. A map drawn by hand has no land of its own and is left alone.
    ///
    /// The two go together on purpose. Ground that exists with nothing on it, waiting for a
    /// second pass to plant the trees, is ground a player can walk through a wood on.
    pub fn make_around(&mut self, at: Vec2) {
        if self.land.is_none() {
            return;
        }
        let tile = self.collision.terrain.tile();
        let patch = i64::from(Terrain::PATCH);
        let (col, row) = self.collision.terrain.tile_of(at);
        let (patch_col, patch_row) = (col.div_euclid(patch), row.div_euclid(patch));
        for down in -AHEAD..=AHEAD {
            for across in -AHEAD..=AHEAD {
                let (c, r) = ((patch_col + across) * patch, (patch_row + down) * patch);
                if c < 0 || r < 0 {
                    continue;
                }
                self.make_patch(Vec2::new(c as f32, r as f32) * tile);
            }
        }
    }

    /// Makes one patch of land — the one holding `at` — and grows what stands on it, if it has
    /// not been made already. Says whether it made anything.
    ///
    /// The game makes land around the people walking on it ([`Self::make_around`]); an editor
    /// makes the land it is *looking* at, which is a different shape of question. Both come
    /// through here, so neither can make ground without planting on it (§24.5).
    pub fn make_patch(&mut self, at: Vec2) -> bool {
        let Some(land) = self.land else {
            return false;
        };
        let (col, row) = self.collision.terrain.tile_of(at);
        let patch = i64::from(Terrain::PATCH);
        let (c, r) = (col.div_euclid(patch) * patch, row.div_euclid(patch) * patch);
        let (Ok(uc), Ok(ur)) = (u32::try_from(c), u32::try_from(r)) else {
            return false;
        };
        if self.collision.terrain.is_made(uc, ur) {
            return false;
        }
        self.collision
            .terrain
            .shape(uc, ur, |col, row| land.cell(col, row));
        let grown = self.grow_patch(c, r);
        if !grown.is_empty() {
            self.grown.insert((c, r), grown);
        }
        true
    }

    /// Lets go of the made land none of `standing` is near, and of everything that grew on it, on
    /// the terms the whole game uses: a host does this for every player on a map, and a client
    /// for the one player it is predicting.
    ///
    /// A map with nobody on it keeps nothing, which is what a map a party has left should cost.
    pub fn forget_far_from(&mut self, standing: &[Vec2]) {
        if self.land.is_none() {
            return;
        }
        let near: Vec<(i64, i64)> = standing
            .iter()
            .map(|at| self.collision.terrain.tile_of(*at))
            .collect();
        for (col, row) in self.collision.terrain.forget_far(&near, KEEP_NEAR, KEPT) {
            let Some(held) = self.grown.remove(&(i64::from(col), i64::from(row))) else {
                continue;
            };
            for nth in held {
                self.collision.remove(nth);
            }
        }
    }
}

/// Shapes the patches within [`AHEAD`] of a place — the ground alone.
///
/// The game never calls this: it calls [`crate::maps::Map::make_around`], which makes the land
/// *and* grows what stands on it. A patch is only ever made once, so ground made here would hold
/// no trees for the rest of the session while the drawing showed them (§24.5). This is left for
/// the tests that are about the ground itself, and for nothing else.
#[cfg(test)]
fn shape_around(terrain: &mut Terrain, land: &Land, at: Vec2) {
    let patch = i64::from(Terrain::PATCH);
    let (col, row) = terrain.tile_of(at);
    let (patch_col, patch_row) = (col.div_euclid(patch), row.div_euclid(patch));
    for down in -AHEAD..=AHEAD {
        for across in -AHEAD..=AHEAD {
            let (c, r) = ((patch_col + across) * patch, (patch_row + down) * patch);
            if c < 0 || r < 0 {
                continue;
            }
            let (Ok(c), Ok(r)) = (u32::try_from(c), u32::try_from(r)) else {
                continue;
            };
            if !terrain.is_made(c, r) {
                terrain.shape(c, r, |col, row| land.cell(col, row));
            }
        }
    }
}

/// How far from where it was asked for somewhere to stand may be looked for, in tiles. Water this
/// land makes is as broad as the lattice that made it — `dark_land::COARSE` tiles — so this
/// reaches across two of them, and past anything short of a sea.
const LOOK_WITHIN: i64 = 2 * dark_land::COARSE;

/// The eight ways out of a place. A shore is found by walking each of them a tile at a time,
/// rather than by asking about every tile in a ring: what this land makes of water is a lake,
/// never a needle, so a ray meets its edge.
const WAYS: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Somewhere near `at` that a body of `radius` can stand: `at` itself where the land allows it,
/// else the nearest place on this map that does.
///
/// Made land has no say in where a scene puts people, so a spawn or a doorway can land in a lake
/// (docs/PLAN.md §24.4). Refusing to load the map would be honest and useless; this finds the
/// shore instead.
///
/// **This changes nothing.** It asks [`Land`] about tiles rather than making them, so a search
/// across two lattices costs no ground and, more to the point, leaves no patch made. Making a
/// patch means growing on it too (§24.5), and a patch is only ever made once — a search that
/// made land as it went would leave a wood with no collision in it behind every shore it looked
/// at, and would plant that wood around the *old* place before the new one was chosen.
///
/// `None` if nowhere within [`LOOK_WITHIN`] and on the map can be stood on, and then the map is
/// refused as before: a scene whose every way out opens into a sea is a scene to fix.
pub fn dry_ground_near(
    world: &dark_physics::World,
    land: &Land,
    at: Vec2,
    radius: f32,
) -> Option<Vec2> {
    let tile = world.terrain.tile();
    // How many tiles either side of its own a body of this radius stands on.
    let spread = (radius / tile).ceil() as i64;
    // What the ground under a body is, without shaping any of it: a tile already made or drawn by
    // hand is whatever the terrain holds, and one the land has not reached yet is what the seed
    // will make of it. Nothing standing on it, either — props are in the grid, not the ground.
    let stands = |world: &dark_physics::World, col: i64, row: i64| {
        let clear = (-spread..=spread).all(|down| {
            (-spread..=spread).all(|across| {
                let (col, row) = (col + across, row + down);
                let made = match u32::try_from(col).ok().zip(u32::try_from(row).ok()) {
                    Some((c, r)) => world.terrain.is_made(c, r),
                    None => false,
                };
                let cell = if made || world.terrain.drawn_by_hand(col, row) {
                    world.terrain.cell(col, row)
                } else {
                    Some(land.cell(col, row))
                };
                cell.is_some_and(|cell| cell.level().is_some())
            })
        });
        let middle = (Vec2::new(col as f32, row as f32) + 0.5) * tile;
        clear && !world.overlapping(middle, radius).any(|c| c.height > 0.0)
    };
    let (from_col, from_row) = world.terrain.tile_of(at);
    if stands(world, from_col, from_row) {
        return Some(at);
    }
    let (cols, rows) = (
        i64::from(world.terrain.cols()),
        i64::from(world.terrain.rows()),
    );
    for out in 1..=LOOK_WITHIN {
        for (across, down) in WAYS {
            let (col, row) = (from_col + across * out, from_row + down * out);
            // Off the map is nowhere to stand, however dry the land there would be.
            if !(spread..cols - spread).contains(&col) || !(spread..rows - spread).contains(&row) {
                continue;
            }
            if stands(world, col, row) {
                return Some((Vec2::new(col as f32, row as f32) + 0.5) * tile);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_physics::Cell;

    /// A scene that asks to be made, loaded as the game loads it: the map carries its own land,
    /// the ground under everywhere the scene puts somebody is made before those places are
    /// checked, and what the scene drew by hand is still there.
    ///
    /// Without this nothing covered the way a seed in a scene file reaches the ground a player
    /// stands on, which is how a map with two seeds and unchecked arrivals went unnoticed.
    #[test]
    fn a_scene_that_asks_to_be_made_is_made_where_people_stand() {
        use dark_assets::Project;

        let dir = std::env::temp_dir().join(format!("dark_world_made_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        // A made map with a hand-drawn block on it, an enemy standing well away from the spawn,
        // and a way out to a second made map with a seed of its own. Seed 1 makes dry plain where
        // this scene puts people, so nothing here is moved; the moving has a test of its own.
        std::fs::write(
            dir.join("scenes/made.ron"),
            r#"(
                size: (8000, 8000),
                land: (seed: 1),
                ground: (sheet: "g", frame: 0),
                player: (sheet: "p", spawn: (1200, 1200)),
                terrain: (fill: [(tiles: (10, 10, 4, 4), cell: Level(2))]),
                enemies: [(kind: "goblin", position: (6000, 6000))],
                npcs: [(sheet: "n", position: (3000, 5000), lines: [],
                        day: [(from: 8, at: (5000, 3000))])],
                inns: [(area: (2000, 6000, 400, 400), bed: (2200, 6200))],
                exits: [(area: (7900, 0, 100, 8000), to: "scenes/far.ron", spawn: (4000, 4000))],
            )"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("scenes/far.ron"),
            r#"(
                size: (8000, 8000),
                land: (seed: 999),
                ground: (sheet: "g", frame: 0),
            )"#,
        )
        .unwrap();
        let project = Project::open(dir.clone()).unwrap();
        let maps = Maps::load(&project, "scenes/made.ron").unwrap();

        // Each map is made from its own seed, not from the first one's.
        let (made, far) = (&maps.maps[0], &maps.maps[1]);
        assert_eq!(made.land, Some(Land::new(1)));
        assert_eq!(far.land, Some(Land::new(999)));

        // Every place this scene puts somebody has had its land made before anybody was put on
        // it: the start, the enemy in the far corner, the villager's post and where their day
        // sends them, and the inn's bed. This asks whether the patch was *made* rather than what
        // the tile holds, because unmade ground reads as level and would agree with a seed that
        // makes level ground there — which is how this test passed while making nothing at all.
        let ground = |map: &crate::maps::Map, at: Vec2| {
            let (col, row) = map.collision.terrain.tile_of(at);
            map.collision.terrain.is_made(col as u32, row as u32)
        };
        for at in [
            Vec2::new(1200.0, 1200.0),
            Vec2::new(6000.0, 6000.0),
            Vec2::new(3000.0, 5000.0),
            Vec2::new(5000.0, 3000.0),
            Vec2::new(2200.0, 6200.0),
        ] {
            assert!(ground(made, at), "the land at {at} was never made");
            let (col, row) = made.collision.terrain.tile_of(at);
            assert_eq!(
                made.collision.terrain.cell(col, row),
                Some(made.land.unwrap().cell(col, row)),
                "the ground at {at} is not the ground the seed makes"
            );
        }
        // And nowhere else: a map is not made all over just because it is made where people are.
        assert!(
            !ground(made, Vec2::new(7900.0, 200.0)),
            "the far corner of the map was made, which is the whole cost this avoids"
        );
        let arrival = Vec2::new(4000.0, 4000.0);
        assert!(ground(far, arrival), "the arrival's land was never made");
        let (col, row) = far.collision.terrain.tile_of(arrival);
        assert_eq!(
            far.collision.terrain.cell(col, row),
            Some(Land::new(999).cell(col, row)),
            "the other map's arrival is made from the other map's seed"
        );
        let tile = made.collision.terrain.tile();

        // And the block the scene drew by hand is still a block.
        let drawn = Vec2::new(11.0 * tile, 11.0 * tile);
        let (col, row) = made.collision.terrain.tile_of(drawn);
        assert_eq!(made.collision.terrain.cell(col, row), Some(Cell::Level(2)));
        // This land is dry where the scene puts people, so nobody was moved.
        assert_eq!(made.def.player.as_ref().unwrap().spawn, (1200.0, 1200.0));
        assert_eq!(made.exits[0].spawn, Vec2::new(4000.0, 4000.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A scene whose start and whose way out land in the water of the land it asks for is loaded
    /// all the same: both are moved to ground somebody can stand on.
    ///
    /// Before this, the map refused to load — honest, and no use to anyone playing.
    #[test]
    fn a_start_and_an_arrival_in_water_are_moved_ashore() {
        use dark_assets::Project;

        let dir = std::env::temp_dir().join(format!("dark_world_lake_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        // Seed 20260923 makes a lake of this whole corner, which is how it was found.
        for (scene, body) in [
            (
                "scenes/wet.ron",
                r#"player: (sheet: "p", spawn: (1200, 1200)),
                exits: [(area: (31900, 0, 100, 32000), to: "scenes/other.ron", spawn: (1200, 1200))],"#,
            ),
            ("scenes/other.ron", ""),
        ] {
            std::fs::write(
                dir.join(scene),
                format!(
                    r#"(
                    size: (32000, 32000),
                    land: (seed: 20260923),
                    ground: (sheet: "g", frame: 0),
                    {body}
                )"#
                ),
            )
            .unwrap();
        }
        let project = Project::open(dir.clone()).unwrap();
        let maps = Maps::load(&project, "scenes/wet.ron").unwrap();

        let wet = &maps.maps[0];
        let start = Vec2::from(wet.def.player.as_ref().unwrap().spawn);
        assert_ne!(start, Vec2::new(1200.0, 1200.0), "the start was in a lake");
        assert!(
            wet.collision.ground_under(start, 6.0).is_finite(),
            "the start at {start} is still not dry"
        );
        // And the arrival, which lives in the map it leaves from but must be dry in the one it
        // leads to.
        let arrival = wet.exits[0].spawn;
        assert_ne!(arrival, Vec2::new(1200.0, 1200.0));
        assert_eq!(arrival, Vec2::from(wet.def.exits[0].spawn));
        assert!(
            maps.maps[1]
                .collision
                .ground_under(arrival, 6.0)
                .is_finite(),
            "a way out arrives at {arrival}, which is under water"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A place a scene asks for that the land made into a lake is found dry ground near it,
    /// rather than the map refusing to load.
    #[test]
    fn a_place_in_a_lake_is_given_dry_ground() {
        let land = Land::new(20_260_923);
        let mut world = dark_physics::World::new(Terrain::new(2_000, 2_000, 16.0, 16.0));
        // This one is under water for this seed, which is how it was found.
        let wanted = Vec2::new(1_200.0, 1_200.0);
        shape_around(&mut world.terrain, &land, wanted);
        assert!(
            !world.ground_under(wanted, 6.0).is_finite(),
            "{wanted} was meant to be water"
        );

        let found = dry_ground_near(&world, &land, wanted, 6.0).expect("a shore somewhere");
        assert!(
            world.ground_under(found, 6.0).is_finite(),
            "somewhere to stand"
        );
        assert!(
            found.distance(wanted) < LOOK_WITHIN as f32 * 16.0,
            "the shore found is {} px away",
            found.distance(wanted)
        );
        // Ground that can already be stood on is left exactly where it was.
        assert_eq!(dry_ground_near(&world, &land, found, 6.0), Some(found));
    }

    /// Walking shapes the land ahead, and the land already walked is left as it was made.
    #[test]
    fn the_land_is_shaped_around_a_walker_and_never_again() {
        let land = Land::new(11);
        let mut terrain = Terrain::new(4_000, 4_000, 16.0, 16.0);
        let at = Vec2::new(16_000.0, 16_000.0);
        shape_around(&mut terrain, &land, at);
        // Five patches each way around the one the walker is in.
        assert_eq!(terrain.shaped(), 25);
        let underfoot = terrain.tile_of(at);
        let made = terrain.cell(underfoot.0, underfoot.1);
        assert_eq!(made, Some(land.cell(underfoot.0, underfoot.1)));

        // A step further on shapes what is newly ahead and leaves the rest alone.
        shape_around(&mut terrain, &land, at + Vec2::new(64.0 * 16.0, 0.0));
        assert_eq!(terrain.shaped(), 30, "one more column of patches");
        assert_eq!(
            terrain.cell(underfoot.0, underfoot.1),
            made,
            "as it was made"
        );
    }

    /// The edges of the map are not shaped past, and a walker near them is fine.
    #[test]
    fn the_far_corner_of_a_map_is_shaped_without_running_off_it() {
        let land = Land::new(5);
        let mut terrain = Terrain::new(100, 100, 16.0, 16.0);
        shape_around(&mut terrain, &land, Vec2::new(99.0 * 16.0, 99.0 * 16.0));
        assert!(terrain.shaped() > 0, "the corner is shaped");
        assert_eq!(terrain.cell(99, 99), Some(land.cell(99, 99)));
        // Nothing beyond the map exists, shaped or not.
        assert_eq!(terrain.cell(100, 99), None);
    }

    /// A walk long enough to make more land than a map keeps lets go of the land behind it, and
    /// the land it comes back to is the land it left.
    ///
    /// Without this a map only ever grew: an evening of four players running would have held a few
    /// hundred megabytes of ground nobody was standing on, against §24's own rule.
    #[test]
    fn land_nobody_is_near_is_let_go_of_and_comes_back_the_same() {
        let land = Land::new(20_260_923);
        let mut terrain = Terrain::new(20_000, 20_000, 16.0, 16.0);
        // A place worth remembering, drawn by hand at the start of the walk.
        terrain.fill(10, 10, 2, 2, Cell::Level(3));
        let tile = terrain.tile();
        // Three lengths of a twenty-thousand-tile map, a patch at a stride and well apart: some
        // four thousand seven hundred patches of made land, past what a map keeps.
        let patch = i64::from(Terrain::PATCH);
        let mut walked = Vec::new();
        for band in 0..3 {
            for step in 0..(20_000 / patch) {
                let across = if band % 2 == 0 {
                    step
                } else {
                    20_000 / patch - step
                };
                let (col, row) = (across * patch, 100 + band * 3_000);
                walked.push(Vec2::new(col as f32, row as f32) * tile);
            }
        }
        for at in &walked {
            shape_around(&mut terrain, &land, *at);
            let (col, row) = terrain.tile_of(*at);
            terrain.forget_far(&[(col, row)], KEEP_NEAR, KEPT);
        }
        assert!(
            terrain.shaped() <= KEPT + 25,
            "the walk is holding {} patches",
            terrain.shaped()
        );
        // What was drawn by hand is still there, however far behind it is left.
        assert_eq!(terrain.cell(10, 10), Some(Cell::Level(3)));

        // And walking back gives the same ground, tile for tile, as walking out did.
        for at in walked.iter().rev() {
            shape_around(&mut terrain, &land, *at);
            let (col, row) = terrain.tile_of(*at);
            assert_eq!(
                terrain.cell(col, row),
                Some(land.cell(col, row)),
                "the ground at {at} came back different"
            );
        }
    }

    /// Land drawn by hand wins, tile by tile: a scene's own fills stay exactly as drawn, and the
    /// land the seed makes fills in around them — including inside the same patch.
    ///
    /// Before this, one drawn tile counted its whole 64×64 patch as shaped, so a single hand-drawn
    /// rock left four thousand tiles of a made world flat and empty around it.
    ///
    /// `Floor` says nothing — every map is filled with it by the acre, and a made world would be
    /// wiped flat by one such fill — so levelling by hand is `Level(0)`, which stands at the same
    /// height and is an opinion. Both are checked here, because getting that the wrong way round
    /// deletes a country without a word.
    #[test]
    fn what_was_drawn_by_hand_is_not_overwritten() {
        // This seed makes water of this corner, so the drawing stands out from the land around it.
        let land = Land::new(20_260_923);
        let mut terrain = Terrain::new(200, 200, 16.0, 16.0);
        // The base fill every map starts with, which must not count as levelling anything.
        terrain.fill(0, 0, 200, 200, Cell::Floor);
        terrain.fill(100, 100, 4, 4, Cell::Level(3));
        // Levelled by hand, which is how a made world is made room in for something built on it.
        terrain.fill(104, 100, 4, 4, Cell::Level(0));
        shape_around(&mut terrain, &land, Vec2::new(100.0 * 16.0, 100.0 * 16.0));
        assert_eq!(terrain.cell(101, 101), Some(Cell::Level(3)), "drawn");
        assert_eq!(terrain.cell(105, 101), Some(Cell::Level(0)), "levelled");
        // A tile the land makes differently from the drawing proves the made land got in.
        let mut found = 0;
        for (col, row) in [(110, 110), (127, 64), (64, 127), (70, 120), (99, 99)] {
            assert_eq!(
                terrain.cell(col, row),
                Some(land.cell(col, row)),
                "({col}, {row}) is not the land the seed makes"
            );
            found += usize::from(land.cell(col, row) != Cell::Level(3));
        }
        assert!(found > 0, "nothing around the drawing differs from it");
    }
}
