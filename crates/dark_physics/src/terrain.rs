//! Height grid: each tile has a ground level; walls and the map edge block at every height.

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// A tile's ground level. `Level(n)` stands `n × level_height` pixels above level 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cell {
    #[default]
    Floor,
    Level(u8),
    /// Impassable at any height (deep water, the inside of a cliff wall, the void).
    Wall,
}

impl Cell {
    pub fn level(self) -> Option<u8> {
        match self {
            Cell::Floor => Some(0),
            Cell::Level(n) => Some(n),
            Cell::Wall => None,
        }
    }
}

/// How many tiles a patch of terrain holds each way (docs/PLAN.md §24.4). A patch is 8 KB, so
/// shaping one tile costs 8 KB and shaping none costs nothing.
const PATCH: u32 = 64;

/// The land: what height each tile stands at.
///
/// Flat ground is not written down. A patch of tiles is made the first time something is put in
/// it, and where nothing has been put the ground is simply level — so a terrain may be as wide
/// as a game likes and costs only what has been shaped in it. Sixteen kilometres of it, at 16 px
/// to the tile, is a quarter of a million tiles a side and a few hundred kilobytes of
/// bookkeeping until someone raises a hill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terrain {
    cols: u32,
    rows: u32,
    tile: f32,
    level_height: f32,
    /// One place per patch of the grid, holding the patch's tiles once it has any.
    patches: Vec<Option<Box<[Cell]>>>,
    /// Patches across.
    wide: u32,
}

impl Terrain {
    /// Flat terrain covering `cols × rows` tiles of `tile` pixels.
    pub fn new(cols: u32, rows: u32, tile: f32, level_height: f32) -> Self {
        assert!(cols > 0 && rows > 0, "terrain must have at least one tile");
        assert!(
            tile > 0.0 && level_height > 0.0,
            "tile and level height must be positive"
        );
        let wide = cols.div_ceil(PATCH);
        let down = rows.div_ceil(PATCH);
        Self {
            cols,
            rows,
            tile,
            level_height,
            patches: vec![None; (wide as usize) * (down as usize)],
            wide,
        }
    }

    /// Where a tile lives: which patch, and where in it.
    fn place(&self, col: u32, row: u32) -> (usize, usize) {
        let patch = (row / PATCH) as usize * self.wide as usize + (col / PATCH) as usize;
        let within = (row % PATCH) as usize * PATCH as usize + (col % PATCH) as usize;
        (patch, within)
    }

    /// How many patches have been shaped, which is what a terrain costs: a map nobody has been
    /// to holds none of them.
    pub fn shaped(&self) -> usize {
        self.patches.iter().filter(|p| p.is_some()).count()
    }

    pub fn cols(&self) -> u32 {
        self.cols
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn tile(&self) -> f32 {
        self.tile
    }

    pub fn level_height(&self) -> f32 {
        self.level_height
    }

    /// Pixel size of the terrain.
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.cols as f32, self.rows as f32) * self.tile
    }

    /// How many tiles a patch holds each way, so whoever shapes the land can work a patch at a
    /// time.
    pub const PATCH: u32 = PATCH;

    /// Whether the patch holding a tile has been shaped yet.
    pub fn is_shaped(&self, col: u32, row: u32) -> bool {
        col < self.cols && row < self.rows && self.patches[self.place(col, row).0].is_some()
    }

    /// Shapes the patch holding a tile, asking `what` for each tile in it. A patch already
    /// shaped is left alone, so land made once is never made differently later.
    ///
    /// This is how a world too large to write down is made: the patches near the players are
    /// shaped as they are reached, from the world's own seed (docs/PLAN.md §24.4).
    pub fn shape(&mut self, col: u32, row: u32, what: impl Fn(i64, i64) -> Cell) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let (patch, _) = self.place(col, row);
        if self.patches[patch].is_some() {
            return;
        }
        let (first_col, first_row) = (col - col % PATCH, row - row % PATCH);
        let mut tiles = vec![Cell::Floor; (PATCH * PATCH) as usize].into_boxed_slice();
        for within_row in 0..PATCH {
            for within_col in 0..PATCH {
                let (c, r) = (first_col + within_col, first_row + within_row);
                if c < self.cols && r < self.rows {
                    tiles[(within_row * PATCH + within_col) as usize] =
                        what(i64::from(c), i64::from(r));
                }
            }
        }
        self.patches[patch] = Some(tiles);
    }

    /// `None` outside the grid; level ground where nothing has been shaped.
    pub fn cell(&self, col: i64, row: i64) -> Option<Cell> {
        if col < 0 || row < 0 || col >= i64::from(self.cols) || row >= i64::from(self.rows) {
            return None;
        }
        let (patch, within) = self.place(col as u32, row as u32);
        Some(match &self.patches[patch] {
            Some(tiles) => tiles[within],
            None => Cell::Floor,
        })
    }

    /// Sets the tiles of a rectangle, clipped to the grid. Levelling ground that was never
    /// shaped leaves it unshaped, so filling a whole world with `Floor` costs nothing.
    pub fn fill(&mut self, col: u32, row: u32, width: u32, height: u32, cell: Cell) {
        for r in row..(row.saturating_add(height)).min(self.rows) {
            for c in col..(col.saturating_add(width)).min(self.cols) {
                let (patch, within) = self.place(c, r);
                let tiles = match &mut self.patches[patch] {
                    Some(tiles) => tiles,
                    None if cell == Cell::Floor => continue,
                    place => {
                        place.insert(vec![Cell::Floor; (PATCH * PATCH) as usize].into_boxed_slice())
                    }
                };
                tiles[within] = cell;
            }
        }
    }

    /// Ground height of a tile in pixels; walls and outside the map are infinitely high.
    pub fn cell_height(&self, col: i64, row: i64) -> f32 {
        match self.cell(col, row).and_then(Cell::level) {
            Some(level) => f32::from(level) * self.level_height,
            None => f32::INFINITY,
        }
    }

    pub fn tile_of(&self, point: Vec2) -> (i64, i64) {
        let t = (point / self.tile).floor();
        (t.x as i64, t.y as i64)
    }

    /// Tiles whose square the circle overlaps, with their ground height.
    pub(crate) fn cells_under_circle(
        &self,
        center: Vec2,
        radius: f32,
    ) -> impl Iterator<Item = f32> + '_ {
        let (c0, r0) = self.tile_of(center - Vec2::splat(radius));
        let (c1, r1) = self.tile_of(center + Vec2::splat(radius));
        (r0..=r1).flat_map(move |row| {
            (c0..=c1).filter_map(move |col| {
                let min = Vec2::new(col as f32, row as f32) * self.tile;
                let closest = center.clamp(min, min + Vec2::splat(self.tile));
                // Touching an edge is not overlapping; otherwise a body resting against a wall
                // would count as inside it.
                (closest.distance_squared(center) < radius * radius)
                    .then(|| self.cell_height(col, row))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sixteen kilometres of land, made in a moment and costing nothing until it is shaped —
    /// which is what lets a world be as large as a game wants (docs/PLAN.md §24.4).
    #[test]
    fn a_world_costs_only_what_has_been_shaped_in_it() {
        // 16 km at 16 px to the metre: a quarter of a million tiles a side, 256 million tiles.
        let mut land = Terrain::new(16_000, 16_000, 16.0, 16.0);
        assert_eq!(land.shaped(), 0, "flat ground is not written down");
        assert_eq!(land.cell(9_000, 9_000), Some(Cell::Floor));
        // Levelling ground that was never shaped still costs nothing, however much of it.
        land.fill(0, 0, 16_000, 16_000, Cell::Floor);
        assert_eq!(land.shaped(), 0);
        // A hill in the middle of it: only the patches it covers are made.
        land.fill(8_000, 8_000, 40, 40, Cell::Level(2));
        assert_eq!(land.shaped(), 1, "40 tiles across fit inside one patch");
        assert_eq!(land.cell(8_010, 8_010), Some(Cell::Level(2)));
        assert_eq!(land.cell(8_100, 8_100), Some(Cell::Floor));
        // A wall across a hundred tiles touches the patches it crosses and no others: it starts
        // inside one, crosses the whole of the next, and ends in a third.
        land.fill(4_000, 4_000, 100, 1, Cell::Wall);
        assert_eq!(land.shaped(), 4, "one hill and three patches of wall");
        assert_eq!(land.cell(4_099, 4_000), Some(Cell::Wall));
        assert_eq!(land.cell(4_100, 4_000), Some(Cell::Floor));
    }

    /// Levelling a patch that was shaped must go through, however cheap levelling unshaped
    /// ground is. Getting this wrong leaves a hill standing where a designer flattened it.
    #[test]
    fn a_shaped_patch_can_be_levelled_again() {
        let mut land = Terrain::new(100, 100, 16.0, 16.0);
        land.fill(10, 10, 4, 4, Cell::Level(3));
        assert_eq!(land.cell(11, 11), Some(Cell::Level(3)));
        land.fill(10, 10, 4, 4, Cell::Floor);
        assert_eq!(land.cell(11, 11), Some(Cell::Floor), "levelled again");
        assert_eq!(
            land.shaped(),
            1,
            "the patch is kept, since it may be shaped again"
        );
    }

    /// A map whose side is exactly a patch, and one a tile over it.
    #[test]
    fn the_last_patch_may_be_full_or_a_sliver() {
        for (cols, rows) in [(64, 64), (65, 64), (64, 65), (127, 129)] {
            let mut land = Terrain::new(cols, rows, 16.0, 16.0);
            let (far_col, far_row) = (i64::from(cols) - 1, i64::from(rows) - 1);
            land.fill(cols - 1, rows - 1, 1, 1, Cell::Wall);
            assert_eq!(
                land.cell(far_col, far_row),
                Some(Cell::Wall),
                "the far corner of a {cols}x{rows} map"
            );
            assert_eq!(land.cell(far_col + 1, far_row), None, "past the edge");
            assert_eq!(land.cell(far_col, far_row + 1), None, "below the edge");
            assert_eq!(land.cell(0, 0), Some(Cell::Floor), "the near corner");
        }
    }

    /// Shaped or not, the land answers as a plain grid of tiles would.
    #[test]
    fn patches_answer_as_one_flat_grid_would() {
        let (cols, rows) = (200u32, 150u32);
        let mut land = Terrain::new(cols, rows, 16.0, 16.0);
        let mut flat = vec![Cell::Floor; (cols * rows) as usize];
        let mut seed = 0x51_7C_C1_B7_27_22_0A_95u64;
        let mut roll = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        for _ in 0..400 {
            let (col, row) = (roll() % (cols + 20), roll() % (rows + 20));
            let (w, h) = (1 + roll() % 70, 1 + roll() % 70);
            let cell = match roll() % 3 {
                0 => Cell::Floor,
                1 => Cell::Wall,
                _ => Cell::Level((roll() % 4) as u8 + 1),
            };
            land.fill(col, row, w, h, cell);
            for r in row..(row.saturating_add(h)).min(rows) {
                for c in col..(col.saturating_add(w)).min(cols) {
                    flat[(r * cols + c) as usize] = cell;
                }
            }
        }
        for row in -2..i64::from(rows) + 2 {
            for col in -2..i64::from(cols) + 2 {
                let inside =
                    (0..i64::from(cols)).contains(&col) && (0..i64::from(rows)).contains(&row);
                let want = inside.then(|| flat[(row as u32 * cols + col as u32) as usize]);
                assert_eq!(land.cell(col, row), want, "tile ({col}, {row})");
            }
        }
    }

    #[test]
    fn heights_walls_and_outside() {
        let mut t = Terrain::new(4, 4, 16.0, 16.0);
        t.fill(1, 1, 2, 2, Cell::Level(2));
        t.fill(3, 0, 1, 1, Cell::Wall);
        assert_eq!(t.cell_height(0, 0), 0.0);
        assert_eq!(t.cell_height(1, 1), 32.0);
        assert_eq!(t.cell_height(3, 0), f32::INFINITY);
        assert_eq!(t.cell_height(-1, 0), f32::INFINITY);
        assert_eq!(t.cell_height(0, 4), f32::INFINITY);
    }

    #[test]
    fn fill_is_clipped() {
        let mut t = Terrain::new(2, 2, 8.0, 8.0);
        t.fill(1, 1, 50, 50, Cell::Level(1));
        assert_eq!(t.cell(1, 1), Some(Cell::Level(1)));
        assert_eq!(t.cell(0, 0), Some(Cell::Floor));
    }

    #[test]
    fn circle_cells_ignore_touching_edges() {
        let t = Terrain::new(4, 4, 16.0, 16.0);
        // Centre 4 px left of the x=16 line with radius 4: touches column 1, does not overlap it.
        let n = t.cells_under_circle(Vec2::new(12.0, 8.0), 4.0).count();
        assert_eq!(n, 1);
        let n = t.cells_under_circle(Vec2::new(12.5, 8.0), 4.0).count();
        assert_eq!(n, 2);
    }
}
