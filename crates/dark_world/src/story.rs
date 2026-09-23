//! Storylets in play (docs/PLAN.md §17): conversations with people of the world, with choices.
//!
//! Interact near a person with a storylet open for you and they tell it instead of their plain
//! lines: their line stays up in your dialogue window, the choices open to you now come to you
//! alone (in your snapshots), and a choice key answers. You say your line, then they say the next
//! node's, or the conversation ends. Walking away ends it too. Choices act on the world
//! simulation and the [`dark_story::Story`] (saved with the world); what they ask of the game (a
//! fade to black, items handed over, going over to the other side) is done here. A player who
//! serves a hostile faction is on the enemies' side: their old friends can fight them.

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use dark_core::{App, FixedUpdate, Plugin};
use dark_net::PlayerId;
use dark_sim::{ActorId, FactionId};
use dark_story::{Conversation, Deed, Story, StoryDef};

use crate::characters::{Asleep, CharacterState, Control, ControlInput, NetId, PlayerAvatar};
use crate::combat::Hostile;
use crate::life::Life;
use crate::party::Person;
use crate::talk::{HELD_TICKS, LEAVE_RANGE, Npc, Speech, talk_target};
use crate::world_sim::{WorldState, advance_world};
use crate::{BodyState, MapId, WorldStep};

/// Ticks the player's answer shows before the person's reply.
const ANSWER_TICKS: u32 = 90;

/// The story, what it has left behind, and the conversations under way.
#[derive(Resource)]
pub struct StoryState {
    pub def: StoryDef,
    pub story: Story,
    talks: Vec<Talk>,
    /// Players whose conversation ended last tick: free again this tick, so the press that
    /// ended it does not also start plain talk.
    release: Vec<Entity>,
    /// How the year ended, once it has (a string key).
    pub ending: Option<String>,
}

/// The story state carried on from a save, for [`StoryPlugin`] to pick up.
#[derive(Resource, Default)]
pub(crate) struct SavedStory(pub(crate) Story);

struct Talk {
    player: Entity,
    person: Entity,
    conversation: Conversation,
    /// The player's answer is showing; the person replies (with this node, or not) after it.
    answering: Option<(u32, Option<Conversation>)>,
}

/// In a conversation: plain talk leaves them be.
#[derive(Component, Clone, Copy, Debug)]
pub struct Conversing;

impl StoryState {
    /// Whether this person has a storylet under way — including the moment a player's answer
    /// is showing, when neither side is saying anything. They keep still through all of it.
    pub(crate) fn busy_with(&self, person: Entity) -> bool {
        self.talks.iter().any(|talk| talk.person == person)
    }
}

/// Fades to black the player's character has been through, counted: the view fades when it
/// changes. Replicated to the player.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faded(pub u32);

/// Runs storylets. Needs [`crate::WorldSimPlugin`], [`crate::TalkPlugin`] and
/// [`crate::PartyPlugin`] first.
pub struct StoryPlugin(pub StoryDef);

impl Plugin for StoryPlugin {
    fn build(self, app: &mut App) {
        let story = app
            .world
            .remove_resource::<SavedStory>()
            .map(|s| s.0)
            .unwrap_or_default();
        app.insert_resource(StoryState {
            def: self.0,
            story,
            talks: Vec::new(),
            release: Vec::new(),
            ending: None,
        })
        .add_systems(
            FixedUpdate,
            converse.before(crate::talk::talk).in_set(Control),
        )
        .add_systems(
            FixedUpdate,
            (match_sides, end_the_year)
                .after(advance_world)
                .in_set(WorldStep),
        );
    }
}

/// The world actor playing `player`, if they have one yet.
fn actor_of(world: &WorldState, player: PlayerId) -> Option<ActorId> {
    let key = player.0.as_u128();
    world
        .sim
        .world()
        .actors
        .iter()
        .position(|a| a.player == Some(key))
        .map(|i| ActorId(i as u16))
}

/// A player who might talk.
type Speaker = (
    Entity,
    &'static PlayerAvatar,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static ControlInput,
    &'static CharacterState,
    Has<Conversing>,
);

/// A person who might be talked to (turned to face the speaker).
type Listener = (
    Entity,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static Person,
);

#[allow(clippy::too_many_arguments)]
fn converse(
    mut commands: Commands,
    world: Option<ResMut<WorldState>>,
    mut state: ResMut<StoryState>,
    players: Query<Speaker, (Without<Asleep>, Without<Npc>)>,
    people: Query<Listener, With<Npc>>,
    mut facings: Query<&mut CharacterState, (With<Npc>, Without<PlayerAvatar>)>,
    speeches: Query<&Speech>,
    npcs: Query<&Npc>,
    mut lives: Query<&mut Life>,
    mut faded: Query<&mut Faded>,
) {
    let Some(mut world) = world else {
        return;
    };
    let state = &mut *state;
    for player in std::mem::take(&mut state.release) {
        if let Ok(mut entity) = commands.get_entity(player) {
            entity.remove::<Conversing>();
        }
    }
    let say = |commands: &mut Commands, who: Entity, line: &str, to: NetId, ticks: u32| {
        commands.entity(who).insert(Speech {
            line: line.to_owned(),
            to: Some(to),
            ticks_left: ticks,
        });
    };
    // Under way: answers, replies, walking away.
    let mut ended = Vec::new();
    for (i, talk) in state.talks.iter_mut().enumerate() {
        let (Ok(player), Ok(person)) = (players.get(talk.player), people.get(talk.person)) else {
            ended.push(i);
            continue;
        };
        let (_, _, player_id, map, body, control, ..) = player;
        let (_, person_id, person_map, person_body, _) = person;
        let apart = body.0.position.distance(person_body.0.position) > LEAVE_RANGE;
        if map != person_map || apart {
            ended.push(i);
            continue;
        }
        if let Some((ticks, next)) = &mut talk.answering {
            *ticks = ticks.saturating_sub(1);
            if *ticks > 0 {
                continue;
            }
            match next.take() {
                Some(next) => {
                    let view = state.story.view(&state.def, &world.sim, &next);
                    say(
                        &mut commands,
                        talk.person,
                        &view.line,
                        *player_id,
                        HELD_TICKS,
                    );
                    talk.conversation = next;
                    talk.answering = None;
                }
                None => ended.push(i),
            }
            continue;
        }
        let view = state.story.view(&state.def, &world.sim, &talk.conversation);
        if view.choices.is_empty() {
            // The last line: interact ends it, heard out.
            if control.input.interact && !control.hold {
                state
                    .story
                    .finish(&state.def, &world.sim, &talk.conversation);
                ended.push(i);
            }
            continue;
        }
        // `choice` is the node's choice index plus one, as the snapshot showed it; one not open
        // now does nothing.
        let Some(index) = usize::from(control.input.choice).checked_sub(1) else {
            continue;
        };
        let Some((index, says)) = view
            .choices
            .iter()
            .find(|(i, _)| *i == index)
            .cloned()
            .filter(|_| !control.hold)
        else {
            continue;
        };
        let (next, deeds) =
            state
                .story
                .choose(&state.def, &mut world.sim, &talk.conversation, index);
        commands.entity(talk.person).remove::<Speech>();
        say(&mut commands, talk.player, &says, *person_id, ANSWER_TICKS);
        carry_out(&mut commands, talk.player, &deeds, &mut lives, &mut faded);
        talk.answering = Some((ANSWER_TICKS, next));
    }
    for i in ended.into_iter().rev() {
        let talk = state.talks.remove(i);
        state.release.push(talk.player);
        // Their line to this player ends with the conversation (not one they have since said
        // to anyone else, like a traitor's last words).
        let to_player = players.get(talk.player).ok().map(|p| *p.2);
        let saying_to_player = speeches
            .get(talk.person)
            .is_ok_and(|s| s.to.is_some() && s.to == to_player);
        if saying_to_player && let Ok(mut person) = commands.get_entity(talk.person) {
            person.remove::<Speech>();
        }
    }
    // Start (after the others, so the press that starts one is not also an answer):
    // interact near a person with a storylet open for this player.
    for (entity, avatar, id, map, body, control, character, conversing) in &players {
        // As plain talk: nobody asleep, out cold or dead starts a conversation.
        let unable = character.sleeping || character.impaired.out || character.fighter.is_dead();
        if conversing || unable || control.hold || !control.input.interact {
            continue;
        }
        let Some(player) = crate::party::player_actor(&mut world, avatar.0, *map) else {
            continue;
        };
        // A sleeper is left to sleep, storylet or no storylet (§21). Their state is read
        // through `facings`, the one query that holds it.
        let near = people
            .iter()
            .filter(|&(e, _, m, ..)| {
                *m == *map && facings.get(e).is_ok_and(|person| !person.sleeping)
            })
            .map(|(e, _, _, b, _)| (e, b.0.position, b.0.elevation));
        let Some(target) = talk_target((body.0.position, body.0.elevation), near) else {
            continue;
        };
        let Ok((person_entity, .., who)) = people.get(target) else {
            continue;
        };
        // One conversation at a time, a storylet or plain talk: another player waits their turn.
        let plain = npcs
            .get(person_entity)
            .ok()
            .and_then(Npc::talking_with)
            .is_some_and(|with| with != *id);
        if plain || state.talks.iter().any(|t| t.person == person_entity) {
            continue;
        }
        let Some(person) = world.sim.world().actor(&who.0) else {
            continue;
        };
        let Some(conversation) = state.story.open(&state.def, &world.sim, player, person) else {
            continue;
        };
        let deeds = state.story.begin(&state.def, &mut world.sim, &conversation);
        let view = state.story.view(&state.def, &world.sim, &conversation);
        commands
            .entity(entity)
            .insert(Conversing)
            .remove::<Speech>();
        say(&mut commands, person_entity, &view.line, *id, HELD_TICKS);
        if let Ok(mut facing) = facings.get_mut(person_entity)
            && let Some(towards) = dark_sprite::Facing::from_vector(
                body.0.position - people_position(&people, person_entity),
            )
        {
            facing.facing = towards;
        }
        carry_out(&mut commands, entity, &deeds, &mut lives, &mut faded);
        state.talks.push(Talk {
            player: entity,
            person: person_entity,
            conversation,
            answering: None,
        });
    }
}

fn people_position(people: &Query<Listener, With<Npc>>, person: Entity) -> glam::Vec2 {
    people
        .get(person)
        .map_or(glam::Vec2::ZERO, |(_, _, _, b, _)| b.0.position)
}

/// Does what a choice asks of the game.
fn carry_out(
    commands: &mut Commands,
    player: Entity,
    deeds: &[Deed],
    lives: &mut Query<&mut Life>,
    faded: &mut Query<&mut Faded>,
) {
    let fades = deeds.iter().filter(|d| **d == Deed::FadeToBlack).count() as u32;
    if fades > 0 {
        match faded.get_mut(player) {
            Ok(mut f) => f.0 += fades,
            Err(_) => {
                commands.entity(player).insert(Faded(fades));
            }
        }
    }
    for deed in deeds {
        match deed {
            Deed::FadeToBlack => {}
            Deed::Give { item, count } => {
                if let Ok(mut life) = lives.get_mut(player) {
                    life.inventory.add(item, *count);
                }
            }
            // Sides follow factions (see `match_sides`).
            Deed::Defected(_) => {}
        }
    }
}

/// A player serving a hostile faction is on the enemies' side; one who is not, is not.
fn match_sides(
    mut commands: Commands,
    world: Option<Res<WorldState>>,
    players: Query<(Entity, &PlayerAvatar, Has<Hostile>)>,
) {
    let Some(world) = world else {
        return;
    };
    for (entity, avatar, hostile) in &players {
        let Some(actor) = actor_of(&world, avatar.0) else {
            continue;
        };
        let w = world.sim.world();
        let enemy = w.is_hostile(w.get(actor).faction);
        if enemy && !hostile {
            tracing::info!("{} goes over to the enemy", avatar.0);
            commands.entity(entity).insert(Hostile);
        } else if !enemy && hostile {
            commands.entity(entity).remove::<Hostile>();
        }
    }
}

/// When the year is over, the story's endings decide how it ended.
fn end_the_year(world: Option<Res<WorldState>>, mut state: ResMut<StoryState>) {
    let Some(world) = world else {
        return;
    };
    if state.ending.is_some() || !world.sim.is_year_over() {
        return;
    }
    let ending = state
        .story
        .ending(&state.def, &world.sim)
        .map(|e| (e.id.clone(), e.text.clone()));
    if let Some((id, text)) = ending {
        tracing::info!("the year is over: ending {id}");
        state.ending = Some(text);
    }
}

/// The choices open to `player` in their conversation now, for their snapshot: each as the
/// node's choice index and what the player would say (a string key).
pub(crate) fn choices_of(
    state: &StoryState,
    world: &WorldState,
    player: Entity,
) -> Vec<(u8, String)> {
    state
        .talks
        .iter()
        .find(|t| t.player == player && t.answering.is_none())
        .map(|t| {
            state
                .story
                .view(&state.def, &world.sim, &t.conversation)
                .choices
                .into_iter()
                .filter_map(|(i, says)| u8::try_from(i).ok().map(|i| (i, says)))
                .collect()
        })
        .unwrap_or_default()
}

/// A player's faction standing and people's feelings (see [`StoryView`]).
pub type Ties = (Vec<(String, i32)>, Vec<(String, i32)>);

/// Where `player` stands with each faction, and how the people who feel anything about them
/// feel, as name keys and values.
pub(crate) fn ties_of(state: &StoryState, sim: &WorldState, player: PlayerId) -> Ties {
    let Some(me) = actor_of(sim, player) else {
        return Ties::default();
    };
    let world = sim.sim.world();
    let standing = world
        .factions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), world.standing(me, FactionId(i as u16))))
        .collect();
    let feelings = world
        .actors
        .iter()
        .enumerate()
        .filter(|(_, a)| a.player.is_none() && a.alive)
        .filter_map(|(i, a)| {
            let felt = state.story.affinity(ActorId(i as u16), me);
            (felt != 0).then(|| (a.name.clone(), felt))
        })
        .collect();
    (standing, feelings)
}

/// What `player`'s screen shows of the story: the choices open to them, fades so far, and the
/// ending once the year is over.
pub fn story_view(world: &mut World, player: PlayerId) -> StoryView {
    let entity = world
        .query::<(Entity, &PlayerAvatar)>()
        .iter(world)
        .find(|(_, a)| a.0 == player)
        .map(|(e, _)| e);
    let faded = entity
        .and_then(|e| world.get::<Faded>(e).copied())
        .unwrap_or_default()
        .0;
    let (Some(state), Some(sim)) = (
        world.get_resource::<StoryState>(),
        world.get_resource::<WorldState>(),
    ) else {
        return StoryView::default();
    };
    let (standing, feelings) = ties_of(state, sim, player);
    StoryView {
        choices: entity
            .map(|e| choices_of(state, sim, e))
            .unwrap_or_default(),
        faded,
        ending: state.ending.clone(),
        standing,
        feelings,
    }
}

/// What a player's screen shows of the story. Replicated to them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoryView {
    /// The node's choice index (send it plus one as `TickInput::choice`) and what is said.
    pub choices: Vec<(u8, String)>,
    pub faded: u32,
    pub ending: Option<String>,
    /// The player's standing with each faction (name key, −1000 to 1000).
    pub standing: Vec<(String, i32)>,
    /// How people feel about the player (name key, −1000 to 1000), those who feel anything.
    pub feelings: Vec<(String, i32)>,
}
