//! Host side of character replication (docs/PLAN.md §5).
//!
//! Remote players send their inputs, numbered, with the last few repeated so a lost packet
//! costs nothing. The host applies one input per tick per player and sends each player a
//! snapshot of the characters in *their* map, with the last input it applied so the client can
//! reconcile its prediction. The host's own player feeds its character from [`LocalInput`]:
//! its session still goes through the `Host`, but its input has no network to cross.

use std::collections::{BTreeMap, HashMap};

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_core::{App, FixedUpdate, Plugin, SimTick};
use dark_net::{Channel, PlayerId, SessionEvent, SessionState, decode, encode};
use dark_physics::Body;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::characters::{
    Asleep, CharacterSheets, CharacterState, ControlInput, LookId, NetId, PlayerAvatar, TickInput,
};
use crate::life::{Life, LifeRules, LifeView, Placed, StructureSnapshot, nearest_inn};
use crate::talk::{Npc, Speech};
use crate::{
    BodyState, MapId, Maps, NetHost, NextNetId, PreviousBody, SessionEvents, WorldClock,
    body_bundle,
};
use dark_sprite::Facing;

/// Snapshots go out every this many ticks: 20 Hz at 60 Hz.
pub const SNAPSHOT_INTERVAL: u64 = 3;
/// Inputs a client repeats in every message, covering that many lost packets.
pub const INPUT_REDUNDANCY: usize = 8;
/// Inputs queued beyond this are dropped (oldest first) so a client running fast cannot build
/// up latency on the host. Room for a slow client's burst (up to 8 ticks per frame) plus slack.
const MAX_QUEUED_INPUTS: usize = 20;
/// Ticks to wait for a missing input when later ones have arrived, before skipping it.
const GAP_PATIENCE: u32 = 3;
/// Ticks a character may hold waiting for its player's input (half a second). Past this the host
/// stops waiting and steps it with no input, so a stalled or hostile client cannot freeze it in the
/// air; the client is corrected when it catches up.
const MAX_HOLD_TICKS: u32 = 30;
/// Inputs further ahead of the last applied one than this are refused: a client never has
/// more unacknowledged inputs than it keeps for replay.
const MAX_INPUTS_AHEAD: u32 = 240;

/// Client → host, on [`Channel::State`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientInputs {
    /// `(sequence, input)`, oldest first; sequences start at 1.
    pub inputs: Vec<(u32, TickInput)>,
}

/// Host → client, on [`Channel::State`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u64,
    /// Last input sequence the host applied for this client.
    pub acked: u32,
    /// Inputs from this client waiting on the host; the client paces itself to keep this small.
    pub queued: u8,
    /// This client's own character.
    pub you: Option<NetId>,
    /// Map of the client's character; `characters` only covers this map.
    pub map: MapId,
    pub day: u32,
    pub hour: f32,
    pub characters: Vec<CharacterSnapshot>,
    /// Tents and campfires in this map.
    pub structures: Vec<StructureSnapshot>,
    /// The client's own body and pack; nobody else's.
    pub life: Option<LifeView>,
    /// The others in the client's party.
    pub party: Vec<NetId>,
    /// The client's conversation choices, fades and the year's ending.
    pub story: crate::StoryView,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterSnapshot {
    pub id: NetId,
    pub body: Body,
    pub state: CharacterState,
    /// Placed by the world rather than played by someone; can be talked to.
    pub npc: bool,
    /// On the enemies' side.
    pub hostile: bool,
    pub speech: Option<Speech>,
}

/// The host's own player's input, written by the game each frame: set the held `movement` and
/// `run`, and [`TickInput::latch`] presses so they wait for the next tick to consume them.
#[derive(Resource, Default)]
pub struct LocalInput(pub TickInput);

/// Receiving and applying inputs; runs after the host has read the network this tick.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NetReceive;

/// Snapshots; runs after physics and before the host flushes packets.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NetSend;

/// Inputs a remote player has sent that the host has not applied yet.
#[derive(Component, Default)]
struct InputQueue {
    acked: u32,
    pending: BTreeMap<u32, TickInput>,
    /// Ticks spent waiting for the next input while later ones are already here.
    waited: u32,
    /// Consecutive ticks held for lack of input.
    held: u32,
}

pub struct ReplicationPlugin;

impl Plugin for ReplicationPlugin {
    fn build(self, app: &mut App) {
        app.world.init_resource::<NextNetId>();
        app.insert_resource(LocalInput::default())
            .add_systems(
                FixedUpdate,
                (
                    handle_sessions,
                    read_inputs,
                    apply_inputs,
                    apply_local_input,
                )
                    .chain()
                    .in_set(NetReceive),
            )
            .add_systems(FixedUpdate, send_snapshots.in_set(NetSend));
    }
}

/// Joining spawns a character, or wakes the one the player left asleep; leaving puts it to sleep.
fn handle_sessions(
    mut commands: Commands,
    events: Res<SessionEvents>,
    maps: Res<Maps>,
    sheets: Res<CharacterSheets>,
    mut next_id: ResMut<NextNetId>,
    avatars: Query<(Entity, &PlayerAvatar)>,
    mut bodies: Query<(&mut BodyState, &mut PreviousBody, &mut MapId)>,
) {
    // Spawns are deferred until the system ends, so remember this tick's own spawns: two events
    // for one new player in the same tick must not make two characters.
    let mut spawned: HashMap<PlayerId, Entity> = HashMap::new();
    for event in &events.0 {
        let avatar_of = |player: PlayerId, spawned: &HashMap<PlayerId, Entity>| {
            spawned
                .get(&player)
                .copied()
                .or_else(|| avatars.iter().find(|(_, a)| a.0 == player).map(|(e, _)| e))
        };
        match *event {
            SessionEvent::Joined { player, .. } | SessionEvent::Resumed { player } => {
                if let Some(entity) = avatar_of(player, &spawned) {
                    // A new connection numbers its inputs from 1 again.
                    commands
                        .entity(entity)
                        .remove::<Asleep>()
                        .insert(InputQueue::default());
                    continue;
                }
                let spawn = maps.maps[0]
                    .def
                    .player
                    .as_ref()
                    .map_or(Vec2::ZERO, |p| Vec2::from(p.spawn));
                let id = next_id.allocate();
                let entity = commands
                    .spawn((
                        body_bundle(MapId(0), spawn, crate::CHARACTER_RADIUS),
                        CharacterState::new(&sheets, LookId(0), Facing::Down),
                        ControlInput::default(),
                        PlayerAvatar(player),
                        id,
                        InputQueue::default(),
                        crate::combat::SwingHits::default(),
                    ))
                    .id();
                spawned.insert(player, entity);
                tracing::info!("spawned character {} for {player}", id.0);
            }
            // Stays in the world, vulnerable, and stops walking: with no connection its input
            // is idle (see `apply_inputs`).
            SessionEvent::ConnectionLost { .. } => {}
            // Put to bed at the nearest inn (where it is, in a world without one).
            SessionEvent::SendToInn { player, .. } => {
                let Some(entity) = avatar_of(player, &spawned) else {
                    continue;
                };
                commands.entity(entity).insert(Asleep);
                if let Ok((mut body, mut previous, mut map)) = bodies.get_mut(entity)
                    && let Some((inn_map, bed)) = nearest_inn(&maps, *map, body.0.position)
                {
                    crate::combat::place(&mut body, &mut previous, bed, &maps, inn_map);
                    *map = inn_map;
                }
            }
        }
    }
}

fn read_inputs(mut host: ResMut<NetHost>, mut queues: Query<(&PlayerAvatar, &mut InputQueue)>) {
    let mut by_player: HashMap<PlayerId, Vec<(u32, TickInput)>> = HashMap::new();
    for (player, bytes) in host.0.receive(Channel::State) {
        match decode::<ClientInputs>(&bytes) {
            // Honest clients send INPUT_REDUNDANCY; anything much bigger is refused unread.
            Some(message) if message.inputs.len() <= 2 * INPUT_REDUNDANCY => {
                by_player.entry(player).or_default().extend(message.inputs);
            }
            Some(message) => tracing::warn!(
                "{player} sent {} inputs in one message; ignored",
                message.inputs.len()
            ),
            None => tracing::warn!("{player} sent an undecodable input message"),
        }
    }
    for (avatar, mut queue) in &mut queues {
        let Some(inputs) = by_player.remove(&avatar.0) else {
            continue;
        };
        let newest_allowed = queue.acked.saturating_add(MAX_INPUTS_AHEAD);
        for (seq, input) in inputs {
            // Already applied, or implausibly far ahead (a bad or hostile client).
            if seq > queue.acked && seq <= newest_allowed {
                queue.pending.insert(seq, input.sanitized());
            }
        }
        // Too far behind: drop the oldest, but keep their presses so a jump, attack or talk still
        // happens (the movement of dropped inputs is lost and the client gets corrected).
        while queue.pending.len() > MAX_QUEUED_INPUTS {
            if let Some((seq, dropped)) = queue.pending.pop_first() {
                queue.acked = seq;
                if let Some(mut next) = queue.pending.first_entry() {
                    // `latch` takes the newer input's slot and choice; here `dropped` is the
                    // older one.
                    let (item, choice) = (next.get().item, next.get().choice);
                    next.get_mut().latch(dropped);
                    if item != 0 {
                        next.get_mut().item = item;
                    }
                    if choice != 0 {
                        next.get_mut().choice = choice;
                    }
                }
            }
        }
    }
}

/// One input per tick per remote player, each applied exactly once, in order. A tick with no
/// input waiting holds the character still (see [`ControlInput::hold`]) and acknowledges
/// nothing; the queue that builds up afterwards is a jitter buffer. So the host's state after
/// input N is exactly what the client predicted after N, however late N arrived, as long as it
/// arrives within [`MAX_HOLD_TICKS`].
fn apply_inputs(
    host: Res<NetHost>,
    mut avatars: Query<(&PlayerAvatar, &mut InputQueue, &mut ControlInput)>,
) {
    for (avatar, mut queue, mut control) in &mut avatars {
        if !matches!(
            host.0.sessions().state(avatar.0),
            Some(SessionState::InGame { .. })
        ) {
            // Nobody is predicting this character: it simply stands (grace window, asleep).
            *control = ControlInput::default();
            continue;
        }
        // At u32::MAX the client has sent every input it ever can; it just stands.
        let Some(next) = queue.acked.checked_add(1) else {
            *control = ControlInput::default();
            continue;
        };
        let found = match queue.pending.remove(&next) {
            Some(input) => Some((next, input)),
            // A later input is here but not this one: jitter may still deliver it (messages carry
            // only the last few inputs), so wait a little before writing it off as lost.
            None if !queue.pending.is_empty() && queue.waited >= GAP_PATIENCE => {
                queue.pending.pop_first()
            }
            None => None,
        };
        queue.waited = if found.is_none() && !queue.pending.is_empty() {
            queue.waited + 1
        } else {
            0
        };
        *control = match found {
            Some((seq, input)) => {
                queue.acked = seq;
                ControlInput { input, hold: false }
            }
            None => ControlInput::default(),
        };
        queue.held = if found.is_some() { 0 } else { queue.held + 1 };
        control.hold = found.is_none() && queue.held <= MAX_HOLD_TICKS;
    }
}

fn apply_local_input(
    host: Res<NetHost>,
    mut local: ResMut<LocalInput>,
    mut avatars: Query<(&PlayerAvatar, &mut ControlInput), Without<Asleep>>,
) {
    let Some(me) = host.0.local_player() else {
        return;
    };
    let input = local.0;
    local.0.take_presses();
    for (avatar, mut control) in &mut avatars {
        if avatar.0 == me {
            *control = ControlInput { input, hold: false };
        }
    }
}

/// What a snapshot carries of each character.
type Replicated = (
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    Has<Npc>,
    Option<&'static Speech>,
    Has<crate::Hostile>,
);

/// A player a snapshot goes to.
type Recipient = (
    Entity,
    &'static PlayerAvatar,
    &'static NetId,
    &'static MapId,
    &'static InputQueue,
    Option<&'static Life>,
);

/// What each recipient is sent of their own: body, party, conversation, fades.
#[derive(bevy_ecs::system::SystemParam)]
struct Personal<'w, 's> {
    rules: Option<Res<'w, LifeRules>>,
    roster: Option<Res<'w, crate::PartyRoster>>,
    story: Option<Res<'w, crate::StoryState>>,
    world: Option<Res<'w, crate::WorldState>>,
    faded: Query<'w, 's, &'static crate::Faded>,
}

fn send_snapshots(
    tick: Res<SimTick>,
    clock: Res<WorldClock>,
    mut host: ResMut<NetHost>,
    personal: Personal,
    avatars: Query<Recipient>,
    characters: Query<Replicated, Without<crate::Dormant>>,
    structures: Query<(&NetId, &MapId, &Placed)>,
) {
    if !tick.0.is_multiple_of(SNAPSHOT_INTERVAL) {
        return;
    }
    let local = host.0.local_player();
    for (entity, avatar, you, map, queue, life) in &avatars {
        let online = matches!(
            host.0.sessions().state(avatar.0),
            Some(SessionState::InGame { .. })
        );
        if !online || Some(avatar.0) == local {
            continue;
        }
        let snapshot = Snapshot {
            tick: tick.0,
            acked: queue.acked,
            queued: queue.pending.len().min(255) as u8,
            you: Some(*you),
            map: *map,
            day: clock.0.day(),
            hour: clock.0.hour() as f32,
            characters: characters
                .iter()
                .filter(|(_, m, ..)| *m == map)
                .map(
                    |(id, _, body, state, npc, speech, hostile)| CharacterSnapshot {
                        id: *id,
                        body: body.0,
                        state: *state,
                        npc,
                        hostile,
                        speech: speech.cloned(),
                    },
                )
                .collect(),
            structures: structures
                .iter()
                .filter(|(_, m, _)| *m == map)
                .map(|(id, _, p)| StructureSnapshot {
                    id: *id,
                    structure: p.structure,
                    at: p.at,
                })
                .collect(),
            life: life
                .zip(personal.rules.as_ref())
                .map(|(l, r)| l.view(&r.def)),
            party: personal
                .roster
                .as_ref()
                .map(|r| crate::party_members(r, entity))
                .unwrap_or_default(),
            story: crate::StoryView {
                choices: personal
                    .story
                    .as_ref()
                    .zip(personal.world.as_ref())
                    .map(|(s, w)| crate::story::choices_of(s, w, entity))
                    .unwrap_or_default(),
                faded: personal.faded.get(entity).map_or(0, |f| f.0),
                ending: personal.story.as_ref().and_then(|s| s.ending.clone()),
            },
        };
        host.0.send(avatar.0, Channel::State, encode(&snapshot));
    }
}
