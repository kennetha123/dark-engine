//! Combat (docs/PLAN.md §4.7, §13): what a fighter is doing from tick to tick, and what a hit
//! does to it.
//!
//! [`Fighter::step`] is a pure function of the fighter, its [`Moveset`] and this tick's wishes,
//! like the character controller it runs inside: a client predicts its own attacks and dodges
//! with the code the host runs. Hits are the host's business ([`Fighter::take_hit`]): a client
//! learns it was hit from the next snapshot.
//!
//! An attack is startup (wind-up), active (the hitbox is out) and recovery. Pressing attack again
//! chains the combo: during recovery from `chain_from` on it cuts straight into the next attack,
//! and just after an attack it continues the combo instead of starting over. Presses are buffered
//! for a few ticks, so a press slightly early still counts. A dodge can cut recovery short too,
//! and is untouchable for its invulnerable ticks.

mod def;

use glam::Vec2;
use serde::{Deserialize, Serialize};

pub use def::{AiDef, AttackDef, CombatDef, CombatError, DodgeDef, EnemyDef, Moveset};

/// Ticks a press waits to be used.
pub const BUFFER_TICKS: u8 = 10;
/// Ticks after an attack in which attacking again continues the combo.
pub const COMBO_WINDOW: u8 = 24;

/// What a fighter is doing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Free to move and act.
    Free,
    Attack {
        step: u8,
        tick: u16,
    },
    Dodge {
        tick: u16,
        dir: Vec2,
    },
    /// Staggered: pushed by `knock` (fading), unable to act until `total`.
    Hurt {
        tick: u16,
        total: u16,
        knock: Vec2,
    },
    Dead {
        tick: u16,
    },
}

/// A fighter's state. Small and `Copy`: it is replicated with the character.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fighter {
    pub action: Action,
    pub health: u16,
    pub poise: u16,
    /// Ticks since the last hit taken (poise comes back after a while).
    since_hit: u16,
    attack_buffer: u8,
    dodge_buffer: u8,
    /// After an attack: ticks left to continue the combo, and the step it continues with.
    combo_window: u8,
    next_step: u8,
    /// Counts attacks started, so each swing hits a target at most once.
    pub swing: u16,
    /// Hits taken; the presentation reacts when this changes.
    pub hurts: u8,
    /// Who landed the last hit (a replicated id), so both sides can freeze for hitstop.
    pub hit_by: Option<u32>,
}

/// What the controller wants this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wants {
    pub movement: Vec2,
    pub attack: bool,
    pub dodge: bool,
}

/// What the fighter does with its body this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    /// `Some`: combat moves the body at this velocity and it cannot jump. `None`: free movement.
    pub velocity: Option<Vec2>,
    pub started: Option<Started>,
}

/// An action that began this tick, for the animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Started {
    Attack(u8),
    Dodge,
}

/// What a hit did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitOutcome {
    /// Dodged through or already dead.
    Missed,
    /// Hurt, but poise held: it keeps doing what it was doing.
    Damaged,
    Staggered,
    Killed,
}

impl Fighter {
    pub fn new(moveset: &Moveset) -> Self {
        Self {
            action: Action::Free,
            health: moveset.health,
            poise: moveset.poise,
            since_hit: u16::MAX,
            attack_buffer: 0,
            dodge_buffer: 0,
            combo_window: 0,
            next_step: 0,
            swing: 0,
            hurts: 0,
            hit_by: None,
        }
    }

    pub fn is_dead(&self) -> bool {
        matches!(self.action, Action::Dead { .. })
    }

    /// Free to walk, jump, talk or sleep.
    pub fn is_free(&self) -> bool {
        self.action == Action::Free
    }

    pub fn invulnerable(&self, moveset: &Moveset) -> bool {
        match self.action {
            Action::Dead { .. } => true,
            Action::Dodge { tick, .. } => moveset
                .dodge
                .as_ref()
                .is_some_and(|d| (d.invulnerable.0..=d.invulnerable.1).contains(&tick)),
            _ => false,
        }
    }

    /// The attack being made, if any.
    pub fn attack<'m>(&self, moveset: &'m Moveset) -> Option<&'m AttackDef> {
        match self.action {
            Action::Attack { step, .. } => moveset.combo.get(usize::from(step)),
            _ => None,
        }
    }

    /// Where the attack can hit this tick: a circle (centre, radius) in front of `feet`, only
    /// while the attack is active.
    pub fn hitbox(&self, moveset: &Moveset, feet: Vec2, facing: Vec2) -> Option<(Vec2, f32)> {
        let Action::Attack { tick, .. } = self.action else {
            return None;
        };
        let a = self.attack(moveset)?;
        (a.startup..a.startup + a.active)
            .contains(&tick)
            .then(|| (feet + facing * a.reach, a.radius))
    }

    /// One tick. `facing` is the unit vector the character faces; attacks go that way, and so
    /// does a dodge without movement.
    pub fn step(&mut self, moveset: &Moveset, wants: Wants, facing: Vec2, grounded: bool) -> Tick {
        self.attack_buffer = if wants.attack {
            BUFFER_TICKS
        } else {
            self.attack_buffer.saturating_sub(1)
        };
        self.dodge_buffer = if wants.dodge {
            BUFFER_TICKS
        } else {
            self.dodge_buffer.saturating_sub(1)
        };
        self.since_hit = self.since_hit.saturating_add(1);
        if self.since_hit >= moveset.poise_recovery {
            self.poise = moveset.poise;
        }
        let still = Tick {
            velocity: Some(Vec2::ZERO),
            started: None,
        };
        match self.action {
            Action::Dead { tick } => {
                self.action = Action::Dead {
                    tick: tick.saturating_add(1),
                };
                still
            }
            Action::Hurt { tick, total, knock } => {
                let tick = tick + 1;
                if tick >= total {
                    self.action = Action::Free;
                    return still;
                }
                self.action = Action::Hurt { tick, total, knock };
                let fade = f32::from(total - tick) / f32::from(total);
                Tick {
                    velocity: Some(knock * fade),
                    started: None,
                }
            }
            Action::Dodge { tick, dir } => {
                let Some(dodge) = &moveset.dodge else {
                    self.action = Action::Free;
                    return still;
                };
                let tick = tick + 1;
                if tick >= dodge.ticks {
                    self.action = Action::Free;
                    return still;
                }
                // Out of the roll, an attack can start straight away.
                if tick >= dodge.moving && self.attack_buffer > 0 && !moveset.combo.is_empty() {
                    return self.start_attack(moveset, 0, facing);
                }
                self.action = Action::Dodge { tick, dir };
                Tick {
                    velocity: Some(if tick < dodge.moving {
                        dir * dodge.speed
                    } else {
                        Vec2::ZERO
                    }),
                    started: None,
                }
            }
            Action::Attack { step, tick } => {
                let Some(attack) = moveset.combo.get(usize::from(step)) else {
                    self.action = Action::Free;
                    return still;
                };
                let tick = tick + 1;
                let next = (step + 1) % moveset.combo.len() as u8;
                let striking = attack.startup + attack.active;
                if tick >= striking && tick - striking >= attack.chain_from {
                    if self.dodge_buffer > 0 && moveset.dodge.is_some() {
                        return self.start_dodge(moveset, wants.movement, facing);
                    }
                    if self.attack_buffer > 0 {
                        return self.start_attack(moveset, next, facing);
                    }
                }
                if tick >= attack.total() {
                    self.action = Action::Free;
                    self.combo_window = COMBO_WINDOW;
                    self.next_step = next;
                    return still;
                }
                self.action = Action::Attack { step, tick };
                Tick {
                    velocity: Some(if tick < striking {
                        facing * attack.lunge
                    } else {
                        Vec2::ZERO
                    }),
                    started: None,
                }
            }
            Action::Free => {
                self.combo_window = self.combo_window.saturating_sub(1);
                if !grounded {
                    return Tick {
                        velocity: None,
                        started: None,
                    };
                }
                if self.dodge_buffer > 0 && moveset.dodge.is_some() {
                    return self.start_dodge(moveset, wants.movement, facing);
                }
                if self.attack_buffer > 0 && !moveset.combo.is_empty() {
                    let step = if self.combo_window > 0 {
                        self.next_step
                    } else {
                        0
                    };
                    return self.start_attack(moveset, step, facing);
                }
                Tick {
                    velocity: None,
                    started: None,
                }
            }
        }
    }

    fn start_attack(&mut self, moveset: &Moveset, step: u8, facing: Vec2) -> Tick {
        self.attack_buffer = 0;
        self.combo_window = 0;
        self.swing = self.swing.wrapping_add(1);
        self.action = Action::Attack { step, tick: 0 };
        let lunge = moveset.combo[usize::from(step)].lunge;
        Tick {
            velocity: Some(facing * lunge),
            started: Some(Started::Attack(step)),
        }
    }

    fn start_dodge(&mut self, moveset: &Moveset, movement: Vec2, facing: Vec2) -> Tick {
        let dodge = moveset.dodge.as_ref().expect("checked by the caller");
        self.dodge_buffer = 0;
        self.attack_buffer = 0;
        self.combo_window = 0;
        let dir = movement.try_normalize().unwrap_or(facing);
        self.action = Action::Dodge { tick: 0, dir };
        Tick {
            velocity: Some(dir * dodge.speed),
            started: Some(Started::Dodge),
        }
    }

    /// `attack` from the fighter with replicated id `attacker` lands; `push` points away from
    /// the attacker. Host only.
    pub fn take_hit(
        &mut self,
        moveset: &Moveset,
        attack: &AttackDef,
        push: Vec2,
        attacker: u32,
    ) -> HitOutcome {
        if self.invulnerable(moveset) {
            return HitOutcome::Missed;
        }
        self.health = self.health.saturating_sub(attack.damage);
        self.since_hit = 0;
        self.hurts = self.hurts.wrapping_add(1);
        self.hit_by = Some(attacker);
        self.attack_buffer = 0;
        self.dodge_buffer = 0;
        if self.health == 0 {
            self.action = Action::Dead { tick: 0 };
            return HitOutcome::Killed;
        }
        self.poise = self.poise.saturating_sub(attack.poise_damage);
        if self.poise > 0 {
            return HitOutcome::Damaged;
        }
        self.poise = moveset.poise;
        self.combo_window = 0;
        self.action = Action::Hurt {
            tick: 0,
            total: attack.hitstun.max(1),
            knock: push.normalize_or_zero() * attack.knockback,
        };
        HitOutcome::Staggered
    }

    /// Health lost to the body (hunger, cold), not a blow: no stagger, no hit reaction. True if
    /// it killed. Host only.
    pub fn suffer(&mut self, damage: u16) -> bool {
        if damage == 0 || self.is_dead() {
            return false;
        }
        self.health = self.health.saturating_sub(damage);
        if self.health == 0 {
            self.action = Action::Dead { tick: 0 };
            self.attack_buffer = 0;
            self.dodge_buffer = 0;
            self.hit_by = None;
            return true;
        }
        false
    }

    /// Whole again and free: after respawning.
    pub fn revive(&mut self, moveset: &Moveset) {
        let (swing, hurts) = (self.swing, self.hurts);
        *self = Self::new(moveset);
        self.swing = swing;
        self.hurts = hurts;
    }
}

#[cfg(test)]
mod tests;
