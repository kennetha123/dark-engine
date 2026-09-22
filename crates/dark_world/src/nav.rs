//! Finding a way on foot for characters the world moves (enemies now, NPCs later): who can walk
//! where without jumping, whether a straight line is clear, and A* over the tile grid when not.

use dark_physics::{MoveParams, World};
use glam::Vec2;

use crate::maps::overlaps;

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
    flat && !world.colliders.iter().any(|c| {
        c.base + c.height > elevation + params.step_up
            && c.base <= elevation + params.step_up
            && overlaps(c, center, radius)
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
/// A* over tile centres (8-way, no cutting corners), then shortened wherever a straight line is
/// clear. `None` if there is no way.
pub fn find_path(
    world: &World,
    from: Vec2,
    to: Vec2,
    radius: f32,
    elevation: f32,
    params: &MoveParams,
) -> Option<Vec<Vec2>> {
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
        (1..=2i64)
            .flat_map(|r| {
                (-r..=r).flat_map(move |dc| (-r..=r).map(move |dr| (wanted.0 + dc, wanted.1 + dr)))
            })
            .filter(|&t| open(t))
            .min_by(|&a, &b| center(a).distance(to).total_cmp(&center(b).distance(to)))?
    };
    let (tiles, _) = pathfinding::prelude::astar(
        &start,
        |&(c, r)| {
            let mut next = Vec::with_capacity(8);
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
    )?;
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
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_physics::{Cell, Collider, Shape, Terrain};

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
        world.colliders.push(Collider {
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
