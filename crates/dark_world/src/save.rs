//! The whole world saved (docs/PLAN.md §17): the world simulation and what is on the maps that
//! must outlast the host, in one RON file.
//!
//! Saved: the world simulation (its clock, actors, titles, standing and parties), every player's
//! character (where it stands, its health, body and pack), the structures set down, and where the
//! world's people stand. Not saved: enemies (they return to their posts), speech, fights in
//! progress. A save is written at each new day and when the host quits; loading it puts every
//! player's character back asleep where it was, to wake when they join again.

use std::path::Path;

use bevy_ecs::prelude::*;
use dark_life::{Body, Inventory, Structure};
use dark_net::PlayerId;
use dark_sim::WorldSim;
use dark_sprite::Facing;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::characters::{
    Asleep, CHARACTER_RADIUS, CharacterSheets, CharacterState, ControlInput, LookId, PlayerAvatar,
};
use crate::life::{Life, LifeRules, Placed};
use crate::party::Person;
use crate::{BodyState, MapId, Maps, NextNetId, PreviousBody, body_bundle};

/// Written by this version; older files load too (a bare world simulation, before M7).
pub const SAVE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSave {
    pub version: u32,
    /// The player who hosted: their own character comes back to them.
    #[serde(default)]
    pub host: Option<PlayerId>,
    pub sim: WorldSim,
    #[serde(default)]
    pub characters: Vec<SavedCharacter>,
    #[serde(default)]
    pub structures: Vec<SavedStructure>,
    #[serde(default)]
    pub people: Vec<SavedPerson>,
    /// Flags, affinity, marriages, storylets told.
    #[serde(default)]
    pub story: dark_story::Story,
}

/// Maps are named by scene, not numbered: a project may add maps between saves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCharacter {
    pub player: PlayerId,
    pub map: String,
    pub position: Vec2,
    pub elevation: f32,
    pub facing: Facing,
    pub health: u16,
    #[serde(default)]
    pub life: Option<(Body, Inventory)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedStructure {
    pub map: String,
    pub at: Vec2,
    pub elevation: f32,
    pub structure: Structure,
    pub item: String,
    pub owner: Option<PlayerId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPerson {
    /// The world actor's id.
    pub actor: String,
    pub map: String,
    pub position: Vec2,
    pub facing: Facing,
}

impl WorldSave {
    /// A new world: nobody has played it yet.
    pub fn new(sim: WorldSim) -> Self {
        Self {
            version: SAVE_VERSION,
            host: None,
            sim,
            characters: Vec::new(),
            structures: Vec::new(),
            people: Vec::new(),
            story: dark_story::Story::default(),
        }
    }

    pub fn to_ron(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    /// A save file: this format, or a world simulation on its own (older saves).
    pub fn parse(text: &str) -> Result<Self, String> {
        match ron::from_str::<Self>(text) {
            Ok(save) if save.version <= SAVE_VERSION => Ok(save),
            Ok(save) => Err(format!(
                "saved by a newer engine (version {}, this one reads up to {SAVE_VERSION})",
                save.version
            )),
            Err(err) => match WorldSim::load(text) {
                Ok(sim) => Ok(Self::new(sim)),
                Err(_) => Err(err.to_string()),
            },
        }
    }

    /// Everything saveable in `world` now, with its world simulation.
    pub fn gather(world: &mut World, sim: WorldSim) -> Self {
        let names: Vec<String> = world
            .resource::<Maps>()
            .maps
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let name = |map: &MapId| names.get(usize::from(map.0)).cloned().unwrap_or_default();
        // The host's own player; a headless host keeps whoever hosted before.
        let host = world
            .get_resource::<crate::NetHost>()
            .and_then(|h| h.0.local_player())
            .or_else(|| {
                world
                    .get_resource::<crate::WorldState>()
                    .and_then(|w| w.saved_host)
            });
        let mut characters: Vec<SavedCharacter> = world
            .query::<(
                &PlayerAvatar,
                &MapId,
                &BodyState,
                &CharacterState,
                Option<&Life>,
            )>()
            .iter(world)
            .map(|(avatar, map, body, state, life)| SavedCharacter {
                player: avatar.0,
                map: name(map),
                position: body.0.position,
                elevation: body.0.elevation,
                facing: state.facing,
                // A character saved dead gets up whole when loaded.
                health: state.fighter.health,
                life: life.map(|l| (l.body, l.inventory.clone())),
            })
            .collect();
        characters.sort_by_key(|c| c.player);
        let structures = world
            .query::<(&MapId, &Placed)>()
            .iter(world)
            .map(|(map, p)| SavedStructure {
                map: name(map),
                at: p.at,
                elevation: p.elevation,
                structure: p.structure,
                item: p.item.clone(),
                owner: p.owner,
            })
            .collect();
        let mut people: Vec<SavedPerson> = world
            .query::<(&Person, &MapId, &BodyState, &CharacterState)>()
            .iter(world)
            .map(|(person, map, body, state)| SavedPerson {
                actor: person.0.clone(),
                map: name(map),
                position: body.0.position,
                facing: state.facing,
            })
            .collect();
        people.sort_by(|a, b| a.actor.cmp(&b.actor));
        let story = world
            .get_resource::<crate::StoryState>()
            .map(|s| s.story.clone())
            .unwrap_or_default();
        Self {
            version: SAVE_VERSION,
            host,
            sim,
            characters,
            structures,
            people,
            story,
        }
    }

    /// Puts the saved characters, structures and people into `world`, whose maps, looks,
    /// people and life rules are already there. Characters come back asleep (their players are
    /// away until they join).
    pub(crate) fn restore(&self, world: &mut World) {
        let map_of = |world: &World, name: &str| {
            world
                .resource::<Maps>()
                .maps
                .iter()
                .position(|m| m.name == name)
                .map(|i| MapId(i as u16))
        };
        world.init_resource::<NextNetId>();
        world.insert_resource(crate::story::SavedStory(self.story.clone()));
        let life_def = world
            .get_resource::<LifeRules>()
            .map(|r| r.def.clone())
            .unwrap_or_default();
        for c in &self.characters {
            let Some(map) = map_of(world, &c.map) else {
                tracing::warn!(
                    "save: map {} is gone; {}'s character starts over",
                    c.map,
                    c.player
                );
                continue;
            };
            let sheets = world.resource::<CharacterSheets>();
            let mut state = CharacterState::new(sheets, LookId(0), c.facing);
            if c.health > 0 {
                state.fighter.health = c.health;
            }
            let id = world.resource_mut::<NextNetId>().allocate();
            let mut entity = world.spawn((
                body_bundle(map, c.position, CHARACTER_RADIUS),
                state,
                ControlInput::default(),
                PlayerAvatar(c.player),
                id,
                Asleep,
                crate::combat::SwingHits::default(),
            ));
            entity.get_mut::<BodyState>().expect("spawned").0.elevation = c.elevation;
            entity.get_mut::<PreviousBody>().expect("spawned").elevation = c.elevation;
            if let Some((body, inventory)) = &c.life {
                let mut life = Life::new(&life_def);
                life.body = *body;
                life.inventory = inventory.clone();
                entity.insert(life);
            }
        }
        for s in &self.structures {
            let Some(map) = map_of(world, &s.map) else {
                continue;
            };
            let id = world.resource_mut::<NextNetId>().allocate();
            world.spawn((
                Placed {
                    structure: s.structure,
                    at: s.at,
                    elevation: s.elevation,
                    item: s.item.clone(),
                    owner: s.owner,
                },
                map,
                id,
            ));
        }
        for p in &self.people {
            let Some(map) = map_of(world, &p.map) else {
                continue;
            };
            let mut q = world.query::<(
                &Person,
                &mut MapId,
                &mut BodyState,
                &mut PreviousBody,
                &mut CharacterState,
            )>();
            for (person, mut m, mut body, mut previous, mut state) in q.iter_mut(world) {
                if person.0 == p.actor {
                    *m = map;
                    body.0.position = p.position;
                    previous.position = p.position;
                    state.facing = p.facing;
                }
            }
        }
    }
}

/// Writes beside the save first, so a crash mid-write never leaves half a world.
pub(crate) fn write(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let partial = path.with_extension("ron.partial");
    let mut file = std::fs::File::create(&partial)?;
    file.write_all(text.as_bytes())?;
    // On disk before the rename, or a power cut could leave the new name on empty data.
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
}
