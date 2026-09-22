//! How hits feel on screen: hitstop, flashes, sparks and shake. Presentation only.
//!
//! The simulation never pauses for a hit (a client predicting its own character must stay in
//! step with the host), so hitstop is drawn: when a character's hit counter changes, it and its
//! attacker are shown frozen, as they were, for the attack's hitstop frames, then carry on from
//! where the simulation is. Everything here reacts to replicated state, so it looks the same for
//! the host's player and for every client.

use std::collections::HashMap;

use dark_combat::Action;
use dark_world::{CharacterSheets, DrawCharacter, NetId};
use glam::Vec2;

/// Seconds a hit flash lasts.
const FLASH_SECS: f32 = 0.12;
/// Seconds a spark lasts.
const SPARK_SECS: f32 = 0.1;
/// Hitstop frames when the attack cannot be told (60 per second).
const DEFAULT_HITSTOP: u8 = 4;

/// What a character was doing when last seen, to notice what it starts doing.
#[derive(Clone, Copy)]
struct Seen {
    hurts: u8,
    swing: u16,
    dodging: bool,
    dead: bool,
}

impl Seen {
    fn of(c: &DrawCharacter) -> Self {
        let f = &c.state.fighter;
        Self {
            hurts: f.hurts,
            swing: f.swing,
            dodging: matches!(f.action, Action::Dodge { .. }),
            dead: f.is_dead(),
        }
    }
}

pub struct Spark {
    pub at: Vec2,
    pub lift: f32,
    pub left: f32,
}

#[derive(Default)]
pub struct CombatFx {
    seen: HashMap<NetId, Seen>,
    /// Characters shown frozen: how they looked when the hit landed, and for how much longer.
    frozen: HashMap<NetId, (DrawCharacter, f32)>,
    flashes: HashMap<NetId, f32>,
    pub sparks: Vec<Spark>,
    /// Sounds to play this frame: FMOD event paths and where.
    pub sounds: Vec<(&'static str, Vec2)>,
    /// Screen shake strength in pixels, fading.
    shake: f32,
    shake_phase: u32,
}

impl CombatFx {
    /// What to draw this frame: `characters` with frozen ones held as they were. Notices new
    /// hits and starts their effects.
    pub fn update(
        &mut self,
        characters: &[DrawCharacter],
        sheets: &CharacterSheets,
        dt: f32,
    ) -> Vec<DrawCharacter> {
        for timer in self.flashes.values_mut() {
            *timer -= dt;
        }
        self.flashes.retain(|_, t| *t > 0.0);
        for spark in &mut self.sparks {
            spark.left -= dt;
        }
        self.sparks.retain(|s| s.left > 0.0);
        for (_, left) in self.frozen.values_mut() {
            *left -= dt;
        }
        self.frozen.retain(|_, (_, left)| *left > 0.0);
        self.shake = (self.shake - 60.0 * dt).max(0.0);
        self.shake_phase = self.shake_phase.wrapping_add(1);

        self.sounds.clear();
        let me = characters.iter().find(|c| c.you).map(|c| c.id);
        for victim in characters {
            let now = Seen::of(victim);
            // The first sight of a character is not news.
            let Some(before) = self.seen.insert(victim.id, now) else {
                continue;
            };
            // Only a swing counter moving on is a new swing: a rewind after a misprediction can
            // step it back, and that is no swing at all.
            if (1..0x8000).contains(&now.swing.wrapping_sub(before.swing)) {
                // An enemy's swing starts with its tell; a friend's is the swing itself.
                let cue = if victim.hostile {
                    "event:/combat/windup"
                } else {
                    "event:/combat/swing"
                };
                self.sounds.push((cue, victim.ground));
            }
            if now.dodging && !before.dodging {
                self.sounds.push(("event:/combat/dodge", victim.ground));
            }
            if now.dead && !before.dead {
                self.sounds.push(("event:/combat/death", victim.ground));
            }
            if now.hurts == before.hurts {
                continue;
            }
            let cue = if victim.you {
                "event:/combat/hurt"
            } else {
                "event:/combat/hit"
            };
            self.sounds.push((cue, victim.ground));
            let attacker = victim
                .state
                .fighter
                .hit_by
                .and_then(|id| characters.iter().find(|c| c.id.0 == id));
            let hitstop = attacker
                .and_then(|a| {
                    let moveset = &sheets.look(a.state.look).moveset;
                    match a.state.fighter.action {
                        Action::Attack { step, .. } => {
                            moveset.combo.get(usize::from(step)).map(|s| s.hitstop)
                        }
                        _ => None,
                    }
                })
                .unwrap_or(DEFAULT_HITSTOP);
            let secs = f32::from(hitstop) / 60.0;
            for c in std::iter::once(victim).chain(attacker) {
                self.frozen.entry(c.id).or_insert((c.clone(), secs));
            }
            self.flashes.insert(victim.id, FLASH_SECS);
            let toward = attacker.map_or(Vec2::ZERO, |a| (a.ground - victim.ground) * 0.3);
            self.sparks.push(Spark {
                at: victim.ground + toward,
                lift: victim.elevation + 14.0,
                left: SPARK_SECS,
            });
            if Some(victim.id) == me || attacker.is_some_and(|a| a.you) {
                self.shake = self.shake.max(f32::from(hitstop) * 0.6);
            }
        }
        self.seen
            .retain(|id, _| characters.iter().any(|c| c.id == *id));

        characters
            .iter()
            .map(|c| match self.frozen.get(&c.id) {
                Some((held, _)) => held.clone(),
                None => c.clone(),
            })
            .collect()
    }

    /// Strength of the red hit flush on `id`, 0 to 1.
    pub fn flash(&self, id: NetId) -> f32 {
        self.flashes
            .get(&id)
            .map_or(0.0, |t| (t / FLASH_SECS).clamp(0.0, 1.0))
    }

    /// The camera's shake offset this frame, in whole pixels.
    pub fn shake(&self) -> Vec2 {
        if self.shake < 0.5 {
            return Vec2::ZERO;
        }
        // A cheap jitter: alternate directions, never the same twice running.
        let s = self.shake.round();
        match self.shake_phase % 4 {
            0 => Vec2::new(s, 0.0),
            1 => Vec2::new(-s, s * 0.5),
            2 => Vec2::new(s * 0.5, -s),
            _ => Vec2::new(-s * 0.5, 0.0),
        }
    }
}

/// How visible a dead enemy still is: it lies whole for a moment, then fades before it goes.
pub fn corpse_alpha(c: &DrawCharacter) -> f32 {
    match c.state.fighter.action {
        Action::Dead { tick } if c.hostile => {
            let total = f32::from(dark_world::CORPSE_TICKS);
            (1.0 - (f32::from(tick) - total * 0.5) / (total * 0.5)).clamp(0.0, 1.0)
        }
        _ => 1.0,
    }
}

/// Seconds a lost chunk of health stays shown before it drains away.
const TRAIL_HOLD_SECS: f32 = 0.45;
/// How fast the trail drains, in whole health bars per second.
const TRAIL_DRAIN: f32 = 0.9;
/// Seconds a friend's bar stays up after a hit.
const BAR_SHOWN_SECS: f32 = 4.0;

/// Health as the bars show it: the health just lost lingers as a lighter "trail" for a moment,
/// then drains down to the new value, so every hit reads at a glance.
#[derive(Default)]
pub struct HealthTrails {
    bars: HashMap<NetId, Trail>,
}

#[derive(Clone, Copy)]
struct Trail {
    health: u16,
    /// Where the trail ends, in health.
    shown: f32,
    hold: f32,
    visible: f32,
}

impl HealthTrails {
    /// Follows each character's health; `max_of` gives its full health.
    pub fn update(
        &mut self,
        characters: &[DrawCharacter],
        max_of: impl Fn(&DrawCharacter) -> u16,
        dt: f32,
    ) {
        for c in characters {
            self.track(c.id, c.state.fighter.health, max_of(c), dt);
        }
        self.bars
            .retain(|id, _| characters.iter().any(|c| c.id == *id));
    }

    /// One character's health this frame.
    fn track(&mut self, id: NetId, health: u16, max: u16, dt: f32) {
        let max = f32::from(max.max(1));
        let trail = self.bars.entry(id).or_insert(Trail {
            health,
            shown: f32::from(health),
            hold: 0.0,
            visible: 0.0,
        });
        if health < trail.health {
            trail.hold = TRAIL_HOLD_SECS;
            trail.visible = BAR_SHOWN_SECS;
        }
        trail.health = health;
        let now = f32::from(health);
        if now >= trail.shown {
            // Healing shows at once.
            trail.shown = now;
        } else if trail.hold > 0.0 {
            trail.hold -= dt;
        } else {
            trail.shown = (trail.shown - TRAIL_DRAIN * max * dt).max(now);
        }
        trail.visible = (trail.visible - dt).max(0.0);
    }

    /// Where the lingering part of `id`'s bar ends, in health.
    pub fn trail(&self, id: NetId) -> Option<f32> {
        self.bars.get(&id).map(|t| t.shown)
    }

    /// Hit in the last few seconds.
    pub fn recently_hit(&self, id: NetId) -> bool {
        self.bars.get(&id).is_some_and(|t| t.visible > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_health_lingers_then_drains_and_healing_shows_at_once() {
        let mut t = HealthTrails::default();
        let id = NetId(1);
        let frame = 1.0 / 60.0;
        t.track(id, 100, 100, frame);
        assert!(!t.recently_hit(id));
        // A 40-point hit: the bar shows 60, the trail still 100 for a moment.
        t.track(id, 60, 100, frame);
        assert!(t.recently_hit(id));
        assert_eq!(t.trail(id), Some(100.0));
        for _ in 0..20 {
            t.track(id, 60, 100, frame);
        }
        assert_eq!(t.trail(id), Some(100.0), "held for {TRAIL_HOLD_SECS} s");
        for _ in 0..60 {
            t.track(id, 60, 100, frame);
        }
        assert_eq!(t.trail(id), Some(60.0), "drained down to the new health");
        t.track(id, 90, 100, frame);
        assert_eq!(t.trail(id), Some(90.0), "healing needs no trail");
    }
}
