//! The title screen (docs/PLAN.md §22): the game's name, and what a player can do before the
//! world starts — carry on, begin a new game, open one of the games they have going, play
//! together with other people (§23), change a setting, or leave.
//!
//! The screen is only the choosing; the player app does what is chosen.

use std::net::SocketAddr;
use std::path::PathBuf;

use dark_assets::Localization;
use dark_net::Status;

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
        /// Let other people join, on this port; alone when it is none.
        open: Option<u16>,
    },
    /// Play in someone else's game.
    Join(SocketAddr),
    /// Ask the network again who is playing.
    Look,
    /// Change to the next language the project has.
    Language,
    /// A tenth louder or quieter.
    Volume(i8),
    /// Windowed or the whole screen.
    Fullscreen,
    Quit,
}

/// Which list is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root,
    Games,
    /// Games on this network to join, and the one this player can open.
    Together,
    Settings,
}

/// A line of the first list. The list is shorter for a player with no games going, so what each
/// line does is carried with it rather than counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    CarryOn,
    NewGame,
    Games,
    Together,
    Settings,
    Quit,
}

/// The port a game opened from the title screen is played on, and the one looked for.
pub const TOGETHER_PORT: u16 = 7777;

/// The line the games found on the network start at: below opening one of this player's own.
const GAMES_FROM: usize = 1;

/// A row of blocks for how loud the game is, so the setting reads at a glance.
fn loudness_bar(volume: u8) -> String {
    let full = usize::from(volume.min(crate::settings::LOUDEST));
    let empty = usize::from(crate::settings::LOUDEST) - full;
    format!("{}{}", "=".repeat(full), ".".repeat(empty))
}

/// The title screen, and where the player is in it.
pub struct Title {
    page: Page,
    picked: usize,
    saves: Vec<Slot>,
    /// A new game's file, made when one is started.
    project: dark_assets::Project,
    /// What the settings page shows; the game keeps and acts on them.
    settings: crate::settings::Settings,
    /// Games found on the network, as the game last heard them.
    games: Vec<(SocketAddr, Status)>,
    /// A line under the list: what went wrong, when something did, by the string it reads.
    note: Option<&'static str>,
}

/// What the choosing is on, rather than which line it is: the list moves under the player as
/// games open and close, and the choosing should stay on the thing, not the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    /// Opening a game of one's own, the line above the rest.
    Opening,
    Game(SocketAddr),
    /// Asking the network again, and going back: the two lines that end the list.
    Looking,
    Leaving,
}

impl Title {
    pub fn new(project: dark_assets::Project, settings: crate::settings::Settings) -> Self {
        Self {
            page: Page::Root,
            picked: 0,
            saves: saves::list(&project),
            project,
            settings,
            games: Vec::new(),
            note: None,
        }
    }

    /// Says what went wrong, under the list, until the player goes somewhere else. The string
    /// is named, not taken: the language can change while it is up.
    pub fn says(&mut self, note: &'static str) {
        self.note = Some(note);
    }

    pub fn note(&self) -> Option<&'static str> {
        self.note
    }

    /// The settings shown, after the game has changed them.
    pub fn shows(&mut self, settings: crate::settings::Settings) {
        self.settings = settings;
    }

    /// Whether the player is looking at the games on the network, so the game keeps listening.
    pub fn looking(&self) -> bool {
        self.page == Page::Together
    }

    /// The games heard from since the last frame. A game opening or closing moves every line
    /// below it, so the choosing is put back on whatever it was on, not on the row it was in;
    /// a game that has gone leaves it on the line below the games, where a press is harmless.
    pub fn sees(&mut self, games: Vec<(SocketAddr, Status)>) {
        let was = self.on();
        self.games = games;
        if let Some(was) = was {
            self.picked = self.line_of(was);
        }
    }

    /// What the choosing is on, while the games are showing.
    fn on(&self) -> Option<On> {
        if self.page != Page::Together {
            return None;
        }
        let last = self.lines() - 1;
        Some(match self.picked {
            0 => On::Opening,
            line if line >= last => On::Leaving,
            line if line == last - 1 => On::Looking,
            line => match self.games.get(line - GAMES_FROM) {
                Some((at, _)) => On::Game(*at),
                None => On::Looking,
            },
        })
    }

    /// Which line that is now.
    fn line_of(&self, what: On) -> usize {
        let looking = self.games.len() + GAMES_FROM;
        match what {
            On::Opening => 0,
            On::Looking => looking,
            On::Leaving => looking + 1,
            // The game is gone: the line below the games holds nothing that joins anyone.
            On::Game(at) => match self.games.iter().position(|(now, _)| *now == at) {
                Some(nth) => nth + GAMES_FROM,
                None => looking,
            },
        }
    }

    /// How many lines the list showing has, without laying any of them out.
    fn lines(&self) -> usize {
        match self.page {
            Page::Root => self.rows().len(),
            // Opening a game, a line each, looking again, and back.
            Page::Together => self.games.len() + GAMES_FROM + 2,
            Page::Games => self.saves.len() + 1,
            Page::Settings => 4,
        }
    }

    /// Opens on the games list, as `--together` asks for a picture of it.
    pub fn look_together(&mut self) {
        self.open(Page::Together);
    }

    /// The game's name, shown above the list.
    pub fn heading(&self, strings: &Localization) -> String {
        match self.page {
            Page::Root => self.project.settings.name.clone(),
            Page::Games => strings.text("ui.games").to_owned(),
            Page::Together => strings.text("ui.together").to_owned(),
            Page::Settings => strings.text("ui.settings").to_owned(),
        }
    }

    /// The first list's lines: carrying on and opening a game only when there is one.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if !self.saves.is_empty() {
            rows.push(Row::CarryOn);
        }
        rows.push(Row::NewGame);
        if !self.saves.is_empty() {
            rows.push(Row::Games);
        }
        rows.extend([Row::Together, Row::Settings, Row::Quit]);
        rows
    }

    /// What can be chosen here, in order.
    pub fn items(&self, strings: &Localization) -> Vec<String> {
        let text = |key: &str| strings.text(key).to_owned();
        let day = |day: u32| text("ui.day").replace("{day}", &day.to_string());
        match self.page {
            Page::Root => self
                .rows()
                .into_iter()
                .map(|row| match row {
                    Row::CarryOn => match self.saves.first() {
                        Some(newest) => format!(
                            "{}  ({}, {})",
                            text("ui.carry_on"),
                            newest.name,
                            day(newest.day)
                        ),
                        None => text("ui.carry_on"),
                    },
                    Row::NewGame => text("ui.new_game"),
                    Row::Games => text("ui.games"),
                    Row::Together => text("ui.together"),
                    Row::Settings => text("ui.settings"),
                    Row::Quit => text("ui.quit"),
                })
                .collect(),
            // Opening a game of one's own, then whoever else is playing: each with how many are
            // in it out of how many it holds, `1/4` until it is `4/4` and closed.
            Page::Together => std::iter::once(text("ui.host_game"))
                .chain(self.games.iter().map(|(_, status)| {
                    // Why a game cannot be joined, when it cannot: it is full, or it is a
                    // different build of the game and the two could not play together.
                    let why = match (status.understood(), status.joinable()) {
                        (_, true) => String::new(),
                        (true, false) => format!("  {}", text("ui.full")),
                        (false, _) => format!("  {}", text("ui.other_version")),
                    };
                    format!("{}  {}/{}{why}", status.name, status.players, status.most)
                }))
                .chain(std::iter::once(text(if self.games.is_empty() {
                    "ui.looking"
                } else {
                    "ui.look_again"
                })))
                .chain(std::iter::once(text("ui.back")))
                .collect(),
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
                format!(
                    "{}  {}",
                    text("ui.volume"),
                    loudness_bar(self.settings.volume)
                ),
                format!(
                    "{}  {}",
                    text("ui.fullscreen"),
                    text(if self.settings.fullscreen {
                        "ui.on"
                    } else {
                        "ui.off"
                    })
                ),
                text("ui.back"),
            ],
        }
    }

    pub fn picked(&self) -> usize {
        self.picked
    }

    /// Moves the choosing up or down, stopping at each end.
    pub fn move_by(&mut self, delta: i32) {
        let last = self.lines().saturating_sub(1);
        self.picked = (self.picked as i32 + delta).clamp(0, last as i32) as usize;
    }

    /// Takes what is picked. Pages that only lead somewhere else handle themselves.
    pub fn choose(&mut self) -> Chosen {
        // The list may have grown shorter since the choosing last moved.
        self.picked = self.picked.min(self.lines().saturating_sub(1));
        let picked = self.picked;
        match self.page {
            Page::Root => match self.rows().get(picked) {
                Some(Row::Quit) => Chosen::Quit,
                Some(Row::Settings) => self.open(Page::Settings),
                Some(Row::Together) => self.open(Page::Together),
                Some(Row::Games) => self.open(Page::Games),
                Some(Row::CarryOn) => self.carry_on(None),
                // A new game, and anything a shorter list has left behind.
                _ => Chosen::Play {
                    save: saves::fresh(&self.project),
                    fresh: true,
                    open: None,
                },
            },
            // The first line opens this player's own game to others; then a game each, and the
            // two lines that end the list.
            Page::Together => match picked.checked_sub(GAMES_FROM) {
                None => self.carry_on(Some(TOGETHER_PORT)),
                Some(nth) => match self.games.get(nth) {
                    // A game nobody can join (it is full, or of another version) is shown, so
                    // the player can see it is there, but picking it does nothing.
                    Some((at, status)) if status.joinable() => Chosen::Join(*at),
                    Some(_) => Chosen::Waiting,
                    None if nth == self.games.len() => Chosen::Look,
                    None => {
                        self.back();
                        Chosen::Waiting
                    }
                },
            },
            Page::Games => match self.saves.get(picked) {
                Some(slot) => Chosen::Play {
                    save: slot.path.clone(),
                    fresh: false,
                    open: None,
                },
                None => {
                    self.back();
                    Chosen::Waiting
                }
            },
            Page::Settings => match picked {
                0 => Chosen::Language,
                1 => Chosen::Volume(1),
                2 => Chosen::Fullscreen,
                _ => {
                    self.back();
                    Chosen::Waiting
                }
            },
        }
    }

    /// Goes to another list, from the top of it. Whatever went wrong belonged to the list being
    /// left, so it is not carried along.
    fn open(&mut self, page: Page) -> Chosen {
        self.page = page;
        self.picked = 0;
        self.note = None;
        Chosen::Waiting
    }

    /// The newest game this player has going, or a new one when they have none. `open` is the
    /// port other people can join on.
    fn carry_on(&self, open: Option<u16>) -> Chosen {
        match self.saves.first() {
            Some(newest) => Chosen::Play {
                save: newest.path.clone(),
                fresh: false,
                open,
            },
            None => Chosen::Play {
                save: saves::fresh(&self.project),
                fresh: true,
                open,
            },
        }
    }

    /// Left or right on a setting that has a range: the sound, so far. Anything else is
    /// unmoved by it.
    pub fn nudge(&mut self, delta: i8) -> Chosen {
        match (self.page, self.picked) {
            (Page::Settings, 1) => Chosen::Volume(delta),
            _ => Chosen::Waiting,
        }
    }

    /// Back to the first list; on it, back does nothing (there is nowhere behind the title).
    pub fn back(&mut self) {
        if self.page != Page::Root {
            self.open(Page::Root);
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
        let mut title = Title::new(project, crate::settings::Settings::default());
        let items = title.items(&strings());
        assert_eq!(
            items.len(),
            4,
            "new game, together, settings, quit: {items:?}"
        );
        assert_eq!(title.choose(), {
            let fresh = saves::fresh(&title.project);
            Chosen::Play {
                save: fresh,
                fresh: true,
                open: None,
            }
        });
        // The last is always leaving, whatever is above it.
        title.move_by(9);
        assert_eq!(title.choose(), Chosen::Quit);
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
        let mut title = Title::new(project, crate::settings::Settings::default());
        let items = title.items(&strings());
        assert_eq!(
            items.len(),
            6,
            "carry on, new, games, together, settings, quit"
        );
        match title.choose() {
            Chosen::Play { save, fresh, open } => {
                assert!(save.ends_with("game.sav"));
                assert!(!fresh, "carrying on, not starting over");
                assert_eq!(open, None, "alone, unless the player asked for company");
            }
            other => panic!("{other:?}"),
        }
        // Settings is second from the end, wherever the list has grown.
        title.move_by(4);
        assert_eq!(title.choose(), Chosen::Waiting);
        assert_eq!(title.choose(), Chosen::Language);
        std::fs::remove_dir_all(title.project.path("")).unwrap();
    }

    fn game(name: &str, players: u8, version: u32) -> (SocketAddr, Status) {
        (
            SocketAddr::from(([10, 0, 0, players], 7777)),
            Status {
                version,
                name: name.into(),
                players,
                most: 4,
                port: 7777,
            },
        )
    }

    #[test]
    fn games_on_the_network_show_how_full_they_are_and_only_open_ones_are_joined() {
        let project = project("together");
        let mut title = Title::new(project, crate::settings::Settings::default());
        // Down to "Play together" (new game, together) and into it.
        title.move_by(1);
        assert_eq!(title.choose(), Chosen::Waiting);
        assert!(
            title.looking(),
            "the game keeps listening while this list is up"
        );
        let room = game("Ann", 1, dark_net::PROTOCOL_VERSION);
        let full = game("Bo", 4, dark_net::PROTOCOL_VERSION);
        title.sees(vec![room.clone(), full]);
        let items = title.items(&strings());
        assert!(items[1].contains("Ann  1/4"), "{items:?}");
        assert!(items[2].contains("Bo  4/4"), "{items:?}");
        assert_eq!(
            items.len(),
            5,
            "host, two games, look again, back: {items:?}"
        );
        // The first line opens a game of this player's own to others.
        match title.choose() {
            Chosen::Play { open, fresh, .. } => {
                assert_eq!(open, Some(TOGETHER_PORT));
                assert!(fresh, "no game going, so a new one");
            }
            other => panic!("{other:?}"),
        }
        title.move_by(1);
        assert_eq!(title.choose(), Chosen::Join(room.0));
        // A full game is shown but cannot be joined.
        title.move_by(1);
        assert_eq!(title.choose(), Chosen::Waiting);
        title.move_by(1);
        assert_eq!(title.choose(), Chosen::Look);
        // Every page's lines are counted the same way they are written.
        for page in [Page::Root, Page::Together, Page::Games, Page::Settings] {
            title.page = page;
            assert_eq!(title.lines(), title.items(&strings()).len(), "{page:?}");
        }
        title.page = Page::Together;
        title.move_by(1);
        assert_eq!(title.choose(), Chosen::Waiting);
        assert!(!title.looking(), "back on the first list");
        std::fs::remove_dir_all(title.project.path("")).unwrap();
    }

    /// Games come and go while the player reads the list. What is picked follows the game it
    /// was on, so nobody joins one they were not pointing at.
    #[test]
    fn the_choosing_follows_its_game_as_the_list_changes() {
        let project = project("moving");
        let mut title = Title::new(project, crate::settings::Settings::default());
        title.look_together();
        let ann = game("Ann", 1, dark_net::PROTOCOL_VERSION);
        let bo = game("Bo", 2, dark_net::PROTOCOL_VERSION);
        title.sees(vec![ann.clone(), bo.clone()]);
        // On Bo, the second game.
        title.move_by(2);
        assert_eq!(title.on(), Some(On::Game(bo.0)));
        // Ann's game closes: Bo moves up a line, and so does the choosing.
        title.sees(vec![bo.clone()]);
        assert_eq!(title.picked(), GAMES_FROM);
        assert_eq!(title.choose(), Chosen::Join(bo.0));
        // Bo's closes too, with the choosing on it: it falls to the line below the games,
        // where a press asks the network again rather than joining a stranger.
        title.sees(Vec::new());
        assert_eq!(title.on(), Some(On::Looking));
        assert_eq!(title.choose(), Chosen::Look);
        // A game appearing below the choosing does not walk it onto that game either.
        title.look_together();
        title.sees(vec![ann.clone()]);
        title.move_by(2);
        assert_eq!(title.on(), Some(On::Looking), "resting on looking again");
        title.sees(vec![ann.clone(), bo.clone()]);
        assert_eq!(title.on(), Some(On::Looking), "still, with Bo above it");
        assert_eq!(title.choose(), Chosen::Look);
        std::fs::remove_dir_all(title.project.path("")).unwrap();
    }
}
