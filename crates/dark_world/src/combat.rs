//! Combat on the host (docs/PLAN.md §13): enemies placed by scenes, hits landing, the dead
//! falling and coming back.
//!
//! Fighting itself (attacks, combos, dodges) is in each character's controller and predicted by
//! clients; this is what only the host decides: whose hitbox touched whom, and what happens to
//! the dead. Players and friendly NPCs are one side, [`Hostile`] characters the other. NPCs
//! cannot be hurt yet (they have no way back from death).

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_combat::{Action, CombatDef, HitOutcome};
use dark_core::{App, FixedUpdate, Plugin, SimTick};
use dark_physics::Body;
use dark_sprite::Facing;
use glam::Vec2;

use crate::ai::Brain;
use crate::characters::{
    CHARACTER_RADIUS, CharacterSheets, CharacterState, ControlInput, NetId, PlayerAvatar,
    enemy_look,
};
use crate::talk::Npc;
use crate::{BodyState, MapId, Maps, NextNetId, PreviousBody, body_bundle};

/// A dead player gets up again at the start after this many ticks (3 s).
pub const PLAYER_RESPAWN_TICKS: u16 = 180;
/// A dead enemy lies this long before it is gone (and later returns to its post).
pub const CORPSE_TICKS: u16 = 150;
/// Attacker and victim must be this close in height for a hit.
const HIT_HEIGHT: f32 = 12.0;

/// On the enemies' side.
#[derive(Component, Clone, Copy, Debug)]
pub struct Hostile;

/// Where an enemy stands guard, and how long it stays gone after dying.
#[derive(Component, Clone, Copy, Debug)]
pub struct Post {
    pub map: MapId,
    pub home: Vec2,
    pub facing: Facing,
    pub respawn: u32,
}

/// A dead enemy, gone from the world until `until` (a simulation tick); not replicated.
#[derive(Component, Clone, Copy, Debug)]
pub struct Dormant {
    pub until: u64,
}

/// Whom the current swing (by [`dark_combat::Fighter::swing`]) has hit: each at most once.
#[derive(Component, Clone, Debug, Default)]
pub(crate) struct SwingHits {
    swing: u16,
    hit: Vec<Entity>,
}

/// Hits resolve in this set, after everyone has moved.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fight;

/// Spawns every scene's enemies and runs hits, deaths and respawns. Needs
/// [`crate::MapsPlugin`] and [`crate::CharactersPlugin`] first.
pub struct CombatPlugin(pub CombatDef);

impl Plugin for CombatPlugin {
    fn build(self, app: &mut App) {
        let maps = app.world.resource::<Maps>();
        let sheets = app.world.resource::<CharacterSheets>();
        let mut spawns = Vec::new();
        for (i, map) in maps.maps.iter().enumerate() {
            for placed in &map.def.enemies {
                let (look, kind) = enemy_look(&self.0, &placed.kind)
                    .and_then(|def| sheets.id_of(&def))
                    .zip(self.0.enemies.get(&placed.kind))
                    .expect("CharacterSheets is built from these same maps and definitions");
                let home = Vec2::from(placed.position);
                spawns.push((
                    body_bundle(MapId(i as u16), home, CHARACTER_RADIUS),
                    CharacterState::new(sheets, look, placed.facing),
                    Post {
                        map: MapId(i as u16),
                        home,
                        facing: placed.facing,
                        respawn: kind.ai.respawn,
                    },
                    Brain::new(kind.ai.clone()),
                ));
            }
        }
        app.world.init_resource::<NextNetId>();
        for (body, state, post, brain) in spawns {
            let id = app.world.resource_mut::<NextNetId>().allocate();
            app.world.spawn((
                body,
                state,
                post,
                brain,
                Hostile,
                crate::maps::StaysInMap,
                ControlInput::default(),
                SwingHits::default(),
                id,
            ));
        }
        app.add_systems(
            FixedUpdate,
            (crate::ai::think.before(crate::characters::control_characters)).in_set(crate::Control),
        )
        .add_systems(
            FixedUpdate,
            (resolve_hits, fall_and_rise, return_dormant)
                .chain()
                .in_set(Fight),
        );
    }
}

/// One fighter as the hit check sees it.
struct Contender {
    entity: Entity,
    map: MapId,
    at: Vec2,
    elevation: f32,
    hostile: bool,
    /// Friendly NPCs are never hit.
    untouchable: bool,
}

type HitQuery = (
    Entity,
    &'static NetId,
    &'static MapId,
    &'static BodyState,
    &'static mut CharacterState,
    Has<Hostile>,
    // Friendly NPCs are untouchable unless they fight at someone's side.
    (Has<Npc>, Has<crate::Companion>),
    Option<&'static mut SwingHits>,
);

/// Every active hitbox against every fighter on the other side, in the same map, at about the
/// same height, not already hit by this swing.
pub(crate) fn resolve_hits(
    mut commands: Commands,
    sheets: Res<CharacterSheets>,
    mut fighters: Query<HitQuery, Without<Dormant>>,
) {
    let contenders: Vec<Contender> = fighters
        .iter()
        .map(
            |(entity, _, map, body, _, hostile, (npc, companion), _)| Contender {
                entity,
                map: *map,
                at: body.0.position,
                elevation: body.0.elevation,
                hostile,
                untouchable: npc && !companion,
            },
        )
        .collect();
    let mut landed: Vec<(Entity, Entity, u32, u8, Vec2)> = Vec::new();
    for (entity, id, map, body, state, hostile, _, hits) in &fighters {
        let look = sheets.look(state.look);
        let facing = state.facing.vector();
        let Some((center, radius)) = state.fighter.hitbox(&look.moveset, body.0.position, facing)
        else {
            continue;
        };
        let Action::Attack { step, .. } = state.fighter.action else {
            continue;
        };
        let already = |victim: Entity| {
            hits.as_ref()
                .is_some_and(|h| h.swing == state.fighter.swing && h.hit.contains(&victim))
        };
        for victim in &contenders {
            let reaches = victim.at.distance(center) <= radius + CHARACTER_RADIUS;
            let level = (victim.elevation - body.0.elevation).abs() <= HIT_HEIGHT;
            if victim.entity != entity
                && victim.map == *map
                && victim.hostile != hostile
                && !victim.untouchable
                && reaches
                && level
                && !already(victim.entity)
            {
                landed.push((
                    entity,
                    victim.entity,
                    id.0,
                    step,
                    victim.at - body.0.position,
                ));
            }
        }
    }
    // Records for attackers that had none yet, gathered so two victims in one tick both count.
    let mut new_records: std::collections::HashMap<Entity, SwingHits> = Default::default();
    for (attacker, victim, attacker_id, step, push) in landed {
        // Whom this swing hit, whether or not the victim could be hurt (a dodge is still a miss).
        let Ok((.., state, _, _, hits)) = fighters.get_mut(attacker) else {
            continue;
        };
        let swing = state.fighter.swing;
        let attack = sheets.look(state.look).moveset.combo[usize::from(step)].clone();
        match hits {
            Some(mut hits) => {
                if hits.swing != swing {
                    *hits = SwingHits {
                        swing,
                        hit: Vec::new(),
                    };
                }
                hits.hit.push(victim);
            }
            None => {
                let record = new_records.entry(attacker).or_insert(SwingHits {
                    swing,
                    hit: Vec::new(),
                });
                record.hit.push(victim);
            }
        }
        let Ok((.., mut state, _, _, _)) = fighters.get_mut(victim) else {
            continue;
        };
        let moveset = &sheets.look(state.look).moveset;
        let outcome = state.fighter.take_hit(moveset, &attack, push, attacker_id);
        if outcome != HitOutcome::Missed {
            state.sleeping = false;
        }
    }
    for (attacker, record) in new_records {
        commands.entity(attacker).insert(record);
    }
}

/// Anyone who may be lying dead.
type Fallen = (
    Entity,
    &'static mut CharacterState,
    &'static mut BodyState,
    &'static mut PreviousBody,
    &'static mut MapId,
    Has<PlayerAvatar>,
    Option<&'static Post>,
    Option<&'static mut crate::Life>,
    Has<crate::Asleep>,
    Has<crate::party::Mortal>,
);

/// A dormant enemy and what it needs to return.
type Sleeper = (
    Entity,
    &'static Dormant,
    &'static Post,
    &'static mut CharacterState,
    &'static mut BodyState,
    &'static mut PreviousBody,
    &'static mut MapId,
    &'static mut Brain,
);

/// The dead lie still; players get up at the start after a while, enemies go dormant.
fn fall_and_rise(
    mut commands: Commands,
    tick: Res<SimTick>,
    maps: Res<Maps>,
    sheets: Res<CharacterSheets>,
    mut dead: Query<Fallen, Without<Dormant>>,
) {
    for (entity, mut state, mut body, mut previous, mut map, player, post, life, away, mortal) in
        &mut dead
    {
        let Action::Dead { tick: lying } = state.fighter.action else {
            continue;
        };
        // A person of the world who died is gone once their body has lain its time.
        if mortal {
            if lying >= CORPSE_TICKS {
                commands.entity(entity).despawn();
            }
            continue;
        }
        // A player who is away lies where they fell and gets up when they are back.
        if player && !away && lying >= PLAYER_RESPAWN_TICKS {
            let spawn = maps.maps[0]
                .def
                .player
                .as_ref()
                .map_or(Vec2::ZERO, |p| Vec2::from(p.spawn));
            let moveset = &sheets.look(state.look).moveset;
            state.fighter.revive(moveset);
            place(&mut body, &mut previous, spawn, &maps, MapId(0));
            *map = MapId(0);
            // Found and looked after: whatever killed them, the body starts over.
            if let Some(mut life) = life {
                life.body = dark_life::Body::new();
                state.impaired = crate::Impairment::default();
            }
            tracing::info!("a player's character gets up again at the start");
        } else if let Some(post) = post
            && lying >= CORPSE_TICKS
        {
            commands.entity(entity).insert(Dormant {
                until: tick.0 + u64::from(post.respawn),
            });
        }
    }
}

/// Dormant enemies return to their posts, whole, when their time comes.
fn return_dormant(
    mut commands: Commands,
    tick: Res<SimTick>,
    maps: Res<Maps>,
    sheets: Res<CharacterSheets>,
    mut dormant: Query<Sleeper>,
) {
    for (entity, dormant, post, mut state, mut body, mut previous, mut map, mut brain) in
        &mut dormant
    {
        if tick.0 < dormant.until {
            continue;
        }
        let moveset = &sheets.look(state.look).moveset;
        state.fighter.revive(moveset);
        state.facing = post.facing;
        place(&mut body, &mut previous, post.home, &maps, post.map);
        *map = post.map;
        *brain = Brain::new(brain.def().clone());
        commands.entity(entity).remove::<Dormant>();
    }
}

/// Stands the body at `at` in `map`, on the ground there.
pub(crate) fn place(
    body: &mut BodyState,
    previous: &mut PreviousBody,
    at: Vec2,
    maps: &Maps,
    map: MapId,
) {
    let mut placed = Body::new(at, body.0.radius);
    let ground = maps.get(map).collision.ground_under(at, placed.radius);
    placed.elevation = if ground.is_finite() { ground } else { 0.0 };
    body.0 = placed;
    *previous = PreviousBody {
        position: at,
        elevation: placed.elevation,
    };
}
