//! The world director's hero party: the year-long plan of the party that sets out to defeat the
//! goal (the Demon Lord), expressed as an HTN domain and carried out an hour at a time.
//!
//! Its reasoning, in priority order: carry on after winning; wait if nobody is left; follow a
//! player who took the lead; bring the leader and every role into the party; recover from
//! wounds; march on the goal when the year is running out or the party is strong enough; defeat
//! the lieutenants in the way, weakest first; otherwise train. It plans again whenever its plan
//! runs out and every morning between journeys, so deaths, lost titles and new members change
//! the plan within the day.

use serde::{Deserialize, Serialize};

use crate::battle;
use crate::def::{HeroPartyDef, Tuning};
use crate::htn::{self, Domain, Method, Task};
use crate::rng::Rng;
use crate::world::{ActorId, Happening, RegionId, TitleId, World, WorldEvent};

/// The year, in hours.
pub const YEAR_HOURS: u32 = dark_time::DAYS_PER_YEAR * 24;

/// Where the party is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    At(RegionId),
    /// On the road from one region to the next.
    Road {
        from: RegionId,
        to: RegionId,
        done: u32,
        total: u32,
    },
}

/// A primitive step of the party's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    Travel {
        to: RegionId,
    },
    /// Heal until everyone is nearly whole.
    Rest,
    /// Train until the party's strength reaches `until` (the morning replan rethinks it daily).
    Train {
        until: u64,
    },
    Recruit {
        actor: ActorId,
    },
    Fight {
        boss: ActorId,
    },
    Wait {
        until: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeroParty {
    pub leader_title: TitleId,
    pub roles: Vec<String>,
    pub members: Vec<ActorId>,
    pub goal: ActorId,
    pub lieutenants: Vec<ActorId>,
    pub location: Location,
    pub plan: Vec<Step>,
    /// What the party is doing, as the last plan's reasoning.
    pub intent: String,
    /// The goal is weakened by this percentage (defeated lieutenants).
    pub goal_weakened: u32,
    trained_hours: u32,
    /// Where the current journey ends, to announce each journey once.
    journey: Option<RegionId>,
    /// Players already told they could fill a role, so they are asked once.
    invited: Vec<ActorId>,
    fell: bool,
}

impl HeroParty {
    pub fn build(def: &HeroPartyDef, world: &World) -> Result<Self, crate::DefError> {
        let bad = |m: String| crate::DefError::Invalid(m);
        let actor = |id: &str| {
            world
                .actor(id)
                .ok_or_else(|| bad(format!("hero party names unknown actor {id}")))
        };
        let leader_title = world
            .title(&def.leader_title)
            .ok_or_else(|| bad(format!("unknown leader title {}", def.leader_title)))?;
        let members = def
            .members
            .iter()
            .map(|m| actor(m))
            .collect::<Result<Vec<_>, _>>()?;
        let goal = actor(&def.goal)?;
        let lieutenants = def
            .lieutenants
            .iter()
            .map(|m| actor(m))
            .collect::<Result<Vec<_>, _>>()?;
        let start = members
            .first()
            .map(|&m| world.get(m).region)
            .ok_or_else(|| bad("the hero party starts with no members".into()))?;
        if members.iter().any(|&m| world.get(m).region != start) {
            return Err(bad(
                "the hero party's members must start in one region".into()
            ));
        }
        for &boss in std::iter::once(&goal).chain(&lieutenants) {
            if world.path(start, world.get(boss).region).is_none() {
                return Err(bad(format!(
                    "no road from the party's start to {}",
                    world.get(boss).id
                )));
            }
        }
        Ok(Self {
            leader_title,
            roles: def.roles.clone(),
            members,
            goal,
            lieutenants,
            location: Location::At(start),
            plan: Vec::new(),
            intent: String::new(),
            goal_weakened: 0,
            trained_hours: 0,
            journey: None,
            invited: Vec::new(),
            fell: false,
        })
    }

    /// The region the party is in, or last left.
    pub fn region(&self) -> RegionId {
        match self.location {
            Location::At(r) => r,
            Location::Road { from, .. } => from,
        }
    }

    /// The members the director moves and whose fights it resolves: living NPCs. Players are
    /// never part of a statistical fight (or trained, or rested); they fight where they are, in
    /// the full simulation.
    pub fn present(&self, world: &World) -> Vec<ActorId> {
        self.members
            .iter()
            .copied()
            .filter(|&m| {
                let a = world.get(m);
                a.alive && a.player.is_none()
            })
            .collect()
    }

    pub fn leader(&self, world: &World) -> Option<ActorId> {
        world
            .holder(self.leader_title)
            .filter(|h| self.members.contains(h))
            .or_else(|| self.members.first().copied())
    }

    /// Everything the director does in one hour: plan if needed, then advance the plan.
    pub fn hour(
        &mut self,
        world: &mut World,
        rng: &mut Rng,
        tuning: &Tuning,
        hour: u32,
    ) -> Vec<WorldEvent> {
        let mut events = Vec::new();
        self.members.retain(|&m| world.get(m).alive);
        // A new plan whenever the last one ran out, and every morning between journeys: a party
        // on the road sees its journey through.
        let morning =
            hour % 24 == 6 && matches!(self.location, Location::At(_)) && self.journey.is_none();
        if self.plan.is_empty() || morning {
            self.replan(world, tuning, hour, &mut events);
        }
        self.invite_players(world, hour, &mut events);
        if let Some(step) = self.plan.first().cloned()
            && self.run(&step, world, rng, tuning, hour, &mut events)
        {
            self.plan.remove(0);
        }
        events
    }

    fn replan(&mut self, world: &World, tuning: &Tuning, hour: u32, events: &mut Vec<WorldEvent>) {
        let view = View {
            world,
            party: self,
            tuning,
            hour,
        };
        let (plan, reasons) = htn::plan(&HeroDomain::new(), &view, Goal::Year)
            .unwrap_or_else(|| (Vec::new(), vec!["stuck"]));
        let intent = reasons.join(" > ");
        let fell = reasons.contains(&"nobody left");
        self.plan = plan;
        // A journey dropped by the new plan is over; the next one is announced afresh.
        if !matches!(self.plan.first(), Some(Step::Travel { to }) if Some(*to) == self.journey) {
            self.journey = None;
        }
        if fell && !self.fell {
            events.push(event(hour, Happening::PartyFell));
        }
        self.fell = fell;
        if intent != self.intent {
            events.push(event(
                hour,
                Happening::Plan {
                    intent: intent.clone(),
                },
            ));
            self.intent = intent;
        }
    }

    /// Players where the party is, told once that they could fill a missing role.
    fn invite_players(&mut self, world: &World, hour: u32, events: &mut Vec<WorldEvent>) {
        let Location::At(here) = self.location else {
            return;
        };
        let Some(role) = missing_roles(world, self).into_iter().next() else {
            return;
        };
        for (i, a) in world.actors.iter().enumerate() {
            let id = ActorId(i as u16);
            let asked = self.invited.contains(&id) || self.members.contains(&id);
            if a.alive && a.player.is_some() && a.online && a.region == here && !asked {
                self.invited.push(id);
                events.push(event(
                    hour,
                    Happening::Invited {
                        actor: id,
                        role: role.clone(),
                    },
                ));
            }
        }
    }

    /// Advances `step` by an hour; true when it is finished (or cannot go on).
    fn run(
        &mut self,
        step: &Step,
        world: &mut World,
        rng: &mut Rng,
        tuning: &Tuning,
        hour: u32,
        events: &mut Vec<WorldEvent>,
    ) -> bool {
        match *step {
            Step::Travel { to } => self.travel(to, world, rng, tuning, hour, events),
            Step::Rest => {
                self.heal(world, tuning);
                let done = self
                    .present(world)
                    .iter()
                    .all(|&m| world.get(m).health >= 95);
                if done {
                    events.push(event(
                        hour,
                        Happening::Rested {
                            region: self.region(),
                        },
                    ));
                }
                done
            }
            Step::Train { until } => {
                self.heal(world, tuning);
                self.trained_hours += 1;
                if self.trained_hours.is_multiple_of(24) {
                    for m in self.present(world) {
                        let a = world.get_mut(m);
                        a.power = a.power.saturating_add(tuning.train_per_day);
                    }
                    events.push(event(
                        hour,
                        Happening::Trained {
                            strength: strength(world, &self.present(world)),
                        },
                    ));
                }
                strength(world, &self.present(world)) >= until
            }
            Step::Recruit { actor } => {
                let a = world.get(actor);
                if a.alive
                    && Location::At(a.region) == self.location
                    && !self.members.contains(&actor)
                {
                    self.members.push(actor);
                    events.push(event(hour, Happening::Joined { actor }));
                }
                true
            }
            Step::Fight { boss } => {
                self.fight(boss, world, rng, tuning, hour, events);
                true
            }
            Step::Wait { until } => hour >= until,
        }
    }

    fn travel(
        &mut self,
        to: RegionId,
        world: &mut World,
        rng: &mut Rng,
        tuning: &Tuning,
        hour: u32,
        events: &mut Vec<WorldEvent>,
    ) -> bool {
        if let Location::At(here) = self.location {
            if here == to {
                return true;
            }
            let Some((path, total)) = world.path(here, to) else {
                return true;
            };
            let next = path[0];
            if self.journey != Some(to) {
                self.journey = Some(to);
                events.push(event(
                    hour,
                    Happening::Departed {
                        from: here,
                        to,
                        hours: total,
                    },
                ));
            }
            self.location = Location::Road {
                from: here,
                to: next,
                done: 0,
                total: world.road_hours(here, next).unwrap_or(1),
            };
        }
        let Location::Road {
            from,
            to: next,
            done,
            total,
        } = self.location
        else {
            return false;
        };
        let danger = world
            .region_at(from)
            .danger
            .max(world.region_at(next).danger);
        let watched = world.region_at(from).detailed || world.region_at(next).detailed;
        if !watched
            && rng.chance(
                u128::from(danger * tuning.encounter_permille_per_danger),
                1000,
            )
        {
            self.encounter(next, danger, world, rng, tuning, hour, events);
        }
        if done + 1 >= total {
            self.location = Location::At(next);
            for &m in &self.members {
                let a = world.get_mut(m);
                if a.player.is_none() {
                    a.region = next;
                }
            }
            if next == to {
                self.journey = None;
                events.push(event(hour, Happening::Arrived { region: to }));
                return true;
            }
        } else {
            self.location = Location::Road {
                from,
                to: next,
                done: done + 1,
                total,
            };
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn encounter(
        &mut self,
        region: RegionId,
        danger: u32,
        world: &mut World,
        rng: &mut Rng,
        tuning: &Tuning,
        hour: u32,
        events: &mut Vec<WorldEvent>,
    ) {
        let fighters = self.present(world);
        if fighters.is_empty() {
            return;
        }
        let enemy = danger * tuning.monster_power_per_danger * rng.range(50, 150) / 100;
        let result = battle::resolve(world, &fighters, u64::from(enemy), rng, tuning);
        events.push(event(
            hour,
            Happening::Encounter {
                region,
                enemy,
                won: result.won,
                deaths: result.deaths.clone(),
            },
        ));
        for dead in result.deaths {
            events.extend(world.die(dead, None, hour));
        }
        self.members.retain(|&m| world.get(m).alive);
    }

    fn fight(
        &mut self,
        boss: ActorId,
        world: &mut World,
        rng: &mut Rng,
        tuning: &Tuning,
        hour: u32,
        events: &mut Vec<WorldEvent>,
    ) {
        let here = Location::At(world.get(boss).region);
        if !world.get(boss).alive || here != self.location {
            return;
        }
        let fighters = self.present(world);
        // Nobody here to fight: no battle (and nothing to log).
        if fighters.is_empty() {
            return;
        }
        let enemy = boss_strength(world, self, boss);
        let result = battle::resolve(world, &fighters, enemy, rng, tuning);
        events.push(event(
            hour,
            Happening::Battle {
                boss,
                won: result.won,
                deaths: result.deaths.clone(),
            },
        ));
        for &dead in &result.deaths {
            events.extend(world.die(dead, Some(boss), hour));
        }
        self.members.retain(|&m| world.get(m).alive);
        if result.won {
            // Credit the leader if the leader fought, else whoever did.
            let killer = self
                .leader(world)
                .filter(|l| fighters.contains(l))
                .or_else(|| fighters.iter().copied().find(|&f| world.get(f).alive));
            events.extend(world.die(boss, killer, hour));
            if self.lieutenants.contains(&boss) {
                self.goal_weakened =
                    (self.goal_weakened + tuning.lieutenant_weakens_percent).min(90);
            }
        }
    }

    fn heal(&self, world: &mut World, tuning: &Tuning) {
        let rate = match self.location {
            Location::At(r) if world.region_at(r).inn => tuning.rest_inn_per_hour,
            _ => tuning.rest_wild_per_hour,
        };
        for m in self.present(world) {
            let a = world.get_mut(m);
            a.health = (a.health + rate).min(100);
        }
    }
}

fn event(hour: u32, what: Happening) -> WorldEvent {
    WorldEvent { hour, what }
}

fn strength(world: &World, members: &[ActorId]) -> u64 {
    members.iter().map(|&m| world.get(m).strength()).sum()
}

/// A boss's strength, the goal's less what defeated lieutenants took from it.
pub fn boss_strength(world: &World, party: &HeroParty, boss: ActorId) -> u64 {
    let s = world.get(boss).strength();
    if boss == party.goal {
        s * u64::from(100 - party.goal_weakened) / 100
    } else {
        s
    }
}

/// Roles no living member covers, in the party's order.
fn missing_roles(world: &World, party: &HeroParty) -> Vec<String> {
    let mut covered: Vec<&str> = party
        .members
        .iter()
        .filter(|&&m| world.get(m).alive)
        .map(|&m| world.get(m).role.as_str())
        .collect();
    let mut missing = Vec::new();
    for role in &party.roles {
        match covered.iter().position(|r| r == role) {
            Some(i) => {
                covered.remove(i);
            }
            None => missing.push(role.clone()),
        }
    }
    missing
}

/// What the planner sees.
pub struct View<'a> {
    world: &'a World,
    party: &'a HeroParty,
    tuning: &'a Tuning,
    hour: u32,
}

impl View<'_> {
    fn here(&self) -> RegionId {
        match self.party.location {
            Location::At(r) => r,
            // Mid-road, plans start from where the road leads.
            Location::Road { to, .. } => to,
        }
    }

    fn hours_to(&self, to: RegionId) -> u32 {
        self.world
            .path(self.here(), to)
            .map_or(u32::MAX, |(_, h)| h)
    }

    fn strength(&self) -> u64 {
        strength(self.world, &self.party.present(self.world))
    }

    /// Strength with everyone at full health: what training builds.
    fn potential(&self) -> u64 {
        self.party
            .present(self.world)
            .iter()
            .map(|&m| u64::from(self.world.get(m).power))
            .sum()
    }

    fn ready_for(&self, boss: ActorId) -> bool {
        self.strength() * 100 >= self.needed(boss) * 100
    }

    fn needed(&self, boss: ActorId) -> u64 {
        boss_strength(self.world, self.party, boss) * u64::from(self.tuning.readiness_percent) / 100
    }

    fn goal_alive(&self) -> bool {
        self.world.get(self.party.goal).alive
    }

    /// Hours of the year left.
    fn hours_left(&self) -> u32 {
        YEAR_HOURS.saturating_sub(self.hour)
    }

    /// Time to march on the goal whatever the odds: the road there plus the margin is all the
    /// year has left.
    fn out_of_time(&self) -> bool {
        let travel = self.hours_to(self.world.get(self.party.goal).region);
        travel.saturating_add(self.tuning.deadline_margin_days * 24) >= self.hours_left()
    }

    /// The lieutenant to face next: the weakest one still alive.
    fn next_lieutenant(&self) -> Option<ActorId> {
        self.party
            .lieutenants
            .iter()
            .copied()
            .filter(|&l| self.world.get(l).alive)
            .min_by_key(|&l| (self.world.get(l).power, l))
    }

    /// Someone the director can bring into the party: alive, not a player (players join by
    /// choice), not a boss or of a hostile faction, not already in, and reachable.
    fn joinable(&self, id: ActorId) -> bool {
        let a = self.world.get(id);
        a.alive
            && !a.boss
            && a.player.is_none()
            && !self.world.factions[usize::from(a.faction.0)].hostile
            && !a.in_party
            && !a.outlaw
            && !self.party.members.contains(&id)
            && self.world.path(self.here(), a.region).is_some()
    }

    /// Who should join: the leader title's holder first, then someone for each missing role.
    fn recruit(&self) -> Option<ActorId> {
        let world = self.world;
        if let Some(leader) = world.holder(self.party.leader_title)
            && self.joinable(leader)
        {
            return Some(leader);
        }
        missing_roles(world, self.party).iter().find_map(|role| {
            (0..world.actors.len())
                .map(|i| ActorId(i as u16))
                .filter(|&id| world.get(id).role == *role && self.joinable(id))
                // Title holders first (a new Grand Mage joins before a hedge wizard), then the
                // strongest, then the nearest.
                .max_by_key(|&id| {
                    let a = world.get(id);
                    (
                        world.titles_of(id).next().is_some(),
                        a.power,
                        std::cmp::Reverse(self.hours_to(a.region)),
                        std::cmp::Reverse(id),
                    )
                })
        })
    }

    fn wounded(&self) -> bool {
        let present = self.party.present(self.world);
        !present.is_empty()
            && present
                .iter()
                .map(|&m| self.world.get(m).health)
                .sum::<u32>()
                < 70 * present.len() as u32
    }

    /// The nearest region where `accept` holds (here counts).
    fn nearest(&self, accept: impl Fn(&crate::world::Region) -> bool) -> Option<RegionId> {
        (0..self.world.regions.len())
            .map(|i| RegionId(i as u16))
            .filter(|&r| accept(self.world.region_at(r)))
            .min_by_key(|&r| (self.hours_to(r), r))
            .filter(|&r| self.hours_to(r) != u32::MAX)
    }

    /// A player online holds the leader title and is in the party: they lead, the rest follow.
    fn leader_is_player(&self) -> bool {
        self.world
            .holder(self.party.leader_title)
            .filter(|h| self.party.members.contains(h))
            .is_some_and(|h| {
                let a = self.world.get(h);
                a.player.is_some() && a.online && a.alive
            })
    }

    /// No member alive anywhere, and nobody to recruit.
    fn nobody_left(&self) -> bool {
        !self.party.members.iter().any(|&m| self.world.get(m).alive) && self.recruit().is_none()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Goal {
    Year,
    Recover,
    Strengthen,
}

type HeroMethod<'w> = Method<View<'w>, Goal, Step>;

/// The hero party's methods. Built per plan: the view borrows the world, so the tables are
/// made for that borrow.
pub struct HeroDomain<'w> {
    year: Vec<HeroMethod<'w>>,
    recover: Vec<HeroMethod<'w>>,
    strengthen: Vec<HeroMethod<'w>>,
}

impl<'w> HeroDomain<'w> {
    pub fn new() -> Self {
        Self {
            year: vec![
                Method {
                    name: "the goal is defeated",
                    applies: |v| !v.goal_alive(),
                    subtasks: |v| vec![Task::Primitive(Step::Wait { until: v.hour + 24 })],
                },
                Method {
                    name: "nobody left",
                    applies: |v| v.nobody_left(),
                    subtasks: |v| vec![Task::Primitive(Step::Wait { until: v.hour + 24 })],
                },
                Method {
                    name: "follow the player leading",
                    applies: |v| v.leader_is_player(),
                    subtasks: |v| {
                        let leader = v.world.holder(v.party.leader_title).expect("checked");
                        vec![Task::Primitive(Step::Travel {
                            to: v.world.get(leader).region,
                        })]
                    },
                },
                Method {
                    name: "fill the ranks",
                    applies: |v| v.recruit().is_some(),
                    subtasks: |v| {
                        let actor = v.recruit().expect("checked");
                        vec![
                            Task::Primitive(Step::Travel {
                                to: v.world.get(actor).region,
                            }),
                            Task::Primitive(Step::Recruit { actor }),
                        ]
                    },
                },
                Method {
                    name: "wait for its players",
                    applies: |v| v.party.present(v.world).is_empty(),
                    subtasks: |v| vec![Task::Primitive(Step::Wait { until: v.hour + 24 })],
                },
                Method {
                    name: "march on the goal before the year ends",
                    applies: |v| v.out_of_time(),
                    subtasks: |v| strike(v, v.party.goal),
                },
                Method {
                    name: "recover",
                    applies: |v| v.wounded(),
                    subtasks: |_| vec![Task::Compound(Goal::Recover)],
                },
                Method {
                    name: "strike the goal",
                    applies: |v| v.ready_for(v.party.goal),
                    subtasks: |v| strike(v, v.party.goal),
                },
                Method {
                    name: "hunt a lieutenant",
                    applies: |v| v.next_lieutenant().is_some_and(|l| v.ready_for(l)),
                    subtasks: |v| strike(v, v.next_lieutenant().expect("checked")),
                },
                Method {
                    name: "grow stronger",
                    applies: |_| true,
                    subtasks: |_| vec![Task::Compound(Goal::Strengthen)],
                },
            ],
            recover: vec![
                Method {
                    name: "rest at an inn",
                    applies: |v| v.nearest(|r| r.inn).is_some(),
                    subtasks: |v| {
                        vec![
                            Task::Primitive(Step::Travel {
                                to: v.nearest(|r| r.inn).expect("checked"),
                            }),
                            Task::Primitive(Step::Rest),
                        ]
                    },
                },
                Method {
                    name: "rest where we are",
                    applies: |_| true,
                    subtasks: |_| vec![Task::Primitive(Step::Rest)],
                },
            ],
            strengthen: vec![
                Method {
                    name: "train in a settlement",
                    applies: |v| v.nearest(|r| r.kind.is_settlement()).is_some(),
                    subtasks: |v| {
                        // Train for the next fight: the weakest lieutenant, or else the goal.
                        let target = v.next_lieutenant().unwrap_or(v.party.goal);
                        let until = v.needed(target).max(v.potential() + 1);
                        vec![
                            Task::Primitive(Step::Travel {
                                to: v.nearest(|r| r.kind.is_settlement()).expect("checked"),
                            }),
                            Task::Primitive(Step::Train { until }),
                        ]
                    },
                },
                Method {
                    name: "wait",
                    applies: |_| true,
                    subtasks: |v| vec![Task::Primitive(Step::Wait { until: v.hour + 24 })],
                },
            ],
        }
    }
}

impl Default for HeroDomain<'_> {
    fn default() -> Self {
        Self::new()
    }
}

fn strike(v: &View, boss: ActorId) -> Vec<Task<Goal, Step>> {
    vec![
        Task::Primitive(Step::Travel {
            to: v.world.get(boss).region,
        }),
        Task::Primitive(Step::Fight { boss }),
    ]
}

impl<'w> Domain for HeroDomain<'w> {
    type State = View<'w>;
    type Compound = Goal;
    type Primitive = Step;

    fn methods(&self, task: Goal) -> &[HeroMethod<'w>] {
        match task {
            Goal::Year => &self.year,
            Goal::Recover => &self.recover,
            Goal::Strengthen => &self.strengthen,
        }
    }
}
