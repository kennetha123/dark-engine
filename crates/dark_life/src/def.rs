//! Life as authored: the project's `life.ron`. Rates are ‰ per game hour (for alcohol, ‰ of a
//! drink per hour); temperatures are hundredths of a degree.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Inventory;
use crate::body::Consumable;

#[derive(Debug, thiserror::Error)]
pub enum LifeError {
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
    #[error("life definition: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct LifeDef {
    #[serde(default)]
    pub rates: Rates,
    /// Things that can be carried, by id.
    #[serde(default)]
    pub items: BTreeMap<String, ItemDef>,
    /// What a new character carries.
    #[serde(default)]
    pub start: Vec<(String, u16)>,
    /// What a new character wears (one of `start`).
    #[serde(default)]
    pub wear: Option<String>,
    /// Air temperature by world region (a scene's `region`).
    #[serde(default)]
    pub climates: BTreeMap<String, Climate>,
    /// For scenes in no region, or a region not listed.
    #[serde(default)]
    pub climate: Climate,
    /// Sheets that draw set-down structures.
    #[serde(default)]
    pub art: StructureArt,
}

impl LifeDef {
    /// The definitions at `path`, or the defaults if the project has no such file.
    pub fn load_or_default(path: &Path) -> Result<Self, LifeError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| LifeError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let def = Self::parse(&text).map_err(|source| LifeError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })?;
        def.validate()?;
        Ok(def)
    }

    /// What a new character carries and wears.
    pub fn new_inventory(&self) -> Inventory {
        let mut inventory = Inventory::with(&self.start);
        inventory.worn = self.wear.clone();
        inventory
    }

    /// Air temperature in `region` at `minute` of the day.
    pub fn air(&self, region: Option<&str>, minute: u32) -> i32 {
        region
            .and_then(|r| self.climates.get(r))
            .unwrap_or(&self.climate)
            .air(minute)
    }

    pub fn parse(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
    }

    pub fn validate(&self) -> Result<(), LifeError> {
        for (id, _) in &self.start {
            if !self.items.contains_key(id) {
                return Err(LifeError::Invalid(format!(
                    "start carries unknown item {id}"
                )));
            }
        }
        if let Some(worn) = &self.wear {
            let wearable = matches!(
                self.items.get(worn).map(|d| &d.use_),
                Some(ItemUse::Wear { .. })
            );
            if !wearable || !self.start.iter().any(|(id, n)| id == worn && *n > 0) {
                return Err(LifeError::Invalid(format!(
                    "wear {worn} must be clothing that start carries"
                )));
            }
        }
        if self.rates.chill <= 0 {
            return Err(LifeError::Invalid("rates.chill must be positive".into()));
        }
        Ok(())
    }
}

/// How fast bodies change. Every number here is a tuning knob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rates {
    pub awake: NeedRates,
    pub asleep: NeedRates,
    /// Fatigue recovered per hour asleep in the open, in a tent, at an inn.
    pub rest_open: u32,
    pub rest_tent: u32,
    pub rest_inn: u32,
    /// Asleep (or out cold), fatigue under this lets a body come to.
    pub come_to_fatigue: u32,
    /// Alcohol cleared per hour awake and asleep.
    pub sober_awake: u32,
    pub sober_asleep: u32,
    pub drunk: DrunkLevels,
    /// Felt temperatures that are comfortable (the core stays warm).
    pub comfort: (i32, i32),
    /// Outside comfort, the core moves by (felt - edge) / chill each minute.
    pub chill: i32,
    /// Inside comfort, the core recovers this much a minute.
    pub recover: i32,
    pub core: CoreLevels,
    /// An inn's air, and what a tent, a fire nearby add.
    pub indoor_air: i32,
    pub tent_warmth: i32,
    pub fire_warmth: i32,
    /// Health lost per hour.
    pub harm: Harm,
}

impl Default for Rates {
    fn default() -> Self {
        Self {
            awake: NeedRates {
                hunger: 40,
                thirst: 60,
                fatigue: 55,
                bladder: 70,
                bowel: 30,
                hygiene: 20,
            },
            asleep: NeedRates {
                hunger: 20,
                thirst: 30,
                fatigue: 0,
                bladder: 35,
                bowel: 15,
                hygiene: 10,
            },
            rest_open: 90,
            rest_tent: 130,
            rest_inn: 180,
            come_to_fatigue: 600,
            sober_awake: 350,
            sober_asleep: 500,
            drunk: DrunkLevels::default(),
            comfort: (1200, 2800),
            chill: 600,
            recover: 10,
            core: CoreLevels::default(),
            indoor_air: 2000,
            tent_warmth: 400,
            fire_warmth: 1200,
            harm: Harm::default(),
        }
    }
}

/// ‰ per hour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NeedRates {
    pub hunger: u32,
    pub thirst: u32,
    pub fatigue: u32,
    pub bladder: u32,
    pub bowel: u32,
    pub hygiene: u32,
}

/// ‰ of a drink in the blood at each stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrunkLevels {
    pub tipsy: u32,
    pub drunk: u32,
    pub wasted: u32,
    pub blackout: u32,
    /// Extra fatigue per hour when drunk (twice that when wasted).
    pub drowsy: u32,
}

impl Default for DrunkLevels {
    fn default() -> Self {
        Self {
            tipsy: 800,
            drunk: 2000,
            wasted: 3500,
            blackout: 5000,
            drowsy: 60,
        }
    }
}

/// Core temperatures (hundredths of a degree) where each stage begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreLevels {
    pub cold: i32,
    pub freezing: i32,
    pub hypothermic: i32,
    pub hot: i32,
    pub heatstroke: i32,
}

impl Default for CoreLevels {
    fn default() -> Self {
        Self {
            cold: 3600,
            freezing: 3450,
            hypothermic: 3250,
            hot: 3800,
            heatstroke: 3950,
        }
    }
}

/// Health lost per hour in each state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Harm {
    pub starving: u32,
    pub parched: u32,
    pub freezing: u32,
    pub hypothermic: u32,
    pub heatstroke: u32,
}

impl Default for Harm {
    fn default() -> Self {
        Self {
            starving: 6,
            parched: 12,
            freezing: 10,
            hypothermic: 40,
            heatstroke: 20,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ItemDef {
    /// Name key (`locale/<code>.ron`).
    pub name: String,
    #[serde(default)]
    pub icon: Option<IconDef>,
    #[serde(rename = "use")]
    pub use_: ItemUse,
}

/// An icon on an icon sheet (a grid of `size`-pixel cells).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct IconDef {
    pub image: String,
    pub size: u32,
    pub cell: (u32, u32),
}

/// A place's air through the day, in hundredths of a degree: coldest (`night`) at 05:00,
/// warmest (`day`) at 17:00, in a straight line between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Climate {
    pub day: i32,
    pub night: i32,
}

impl Default for Climate {
    fn default() -> Self {
        Self {
            day: 2000,
            night: 1400,
        }
    }
}

impl Climate {
    pub const COLDEST: u32 = 5 * 60;
    pub const WARMEST: u32 = 17 * 60;

    /// The air at `minute` of the day (0 to 1439).
    pub fn air(&self, minute: u32) -> i32 {
        let minute = minute % (24 * 60);
        let half = (Self::WARMEST - Self::COLDEST) as i32;
        // Minutes since the coldest hour, folded so the afternoon mirrors the morning.
        let since = ((minute + 24 * 60 - Self::COLDEST) % (24 * 60)) as i32;
        let warming = if since <= half {
            since
        } else {
            2 * half - since
        };
        self.night + (self.day - self.night) * warming / half
    }
}

/// Sheets (project paths) for set-down structures: a tent's first frame; a campfire's `burn`
/// clip while it burns, its first frame once out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct StructureArt {
    #[serde(default)]
    pub tent: Option<String>,
    #[serde(default)]
    pub campfire: Option<String>,
}

/// What using an item does.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum ItemUse {
    /// Eaten or drunk: gone once used.
    Consume(Consumable),
    /// Put on (or taken off): warmth while worn, in hundredths of a degree.
    Wear { insulation: i32 },
    /// Heating magic: warmth for a while. Used up.
    Warm { bonus: i32, minutes: u32 },
    /// Set down in the world. Used up.
    Place(Structure),
    /// Soap and water: clean again. Used up.
    Wash,
}

/// Something set down in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Structure {
    /// Shelter to sleep in.
    Tent,
    /// Warmth nearby, for as many game minutes as it burns.
    Campfire { minutes: u32 },
}
