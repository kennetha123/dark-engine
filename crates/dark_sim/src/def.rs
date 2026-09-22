//! The world as authored: `world.ron` in the game project. Names are string-table keys
//! (`locale/<code>.ron`), ids are what other entries refer to.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum DefError {
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
    #[error("world definition: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorldDef {
    pub regions: Vec<RegionDef>,
    pub roads: Vec<RoadDef>,
    pub factions: Vec<FactionDef>,
    pub titles: Vec<TitleDef>,
    pub actors: Vec<ActorDef>,
    pub hero_party: HeroPartyDef,
    /// The faction players belong to when they arrive; the first friendly one if unset.
    #[serde(default)]
    pub player_faction: Option<String>,
    #[serde(default)]
    pub tuning: Tuning,
    /// Standing, parties and loyalty.
    #[serde(default)]
    pub social: Social,
    /// Seasons and the days that matter.
    #[serde(default, skip_serializing_if = "CalendarDef::is_empty")]
    pub calendar: CalendarDef,
}

/// The year's seasons and dated events. Days count from 1 to 365.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct CalendarDef {
    /// Each from its first day to the next one's (in any order in the file): the latest runs to
    /// the year's end, and the days before the earliest belong to the latest (winter into the
    /// new year).
    #[serde(default)]
    pub seasons: Vec<SeasonDef>,
    #[serde(default)]
    pub events: Vec<EventDef>,
}

/// A season: its name (a string key) and how much warmer it is than the climate (hundredths of
/// a degree, added to every region's air; negative is colder).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SeasonDef {
    pub id: String,
    pub name: String,
    pub from_day: u32,
    #[serde(default)]
    pub warmth: i32,
}

/// Something on the calendar (a festival, a market day), from `day` for `days` days. Storylets
/// can be offered only then.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EventDef {
    pub id: String,
    pub name: String,
    pub day: u32,
    #[serde(default = "one_day")]
    pub days: u32,
}

fn one_day() -> u32 {
    1
}

impl CalendarDef {
    pub fn is_empty(&self) -> bool {
        self.seasons.is_empty() && self.events.is_empty()
    }

    /// The season of `day` (from 1): the one begun most recently, counting round the year.
    pub fn season(&self, day: u32) -> Option<&SeasonDef> {
        self.seasons
            .iter()
            .filter(|s| s.from_day <= day)
            .max_by_key(|s| s.from_day)
            .or_else(|| self.seasons.iter().max_by_key(|s| s.from_day))
    }

    /// Whether event `id` is on during `day` (from 1).
    pub fn during(&self, id: &str, day: u32) -> bool {
        self.events
            .iter()
            .any(|e| e.id == id && (e.day..e.day.saturating_add(e.days.max(1))).contains(&day))
    }

    /// How much warmer than its climate `day` is.
    pub fn warmth(&self, day: u32) -> i32 {
        self.season(day).map_or(0, |s| s.warmth)
    }

    pub fn validate(&self) -> Result<(), DefError> {
        let bad = |m: String| Err(DefError::Invalid(m));
        for (i, s) in self.seasons.iter().enumerate() {
            if !(1..=365).contains(&s.from_day) {
                return bad(format!("season {} must start on a day from 1 to 365", s.id));
            }
            if let Some(other) = self.seasons[..i].iter().find(|o| o.from_day == s.from_day) {
                return bad(format!(
                    "seasons {} and {} both start on day {}",
                    other.id, s.id, s.from_day
                ));
            }
        }
        for e in &self.events {
            if !(1..=365).contains(&e.day) || !(1..=365).contains(&e.days) {
                return bad(format!(
                    "event {} must be on a day from 1 to 365, and last 1 to 365 days",
                    e.id
                ));
            }
        }
        let ids: Vec<&String> = self
            .seasons
            .iter()
            .map(|s| &s.id)
            .chain(self.events.iter().map(|e| &e.id))
            .collect();
        if let Some(twice) = ids
            .iter()
            .enumerate()
            .find_map(|(i, id)| ids[..i].contains(id).then_some(id))
        {
            return bad(format!("{twice} is on the calendar twice"));
        }
        Ok(())
    }
}

impl WorldDef {
    pub fn load(path: &Path) -> Result<Self, DefError> {
        let text = std::fs::read_to_string(path).map_err(|source| DefError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text).map_err(|source| DefError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })
    }

    pub fn parse(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionKind {
    City,
    Town,
    Village,
    Wilds,
    Dungeon,
    Fortress,
}

impl RegionKind {
    /// Somewhere to train, recruit and buy supplies.
    pub fn is_settlement(self) -> bool {
        matches!(
            self,
            RegionKind::City | RegionKind::Town | RegionKind::Village
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegionDef {
    pub id: String,
    pub name: String,
    pub kind: RegionKind,
    /// 0 (safe) to 10: how often monsters find travellers here, and how strong they are.
    pub danger: u32,
    /// Somewhere to sleep safely and heal quickly.
    #[serde(default)]
    pub inn: bool,
}

/// A way between two regions, walked in `hours`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoadDef {
    pub between: (String, String),
    pub hours: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FactionDef {
    pub id: String,
    pub name: String,
    /// Hostile to everyone else (the demon army).
    #[serde(default)]
    pub hostile: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TitleDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub succession: Succession,
}

/// Who holds a title after its holder dies. Rules apply in order: the killer first (if
/// `to_killer` and the killer is a person, not a monster), then a successor appointed by a
/// faction, else the title falls vacant.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Succession {
    #[serde(default)]
    pub to_killer: bool,
    /// The faction picks its strongest living member with this role who holds no title.
    #[serde(default)]
    pub appointed: Option<Appointment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Appointment {
    pub faction: String,
    pub role: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActorDef {
    pub id: String,
    pub name: String,
    /// Occupation: warrior, mage, priest, commoner…
    pub role: String,
    pub faction: String,
    /// Fighting strength; parties add theirs up.
    pub power: u32,
    /// Where they start, and where a boss waits.
    pub home: String,
    #[serde(default)]
    pub titles: Vec<String>,
    /// Guards `home` and never leaves: the Demon Lord and his generals.
    #[serde(default)]
    pub boss: bool,
    /// Standing someone needs with this actor's faction before it will follow them.
    #[serde(default)]
    pub trust: i32,
}

/// The party the world follows through the year: who leads it, which roles it needs, whom
/// it must defeat and who stands in the way.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HeroPartyDef {
    /// The holder of this title leads the party.
    pub leader_title: String,
    /// One member per role; a dead member is replaced by someone of the same role.
    pub roles: Vec<String>,
    /// Members at the start of the year (the leader included).
    pub members: Vec<String>,
    /// The boss whose defeat wins the year.
    pub goal: String,
    /// Bosses worth defeating first, for strength and to weaken the goal.
    #[serde(default)]
    pub lieutenants: Vec<String>,
}

/// Balance numbers. Tune them with `dark-cli simulate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    /// Power each member gains per day spent training in a settlement.
    pub train_per_day: u32,
    /// Health (of 100) regained per hour resting at an inn, and elsewhere.
    pub rest_inn_per_hour: u32,
    pub rest_wild_per_hour: u32,
    /// Chance per hour, in tenths of a percent per point of danger, of meeting monsters.
    pub encounter_permille_per_danger: u32,
    /// Monster strength per point of danger (±50%).
    pub monster_power_per_danger: u32,
    /// After a win each member gains the enemy's power divided by this, shared.
    pub xp_divisor: u32,
    /// The party attacks a boss once its strength is this percentage of the boss's.
    pub readiness_percent: u32,
    /// Days kept in hand when deciding it is time to march on the goal regardless.
    pub deadline_margin_days: u32,
    /// How decisive strength is: the win chance is S^k / (S^k + E^k).
    pub sharpness: u32,
    /// Health (of 100) lost in a fight against an equal enemy, before randomness.
    pub wound_percent: u32,
    /// Each lieutenant defeated weakens the goal by this percentage of its power.
    pub lieutenant_weakens_percent: u32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            train_per_day: 1,
            rest_inn_per_hour: 5,
            rest_wild_per_hour: 1,
            encounter_permille_per_danger: 8,
            monster_power_per_danger: 14,
            xp_divisor: 10,
            readiness_percent: 115,
            deadline_margin_days: 12,
            sharpness: 3,
            wound_percent: 50,
            lieutenant_weakens_percent: 10,
        }
    }
}

/// How standing, parties and loyalty work. Standing and loyalty run from −1000 to 1000.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Social {
    /// Standing an actor starts with in its own faction, and with the other side (friendly
    /// factions towards the hostile, and the hostile towards everyone else).
    pub own_standing: i32,
    pub enemy_standing: i32,
    /// Killing someone costs this with their faction; a friendly victim is murder, which also
    /// costs `murder_penalty` with every other friendly faction; slaying someone of a hostile
    /// faction (a person or a monster) earns `slay_reward` with every friendly faction.
    pub kill_penalty: i32,
    pub murder_penalty: i32,
    pub slay_reward: i32,
    /// A new follower's loyalty: this plus half the leader's standing with its faction.
    pub loyalty_start: i32,
    /// Loyalty gained each day following (less a tenth of any bad standing of the leader).
    pub loyalty_per_day: i32,
    /// Below this a follower leaves; below `betray_below` it turns on the party.
    pub desert_below: i32,
    pub betray_below: i32,
    /// Standing lost with every faction in a party by betraying it.
    pub betrayal_penalty: i32,
    pub max_party: usize,
    pub max_players: usize,
}

impl Default for Social {
    fn default() -> Self {
        Self {
            own_standing: 500,
            enemy_standing: -500,
            kill_penalty: 400,
            murder_penalty: 150,
            slay_reward: 5,
            loyalty_start: 300,
            loyalty_per_day: 10,
            desert_below: 0,
            betray_below: -300,
            betrayal_penalty: 600,
            max_party: 8,
            max_players: 4,
        }
    }
}
