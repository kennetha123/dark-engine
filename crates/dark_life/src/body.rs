//! One living body: needs, drink and warmth, stepped a game minute at a time.

use serde::{Deserialize, Serialize};

use crate::def::Rates;

/// Needs are counted in sixtieths of a per-mille: a rate "per hour, in ‰" is then exactly what
/// one game minute adds, with no rounding.
pub const SCALE: u32 = 60;
/// A need at 1000 ‰: failed.
pub const FULL: u32 = 1000 * SCALE;
/// Normal body temperature, in hundredths of a degree.
pub const WARM_CORE: i32 = 3700;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Need {
    Hunger,
    Thirst,
    Fatigue,
    Bladder,
    Bowel,
    /// Dirt, sweat and worse: 0 is clean.
    Hygiene,
}

impl Need {
    pub const ALL: [Need; 6] = [
        Need::Hunger,
        Need::Thirst,
        Need::Fatigue,
        Need::Bladder,
        Need::Bowel,
        Need::Hygiene,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// How pressing a need is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Fine,
    /// From 500 ‰.
    Wanting,
    /// From 800 ‰.
    Urgent,
    /// At 1000 ‰.
    Failed,
}

/// Where the body is, for one minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Surroundings {
    /// Air temperature, in hundredths of a degree.
    pub air: i32,
    pub shelter: Shelter,
    /// Close to a fire.
    pub fire: bool,
    /// Warmth the clothes worn add, in hundredths of a degree.
    pub insulation: i32,
    /// Lying down asleep (by choice; passing out counts as asleep on its own).
    pub asleep: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Shelter {
    #[default]
    Open,
    Tent,
    /// Indoors: its own warm air, and the best sleep.
    Inn,
}

/// What happened to the body in a minute that others need to know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Minute {
    /// Health lost to hunger, thirst, cold or heat.
    pub damage: u16,
    /// Needs that failed, embarrassingly (bladder, bowel).
    pub accidents: Vec<Need>,
    pub passed_out: bool,
    pub came_to: bool,
}

/// Something eaten or drunk: ‰ changes to needs (negative relieves) and drink in ‰ of a drink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Consumable {
    pub hunger: i32,
    pub thirst: i32,
    pub fatigue: i32,
    pub bladder: i32,
    pub bowel: i32,
    pub hygiene: i32,
    /// Alcohol: 1000 is one drink.
    pub alcohol: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    needs: [u32; 6],
    /// Alcohol in the blood, sixtieths of ‰ of a drink (like needs).
    alcohol: u32,
    /// Core temperature, hundredths of a degree.
    pub core: i32,
    /// Heating magic: extra warmth, and minutes it lasts.
    pub warmth: i32,
    pub warmth_minutes: u32,
    /// Had an accident and has not washed since.
    pub soiled: bool,
    /// Out cold (exhaustion or drink): cannot act until it passes.
    pub passed_out: bool,
    /// Harm owed, in sixtieths of a point: harm per hour adds up exactly.
    harm: u32,
    /// Chill (or heat) owed to the core, below one hundredth of a degree: a little cold adds
    /// up instead of rounding away.
    #[serde(default)]
    chill: i32,
}

impl Default for Body {
    fn default() -> Self {
        Self::new()
    }
}

impl Body {
    /// Rested, fed and warm.
    pub fn new() -> Self {
        Self {
            needs: [0; 6],
            alcohol: 0,
            core: WARM_CORE,
            warmth: 0,
            warmth_minutes: 0,
            soiled: false,
            passed_out: false,
            harm: 0,
            chill: 0,
        }
    }

    /// A need in ‰, 0 (fine) to 1000 (failed).
    pub fn need(&self, need: Need) -> u32 {
        self.needs[need.index()] / SCALE
    }

    pub fn level(&self, need: Need) -> Level {
        match self.need(need) {
            1000.. => Level::Failed,
            800.. => Level::Urgent,
            500.. => Level::Wanting,
            _ => Level::Fine,
        }
    }

    pub fn set_need(&mut self, need: Need, permille: u32) {
        self.needs[need.index()] = permille.min(1000) * SCALE;
    }

    fn add(&mut self, need: Need, units: i64) {
        let v = &mut self.needs[need.index()];
        *v = (i64::from(*v) + units).clamp(0, i64::from(FULL)) as u32;
    }

    /// Drink in the blood, in ‰ of a drink.
    pub fn alcohol(&self) -> u32 {
        self.alcohol / SCALE
    }

    pub fn drunkenness(&self, rates: &Rates) -> Drunk {
        let a = self.alcohol();
        let d = &rates.drunk;
        if a >= d.blackout {
            Drunk::Blackout
        } else if a >= d.wasted {
            Drunk::Wasted
        } else if a >= d.drunk {
            Drunk::Drunk
        } else if a >= d.tipsy {
            Drunk::Tipsy
        } else {
            Drunk::Sober
        }
    }

    /// How warm the body feels where it is, in hundredths of a degree: the air (or the inn's),
    /// plus clothes, fire, shelter and magic.
    pub fn felt(&self, around: &Surroundings, rates: &Rates) -> i32 {
        let air = match around.shelter {
            Shelter::Inn => rates.indoor_air.max(around.air),
            _ => around.air,
        };
        let shelter = if around.shelter == Shelter::Tent {
            rates.tent_warmth
        } else {
            0
        };
        let fire = if around.fire { rates.fire_warmth } else { 0 };
        let magic = if self.warmth_minutes > 0 {
            self.warmth
        } else {
            0
        };
        air + around.insulation + shelter + fire + magic
    }

    pub fn eat_or_drink(&mut self, what: &Consumable) {
        let s = i64::from(SCALE);
        self.add(Need::Hunger, i64::from(what.hunger) * s);
        self.add(Need::Thirst, i64::from(what.thirst) * s);
        self.add(Need::Fatigue, i64::from(what.fatigue) * s);
        self.add(Need::Bladder, i64::from(what.bladder) * s);
        self.add(Need::Bowel, i64::from(what.bowel) * s);
        self.add(Need::Hygiene, i64::from(what.hygiene) * s);
        self.alcohol = self.alcohol.saturating_add(what.alcohol * SCALE);
    }

    /// Goes to the toilet (or behind a bush).
    pub fn relieve(&mut self) {
        self.set_need(Need::Bladder, 0);
        self.set_need(Need::Bowel, 0);
    }

    pub fn wash(&mut self) {
        self.set_need(Need::Hygiene, 0);
        self.soiled = false;
    }

    /// Heating magic: `bonus` warmth for `minutes`.
    pub fn warm(&mut self, bonus: i32, minutes: u32) {
        self.warmth = bonus;
        self.warmth_minutes = minutes;
    }

    /// One game minute.
    pub fn minute(&mut self, around: &Surroundings, rates: &Rates) -> Minute {
        let mut out = Minute::default();
        let asleep = around.asleep || self.passed_out;
        let r = if asleep { &rates.asleep } else { &rates.awake };
        let drunk = self.drunkenness(rates);
        self.add(Need::Hunger, i64::from(r.hunger));
        self.add(Need::Thirst, i64::from(r.thirst));
        self.add(Need::Bladder, i64::from(r.bladder));
        self.add(Need::Bowel, i64::from(r.bowel));
        self.add(Need::Hygiene, i64::from(r.hygiene));
        let fatigue = if asleep {
            -i64::from(match around.shelter {
                Shelter::Open => rates.rest_open,
                Shelter::Tent => rates.rest_tent,
                Shelter::Inn => rates.rest_inn,
            })
        } else {
            i64::from(r.fatigue) + i64::from(drunk.extra_fatigue(rates))
        };
        self.add(Need::Fatigue, fatigue);
        self.alcohol = self.alcohol.saturating_sub(if asleep {
            rates.sober_asleep
        } else {
            rates.sober_awake
        });
        if self.warmth_minutes > 0 {
            self.warmth_minutes -= 1;
        }

        // The core drifts towards what the surroundings allow, by how far outside comfort they
        // are; what is left over below a hundredth of a degree is carried to the next minute.
        let felt = self.felt(around, rates);
        let (low, high) = rates.comfort;
        let outside = if felt < low {
            felt - low
        } else if felt > high {
            felt - high
        } else {
            0
        };
        if outside == 0 {
            self.chill = 0;
            self.core +=
                (WARM_CORE - self.core).signum() * rates.recover.min((WARM_CORE - self.core).abs());
        } else {
            let owed = self.chill + outside;
            self.core += owed / rates.chill;
            self.chill = owed % rates.chill;
        }
        self.core = self.core.clamp(2500, 4500);

        // Needs that fail.
        for need in [Need::Bladder, Need::Bowel] {
            if self.level(need) == Level::Failed {
                self.set_need(need, 0);
                self.set_need(Need::Hygiene, 1000);
                self.soiled = true;
                out.accidents.push(need);
            }
        }
        let mut harm = 0;
        if self.level(Need::Hunger) == Level::Failed {
            harm += rates.harm.starving;
        }
        if self.level(Need::Thirst) == Level::Failed {
            harm += rates.harm.parched;
        }
        harm += match self.temperature(rates) {
            Temperature::Hypothermic => rates.harm.hypothermic,
            Temperature::Freezing => rates.harm.freezing,
            Temperature::Heatstroke => rates.harm.heatstroke,
            _ => 0,
        };
        self.harm += harm;
        out.damage = (self.harm / 60) as u16;
        self.harm %= 60;

        // Out cold from exhaustion or drink (as drunk as it was when the minute began), until
        // rested and sobered enough.
        let collapse = self.level(Need::Fatigue) == Level::Failed || drunk == Drunk::Blackout;
        if !self.passed_out && collapse {
            self.passed_out = true;
            out.passed_out = true;
        } else if self.passed_out
            && self.need(Need::Fatigue) < rates.come_to_fatigue
            && self.drunkenness(rates) < Drunk::Wasted
        {
            self.passed_out = false;
            out.came_to = true;
        }
        out
    }

    pub fn temperature(&self, rates: &Rates) -> Temperature {
        let t = &rates.core;
        match self.core {
            c if c < t.hypothermic => Temperature::Hypothermic,
            c if c < t.freezing => Temperature::Freezing,
            c if c < t.cold => Temperature::Cold,
            c if c > t.heatstroke => Temperature::Heatstroke,
            c if c > t.hot => Temperature::Hot,
            _ => Temperature::Comfortable,
        }
    }

    /// What the body's state does to the character, and how it shows.
    pub fn condition(&self, rates: &Rates) -> Condition {
        let mut slow = 0u32;
        let mut shown = Vec::new();
        let mut note = |status: Status, penalty: u32| {
            shown.push(status);
            slow += penalty;
        };
        match self.level(Need::Hunger) {
            Level::Failed => note(Status::Starving, 20),
            Level::Urgent => note(Status::Hungry, 10),
            _ => {}
        }
        match self.level(Need::Thirst) {
            Level::Failed => note(Status::Parched, 25),
            Level::Urgent => note(Status::Thirsty, 15),
            _ => {}
        }
        if self.level(Need::Fatigue) >= Level::Urgent {
            note(Status::Exhausted, 15);
        }
        if self.level(Need::Bladder) >= Level::Urgent || self.level(Need::Bowel) >= Level::Urgent {
            note(Status::Bursting, 5);
        }
        if self.soiled {
            note(Status::Soiled, 0);
        } else if self.level(Need::Hygiene) >= Level::Urgent {
            note(Status::Filthy, 0);
        }
        match self.temperature(rates) {
            Temperature::Hypothermic => note(Status::Hypothermic, 35),
            Temperature::Freezing => note(Status::Freezing, 25),
            Temperature::Cold => note(Status::Cold, 10),
            Temperature::Hot => note(Status::Hot, 10),
            Temperature::Heatstroke => note(Status::Heatstroke, 25),
            Temperature::Comfortable => {}
        }
        let drunk = self.drunkenness(rates);
        match drunk {
            Drunk::Tipsy => note(Status::Tipsy, 5),
            Drunk::Drunk => note(Status::Drunk, 15),
            Drunk::Wasted | Drunk::Blackout => note(Status::Wasted, 30),
            Drunk::Sober => {}
        }
        if self.passed_out {
            note(Status::PassedOut, 0);
        }
        Condition {
            speed: 100u32.saturating_sub(slow).max(40) as u8,
            wobble: drunk as u8,
            passed_out: self.passed_out,
            statuses: shown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Drunk {
    Sober,
    Tipsy,
    Drunk,
    Wasted,
    Blackout,
}

impl Drunk {
    fn extra_fatigue(self, rates: &Rates) -> u32 {
        match self {
            Drunk::Sober | Drunk::Tipsy => 0,
            Drunk::Drunk => rates.drunk.drowsy,
            Drunk::Wasted | Drunk::Blackout => rates.drunk.drowsy * 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Temperature {
    Hypothermic,
    Freezing,
    Cold,
    Comfortable,
    Hot,
    Heatstroke,
}

/// How a body state shows (for the HUD) and what it costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Status {
    Hungry,
    Starving,
    Thirsty,
    Parched,
    Exhausted,
    Bursting,
    Filthy,
    Soiled,
    Cold,
    Freezing,
    Hypothermic,
    Hot,
    Heatstroke,
    Tipsy,
    Drunk,
    Wasted,
    PassedOut,
}

impl Status {
    pub const ALL: [Status; 17] = [
        Status::Hungry,
        Status::Starving,
        Status::Thirsty,
        Status::Parched,
        Status::Exhausted,
        Status::Bursting,
        Status::Filthy,
        Status::Soiled,
        Status::Cold,
        Status::Freezing,
        Status::Hypothermic,
        Status::Hot,
        Status::Heatstroke,
        Status::Tipsy,
        Status::Drunk,
        Status::Wasted,
        Status::PassedOut,
    ];

    /// String-table key: `status.<name>`.
    pub fn key(self) -> &'static str {
        match self {
            Status::Hungry => "status.hungry",
            Status::Starving => "status.starving",
            Status::Thirsty => "status.thirsty",
            Status::Parched => "status.parched",
            Status::Exhausted => "status.exhausted",
            Status::Bursting => "status.bursting",
            Status::Filthy => "status.filthy",
            Status::Soiled => "status.soiled",
            Status::Cold => "status.cold",
            Status::Freezing => "status.freezing",
            Status::Hypothermic => "status.hypothermic",
            Status::Hot => "status.hot",
            Status::Heatstroke => "status.heatstroke",
            Status::Tipsy => "status.tipsy",
            Status::Drunk => "status.drunk",
            Status::Wasted => "status.wasted",
            Status::PassedOut => "status.passed_out",
        }
    }
}

/// What a body's state does to its character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    /// Movement speed, percent of normal (40 to 100).
    pub speed: u8,
    /// Drunken stagger, 0 (steady) to 4.
    pub wobble: u8,
    pub passed_out: bool,
    pub statuses: Vec<Status>,
}
