//! Bodies make room for each other (docs/PLAN.md §10), so nobody walks through anybody.
//!
//! **A body is pushed back at most as far as it just moved.** Walk into someone and you are put
//! back exactly where you leaned on them, so you slide around them; stand still and nobody can
//! shove you, so a villager left somewhere stays there and a crowd cannot push a player around.
//! Two walkers leaning on each other are each put back as far as they closed, so they meet and
//! stay there instead of bouncing. Bodies that overlap without either moving — two players
//! spawning on the same spot — stay overlapped until one of them walks, which parts them.
//!
//! One rule, used twice: the host runs it over every character in a map, and a client runs it
//! for its own character against the ones the host last showed it. A client needs nothing of the
//! others but where they stood, so what it predicts is what the host does, to the pixel, for
//! every body that is standing still and replicated to it.

use bevy_ecs::prelude::*;
use dark_physics::{Body, MoveParams, World};
use glam::Vec2;

use crate::characters::{CharacterState, NetId};
use crate::maps::{BodyState, PreviousBody};
use crate::{MapId, Maps};

/// The most a body is moved in a tick, however many others it is inside. Above the fastest a
/// body travels in a tick (a dodge at 230 px/s is 3.8, the heaviest knockback 4.3), or a roll
/// would carry someone through another body faster than they could be put back.
const MOST: f32 = 6.0;
/// Feet this much of a level apart are one above the other — on a ledge, or jumping over
/// someone — and pass each other.
const LEVEL_SHARE: f32 = 0.9;

/// Somebody standing there: who they are, where their feet are, how wide they are, and how high
/// they stand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Standing {
    pub who: NetId,
    pub position: Vec2,
    pub radius: f32,
    pub elevation: f32,
}

impl Standing {
    pub(crate) fn of(who: NetId, body: &Body) -> Self {
        Self {
            who,
            position: body.position,
            radius: body.radius,
            elevation: body.elevation,
        }
    }
}

/// How far `me`'s `body` must move to stand clear of `others`, given how far it `travelled` this
/// tick: never further than that, so only what walked in is put back and a body that did not
/// move is not moved at all, and never more than [`MOST`], so nobody is flung. Bodies more than
/// `reach` apart in height pass each other.
pub(crate) fn push_from(
    me: NetId,
    body: &Body,
    travelled: f32,
    reach: f32,
    others: impl IntoIterator<Item = Standing>,
) -> Vec2 {
    let mut push = Vec2::ZERO;
    for other in others {
        if other.who == me || (other.elevation - body.elevation).abs() > reach {
            continue;
        }
        let between = body.position - other.position;
        let room = body.radius + other.radius;
        let gap = between.length();
        if gap >= room {
            continue;
        }
        // Standing in exactly the same spot (two players spawning together), step aside: the
        // earlier of the two goes west and the other east, so they part instead of moving as one.
        let away = if gap > f32::EPSILON {
            between / gap
        } else if me.0 < other.who.0 {
            Vec2::NEG_X
        } else {
            Vec2::X
        };
        push += away * (room - gap);
    }
    // A body thrown a long way (knocked back, arriving through an exit) is still only eased out.
    push.clamp_length_max(travelled.min(MOST))
}

/// Pushes `body` out of `others` through `world`, so a push slides along walls instead of
/// through them. A body standing clear is left alone.
pub(crate) fn make_room(
    me: NetId,
    body: &mut Body,
    travelled: f32,
    world: &World,
    params: &MoveParams,
    others: impl IntoIterator<Item = Standing>,
) {
    let reach = world.terrain.level_height() * LEVEL_SHARE;
    let push = push_from(me, body, travelled, reach, others);
    world.slide(body, push, params);
}

/// A character in the crowd.
type InCrowd = (
    &'static NetId,
    &'static MapId,
    &'static mut BodyState,
    &'static PreviousBody,
    &'static CharacterState,
);

/// The host's pass: everyone makes room for the others in their map, each as far as they just
/// moved. The dead are stepped over rather than walked around, and so is anyone standing in an
/// exit — a guard posted in a doorway is a way through, not a wall.
pub(crate) fn make_room_for_each_other(
    maps: Res<Maps>,
    mut crowd: Query<InCrowd, Without<crate::Dormant>>,
) {
    // Everyone's place is read before anyone moves, so who is pushed first changes nothing;
    // `NetId` order keeps the sums the same on the host and on its clients.
    let mut standing: Vec<(MapId, Standing)> = crowd
        .iter()
        .filter(|(_, map, body, _, state)| {
            !state.fighter.is_dead() && !in_a_doorway(&maps, **map, body.0.position)
        })
        .map(|(id, map, body, _, _)| (*map, Standing::of(*id, &body.0)))
        .collect();
    if standing.len() < 2 {
        return;
    }
    standing.sort_by_key(|(_, standing)| standing.who.0);
    for (id, map, mut body, previous, state) in &mut crowd {
        if state.fighter.is_dead() || in_a_doorway(&maps, *map, body.0.position) {
            continue;
        }
        let travelled = body.0.position.distance(previous.position);
        let others = standing
            .iter()
            .filter(|(other_map, _)| other_map == map)
            .map(|(_, standing)| *standing);
        let world = &maps.get(*map).collision;
        make_room(*id, &mut body.0, travelled, world, &maps.params, others);
    }
}

/// Whether the body stands in an exit. Nobody is pushed out of a doorway, and nobody makes room
/// for anyone standing in one. Only a body an exit does not move can be there — a posted enemy
/// (`StaysInMap`) — and it must not become a cork in the door.
pub(crate) fn in_a_doorway(maps: &Maps, map: MapId, at: Vec2) -> bool {
    maps.get(map).exits.iter().any(|exit| exit.contains(at))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: NetId = NetId(1);
    /// A 16 px level, as the game's tiles are.
    const REACH: f32 = 16.0 * LEVEL_SHARE;
    /// A walk at 80 px/s covers this in a tick.
    const STEP: f32 = 80.0 / 60.0;

    fn body(x: f32, y: f32) -> Body {
        Body::new(Vec2::new(x, y), 5.0)
    }

    /// Someone standing at `x`, `y`, whom `ME` walks into.
    fn them(x: f32, y: f32) -> Standing {
        Standing::of(NetId(2), &body(x, y))
    }

    /// Walking into someone puts you back where you leaned on them, and leaves them be.
    #[test]
    fn a_walker_is_put_back_as_far_as_they_walked_in() {
        let still = them(100.0, 100.0);
        let mut walker = body(110.0, 100.0);
        for _ in 0..30 {
            walker.position.x -= STEP;
            walker.position += push_from(ME, &walker, STEP, REACH, [still]);
        }
        let gap = walker.position.distance(still.position);
        assert!((10.0 - gap).abs() < 0.01, "stopped {gap} px apart");
        assert_eq!(walker.position.y, 100.0, "put straight back, not sideways");
    }

    /// Two walking into each other meet and stay there: each is put back only as far as it
    /// closed, so they come to rest touching instead of bouncing apart and closing again.
    #[test]
    fn two_walkers_meet_without_bouncing() {
        let (mut west, mut east) = (body(94.0, 100.0), body(106.0, 100.0));
        let mut gaps = Vec::new();
        for _ in 0..20 {
            west.position.x += STEP;
            east.position.x -= STEP;
            let (was_west, was_east) =
                (Standing::of(NetId(1), &west), Standing::of(NetId(2), &east));
            west.position += push_from(NetId(1), &west, STEP, REACH, [was_east]);
            east.position += push_from(NetId(2), &east, STEP, REACH, [was_west]);
            gaps.push(west.position.distance(east.position));
        }
        let settled = &gaps[gaps.len() - 6..];
        let jitter = settled
            .windows(2)
            .fold(0.0_f32, |worst, pair| worst.max((pair[1] - pair[0]).abs()));
        assert!(jitter < 0.01, "they jitter {jitter} px: {settled:?}");
        let gap = settled[settled.len() - 1];
        assert!(
            (10.0..10.0 + STEP).contains(&gap),
            "came to rest {gap} px apart, not touching"
        );
    }

    #[test]
    fn nobody_makes_room_for_someone_on_a_ledge_above_or_standing_clear() {
        let mut above = them(100.0, 100.0);
        above.elevation = 16.0;
        let me = body(100.0, 100.0);
        assert_eq!(push_from(ME, &me, STEP, REACH, [above]), Vec2::ZERO);
        assert_eq!(
            push_from(ME, &me, STEP, REACH, [them(120.0, 100.0)]),
            Vec2::ZERO
        );
        // Standing on the same level, they do make room.
        assert_ne!(
            push_from(ME, &me, STEP, REACH, [them(104.0, 100.0)]),
            Vec2::ZERO
        );
    }

    /// Two players spawning on one spot stay there until one of them walks, and then part:
    /// whoever moves is moved, and the earlier of the two goes west.
    #[test]
    fn an_overlap_nobody_walked_into_waits_for_one_of_them_to_move() {
        let (first, second) = (NetId(1), NetId(2));
        let at = body(100.0, 100.0);
        assert_eq!(
            push_from(first, &at, 0.0, REACH, [Standing::of(second, &at)]),
            Vec2::ZERO
        );
        let walking = push_from(first, &at, STEP, REACH, [Standing::of(second, &at)]);
        assert_eq!(walking, Vec2::new(-STEP, 0.0));
        assert_eq!(
            push_from(second, &at, STEP, REACH, [Standing::of(first, &at)]),
            -walking
        );
    }

    /// A dodge or a heavy blow moves a body further in a tick than a walk; the cap must still
    /// be able to put it back, or a roll carries it clean through somebody.
    #[test]
    fn a_dodge_cannot_roll_through_a_body() {
        // A dodge at 230 px/s, the fastest the game moves a body.
        let roll = 230.0 / 60.0;
        let still = them(100.0, 100.0);
        let mut roller = body(112.0, 100.0);
        for _ in 0..14 {
            roller.position.x -= roll;
            roller.position += push_from(ME, &roller, roll, REACH, [still]);
        }
        let gap = roller.position.distance(still.position);
        assert!(gap >= 10.0 - 0.01, "rolled to {gap} px: through or inside");
        assert!(
            roller.position.x > still.position.x,
            "came out the far side"
        );
    }

    /// An exit is a way through: a body standing in one takes no part in the crowd, so a guard
    /// posted in a doorway cannot cork it.
    #[test]
    fn nobody_is_pushed_out_of_a_doorway_or_by_someone_in_one() {
        let maps = Maps::load(&crate::maps::tests::project(), "scenes/a.ron").unwrap();
        let exit = &maps.get(MapId(0)).exits[0];
        let doorway = (exit.min + exit.max) / 2.0;
        assert!(in_a_doorway(&maps, MapId(0), doorway));
        assert!(!in_a_doorway(&maps, MapId(0), Vec2::new(40.0, 40.0)));
    }

    /// However many bodies are involved, and however far a body was thrown into them.
    #[test]
    fn a_push_is_never_a_fling() {
        let me = body(100.0, 100.0);
        let crowd: Vec<Standing> = (0..6)
            .map(|i| Standing::of(NetId(i + 2), &body(100.5 + i as f32 * 0.1, 100.0)))
            .collect();
        let push = push_from(ME, &me, 40.0, REACH, crowd);
        assert!(push.length() <= MOST + 0.001, "{push}");
    }
}
