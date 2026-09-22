//! NPCs placed by scenes, and talking to them.
//!
//! Pressing interact next to an NPC makes it turn to the speaker and say its next line. A line is
//! a string-table key, not text ([`dark_assets::Localization`]): the host only decides *what* is
//! said, and every player reads it in their own language. Speech is part of the character's
//! replicated state, so anyone in the map sees the bubble, including players who arrive mid-line.

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use dark_core::{App, FixedUpdate, Plugin};
use dark_sprite::Facing;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::characters::{
    Asleep, CHARACTER_RADIUS, CharacterSheets, CharacterState, Control, ControlInput, NetId,
    control_characters,
};
use crate::{BodyState, MapId, Maps, NextNetId, body_bundle};
use dark_assets::LineDef;

/// How close (feet to feet, pixels) a character must be to talk to an NPC.
pub const TALK_RANGE: f32 = 32.0;
/// Characters whose feet differ in height by more than this cannot talk (one on a ledge).
const TALK_HEIGHT: f32 = 8.0;
/// A conversation ends when the two are further apart than this.
pub const LEAVE_RANGE: f32 = 3.0 * TALK_RANGE;
/// How long a line stays up: 5 s.
pub const SPEECH_TICKS: u32 = 300;

/// A character the world placed, with a conversation to go through one line per talk.
#[derive(Component, Clone, Debug)]
pub struct Npc {
    pub lines: Vec<LineDef>,
    /// Where each talker is in the conversation: everyone hears it from the start.
    next: HashMap<NetId, usize>,
    /// Faces this way again when not talking.
    home: Facing,
}

/// A line a character is saying, as a string-table key.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speech {
    pub line: String,
    /// Who it is said to: the other side of the conversation.
    pub to: Option<NetId>,
    pub ticks_left: u32,
}

/// The NPC a character at `feet` (ground position and elevation) would talk to: the nearest
/// one in reach at about the same height. The client uses this too, to show the prompt.
pub fn talk_target<T>(
    feet: (Vec2, f32),
    npcs: impl IntoIterator<Item = (T, Vec2, f32)>,
) -> Option<T> {
    npcs.into_iter()
        .map(|(npc, at, elevation)| (npc, at.distance(feet.0), (elevation - feet.1).abs()))
        .filter(|&(_, distance, height)| distance <= TALK_RANGE && height <= TALK_HEIGHT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(npc, ..)| npc)
}

/// Spawns every scene's NPCs and lets characters talk to them. Needs [`crate::MapsPlugin`] and
/// [`crate::CharactersPlugin`] first.
pub struct TalkPlugin;

impl Plugin for TalkPlugin {
    fn build(self, app: &mut App) {
        let maps = app.world.resource::<Maps>();
        let sheets = app.world.resource::<CharacterSheets>();
        let spawns: Vec<_> = maps
            .maps
            .iter()
            .enumerate()
            .flat_map(|(i, map)| map.def.npcs.iter().map(move |npc| (i, npc)))
            .map(|(i, npc)| {
                let look = sheets
                    .id_of(&npc.look())
                    .expect("CharacterSheets is built from these same maps");
                (
                    body_bundle(MapId(i as u16), Vec2::from(npc.position), CHARACTER_RADIUS),
                    CharacterState::new(sheets, look, npc.facing),
                    Npc {
                        lines: npc.lines.clone(),
                        next: HashMap::new(),
                        home: npc.facing,
                    },
                    npc.actor.clone().map(crate::Person),
                )
            })
            .collect();
        app.world.init_resource::<NextNetId>();
        for (body, state, npc, person) in spawns {
            let id = app.world.resource_mut::<NextNetId>().allocate();
            let mut spawned = app
                .world
                .spawn((body, state, npc, ControlInput::default(), id));
            if let Some(person) = person {
                spawned.insert(person);
            }
        }
        // Before the controllers, so an NPC turned to its speaker shows that way in the same tick.
        app.add_systems(
            FixedUpdate,
            (age_speech, face_home, talk)
                .chain()
                .before(control_characters)
                .in_set(Control),
        );
    }
}

/// Lines run out after [`SPEECH_TICKS`], or as soon as the one spoken to walks away (further than
/// [`LEAVE_RANGE`] or into another map): the conversation is over.
fn age_speech(
    mut commands: Commands,
    mut speakers: Query<Speaker>,
    everyone: Query<(&NetId, &MapId, &BodyState)>,
) {
    for (entity, mut speech, map, body) in &mut speakers {
        speech.ticks_left = speech.ticks_left.saturating_sub(1);
        let left = speech.to.is_some_and(|to| {
            !everyone.iter().any(|(id, m, other)| {
                *id == to && m == map && other.0.position.distance(body.0.position) <= LEAVE_RANGE
            })
        });
        if speech.ticks_left == 0 || left {
            commands.entity(entity).remove::<Speech>();
        }
    }
}

/// An NPC standing its ground: silent, and following nobody.
type AtPost = (Without<Speech>, Without<crate::Companion>);

/// An NPC nobody is speaking with, neither it nor anyone to it, turns back its own way.
fn face_home(mut npcs: Query<(&NetId, &Npc, &mut CharacterState), AtPost>, speech: Query<&Speech>) {
    for (id, npc, mut state) in &mut npcs {
        if state.facing != npc.home && !speech.iter().any(|s| s.to == Some(*id)) {
            state.facing = npc.home;
        }
    }
}

/// Anyone saying something.
type Speaker = (
    Entity,
    &'static mut Speech,
    &'static MapId,
    &'static BodyState,
);

/// Who can start a conversation: any awake character that is not itself an NPC.
type Talker = (
    Entity,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static ControlInput,
    &'static CharacterState,
);

/// Who can be talked to.
type Listener = (
    Entity,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static mut Npc,
    &'static mut CharacterState,
);

fn talk(
    mut commands: Commands,
    talkers: Query<Talker, (Without<Npc>, Without<Asleep>)>,
    mut npcs: Query<Listener>,
) {
    for (talker, talker_id, map, body, control, state) in &talkers {
        // Lying down asleep or out cold, nobody talks.
        if !control.input.interact || control.hold || state.sleeping || state.impaired.out {
            continue;
        }
        let feet = (body.0.position, body.0.elevation);
        let in_map =
            npcs.iter()
                .filter(|(_, _, m, ..)| *m == map)
                .map(|(entity, _, _, npc_body, ..)| {
                    (entity, npc_body.0.position, npc_body.0.elevation)
                });
        let Some(target) = talk_target(feet, in_map) else {
            continue;
        };
        let Ok((entity, npc_id, _, npc_body, mut npc, mut state)) = npcs.get_mut(target) else {
            continue;
        };
        if npc.lines.is_empty() {
            continue;
        }
        if let Some(facing) = Facing::from_vector(feet.0 - npc_body.0.position) {
            state.facing = facing;
        }
        let Npc { lines, next, .. } = &mut *npc;
        let step = next.entry(*talker_id).or_default();
        let line = lines[*step % lines.len()].clone();
        *step = (*step + 1) % lines.len();
        // One line of the conversation is up at a time: the other side falls quiet.
        let (speaker, silent, to) = match &line {
            LineDef::Says(_) => (entity, talker, *talker_id),
            LineDef::Reply { .. } => (talker, entity, *npc_id),
        };
        tracing::debug!(
            "{} says {}",
            if speaker == entity { "NPC" } else { "talker" },
            line.key()
        );
        commands.entity(silent).remove::<Speech>();
        commands.entity(speaker).insert(Speech {
            line: line.key().to_owned(),
            to: Some(to),
            ticks_left: SPEECH_TICKS,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talks_to_the_nearest_npc_in_reach_at_the_same_height() {
        let feet = (Vec2::new(100.0, 100.0), 0.0);
        let npcs = [
            ("far", Vec2::new(140.0, 100.0), 0.0),
            ("near", Vec2::new(120.0, 100.0), 0.0),
            ("nearest but up a ledge", Vec2::new(105.0, 100.0), 16.0),
        ];
        assert_eq!(talk_target(feet, npcs), Some("near"));
        assert_eq!(talk_target(feet, [npcs[0]]), None);
    }
}
