//! The story as authored: the project's `story.ron`. Lines and choices are string-table keys;
//! ids name world actors, factions and titles from `world.ron`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoryError {
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
    #[error("story: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct StoryDef {
    #[serde(default)]
    pub storylets: Vec<Storylet>,
    /// Tried in order when the year ends: the first whose conditions hold is the ending.
    #[serde(default)]
    pub endings: Vec<EndingDef>,
}

/// A conversation with one person of the world, offered while its conditions hold.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Storylet {
    pub id: String,
    /// The world actor it is with.
    pub with: String,
    /// When several are open, the highest is told.
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub when: Vec<Condition>,
    /// The node it begins at.
    pub start: String,
    pub nodes: BTreeMap<String, Node>,
}

/// How often a storylet can be told to one player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    /// Every time.
    #[default]
    Always,
    /// Once a day.
    Daily,
    Once,
}

/// What the person says, and what can be said back. With no choices the conversation ends there.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Node {
    pub line: String,
    #[serde(default)]
    pub then: Vec<Effect>,
    #[serde(default)]
    pub choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Choice {
    /// What the player says.
    pub says: String,
    /// Shown only while these hold.
    #[serde(default)]
    pub when: Vec<Condition>,
    #[serde(default)]
    pub then: Vec<Effect>,
    /// The node that follows; none ends the conversation.
    #[serde(default)]
    pub next: Option<String>,
}

/// Something about the world, the player, or the person they are talking to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    /// The player's standing with a faction is at least this.
    Standing {
        faction: String,
        at_least: i32,
    },
    /// The player's standing with a faction is below this.
    StandingBelow {
        faction: String,
        below: i32,
    },
    /// The player holds a title.
    Title(String),
    /// From this day on (1 is the first).
    FromDay(u32),
    Alive(String),
    Dead(String),
    Flag(String),
    NotFlag(String),
    /// How the person feels about the player is at least this.
    Affinity(i32),
    Married,
    Unmarried,
    /// The person talking is not married (to anyone).
    PersonUnmarried,
    /// The person would follow the player if asked now.
    WillFollow,
    /// The player is married to the person they are talking to.
    SpouseHere,
    /// The person follows the player.
    Follows,
    NotFollowing,
    /// The player is in the hero party.
    InHeroParty,
    NotInHeroParty,
    /// The player serves this faction.
    Faction(String),
    /// Endings: the hero party's goal fell this year, or still stands.
    GoalDefeated,
    GoalSurvived,
    Not(Box<Condition>),
}

/// What choosing (or reaching a node) does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    SetFlag(String),
    ClearFlag(String),
    /// The player's standing with a faction moves.
    Standing {
        faction: String,
        by: i32,
    },
    /// How the person feels about the player moves.
    Affinity(i32),
    /// The person follows the player (if they will).
    Follow,
    /// The player joins the hero party.
    JoinHeroParty,
    /// The player goes over to a faction.
    Defect(String),
    /// The player and the person marry.
    Marry,
    /// The screen fades to black and back (an intimate scene, never shown).
    FadeToBlack,
    /// The player is handed items.
    Give {
        item: String,
        count: u16,
    },
}

/// One way the year can end.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct EndingDef {
    pub id: String,
    /// What is shown (a string key).
    pub text: String,
    /// Player conditions hold if any player meets them.
    #[serde(default)]
    pub when: Vec<Condition>,
}

impl StoryDef {
    /// The project's story, or none if it has no `story.ron`.
    pub fn load_or_default(path: &Path) -> Result<Self, StoryError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| StoryError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let def = Self::parse(&text).map_err(|source| StoryError::Parse {
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

    /// Every node a storylet names exists, and ids are unique.
    pub fn validate(&self) -> Result<(), StoryError> {
        let bad = |m: String| Err(StoryError::Invalid(m));
        let mut ids = std::collections::BTreeSet::new();
        for s in &self.storylets {
            if !ids.insert(&s.id) {
                return bad(format!("storylet {} is defined twice", s.id));
            }
            if !s.nodes.contains_key(&s.start) {
                return bad(format!("storylet {}: no start node {}", s.id, s.start));
            }
            for (name, node) in &s.nodes {
                for c in &node.choices {
                    if let Some(next) = &c.next
                        && !s.nodes.contains_key(next)
                    {
                        return bad(format!("storylet {}: {name} leads to no node {next}", s.id));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every string key the story shows, for checking every language has them.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for s in &self.storylets {
            for node in s.nodes.values() {
                keys.push(node.line.clone());
                keys.extend(node.choices.iter().map(|c| c.says.clone()));
            }
        }
        keys.extend(self.endings.iter().map(|e| e.text.clone()));
        keys
    }
}
