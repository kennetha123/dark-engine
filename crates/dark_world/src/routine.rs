//! People's days (docs/PLAN.md §21): a villager walks to where their day says they should be,
//! and lies down where it says they sleep.
//!
//! A routine is a handful of hours and places, written per person in the scene. The person in
//! force at any hour is found the way a season is: the latest entry whose hour has come, so the
//! last of the day runs through midnight. Walking is the same steering the enemies use — a
//! straight line while it is clear, an A* way round when it is not.
//!
//! A routine never fights anything else for the controls: whoever is talking, following a
//! player, or otherwise held stands where they are.

use bevy_ecs::prelude::*;
use dark_assets::DayEntry;
use glam::Vec2;

use crate::characters::{CharacterState, ControlInput, TickInput};
use crate::maps::BodyState;
use crate::talk::{Npc, Speech};
use crate::{MapId, Maps, WorldClock};

/// Near enough to where they were going.
const ARRIVED: f32 = 5.0;
/// Ticks between working out the way again while walking.
const REPATH_TICKS: u16 = 20;
/// A villager walks at this share of walking speed: an unhurried pace.
const PACE: f32 = 0.7;
/// Times a villager looks for a way to somewhere before letting the hour pass without it.
const GIVES_UP: u8 = 3;

/// The day a person keeps, and where they are in it.
#[derive(Component, Clone, Debug)]
pub struct Routine {
    pub day: Vec<DayEntry>,
    /// The scene the day is written in: its places mean nothing in any other map.
    in_map: MapId,
    /// The way to where they are going, while a straight line is blocked.
    path: Vec<Vec2>,
    /// Ticks before the way there is worked out again; the way is kept until then.
    repath_in: u16,
    /// The hour they are keeping, so a new one starts the thinking again.
    keeping: Option<f32>,
    /// Times the way there could not be found at all. Past [`GIVES_UP`] they stand and wait
    /// for the hour to change rather than search the map every tick.
    lost: u8,
}

impl Routine {
    pub(crate) fn new(day: Vec<DayEntry>, in_map: MapId) -> Self {
        Self {
            day,
            in_map,
            path: Vec::new(),
            repath_in: 0,
            keeping: None,
            lost: 0,
        }
    }

    /// Starts afresh when the hour changes: somewhere new to be, and the old way there — and
    /// having given up on it — mean nothing.
    fn keeps(&mut self, hour: f32) {
        if self.keeping != Some(hour) {
            self.keeping = Some(hour);
            self.path.clear();
            self.repath_in = 0;
            self.lost = 0;
        }
    }
}

/// A villager keeping their day: not one in a conversation, and not one following a player.
type Villager = (
    Entity,
    &'static Npc,
    &'static MapId,
    &'static BodyState,
    &'static mut CharacterState,
    &'static mut ControlInput,
    &'static mut Routine,
    Has<Speech>,
);

/// Whose day is their own: an NPC nobody is leading anywhere.
type OnTheirOwn = (
    With<Npc>,
    Without<crate::Companion>,
    Without<crate::Dormant>,
);

/// Walks each villager towards where the hour says they should be, and puts them to bed where
/// their day says they sleep.
pub(crate) fn walk_the_day(
    clock: Res<WorldClock>,
    maps: Res<Maps>,
    story: Option<Res<crate::StoryState>>,
    mut villagers: Query<Villager, OnTheirOwn>,
) {
    let hour = clock.0.hour() as f32;
    for (entity, npc, map, body, mut state, mut control, mut routine, saying) in &mut villagers {
        // A day is written in one scene's places: carried into another map (led there and let
        // go, or put there by a save), its keeper simply stands, as they did before they had one.
        if *map != routine.in_map {
            continue;
        }
        // Anyone in a conversation — saying a line, hearing one, or waiting through a storylet's
        // answer — stays where they are: walking off would end the conversation they are in.
        let talking = saying
            || npc.talking_with().is_some()
            || story.as_ref().is_some_and(|s| s.busy_with(entity));
        if control.hold || talking || !state.fighter.is_free() || state.impaired.out {
            control.input = TickInput::default();
            continue;
        }
        let Some(entry) = DayEntry::at_hour(&routine.day, hour).copied() else {
            continue;
        };
        let (me, target) = (body.0.position, Vec2::from(entry.at));
        let routine = &mut *routine;
        routine.keeps(entry.from);
        let arrived = me.distance(target) <= ARRIVED;
        state.sleeping = entry.sleep && arrived;
        control.input = TickInput::default();
        if arrived || state.sleeping || routine.lost >= GIVES_UP {
            routine.path.clear();
            continue;
        }
        let world = &maps.get(*map).collision;
        let (radius, elevation) = (body.0.radius, body.0.elevation);
        let walk = |toward: Vec2, input: &mut TickInput| {
            let away = toward - me;
            if away.length() > f32::EPSILON {
                input.movement = away.normalize() * PACE;
            }
        };
        // The way is worked out now and then, not every tick: a village's worth of people all
        // looking at once is the expensive part, and a way once found is walked for free. It is
        // worked out again even while a way is held, so somebody standing in the road is gone
        // round rather than leaned on all hour.
        routine.repath_in = routine.repath_in.saturating_sub(1);
        if routine.repath_in == 0 {
            routine.repath_in = REPATH_TICKS;
            routine.path.clear();
            if crate::nav::clear_line(world, me, target, radius, elevation, &maps.params) {
                walk(target, &mut control.input);
                continue;
            }
            match crate::nav::find_path(world, me, target, radius, elevation, &maps.params) {
                Some(path) => {
                    routine.path = path;
                    routine.lost = 0;
                }
                None => {
                    routine.lost += 1;
                    if routine.lost == GIVES_UP {
                        tracing::warn!("a villager cannot reach {target} and gives up on it");
                    }
                    continue;
                }
            }
        }
        if routine.path.is_empty() {
            // Between one working-out and the next, with the way already walked: head straight
            // for it, which is where the last leg was taking them anyway.
            walk(target, &mut control.input);
            continue;
        }
        crate::ai::follow(&mut routine.path, me, &mut control.input, walk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> Vec<DayEntry> {
        vec![
            DayEntry {
                from: 6.0,
                at: (100.0, 100.0),
                sleep: false,
            },
            DayEntry {
                from: 18.0,
                at: (200.0, 100.0),
                sleep: false,
            },
            DayEntry {
                from: 22.0,
                at: (300.0, 100.0),
                sleep: true,
            },
        ]
    }

    #[test]
    fn the_hour_decides_where_someone_should_be() {
        let day = day();
        let at = |hour| DayEntry::at_hour(&day, hour).unwrap().at;
        assert_eq!(at(7.0), (100.0, 100.0), "the morning's place");
        assert_eq!(at(17.9), (100.0, 100.0), "until the evening");
        assert_eq!(at(18.0), (200.0, 100.0), "on the hour it changes");
        assert_eq!(at(23.5), (300.0, 100.0), "abed");
        // Before the first hour of the day: still the last of the night before.
        assert_eq!(at(3.0), (300.0, 100.0), "the small hours");
        assert!(DayEntry::at_hour(&day, 23.0).unwrap().sleep);
        assert!(!DayEntry::at_hour(&day, 12.0).unwrap().sleep);
    }

    #[test]
    fn someone_with_no_day_written_has_nowhere_to_be() {
        assert!(DayEntry::at_hour(&[], 12.0).is_none());
    }
}
