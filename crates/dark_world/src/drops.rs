//! Items on the ground (docs/PLAN.md §14): dropped from the pack, and picked up by walking
//! over them.
//!
//! A drop is put down in front of the character, on the ground it stands on, and whoever walks
//! over it picks it up — except the one who laid it down, until they have stepped away from it,
//! so dropping something where you stand does not hand it straight back. A character with
//! nothing to carry things in (an NPC, so far) walks past it.

use bevy_ecs::prelude::*;
use dark_physics::Body;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use dark_net::PlayerId;

use crate::characters::{Asleep, CharacterState, ControlInput, NetId, PlayerAvatar};
use crate::life::{Life, LifeRules};
use crate::maps::BodyState;
use crate::{MapId, Maps, NextNetId};

/// How far in front of the feet a dropped item lands.
const DROP_AHEAD: f32 = 12.0;
/// How close a body's middle must come to take something off the ground.
const REACH: f32 = 17.0;
/// Feet this far above or below a drop cannot reach it (about a step).
const REACH_UP: f32 = 12.0;
/// More of the same item dropped this close joins the pile instead of making another.
const PILES_WITHIN: f32 = 10.0;
/// How wide a footprint a drop needs to land on.
const DROP_RADIUS: f32 = 4.0;

/// An item lying in the world.
#[derive(Component, Clone, Debug)]
pub struct Dropped {
    pub item: String,
    pub count: u16,
    pub at: Vec2,
    /// Height of the ground it lies on.
    pub elevation: f32,
    /// Whoever put it down, until they step out of its reach: it is theirs to leave, so
    /// dropping something at your feet does not hand it straight back. Anyone else may take it.
    pub(crate) laid_by: Option<PlayerId>,
}

/// A drop as clients see it, for the icon drawn on the ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DropSnapshot {
    pub id: NetId,
    pub item: String,
    pub count: u16,
    pub at: (f32, f32),
    pub elevation: f32,
}

/// Someone who can drop what they carry.
type Dropper = (
    Option<&'static PlayerAvatar>,
    &'static MapId,
    &'static BodyState,
    &'static ControlInput,
    &'static CharacterState,
    &'static mut Life,
);

/// `drop` names a hotbar slot: one of that item is laid down in front of the character, or at
/// their feet when there is no room ahead. More of the same item joins the pile already there.
pub(crate) fn drop_items(
    mut commands: Commands,
    mut next_id: ResMut<NextNetId>,
    maps: Res<Maps>,
    mut droppers: Query<Dropper, (Without<Asleep>, Without<crate::Dormant>)>,
    mut piles: Query<(&MapId, &mut Dropped)>,
) {
    for (avatar, map, body, control, state, mut life) in &mut droppers {
        let slot = control.input.drop;
        let busy = control.hold
            || !state.fighter.is_free()
            || state.impaired.out
            || state.sleeping
            || !body.0.grounded
            || slot == 0;
        if busy {
            continue;
        }
        let Some((item, _)) = life
            .inventory
            .slots()
            .get(usize::from(slot - 1))
            .filter(|(_, count)| *count > 0)
            .cloned()
        else {
            continue;
        };
        if !life.inventory.take(&item) {
            continue;
        }
        // Clothes come off as the last of them is dropped, or they would warm nobody.
        if life.inventory.worn.as_deref() == Some(item.as_str()) && life.inventory.count(&item) == 0
        {
            life.inventory.worn = None;
        }
        let by = avatar.map(|a| a.0);
        let at = lands_at(&maps, *map, &body.0, state.facing.vector());
        // Another of the same lying here: one pile, not a heap of ones.
        let pile = piles.iter_mut().find(|(pile_map, pile)| {
            *pile_map == map && pile.item == item && pile.at.distance(at) <= PILES_WITHIN
        });
        if let Some((_, mut pile)) = pile {
            pile.count = pile.count.saturating_add(1);
            pile.laid_by = by;
            continue;
        }
        commands.spawn((
            Dropped {
                item,
                count: 1,
                at,
                elevation: maps.get(*map).collision.ground_under(at, DROP_RADIUS),
                laid_by: by,
            },
            *map,
            next_id.allocate(),
        ));
    }
}

/// Where a drop lands: in front of the feet if that is clear, open ground level with them, else
/// at the feet. Props, walls, ledges and doorways are all no place to leave something.
fn lands_at(maps: &Maps, map: MapId, body: &Body, facing: Vec2) -> Vec2 {
    let ahead = body.position + facing * DROP_AHEAD;
    let world = &maps.get(map).collision;
    let ground = world.ground_under(ahead, DROP_RADIUS);
    let blocked = (ground - body.elevation).abs() > 0.5
        || world
            .colliders
            .iter()
            .any(|c| c.height > 0.0 && crate::maps::overlaps(c, ahead, DROP_RADIUS))
        || maps.get(map).exits.iter().any(|e| e.contains(ahead));
    if blocked { body.position } else { ahead }
}

/// Someone who can pick things up.
type Taker = (
    Option<&'static PlayerAvatar>,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    &'static mut Life,
);

/// A drop goes to whoever walks over it — except the one who put it down, until they have
/// stepped out of its reach, so dropping something where you stand does not hand it back.
pub(crate) fn pick_up_items(
    mut commands: Commands,
    rules: Res<LifeRules>,
    mut drops: Query<(Entity, &MapId, &mut Dropped)>,
    mut takers: Query<Taker, (Without<Asleep>, Without<crate::Dormant>)>,
) {
    for (entity, map, mut drop) in &mut drops {
        // The item may have been taken out of the game's data since it was dropped.
        if !rules.def.items.contains_key(&drop.item) {
            continue;
        }
        // Whoever left it there is free of it once they walk away.
        if let Some(by) = drop.laid_by {
            let still_there = takers.iter().any(|(avatar, taker_map, body, _, _)| {
                avatar.is_some_and(|a| a.0 == by) && *taker_map == *map && reaches(&body.0, &drop)
            });
            if !still_there {
                drop.laid_by = None;
            }
        }
        let taken = takers
            .iter_mut()
            .find(|(avatar, taker_map, body, state, _)| {
                *taker_map == map
                    && drop.laid_by != avatar.map(|a| a.0)
                    && !state.sleeping
                    && !state.impaired.out
                    && !state.fighter.is_dead()
                    && reaches(&body.0, &drop)
            });
        if let Some((_, _, _, _, mut life)) = taken {
            life.inventory.add(&drop.item, drop.count);
            commands.entity(entity).despawn();
        }
    }
}

/// What lies on the ground in `map`, for the host's own screen (clients are sent it).
pub fn drops_in(world: &mut World, map: MapId) -> Vec<DropSnapshot> {
    world
        .query::<(&NetId, &MapId, &Dropped)>()
        .iter(world)
        .filter(|(_, m, _)| **m == map)
        .map(|(id, _, d)| DropSnapshot {
            id: *id,
            item: d.item.clone(),
            count: d.count,
            at: (d.at.x, d.at.y),
            elevation: d.elevation,
        })
        .collect()
}

/// Whether a body is close enough to a drop, and level enough with it, to take it.
fn reaches(body: &Body, drop: &Dropped) -> bool {
    body.position.distance(drop.at) <= REACH && (body.elevation - drop.elevation).abs() <= REACH_UP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop_at(x: f32, y: f32) -> Dropped {
        Dropped {
            item: "bread".into(),
            count: 1,
            at: Vec2::new(x, y),
            elevation: 0.0,
            laid_by: None,
        }
    }

    #[test]
    fn a_drop_is_reached_from_beside_it_but_not_from_across_the_field_or_a_ledge() {
        let mut body = Body::new(Vec2::new(100.0, 100.0), 5.0);
        assert!(reaches(&body, &drop_at(110.0, 100.0)));
        assert!(!reaches(&body, &drop_at(130.0, 100.0)));
        body.elevation = 32.0;
        assert!(!reaches(&body, &drop_at(104.0, 100.0)), "a ledge above it");
    }

    /// A drop lands in front of the feet, or on them when what is ahead is no place for it.
    #[test]
    fn what_is_ahead_decides_where_a_drop_lands() {
        let maps = Maps::load(&crate::maps::tests::project(), "scenes/a.ron").unwrap();
        let (map, world) = (MapId(0), &maps.get(MapId(0)).collision);
        let clear = Vec2::new(160.0, 100.0);
        let body = Body::new(clear, 5.0);
        assert_eq!(
            lands_at(&maps, map, &body, Vec2::X),
            clear + Vec2::X * DROP_AHEAD,
            "clear ground ahead"
        );
        // The test map's prop stands at (60, 40) with a 6 px circle.
        let prop = Vec2::new(60.0, 40.0);
        let at_prop = Body::new(prop - Vec2::X * DROP_AHEAD, 5.0);
        assert_eq!(
            lands_at(&maps, map, &at_prop, Vec2::X),
            at_prop.position,
            "a prop is no place to leave something"
        );
        // Its exit runs down the east edge.
        let door = maps.get(map).exits[0].min;
        let at_door = Body::new(door - Vec2::X * DROP_AHEAD + Vec2::Y * 8.0, 5.0);
        assert_eq!(
            lands_at(&maps, map, &at_door, Vec2::X),
            at_door.position,
            "nor a doorway"
        );
        // A ledge above: the level-1 block starts at tile 8 (128 px).
        let at_ledge = Body::new(Vec2::new(128.0 - DROP_AHEAD, 40.0), 5.0);
        assert_eq!(
            lands_at(&maps, map, &at_ledge, Vec2::X),
            at_ledge.position,
            "nor a step up"
        );
        assert!(world.ground_under(clear, DROP_RADIUS).is_finite());
    }
}
