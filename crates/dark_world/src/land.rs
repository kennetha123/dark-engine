//! Shaping the land around the players (docs/PLAN.md §24.4).
//!
//! A world of ten or sixteen kilometres is not written down; it is made from the world's seed,
//! a patch of tiles at a time, as somebody walks towards it. This keeps the patches around every
//! player shaped, and never unshapes one: land made once stays as it was made, so a player
//! walking back finds the hill they walked over.
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

/// The world's land, where a map is made rather than drawn.
#[derive(Resource, Clone, Copy, Debug)]
pub struct MadeLand(pub Land);

/// Shapes the patches around each player that have not been shaped yet.
pub fn shape_around_players(
    mut maps: ResMut<Maps>,
    land: Res<MadeLand>,
    players: Query<(&MapId, &BodyState), With<PlayerAvatar>>,
) {
    for (map, body) in &players {
        let Some(map) = maps.maps.get_mut(map.0 as usize) else {
            continue;
        };
        // Only maps that say they are made: an authored one is drawn, not worked out.
        if map.def.land.is_none() {
            continue;
        }
        shape_around(&mut map.collision.terrain, &land.0, body.0.position);
    }
}

/// Shapes the patches within [`AHEAD`] of a place.
pub fn shape_around(terrain: &mut Terrain, land: &Land, at: Vec2) {
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
            if !terrain.is_shaped(c, r) {
                terrain.shape(c, r, |col, row| land.cell(col, row));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_physics::Cell;

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

    /// Land drawn by hand wins: a scene's own fills are laid over what the seed makes, so a
    /// place someone built stays built.
    #[test]
    fn what_was_drawn_by_hand_is_not_overwritten() {
        let land = Land::new(2);
        let mut terrain = Terrain::new(200, 200, 16.0, 16.0);
        terrain.fill(100, 100, 4, 4, Cell::Level(3));
        shape_around(&mut terrain, &land, Vec2::new(100.0 * 16.0, 100.0 * 16.0));
        assert_eq!(
            terrain.cell(101, 101),
            Some(Cell::Level(3)),
            "the patch was already shaped by hand"
        );
    }
}
