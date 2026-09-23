//! Finding a way on foot for characters the world moves (enemies now, NPCs later): who can walk
//! where without jumping, whether a straight line is clear, and A* over the tile grid when not.
//!
//! Searching has a budget (docs/PLAN.md §24.3). Without one, a way that does not exist costs the
//! whole map — every tile of it, with the props under each asked about — and on a large map that
//! is a frozen host. A walker that runs out of budget gives up and does what it does when there
//! is no way at all, which it must already handle.

use dark_physics::{MoveParams, World};
use glam::Vec2;

/// Whether a footprint at `center` can stand there at `elevation` without climbing or dropping
/// a level, and clear of every prop taller than a step.
pub fn walkable(
    world: &World,
    center: Vec2,
    radius: f32,
    elevation: f32,
    params: &MoveParams,
) -> bool {
    let (lo, hi) = world.ground_span(center, radius);
    let flat = hi <= elevation + params.step_up && lo >= elevation - params.step_up;
    flat && !world.overlapping(center, radius).any(|c| {
        c.base + c.height > elevation + params.step_up && c.base <= elevation + params.step_up
    })
}

/// Whether a footprint can walk the straight line from `from` to `to`.
pub fn clear_line(
    world: &World,
    from: Vec2,
    to: Vec2,
    radius: f32,
    elevation: f32,
    params: &MoveParams,
) -> bool {
    let steps = (from.distance(to) / 4.0).ceil().max(1.0) as u32;
    (1..=steps).all(|i| {
        let p = from.lerp(to, i as f32 / steps as f32);
        walkable(world, p, radius, elevation, params)
    })
}

/// A walkable way from `from` to `to`: points to head for in turn, the last being `to` itself.
/// How many tiles a search may take from its queue before giving up. Each costs up to sixteen
/// questions of the map, so this is the real work, not a pure count of steps.
///
/// It is set above the whole tile count of any map the first game has (the largest is 7 500),
/// so on today's maps it can never refuse a way that exists — a search that runs out has already
/// looked at more tiles than the map holds. It matters on a map far larger than those: a wall
/// with a gap at the far end costs about thirty thousand tiles to go round, so a walker there
/// gives up and makes for the target directly. Searching chunk by chunk, with a coarse graph of
/// the ways between them, is §24.4's answer; this is the bound until then.
const BUDGET: usize = 12_000;

/// A* over tile centres (8-way, no cutting corners), then shortened wherever a straight line is
/// clear. `None` if there is no way, and also if finding one would cost more than [`BUDGET`]
/// tiles — a walker cannot tell those apart, and treats both as "make for it directly". A way
/// found after the budget ran out may not be the shortest one; it is still a way, and what uses
/// it is an NPC deciding where to put its feet.
pub fn find_path(
    world: &World,
    from: Vec2,
    to: Vec2,
    radius: f32,
    elevation: f32,
    params: &MoveParams,
) -> Option<Vec<Vec2>> {
    way(world, from, to, radius, elevation, params).0
}

/// The way, and how many tiles were looked at finding it or failing to — which is what the
/// budget holds down, and what a test can check.
fn way(
    world: &World,
    from: Vec2,
    to: Vec2,
    radius: f32,
    elevation: f32,
    params: &MoveParams,
) -> (Option<Vec<Vec2>>, usize) {
    let terrain = &world.terrain;
    let tile = terrain.tile();
    let (cols, rows) = (i64::from(terrain.cols()), i64::from(terrain.rows()));
    let center = |(c, r): (i64, i64)| Vec2::new((c as f32 + 0.5) * tile, (r as f32 + 0.5) * tile);
    let open = |(c, r): (i64, i64)| {
        (0..cols).contains(&c)
            && (0..rows).contains(&r)
            && walkable(world, center((c, r)), radius, elevation, params)
    };
    let start = terrain.tile_of(from);
    // A goal on an unwalkable tile (a target beside a prop) aims for the nearest open tile.
    let wanted = terrain.tile_of(to);
    let goal = if open(wanted) {
        wanted
    } else {
        let near = (1..=2i64)
            .flat_map(|r| {
                (-r..=r).flat_map(move |dc| (-r..=r).map(move |dr| (wanted.0 + dc, wanted.1 + dr)))
            })
            .filter(|&t| open(t))
            .min_by(|&a, &b| center(a).distance(to).total_cmp(&center(b).distance(to)));
        match near {
            Some(goal) => goal,
            None => return (None, 0),
        }
    };
    // What a search has looked at so far. A way that does not exist would otherwise be paid for
    // with the whole map.
    let looked = std::cell::Cell::new(0usize);
    let (tiles, _) = match pathfinding::prelude::astar(
        &start,
        |&(c, r)| {
            let mut next = Vec::with_capacity(8);
            if looked.get() >= BUDGET {
                // Nowhere to go from here: the search runs out and finds nothing. Whatever it
                // had already queued is still asked, and each of those is turned away here.
                return next;
            }
            looked.set(looked.get() + 1);
            for (dc, dr) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let n = (c + dc, r + dr);
                let diagonal = dc != 0 && dr != 0;
                let ok = open(n) && (!diagonal || (open((c + dc, r)) && open((c, r + dr))));
                if ok {
                    next.push((n, if diagonal { 14u32 } else { 10 }));
                }
            }
            next
        },
        |&(c, r)| {
            let (dx, dy) = ((c - goal.0).unsigned_abs(), (r - goal.1).unsigned_abs());
            (10 * dx.max(dy) + 4 * dx.min(dy)) as u32
        },
        |&n| n == goal,
    ) {
        Some(found) => found,
        None => return (None, looked.get()),
    };
    // Tile centres after the start, ending exactly at `to`; then drop every point a straight
    // line can skip.
    let mut points: Vec<Vec2> = tiles.iter().skip(1).map(|&t| center(t)).collect();
    if let Some(last) = points.last_mut() {
        *last = to;
    } else {
        points.push(to);
    }
    let mut path = Vec::new();
    let mut at = from;
    let mut i = 0;
    while i < points.len() {
        let mut j = points.len() - 1;
        while j > i && !clear_line(world, at, points[j], radius, elevation, params) {
            j -= 1;
        }
        path.push(points[j]);
        at = points[j];
        i = j + 1;
    }
    (Some(path), looked.get())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_physics::{Cell, Collider, Shape, Terrain};

    /// A way that does not exist costs a few screens of looking, not the whole map. Without the
    /// budget this walks every tile of a 300×300 map, asking about the props under each — which
    /// is the frozen host §24.3 is about.
    #[test]
    fn a_hopeless_search_gives_up_instead_of_walking_the_whole_map() {
        // A big open field with a wall right across it: nothing on the far side is reachable.
        let (cols, rows) = (300u32, 300u32);
        let mut terrain = Terrain::new(cols, rows, 16.0, 16.0);
        for col in 0..cols {
            terrain.fill(col, 150, 1, 1, Cell::Wall);
        }
        let world = World::new(terrain);
        let params = MoveParams::default();
        let from = Vec2::new(2400.0, 800.0);
        let (path, looked) = way(&world, from, Vec2::new(2400.0, 4000.0), 5.0, 0.0, &params);
        assert!(path.is_none(), "there is no way through a wall");
        assert_eq!(
            looked, BUDGET,
            "it stopped at the budget, not at the map's edge"
        );
        assert!(
            (looked as u32) < cols * rows / 4,
            "looked at {looked} tiles of the {} in the map",
            cols * rows
        );
        // A way that does exist, on the same map, is still found — and cheaply.
        let (path, looked) = way(&world, from, Vec2::new(2800.0, 1200.0), 5.0, 0.0, &params);
        assert!(path.is_some(), "the near corner is walkable");
        assert!(looked < BUDGET / 4, "an easy way looked at {looked} tiles");
    }

    /// And on a map the size of the ones the game has, the budget refuses nothing: a way right
    /// round a wall, the dearest kind of search there is, is still found.
    #[test]
    fn on_a_map_the_size_of_the_games_own_every_way_is_still_found() {
        // Meadow is 100×75 tiles; this is that, with a wall across all but its far end.
        let (cols, rows) = (100u32, 75u32);
        let mut terrain = Terrain::new(cols, rows, 16.0, 16.0);
        for col in 0..cols - 6 {
            terrain.fill(col, 40, 1, 1, Cell::Wall);
        }
        let world = World::new(terrain);
        let params = MoveParams::default();
        let (path, looked) = way(
            &world,
            Vec2::new(200.0, 200.0),
            Vec2::new(200.0, 1000.0),
            5.0,
            0.0,
            &params,
        );
        assert!(
            path.is_some(),
            "the way round the wall is there to be found"
        );
        assert!(looked < BUDGET, "it took {looked} tiles of the {BUDGET}");
        assert!(
            (cols * rows) < BUDGET as u32,
            "the budget is bigger than the whole map, so nothing reachable can be refused"
        );
    }

    /// 20×10 tiles of 16 px with a wall across the middle, open only at the bottom row.
    fn walled() -> World {
        let mut terrain = Terrain::new(20, 10, 16.0, 16.0);
        terrain.fill(10, 0, 1, 9, Cell::Wall);
        World::new(terrain)
    }

    #[test]
    fn a_straight_line_through_a_wall_is_not_clear() {
        let world = walled();
        let p = MoveParams::default();
        let (a, b) = (Vec2::new(40.0, 40.0), Vec2::new(280.0, 40.0));
        assert!(!clear_line(&world, a, b, 5.0, 0.0, &p));
        assert!(clear_line(&world, a, Vec2::new(120.0, 100.0), 5.0, 0.0, &p));
    }

    #[test]
    fn the_way_round_goes_through_the_gap() {
        let world = walled();
        let p = MoveParams::default();
        let (a, b) = (Vec2::new(40.0, 40.0), Vec2::new(280.0, 40.0));
        let path = find_path(&world, a, b, 5.0, 0.0, &p).expect("a way round");
        assert_eq!(*path.last().unwrap(), b);
        assert!(
            path.iter().any(|p| p.y > 144.0),
            "passes the gap in the bottom row: {path:?}"
        );
        // Every leg is walkable.
        let mut at = a;
        for &next in &path {
            assert!(clear_line(&world, at, next, 5.0, 0.0, &p), "{at} -> {next}");
            at = next;
        }
    }

    #[test]
    fn props_and_ledges_block_but_a_clear_field_is_one_straight_leg() {
        let mut world = World::new(Terrain::new(20, 10, 16.0, 16.0));
        let p = MoveParams::default();
        let (a, b) = (Vec2::new(40.0, 80.0), Vec2::new(280.0, 80.0));
        assert_eq!(find_path(&world, a, b, 5.0, 0.0, &p), Some(vec![b]));
        world.add(Collider {
            center: Vec2::new(160.0, 80.0),
            shape: Shape::Circle { radius: 20.0 },
            base: 0.0,
            height: 1000.0,
        });
        assert!(!walkable(&world, Vec2::new(160.0, 80.0), 5.0, 0.0, &p));
        assert!(find_path(&world, a, b, 5.0, 0.0, &p).unwrap().len() > 1);
        // A raised level is not walked onto without jumping.
        world.terrain.fill(0, 0, 20, 2, Cell::Level(1));
        assert!(!walkable(&world, Vec2::new(40.0, 16.0), 5.0, 0.0, &p));
        // A goal inside a prop is reached as near as it can be.
        let beside = Vec2::new(160.0, 80.0);
        let path = find_path(&world, a, beside, 5.0, 0.0, &p).expect("to the prop's edge");
        assert_eq!(*path.last().unwrap(), beside);
        // And there is no way into a sealed room.
        let mut sealed = World::new(Terrain::new(20, 10, 16.0, 16.0));
        sealed.terrain.fill(10, 0, 1, 10, Cell::Wall);
        assert_eq!(find_path(&sealed, a, b, 5.0, 0.0, &p), None);
    }
}
