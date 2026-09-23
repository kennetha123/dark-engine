//! Parties in play (docs/PLAN.md §15): the world simulation's standing, parties and loyalty
//! ([`dark_sim::social`]) meeting the characters on the maps.
//!
//! A scene NPC that is a person of the world ([`Person`]) can be asked to follow (recruit near
//! them); a follower is a [`Companion`]: it walks after its leader (into other maps too), fights
//! the enemies around them and can be hurt. Recruit near your own follower sends it away, near
//! another player invites them (the same back accepts), and with nobody near leaves your party.
//! Who follows whom is the world simulation's: companions are made to match its parties every
//! tick. Followers watch their leader: soiling yourself or getting drunk in front of them costs
//! loyalty, winning fights beside them earns it; a follower whose loyalty runs out leaves, and one
//! who turns on the party becomes an enemy on the spot. Slaying monsters earns standing with
//! every friendly faction.

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use dark_combat::AiDef;
use dark_core::{App, FixedUpdate, Plugin};
use dark_life::Status;
use dark_net::PlayerId;
use dark_sim::{ActorId, Happening, Refusal};
use glam::Vec2;

use crate::ai::Brain;
use crate::characters::{
    Asleep, CHARACTER_RADIUS, CharacterState, Control, ControlInput, NetId, PlayerAvatar,
    control_characters,
};
use crate::combat::{Fight, Hostile, Post, resolve_hits};
use crate::life::{Life, NightPass};
use crate::talk::{Npc, SPEECH_TICKS, Speech, talk_target};
use crate::world_sim::{WorldState, advance_world};
use crate::{BodyState, Dormant, MapId, Maps, PreviousBody, WorldStep, nav};

/// A world person standing in a scene (the actor's id in `world.ron`).
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Person(pub String);

/// Following `leader` (a player's character).
#[derive(Component, Clone, Debug)]
pub struct Companion {
    pub leader: Entity,
    path: Vec<Vec2>,
    repath_in: u16,
}

/// Another player asked this one to travel together; recruit near them within the time accepts.
#[derive(Component, Clone, Copy, Debug)]
struct Invited {
    by: Entity,
    ticks_left: u32,
}

/// An invitation stands this long (10 s).
const INVITE_TICKS: u32 = 600;
/// Recruit works at most this often per player (half a second): holding it does nothing more.
const RECRUIT_TICKS: u32 = 30;
/// A companion keeps within this of its leader...
const FOLLOW_NEAR: f32 = 28.0;
/// ...runs to catch up past this...
const FOLLOW_RUN: f32 = 80.0;
/// ...fights enemies within this of itself, if they are within `GUARD` of the leader.
const FOLLOWER_SIGHT: f32 = 110.0;
const GUARD: f32 = 160.0;
const FOLLOWER_REACH: f32 = 16.0;
const REACH_HEIGHT: f32 = 8.0;
const REPATH_TICKS: u16 = 20;
/// Followers see what their leader does within this.
const WITNESS: f32 = 120.0;
/// Loyalty a follower gains when an enemy falls near it.
const VICTORY_LOYALTY: i32 = 15;
/// Loyalty lost for watching the leader soil themselves, and per game minute of watching them
/// wasted or filthy.
const ACCIDENT_SHAME: i32 = 150;
const SHAME_PER_MINUTE: i32 = 1;
/// Followers speak up about the leader's behaviour at most this often (ticks: 30 s).
const REMARK_TICKS: u32 = 1800;

/// Ties the world simulation's parties to characters. Needs [`crate::WorldSimPlugin`],
/// [`crate::CombatPlugin`], [`crate::TalkPlugin`] and [`crate::LifePlugin`] first.
pub struct PartyPlugin;

impl Plugin for PartyPlugin {
    fn build(self, app: &mut App) {
        app.world.init_resource::<PartyRoster>();
        // A scene's person who is not in the world would silently never answer.
        if let Some(world) = app.world.get_resource::<WorldState>() {
            let maps = app.world.resource::<Maps>();
            for map in &maps.maps {
                for actor in map.def.npcs.iter().filter_map(|n| n.actor.as_deref()) {
                    if world.sim.world().actor(actor).is_none() {
                        tracing::warn!("{}: actor {actor} is not in the world", map.name);
                    }
                }
            }
        }
        app.add_systems(
            FixedUpdate,
            (age_invitations, socialize, follow)
                .chain()
                .after(crate::ai::think)
                .before(control_characters)
                .in_set(Control),
        )
        .add_systems(FixedUpdate, credit.after(resolve_hits).in_set(Fight))
        .add_systems(
            FixedUpdate,
            // Reactions reach the world simulation before it reports this tick's events.
            (
                react.after(crate::life::live).before(advance_world),
                match_parties.after(advance_world),
            )
                .after(NightPass)
                .in_set(WorldStep),
        );
    }
}

/// The world actor playing `player`, if their map is in a region (made the first time), placed
/// in that region now (the world simulation otherwise hears where players are once an hour).
pub(crate) fn player_actor(
    world: &mut WorldState,
    player: PlayerId,
    map: MapId,
) -> Option<ActorId> {
    let region = world.region_of(map)?;
    let actor = world.sim.player_actor(player.0.as_u128(), region);
    world.sim.move_player(actor, region);
    Some(actor)
}

/// A person of the world someone may ask along.
type Asked = (
    Entity,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static Person,
    Option<&'static Companion>,
    &'static CharacterState,
);

/// A player asking someone to join, leaving, inviting another player.
type Asker = (
    Entity,
    &'static PlayerAvatar,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static ControlInput,
    &'static CharacterState,
    Option<&'static Invited>,
    Has<RecruitWait>,
);

/// Invitations lapse; a recent recruit press wears off.
fn age_invitations(
    mut commands: Commands,
    mut invites: Query<(Entity, &mut Invited)>,
    mut waits: Query<(Entity, &mut RecruitWait)>,
) {
    for (entity, mut invite) in &mut invites {
        invite.ticks_left = invite.ticks_left.saturating_sub(1);
        if invite.ticks_left == 0 {
            commands.entity(entity).remove::<Invited>();
        }
    }
    for (entity, mut wait) in &mut waits {
        wait.0 = wait.0.saturating_sub(1);
        if wait.0 == 0 {
            commands.entity(entity).remove::<RecruitWait>();
        }
    }
}

/// Ticks until this player's recruit works again.
#[derive(Component, Clone, Copy, Debug)]
struct RecruitWait(u32);

fn socialize(
    mut commands: Commands,
    mut world: Option<ResMut<WorldState>>,
    askers: Query<Asker, Without<Asleep>>,
    people: Query<Asked, With<Npc>>,
) {
    let Some(world) = world.as_deref_mut() else {
        return;
    };
    for (me, avatar, my_id, map, body, control, state, invited, waiting) in &askers {
        let busy = control.hold || state.sleeping || state.impaired.out || waiting;
        if !control.input.recruit || busy {
            continue;
        }
        commands.entity(me).insert(RecruitWait(RECRUIT_TICKS));
        let Some(actor) = player_actor(world, avatar.0, *map) else {
            continue;
        };
        let feet = (body.0.position, body.0.elevation);
        let say = |commands: &mut Commands, speaker: Entity, line: &str, to: Option<NetId>| {
            commands.entity(speaker).insert(Speech {
                line: line.into(),
                to,
                ticks_left: SPEECH_TICKS,
            });
        };
        // A person of the world in reach: ask them along, or send your own follower away.
        let person = talk_target(
            feet,
            people
                .iter()
                .filter(|(_, _, m, ..)| *m == map)
                .map(|(e, _, _, b, ..)| (e, b.0.position, b.0.elevation)),
        );
        // A sleeper (§21) is let be — and asking beside one is not "nobody in reach", which
        // would have the player walk out of their own party.
        if person.is_some_and(|target| {
            people
                .get(target)
                .is_ok_and(|(.., person_state)| person_state.sleeping)
        }) {
            continue;
        }
        if let Some(target) = person
            && let Ok((npc, _, _, _, who, companion, _)) = people.get(target)
        {
            let Some(npc_actor) = world.sim.world().actor(&who.0) else {
                continue;
            };
            let line = if companion.is_some_and(|c| c.leader == me) {
                world.sim.dismiss(actor, npc_actor);
                "party.farewell"
            } else {
                match world.sim.ask_to_join(actor, npc_actor) {
                    Ok(()) => "party.yes",
                    Err(Refusal::Distrust) => "party.no.distrust",
                    Err(Refusal::Grudge) => "party.no.grudge",
                    Err(Refusal::Enemy) => "party.no.enemy",
                    Err(Refusal::Busy) => "party.no.busy",
                    Err(Refusal::Full) => "party.no.full",
                    Err(_) => "party.no",
                }
            };
            commands.entity(me).remove::<Speech>();
            say(&mut commands, npc, line, Some(*my_id));
            continue;
        }
        // Another player in reach: invite them, or accept their invitation.
        let other = talk_target(
            feet,
            askers
                .iter()
                .filter(|(e, _, _, m, ..)| *e != me && *m == map)
                .map(|(e, _, _, _, b, ..)| (e, b.0.position, b.0.elevation)),
        );
        if let Some(other) = other
            && let Ok((_, other_avatar, other_id, ..)) = askers.get(other)
        {
            if invited.is_some_and(|i| i.by == other) {
                let Some(inviter) = player_actor(world, other_avatar.0, *map) else {
                    continue;
                };
                let line = match world.sim.join_players(inviter, actor) {
                    Ok(()) => "party.together",
                    Err(Refusal::Full) => "party.no.full",
                    Err(_) => "party.no",
                };
                commands.entity(me).remove::<Invited>();
                say(&mut commands, me, line, Some(*other_id));
            } else {
                commands.entity(other).insert(Invited {
                    by: me,
                    ticks_left: INVITE_TICKS,
                });
                say(&mut commands, me, "party.invite", Some(*other_id));
            }
            continue;
        }
        // Nobody in reach: leave the party.
        if world.sim.party_of(actor).is_some() {
            world.sim.leave_party(actor);
            say(&mut commands, me, "party.leave", None);
        }
    }
}

/// A companion and what it moves.
type Follower = (
    &'static mut Companion,
    &'static mut MapId,
    &'static mut BodyState,
    &'static mut PreviousBody,
    &'static CharacterState,
    &'static mut ControlInput,
);

/// Followers are neither players nor enemies (so their queries never overlap).
type FollowerOnly = (Without<PlayerAvatar>, Without<Hostile>, Without<Dormant>);

/// An enemy a follower may fight.
type Foe = (
    Entity,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
);

/// Companions follow their leader and fight the enemies around them.
fn follow(
    maps: Res<Maps>,
    mut followers: Query<Follower, FollowerOnly>,
    leaders: Query<(&MapId, &BodyState), With<PlayerAvatar>>,
    foes: Query<Foe, (With<Hostile>, Without<Dormant>)>,
) {
    for (mut companion, mut map, mut body, mut previous, state, mut control) in &mut followers {
        *control = ControlInput::default();
        let Ok((leader_map, leader)) = leaders.get(companion.leader) else {
            continue;
        };
        // The leader went through an exit: the companion comes along, just behind them if that is
        // open ground, else where they stand.
        if *leader_map != *map {
            let behind = leader.0.position + Vec2::new(0.0, 10.0);
            let world = &maps.get(*leader_map).collision;
            let open = world.ground_under(behind, CHARACTER_RADIUS).is_finite()
                && !world
                    .colliders
                    .iter()
                    .any(|c| c.height > 0.0 && crate::maps::overlaps(c, behind, CHARACTER_RADIUS));
            let at = if open { behind } else { leader.0.position };
            crate::combat::place(&mut body, &mut previous, at, &maps, *leader_map);
            *map = *leader_map;
            companion.path.clear();
            continue;
        }
        if !state.fighter.is_free() {
            continue;
        }
        let me = body.0.position;
        let near_leader = |at: Vec2| at.distance(leader.0.position) <= GUARD;
        let foe = foes
            .iter()
            .filter(|(e, ..)| *e != companion.leader)
            .map(|(_, m, b, s)| (m, b, s))
            .filter(|(m, b, s)| {
                *m == &*map
                    && !s.fighter.is_dead()
                    && (b.0.elevation - body.0.elevation).abs() <= REACH_HEIGHT
                    && b.0.position.distance(me) <= FOLLOWER_SIGHT
                    && near_leader(b.0.position)
            })
            .map(|(_, b, _)| b.0.position)
            .min_by(|a, b| a.distance(me).total_cmp(&b.distance(me)));
        let input = &mut control.input;
        let (goal, stop) = match foe {
            Some(foe) if foe.distance(me) <= FOLLOWER_REACH => {
                input.movement = (foe - me).normalize_or_zero() * 0.05;
                input.attack = true;
                continue;
            }
            Some(foe) => (foe, 0.0),
            None => (leader.0.position, FOLLOW_NEAR),
        };
        let distance = goal.distance(me);
        if distance <= stop {
            companion.path.clear();
            continue;
        }
        let world = &maps.get(*map).collision;
        let (r, e) = (CHARACTER_RADIUS, body.0.elevation);
        let toward = if nav::clear_line(world, me, goal, r, e, &maps.params) {
            companion.path.clear();
            Some(goal)
        } else {
            if companion.repath_in == 0 {
                companion.path =
                    nav::find_path(world, me, goal, r, e, &maps.params).unwrap_or_default();
                companion.repath_in = REPATH_TICKS;
            }
            companion.repath_in -= 1;
            while companion
                .path
                .first()
                .is_some_and(|p| p.distance(me) <= 4.0)
            {
                companion.path.remove(0);
            }
            companion.path.first().copied()
        };
        if let Some(toward) = toward {
            input.movement = (toward - me).normalize_or_zero();
            input.run = distance > FOLLOW_RUN;
        }
    }
}

/// Deaths the world cares about, each seen once: a person of the world dies for good (and
/// leaves the world's parties), and a monster slain by a player earns that player standing and
/// their nearby followers' loyalty.
fn credit(
    mut commands: Commands,
    mut world: Option<ResMut<WorldState>>,
    fallen: Query<Fallen, Without<Counted>>,
    counted: Query<(Entity, &CharacterState), With<Counted>>,
    players: Query<(&NetId, &PlayerAvatar, &MapId)>,
    companions: Query<(&Person, &MapId, &BodyState), With<Companion>>,
) {
    // Up again (a player, an enemy back at its post): its next death counts.
    for (entity, state) in &counted {
        if !state.fighter.is_dead() {
            commands.entity(entity).remove::<Counted>();
        }
    }
    for (entity, map, body, state, hostile, person, player_fell) in &fallen {
        if !state.fighter.is_dead() {
            continue;
        }
        commands.entity(entity).insert(Counted);
        let Some(world) = world.as_deref_mut() else {
            continue;
        };
        if let Some(person) = person
            && let Some(actor) = world.sim.world().actor(&person.0)
        {
            world.sim.kill(actor, None);
            commands.entity(entity).insert(Mortal);
        }
        // Slaying the other side earns standing; a player gone over to it is still a person.
        if !hostile || player_fell {
            continue;
        }
        // Whoever landed the killing blow, if a player.
        let Some((_, killer, killer_map)) = players
            .iter()
            .find(|(id, ..)| Some(id.0) == state.fighter.hit_by)
        else {
            continue;
        };
        let Some(actor) = player_actor(world, killer.0, *killer_map) else {
            continue;
        };
        world.sim.slain_enemy(actor);
        for (who, m, b) in &companions {
            if m == map
                && b.0.position.distance(body.0.position) <= WITNESS
                && let Some(follower) = world.sim.world().actor(&who.0)
                && world
                    .sim
                    .party_of(follower)
                    .is_some_and(|p| p.members.contains(&actor))
            {
                world.sim.sway(follower, VICTORY_LOYALTY);
            }
        }
    }
}

/// Someone who may have died.
type Fallen = (
    Entity,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    Has<Hostile>,
    Option<&'static Person>,
    Has<PlayerAvatar>,
);

/// A death already dealt with.
#[derive(Component, Clone, Copy, Debug)]
struct Counted;

/// A person of the world who died: gone for good once their body has lain its time.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct Mortal;

/// What followers saw their leader do since they last looked, from the leader's [`Life`].
fn react(
    mut commands: Commands,
    mut world: Option<ResMut<WorldState>>,
    mut leaders: Query<(Entity, &NetId, &MapId, &BodyState, &mut Life), With<PlayerAvatar>>,
    mut companions: Query<(
        Entity,
        &Person,
        &Companion,
        &MapId,
        &BodyState,
        &mut Remarks,
    )>,
    fresh: Query<Entity, (With<Companion>, Without<Remarks>)>,
    rules: Option<Res<crate::LifeRules>>,
) {
    for entity in &fresh {
        commands.entity(entity).insert(Remarks::default());
    }
    let (Some(world), Some(rules)) = (world.as_deref_mut(), rules) else {
        return;
    };
    for (leader, leader_id, map, body, mut life) in &mut leaders {
        let seen = std::mem::take(&mut life.witnessed);
        if seen.is_quiet() {
            continue;
        }
        let statuses = life.body.condition(&rules.def.rates).statuses;
        for (entity, person, companion, m, b, mut remarks) in &mut companions {
            if companion.leader != leader
                || m != map
                || b.0.position.distance(body.0.position) > WITNESS
            {
                continue;
            }
            let Some(actor) = world.sim.world().actor(&person.0) else {
                continue;
            };
            let shame = seen.accidents * ACCIDENT_SHAME + seen.shameful_minutes * SHAME_PER_MINUTE;
            world.sim.sway(actor, -shame);
            let remark = if seen.accidents > 0 {
                Some("react.accident")
            } else if statuses.contains(&Status::Wasted) || statuses.contains(&Status::Drunk) {
                Some("react.drunk")
            } else if statuses.contains(&Status::Soiled) || statuses.contains(&Status::Filthy) {
                Some("react.filthy")
            } else {
                None
            };
            if let Some(line) = remark
                && (seen.accidents > 0 || remarks.quiet == 0)
            {
                remarks.quiet = REMARK_TICKS;
                commands.entity(entity).insert(Speech {
                    line: line.into(),
                    to: Some(*leader_id),
                    ticks_left: SPEECH_TICKS,
                });
            }
        }
    }
    for (.., mut remarks) in &mut companions {
        remarks.quiet = remarks.quiet.saturating_sub(1);
    }
}

/// When a companion last spoke up about its leader.
#[derive(Component, Clone, Copy, Debug, Default)]
struct Remarks {
    quiet: u32,
}

/// What a body did that others would notice, gathered minute by minute (see [`Life`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Witnessed {
    pub(crate) accidents: i32,
    /// Minutes spent drunk, soiled or filthy.
    pub(crate) shameful_minutes: i32,
}

impl Witnessed {
    fn is_quiet(&self) -> bool {
        *self == Self::default()
    }
}

/// Companions match the world's parties: who follows whom there follows them here. A follower
/// who turns on the party becomes an enemy where it stands.
fn match_parties(
    mut commands: Commands,
    world: Option<Res<WorldState>>,
    mut roster: ResMut<PartyRoster>,
    players: Query<(Entity, &PlayerAvatar, &NetId)>,
    people: Query<Person_, With<Npc>>,
    everyone: Query<(&Person, &NetId)>,
) {
    let Some(world) = world.as_deref() else {
        return;
    };
    let sim = &world.sim;
    // Each player's party, as replicated ids, for their snapshots and HUD.
    roster.0.clear();
    for party in sim.parties() {
        let ids: Vec<(Option<Entity>, NetId)> = party
            .members
            .iter()
            .filter_map(|&m| {
                let actor = sim.world().get(m);
                match actor.player {
                    Some(player) => players
                        .iter()
                        .find(|(_, a, _)| a.0.0.as_u128() == player)
                        .map(|(e, _, id)| (Some(e), *id)),
                    None => everyone
                        .iter()
                        .find(|(p, _)| p.0 == actor.id)
                        .map(|(_, id)| (None, *id)),
                }
            })
            .collect();
        for &(entity, me) in &ids {
            if let Some(entity) = entity {
                let others = ids.iter().map(|(_, id)| *id).filter(|id| *id != me);
                roster.0.push((entity, others.collect()));
            }
        }
    }
    let betrayed: Vec<ActorId> = world
        .recent()
        .iter()
        .filter_map(|e| match e.what {
            Happening::Betrayed { actor, .. } => Some(actor),
            _ => None,
        })
        .collect();
    for (entity, person, map, body, state, companion) in &people {
        let Some(actor) = sim.world().actor(&person.0) else {
            continue;
        };
        let a = sim.world().get(actor);
        // Dead in the world (a world carried on from a save, say): not standing here.
        if !a.alive && !state.fighter.is_dead() {
            commands.entity(entity).despawn();
            continue;
        }
        if betrayed.contains(&actor) || a.outlaw {
            turn(&mut commands, entity, *map, body, state);
            continue;
        }
        let leader = sim.party_of(actor).and_then(|p| {
            let player = sim.world().get(p.leader).player?;
            players
                .iter()
                .find(|(_, a, _)| a.0.0.as_u128() == player)
                .map(|(e, ..)| e)
        });
        match (leader, companion) {
            (Some(leader), Some(c)) if c.leader == leader => {}
            (Some(leader), _) => {
                commands.entity(entity).insert(Companion {
                    leader,
                    path: Vec::new(),
                    repath_in: 0,
                });
            }
            (None, Some(_)) => {
                commands.entity(entity).remove::<Companion>();
            }
            (None, None) => {}
        }
    }
}

/// A person standing in a scene, as the party matcher sees them.
type Person_ = (
    Entity,
    &'static Person,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    Option<&'static Companion>,
);

/// Each player character's party: the other members' replicated ids.
#[derive(Resource, Default)]
pub struct PartyRoster(Vec<(Entity, Vec<NetId>)>);

/// The others in `player`'s party.
pub fn party_members(roster: &PartyRoster, player: Entity) -> Vec<NetId> {
    roster
        .0
        .iter()
        .find(|(e, _)| *e == player)
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default()
}

/// The traitor stops being a friendly NPC and fights where it stands.
fn turn(
    commands: &mut Commands,
    entity: Entity,
    map: MapId,
    body: &BodyState,
    state: &CharacterState,
) {
    commands
        .entity(entity)
        .remove::<(Npc, Companion)>()
        .insert((
            Hostile,
            crate::maps::StaysInMap,
            Post {
                map,
                home: body.0.position,
                facing: state.facing,
                respawn: u32::MAX,
            },
            Brain::new(AiDef {
                sight: 200.0,
                leash: 400.0,
                attack_range: FOLLOWER_REACH,
                speed: 1.2,
                back_off: 30,
                respawn: u32::MAX,
            }),
            Speech {
                line: "party.betrayed".into(),
                to: None,
                ticks_left: SPEECH_TICKS,
            },
        ));
}
