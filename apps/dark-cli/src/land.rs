//! Looking at a made world from far above (docs/PLAN.md §24.4): one pixel a tile, or one pixel
//! for many, so a designer can see the country a seed makes before walking a step of it.

use dark_land::Land;
use dark_physics::Cell;

/// Draws `tiles` square of the land a seed makes, `every` tiles to the pixel, into a PNG.
pub fn preview(seed: u64, tiles: u32, every: u32, out: &str) -> Result<(), String> {
    if tiles == 0 || every == 0 {
        return Err("a preview needs a size and a step, both above zero".into());
    }
    let land = Land::new(seed);
    let side = tiles.div_ceil(every);
    let mut rgba = vec![0u8; (side as usize) * (side as usize) * 4];
    for row in 0..side {
        for col in 0..side {
            let (c, r) = (i64::from(col * every), i64::from(row * every));
            let colour = match land.cell(c, r) {
                // Water, level plain, and the three steps up out of it.
                Cell::Wall => [40, 70, 120],
                Cell::Floor => [96, 140, 74],
                Cell::Level(1) => [124, 156, 82],
                Cell::Level(2) => [156, 148, 96],
                Cell::Level(_) => [196, 190, 176],
            };
            let at = ((row as usize) * (side as usize) + col as usize) * 4;
            rgba[at..at + 3].copy_from_slice(&colour);
            rgba[at + 3] = 255;
        }
    }
    image::save_buffer(out, &rgba, side, side, image::ColorType::Rgba8)
        .map_err(|err| format!("cannot save {out}: {err}"))?;
    let km = f64::from(tiles) / 1000.0;
    println!("{out}: {km} km of the land of seed {seed}, {every} tiles to the pixel");
    if let Some((col, row)) = most_varied(&land, tiles) {
        println!("the most varied ground in it is around tile ({col}, {row})");
    }
    Ok(())
}

/// Where the land changes most within a screenful — a coast, or a hillside with cliffs. Somewhere
/// worth standing to see what a seed makes, rather than the middle of a plain.
fn most_varied(land: &Land, tiles: u32) -> Option<(i64, i64)> {
    // A screenful of the first game is about forty tiles across.
    let screen = 40i64;
    let mut best: Option<(usize, (i64, i64))> = None;
    let step = (tiles / 64).max(screen as u32);
    // A screenful in from the edges, so what is suggested is somewhere a camera can sit.
    let margin = screen as u32 * 2;
    for row in (margin..tiles.saturating_sub(margin)).step_by(step as usize) {
        for col in (margin..tiles.saturating_sub(margin)).step_by(step as usize) {
            let (c, r) = (i64::from(col), i64::from(row));
            let mut seen = [false; 5];
            for down in 0..4 {
                for across in 0..4 {
                    let at = land.cell(c + across * screen / 4, r + down * screen / 4);
                    let which = match at {
                        Cell::Wall => 0,
                        Cell::Floor => 1,
                        Cell::Level(n) => (n as usize + 1).min(4),
                    };
                    seen[which] = true;
                }
            }
            // Somewhere to stand, not the middle of a lake.
            if land.cell(c, r) == Cell::Wall {
                continue;
            }
            let sorts = seen.iter().filter(|s| **s).count();
            if best.is_none_or(|(most, _)| sorts > most) {
                // The middle of the screenful, which is where a camera would sit.
                best = Some((sorts, (c + screen / 2, r + screen / 2)));
            }
        }
    }
    best.map(|(_, at)| at)
}
