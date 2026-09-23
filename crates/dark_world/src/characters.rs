//! Characters: players, NPCs and enemies share this model (docs/PLAN.md §2 rule 4); only where
//! their input comes from differs.
//!
//! [`control`] is a pure function of state and input so the client can predict its own
//! character with exactly the code the host runs, fighting included ([`dark_combat`]).

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, SystemSet};
use dark_assets::{AssetError, LookDef, Project};
use dark_combat::{Action, CombatDef, Fighter, Moveset, Started, Wants};
use dark_core::{App, FixedUpdate, Plugin};
use dark_net::PlayerId;
use dark_sprite::{AnimationPlayer, Facing, SpriteSheet};
use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::{BodyState, Maps, MoveIntent};

/// Walking speed in pixels per second.
pub const WALK_SPEED: f32 = 80.0;
/// Running speed (run held) in pixels per second.
pub const RUN_SPEED: f32 = 150.0;
/// Footprint radius; small against 16 px tiles so gaps of one tile are passable.
pub const CHARACTER_RADIUS: f32 = 5.0;

/// A controller's wishes for one tick. `movement` and `run` are held; the rest are presses,
/// true on the tick they happen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TickInput {
    pub movement: Vec2,
    pub run: bool,
    pub jump: bool,
    pub attack: bool,
    /// Roll out of the way (untouchable for a moment).
    pub dodge: bool,
    /// Talk to (later: use) whatever is in reach.
    pub interact: bool,
    /// Lie down to sleep, or get up.
    pub sleep: bool,
    /// Go to the toilet (or behind a bush).
    pub relieve: bool,
    /// Ask whoever is in reach to join you (or send your follower away, or leave your party).
    pub recruit: bool,
    /// Use what is in hotbar slot `item` (1 to 8); 0 uses nothing.
    pub item: u8,
    /// Lay down one of what is in hotbar slot `drop` (1 to 8); 0 drops nothing.
    pub drop: u8,
    /// In a conversation, answer with the choice whose index (as the snapshot gives it) is
    /// `choice - 1`; 0 answers nothing.
    pub choice: u8,
}

impl TickInput {
    /// Only finite, at most unit-length movement: input may come from the network.
    pub fn sanitized(self) -> Self {
        let movement = if self.movement.is_finite() {
            self.movement.clamp_length_max(1.0)
        } else {
            Vec2::ZERO
        };
        Self { movement, ..self }
    }

    /// Adds `other`'s presses to these; for latching presses between ticks.
    pub fn latch(&mut self, other: TickInput) {
        self.jump |= other.jump;
        self.attack |= other.attack;
        self.dodge |= other.dodge;
        self.interact |= other.interact;
        self.sleep |= other.sleep;
        self.relieve |= other.relieve;
        self.recruit |= other.recruit;
        if other.item != 0 {
            self.item = other.item;
        }
        if other.drop != 0 {
            self.drop = other.drop;
        }
        if other.choice != 0 {
            self.choice = other.choice;
        }
    }

    /// Takes the latched presses (as an input with only presses set), leaving them clear.
    pub fn take_presses(&mut self) -> TickInput {
        TickInput {
            movement: Vec2::ZERO,
            run: false,
            jump: std::mem::take(&mut self.jump),
            attack: std::mem::take(&mut self.attack),
            dodge: std::mem::take(&mut self.dodge),
            interact: std::mem::take(&mut self.interact),
            sleep: std::mem::take(&mut self.sleep),
            relieve: std::mem::take(&mut self.relieve),
            recruit: std::mem::take(&mut self.recruit),
            item: std::mem::take(&mut self.item),
            drop: std::mem::take(&mut self.drop),
            choice: std::mem::take(&mut self.choice),
        }
    }
}

/// Index into [`CharacterSheets::looks`]. Replicated, so host and clients must build the same
/// list: both do, from the same project files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LookId(pub u16);

/// Everything about a character besides its body that the controller changes. Replicated.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterState {
    pub look: LookId,
    pub facing: Facing,
    pub anim: AnimationPlayer,
    /// The current clip is on the look's attack sheet (attacks, dodges, hurts), not its base.
    pub on_attack_sheet: bool,
    /// Lying down asleep, by choice: when every player online sleeps, the night passes.
    pub sleeping: bool,
    pub fighter: Fighter,
    /// What the body's state does to the character (set by the host, see [`crate::LifePlugin`]).
    pub impaired: Impairment,
    /// Ticks the controller has run, wrapping: the phase of a drunken stagger.
    pub sway: u16,
}

/// What a body's state does to its character: slower, staggering, or out cold. Part of the
/// replicated state, so a client predicts its own stagger exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Impairment {
    /// Walking and running speed, percent of normal.
    pub speed: u8,
    /// Drunken stagger, 0 (steady) to 4.
    pub wobble: u8,
    /// Out cold: lies where it fell and does nothing until it comes to.
    pub out: bool,
}

impl Default for Impairment {
    fn default() -> Self {
        Self {
            speed: 100,
            wobble: 0,
            out: false,
        }
    }
}

/// A stagger ticks through this many ticks, side to side and back.
const SWAY_TICKS: u16 = 90;
/// How far each step of wobble pushes sideways, as a share of the step forward.
const SWAY_PER_WOBBLE: f32 = 0.18;

impl CharacterState {
    pub fn new(sheets: &CharacterSheets, look: LookId, facing: Facing) -> Self {
        let mut anim = AnimationPlayer::default();
        let l = sheets.look(look);
        play(&mut anim, &l.base, &["idle"], facing, true);
        Self {
            look,
            facing,
            anim,
            on_attack_sheet: false,
            sleeping: false,
            fighter: Fighter::new(&l.moveset),
            impaired: Impairment::default(),
            sway: 0,
        }
    }
}

/// One look: its sheets (frames and clips only, no pixels, so the headless host has them) and
/// how it fights.
#[derive(Clone, Debug, Default)]
pub struct Look {
    /// Idle and walk clips, optionally run and jump.
    pub base: SpriteSheet,
    /// Attack, dodge, hurt and death clips; empty for a look without them.
    pub attack: SpriteSheet,
    pub moveset: Moveset,
}

/// Every look characters in the loaded maps use: the start scene's player first, then NPCs,
/// then enemies, in map order.
#[derive(Resource, Clone, Debug)]
pub struct CharacterSheets {
    pub looks: Vec<Look>,
    defs: Vec<LookDef>,
}

impl CharacterSheets {
    /// The looks `maps` need, in [`LookId`] order, without loading anything.
    pub fn defs_in(maps: &Maps, combat: &CombatDef) -> Result<Vec<LookDef>, String> {
        let player = maps.maps[0]
            .def
            .player
            .as_ref()
            .ok_or("the start scene needs a `player`")?;
        let mut defs = vec![player.look()];
        let mut add = |look: LookDef| {
            if !defs.contains(&look) {
                defs.push(look);
            }
        };
        for npc in maps.maps.iter().flat_map(|m| &m.def.npcs) {
            add(npc.look());
        }
        for (map, enemy) in maps
            .maps
            .iter()
            .flat_map(|m| m.def.enemies.iter().map(move |e| (m, e)))
        {
            let look = enemy_look(combat, &enemy.kind)
                .ok_or_else(|| format!("{}: unknown enemy kind {}", map.name, enemy.kind))?;
            add(look);
        }
        if defs.len() > usize::from(u16::MAX) {
            return Err("more than 65535 character looks".into());
        }
        Ok(defs)
    }

    /// Builds the looks `maps` need, getting each sheet from `sheet`.
    pub fn build(
        maps: &Maps,
        combat: &CombatDef,
        project: &Project,
        mut sheet: impl FnMut(&str) -> Result<SpriteSheet, AssetError>,
    ) -> Result<Self, AssetError> {
        let invalid = |message: String| AssetError::Invalid {
            path: project.path(&maps.maps[0].name),
            message,
        };
        let defs = Self::defs_in(maps, combat).map_err(invalid)?;
        let looks = defs
            .iter()
            .map(|def| {
                let moveset = match &def.moveset {
                    Some(id) => combat
                        .movesets
                        .get(id)
                        .cloned()
                        .ok_or_else(|| invalid(format!("unknown moveset {id}")))?,
                    None => Moveset::default(),
                };
                let mut look = Look {
                    base: sheet(&def.sheet)?,
                    attack: match &def.attack {
                        Some(path) => sheet(path)?,
                        None => SpriteSheet::default(),
                    },
                    moveset: Moveset::default(),
                };
                let mut moveset = moveset;
                timed_by_animation(&mut moveset, &look)
                    .map_err(|m| invalid(format!("{}: {m}", def.sheet)))?;
                look.moveset = moveset;
                Ok(look)
            })
            .collect::<Result<_, AssetError>>()?;
        Ok(Self { looks, defs })
    }

    /// Loads the looks `maps` need. Headless-safe: sheets are frames and clips; the pixels are
    /// decoded only to slice them.
    pub fn load(project: &Project, maps: &Maps, combat: &CombatDef) -> Result<Self, AssetError> {
        Self::build(maps, combat, project, |path| {
            Ok(project.load_sheet(path)?.sheet)
        })
    }

    /// A single look, for tests and tools.
    pub fn single(look: Look) -> Self {
        Self {
            looks: vec![look],
            defs: vec![LookDef {
                sheet: String::new(),
                attack: None,
                face: None,
                name: None,
                moveset: None,
            }],
        }
    }

    /// The look with `id`, or the first one for an unknown id (bad data from the network).
    pub fn look(&self, id: LookId) -> &Look {
        self.looks.get(usize::from(id.0)).unwrap_or(&self.looks[0])
    }

    pub fn id_of(&self, def: &LookDef) -> Option<LookId> {
        self.defs
            .iter()
            .position(|d| d == def)
            .map(|i| LookId(i as u16))
    }
}

/// A look whose clips were baked from a skeleton (docs/PLAN.md §16) has its attacks timed by
/// the animation: each attack lands on the frame its clip's `strike` event is on and recovers for
/// the rest of the clip, so the swing on screen and the hit on the host agree. Every direction of
/// an attack must strike on the same frame and last as long (one moveset times them all).
///
/// An attack's clip starts on the tick the attack does and steps once in that same tick, so the
/// frame drawn is always one ahead of the fighter's tick: striking on frame `s` is fighter tick
/// `s - 1`, and a clip of `n` frames is over after `n - 1` ticks.
fn timed_by_animation(moveset: &mut Moveset, look: &Look) -> Result<(), String> {
    for (i, attack) in moveset.combo.iter_mut().enumerate() {
        let mut baked: Vec<(u32, u32)> = Vec::new();
        for facing in Facing::ALL {
            let name = format!("{}_{}", attack.clip, facing.name());
            // The sheet the controller plays it from: the attack sheet first, as `show` does.
            let found = [&look.attack, &look.base]
                .into_iter()
                .find_map(|sheet| sheet.clip_id(&name).map(|id| (sheet, id)));
            let Some((sheet, id)) = found else {
                continue;
            };
            if let Some(strike) = sheet.timing(id).and_then(|t| t.event("strike")) {
                let ticks = sheet.clips[usize::from(id.0)].frames.len() as u32;
                baked.push((strike, ticks));
            }
        }
        let Some(&(strike, ticks)) = baked.first() else {
            continue;
        };
        if baked.iter().any(|&b| b != (strike, ticks)) {
            return Err(format!(
                "attack {i} ({}): its directions strike on different frames or last differently",
                attack.clip
            ));
        }
        let startup = strike.saturating_sub(1).min(u32::from(u16::MAX)) as u16;
        let rest = ticks
            .saturating_sub(1)
            .saturating_sub(u32::from(startup) + u32::from(attack.active));
        attack.startup = startup;
        attack.recovery = rest.clamp(1, u32::from(u16::MAX)) as u16;
        attack.chain_from = attack.chain_from.min(attack.recovery);
    }
    Ok(())
}

/// How an enemy kind looks, from the project's combat definitions.
pub fn enemy_look(combat: &CombatDef, kind: &str) -> Option<LookDef> {
    let e = combat.enemies.get(kind)?;
    Some(LookDef {
        sheet: e.sheet.clone(),
        attack: e.attack.clone(),
        face: None,
        name: Some(e.name.clone()),
        moveset: Some(e.moveset.clone()),
    })
}

/// This tick's input, filled by replication (remote players), local input (the host's own
/// player) or AI.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlInput {
    pub input: TickInput,
    /// The player's input for this tick has not arrived. The character holds still: no control,
    /// no physics, not even gravity, so after the input is applied later the host is exactly
    /// where the client predicted.
    pub hold: bool,
}

/// The character a player controls.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerAvatar(pub PlayerId);

/// Stable id of a replicated entity, the same on host and clients.
#[derive(
    Component, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct NetId(pub u32);

/// The player is offline; the character sleeps where it is and takes no input.
#[derive(Component, Clone, Copy, Debug)]
pub struct Asleep;

/// Controllers run in this set, after input arrives and before physics.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Control;

/// One tick of a character's controller: combat, animation and what the body should do.
pub fn control(
    state: &mut CharacterState,
    grounded: bool,
    input: TickInput,
    sheets: &CharacterSheets,
) -> MoveIntent {
    let look = sheets.look(state.look);
    let moveset = &look.moveset;
    state.sway = state.sway.wrapping_add(1);
    // Out cold, a character has no will of its own; a blow still moves it.
    let input = if state.impaired.out {
        TickInput::default()
    } else {
        input.sanitized()
    };
    let moving = Facing::from_vector(input.movement);
    if state.fighter.is_free() && state.impaired.out {
        state.sleeping = false;
        if !show(state, look, &[moveset.death_clip.as_str()], false) {
            show_base(state, look, &["idle"], false);
        }
        tick_anim(state, look);
        return MoveIntent::default();
    }
    if state.fighter.is_free() {
        // Asleep: the sleep press gets up again, and so does any other action.
        if state.sleeping {
            let wake = input.sleep || moving.is_some() || input.jump || input.attack || input.dodge;
            if !wake {
                show_base(state, look, &["idle"], false);
                tick_anim(state, look);
                return MoveIntent::default();
            }
            state.sleeping = false;
        } else if input.sleep && grounded {
            state.sleeping = true;
            show_base(state, look, &["idle"], false);
            return MoveIntent::default();
        }
        // A free fighter turns where it walks, and attacks or dodges that way.
        if let Some(facing) = moving {
            state.facing = facing;
        }
    } else {
        state.sleeping = false;
    }

    let wants = Wants {
        movement: input.movement,
        attack: input.attack,
        dodge: input.dodge,
    };
    let tick = state
        .fighter
        .step(moveset, wants, state.facing.vector(), grounded);
    match tick.started {
        Some(Started::Attack(step)) => {
            let clip = moveset.combo[usize::from(step)].clip.as_str();
            show(state, look, &[clip, "attack"], true);
        }
        Some(Started::Dodge) => {
            let clip = moveset.dodge.as_ref().map_or("dodge", |d| d.clip.as_str());
            show(state, look, &[clip, "dodge", "run"], true);
        }
        None => {}
    }
    // Combat owns the body: attacking, dodging, staggered or dead.
    if let Some(velocity) = tick.velocity {
        let animated = match state.fighter.action {
            Action::Hurt { .. } => show(state, look, &[moveset.hurt_clip.as_str()], false),
            Action::Dead { .. } => show(state, look, &[moveset.death_clip.as_str()], false),
            Action::Free => show_base(state, look, &["idle"], false),
            Action::Attack { .. } | Action::Dodge { .. } => true,
        };
        // A look without a hurt or death clip holds its current frame rather than walking on.
        if animated {
            tick_anim(state, look);
        }
        return MoveIntent {
            velocity,
            jump: false,
            hold: false,
        };
    }

    let speed = if input.run { RUN_SPEED } else { WALK_SPEED };
    let speed = speed * f32::from(state.impaired.speed.min(100)) / 100.0;
    let intent = MoveIntent {
        velocity: if moving.is_some() {
            stagger(input.movement, state.sway, state.impaired.wobble) * speed
        } else {
            Vec2::ZERO
        },
        jump: input.jump,
        hold: false,
    };
    // In the air (or leaving the ground this tick) the jump clip plays once from takeoff; a look
    // without one keeps walking in the air.
    let takeoff = input.jump && grounded;
    let airborne = (takeoff || !grounded) && show_base(state, look, &["jump"], takeoff);
    if !airborne {
        let actions: &[&str] = match (moving, input.run) {
            (Some(_), true) => &["run", "walk"],
            (Some(_), false) => &["walk"],
            (None, _) => &["idle"],
        };
        show_base(state, look, actions, false);
    }
    tick_anim(state, look);
    intent
}

/// A drunk's walk: pushed side to side in a triangle wave. Integer phase and plain arithmetic,
/// so host and client compute the same path.
fn stagger(movement: Vec2, sway: u16, wobble: u8) -> Vec2 {
    if wobble == 0 {
        return movement;
    }
    let half = f32::from(SWAY_TICKS / 2);
    let phase = f32::from(sway % SWAY_TICKS);
    let side = if phase < half {
        phase / half * 2.0 - 1.0
    } else {
        3.0 - phase / half * 2.0
    };
    let push = movement.perp() * side * SWAY_PER_WOBBLE * f32::from(wobble.min(4));
    (movement + push).clamp_length_max(1.0)
}

/// The sheet a character's current frame comes from.
pub fn sheet_of<'a>(state: &CharacterState, sheets: &'a CharacterSheets) -> &'a SpriteSheet {
    let look = sheets.look(state.look);
    if state.on_attack_sheet {
        &look.attack
    } else {
        &look.base
    }
}

/// Plays the first of `actions` the attack sheet has, else the base sheet. False if neither
/// has any (the current clip carries on).
fn show(state: &mut CharacterState, look: &Look, actions: &[&str], restart: bool) -> bool {
    for (sheet, on_attack_sheet) in [(&look.attack, true), (&look.base, false)] {
        // Clip numbers belong to their sheet: changing sheet always starts the clip afresh.
        let changing = on_attack_sheet != state.on_attack_sheet;
        if play(
            &mut state.anim,
            sheet,
            actions,
            state.facing,
            restart || changing,
        ) {
            state.on_attack_sheet = on_attack_sheet;
            return true;
        }
    }
    false
}

/// Like [`show`], on the base sheet only (walking, idling, jumping).
fn show_base(state: &mut CharacterState, look: &Look, actions: &[&str], restart: bool) -> bool {
    let changing = state.on_attack_sheet;
    if play(
        &mut state.anim,
        &look.base,
        actions,
        state.facing,
        restart || changing,
    ) {
        state.on_attack_sheet = false;
        return true;
    }
    false
}

fn tick_anim(state: &mut CharacterState, look: &Look) {
    let sheet = if state.on_attack_sheet {
        &look.attack
    } else {
        &look.base
    };
    state.anim.tick(&sheet.clips);
}

/// Plays the first of `actions` that `sheet` has, as `<action>_<facing>` or, for a sheet
/// without that diagonal, `<action>_<cardinal>`. Turning to another direction of the action
/// already playing keeps its progress. False if the sheet has none of them.
fn play(
    anim: &mut AnimationPlayer,
    sheet: &SpriteSheet,
    actions: &[&str],
    facing: Facing,
    restart: bool,
) -> bool {
    for action in actions {
        for dir in [facing, facing.cardinal()] {
            let Some(clip) = sheet.clip_id(&format!("{action}_{}", dir.name())) else {
                continue;
            };
            let current = sheet.clips.get(usize::from(anim.clip().0));
            let same_action = current.is_some_and(|c| {
                c.name
                    .strip_prefix(action)
                    .is_some_and(|rest| rest.starts_with('_'))
            });
            if same_action && !restart {
                anim.switch_keeping_progress(clip);
            } else {
                anim.play(clip, restart);
            }
            return true;
        }
    }
    false
}

/// Runs every character's controller each tick.
pub struct CharactersPlugin(pub CharacterSheets);

impl Plugin for CharactersPlugin {
    fn build(self, app: &mut App) {
        app.insert_resource(self.0)
            .add_systems(FixedUpdate, control_characters.in_set(Control));
    }
}

/// A character its controller runs.
type Controlled = (
    &'static mut CharacterState,
    &'static BodyState,
    &'static ControlInput,
    &'static mut MoveIntent,
    Has<Asleep>,
);

pub(crate) fn control_characters(
    sheets: Res<CharacterSheets>,
    mut characters: Query<Controlled, Without<crate::Dormant>>,
) {
    for (mut state, body, control_input, mut intent, asleep) in &mut characters {
        if control_input.hold && !asleep {
            *intent = MoveIntent {
                hold: true,
                ..MoveIntent::default()
            };
            continue;
        }
        let input = if asleep {
            TickInput::default()
        } else {
            control_input.input
        };
        *intent = control(&mut state, body.0.grounded, input, &sheets);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dark_sprite::Clip;

    /// Four-way walk and attack sheets with the clips the controller looks for, plus a jump
    /// clip facing down only; no pixels needed.
    pub(crate) fn sheets() -> CharacterSheets {
        let mut base = SpriteSheet::default();
        let mut attack = SpriteSheet::default();
        for (i, facing) in Facing::CARDINAL.into_iter().enumerate() {
            let first = i as u32 * 4;
            let dir = facing.name();
            base.add_clip(Clip::walk_cycle(format!("walk_{dir}"), first, 4, 8));
            base.add_clip(Clip::idle(format!("idle_{dir}"), first, 4));
            attack.add_clip(Clip {
                name: format!("attack_{dir}"),
                frames: (first..first + 4).collect(),
                ticks_per_frame: 5,
                looping: false,
                flip_x: false,
            });
        }
        base.add_clip(Clip {
            name: "run_right".into(),
            frames: vec![20, 21],
            ticks_per_frame: 4,
            looping: true,
            flip_x: false,
        });
        base.add_clip(Clip {
            name: "jump_down".into(),
            frames: vec![16, 17, 18, 19],
            ticks_per_frame: 7,
            looping: false,
            flip_x: false,
        });
        CharacterSheets::single(Look {
            base,
            attack,
            moveset: moveset(),
        })
    }

    /// One 20-tick attack (the four five-tick frames of the clip) and a 12-tick dodge.
    pub(crate) fn moveset() -> Moveset {
        Moveset {
            health: 50,
            poise: 0,
            combo: vec![dark_combat::AttackDef {
                clip: "attack".into(),
                startup: 5,
                active: 5,
                recovery: 10,
                chain_from: 4,
                damage: 20,
                poise_damage: 0,
                knockback: 120.0,
                hitstun: 12,
                reach: 14.0,
                radius: 10.0,
                lunge: 0.0,
                hitstop: 4,
            }],
            dodge: Some(dark_combat::DodgeDef {
                clip: "dodge".into(),
                ticks: 12,
                moving: 8,
                speed: 200.0,
                invulnerable: (1, 6),
            }),
            ..Moveset::default()
        }
    }

    fn new_state(sheets: &CharacterSheets) -> CharacterState {
        CharacterState::new(sheets, LookId(0), Facing::Down)
    }

    fn clip_name(state: &CharacterState, sheets: &CharacterSheets) -> String {
        let sheet = sheet_of(state, sheets);
        sheet.clips[state.anim.clip().0 as usize].name.clone()
    }

    #[test]
    fn walking_faces_and_moves() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let intent = control(
            &mut state,
            true,
            TickInput {
                movement: Vec2::new(-1.0, 0.0),
                ..Default::default()
            },
            &sheets,
        );
        assert_eq!(state.facing, Facing::Left);
        assert_eq!(intent.velocity, Vec2::new(-WALK_SPEED, 0.0));
        assert_eq!(clip_name(&state, &sheets), "walk_left");
    }

    #[test]
    fn diagonals_face_eight_ways_but_fall_back_to_sideways_clips() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let diagonal = Vec2::new(-1.0, -1.0).normalize();
        control(
            &mut state,
            true,
            TickInput {
                movement: diagonal,
                ..Default::default()
            },
            &sheets,
        );
        assert_eq!(state.facing, Facing::UpLeft);
        assert_eq!(clip_name(&state, &sheets), "walk_left");
    }

    #[test]
    fn turning_mid_stride_keeps_the_step() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let walk = |x: f32| TickInput {
            movement: Vec2::new(x, 0.0),
            ..Default::default()
        };
        for _ in 0..10 {
            control(&mut state, true, walk(-1.0), &sheets);
        }
        let frame = |s: &CharacterState| s.anim.frame(&sheet_of(s, &sheets).clips).unwrap();
        let step_left = frame(&state) - 4; // walk_left is frames 4..8
        control(&mut state, true, walk(1.0), &sheets);
        assert_eq!(clip_name(&state, &sheets), "walk_right");
        assert_eq!(frame(&state) - 8, step_left, "same step, other direction");
    }

    #[test]
    fn running_is_faster_and_uses_the_run_clip_when_there_is_one() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let run = |movement| TickInput {
            movement,
            run: true,
            ..Default::default()
        };
        let intent = control(&mut state, true, run(Vec2::X), &sheets);
        assert_eq!(intent.velocity, Vec2::new(RUN_SPEED, 0.0));
        assert_eq!(clip_name(&state, &sheets), "run_right");
        // No run clip facing up: it runs with the walk animation.
        control(&mut state, true, run(Vec2::NEG_Y), &sheets);
        assert_eq!(clip_name(&state, &sheets), "walk_up");
        // Run held while standing still is just standing.
        let intent = control(&mut state, true, run(Vec2::ZERO), &sheets);
        assert_eq!(intent.velocity, Vec2::ZERO);
        assert_eq!(clip_name(&state, &sheets), "idle_up");
    }

    #[test]
    fn a_jump_plays_the_jump_clip_until_landing() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let jump = TickInput {
            jump: true,
            ..Default::default()
        };
        assert!(control(&mut state, true, jump, &sheets).jump);
        assert_eq!(clip_name(&state, &sheets), "jump_down");
        for _ in 0..10 {
            control(&mut state, false, TickInput::default(), &sheets);
            assert_eq!(clip_name(&state, &sheets), "jump_down", "still in the air");
        }
        control(&mut state, true, TickInput::default(), &sheets);
        assert_eq!(clip_name(&state, &sheets), "idle_down", "landed");
        // Facing left there is no jump clip: the air shows the walk or idle clip.
        control(
            &mut state,
            false,
            TickInput {
                movement: Vec2::NEG_X,
                ..Default::default()
            },
            &sheets,
        );
        assert_eq!(clip_name(&state, &sheets), "walk_left");
    }

    #[test]
    fn attack_roots_the_character_until_the_clip_ends() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let press = TickInput {
            attack: true,
            movement: Vec2::X,
            ..Default::default()
        };
        assert_eq!(
            control(&mut state, true, press, &sheets).velocity,
            Vec2::ZERO
        );
        assert!(!state.fighter.is_free());
        assert_eq!(
            clip_name(&state, &sheets),
            "attack_right",
            "turned to the press first"
        );
        let walk = TickInput {
            movement: Vec2::X,
            ..Default::default()
        };
        let mut ticks = 0;
        while !state.fighter.is_free() {
            assert_eq!(
                control(&mut state, true, walk, &sheets).velocity,
                Vec2::ZERO
            );
            ticks += 1;
            assert!(ticks < 100);
        }
        assert_eq!(ticks, 4 * 5, "four frames of five ticks");
        assert_ne!(
            control(&mut state, true, walk, &sheets).velocity,
            Vec2::ZERO
        );
    }

    #[test]
    fn a_look_without_a_combo_does_not_attack() {
        let mut sheets = sheets();
        sheets.looks[0].moveset = Moveset::default();
        let mut state = new_state(&sheets);
        let press = TickInput {
            attack: true,
            movement: Vec2::X,
            ..Default::default()
        };
        let intent = control(&mut state, true, press, &sheets);
        assert!(state.fighter.is_free());
        assert_eq!(intent.velocity, Vec2::new(WALK_SPEED, 0.0));
    }

    #[test]
    fn no_attack_in_the_air_and_bad_input_is_sanitized() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        control(
            &mut state,
            false,
            TickInput {
                attack: true,
                ..Default::default()
            },
            &sheets,
        );
        assert!(state.fighter.is_free(), "no attacking in the air");
        // The press is buffered, though: landing straight after, the attack comes out.
        control(&mut state, true, TickInput::default(), &sheets);
        assert!(matches!(
            state.fighter.action,
            dark_combat::Action::Attack { .. }
        ));
        let mut state = new_state(&sheets);
        let intent = control(
            &mut state,
            true,
            TickInput {
                movement: Vec2::new(f32::NAN, 5.0),
                ..Default::default()
            },
            &sheets,
        );
        assert_eq!(intent.velocity, Vec2::ZERO);
        let intent = control(
            &mut state,
            true,
            TickInput {
                movement: Vec2::new(0.0, 5.0),
                ..Default::default()
            },
            &sheets,
        );
        assert_eq!(
            intent.velocity,
            Vec2::new(0.0, WALK_SPEED),
            "clamped to unit length"
        );
    }

    #[test]
    fn a_dodge_rolls_where_the_stick_points_and_a_hit_pushes_back() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let roll = TickInput {
            movement: Vec2::NEG_Y,
            dodge: true,
            ..Default::default()
        };
        let intent = control(&mut state, true, roll, &sheets);
        assert_eq!(intent.velocity, Vec2::new(0.0, -200.0));
        assert!(!intent.jump);
        control(&mut state, true, TickInput::default(), &sheets);
        assert!(state.fighter.invulnerable(&sheets.looks[0].moveset));

        // A hit from the host (from the left) staggers and pushes right, fading.
        let mut state = new_state(&sheets);
        let m = &sheets.looks[0].moveset;
        state.fighter.take_hit(m, &m.combo[0], Vec2::X, 9);
        let push = control(&mut state, true, TickInput::default(), &sheets).velocity;
        assert!(push.x > 0.0 && push.x < 120.0, "{push}");
        // Staggered, it cannot walk or attack.
        let press = TickInput {
            attack: true,
            movement: Vec2::NEG_X,
            ..Default::default()
        };
        assert!(control(&mut state, true, press, &sheets).velocity.x > 0.0);
    }

    #[test]
    fn sleeping_ends_when_the_sleeper_acts_or_is_hit() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        let sleep = TickInput {
            sleep: true,
            ..Default::default()
        };
        control(&mut state, true, sleep, &sheets);
        assert!(state.sleeping);
        let m = &sheets.looks[0].moveset;
        state.fighter.take_hit(m, &m.combo[0], Vec2::X, 9);
        control(&mut state, true, TickInput::default(), &sheets);
        assert!(!state.sleeping, "a hit wakes you");
    }

    #[test]
    fn a_slowed_character_walks_slower_and_a_drunk_one_staggers() {
        let sheets = sheets();
        let walk = TickInput {
            movement: Vec2::X,
            ..Default::default()
        };
        let mut state = new_state(&sheets);
        state.impaired.speed = 50;
        let v = control(&mut state, true, walk, &sheets).velocity;
        assert_eq!(v, Vec2::new(WALK_SPEED / 2.0, 0.0));

        let mut state = new_state(&sheets);
        state.impaired.wobble = 3;
        let sideways: Vec<f32> = (0..SWAY_TICKS)
            .map(|_| control(&mut state, true, walk, &sheets).velocity.y)
            .collect();
        assert!(sideways.iter().any(|&y| y > 10.0) && sideways.iter().any(|&y| y < -10.0));
        assert_eq!(
            state.facing,
            Facing::Right,
            "still faces where it means to go"
        );
        // The same state and input stagger the same way: prediction holds.
        let (mut a, mut b) = (state, state);
        assert_eq!(
            control(&mut a, true, walk, &sheets),
            control(&mut b, true, walk, &sheets)
        );
    }

    #[test]
    fn out_cold_a_character_does_nothing_until_it_comes_to() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        state.sleeping = true;
        state.impaired.out = true;
        let busy = TickInput {
            movement: Vec2::X,
            attack: true,
            jump: true,
            ..Default::default()
        };
        for _ in 0..30 {
            let intent = control(&mut state, true, busy, &sheets);
            assert_eq!(intent, MoveIntent::default());
        }
        assert!(state.fighter.is_free() && !state.sleeping);
        state.impaired.out = false;
        let walk = TickInput {
            movement: Vec2::X,
            ..Default::default()
        };
        assert_ne!(
            control(&mut state, true, walk, &sheets).velocity,
            Vec2::ZERO
        );
    }

    #[test]
    fn a_baked_strike_times_the_attack() {
        let mut sheet = SpriteSheet::default();
        for dir in ["down", "up"] {
            sheet.add_clip(Clip {
                name: format!("attack_{dir}"),
                frames: (0..40).collect(),
                ticks_per_frame: 1,
                looping: false,
                flip_x: false,
            });
            sheet.timing.push(dark_sprite::ClipTiming {
                events: vec![("strike".into(), 18)],
                ..Default::default()
            });
        }
        let look = Look {
            base: sheet.clone(),
            ..Look::default()
        };
        let mut m = moveset();
        timed_by_animation(&mut m, &look).unwrap();
        let a = &m.combo[0];
        assert_eq!((a.startup, a.active, a.recovery), (17, 5, 17));
        assert!(a.chain_from <= a.recovery);

        // On screen: the hit starts on the very tick frame 18 (the strike) is drawn, and the
        // fighter is free when the clip has played out.
        let sheets = CharacterSheets::single(Look {
            base: sheet.clone(),
            attack: SpriteSheet::default(),
            moveset: m.clone(),
        });
        let mut state = CharacterState::new(&sheets, LookId(0), Facing::Down);
        let press = TickInput {
            attack: true,
            ..Default::default()
        };
        control(&mut state, true, press, &sheets);
        let mut ticks = 1;
        while state.fighter.hitbox(&m, Vec2::ZERO, Vec2::Y).is_none() {
            control(&mut state, true, TickInput::default(), &sheets);
            ticks += 1;
        }
        assert_eq!(
            state.anim.step(),
            18,
            "the hit and the strike frame together"
        );
        while !state.fighter.is_free() {
            control(&mut state, true, TickInput::default(), &sheets);
            ticks += 1;
        }
        assert_eq!(ticks, 40, "over with the clip");

        // Directions that disagree are refused.
        let mut uneven = sheet;
        uneven.timing[1].events = vec![("strike".into(), 12)];
        let uneven = Look {
            base: uneven,
            ..Look::default()
        };
        assert!(timed_by_animation(&mut moveset(), &uneven).is_err());
        // A plain sheet leaves the authored timing alone.
        let mut plain = moveset();
        timed_by_animation(&mut plain, &sheets_plain()).unwrap();
        assert_eq!(plain.combo[0].startup, 5);
    }

    fn sheets_plain() -> Look {
        sheets().looks[0].clone()
    }

    #[test]
    fn an_unknown_look_falls_back_to_the_first() {
        let sheets = sheets();
        let mut state = new_state(&sheets);
        state.look = LookId(999);
        control(&mut state, true, TickInput::default(), &sheets);
        assert_eq!(clip_name(&state, &sheets), "idle_down");
    }
}
