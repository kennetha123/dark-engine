//! Bodies in play (docs/PLAN.md §14): [`dark_life`] run for every player's character on the host.
//!
//! Each game minute every body steps with its surroundings: the region's air at that hour,
//! shelter (an inn's area, a tent close by), a fire nearby, the clothes worn, asleep or not. When
//! the night passes because everyone slept, bodies sleep through every skipped minute. What a
//! body's state does to its character (slower, staggering, out cold) goes into the replicated
//! [`CharacterState::impaired`], so clients predict it; hunger and cold cost health. Using items,
//! relieving yourself and packing up a tent are the host's to decide, like talking.

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_core::{App, FixedUpdate, Plugin};
use dark_life::{
    Body, Condition, Inventory, LifeDef, Need, Shelter, Status, Structure, Surroundings, Used,
    use_item,
};
use dark_net::PlayerId;
use dark_time::GameClock;
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::characters::{
    Asleep, CharacterState, Control, ControlInput, Impairment, NetId, PlayerAvatar,
    control_characters,
};
use crate::talk::{Npc, SPEECH_TICKS, Speech, talk_target};
use crate::{BodyState, Dormant, MapId, Maps, NextNetId, WorldClock, WorldStep};

/// Standing (or lying) this close to a tent is being in it.
pub const TENT_REACH: f32 = 16.0;
/// A burning campfire warms everyone this close.
pub const FIRE_REACH: f32 = 48.0;
/// Structures reach only those about level with them (not someone below a cliff).
const REACH_HEIGHT: f32 = 8.0;
/// Things are set down this far ahead of the one placing them.
const PLACE_AHEAD: f32 = 14.0;
/// A structure needs this much clear ground, and this far from any other.
const PLACE_RADIUS: f32 = 6.0;
const PLACE_APART: f32 = 14.0;
/// Relieving yourself needs something to relieve (‰).
const RELIEVE_FROM: u32 = 200;
const DAY_MINUTES: u64 = 24 * 60;

/// The night passing when everyone sleeps runs in this set; bodies step after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NightPass;

/// Set when everyone slept and the clock jumped to morning: the minute the night began. Bodies
/// sleep through every minute after it.
#[derive(Resource, Default)]
pub(crate) struct NightPassed(pub(crate) Option<u64>);

/// The project's life definitions, and the last game minute bodies have lived.
#[derive(Resource)]
pub struct LifeRules {
    pub def: LifeDef,
    minute: Option<u64>,
}

/// A body and what it carries. Players' characters have one.
#[derive(Component, Clone, Debug)]
pub struct Life {
    pub body: Body,
    pub inventory: Inventory,
    /// Where it is (updated every tick), for its minutes and the HUD.
    around: Surroundings,
    /// What others could have seen it do since they last looked (see [`crate::PartyPlugin`]).
    pub(crate) witnessed: crate::party::Witnessed,
}

impl Life {
    pub fn new(def: &LifeDef) -> Self {
        Self {
            body: Body::new(),
            inventory: def.new_inventory(),
            around: Surroundings::default(),
            witnessed: Default::default(),
        }
    }

    /// What the owner's HUD shows.
    pub fn view(&self, def: &LifeDef) -> LifeView {
        let rates = &def.rates;
        LifeView {
            needs: Need::ALL.map(|n| self.body.need(n) as u16),
            alcohol: self.body.alcohol().min(u32::from(u16::MAX)) as u16,
            core: self.body.core,
            air: self.around.air,
            felt: self.body.felt(&self.around, rates),
            shelter: self.around.shelter,
            fire: self.around.fire,
            statuses: self.body.condition(rates).statuses,
            slots: self.inventory.slots().to_vec(),
            worn: self.inventory.worn.clone(),
        }
    }
}

/// A body as its owner sees it; sent only to them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LifeView {
    /// Each need in ‰, in [`Need::ALL`] order.
    pub needs: [u16; 6],
    /// Drink in the blood, ‰ of a drink.
    pub alcohol: u16,
    /// Core, air and felt temperature, hundredths of a degree.
    pub core: i32,
    pub air: i32,
    pub felt: i32,
    pub shelter: Shelter,
    pub fire: bool,
    pub statuses: Vec<Status>,
    /// The hotbar: item ids and counts.
    pub slots: Vec<(String, u16)>,
    pub worn: Option<String>,
}

/// Something set down in the world: a tent, a campfire (its minutes left burning).
#[derive(Component, Clone, Debug)]
pub struct Placed {
    pub structure: Structure,
    pub at: Vec2,
    /// Height of the ground it stands on.
    pub elevation: f32,
    /// The item it came from, given back when packed up.
    pub item: String,
    /// Who set it down; only they can pack it up.
    pub owner: Option<PlayerId>,
}

/// A structure as clients see it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructureSnapshot {
    pub id: NetId,
    pub structure: Structure,
    pub at: Vec2,
}

/// The absolute game minute on `clock`.
pub(crate) fn minute_of(clock: &GameClock) -> u64 {
    u64::from(clock.day()) * DAY_MINUTES + (clock.hour() * 60.0) as u64
}

/// What a body's condition does to its character.
pub(crate) fn impairment(condition: &Condition) -> Impairment {
    Impairment {
        speed: condition.speed,
        wobble: condition.wobble,
        out: condition.passed_out,
    }
}

/// Sleeping in an inn: enemies leave you be.
pub(crate) fn safe_at_inn(maps: &Maps, map: MapId, at: Vec2, asleep: bool) -> bool {
    asleep && maps.get(map).def.inns.iter().any(|i| i.contains(at.into()))
}

/// The inn bed to send a character at `at` in `map` to: the nearest in its map, else the first
/// inn of any map.
pub(crate) fn nearest_inn(maps: &Maps, map: MapId, at: Vec2) -> Option<(MapId, Vec2)> {
    let here = maps
        .get(map)
        .def
        .inns
        .iter()
        .map(|i| Vec2::from(i.bed))
        .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)));
    here.map(|bed| (map, bed)).or_else(|| {
        maps.maps.iter().enumerate().find_map(|(i, m)| {
            m.def
                .inns
                .first()
                .map(|inn| (MapId(i as u16), Vec2::from(inn.bed)))
        })
    })
}

/// Gives players' characters bodies and runs them (any character with a [`Life`] lives; only
/// players get one so far). Needs [`crate::MapsPlugin`] and
/// [`crate::CharactersPlugin`] first.
pub struct LifePlugin(pub LifeDef);

impl Plugin for LifePlugin {
    fn build(self, app: &mut App) {
        app.world.init_resource::<NextNetId>();
        app.world.init_resource::<NightPassed>();
        app.insert_resource(LifeRules {
            def: self.0,
            minute: None,
        })
        .add_systems(
            FixedUpdate,
            (give_life, act)
                .chain()
                .after(control_characters)
                .in_set(Control),
        )
        .add_systems(FixedUpdate, live.after(NightPass).in_set(WorldStep));
    }
}

fn give_life(
    mut commands: Commands,
    rules: Res<LifeRules>,
    players: Query<Entity, (With<PlayerAvatar>, Without<Life>)>,
) {
    for entity in &players {
        commands.entity(entity).insert(Life::new(&rules.def));
    }
}

/// A character acting on their body: using what they carry, relieving themselves, packing up a
/// tent (a player's own).
type Actor = (
    Option<&'static PlayerAvatar>,
    &'static MapId,
    &'static BodyState,
    &'static ControlInput,
    &'static mut CharacterState,
    &'static mut Life,
);

fn act(
    mut commands: Commands,
    rules: Res<LifeRules>,
    mut next_id: ResMut<NextNetId>,
    maps: Res<Maps>,
    mut actors: Query<Actor, (Without<Asleep>, Without<Dormant>)>,
    npcs: Query<(&MapId, &BodyState), With<Npc>>,
    placed: Query<(Entity, &MapId, &Placed)>,
) {
    for (avatar, map, body, control, mut state, mut life) in &mut actors {
        let input = control.input;
        if control.hold || !state.fighter.is_free() || state.impaired.out || state.sleeping {
            continue;
        }
        let owner = avatar.map(|a| a.0);
        let life = &mut *life;
        let feet = body.0.position;
        let ahead = feet + state.facing.vector() * PLACE_AHEAD;
        let world = &maps.get(*map).collision;
        let ground = world.ground_under(ahead, PLACE_RADIUS);
        // Something to set down needs clear, open ground ahead, level with the feet (not over a
        // ledge), away from other structures.
        let blocked = || {
            !body.0.grounded
                || (ground - body.0.elevation).abs() > 0.5
                || world
                    .colliders
                    .iter()
                    .any(|c| c.height > 0.0 && crate::maps::overlaps(c, ahead, PLACE_RADIUS))
                || maps.get(*map).exits.iter().any(|e| e.contains(ahead))
                || placed
                    .iter()
                    .any(|(_, m, p)| m == map && p.at.distance(ahead) < PLACE_APART)
        };
        let places = |slot: usize| {
            life.inventory
                .slots()
                .get(slot)
                .and_then(|(id, _)| rules.def.items.get(id))
                .is_some_and(|d| matches!(d.use_, dark_life::ItemUse::Place(_)))
        };
        if input.item > 0 && !(places(usize::from(input.item - 1)) && blocked()) {
            let slot = usize::from(input.item - 1);
            match use_item(&mut life.body, &mut life.inventory, &rules.def.items, slot) {
                Used::Nothing => {}
                Used::Place(item, structure) => {
                    commands.spawn((
                        Placed {
                            structure,
                            at: ahead,
                            elevation: ground,
                            item,
                            owner,
                        },
                        *map,
                        next_id.allocate(),
                    ));
                }
                used => tracing::debug!("{owner:?}: {used:?}"),
            }
            // Drink shows at once, not a minute later.
            state.impaired = impairment(&life.body.condition(&rules.def.rates));
        }
        let pressing = |n: Need| life.body.need(n) >= RELIEVE_FROM;
        if input.relieve && (pressing(Need::Bladder) || pressing(Need::Bowel)) {
            life.body.relieve();
        }
        // Interact talks when an NPC is in reach; otherwise it packs up your tent.
        if input.interact {
            let npc_near = talk_target(
                (feet, body.0.elevation),
                npcs.iter()
                    .filter(|(m, _)| *m == map)
                    .map(|(_, b)| ((), b.0.position, b.0.elevation)),
            )
            .is_some();
            let tent = placed
                .iter()
                .filter(|(_, m, p)| {
                    *m == map
                        && p.structure == Structure::Tent
                        && (p.owner.is_none() || p.owner == owner)
                        && p.at.distance(feet) <= TENT_REACH
                })
                .min_by(|a, b| a.2.at.distance(feet).total_cmp(&b.2.at.distance(feet)));
            if let (false, Some((entity, _, p))) = (npc_near, tent) {
                life.inventory.add(&p.item, 1);
                commands.entity(entity).despawn();
            }
        }
    }
}

/// A body stepping through the minutes.
type Living = (
    Entity,
    &'static MapId,
    &'static BodyState,
    &'static mut CharacterState,
    &'static mut Life,
    Has<Asleep>,
);

pub(crate) fn live(
    mut commands: Commands,
    clock: Res<WorldClock>,
    maps: Res<Maps>,
    mut rules: ResMut<LifeRules>,
    mut night: ResMut<NightPassed>,
    mut living: Query<Living, Without<Dormant>>,
    mut structures: Query<(Entity, &MapId, &mut Placed)>,
) {
    let now = minute_of(&clock.0);
    let slept_from = night.0.take();
    let rules = &mut *rules;
    // Where everyone is, every tick, so the HUD is right from the first frame.
    let spots: Vec<Spot> = structures.iter().map(|(_, m, p)| Spot::of(*m, p)).collect();
    for (_, map, body, state, mut life, offline) in &mut living {
        let insulation = life.inventory.insulation(&rules.def.items);
        life.around = surroundings(&rules.def, &maps, &spots, *map, &body.0, now);
        life.around.insulation = insulation;
        life.around.asleep = state.sleeping || offline;
    }
    let last = *rules.minute.get_or_insert(now);
    if now <= last {
        return;
    }
    // A day at most: longer gaps (a loaded world) are not lived through minute by minute.
    let first = last.max(now.saturating_sub(DAY_MINUTES)) + 1;
    for minute in first..=now {
        let everyone_asleep = slept_from.is_some_and(|from| minute > from);
        // Fires burn down minute by minute, through a night that passes too.
        let spots: Vec<Spot> = structures.iter().map(|(_, m, p)| Spot::of(*m, p)).collect();
        for (entity, map, body, mut state, mut life, offline) in &mut living {
            // A player who is away sleeps until they return, their body kept as it was.
            if offline || state.fighter.is_dead() {
                continue;
            }
            let mut around = life.around;
            around.air = air(&rules.def, &maps, *map, minute);
            around.fire = surroundings(&rules.def, &maps, &spots, *map, &body.0, minute).fire;
            around.asleep |= everyone_asleep;
            let happened = life.body.minute(&around, &rules.def.rates);
            if state.fighter.suffer(happened.damage) {
                tracing::info!("a character has died of cold, hunger or thirst");
            }
            // Said aloud, over whatever the character was saying.
            if !happened.accidents.is_empty() {
                commands.entity(entity).insert(Speech {
                    line: "life.accident".into(),
                    to: None,
                    ticks_left: SPEECH_TICKS,
                });
            }
            let condition = life.body.condition(&rules.def.rates);
            life.witnessed.accidents += happened.accidents.len() as i32;
            let shameful = [
                Status::Drunk,
                Status::Wasted,
                Status::Soiled,
                Status::Filthy,
            ];
            // Only what others could see: not the minutes slept through.
            if !around.asleep && condition.statuses.iter().any(|s| shameful.contains(s)) {
                life.witnessed.shameful_minutes += 1;
            }
            state.impaired = impairment(&condition);
        }
        for (entity, _, mut placed) in &mut structures {
            if let Structure::Campfire { minutes } = &mut placed.structure
                && *minutes > 0
            {
                *minutes -= 1;
                if *minutes == 0 {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
    rules.minute = Some(now);
}

/// A structure, as the surroundings see it.
struct Spot {
    map: MapId,
    at: Vec2,
    elevation: f32,
    structure: Structure,
}

impl Spot {
    fn of(map: MapId, placed: &Placed) -> Self {
        Self {
            map,
            at: placed.at,
            elevation: placed.elevation,
            structure: placed.structure,
        }
    }
}

/// The region's air in `map` at absolute `minute`.
fn air(def: &LifeDef, maps: &Maps, map: MapId, minute: u64) -> i32 {
    let region = maps.get(map).def.region.as_deref();
    def.air(region, (minute % DAY_MINUTES) as u32)
}

/// What is around `body` in `map`: the air, an inn or a tent over it, a fire nearby (at about
/// its height). Clothes and sleep are the caller's.
fn surroundings(
    def: &LifeDef,
    maps: &Maps,
    spots: &[Spot],
    map: MapId,
    body: &dark_physics::Body,
    minute: u64,
) -> Surroundings {
    let at = body.position;
    let near = |want: fn(&Structure) -> bool, reach: f32| {
        spots.iter().any(|s| {
            s.map == map
                && want(&s.structure)
                && s.at.distance(at) <= reach
                && (s.elevation - body.elevation).abs() <= REACH_HEIGHT
        })
    };
    let shelter = if maps.get(map).def.inns.iter().any(|i| i.contains(at.into())) {
        Shelter::Inn
    } else if near(|s| *s == Structure::Tent, TENT_REACH) {
        Shelter::Tent
    } else {
        Shelter::Open
    };
    Surroundings {
        air: air(def, maps, map, minute),
        shelter,
        fire: near(
            |s| matches!(s, Structure::Campfire { minutes } if *minutes > 0),
            FIRE_REACH,
        ),
        insulation: 0,
        asleep: false,
    }
}

/// The body of `player`'s character, as their HUD shows it.
pub fn life_of(world: &mut World, player: PlayerId) -> Option<LifeView> {
    let mut query = world.query::<(&PlayerAvatar, &Life)>();
    let rules = world.get_resource::<LifeRules>()?;
    query
        .iter(world)
        .find(|(a, _)| a.0 == player)
        .map(|(_, life)| life.view(&rules.def))
}

/// Structures set down in `map`.
pub fn structures_in(world: &mut World, map: MapId) -> Vec<StructureSnapshot> {
    world
        .query::<(&NetId, &MapId, &Placed)>()
        .iter(world)
        .filter(|(_, m, _)| **m == map)
        .map(|(id, _, p)| StructureSnapshot {
            id: *id,
            structure: p.structure,
            at: p.at,
        })
        .collect()
}
