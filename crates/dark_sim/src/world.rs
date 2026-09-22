//! The world while it runs: regions joined by roads, actors, factions and titles. Built once from
//! a [`WorldDef`], then changed only by the simulation, so it saves and replays exactly.

use serde::{Deserialize, Serialize};

use crate::def::{DefError, RegionKind, WorldDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RegionId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActorId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TitleId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FactionId(pub u16);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: String,
    pub name: String,
    pub kind: RegionKind,
    pub danger: u32,
    pub inn: bool,
    /// A player is here: the full simulation runs this region, so the abstract one does not
    /// roll its random encounters.
    pub detailed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Road {
    pub a: RegionId,
    pub b: RegionId,
    pub hours: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Faction {
    pub id: String,
    pub name: String,
    pub hostile: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Title {
    pub id: String,
    pub name: String,
    pub to_killer: bool,
    /// Faction and role that appoint a successor.
    pub appointed: Option<(FactionId, String)>,
    pub holder: Option<ActorId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub name: String,
    pub role: String,
    pub faction: FactionId,
    pub power: u32,
    /// 0..=100; at 0 the actor dies.
    pub health: u32,
    pub alive: bool,
    /// Where the actor is when not travelling with a party.
    pub region: RegionId,
    pub boss: bool,
    /// Played by this player (their id), not directed by the world.
    pub player: Option<u128>,
    /// A player connected now (an offline player's character sleeps and leads nobody).
    pub online: bool,
    /// How each faction regards this actor, by `FactionId`, −1000 to 1000.
    #[serde(default)]
    pub standing: Vec<i32>,
    /// Standing a leader needs with this actor's faction to be followed.
    #[serde(default)]
    pub trust: i32,
    /// In a social party (see [`crate::social`]); the hero party's director leaves them be.
    #[serde(default)]
    pub in_party: bool,
    /// Turned on a party: nobody of the old side takes them in again.
    #[serde(default)]
    pub outlaw: bool,
}

impl Actor {
    /// Strength in a fight: power, scaled down by wounds.
    pub fn strength(&self) -> u64 {
        if self.alive {
            u64::from(self.power) * u64::from(self.health) / 100
        } else {
            0
        }
    }
}

/// Something that happened, for the chronicle, the host's log and (later) storylets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldEvent {
    /// Hours since the start of the year.
    pub hour: u32,
    pub what: Happening,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Happening {
    /// The hero party decided what it is doing next.
    Plan {
        intent: String,
    },
    Departed {
        from: RegionId,
        to: RegionId,
        hours: u32,
    },
    Arrived {
        region: RegionId,
    },
    /// Monsters on the way.
    Encounter {
        region: RegionId,
        enemy: u32,
        won: bool,
        deaths: Vec<ActorId>,
    },
    Battle {
        boss: ActorId,
        won: bool,
        deaths: Vec<ActorId>,
    },
    Trained {
        strength: u64,
    },
    Rested {
        region: RegionId,
    },
    Joined {
        actor: ActorId,
    },
    /// A player could fill a place in the party (their answer comes through storylets, M7).
    Invited {
        actor: ActorId,
        role: String,
    },
    /// Into a social party led by `leader`.
    JoinedParty {
        actor: ActorId,
        leader: ActorId,
    },
    LeftParty {
        actor: ActorId,
        leader: ActorId,
        how: crate::social::Parting,
    },
    /// Turned on the party led by `leader`.
    Betrayed {
        actor: ActorId,
        leader: ActorId,
    },
    /// Changed sides.
    Defected {
        actor: ActorId,
        faction: FactionId,
    },
    Died {
        actor: ActorId,
        killer: Option<ActorId>,
    },
    TitlePassed {
        title: TitleId,
        to: ActorId,
    },
    TitleVacant {
        title: TitleId,
    },
    /// Nobody is left to carry on.
    PartyFell,
    YearEnded {
        outcome: Outcome,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The goal boss fell on this day (zero-based).
    GoalDefeated { day: u32 },
    /// The year ran out with the goal alive.
    GoalSurvived,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub regions: Vec<Region>,
    pub roads: Vec<Road>,
    pub factions: Vec<Faction>,
    pub titles: Vec<Title>,
    pub actors: Vec<Actor>,
}

impl World {
    pub fn build(def: &WorldDef) -> Result<Self, DefError> {
        let social = &def.social;
        let bad = |m: String| DefError::Invalid(m);
        unique(def.regions.iter().map(|r| r.id.as_str()), "region")?;
        unique(def.factions.iter().map(|f| f.id.as_str()), "faction")?;
        unique(def.titles.iter().map(|t| t.id.as_str()), "title")?;
        unique(def.actors.iter().map(|a| a.id.as_str()), "actor")?;
        if def.regions.len() > usize::from(u16::MAX) || def.actors.len() > usize::from(u16::MAX) {
            return Err(bad("more than 65535 regions or actors".into()));
        }

        let regions: Vec<Region> = def
            .regions
            .iter()
            .map(|r| Region {
                id: r.id.clone(),
                name: r.name.clone(),
                kind: r.kind,
                danger: r.danger.min(10),
                inn: r.inn,
                detailed: false,
            })
            .collect();
        let factions: Vec<Faction> = def
            .factions
            .iter()
            .map(|f| Faction {
                id: f.id.clone(),
                name: f.name.clone(),
                hostile: f.hostile,
            })
            .collect();
        let mut world = World {
            regions,
            roads: Vec::new(),
            factions,
            titles: Vec::new(),
            actors: Vec::new(),
        };
        for road in &def.roads {
            let a = world
                .region(&road.between.0)
                .ok_or_else(|| bad(format!("road from unknown region {}", road.between.0)))?;
            let b = world
                .region(&road.between.1)
                .ok_or_else(|| bad(format!("road to unknown region {}", road.between.1)))?;
            if a == b || road.hours == 0 {
                return Err(bad(format!(
                    "road {}–{} must join two regions and take time",
                    road.between.0, road.between.1
                )));
            }
            world.roads.push(Road {
                a,
                b,
                hours: road.hours,
            });
        }
        for title in &def.titles {
            let appointed = match &title.succession.appointed {
                Some(a) => Some((
                    world.faction(&a.faction).ok_or_else(|| {
                        bad(format!(
                            "title {} appointed by unknown faction {}",
                            title.id, a.faction
                        ))
                    })?,
                    a.role.clone(),
                )),
                None => None,
            };
            world.titles.push(Title {
                id: title.id.clone(),
                name: title.name.clone(),
                to_killer: title.succession.to_killer,
                appointed,
                holder: None,
            });
        }
        for actor in &def.actors {
            let faction = world.faction(&actor.faction).ok_or_else(|| {
                bad(format!(
                    "actor {} in unknown faction {}",
                    actor.id, actor.faction
                ))
            })?;
            let region = world.region(&actor.home).ok_or_else(|| {
                bad(format!(
                    "actor {} at unknown region {}",
                    actor.id, actor.home
                ))
            })?;
            let id = ActorId(world.actors.len() as u16);
            for title in &actor.titles {
                let t = world.title(title).ok_or_else(|| {
                    bad(format!("actor {} holds unknown title {title}", actor.id))
                })?;
                let slot = &mut world.titles[usize::from(t.0)].holder;
                if slot.is_some() {
                    return Err(bad(format!("title {title} is held twice")));
                }
                *slot = Some(id);
            }
            world.actors.push(Actor {
                id: actor.id.clone(),
                name: actor.name.clone(),
                role: actor.role.clone(),
                faction,
                power: actor.power,
                health: 100,
                alive: true,
                region,
                boss: actor.boss,
                player: None,
                online: false,
                standing: world.first_standing(faction, social),
                trust: actor.trust,
                in_party: false,
                outlaw: false,
            });
        }
        Ok(world)
    }

    /// Standing a new member of `faction` starts with: well regarded at home, an enemy of the
    /// other side.
    pub fn first_standing(&self, faction: FactionId, social: &crate::def::Social) -> Vec<i32> {
        let hostile = self.is_hostile(faction);
        (0..self.factions.len())
            .map(|i| {
                let other = FactionId(i as u16);
                if other == faction {
                    social.own_standing
                } else if self.is_hostile(other) != hostile {
                    social.enemy_standing
                } else {
                    0
                }
            })
            .collect()
    }

    pub fn is_hostile(&self, faction: FactionId) -> bool {
        self.factions[usize::from(faction.0)].hostile
    }

    /// How `faction` regards `actor`.
    pub fn standing(&self, actor: ActorId, faction: FactionId) -> i32 {
        self.get(actor)
            .standing
            .get(usize::from(faction.0))
            .copied()
            .unwrap_or(0)
    }

    pub fn adjust_standing(&mut self, actor: ActorId, faction: FactionId, delta: i32) {
        let count = self.factions.len();
        let standing = &mut self.get_mut(actor).standing;
        standing.resize(count, 0);
        let s = &mut standing[usize::from(faction.0)];
        *s = (*s + delta).clamp(-1000, 1000);
    }

    /// How `actor` (through its faction) regards `other`.
    pub fn regard(&self, actor: ActorId, other: ActorId) -> i32 {
        self.standing(other, self.get(actor).faction)
    }

    /// Marks who is in a social party.
    pub(crate) fn sync_parties(&mut self, parties: &[crate::social::Party]) {
        for (i, actor) in self.actors.iter_mut().enumerate() {
            let id = ActorId(i as u16);
            actor.in_party = parties.iter().any(|p| p.members.contains(&id));
        }
    }

    /// Titles `actor` holds from factions that no longer count it a friend pass on by
    /// appointment.
    pub(crate) fn strip_titles(&mut self, actor: ActorId, hour: u32) -> Vec<WorldEvent> {
        let mut events = Vec::new();
        let held: Vec<TitleId> = self.titles_of(actor).collect();
        for title in held {
            let Some((faction, _)) = self.titles[usize::from(title.0)].appointed.clone() else {
                continue;
            };
            if self.is_hostile(faction) == self.is_hostile(self.get(actor).faction) {
                continue;
            }
            let heir = self.heir(title, None);
            self.titles[usize::from(title.0)].holder = heir;
            events.push(WorldEvent {
                hour,
                what: match heir {
                    Some(to) => Happening::TitlePassed { title, to },
                    None => Happening::TitleVacant { title },
                },
            });
        }
        events
    }

    pub fn region(&self, id: &str) -> Option<RegionId> {
        self.regions
            .iter()
            .position(|r| r.id == id)
            .map(|i| RegionId(i as u16))
    }

    pub fn actor(&self, id: &str) -> Option<ActorId> {
        self.actors
            .iter()
            .position(|a| a.id == id)
            .map(|i| ActorId(i as u16))
    }

    pub fn title(&self, id: &str) -> Option<TitleId> {
        self.titles
            .iter()
            .position(|t| t.id == id)
            .map(|i| TitleId(i as u16))
    }

    pub fn faction(&self, id: &str) -> Option<FactionId> {
        self.factions
            .iter()
            .position(|f| f.id == id)
            .map(|i| FactionId(i as u16))
    }

    pub fn get(&self, id: ActorId) -> &Actor {
        &self.actors[usize::from(id.0)]
    }

    pub fn get_mut(&mut self, id: ActorId) -> &mut Actor {
        &mut self.actors[usize::from(id.0)]
    }

    pub fn region_at(&self, id: RegionId) -> &Region {
        &self.regions[usize::from(id.0)]
    }

    pub fn titles_of(&self, actor: ActorId) -> impl Iterator<Item = TitleId> + '_ {
        self.titles
            .iter()
            .enumerate()
            .filter(move |(_, t)| t.holder == Some(actor))
            .map(|(i, _)| TitleId(i as u16))
    }

    pub fn holder(&self, title: TitleId) -> Option<ActorId> {
        self.titles[usize::from(title.0)].holder
    }

    /// The quickest way from `from` to `to`: the regions after `from`, and the hours it takes.
    /// Ties go to the lower region ids, so every run takes the same way.
    pub fn path(&self, from: RegionId, to: RegionId) -> Option<(Vec<RegionId>, u32)> {
        let n = self.regions.len();
        let mut dist = vec![u32::MAX; n];
        let mut prev = vec![None; n];
        let mut done = vec![false; n];
        dist[usize::from(from.0)] = 0;
        while let Some(u) = (0..n)
            .filter(|&i| !done[i] && dist[i] != u32::MAX)
            .min_by_key(|&i| (dist[i], i))
        {
            done[u] = true;
            if u == usize::from(to.0) {
                break;
            }
            for road in &self.roads {
                let v = match (usize::from(road.a.0) == u, usize::from(road.b.0) == u) {
                    (true, _) => usize::from(road.b.0),
                    (_, true) => usize::from(road.a.0),
                    _ => continue,
                };
                let d = dist[u].saturating_add(road.hours);
                if d < dist[v] {
                    dist[v] = d;
                    prev[v] = Some(u);
                }
            }
        }
        let total = dist[usize::from(to.0)];
        if total == u32::MAX {
            return None;
        }
        let mut steps = Vec::new();
        let mut at = usize::from(to.0);
        while at != usize::from(from.0) {
            steps.push(RegionId(at as u16));
            at = prev[at]?;
        }
        steps.reverse();
        Some((steps, total))
    }

    /// Hours along the road joining two neighbouring regions.
    pub fn road_hours(&self, a: RegionId, b: RegionId) -> Option<u32> {
        self.roads
            .iter()
            .filter(|r| (r.a == a && r.b == b) || (r.a == b && r.b == a))
            .map(|r| r.hours)
            .min()
    }

    /// The actor dies; its titles pass on by their succession rules. `killer` is `None` for
    /// monsters, accidents and the like.
    pub fn die(&mut self, victim: ActorId, killer: Option<ActorId>, hour: u32) -> Vec<WorldEvent> {
        let mut events = Vec::new();
        if !self.get(victim).alive {
            return events;
        }
        let actor = self.get_mut(victim);
        actor.alive = false;
        actor.health = 0;
        events.push(WorldEvent {
            hour,
            what: Happening::Died {
                actor: victim,
                killer,
            },
        });
        let held: Vec<TitleId> = self.titles_of(victim).collect();
        for title in held {
            let heir = self.heir(title, killer);
            self.titles[usize::from(title.0)].holder = heir;
            events.push(WorldEvent {
                hour,
                what: match heir {
                    Some(to) => Happening::TitlePassed { title, to },
                    None => Happening::TitleVacant { title },
                },
            });
        }
        events
    }

    fn heir(&self, title: TitleId, killer: Option<ActorId>) -> Option<ActorId> {
        let rule = &self.titles[usize::from(title.0)];
        // A killer inherits only as a person of the world: not a boss, not the demon army.
        if rule.to_killer
            && let Some(killer) = killer
            && self.get(killer).alive
            && !self.get(killer).boss
            && !self.factions[usize::from(self.get(killer).faction.0)].hostile
        {
            return Some(killer);
        }
        let (faction, role) = rule.appointed.as_ref()?;
        // The faction's strongest living member of the role, with no title and no player
        // (a player is offered a title through play, not appointed to one).
        (0..self.actors.len())
            .map(|i| ActorId(i as u16))
            .filter(|&id| {
                let a = self.get(id);
                a.alive
                    && a.faction == *faction
                    && a.role == *role
                    && !a.boss
                    && a.player.is_none()
                    && self.titles_of(id).next().is_none()
            })
            .max_by_key(|&id| (self.get(id).power, std::cmp::Reverse(id)))
    }
}

fn unique<'a>(ids: impl Iterator<Item = &'a str>, what: &str) -> Result<(), DefError> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(DefError::Invalid(format!("{what} {id} is defined twice")));
        }
    }
    Ok(())
}
