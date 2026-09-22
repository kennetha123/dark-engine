//! Host-side world simulation. Shared by the windowed player (when hosting) and the headless host.
//!
//! Network session handling, the world clock, maps with moving bodies, characters and their
//! replication, combat and the world simulation. Each tick runs, in order: [`HostReceive`]
//! (network in), [`NetReceive`] (sessions and inputs), [`Control`] (players' input, enemy AI),
//! [`Physics`], [`Fight`] (hits, deaths), [`WorldStep`] (sleep consensus, bodies' minutes, the
//! world simulation),
//! [`NetSend`] (snapshots), [`HostSend`] (network out). NPCs stand in scenes and talk
//! ([`TalkPlugin`]); enemies guard their posts ([`CombatPlugin`]); players' bodies get hungry,
//! drunk and cold ([`LifePlugin`]), and a player who leaves is put to bed at the nearest inn;
//! people of the world follow players who lead parties ([`PartyPlugin`]).

mod ai;
mod characters;
mod client;
mod combat;
mod life;
mod maps;
mod nav;
mod party;
mod replication;
mod save;
mod story;
mod talk;
mod world_sim;

pub use characters::{
    Asleep, CHARACTER_RADIUS, CharacterSheets, CharacterState, CharactersPlugin, Control,
    ControlInput, Impairment, Look, LookId, NetId, PlayerAvatar, RUN_SPEED, TickInput, WALK_SPEED,
    control, sheet_of,
};
pub use client::{ClientSession, DrawCharacter, INTERPOLATION_DELAY};
pub use combat::{CORPSE_TICKS, CombatPlugin, Dormant, Fight, Hostile, PLAYER_RESPAWN_TICKS, Post};
pub use life::{
    FIRE_REACH, Life, LifePlugin, LifeRules, LifeView, NightPass, Placed, StructureSnapshot,
    TENT_REACH, life_of, structures_in,
};
pub use party::{Companion, PartyPlugin, PartyRoster, Person, party_members};
pub use replication::{
    CharacterSnapshot, ClientInputs, INPUT_REDUNDANCY, LocalInput, NetReceive, NetSend,
    ReplicationPlugin, SNAPSHOT_INTERVAL, Snapshot,
};
pub use save::{SAVE_VERSION, SavedCharacter, SavedPerson, SavedStructure, WorldSave};
pub use story::{Conversing, Faded, StoryPlugin, StoryState, StoryView, story_view};
pub use talk::{LEAVE_RANGE, Npc, SPEECH_TICKS, Speech, TALK_RANGE, TalkPlugin, talk_target};
pub use world_sim::{WorldSimPlugin, WorldState, WorldStep, load_world, save_now};

pub use maps::{
    BodyState, Exit, Map, MapId, Maps, MapsPlugin, MoveIntent, Physics, PreviousBody, StaysInMap,
    body_bundle,
};

/// Scattered props keep this far from a scene's player spawn.
pub const SPAWN_CLEARING: f32 = 90.0;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use dark_core::{App, FixedDelta, FixedUpdate, Plugin};
use dark_net::{Host, SessionEvent};
use dark_time::{ClockConfig, ClockEvent, GameClock};

/// Reads the network and advances the clock; first in every tick.
#[derive(bevy_ecs::schedule::SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostReceive;

/// Flushes packets; last in every tick.
#[derive(bevy_ecs::schedule::SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostSend;

#[derive(Resource)]
pub struct NetHost(pub Host);

#[derive(Resource)]
pub struct WorldClock(pub GameClock);

/// Session changes produced this tick, for gameplay systems to act on.
#[derive(Resource, Default)]
pub struct SessionEvents(pub Vec<SessionEvent>);

/// Hands out [`NetId`]s: to players' characters as they join and to NPCs as maps load.
#[derive(Resource, Default)]
pub(crate) struct NextNetId(u32);

impl NextNetId {
    pub(crate) fn allocate(&mut self) -> NetId {
        self.0 += 1;
        NetId(self.0)
    }
}

/// Host simulation: receive → simulate → send, once per fixed tick.
pub struct HostPlugin {
    pub host: Host,
    pub clock: ClockConfig,
}

impl Plugin for HostPlugin {
    fn build(self, app: &mut App) {
        assert_eq!(
            app.world.resource::<FixedDelta>().0,
            std::time::Duration::from_secs(1) / self.clock.tick_rate,
            "clock tick rate must match the app tick rate"
        );
        app.insert_resource(NetHost(self.host))
            .insert_resource(WorldClock(GameClock::new(self.clock)))
            .insert_resource(SessionEvents::default())
            .configure_sets(
                FixedUpdate,
                (
                    HostReceive,
                    NetReceive,
                    Control,
                    Physics,
                    Fight,
                    WorldStep,
                    NetSend,
                    HostSend,
                )
                    .chain(),
            )
            .configure_sets(FixedUpdate, NightPass.in_set(WorldStep))
            .add_systems(
                FixedUpdate,
                (receive, advance_clock).chain().in_set(HostReceive),
            )
            .add_systems(FixedUpdate, send.in_set(HostSend));
    }
}

fn receive(mut host: ResMut<NetHost>, dt: Res<FixedDelta>, mut events: ResMut<SessionEvents>) {
    events.0 = host.0.update(dt.0);
    for event in &events.0 {
        tracing::info!("session: {event:?}");
    }
}

fn advance_clock(mut clock: ResMut<WorldClock>) {
    match clock.0.tick() {
        Some(ClockEvent::NewDay(day)) => tracing::info!("day {} begins", day + 1),
        Some(ClockEvent::YearEnded) => tracing::info!("the year is over"),
        None => {}
    }
}

fn send(mut host: ResMut<NetHost>) {
    host.0.send_packets();
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_net::{HostConfig, PlayerId};

    #[test]
    fn local_player_joins_through_the_sim() {
        let mut app = App::new(dark_core::DEFAULT_TICK_RATE);
        let mut host = Host::new(HostConfig::default()).unwrap();
        let player = PlayerId::random();
        host.connect_local(player);
        app.add_plugin(HostPlugin {
            host,
            clock: ClockConfig::default(),
        });

        let mut joined = false;
        for _ in 0..5 {
            app.tick();
            joined |= app
                .world
                .resource::<SessionEvents>()
                .0
                .contains(&SessionEvent::Joined {
                    player,
                    first_time: true,
                });
        }
        assert!(joined);
    }
}

#[cfg(test)]
mod net_tests;
