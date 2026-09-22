//! Standing, parties and loyalty (docs/PLAN.md §4.4, §4.5, §15).
//!
//! Every actor has a standing with every faction (how that faction regards them, −1000 to
//! 1000). Killing costs standing with the victim's faction, murdering a friendly person costs it
//! with every friendly faction, and slaying the demon army earns it. An actor follows someone
//! only if the leader's standing with its faction is at least its `trust`.
//!
//! Parties other than the directed hero party are social: a player leads, players and NPCs
//! follow (at most 8, at most 4 players). NPC followers have loyalty, which grows a little each
//! day, sinks while the leader is in bad standing with their faction, and moves with how the
//! leader behaves in front of them (the game reports that). A follower whose loyalty falls
//! below `desert_below` leaves; below `betray_below` it turns on the party and is an outlaw from
//! then on. A follower who left is remembered: asked again it comes back as loyal as it left (or
//! not at all, if it left unhappy, until that fades). Anyone may betray a party or defect to
//! another faction; defecting to a hostile one makes an enemy of every friendly faction and costs
//! any title those factions appointed.

use serde::{Deserialize, Serialize};

use crate::def::Social;
use crate::world::{ActorId, FactionId, Happening, World, WorldEvent};

/// A party of players and NPCs, led by `leader` (also its first member).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Party {
    pub leader: ActorId,
    pub members: Vec<ActorId>,
    /// NPC members' loyalty to the party.
    loyalty: Vec<(ActorId, i32)>,
}

impl Party {
    /// An NPC member's loyalty; `None` for players and non-members.
    pub fn loyalty(&self, actor: ActorId) -> Option<i32> {
        self.loyalty
            .iter()
            .find(|(a, _)| *a == actor)
            .map(|(_, l)| *l)
    }

    fn players(&self, world: &World) -> usize {
        self.members
            .iter()
            .filter(|&&m| world.get(m).player.is_some())
            .count()
    }
}

/// Why someone would not join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// Dead, or not a person who follows anyone (a boss, a player asked like an NPC).
    Unable,
    /// Already in a party (or the hero party).
    Busy,
    /// Only a party's leader brings people in.
    NotLeader,
    /// Not in the same place.
    Away,
    /// On the other side: one of them serves a hostile faction.
    Enemy,
    /// The leader's standing with their faction is below their trust.
    Distrust,
    /// Left this leader's party unhappy, not long ago, and has not forgotten.
    Grudge,
    Full,
}

/// How someone left a party.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parting {
    Left,
    Dismissed,
    /// Loyalty ran out.
    Deserted,
    /// Turned on the party.
    Betrayed,
    /// No player was left to lead it.
    Disbanded,
}

/// The parties, with the rules they follow. Owned by [`crate::WorldSim`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Society {
    pub parties: Vec<Party>,
    /// Loyalty NPCs had when they last left a party: asked again, they pick up where they were
    /// (it drifts back up a day at a time until forgotten), so sending a follower away and asking
    /// again mends nothing.
    #[serde(default)]
    memory: Vec<(ActorId, i32)>,
}

impl Society {
    pub fn party_of(&self, actor: ActorId) -> Option<&Party> {
        self.parties.iter().find(|p| p.members.contains(&actor))
    }

    fn index_of(&self, actor: ActorId) -> Option<usize> {
        self.parties.iter().position(|p| p.members.contains(&actor))
    }

    /// `leader` asks NPC `actor` to follow them.
    pub(crate) fn ask(
        &mut self,
        world: &mut World,
        rules: &Social,
        hero_party: &[ActorId],
        leader: ActorId,
        actor: ActorId,
        hour: u32,
    ) -> Result<Vec<WorldEvent>, Refusal> {
        self.check(world, rules, hero_party, leader, actor)?;
        let regard = world.regard(actor, leader);
        let remembered = self
            .memory
            .iter()
            .find(|(m, _)| *m == actor)
            .map(|(_, l)| *l);
        let party = match self.index_of(leader) {
            Some(i) => i,
            None => {
                self.parties.push(Party {
                    leader,
                    members: vec![leader],
                    loyalty: Vec::new(),
                });
                self.parties.len() - 1
            }
        };
        let p = &mut self.parties[party];
        p.members.push(actor);
        let fresh = (rules.loyalty_start + regard / 2).clamp(-1000, 1000);
        p.loyalty
            .push((actor, remembered.map_or(fresh, |l| l.min(fresh))));
        self.memory.retain(|(m, _)| *m != actor);
        world.sync_parties(&self.parties);
        Ok(vec![joined(hour, actor, leader)])
    }

    /// Why NPC `actor` would not follow `leader` now, if it would not.
    pub(crate) fn check(
        &self,
        world: &World,
        rules: &Social,
        hero_party: &[ActorId],
        leader: ActorId,
        actor: ActorId,
    ) -> Result<(), Refusal> {
        let (l, a) = (world.get(leader), world.get(actor));
        let unable = !l.alive || l.player.is_none() || !a.alive || a.boss || a.player.is_some();
        // The demon army serves its master, not a party (its people make pacts; they do not
        // travel as companions).
        if unable || leader == actor || world.is_hostile(a.faction) {
            return Err(Refusal::Unable);
        }
        if self.index_of(actor).is_some() || hero_party.contains(&actor) {
            return Err(Refusal::Busy);
        }
        if l.region != a.region {
            return Err(Refusal::Away);
        }
        if world.is_hostile(l.faction) != world.is_hostile(a.faction) || a.outlaw {
            return Err(Refusal::Enemy);
        }
        let regard = world.regard(actor, leader);
        if regard < a.trust {
            return Err(Refusal::Distrust);
        }
        let remembered = self
            .memory
            .iter()
            .find(|(m, _)| *m == actor)
            .map(|(_, l)| *l);
        if remembered.is_some_and(|l| l < rules.desert_below) {
            return Err(Refusal::Grudge);
        }
        if let Some(i) = self.index_of(leader) {
            let p = &self.parties[i];
            if p.leader != leader {
                return Err(Refusal::NotLeader);
            }
            if p.members.len() >= rules.max_party {
                return Err(Refusal::Full);
            }
        }
        Ok(())
    }

    /// Players `a` and `b` agree to travel together: into whichever party either leads or is in
    /// (a's first), merging two parties into a's.
    pub(crate) fn join_players(
        &mut self,
        world: &mut World,
        rules: &Social,
        a: ActorId,
        b: ActorId,
        hour: u32,
    ) -> Result<Vec<WorldEvent>, Refusal> {
        let players = |id: ActorId| world.get(id).player.is_some() && world.get(id).alive;
        if a == b || !players(a) || !players(b) {
            return Err(Refusal::Unable);
        }
        if world.is_hostile(world.get(a).faction) != world.is_hostile(world.get(b).faction) {
            return Err(Refusal::Enemy);
        }
        let (pa, pb) = (self.index_of(a), self.index_of(b));
        if pa.is_some() && pa == pb {
            return Ok(Vec::new());
        }
        let joining: Vec<ActorId> = match pb {
            Some(i) => self.parties[i].members.clone(),
            None => vec![b],
        };
        let (size, player_count) = match pa {
            Some(i) => (
                self.parties[i].members.len(),
                self.parties[i].players(world),
            ),
            None => (1, 1),
        };
        let joining_players = joining
            .iter()
            .filter(|&&m| world.get(m).player.is_some())
            .count();
        if size + joining.len() > rules.max_party
            || player_count + joining_players > rules.max_players
        {
            return Err(Refusal::Full);
        }
        let loyalty = pb
            .map(|i| self.parties[i].loyalty.clone())
            .unwrap_or_default();
        if let Some(i) = pb {
            self.parties.remove(i);
        }
        let party = match self.index_of(a) {
            Some(i) => i,
            None => {
                self.parties.push(Party {
                    leader: a,
                    members: vec![a],
                    loyalty: Vec::new(),
                });
                self.parties.len() - 1
            }
        };
        let leader = self.parties[party].leader;
        let p = &mut self.parties[party];
        p.members.extend(&joining);
        p.loyalty.extend(loyalty);
        world.sync_parties(&self.parties);
        Ok(joining
            .into_iter()
            .map(|m| joined(hour, m, leader))
            .collect())
    }

    /// `actor` leaves its party (`how`). A leader who leaves hands the party to the next player;
    /// a party left with no player breaks up.
    pub(crate) fn part(
        &mut self,
        world: &mut World,
        actor: ActorId,
        how: Parting,
        hour: u32,
    ) -> Vec<WorldEvent> {
        let mut events = Vec::new();
        let Some(i) = self.index_of(actor) else {
            return events;
        };
        let p = &mut self.parties[i];
        let leader = p.leader;
        p.members.retain(|&m| m != actor);
        if let Some(at) = p.loyalty.iter().position(|(m, _)| *m == actor) {
            let (_, loyalty) = p.loyalty.remove(at);
            self.memory.retain(|(m, _)| *m != actor);
            self.memory.push((actor, loyalty));
        }
        let p = &mut self.parties[i];
        events.push(parted(hour, actor, leader, how));
        let next_player = p
            .members
            .iter()
            .copied()
            .find(|&m| world.get(m).player.is_some());
        match next_player {
            Some(player) if p.members.len() > 1 => p.leader = player,
            // Nobody left to lead, or a party of one: it breaks up.
            _ => {
                let rest = std::mem::take(&mut p.members);
                let loyalty = std::mem::take(&mut p.loyalty);
                self.parties.remove(i);
                for (m, l) in loyalty {
                    self.memory.retain(|(r, _)| *r != m);
                    self.memory.push((m, l));
                }
                events.extend(
                    rest.into_iter()
                        .filter(|&m| world.get(m).player.is_none())
                        .map(|m| parted(hour, m, leader, Parting::Disbanded)),
                );
            }
        }
        world.sync_parties(&self.parties);
        events
    }

    /// Loyalty of NPC follower `actor` moves by `delta`; it may desert or betray.
    pub(crate) fn sway(
        &mut self,
        world: &mut World,
        rules: &Social,
        actor: ActorId,
        delta: i32,
        hour: u32,
    ) -> Vec<WorldEvent> {
        let Some(i) = self.index_of(actor) else {
            return Vec::new();
        };
        let Some(slot) = self.parties[i]
            .loyalty
            .iter_mut()
            .find(|(a, _)| *a == actor)
        else {
            return Vec::new();
        };
        slot.1 = (slot.1 + delta).clamp(-1000, 1000);
        let loyalty = slot.1;
        if loyalty < rules.betray_below {
            self.betray(world, rules, actor, hour)
        } else if loyalty < rules.desert_below {
            self.part(world, actor, Parting::Deserted, hour)
        } else {
            Vec::new()
        }
    }

    /// `actor` turns on its party: it leaves, and every faction in the party holds it against
    /// them.
    pub(crate) fn betray(
        &mut self,
        world: &mut World,
        rules: &Social,
        actor: ActorId,
        hour: u32,
    ) -> Vec<WorldEvent> {
        let Some(party) = self.party_of(actor) else {
            return Vec::new();
        };
        let leader = party.leader;
        let mut wronged: Vec<FactionId> = party
            .members
            .iter()
            .filter(|&&m| m != actor)
            .map(|&m| world.get(m).faction)
            .collect();
        // Once per faction, however many of it were betrayed.
        wronged.sort_unstable();
        wronged.dedup();
        if world.get(actor).player.is_none() {
            world.get_mut(actor).outlaw = true;
        }
        for faction in wronged {
            world.adjust_standing(actor, faction, -rules.betrayal_penalty);
        }
        let mut events = vec![WorldEvent {
            hour,
            what: Happening::Betrayed { actor, leader },
        }];
        events.extend(self.part(world, actor, Parting::Betrayed, hour));
        events
    }

    /// Once a day: followers grow closer, or cool towards a leader their faction dislikes.
    pub(crate) fn day(&mut self, world: &mut World, rules: &Social, hour: u32) -> Vec<WorldEvent> {
        let followers: Vec<(ActorId, ActorId)> = self
            .parties
            .iter()
            .flat_map(|p| p.loyalty.iter().map(move |(a, _)| (*a, p.leader)))
            .collect();
        let mut events = Vec::new();
        for (actor, leader) in followers {
            let regard = world.regard(actor, leader);
            let delta = rules.loyalty_per_day + (regard / 10).min(0);
            events.extend(self.sway(world, rules, actor, delta, hour));
        }
        // Old partings fade.
        for (_, loyalty) in &mut self.memory {
            *loyalty += rules.loyalty_per_day;
        }
        self.memory.retain(|(_, l)| *l < rules.loyalty_start);
        events
    }

    /// The dead leave their parties.
    pub(crate) fn bury(&mut self, world: &mut World, hour: u32) -> Vec<WorldEvent> {
        let dead: Vec<ActorId> = self
            .parties
            .iter()
            .flat_map(|p| p.members.iter().copied())
            .filter(|&m| !world.get(m).alive)
            .collect();
        let mut events = Vec::new();
        for actor in dead {
            // Death is its own event; only what it does to the party is reported here.
            let mut parted = self.part(world, actor, Parting::Left, hour);
            parted.retain(|e| {
                !matches!(e.what, Happening::LeftParty { actor: a, how: Parting::Left, .. } if a == actor)
            });
            events.extend(parted);
        }
        events
    }
}

fn joined(hour: u32, actor: ActorId, leader: ActorId) -> WorldEvent {
    WorldEvent {
        hour,
        what: Happening::JoinedParty { actor, leader },
    }
}

fn parted(hour: u32, actor: ActorId, leader: ActorId, how: Parting) -> WorldEvent {
    WorldEvent {
        hour,
        what: Happening::LeftParty { actor, leader, how },
    }
}
