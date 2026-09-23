//! Making land from a seed (docs/PLAN.md §24.4): what the ground is like at a tile, worked out
//! rather than written down.
//!
//! A world too large to draw by hand — ten or sixteen kilometres, a hundred million tiles — is
//! not stored. Given the world's seed, this says what any tile is: level ground, a step up onto a
//! rise, or water too deep to wade. Ask for the same tile twice, on any machine, and the answer
//! is the same, because every number here is a whole one: a host and a client that disagreed
//! about the land would have players falling through different rocks.
//!
//! What it makes is the shape of a country — plains, rises, lakes and the coasts between them.
//! Towns, ruins and anywhere with someone's hand in it are laid over the top of it (§24.5).
//!
//! Sim crate: no presentation dependencies.

use dark_physics::Cell;
use serde::{Deserialize, Serialize};

/// How many tiles apart the corners of the coarsest lattice are: the width of a whole country's
/// worth of high and low ground. Two thousand tiles is two kilometres at a tile to the metre, so
/// a world of sixteen carries eight of them across — highlands, lowlands and the water between.
const COARSE: i64 = 2_048;
/// How many lattices are laid over one another, each half the width and half the weight of the
/// one before. Six of them take 2 km down to 64 tiles, which is the size of a copse.
const OCTAVES: u32 = 6;
/// Everything below this is water. Measured over six kilometres of a made country, it leaves
/// about a twelfth of it under water: lakes and inlets rather than an ocean.
const WATER: u32 = 24_300;
/// Where each step up begins, out of the full height. Measured the same way, these leave about
/// three fifths of the country walkable plain and the rest rising in three steps.
const STEPS: [u32; 3] = [38_100, 41_600, 47_900];

/// The land of one world, from its seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Land {
    seed: u64,
}

impl Land {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// What one tile is: level ground, a rise of one to three steps, or water.
    pub fn cell(&self, col: i64, row: i64) -> Cell {
        let height = self.height(col, row);
        if height < WATER {
            return Cell::Wall;
        }
        let level = STEPS.iter().filter(|step| height >= **step).count();
        match u8::try_from(level).unwrap_or(0) {
            0 => Cell::Floor,
            n => Cell::Level(n),
        }
    }

    /// How high a tile stands, from nothing (0) to the top of the world (65 535). Several
    /// lattices laid over one another: a broad one for the shape of the country, finer ones for
    /// the shape of a hillside.
    pub fn height(&self, col: i64, row: i64) -> u32 {
        let mut height: u64 = 0;
        let mut weight: u64 = 0;
        let mut apart = COARSE;
        for octave in 0..OCTAVES {
            // Each finer lattice counts for half of the one before it, so the country's shape
            // leads and the hillsides only ruffle it.
            let of_it = 64u64 >> octave;
            height += u64::from(self.lattice(col, row, apart, octave)) * of_it;
            weight += of_it;
            apart = (apart / 2).max(1);
        }
        (height / weight.max(1)) as u32
    }

    /// A value between the four corners of a lattice `apart` tiles wide, eased so that hillsides
    /// are round rather than creased. Whole numbers throughout: the same answer everywhere.
    fn lattice(&self, col: i64, row: i64, apart: i64, octave: u32) -> u32 {
        let (cell_col, cell_row) = (col.div_euclid(apart), row.div_euclid(apart));
        let (in_col, in_row) = (col.rem_euclid(apart), row.rem_euclid(apart));
        // Where between the corners this tile is, as a part of 65 536, eased at both ends.
        let across = ease((in_col * 65_536 / apart) as u32);
        let down = ease((in_row * 65_536 / apart) as u32);
        let corner = |dc: i64, dr: i64| self.corner(cell_col + dc, cell_row + dr, octave);
        let top = mix(corner(0, 0), corner(1, 0), across);
        let bottom = mix(corner(0, 1), corner(1, 1), across);
        mix(top, bottom, down)
    }

    /// How high one corner of a lattice stands: the world's seed and that corner, stirred.
    fn corner(&self, col: i64, row: i64, octave: u32) -> u32 {
        let mut n = self.seed;
        for part in [col as u64, row as u64, u64::from(octave)] {
            n = stir(n ^ part.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        }
        (n >> 48) as u32
    }
}

/// Smooths a part of 65 536 so that a hillside leaves and meets the level ground evenly, instead
/// of at a crease: the usual `t² × (3 − 2t)`, in whole numbers.
fn ease(t: u32) -> u32 {
    let t = u64::from(t);
    let smooth = t * t / 65_536 * (3 * 65_536 - 2 * t) / 65_536;
    smooth.min(65_535) as u32
}

/// Part of the way from `a` to `b`.
fn mix(a: u32, b: u32, part: u32) -> u32 {
    let (a, b, part) = (u64::from(a), u64::from(b), u64::from(part));
    ((a * (65_536 - part) + b * part) / 65_536) as u32
}

/// Stirs a number so that neighbouring ones come out nothing alike (splitmix64's finisher).
fn stir(mut n: u64) -> u64 {
    n = (n ^ (n >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    n ^ (n >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same tile, asked for twice, in any order, is the same tile. Everything rests on this:
    /// a host and a client that disagreed would have players falling through different rocks.
    #[test]
    fn a_tile_is_always_the_same_tile() {
        let land = Land::new(12_345);
        let asked: Vec<Cell> = (0..500).map(|n| land.cell(n * 7, n * 13)).collect();
        let again: Vec<Cell> = (0..500).rev().map(|n| land.cell(n * 7, n * 13)).collect();
        assert_eq!(asked, again.into_iter().rev().collect::<Vec<_>>());
        // And far from the origin, where a world of sixteen kilometres reaches.
        let far = (16_000, 15_999);
        assert_eq!(land.cell(far.0, far.1), land.cell(far.0, far.1));
    }

    /// Another seed is another country.
    #[test]
    fn another_seed_makes_another_country() {
        let (one, other) = (Land::new(1), Land::new(2));
        let differ = (0..2_000)
            .filter(|n| one.cell(n * 3, n * 5) != other.cell(n * 3, n * 5))
            .count();
        assert!(differ > 400, "only {differ} of 2000 tiles differ");
    }

    /// A country worth walking: mostly ground to walk on, some of it raised, a little water —
    /// not one flat plain, and not a wall of cliffs either.
    #[test]
    fn the_land_has_plains_rises_and_water() {
        let land = Land::new(7);
        let mut counts = [0usize; 5];
        for row in 0..500 {
            for col in 0..500 {
                // Strided over twenty-five kilometres: the land's largest features are two
                // across, so a smaller window counts one valley rather than a country.
                match land.cell(col * 47, row * 53) {
                    Cell::Wall => counts[0] += 1,
                    Cell::Floor => counts[1] += 1,
                    Cell::Level(n) => counts[(n as usize + 1).min(4)] += 1,
                }
            }
        }
        let all: usize = counts.iter().sum();
        let part = |n: usize| counts[n] * 100 / all;
        assert!(
            (3..20).contains(&part(0)),
            "water is {}% of the country: {counts:?}",
            part(0)
        );
        assert!(
            (40..80).contains(&part(1)),
            "{}% of it is level plain: {counts:?}",
            part(1)
        );
        assert!(
            counts[2] > all / 50 && counts[3] > all / 200 && counts[4] > 0,
            "the rises are too few: {counts:?}"
        );
    }

    /// Hillsides are walked up, not climbed: a tile's neighbours are at most one step away, or a
    /// walker meets a wall of cliffs it can never get up.
    #[test]
    fn the_land_rises_a_step_at_a_time() {
        let land = Land::new(3);
        let mut steep = 0;
        for row in 0..300 {
            for col in 0..300 {
                let here = land.cell(col, row).level();
                let east = land.cell(col + 1, row).level();
                if let (Some(a), Some(b)) = (here, east)
                    && a.abs_diff(b) > 1
                {
                    steep += 1;
                }
            }
        }
        assert_eq!(steep, 0, "{steep} places rise more than a step at once");
    }
}
