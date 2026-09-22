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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terrain {
    cols: u32,
    rows: u32,
    tile: f32,
    level_height: f32,
    cells: Vec<Cell>,
}

impl Terrain {
    /// Flat terrain covering `cols × rows` tiles of `tile` pixels.
    pub fn new(cols: u32, rows: u32, tile: f32, level_height: f32) -> Self {
        assert!(cols > 0 && rows > 0, "terrain must have at least one tile");
        assert!(
            tile > 0.0 && level_height > 0.0,
            "tile and level height must be positive"
        );
        Self {
            cols,
            rows,
            tile,
            level_height,
            cells: vec![Cell::Floor; (cols * rows) as usize],
        }
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

    /// `None` outside the grid.
    pub fn cell(&self, col: i64, row: i64) -> Option<Cell> {
        if col < 0 || row < 0 || col >= i64::from(self.cols) || row >= i64::from(self.rows) {
            return None;
        }
        Some(self.cells[(row as usize) * self.cols as usize + col as usize])
    }

    /// Sets the tiles of a rectangle, clipped to the grid.
    pub fn fill(&mut self, col: u32, row: u32, width: u32, height: u32, cell: Cell) {
        for r in row..(row.saturating_add(height)).min(self.rows) {
            for c in col..(col.saturating_add(width)).min(self.cols) {
                self.cells[(r * self.cols + c) as usize] = cell;
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
