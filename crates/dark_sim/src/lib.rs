//! The world simulation (docs/PLAN.md §4.5, §12): what happens across the whole world over the
//! year, including everywhere no player is looking.
//!
//! Headless and deterministic: a [`WorldDef`] and a seed give the same year on every machine,
//! so a year can be fast-forwarded in well under a second for balancing (`dark-cli simulate`)
//! and a saved world replays exactly. The simulation steps in whole in-game hours; the host
//! keeps it in step with its clock and hands it what players do that matters to the world
//! (where they are, who they kill).
//!
//! In this prototype the world follows one party through the year: the hero party, directed by
//! an HTN planner ([`director`]), travelling a graph of regions, fighting statistically
//! ([`battle`]) and passing titles on by succession rules ([`World::die`]). Beside it, players
//! lead parties of their own, with standing, loyalty and betrayal ([`social`]).

pub mod battle;
mod chronicle;
pub mod def;
pub mod director;
pub mod htn;
mod rng;
pub mod social;
pub mod world;

use serde::{Deserialize, Serialize};

pub use def::{CalendarDef, DefError, EventDef, SeasonDef, Social, Tuning, WorldDef};
pub use director::{HeroParty, Location, Step, YEAR_HOURS};
pub use rng::Rng;
pub use social::{Parting, Party, Refusal};
pub use world::{
    Actor, ActorId, FactionId, Happening, Outcome, Region, RegionId, TitleId, World, WorldEvent,
};

/// The year begins at this hour of day 0, like `dark_time::GameClock`.
pub const START_HOUR: u32 = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSim {
    world: World,
    party: HeroParty,
    tuning: Tuning,
    #[serde(default)]
    social: Social,
    /// Players' parties.
    #[serde(default)]
    society: social::Society,
    /// Players arrive in this faction.
    player_faction: world::FactionId,
    rng: Rng,
    /// Hours since midnight before the first day.
    hour: u32,
    outcome: Option<Outcome>,
    /// Happened since the last [`WorldSim::take_events`].
    events: Vec<WorldEvent>,
    /// The project's calendar: always the current `world.ron`'s, never a save's (see
    /// [`WorldSim::set_calendar`]).
    #[serde(skip)]
    calendar: CalendarDef,
}

impl WorldSim {
    pub fn new(def: &WorldDef, seed: u64) -> Result<Self, DefError> {
        let world = World::build(def)?;
        let party = HeroParty::build(&def.hero_party, &world)?;
        let player_faction = match &def.player_faction {
            Some(id) => world
                .faction(id)
                .ok_or_else(|| DefError::Invalid(format!("unknown player faction {id}")))?,
            None => (0..world.factions.len())
                .map(|i| world::FactionId(i as u16))
                .find(|f| !world.factions[usize::from(f.0)].hostile)
                .ok_or_else(|| DefError::Invalid("no friendly faction for players".into()))?,
        };
        def.calendar.validate()?;
        Ok(Self {
            world,
            party,
            tuning: def.tuning.clone(),
            social: def.social.clone(),
            society: social::Society::default(),
            player_faction,
            rng: Rng::new(seed),
            hour: START_HOUR,
            outcome: None,
            events: Vec::new(),
            calendar: def.calendar.clone(),
        })
    }

    pub fn calendar(&self) -> &CalendarDef {
        &self.calendar
    }

    /// The calendar to follow: a world carried on from a save follows the project's as it is
    /// now, not as it was.
    pub fn set_calendar(&mut self, calendar: CalendarDef) {
        self.calendar = calendar;
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn party(&self) -> &HeroParty {
        &self.party
    }

    pub fn tuning(&self) -> &Tuning {
        &self.tuning
    }

    pub fn social(&self) -> &Social {
        &self.social
    }

    /// The social party `actor` is in.
    pub fn party_of(&self, actor: ActorId) -> Option<&Party> {
        self.society.party_of(actor)
    }

    pub fn parties(&self) -> &[Party] {
        &self.society.parties
    }

    /// Hours since midnight before the first day.
    pub fn hour(&self) -> u32 {
        self.hour
    }

    pub fn day(&self) -> u32 {
        self.hour / 24
    }

    pub fn is_year_over(&self) -> bool {
        self.hour >= YEAR_HOURS
    }

    /// Set once the goal falls, or when the year ends without that.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// Runs whole hours until `hour` (or the end of the year).
    pub fn advance_to(&mut self, hour: u32) {
        while self.hour < hour.min(YEAR_HOURS) {
            self.step();
        }
    }

    pub fn advance_hours(&mut self, hours: u32) {
        self.advance_to(self.hour.saturating_add(hours));
    }

    /// Fast-forward to the end of the year.
    pub fn run_year(&mut self) {
        self.advance_to(YEAR_HOURS);
    }

    /// What happened since the last call.
    pub fn take_events(&mut self) -> Vec<WorldEvent> {
        std::mem::take(&mut self.events)
    }

    fn step(&mut self) {
        let events = self
            .party
            .hour(&mut self.world, &mut self.rng, &self.tuning, self.hour);
        self.events.extend(events);
        let buried = self.society.bury(&mut self.world, self.hour);
        self.events.extend(buried);
        if self.hour.is_multiple_of(24) {
            let day = self.society.day(&mut self.world, &self.social, self.hour);
            self.events.extend(day);
        }
        if self.outcome.is_none() && !self.world.get(self.party.goal).alive {
            self.outcome = Some(Outcome::GoalDefeated { day: self.day() });
        }
        self.hour += 1;
        if self.hour == YEAR_HOURS {
            let outcome = *self.outcome.get_or_insert(Outcome::GoalSurvived);
            self.events.push(WorldEvent {
                hour: self.hour,
                what: Happening::YearEnded { outcome },
            });
        }
    }

    /// The actor playing as `player`, added (as a commoner of the players' faction, at `region`)
    /// the first time.
    pub fn player_actor(&mut self, player: u128, region: RegionId) -> ActorId {
        if let Some(i) = self
            .world
            .actors
            .iter()
            .position(|a| a.player == Some(player))
        {
            return ActorId(i as u16);
        }
        let faction = self.player_faction;
        let standing = self.world.first_standing(faction, &self.social);
        let id = ActorId(self.world.actors.len() as u16);
        self.world.actors.push(Actor {
            id: format!("player:{player:032x}"),
            name: String::new(),
            role: "commoner".into(),
            faction,
            power: 30,
            health: 100,
            alive: true,
            region,
            boss: false,
            player: Some(player),
            online: true,
            standing,
            trust: 0,
            in_party: false,
            outlaw: false,
        });
        id
    }

    /// Which players are connected now; every other player's character is offline.
    pub fn set_online(&mut self, online: &[ActorId]) {
        for (i, actor) in self.world.actors.iter_mut().enumerate() {
            if actor.player.is_some() {
                actor.online = online.contains(&ActorId(i as u16));
            }
        }
    }

    /// Where a player's character is now.
    pub fn move_player(&mut self, actor: ActorId, region: RegionId) {
        let a = self.world.get_mut(actor);
        if a.player.is_some() {
            a.region = region;
        }
    }

    /// The regions players are in; the full simulation runs those.
    pub fn set_detailed(&mut self, regions: &[RegionId]) {
        for (i, region) in self.world.regions.iter_mut().enumerate() {
            region.detailed = regions.contains(&RegionId(i as u16));
        }
    }

    /// Someone died in the full simulation (a player killed the Hero, say); titles pass on, and
    /// the killer's standing changes (see [`Social`]).
    pub fn kill(&mut self, victim: ActorId, killer: Option<ActorId>) {
        if !self.world.get(victim).alive {
            return;
        }
        if let Some(killer) = killer {
            let faction = self.world.get(victim).faction;
            self.world
                .adjust_standing(killer, faction, -self.social.kill_penalty);
            if self.world.is_hostile(faction) {
                self.slain_enemy(killer);
            } else {
                // Murder: every other friendly faction thinks less of the killer.
                for i in 0..self.world.factions.len() {
                    let other = FactionId(i as u16);
                    if other != faction && !self.world.is_hostile(other) {
                        self.world
                            .adjust_standing(killer, other, -self.social.murder_penalty);
                    }
                }
            }
        }
        let events = self.world.die(victim, killer, self.hour);
        self.events.extend(events);
        let buried = self.society.bury(&mut self.world, self.hour);
        self.events.extend(buried);
    }

    /// `actor`'s standing with `faction` moves by `by` (a story choice, say).
    pub fn adjust_standing(&mut self, actor: ActorId, faction: FactionId, by: i32) {
        self.world.adjust_standing(actor, faction, by);
    }

    /// `actor` slew one of the hostile side (a monster in play, say): every friendly faction
    /// thinks better of them.
    pub fn slain_enemy(&mut self, actor: ActorId) {
        for i in 0..self.world.factions.len() {
            let faction = FactionId(i as u16);
            if !self.world.is_hostile(faction) {
                self.world
                    .adjust_standing(actor, faction, self.social.slay_reward);
            }
        }
    }

    /// `leader` asks `actor` to follow them. The leader's party is formed if they have none.
    pub fn ask_to_join(&mut self, leader: ActorId, actor: ActorId) -> Result<(), Refusal> {
        let events = self.society.ask(
            &mut self.world,
            &self.social,
            &self.party.members,
            leader,
            actor,
            self.hour,
        )?;
        self.events.extend(events);
        Ok(())
    }

    /// Why `actor` would not follow `leader` if asked now (`Ok` if it would).
    pub fn would_follow(&self, leader: ActorId, actor: ActorId) -> Result<(), Refusal> {
        self.society.check(
            &self.world,
            &self.social,
            &self.party.members,
            leader,
            actor,
        )
    }

    /// Players `a` and `b` both agreed to travel together.
    pub fn join_players(&mut self, a: ActorId, b: ActorId) -> Result<(), Refusal> {
        let events = self
            .society
            .join_players(&mut self.world, &self.social, a, b, self.hour)?;
        self.events.extend(events);
        Ok(())
    }

    /// `actor` leaves its party.
    pub fn leave_party(&mut self, actor: ActorId) {
        let events = self
            .society
            .part(&mut self.world, actor, Parting::Left, self.hour);
        self.events.extend(events);
    }

    /// The leader of `actor`'s party sends it away. False if `leader` does not lead it.
    pub fn dismiss(&mut self, leader: ActorId, actor: ActorId) -> bool {
        let leads = self
            .society
            .party_of(actor)
            .is_some_and(|p| p.leader == leader && actor != leader);
        if leads {
            let events = self
                .society
                .part(&mut self.world, actor, Parting::Dismissed, self.hour);
            self.events.extend(events);
        }
        leads
    }

    /// How `actor`'s party leader behaved in front of them moves their loyalty by `delta` (the
    /// game decides what counts); they may desert or betray.
    pub fn sway(&mut self, actor: ActorId, delta: i32) {
        let events = self
            .society
            .sway(&mut self.world, &self.social, actor, delta, self.hour);
        self.events.extend(events);
    }

    /// `actor` turns on its party.
    pub fn betray(&mut self, actor: ActorId) {
        let events = self
            .society
            .betray(&mut self.world, &self.social, actor, self.hour);
        self.events.extend(events);
    }

    /// `actor` goes over to `faction`. Going over to the other side makes an enemy of the old
    /// one: standing there drops to the enemy standing, titles that side appointed pass on, and
    /// a party of the old side is betrayed.
    pub fn defect(&mut self, actor: ActorId, faction: FactionId) {
        let old = self.world.get(actor).faction;
        if old == faction {
            return;
        }
        let crossing = self.world.is_hostile(old) != self.world.is_hostile(faction);
        self.world.get_mut(actor).faction = faction;
        let own = self.social.own_standing;
        let now = self.world.standing(actor, faction);
        self.world.adjust_standing(actor, faction, own - now);
        self.events.push(WorldEvent {
            hour: self.hour,
            what: Happening::Defected { actor, faction },
        });
        if !crossing {
            return;
        }
        for i in 0..self.world.factions.len() {
            let other = FactionId(i as u16);
            if self.world.is_hostile(other) != self.world.is_hostile(faction) {
                let s = self.world.standing(actor, other);
                let enemy = self.social.enemy_standing;
                if s > enemy {
                    self.world.adjust_standing(actor, other, enemy - s);
                }
            }
        }
        let stripped = self.world.strip_titles(actor, self.hour);
        self.events.extend(stripped);
        let turned_on_party = self.society.party_of(actor).is_some_and(|p| {
            p.members.iter().any(|&m| {
                m != actor
                    && self.world.is_hostile(self.world.get(m).faction)
                        != self.world.is_hostile(faction)
            })
        });
        if turned_on_party {
            self.betray(actor);
        }
        // The hero party does not keep a traitor.
        self.party.members.retain(|&m| m != actor);
    }

    /// A player accepts a place in the hero party.
    pub fn join_hero_party(&mut self, actor: ActorId) -> bool {
        if !self.world.get(actor).alive || self.party.members.contains(&actor) {
            return false;
        }
        self.party.members.push(actor);
        self.events.push(WorldEvent {
            hour: self.hour,
            what: Happening::Joined { actor },
        });
        true
    }

    /// The whole state as RON: a save that replays exactly.
    pub fn save(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    pub fn load(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }

    /// One line of chronicle for `event`, in English (a developer tool; players see storylets).
    /// `text` turns name keys into names.
    pub fn describe(&self, event: &WorldEvent, text: &dyn Fn(&str) -> String) -> String {
        chronicle::describe(&self.world, self.party.goal, event, text)
    }
}

#[cfg(test)]
mod tests;
