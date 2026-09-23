//! The title screen (docs/PLAN.md §22): the game's name, and what a player can do before the
//! world starts — carry on, begin a new game, open one of the games they have going, change a
//! setting, or leave.
//!
//! The screen is only the choosing; the player app does what is chosen.

use std::path::PathBuf;

use dark_assets::Localization;

use crate::saves::{self, Slot};

/// What the player picked, for the game to act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Chosen {
    /// Nothing yet: the screen is still up.
    Waiting,
    /// Play, carrying this game on, or starting a new one in `save`.
    Play {
        save: PathBuf,
        fresh: bool,
    },
    /// Change to the next language the project has.
    Language,
    Quit,
}

/// Which list is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root,
    Games,
    Settings,
}

/// The title screen, and where the player is in it.
pub struct Title {
    page: Page,
    picked: usize,
    saves: Vec<Slot>,
    /// A new game's file, made when one is started.
    project: dark_assets::Project,
}

impl Title {
    pub fn new(project: dark_assets::Project) -> Self {
        Self {
            page: Page::Root,
            picked: 0,
            saves: saves::list(&project),
            project,
        }
    }

    /// The game's name, shown above the list.
    pub fn heading(&self, strings: &Localization) -> String {
        match self.page {
            Page::Root => self.project.settings.name.clone(),
            Page::Games => strings.text("ui.games").to_owned(),
            Page::Settings => strings.text("ui.settings").to_owned(),
        }
    }

    /// What can be chosen here, in order.
    pub fn items(&self, strings: &Localization) -> Vec<String> {
        let text = |key: &str| strings.text(key).to_owned();
        match self.page {
            Page::Root => {
                let mut items = Vec::new();
                if let Some(newest) = self.saves.first() {
                    items.push(format!(
                        "{}  ({}, {})",
                        text("ui.carry_on"),
                        newest.name,
                        text("ui.day").replace("{day}", &newest.day.to_string())
                    ));
                }
                items.push(text("ui.new_game"));
                if !self.saves.is_empty() {
                    items.push(text("ui.games"));
                }
                items.push(text("ui.settings"));
                items.push(text("ui.quit"));
                items
            }
            Page::Games => self
                .saves
                .iter()
                .map(|slot| {
                    format!(
                        "{}  ({})",
                        slot.name,
                        text("ui.day").replace("{day}", &slot.day.to_string())
                    )
                })
                .chain(std::iter::once(text("ui.back")))
                .collect(),
            Page::Settings => vec![
                format!("{}  {}", text("ui.language_setting"), text("ui.language")),
                text("ui.back"),
            ],
        }
    }

    pub fn picked(&self) -> usize {
        self.picked
    }

    /// Moves the choosing up or down, stopping at each end.
    pub fn move_by(&mut self, delta: i32, strings: &Localization) {
        let last = self.items(strings).len().saturating_sub(1);
        self.picked = (self.picked as i32 + delta).clamp(0, last as i32) as usize;
    }

    /// Takes what is picked. Pages that only lead somewhere else handle themselves.
    pub fn choose(&mut self, strings: &Localization) -> Chosen {
        let items = self.items(strings);
        let picked = self.picked.min(items.len().saturating_sub(1));
        match self.page {
            Page::Root => {
                // The list is shorter without games to carry on or open, so it is counted from
                // the end: quit last, then settings.
                let from_end = items.len() - 1 - picked;
                match (picked, from_end) {
                    (_, 0) => Chosen::Quit,
                    (_, 1) => {
                        self.page = Page::Settings;
                        self.picked = 0;
                        Chosen::Waiting
                    }
                    (0, _) if !self.saves.is_empty() => Chosen::Play {
                        save: self.saves[0].path.clone(),
                        fresh: false,
                    },
                    (_, 2) if !self.saves.is_empty() => {
                        self.page = Page::Games;
                        self.picked = 0;
                        Chosen::Waiting
                    }
                    _ => Chosen::Play {
                        save: saves::fresh(&self.project),
                        fresh: true,
                    },
                }
            }
            Page::Games => match self.saves.get(picked) {
                Some(slot) => Chosen::Play {
                    save: slot.path.clone(),
                    fresh: false,
                },
                None => {
                    self.back();
                    Chosen::Waiting
                }
            },
            Page::Settings => match picked {
                0 => Chosen::Language,
                _ => {
                    self.back();
                    Chosen::Waiting
                }
            },
        }
    }

    /// Back to the first list; on it, back does nothing (there is nowhere behind the title).
    pub fn back(&mut self) {
        if self.page != Page::Root {
            self.page = Page::Root;
            self.picked = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> dark_assets::Project {
        let dir = std::env::temp_dir().join(format!("dark-title-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("saves")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "Adventurer", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        dark_assets::Project::open(dir).unwrap()
    }

    fn strings() -> Localization {
        Localization::default()
    }

    #[test]
    fn with_no_games_going_the_list_is_new_game_settings_and_quit() {
        let project = project("empty");
        let mut title = Title::new(project);
        let items = title.items(&strings());
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(title.choose(&strings()), {
            let fresh = saves::fresh(&title.project);
            Chosen::Play {
                save: fresh,
                fresh: true,
            }
        });
        // The last is always leaving, whatever is above it.
        title.move_by(9, &strings());
        assert_eq!(title.choose(&strings()), Chosen::Quit);
        std::fs::remove_dir_all(title.project.path("")).unwrap();
    }

    #[test]
    fn a_game_to_carry_on_comes_first_and_opens_the_newest() {
        let project = project("saves");
        // A small world: one village, one hero, one villain for them to be after.
        let def = dark_sim::WorldDef::parse(
            r#"(regions: [(id: "here", name: "Here", kind: Village, danger: 0)], roads: [],
                factions: [(id: "folk", name: "Folk"), (id: "foes", name: "Foes", hostile: true)],
                titles: [(id: "hero", name: "Hero")],
                actors: [
                    (id: "hero", name: "Hero", role: "warrior", faction: "folk", power: 10,
                        home: "here", titles: ["hero"]),
                    (id: "foe", name: "Foe", role: "lord", faction: "foes", power: 20,
                        home: "here", boss: true),
                ],
                hero_party: (leader_title: "hero", roles: ["warrior"], members: ["hero"],
                    goal: "foe"))"#,
        )
        .unwrap();
        let world = dark_world::WorldSave::new(dark_sim::WorldSim::new(&def, 1).unwrap());
        std::fs::write(project.path("saves/game.sav"), world.to_ron().unwrap()).unwrap();
        let mut title = Title::new(project);
        let items = title.items(&strings());
        assert_eq!(items.len(), 5, "carry on, new, games, settings, quit");
        match title.choose(&strings()) {
            Chosen::Play { save, fresh } => {
                assert!(save.ends_with("game.sav"));
                assert!(!fresh, "carrying on, not starting over");
            }
            other => panic!("{other:?}"),
        }
        // Settings is second from the end, wherever the list has grown.
        title.move_by(3, &strings());
        assert_eq!(title.choose(&strings()), Chosen::Waiting);
        assert_eq!(title.choose(&strings()), Chosen::Language);
        std::fs::remove_dir_all(title.project.path("")).unwrap();
    }
}
