//! Combat as authored: the project's `combat.ron`. Frame data is in simulation ticks (60 Hz),
//! distances in pixels, speeds in pixels per second.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum CombatError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: String,
        source: Box<ron::error::SpannedError>,
    },
    #[error("combat definition: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CombatDef {
    /// How characters fight, by id; looks name the moveset they use.
    #[serde(default)]
    pub movesets: BTreeMap<String, Moveset>,
    /// Kinds of enemy scenes can place, by id.
    #[serde(default)]
    pub enemies: BTreeMap<String, EnemyDef>,
}

impl CombatDef {
    /// The definitions at `path`, or none at all if the project has no such file.
    pub fn load_or_default(path: &Path) -> Result<Self, CombatError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| CombatError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let def = Self::parse(&text).map_err(|source| CombatError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })?;
        def.validate()?;
        Ok(def)
    }

    pub fn parse(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
    }

    pub fn validate(&self) -> Result<(), CombatError> {
        let bad = |m: String| Err(CombatError::Invalid(m));
        for (id, m) in &self.movesets {
            if m.health == 0 {
                return bad(format!("moveset {id} has no health"));
            }
            if m.combo.len() > usize::from(u8::MAX) {
                return bad(format!("moveset {id}: a combo of at most 255 attacks"));
            }
            for (i, a) in m.combo.iter().enumerate() {
                if u32::from(a.startup) + u32::from(a.active) + u32::from(a.recovery)
                    > u32::from(u16::MAX)
                {
                    return bad(format!("moveset {id}: attack {i} lasts too long"));
                }
                if a.active == 0 {
                    return bad(format!("moveset {id}: attack {i} is never active"));
                }
                if a.chain_from > a.recovery {
                    return bad(format!(
                        "moveset {id}: attack {i} chains from {} ticks into a {}-tick recovery",
                        a.chain_from, a.recovery
                    ));
                }
            }
            if let Some(d) = &m.dodge
                && (d.invulnerable.0 > d.invulnerable.1 || d.invulnerable.1 > d.ticks)
            {
                return bad(format!("moveset {id}: dodge i-frames outside the dodge"));
            }
        }
        for (id, e) in &self.enemies {
            if !self.movesets.contains_key(&e.moveset) {
                return bad(format!("enemy {id} uses unknown moveset {}", e.moveset));
            }
            let ai = &e.ai;
            let positive = |v: f32| v.is_finite() && v > 0.0;
            if !(positive(ai.sight) && positive(ai.leash) && positive(ai.attack_range)) {
                return bad(format!(
                    "enemy {id}: sight, leash and attack_range must be positive"
                ));
            }
            if !(positive(ai.speed) && ai.speed <= 2.0) {
                return bad(format!(
                    "enemy {id}: speed {} must be above 0, at most 2",
                    ai.speed
                ));
            }
        }
        Ok(())
    }
}

/// How a character fights: its body's toughness, its combo and its dodge.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Moveset {
    pub health: u16,
    /// Stagger resistance: hits take poise, and only a hit that empties it staggers.
    #[serde(default)]
    pub poise: u16,
    /// Ticks without being hit before poise is whole again.
    #[serde(default = "default_poise_recovery")]
    pub poise_recovery: u16,
    /// Attacks in order; pressing attack again during or just after one continues the combo.
    #[serde(default)]
    pub combo: Vec<AttackDef>,
    #[serde(default)]
    pub dodge: Option<DodgeDef>,
    /// Staggered by a hit: the clip, and ticks unable to act unless the attack says otherwise.
    #[serde(default = "default_hurt_clip")]
    pub hurt_clip: String,
    #[serde(default = "default_death_clip")]
    pub death_clip: String,
}

impl Default for Moveset {
    /// A plain body: can be hurt, cannot fight back.
    fn default() -> Self {
        Self {
            health: 100,
            poise: 0,
            poise_recovery: default_poise_recovery(),
            combo: Vec::new(),
            dodge: None,
            hurt_clip: default_hurt_clip(),
            death_clip: default_death_clip(),
        }
    }
}

/// One attack: frame data, what it does on hit, and where it reaches.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct AttackDef {
    /// Animation clip, played as `<clip>_<facing>`; falls back to `attack`.
    #[serde(default = "default_attack_clip")]
    pub clip: String,
    /// Ticks before the hit can land (the wind-up an enemy's attack is read by).
    pub startup: u16,
    /// Ticks the hitbox is out.
    pub active: u16,
    /// Ticks after, before moving again.
    pub recovery: u16,
    /// Ticks into recovery from which a buffered attack or a dodge cuts recovery short.
    #[serde(default)]
    pub chain_from: u16,
    pub damage: u16,
    #[serde(default)]
    pub poise_damage: u16,
    /// Push on the victim when it staggers, in pixels per second (fading over the stagger).
    #[serde(default)]
    pub knockback: f32,
    /// Ticks the victim is staggered.
    #[serde(default = "default_hitstun")]
    pub hitstun: u16,
    /// The hitbox: a circle this far in front of the feet, this big.
    pub reach: f32,
    pub radius: f32,
    /// Forward speed while winding up and striking.
    #[serde(default)]
    pub lunge: f32,
    /// Frames both sides freeze on screen when it lands (presentation only).
    #[serde(default = "default_hitstop")]
    pub hitstop: u8,
}

impl AttackDef {
    pub fn total(&self) -> u16 {
        self.startup + self.active + self.recovery
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DodgeDef {
    #[serde(default = "default_dodge_clip")]
    pub clip: String,
    /// The whole dodge, moving then recovering.
    pub ticks: u16,
    /// Ticks moving at `speed`; the rest is recovery.
    pub moving: u16,
    pub speed: f32,
    /// First and last tick (inclusive) nothing can hurt the dodger.
    pub invulnerable: (u16, u16),
}

/// A kind of enemy: how it looks, fights and thinks.
#[derive(Clone, Debug, Deserialize)]
pub struct EnemyDef {
    /// Name key (`locale/<code>.ron`).
    pub name: String,
    pub sheet: String,
    #[serde(default)]
    pub attack: Option<String>,
    pub moveset: String,
    pub ai: AiDef,
}

/// Enemy behaviour tuning (the states are fixed: guard, chase, attack, back off, return).
#[derive(Clone, Debug, Deserialize)]
pub struct AiDef {
    /// Notices a player this close.
    pub sight: f32,
    /// Gives up and walks home this far from home.
    pub leash: f32,
    /// Attacks when the target is this close.
    pub attack_range: f32,
    /// Movement speed as a fraction of walking speed (0..=1), or running above 1 (up to 2).
    #[serde(default = "one")]
    pub speed: f32,
    /// Ticks spent backing off after an attack before closing in again.
    #[serde(default)]
    pub back_off: u16,
    /// Ticks after death before it returns to its post.
    #[serde(default = "default_respawn")]
    pub respawn: u32,
}

fn default_poise_recovery() -> u16 {
    90
}
fn default_hurt_clip() -> String {
    "hurt".into()
}
fn default_death_clip() -> String {
    "dead".into()
}
fn default_attack_clip() -> String {
    "attack".into()
}
fn default_dodge_clip() -> String {
    "dodge".into()
}
fn default_hitstun() -> u16 {
    18
}
fn default_hitstop() -> u8 {
    4
}
fn default_respawn() -> u32 {
    60 * 30
}
fn one() -> f32 {
    1.0
}
