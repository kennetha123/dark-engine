//! Enemy minds: a small state machine that writes the same [`TickInput`] a player would, so
//! enemies move and fight through the one character controller (docs/PLAN.md §2 rule 4).
//!
//! Guard the post; chase a player who comes within sight (straight at them when the way is
//! clear, else along an A* path); attack in range; back off a moment after each attack; walk
//! home when the target is gone or the chase has gone too far from the post.

use bevy_ecs::prelude::*;
use dark_combat::AiDef;
use dark_core::SimTick;
use glam::Vec2;

use crate::characters::{CHARACTER_RADIUS, CharacterState, ControlInput, PlayerAvatar, TickInput};
use crate::combat::{Dormant, Hostile, Post};
use crate::nav;
use crate::{BodyState, MapId, Maps};

/// Ticks between path searches while chasing.
const REPATH_TICKS: u16 = 20;
/// Close enough to a path point or home to count as there.
const ARRIVED: f32 = 4.0;
/// Enemies and targets further apart in height than this cannot fight.
const REACH_HEIGHT: f32 = 8.0;
/// Failed searches for the way home before an enemy gives up and returns to its post unseen.
const LOST_SEARCHES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mind {
    Guard,
    Chase,
    /// Stepping back after an attack for this many more ticks.
    BackOff(u16),
    Return,
}

#[derive(Component, Clone, Debug)]
pub struct Brain {
    def: AiDef,
    mind: Mind,
    target: Option<Entity>,
    path: Vec<Vec2>,
    /// Ticks until the next path search: searches are dear, and one that failed will fail again
    /// for a while.
    repath_in: u16,
    /// Searches for the way home that found none.
    lost: u8,
}

impl Brain {
    pub fn new(def: AiDef) -> Self {
        Self {
            def,
            mind: Mind::Guard,
            target: None,
            path: Vec::new(),
            repath_in: 0,
            lost: 0,
        }
    }

    pub fn def(&self) -> &AiDef {
        &self.def
    }
}

/// A player an enemy may go after.
type Target = (
    Entity,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    Has<crate::Asleep>,
);

/// Players and those who fight at their side.
type Targetable = Or<(With<PlayerAvatar>, With<crate::Companion>)>;

type Mindful = (
    Entity,
    &'static mut Brain,
    &'static Post,
    &'static MapId,
    &'static BodyState,
    &'static CharacterState,
    &'static mut ControlInput,
);

pub(crate) fn think(
    mut commands: Commands,
    tick: Res<SimTick>,
    maps: Res<Maps>,
    players: Query<Target, Targetable>,
    mut enemies: Query<Mindful, (With<Hostile>, Without<Dormant>)>,
) {
    for (entity, mut brain, post, map, body, state, mut control) in &mut enemies {
        let brain = &mut *brain;
        control.input = TickInput::default();
        control.hold = false;
        if !state.fighter.is_free() {
            continue;
        }
        let me = body.0.position;
        let world = &maps.get(*map).collision;
        let far_from_home = me.distance(post.home) > brain.def.leash;
        // The target: kept while alive, here and not left behind; else the nearest player seen.
        // Sleepers at an inn are left be.
        let alive_here = |e: Entity| {
            players.get(e).ok().filter(|(_, m, b, s, offline)| {
                *m == map
                    && !s.fighter.is_dead()
                    && (b.0.elevation - body.0.elevation).abs() <= REACH_HEIGHT
                    && !crate::life::safe_at_inn(
                        &maps,
                        *map,
                        b.0.position,
                        s.sleeping || s.impaired.out || *offline,
                    )
            })
        };
        brain.target = brain
            .target
            .filter(|&t| alive_here(t).is_some() && !far_from_home)
            .or_else(|| {
                players
                    .iter()
                    .filter(|&(e, ..)| alive_here(e).is_some())
                    .map(|(e, _, b, ..)| (e, b.0.position.distance(me)))
                    .filter(|&(_, d)| d <= brain.def.sight)
                    .filter(|_| me.distance(post.home) <= brain.def.leash * 0.75)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(e, _)| e)
            });
        let target = brain
            .target
            .and_then(|t| players.get(t).ok())
            .map(|(_, _, b, ..)| b.0.position);

        let was = std::mem::discriminant(&brain.mind);
        brain.mind = match (brain.mind, target) {
            (Mind::BackOff(0), Some(_)) => Mind::Chase,
            (Mind::BackOff(n), Some(_)) => Mind::BackOff(n - 1),
            (_, Some(_)) if far_from_home => Mind::Return,
            (Mind::Guard | Mind::Return, Some(_)) => Mind::Chase,
            (Mind::Chase | Mind::BackOff(_), None) => Mind::Return,
            (mind, _) => mind,
        };
        // A new state of mind wants its own way: forget the old path.
        if std::mem::discriminant(&brain.mind) != was {
            brain.path.clear();
            brain.repath_in = 0;
        }
        let speed = brain.def.speed.clamp(0.0, 2.0);
        let mut walk = |toward: Vec2, input: &mut TickInput| {
            let d = toward - me;
            if d.length() > f32::EPSILON {
                input.movement = d.normalize() * speed.min(1.0);
                input.run = speed > 1.0;
            }
        };
        let input = &mut control.input;
        match (brain.mind, target) {
            (Mind::Chase, Some(target)) => {
                let d = target - me;
                if d.length() <= brain.def.attack_range {
                    // Turn to the target and swing; the attack's startup is its tell.
                    input.movement = d.normalize_or_zero() * 0.05;
                    input.attack = true;
                    brain.mind = Mind::BackOff(brain.def.back_off);
                    brain.path.clear();
                } else {
                    let r = CHARACTER_RADIUS;
                    let e = body.0.elevation;
                    let params = &maps.params;
                    if nav::clear_line(world, me, target, r, e, params) {
                        brain.path.clear();
                        walk(target, input);
                    } else {
                        if brain.repath_in == 0 {
                            brain.path =
                                nav::find_path(world, me, target, r, e, params).unwrap_or_default();
                            brain.repath_in = REPATH_TICKS;
                        }
                        brain.repath_in -= 1;
                        follow(&mut brain.path, me, input, &mut walk);
                    }
                }
            }
            (Mind::BackOff(_), Some(target)) => {
                let away = me + (me - target).normalize_or_zero() * 16.0;
                walk(away, input);
                input.movement *= 0.6;
            }
            (Mind::Return, _) => {
                if me.distance(post.home) <= ARRIVED {
                    brain.mind = Mind::Guard;
                    brain.path.clear();
                } else {
                    let r = CHARACTER_RADIUS;
                    let e = body.0.elevation;
                    brain.repath_in = brain.repath_in.saturating_sub(1);
                    if brain.path.is_empty() && brain.repath_in == 0 {
                        brain.repath_in = REPATH_TICKS;
                        let found = if nav::clear_line(world, me, post.home, r, e, &maps.params) {
                            Some(vec![post.home])
                        } else {
                            nav::find_path(world, me, post.home, r, e, &maps.params)
                        };
                        match found {
                            Some(path) => {
                                brain.path = path;
                                brain.lost = 0;
                            }
                            // Knocked off a ledge or walled out: after a few tries it slips
                            // away and is back at its post a moment later.
                            None => {
                                brain.lost += 1;
                                if brain.lost >= LOST_SEARCHES {
                                    commands
                                        .entity(entity)
                                        .insert(Dormant { until: tick.0 + 60 });
                                    *brain = Brain::new(brain.def.clone());
                                }
                            }
                        }
                    }
                    follow(&mut brain.path, me, input, &mut walk);
                }
            }
            // Guarding: stand still, facing the way the post faces.
            _ => {
                if state.facing != post.facing {
                    input.movement = post.facing.vector() * 0.05;
                }
            }
        }
    }
}

/// Walks to the path's next point, dropping points already reached.
fn follow(
    path: &mut Vec<Vec2>,
    me: Vec2,
    input: &mut TickInput,
    walk: &mut impl FnMut(Vec2, &mut TickInput),
) {
    while path.first().is_some_and(|p| p.distance(me) <= ARRIVED) {
        path.remove(0);
    }
    if let Some(&next) = path.first() {
        walk(next, input);
    }
}
