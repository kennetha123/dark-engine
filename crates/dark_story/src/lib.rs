//! Storylets, dialogue, relationships and endings (docs/PLAN.md §4.5, §17).
//!
//! A storylet is a conversation with one person of the world, offered while its conditions hold:
//! the player's standing and titles, the day, who lives, flags set by earlier choices, how the
//! person feels about the player, marriage. Its nodes are lines the person says and choices the
//! player can answer with; choices change the world (standing, flags, affinity, following,
//! joining the hero party, going over to another side, marriage) through [`dark_sim::WorldSim`].
//! [`Story`] is the state all of this leaves behind, saved with the world; it is plain data
//! stepped only by choices, so it replays like the rest of the simulation.
//!
//! Relationships: how each person feels about each player (affinity, −1000 to 1000) and who is
//! married to whom. When the year ends, the story's endings are tried in order.

mod def;

use std::collections::{BTreeMap, BTreeSet};

use dark_sim::{ActorId, Outcome, WorldSim};
use serde::{Deserialize, Serialize};

pub use def::{Choice, Condition, Effect, EndingDef, Node, Repeat, StoryDef, StoryError, Storylet};

/// What the story has left behind. Actors are world actors (players are actors too).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Story {
    /// Flags set by choices, per player.
    flags: BTreeSet<(ActorId, String)>,
    /// How a person feels about a player: (person, player).
    affinity: BTreeMap<(ActorId, ActorId), i32>,
    /// Married couples, each once, lower id first.
    married: BTreeSet<(ActorId, ActorId)>,
    /// When each player last heard each storylet, by day: (storylet, player).
    told: BTreeMap<(String, ActorId), u32>,
}

/// A conversation under way: which storylet, which node, between whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    pub storylet: usize,
    pub node: String,
    pub player: ActorId,
    pub person: ActorId,
}

/// What a node shows: the person's line and the choices open to the player now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeView {
    pub line: String,
    /// (index into the node's choices, what the player would say).
    pub choices: Vec<(usize, String)>,
}

/// What a choice did beyond the world simulation and the story, for the game to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deed {
    FadeToBlack,
    Give {
        item: String,
        count: u16,
    },
    /// The player went over to this faction (the game may make them an enemy of their friends).
    Defected(String),
}

impl Story {
    pub fn flag(&self, player: ActorId, flag: &str) -> bool {
        self.flags.contains(&(player, flag.to_owned()))
    }

    pub fn affinity(&self, person: ActorId, player: ActorId) -> i32 {
        self.affinity.get(&(person, player)).copied().unwrap_or(0)
    }

    pub fn spouse(&self, actor: ActorId) -> Option<ActorId> {
        self.married.iter().find_map(|&(a, b)| {
            if a == actor {
                Some(b)
            } else if b == actor {
                Some(a)
            } else {
                None
            }
        })
    }

    /// The storylet `person` tells `player` now, if any: the highest priority one whose
    /// conditions hold and that has not been told as often as it may be.
    pub fn open(
        &self,
        def: &StoryDef,
        sim: &WorldSim,
        player: ActorId,
        person: ActorId,
    ) -> Option<Conversation> {
        let person_id = &sim.world().get(person).id;
        let day = sim.day();
        def.storylets
            .iter()
            .enumerate()
            .filter(|(_, s)| &s.with == person_id)
            .filter(|(_, s)| {
                let told = self.told.get(&(s.id.clone(), player));
                match s.repeat {
                    Repeat::Always => true,
                    Repeat::Daily => told.is_none_or(|&d| d != day),
                    Repeat::Once => told.is_none(),
                }
            })
            .filter(|(_, s)| {
                s.when
                    .iter()
                    .all(|c| self.holds(c, sim, Some(player), Some(person)))
            })
            // The first of the highest priority, in file order.
            .max_by_key(|(i, s)| (s.priority, std::cmp::Reverse(*i)))
            .map(|(i, s)| Conversation {
                storylet: i,
                node: s.start.clone(),
                player,
                person,
            })
    }

    /// Begins `conversation`: runs its first node's effects. It counts as told only once the
    /// player answers or hears it out ([`Story::finish`]), so walking away loses nothing.
    pub fn begin(
        &mut self,
        def: &StoryDef,
        sim: &mut WorldSim,
        conversation: &Conversation,
    ) -> Vec<Deed> {
        let storylet = &def.storylets[conversation.storylet];
        let effects = storylet.nodes[&conversation.node].then.clone();
        self.apply(&effects, sim, conversation)
    }

    /// The player heard `conversation` out (its last line): it counts as told.
    pub fn finish(&mut self, def: &StoryDef, sim: &WorldSim, conversation: &Conversation) {
        self.mark_told(def, sim, conversation);
    }

    fn mark_told(&mut self, def: &StoryDef, sim: &WorldSim, conversation: &Conversation) {
        let id = def.storylets[conversation.storylet].id.clone();
        self.told.insert((id, conversation.player), sim.day());
    }

    /// What the node shows now.
    pub fn view(&self, def: &StoryDef, sim: &WorldSim, conversation: &Conversation) -> NodeView {
        let node = &def.storylets[conversation.storylet].nodes[&conversation.node];
        let choices = node
            .choices
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.when.iter().all(|w| {
                    self.holds(w, sim, Some(conversation.player), Some(conversation.person))
                })
            })
            .map(|(i, c)| (i, c.says.clone()))
            .collect();
        NodeView {
            line: node.line.clone(),
            choices,
        }
    }

    /// The player picks choice `index` of the node (an index from [`NodeView::choices`]). The
    /// conversation moves on (`Some`) or ends (`None`); deeds are for the game.
    pub fn choose(
        &mut self,
        def: &StoryDef,
        sim: &mut WorldSim,
        conversation: &Conversation,
        index: usize,
    ) -> (Option<Conversation>, Vec<Deed>) {
        let storylet = &def.storylets[conversation.storylet];
        let node = &storylet.nodes[&conversation.node];
        let Some(choice) = node.choices.get(index) else {
            return (Some(conversation.clone()), Vec::new());
        };
        // A choice not open now (the world moved on) does nothing.
        let open = choice
            .when
            .iter()
            .all(|w| self.holds(w, sim, Some(conversation.player), Some(conversation.person)));
        if !open {
            return (Some(conversation.clone()), Vec::new());
        }
        self.mark_told(def, sim, conversation);
        let mut deeds = self.apply(&choice.then, sim, conversation);
        let next = choice.next.as_ref().map(|next| Conversation {
            node: next.clone(),
            ..conversation.clone()
        });
        if let Some(next) = &next {
            let effects = storylet.nodes[&next.node].then.clone();
            deeds.extend(self.apply(&effects, sim, next));
        }
        (next, deeds)
    }

    fn apply(
        &mut self,
        effects: &[Effect],
        sim: &mut WorldSim,
        conversation: &Conversation,
    ) -> Vec<Deed> {
        let (player, person) = (conversation.player, conversation.person);
        let mut deeds = Vec::new();
        for effect in effects {
            match effect {
                Effect::SetFlag(f) => {
                    self.flags.insert((player, f.clone()));
                }
                Effect::ClearFlag(f) => {
                    self.flags.remove(&(player, f.clone()));
                }
                Effect::Standing { faction, by } => {
                    if let Some(f) = sim.world().faction(faction) {
                        sim.adjust_standing(player, f, *by);
                    }
                }
                Effect::Affinity(by) => {
                    let a = self.affinity.entry((person, player)).or_default();
                    *a = (*a + by).clamp(-1000, 1000);
                }
                Effect::Follow => {
                    let _ = sim.ask_to_join(player, person);
                }
                Effect::JoinHeroParty => {
                    sim.join_hero_party(player);
                }
                Effect::Defect(faction) => {
                    if let Some(f) = sim.world().faction(faction) {
                        sim.defect(player, f);
                        deeds.push(Deed::Defected(faction.clone()));
                    }
                }
                Effect::Marry => {
                    if self.spouse(player).is_none() && self.spouse(person).is_none() {
                        self.married
                            .insert((player.min(person), player.max(person)));
                    }
                }
                Effect::FadeToBlack => deeds.push(Deed::FadeToBlack),
                Effect::Give { item, count } => deeds.push(Deed::Give {
                    item: item.clone(),
                    count: *count,
                }),
            }
        }
        deeds
    }

    /// Whether `condition` holds, for `player` talking to `person` (either may be absent: an
    /// ending asks about the world, and a player condition then holds if any player meets it).
    pub fn holds(
        &self,
        condition: &Condition,
        sim: &WorldSim,
        player: Option<ActorId>,
        person: Option<ActorId>,
    ) -> bool {
        let world = sim.world();
        let players: Vec<ActorId> = match player {
            Some(p) => vec![p],
            None => (0..world.actors.len())
                .map(|i| ActorId(i as u16))
                .filter(|&a| world.get(a).player.is_some())
                .collect(),
        };
        let any = |test: &dyn Fn(ActorId) -> bool| players.iter().any(|&p| test(p));
        let alive = |id: &str| world.actor(id).is_some_and(|a| world.get(a).alive);
        match condition {
            Condition::Standing { faction, at_least } => world
                .faction(faction)
                .is_some_and(|f| any(&|p| world.standing(p, f) >= *at_least)),
            Condition::StandingBelow { faction, below } => world
                .faction(faction)
                .is_some_and(|f| any(&|p| world.standing(p, f) < *below)),
            Condition::Title(title) => world
                .title(title)
                .and_then(|t| world.holder(t))
                .is_some_and(|h| players.contains(&h)),
            Condition::FromDay(day) => sim.day() + 1 >= *day,
            Condition::Season(id) => sim
                .calendar()
                .season(sim.day() + 1)
                .is_some_and(|s| s.id == *id),
            Condition::During(id) => sim.calendar().during(id, sim.day() + 1),
            Condition::Alive(id) => alive(id),
            Condition::Dead(id) => world.actor(id).is_some() && !alive(id),
            Condition::Flag(f) => any(&|p| self.flag(p, f)),
            Condition::NotFlag(f) => !any(&|p| self.flag(p, f)),
            Condition::Affinity(at_least) => {
                person.is_some_and(|who| any(&|p| self.affinity(who, p) >= *at_least))
            }
            Condition::Married => any(&|p| self.spouse(p).is_some()),
            Condition::Unmarried => !any(&|p| self.spouse(p).is_some()),
            Condition::PersonUnmarried => person.is_none_or(|who| self.spouse(who).is_none()),
            Condition::WillFollow => {
                person.is_some_and(|who| any(&|p| sim.would_follow(p, who).is_ok()))
            }
            Condition::SpouseHere => {
                person.is_some_and(|who| any(&|p| self.spouse(p) == Some(who)))
            }
            Condition::Follows => person.is_some_and(|who| {
                any(&|p| {
                    sim.party_of(who)
                        .is_some_and(|party| party.members.contains(&p))
                })
            }),
            Condition::NotFollowing => person.is_none_or(|who| sim.party_of(who).is_none()),
            Condition::InHeroParty => any(&|p| sim.party().members.contains(&p)),
            Condition::NotInHeroParty => !any(&|p| sim.party().members.contains(&p)),
            Condition::Faction(faction) => world
                .faction(faction)
                .is_some_and(|f| any(&|p| world.get(p).faction == f)),
            Condition::GoalDefeated => {
                matches!(sim.outcome(), Some(Outcome::GoalDefeated { .. }))
            }
            Condition::GoalSurvived => !matches!(sim.outcome(), Some(Outcome::GoalDefeated { .. })),
            Condition::Not(inner) => !self.holds(inner, sim, player, person),
        }
    }

    /// How the year ended: the first ending whose conditions hold.
    pub fn ending<'d>(&self, def: &'d StoryDef, sim: &WorldSim) -> Option<&'d EndingDef> {
        def.endings
            .iter()
            .find(|e| e.when.iter().all(|c| self.holds(c, sim, None, None)))
    }
}

#[cfg(test)]
mod tests;
