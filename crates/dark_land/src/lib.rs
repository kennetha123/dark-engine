//! Making land from a seed (docs/PLAN.md §24.4): what the ground is like at a tile, worked out
//! rather than written down.
//!
//! A world too large to draw by hand — ten or sixteen kilometres, a hundred million tiles — is
//! not stored. Given the seed the scene carries, this says what any tile is: level ground, a step up onto a
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
pub const COARSE: i64 = 2_048;
/// How many lattices are laid over one another, each half the width and half the weight of the
/// one before. Six of them take 2 km down to 64 tiles, which is the size of a copse.
const OCTAVES: u32 = 6;
/// Everything below this is water. Measured over five countries, twenty-five kilometres of each,
/// it leaves 13% to 21% of the land under water, 17% of it taken together: lakes and inlets
/// rather than an ocean.
const WATER: u32 = 24_300;
/// Where each step up begins, out of the full height. Measured the same way, these leave about
/// half the country walkable plain, and the rest rising in three steps — roughly 12, 13 and 4
/// parts in a hundred, so the highest ground is rare.
const STEPS: [u32; 3] = [38_100, 41_600, 47_900];

/// The highest a made tile ever stands, in steps. Anything drawing made land must allow for a
/// tile this high being drawn above where it stands, before it has seen any of the land.
pub const HIGHEST: u8 = STEPS.len() as u8;

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

    /// How high a tile stands. Several lattices laid over one another: a broad one for the shape
    /// of the country, finer ones for the shape of a hillside.
    ///
    /// The number runs 0 to 65 535 in principle, but it is a weighted average of lattice corners
    /// and so keeps to the middle of that: measured over five countries of twenty-five kilometres
    /// each it ran from 5 278 to 60 328, and the thresholds below are set against that spread,
    /// not against the ends.
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

    /// How high one corner of a lattice stands: the seed and that corner, stirred.
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

    /// The land a seed makes, written down. Asking the same tile twice in one process proves
    /// nothing — the answer is a pure sum — so this pins the sum itself: change the stirring,
    /// the easing, the lattices or the thresholds and this fails, which is what should happen,
    /// because a host and a client built from different code would disagree about the ground.
    #[test]
    fn a_seed_makes_the_same_land_it_always_made() {
        let land = Land::new(20_260_923);
        // A handful of tiles, near and far, above and below the origin.
        let asked = [
            (0i64, 0i64),
            (1, 1),
            (-1, -1),
            (9_350, 100),
            (128_000, 128_000),
            (-4_321, 8_765),
            (16_000, 15_999),
            (1_000_000, -1_000_000),
        ];
        let heights: Vec<u32> = asked.iter().map(|(c, r)| land.height(*c, *r)).collect();
        assert_eq!(
            heights,
            vec![
                21_474, 21_475, 21_474, 24_590, 28_185, 27_156, 29_480, 32_859
            ],
            "the land of seed 20260923 is not what it was"
        );
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
        for seed in [1u64, 2, 7, 11, 20_260_923] {
            a_country_of(seed);
        }
    }

    /// What one seed's country is made of, as parts of a hundred.
    fn a_country_of(seed: u64) {
        let land = Land::new(seed);
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
            (8..28).contains(&part(0)),
            "water is {}% of seed {seed}'s country: {counts:?}",
            part(0)
        );
        assert!(
            (40..70).contains(&part(1)),
            "{}% of seed {seed} is level plain: {counts:?}",
            part(1)
        );
        assert!(
            counts[2] > all / 20 && counts[3] > all / 50 && counts[4] > all / 200,
            "seed {seed}'s rises are too few: {counts:?}"
        );
    }

    /// Hillsides are walked up, not climbed. A rise of two steps at once is a wall a walker can
    /// never get up, and a world full of them is a world of pens.
    ///
    /// The real reason it holds is that neighbouring tiles differ in height by far less than the
    /// gap between one step and the next, so that is what is checked — everywhere, in every
    /// direction. Add an octave or narrow the steps and this says so.
    #[test]
    fn the_land_rises_a_step_at_a_time() {
        let mut worst = 0u32;
        for seed in [1u64, 3, 20_260_923] {
            let land = Land::new(seed);
            // Four windows, near and far from the origin, north-west of it as well as south-east.
            for (from_col, from_row) in [(0i64, 0i64), (7_000_000, -3_000_000), (-120_345, 98_765)]
            {
                for row in 0..200 {
                    for col in 0..200 {
                        let (c, r) = (from_col + col, from_row + row);
                        let here = land.height(c, r);
                        for (dc, dr) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                            worst = worst.max(here.abs_diff(land.height(c + dc, r + dr)));
                        }
                        let level = land.cell(c, r).level();
                        for (dc, dr) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                            if let (Some(a), Some(b)) = (level, land.cell(c + dc, r + dr).level()) {
                                assert!(
                                    a.abs_diff(b) <= 1,
                                    "a rise of {} steps at ({c}, {r}) in seed {seed}",
                                    a.abs_diff(b)
                                );
                            }
                        }
                    }
                }
            }
        }
        // And why it holds: no two neighbours are ever a step's worth of height apart.
        let narrowest = STEPS
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .chain(std::iter::once(STEPS[0] - WATER))
            .min()
            .unwrap_or(0);
        assert!(
            worst * 4 < narrowest,
            "neighbours differ by up to {worst}, against {narrowest} between one step and the next"
        );
    }
}
