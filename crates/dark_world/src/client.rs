//! Client side of replication: predicts the local player's character with the host's own code,
//! reconciles with host snapshots, and interpolates everyone else. No rendering (sim crate);
//! the game draws the [`DrawCharacter`]s this produces.

use std::collections::VecDeque;
use std::time::Duration;

use dark_core::FixedTimestep;
use dark_net::{Channel, ClientStatus, RemoteClient, decode, encode};
use dark_physics::Body;
use glam::Vec2;

use crate::characters::{CharacterSheets, CharacterState, NetId, TickInput, control};
use crate::life::{LifeView, StructureSnapshot};
use crate::replication::{ClientInputs, INPUT_REDUNDANCY, Snapshot};
use crate::talk::Speech;
use crate::{MapId, Maps};

/// Others are shown this many ticks behind the newest snapshot (100 ms): two snapshot intervals,
/// so one lost snapshot still leaves a pair to interpolate between.
pub const INTERPOLATION_DELAY: f64 = 6.0;
/// Unacknowledged inputs kept for replay; a host that stops answering is not replayed forever.
const MAX_PENDING: usize = 120;
/// A correction larger than this is a teleport (a map change, a respawn), not an error to smooth.
const MAX_SMOOTHED_ERROR: f32 = 32.0;
/// How much faster or slower than real time the client may run its ticks to keep the host's
/// queue of its inputs short but not empty.
const PACE_ADJUST: f64 = 0.02;

/// A character as the game should draw it this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawCharacter {
    pub id: NetId,
    /// Feet on the ground plane.
    pub ground: Vec2,
    pub elevation: f32,
    pub body: Body,
    pub state: CharacterState,
    /// The local player's own character.
    pub you: bool,
    /// Placed by the world; can be talked to (see [`crate::talk_target`]).
    pub npc: bool,
    /// On the enemies' side.
    pub hostile: bool,
    pub speech: Option<Speech>,
}

struct Predicted {
    id: NetId,
    map: MapId,
    body: Body,
    state: CharacterState,
    /// Position and elevation before the latest predicted tick, for interpolation.
    previous: (Vec2, f32),
    /// Visual offset left over from a correction, decaying to zero so fixes never snap.
    smoothing: Vec2,
    speech: Option<Speech>,
    /// Gone over to the enemies' side.
    hostile: bool,
}

pub struct ClientSession {
    net: RemoteClient,
    maps: Maps,
    sheets: CharacterSheets,
    timestep: FixedTimestep,
    seq: u32,
    pending: VecDeque<(u32, TickInput)>,
    me: Option<Predicted>,
    snapshots: VecDeque<Snapshot>,
    newest: Option<u64>,
    render_tick: f64,
    /// Presses waiting for the next tick.
    presses: TickInput,
    /// Size of the most recent correction, in pixels: how far prediction was off.
    last_correction: f32,
    /// Tick rate multiplier from the host's input queue depth; see [`PACE_ADJUST`].
    pace: f64,
    /// Everyone else as the newest snapshot showed them, in [`NetId`] order, to make room for
    /// while predicting (see [`crate::crowd`]).
    crowd: Vec<crate::crowd::Standing>,
    /// The player's standing and people's feelings, as last sent.
    ties: crate::story::Ties,
}

impl ClientSession {
    pub fn new(net: RemoteClient, maps: Maps, sheets: CharacterSheets) -> Self {
        Self {
            net,
            maps,
            sheets,
            timestep: FixedTimestep::new(dark_core::DEFAULT_TICK_RATE),
            seq: 0,
            pending: VecDeque::new(),
            me: None,
            snapshots: VecDeque::new(),
            newest: None,
            render_tick: 0.0,
            presses: TickInput::default(),
            last_correction: 0.0,
            pace: 1.0,
            crowd: Vec::new(),
            ties: Default::default(),
        }
    }

    /// How far the last snapshot moved the local character's prediction, in pixels. Zero when
    /// prediction agrees with the host; a steady non-zero value means the two simulations differ.
    pub fn last_correction(&self) -> f32 {
        self.last_correction
    }

    pub fn status(&self) -> ClientStatus {
        self.net.status()
    }

    pub fn net_mut(&mut self) -> &mut RemoteClient {
        &mut self.net
    }

    pub fn maps(&self) -> &Maps {
        &self.maps
    }

    /// The map the local player's character is in, once the host has said.
    pub fn map(&self) -> Option<MapId> {
        self.me.as_ref().map(|m| m.map)
    }

    /// The local player's body and pack, from the newest snapshot.
    pub fn life(&self) -> Option<&LifeView> {
        self.snapshots.back().and_then(|s| s.life.as_ref())
    }

    /// Tents and campfires in the local player's map, from the newest snapshot.
    pub fn structures(&self) -> &[StructureSnapshot] {
        match (self.snapshots.back(), self.map()) {
            (Some(s), Some(map)) if s.map == map => &s.structures,
            _ => &[],
        }
    }

    /// Items lying on the ground in the local player's map, from the newest snapshot.
    pub fn drops(&self) -> &[crate::drops::DropSnapshot] {
        match (self.snapshots.back(), self.map()) {
            (Some(s), Some(map)) if s.map == map => &s.drops,
            _ => &[],
        }
    }

    /// The local player's conversation choices, fades and the year's ending, from the newest
    /// snapshot.
    pub fn story(&self) -> crate::StoryView {
        let mut story = self
            .snapshots
            .back()
            .map(|s| s.story.clone())
            .unwrap_or_default();
        (story.standing, story.feelings) = self.ties.clone();
        story
    }

    /// The others in the local player's party, from the newest snapshot.
    pub fn party(&self) -> &[NetId] {
        self.snapshots.back().map_or(&[], |s| &s.party)
    }

    /// Day and hour from the newest snapshot.
    pub fn clock(&self) -> Option<(u32, f32)> {
        self.snapshots.back().map(|s| (s.day, s.hour))
    }

    /// Advances real time: network in, predicted ticks, inputs out. `input`'s movement and run
    /// are held this frame; its presses latch until a tick consumes them.
    pub fn update(&mut self, dt: Duration, input: TickInput) {
        self.presses.latch(input);
        self.net.update(dt);
        while let Some(bytes) = self.net.receive(Channel::State) {
            match decode::<Snapshot>(&bytes) {
                Some(snapshot) => self.on_snapshot(snapshot),
                None => tracing::warn!("host sent an undecodable snapshot"),
            }
        }
        // Nothing is sent on this channel yet; drain it so the inbox cannot grow.
        while self.net.receive(Channel::Events).is_some() {}

        let ticks = self.timestep.accumulate(dt.mul_f64(self.pace));
        if self.net.status() == ClientStatus::InGame && ticks > 0 {
            for _ in 0..ticks {
                let input = TickInput {
                    movement: input.movement,
                    run: input.run,
                    ..self.presses.take_presses()
                };
                self.seq += 1;
                self.pending.push_back((self.seq, input));
                if self.pending.len() > MAX_PENDING {
                    self.pending.pop_front();
                }
                self.predict(input);
            }
            let from = self.pending.len().saturating_sub(INPUT_REDUNDANCY);
            let inputs = self.pending.iter().skip(from).copied().collect();
            self.net
                .send(Channel::State, encode(&ClientInputs { inputs }));
        }
        self.net.send_packets();

        // Keep the interpolation clock just behind the newest snapshot, drifting gently.
        if let Some(newest) = self.newest {
            let target = newest as f64 - INTERPOLATION_DELAY;
            self.render_tick += dt.as_secs_f64() * f64::from(dark_core::DEFAULT_TICK_RATE);
            let behind = target - self.render_tick;
            if behind.abs() > 12.0 {
                self.render_tick = target;
            } else {
                self.render_tick += behind * 0.1;
            }
        }
        if let Some(me) = &mut self.me {
            me.smoothing *= (-15.0 * dt.as_secs_f32()).exp();
        }
    }

    fn predict(&mut self, input: TickInput) {
        let dt = self.timestep.step().as_secs_f32();
        // Made land is shaped here exactly as the host shapes it, from the map's own seed, or
        // this player would be predicting against level ground where the host has a hill
        // (docs/PLAN.md §24.4). The same numbers on both sides, so the same land.
        if let Some(me) = &self.me {
            let map = me.map;
            let at = me.body.position;
            if let Some(map) = self.maps.maps.get_mut(map.0 as usize)
                && let Some(land) = map.land
            {
                crate::land::shape_around(&mut map.collision.terrain, &land, at);
                // And let go of it on the same terms as the host: a client walks as far as a host
                // does, and holds the ground it walked over just as long.
                crate::land::forget_far_from(&mut map.collision.terrain, &[at]);
            }
        }
        let Some(me) = &mut self.me else {
            return;
        };
        me.previous = (me.body.position, me.body.elevation);
        let intent = control(&mut me.state, me.body.grounded, input, &self.sheets);
        let world = &self.maps.get(me.map).collision;
        world.step(
            &mut me.body,
            intent.velocity,
            intent.jump,
            dt,
            &self.maps.params,
        );
        // Made room for exactly as the host makes it: pushed back at most as far as this tick
        // moved, against the others where the host last showed them. Nobody in a doorway takes
        // part, on either side, as on the host.
        let travelled = me.body.position.distance(me.previous.0);
        let (maps, map) = (&self.maps, me.map);
        if !crate::crowd::in_a_doorway(maps, map, me.body.position) {
            let others = self
                .crowd
                .iter()
                .filter(|standing| !crate::crowd::in_a_doorway(maps, map, standing.position))
                .copied();
            crate::crowd::make_room(me.id, &mut me.body, travelled, world, &maps.params, others);
        }
    }

    fn on_snapshot(&mut self, mut snapshot: Snapshot) {
        // Unreliable delivery can reorder; anything older than what we have is stale.
        if self.newest.is_some_and(|n| snapshot.tick <= n) {
            return;
        }
        if self.newest.is_none() {
            self.render_tick = snapshot.tick as f64 - INTERPOLATION_DELAY;
        }
        self.newest = Some(snapshot.tick);
        // An empty queue means the host is waiting on us (it holds our character still); a long
        // one is latency we added. Aim for one or two inputs of slack.
        self.pace = match snapshot.queued {
            0 => 1.0 + PACE_ADJUST,
            1..=3 => 1.0,
            _ => 1.0 - PACE_ADJUST,
        };

        // Everyone else as of this snapshot, before the replay below predicts against them, in
        // `NetId` order so the sums are added up as the host adds them.
        self.crowd = snapshot
            .characters
            .iter()
            .filter(|c| Some(c.id) != snapshot.you && !c.state.fighter.is_dead())
            .map(|c| crate::crowd::Standing::of(c.id, &c.body))
            .collect();
        self.crowd.sort_by_key(|standing| standing.who.0);

        let own = snapshot
            .you
            .and_then(|you| snapshot.characters.iter().find(|c| c.id == you))
            .cloned();
        if let Some(own) = own {
            // Rewind to the host's state as of the last input it applied, then replay the rest.
            let before = self
                .me
                .as_ref()
                .map(|m| (m.map, m.body.position + m.smoothing));
            let smoothing_before = self.me.as_ref().map_or(Vec2::ZERO, |m| m.smoothing);
            self.pending.retain(|(seq, _)| *seq > snapshot.acked);
            self.me = Some(Predicted {
                id: own.id,
                map: snapshot.map,
                body: own.body,
                state: own.state,
                previous: (own.body.position, own.body.elevation),
                smoothing: Vec2::ZERO,
                speech: own.speech,
                hostile: own.hostile,
            });
            let replay: Vec<TickInput> = self.pending.iter().map(|(_, i)| *i).collect();
            for input in replay {
                self.predict(input);
            }
            if let (Some((map, shown)), Some(me)) = (before, &mut self.me) {
                let error = shown - me.body.position;
                if map == me.map {
                    self.last_correction = (error - smoothing_before).length();
                    if error.length() <= MAX_SMOOTHED_ERROR {
                        me.smoothing = error;
                    }
                } else {
                    // A map change moves the character on purpose; that is not a misprediction.
                    self.last_correction = 0.0;
                }
            }
        }

        if let Some(ties) = snapshot.ties.take() {
            self.ties = ties;
        }
        self.snapshots.push_back(snapshot);
        while self.snapshots.len() > 32 {
            self.snapshots.pop_front();
        }
    }

    /// Everyone in the local player's map: the local player predicted, others interpolated.
    pub fn characters(&self) -> Vec<DrawCharacter> {
        let mut out = Vec::new();
        let Some(me) = &self.me else {
            return out;
        };
        let alpha = self.timestep.alpha();
        let ground = me.previous.0.lerp(me.body.position, alpha) + me.smoothing;
        let elevation = me.previous.1 + (me.body.elevation - me.previous.1) * alpha;
        out.push(DrawCharacter {
            id: me.id,
            ground,
            elevation,
            body: me.body,
            state: me.state,
            you: true,
            npc: false,
            hostile: me.hostile,
            speech: me.speech.clone(),
        });

        // The two snapshots around the render time, in this map.
        let in_map: Vec<&Snapshot> = self.snapshots.iter().filter(|s| s.map == me.map).collect();
        let Some(&newest) = in_map.last() else {
            return out;
        };
        let (a, b) = match in_map
            .windows(2)
            .find(|w| (w[1].tick as f64) >= self.render_tick)
        {
            Some(w) => (w[0], w[1]),
            None => (newest, newest),
        };
        let span = (b.tick - a.tick) as f64;
        let t = if span > 0.0 {
            ((self.render_tick - a.tick as f64) / span).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        for later in b.characters.iter().filter(|c| c.id != me.id) {
            let earlier = a
                .characters
                .iter()
                .find(|c| c.id == later.id)
                .unwrap_or(later);
            out.push(DrawCharacter {
                id: later.id,
                ground: earlier.body.position.lerp(later.body.position, t),
                elevation: earlier.body.elevation
                    + (later.body.elevation - earlier.body.elevation) * t,
                body: later.body,
                state: later.state,
                you: false,
                npc: later.npc,
                hostile: later.hostile,
                speech: later.speech.clone(),
            });
        }
        out
    }
}
