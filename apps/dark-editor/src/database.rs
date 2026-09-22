//! The game's database, edited in forms: things people carry (`life.ron`), how characters fight
//! and the kinds of enemy (`combat.ron`), and the world's people, factions, regions, roads,
//! titles and hero party (`world.ron`). Names are typed as text, like villagers' lines; ids are
//! chosen once, when something is made, and are what everything else refers to.

use std::collections::HashMap;

use dark_assets::{Project, SceneDef};
use dark_combat::{AiDef, AttackDef, CombatDef, DodgeDef, EnemyDef, Moveset};
use dark_life::{Consumable, IconDef, ItemDef, ItemUse, LifeDef, Structure};
use dark_sim::WorldDef;
use dark_sim::def::{
    ActorDef, Appointment, FactionDef, RegionDef, RegionKind, RoadDef, Succession, TitleDef,
};
use dark_story::StoryDef;
use egui::{ComboBox, Ui};

use crate::catalog::{Catalog, scene_name, sheet_name};
use crate::strings::Strings;
use crate::widgets::{Tracker, tidy_id, valid_id};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Items,
    StartingKit,
    Climates,
    Movesets,
    Enemies,
    People,
    Factions,
    Regions,
    Roads,
    Titles,
    HeroParty,
    Calendar,
    Balance,
}

impl Category {
    const ALL: [(Category, &'static str, &'static str); 13] = [
        (
            Category::Items,
            "Items",
            "Things people carry: food, drink, clothes, tents…",
        ),
        (
            Category::StartingKit,
            "Starting kit",
            "What a new character carries and wears",
        ),
        (
            Category::Climates,
            "Climates",
            "How warm each region is, day and night",
        ),
        (
            Category::Movesets,
            "Movesets",
            "How characters fight: health, combo, dodge",
        ),
        (
            Category::Enemies,
            "Enemies",
            "Kinds of enemy a map can place",
        ),
        (
            Category::People,
            "People",
            "The people of the world, who live the year",
        ),
        (Category::Factions, "Factions", "Sides people belong to"),
        (
            Category::Regions,
            "Regions",
            "Places of the world; maps belong to one",
        ),
        (
            Category::Roads,
            "Roads",
            "How long it takes to walk between regions",
        ),
        (
            Category::Titles,
            "Titles",
            "Hero, Demon Lord…: who holds them and who is next",
        ),
        (
            Category::HeroParty,
            "Hero party",
            "The party the world follows through the year",
        ),
        (
            Category::Calendar,
            "Calendar",
            "The year's seasons (warmer, colder) and the days that matter",
        ),
        (
            Category::Balance,
            "Balance",
            "Tuning numbers: bodies and weather, the world's year, standing and loyalty",
        ),
    ];

    /// What the category's things are, one by one, for "New …".
    fn one(self) -> &'static str {
        match self {
            Category::Items => "item",
            Category::Movesets => "moveset",
            Category::Enemies => "enemy",
            Category::People => "person",
            Category::Factions => "faction",
            Category::Regions => "region",
            Category::Titles => "title",
            _ => "",
        }
    }

    /// The string-key prefix for a new thing's name.
    fn name_prefix(self) -> &'static str {
        match self {
            Category::Items => "item",
            Category::Enemies => "enemy",
            Category::People => "actor",
            Category::Factions => "faction",
            Category::Regions => "region",
            Category::Titles => "title",
            _ => "",
        }
    }
}

/// Things of the database other parts of the editor pick from: (id, label) pairs.
#[derive(Clone, Default)]
pub struct Lists {
    pub people: Vec<(String, String)>,
    pub factions: Vec<(String, String)>,
    pub titles: Vec<(String, String)>,
    pub items: Vec<(String, String)>,
    /// The calendar's seasons and events.
    pub seasons: Vec<(String, String)>,
    pub events: Vec<(String, String)>,
}

impl Lists {
    /// How `id` shows among `list`: its label, or the bare id if it is not there.
    pub fn label<'a>(list: &'a [(String, String)], id: &'a str) -> &'a str {
        list.iter().find(|(i, _)| i == id).map_or(id, |(_, l)| l)
    }
}

/// The three files, as one undo step's worth.
#[derive(Clone)]
struct Data {
    life: LifeDef,
    combat: CombatDef,
    world: Option<WorldDef>,
}

pub struct Database {
    data: Data,
    /// Files that could not be read, and why: they are not edited (saving would lose them).
    pub broken: Vec<String>,
    /// Which of life, combat and world could not be read.
    unreadable: [bool; 3],
    /// Which files changed since the last save: life, combat, world.
    dirty: [bool; 3],
    /// Steps to undo, each with the files it changed.
    undo: Vec<(Data, [bool; 3])>,
    redo: Vec<(Data, [bool; 3])>,
    editing: Option<egui::Id>,
    category: Category,
    selected: Option<String>,
    new_id: String,
    /// Things the selected one is used by, when asked to delete it.
    uses: Option<(String, Vec<String>)>,
    icons: HashMap<(String, u32, (u32, u32)), Option<egui::TextureHandle>>,
    /// What each thing is called in the language being written, for pickers.
    display: HashMap<(Category, String), String>,
}

/// Steps kept to undo.
const HISTORY: usize = 200;

impl Database {
    pub fn load(project: &Project) -> Self {
        let mut broken = Vec::new();
        let mut unreadable = [false; 3];
        let life =
            dark_life::LifeDef::load_or_default(&project.path("life.ron")).unwrap_or_else(|e| {
                broken.push(format!("life.ron: {e}"));
                unreadable[0] = true;
                LifeDef::default()
            });
        let combat = CombatDef::load_or_default(&project.path("combat.ron")).unwrap_or_else(|e| {
            broken.push(format!("combat.ron: {e}"));
            unreadable[1] = true;
            CombatDef::default()
        });
        let world_path = project.path("world.ron");
        let world = if world_path.exists() {
            match WorldDef::load(&world_path) {
                Ok(def) => Some(def),
                Err(e) => {
                    broken.push(format!("world.ron: {e}"));
                    unreadable[2] = true;
                    None
                }
            }
        } else {
            None
        };
        Self {
            data: Data {
                life,
                combat,
                world,
            },
            broken,
            unreadable,
            dirty: [false; 3],
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            category: Category::Items,
            selected: None,
            new_id: String::new(),
            uses: None,
            icons: HashMap::new(),
            display: HashMap::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.dirty.iter().any(|d| *d)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Records the data as it was before a change to `files`.
    fn record(&mut self, before: Data, files: [bool; 3]) {
        self.undo.push((before, files));
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.touch(files);
    }

    fn touch(&mut self, files: [bool; 3]) {
        for (dirty, changed) in self.dirty.iter_mut().zip(files) {
            *dirty |= changed;
        }
    }

    /// Only the file of the open category.
    fn this_file(&self) -> [bool; 3] {
        let mut files = [false; 3];
        files[self.file()] = true;
        files
    }

    pub fn undo_step(&mut self) {
        if let Some((before, files)) = self.undo.pop() {
            self.redo
                .push((std::mem::replace(&mut self.data, before), files));
            self.touch(files);
            self.editing = None;
        }
    }

    pub fn redo_step(&mut self) {
        if let Some((after, files)) = self.redo.pop() {
            self.undo
                .push((std::mem::replace(&mut self.data, after), files));
            self.touch(files);
            self.editing = None;
        }
    }

    /// The kinds of enemy, with their sheets (for maps).
    pub fn enemies(&self) -> Vec<(String, String)> {
        self.data
            .combat
            .enemies
            .iter()
            .map(|(id, e)| (id.clone(), e.sheet.clone()))
            .collect()
    }

    pub fn actors(&self) -> Vec<String> {
        self.data
            .world
            .iter()
            .flat_map(|w| &w.actors)
            .map(|a| a.id.clone())
            .collect()
    }

    pub fn regions(&self) -> Vec<String> {
        self.data
            .world
            .iter()
            .flat_map(|w| &w.regions)
            .map(|r| r.id.clone())
            .collect()
    }

    /// What other parts of the editor pick from, as (id, "Name (id)") in `language`.
    pub fn lists(&self, strings: &mut Strings, language: &str) -> Lists {
        let mut list = |category| -> Vec<(String, String)> {
            entries(&self.data, category)
                .into_iter()
                .map(|(id, key)| {
                    let name = key.map(|k| strings.text(&k, language)).unwrap_or_default();
                    let label = if name.is_empty() || name == id {
                        id.clone()
                    } else {
                        format!("{name} ({id})")
                    };
                    (id, label)
                })
                .collect()
        };
        let people = list(Category::People);
        let factions = list(Category::Factions);
        let titles = list(Category::Titles);
        let items = list(Category::Items);
        let calendar = self.data.world.as_ref().map(|w| &w.calendar);
        let mut named = |id: &str, key: &str| {
            let name = strings.text(key, language);
            let label = if name.is_empty() {
                id.to_owned()
            } else {
                format!("{name} ({id})")
            };
            (id.to_owned(), label)
        };
        let seasons = calendar
            .iter()
            .flat_map(|c| &c.seasons)
            .map(|s| named(&s.id, &s.name))
            .collect();
        let events = calendar
            .iter()
            .flat_map(|c| &c.events)
            .map(|e| named(&e.id, &e.name))
            .collect();
        Lists {
            people,
            factions,
            titles,
            items,
            seasons,
            events,
        }
    }

    /// Writes the files that changed, then checks them as the game will. Err if nothing could
    /// be written; Ok with a warning if the game will refuse what was written.
    pub fn save(&mut self, project: &Project) -> Result<Option<String>, String> {
        // A file that could not be read is never written: saving it would lose what it holds.
        for (dirty, unreadable) in self.dirty.iter_mut().zip(self.unreadable) {
            *dirty &= !unreadable;
        }
        // Seasons are written in order through the year, however they were typed.
        if let Some(world) = &mut self.data.world {
            world.calendar.seasons.sort_by_key(|s| s.from_day);
        }
        let header = "// Made in the editor (dark-editor).";
        let files: [(&str, Option<String>); 3] = [
            (
                "life.ron",
                self.dirty[0].then(|| to_ron(&self.data.life)).transpose()?,
            ),
            (
                "combat.ron",
                self.dirty[1]
                    .then(|| to_ron(&self.data.combat))
                    .transpose()?,
            ),
            (
                "world.ron",
                match &self.data.world {
                    Some(world) if self.dirty[2] => Some(to_ron(world)?),
                    _ => None,
                },
            ),
        ];
        for (name, text) in files {
            if let Some(text) = text {
                std::fs::write(project.path(name), format!("{header}\n{text}\n"))
                    .map_err(|e| format!("{name}: {e}"))?;
            }
        }
        self.dirty = [false; 3];
        Ok(self.check().err())
    }

    /// The game's own checks, and whether the files agree with each other.
    pub fn check(&self) -> Result<(), String> {
        let d = &self.data;
        d.life.validate().map_err(|e| e.to_string())?;
        d.combat.validate().map_err(|e| e.to_string())?;
        if let Some(world) = &d.world {
            dark_sim::WorldSim::new(world, 1).map_err(|e| e.to_string())?;
            for region in d.life.climates.keys() {
                if !world.regions.iter().any(|r| &r.id == region) {
                    return Err(format!("a climate is set for unknown region {region}"));
                }
            }
        }
        Ok(())
    }

    // --- The interface ---------------------------------------------------------------------

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        catalog: &Catalog,
        strings: &mut Strings,
        language: &str,
        elsewhere: &Elsewhere,
    ) {
        egui::Panel::left("db categories")
            .resizable(false)
            .exact_size(170.0)
            .show(ui, |ui| {
                ui.heading("Database");
                for (category, label, help) in Category::ALL {
                    let chosen = self.category == category;
                    if ui
                        .selectable_label(chosen, label)
                        .on_hover_text(help)
                        .clicked()
                        && !chosen
                    {
                        self.category = category;
                        self.selected = None;
                        self.uses = None;
                        self.editing = None;
                    }
                }
                if !self.broken.is_empty() {
                    ui.separator();
                    for problem in &self.broken {
                        ui.colored_label(egui::Color32::from_rgb(255, 110, 100), problem);
                    }
                }
            });
        let listed = matches!(
            self.category,
            Category::Items
                | Category::Movesets
                | Category::Enemies
                | Category::People
                | Category::Factions
                | Category::Regions
                | Category::Titles
        );
        if listed {
            egui::Panel::left("db list")
                .resizable(true)
                .default_size(220.0)
                .min_size(180.0)
                .show(ui, |ui| self.list(ui, strings, language));
        }
        self.display = self.names(strings, language);
        egui::CentralPanel::default().show(ui, |ui| {
            // Balance's numbers are in life.ron and world.ron both.
            let balance =
                self.category == Category::Balance && (self.unreadable[0] || self.unreadable[2]);
            if self.unreadable[self.file()] || balance {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 110, 100),
                    "This part of the database could not be read, so it cannot be changed here \
                     until the file is fixed (see the left).",
                );
                return;
            }
            if self.data.world.is_none()
                && matches!(
                    self.category,
                    Category::People
                        | Category::Factions
                        | Category::Regions
                        | Category::Roads
                        | Category::Titles
                        | Category::HeroParty
                        | Category::Calendar
                )
            {
                ui.label("This project has no world (world.ron).");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                let before = self.data.clone();
                let mut form = Tracker::default();
                let mut names = Names {
                    strings,
                    language,
                    changed: false,
                };
                self.form(ui, &mut form, &mut names, project, catalog, elsewhere);
                let text_changed = names.changed;
                if let Some(id) = form.changed {
                    // The files this change touched (Balance's numbers live in two).
                    let mut files = changed_files(&before, &self.data);
                    if files == [false; 3] {
                        files = self.this_file();
                    }
                    if self.editing != Some(id) || form.step {
                        self.record(before, files);
                    } else {
                        self.touch(files);
                    }
                    self.editing = (!form.step).then_some(id);
                }
                if text_changed {
                    // Text is saved with everything else; nothing in the files changed.
                    self.editing = None;
                }
            });
        });
    }

    /// Which file the open category lives in.
    fn file(&self) -> usize {
        match self.category {
            Category::Items | Category::StartingKit | Category::Climates => 0,
            Category::Movesets | Category::Enemies => 1,
            _ => 2,
        }
    }

    /// The ids in the open category, in order, with their names' keys.
    fn entries(&self) -> Vec<(String, Option<String>)> {
        entries(&self.data, self.category)
    }

    /// What everything that pickers offer is called, in `language`.
    fn names(&self, strings: &mut Strings, language: &str) -> HashMap<(Category, String), String> {
        let mut names = HashMap::new();
        for category in [
            Category::Items,
            Category::People,
            Category::Factions,
            Category::Regions,
            Category::Titles,
        ] {
            for (id, key) in entries(&self.data, category) {
                if let Some(key) = key {
                    names.insert((category, id), strings.text(&key, language));
                }
            }
        }
        names
    }

    fn list(&mut self, ui: &mut Ui, strings: &mut Strings, language: &str) {
        let one = self.category.one();
        ui.horizontal(|ui| {
            ui.label(format!("New {one}:"));
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.new_id)
                    .hint_text("id, e.g. old_bridge")
                    .desired_width(110.0),
            );
            tidy_id(&mut self.new_id);
            let taken = self.entries().iter().any(|(id, _)| *id == self.new_id);
            let ok = valid_id(&self.new_id) && !taken;
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let has_file = self.data.world.is_some() || self.file() != 2;
            if (ui.add_enabled(ok, egui::Button::new("Add")).clicked() || (enter && ok)) && has_file
            {
                let id = std::mem::take(&mut self.new_id);
                let before = self.data.clone();
                self.add(&id, strings, language);
                self.record(before, self.this_file());
                self.editing = None;
                self.selected = Some(id);
                self.uses = None;
            }
            if taken {
                ui.weak("taken");
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (id, name) in self.entries() {
                let label = match &name {
                    Some(key) => {
                        let text = strings.text(key, language);
                        if text.is_empty() {
                            id.clone()
                        } else {
                            format!("{text}  ({id})")
                        }
                    }
                    None => id.clone(),
                };
                let chosen = self.selected.as_ref() == Some(&id);
                if ui.selectable_label(chosen, label).clicked() {
                    self.selected = Some(id);
                    self.uses = None;
                    self.editing = None;
                }
            }
        });
    }

    /// Makes a new thing called `id` in the open category, with sensible starting values.
    fn add(&mut self, id: &str, strings: &mut Strings, language: &str) {
        let prefix = self.category.name_prefix();
        let key = if prefix.is_empty() {
            String::new()
        } else {
            // `item.stew`, unless that key has text in any language or names something.
            let used: std::collections::HashSet<String> = Category::ALL
                .iter()
                .flat_map(|(c, ..)| entries(&self.data, *c))
                .filter_map(|(_, key)| key)
                .collect();
            let wanted = format!("{prefix}.{id}");
            let key = if strings.is_free(&wanted) && !used.contains(&wanted) {
                wanted
            } else {
                strings.fresh_key(&wanted, |k| used.contains(k))
            };
            // Named after its id until someone types a name.
            strings.set(&key, language, &id.replace('_', " "));
            key
        };
        let d = &mut self.data;
        let first_region = d
            .world
            .as_ref()
            .and_then(|w| w.regions.first())
            .map(|r| r.id.clone())
            .unwrap_or_default();
        let first_faction = d
            .world
            .as_ref()
            .and_then(|w| w.factions.first())
            .map(|f| f.id.clone())
            .unwrap_or_default();
        match self.category {
            Category::Items => {
                d.life.items.insert(
                    id.to_owned(),
                    ItemDef {
                        name: key,
                        icon: None,
                        use_: ItemUse::Consume(Consumable::default()),
                    },
                );
            }
            Category::Movesets => {
                d.combat.movesets.insert(id.to_owned(), Moveset::default());
            }
            Category::Enemies => {
                let moveset = d.combat.movesets.keys().next().cloned().unwrap_or_default();
                let sheet = d
                    .combat
                    .enemies
                    .values()
                    .next()
                    .map(|e| e.sheet.clone())
                    .unwrap_or_default();
                d.combat.enemies.insert(
                    id.to_owned(),
                    EnemyDef {
                        name: key,
                        sheet,
                        attack: None,
                        moveset,
                        ai: AiDef {
                            sight: 120.0,
                            leash: 240.0,
                            attack_range: 20.0,
                            speed: 0.8,
                            back_off: 30,
                            respawn: 60 * 30,
                        },
                    },
                );
            }
            Category::People => {
                if let Some(w) = &mut d.world {
                    w.actors.push(ActorDef {
                        id: id.to_owned(),
                        name: key,
                        role: "commoner".into(),
                        faction: first_faction,
                        power: 10,
                        home: first_region,
                        titles: Vec::new(),
                        boss: false,
                        trust: 0,
                    });
                }
            }
            Category::Factions => {
                if let Some(w) = &mut d.world {
                    w.factions.push(FactionDef {
                        id: id.to_owned(),
                        name: key,
                        hostile: false,
                    });
                }
            }
            Category::Regions => {
                if let Some(w) = &mut d.world {
                    w.regions.push(RegionDef {
                        id: id.to_owned(),
                        name: key,
                        kind: RegionKind::Village,
                        danger: 1,
                        inn: false,
                    });
                }
            }
            Category::Titles => {
                if let Some(w) = &mut d.world {
                    w.titles.push(TitleDef {
                        id: id.to_owned(),
                        name: key,
                        succession: Succession::default(),
                    });
                }
            }
            _ => {}
        }
    }

    /// What refers to `id` in the open category: it cannot go while anything does.
    fn used_by(
        &self,
        id: &str,
        project: &Project,
        catalog: &Catalog,
        elsewhere: &Elsewhere,
    ) -> Vec<String> {
        let d = &self.data;
        let mut uses = Vec::new();
        // The maps as they are now: the open one as edited, the rest as saved.
        let scenes: Vec<(String, SceneDef)> = catalog
            .scenes
            .iter()
            .filter_map(|s| match elsewhere.scene {
                Some((open, def)) if open == s => Some((s.clone(), def.clone())),
                _ => Some((s.clone(), project.load_scene(s).ok()?)),
            })
            .collect();
        if let Some(what) = story_uses(elsewhere.story, self.category, id) {
            uses.push(format!("the story ({what})"));
        }
        let world = d.world.as_ref();
        match self.category {
            Category::Items if d.life.start.iter().any(|(i, _)| i == id) => {
                uses.push("the starting kit".into());
            }
            Category::Movesets => {
                for (e, def) in &d.combat.enemies {
                    if def.moveset == id {
                        uses.push(format!("enemy {e}"));
                    }
                }
                for (path, scene) in &scenes {
                    let looks = scene.npcs.iter().filter_map(|n| n.moveset.as_deref());
                    let player = scene.player.iter().filter_map(|p| p.moveset.as_deref());
                    if looks.chain(player).any(|m| m == id) {
                        uses.push(format!("map {}", scene_name(path)));
                    }
                }
            }
            Category::Enemies => {
                for (path, scene) in &scenes {
                    if scene.enemies.iter().any(|e| e.kind == id) {
                        uses.push(format!("map {}", scene_name(path)));
                    }
                }
            }
            Category::People => {
                for (path, scene) in &scenes {
                    if scene.npcs.iter().any(|n| n.actor.as_deref() == Some(id)) {
                        uses.push(format!("map {}", scene_name(path)));
                    }
                }
                if let Some(w) = world {
                    let party = &w.hero_party;
                    if party
                        .members
                        .iter()
                        .chain(&party.lieutenants)
                        .any(|m| m == id)
                        || party.goal == id
                    {
                        uses.push("the hero party".into());
                    }
                }
            }
            Category::Factions => {
                if let Some(w) = world {
                    for a in w.actors.iter().filter(|a| a.faction == id) {
                        uses.push(format!("person {}", a.id));
                    }
                    for t in &w.titles {
                        if t.succession
                            .appointed
                            .as_ref()
                            .is_some_and(|a| a.faction == id)
                        {
                            uses.push(format!("title {}", t.id));
                        }
                    }
                    if w.player_faction.as_deref() == Some(id) {
                        uses.push("the players' faction".into());
                    }
                }
            }
            Category::Regions => {
                if let Some(w) = world {
                    for a in w.actors.iter().filter(|a| a.home == id) {
                        uses.push(format!("person {}", a.id));
                    }
                    if w.roads
                        .iter()
                        .any(|r| r.between.0 == id || r.between.1 == id)
                    {
                        uses.push("a road".into());
                    }
                }
                if d.life.climates.contains_key(id) {
                    uses.push("a climate".into());
                }
                for (path, scene) in &scenes {
                    if scene.region.as_deref() == Some(id) {
                        uses.push(format!("map {}", scene_name(path)));
                    }
                }
            }
            Category::Titles => {
                if let Some(w) = world {
                    for a in w.actors.iter().filter(|a| a.titles.iter().any(|t| t == id)) {
                        uses.push(format!("person {}", a.id));
                    }
                    if w.hero_party.leader_title == id {
                        uses.push("the hero party".into());
                    }
                }
            }
            _ => {}
        }
        uses
    }

    fn remove(&mut self, id: &str) {
        let d = &mut self.data;
        match self.category {
            Category::Items => {
                d.life.items.remove(id);
            }
            Category::Movesets => {
                d.combat.movesets.remove(id);
            }
            Category::Enemies => {
                d.combat.enemies.remove(id);
            }
            _ => {
                if let Some(w) = &mut d.world {
                    match self.category {
                        Category::People => w.actors.retain(|a| a.id != id),
                        Category::Factions => w.factions.retain(|f| f.id != id),
                        Category::Regions => w.regions.retain(|r| r.id != id),
                        Category::Titles => w.titles.retain(|t| t.id != id),
                        _ => {}
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn form(
        &mut self,
        ui: &mut Ui,
        f: &mut Tracker,
        names: &mut Names,
        project: &Project,
        catalog: &Catalog,
        elsewhere: &Elsewhere,
    ) {
        match self.category {
            Category::StartingKit => return self.starting_kit(ui, f),
            Category::Climates => return self.climates(ui, f),
            Category::Roads => return self.roads(ui, f, names),
            Category::HeroParty => return self.hero_party(ui, f, names),
            Category::Balance => return self.balance(ui, f),
            Category::Calendar => return self.calendar(ui, f, names, elsewhere),
            _ => {}
        }
        // Undoing an Add can leave the selection pointing at nothing.
        if let Some(id) = &self.selected
            && !self.entries().iter().any(|(e, _)| e == id)
        {
            self.selected = None;
        }
        let Some(id) = self.selected.clone() else {
            ui.label(format!(
                "Pick {} on the left, or type an id and Add to make a new one.",
                match self.category.one() {
                    "item" => "an item",
                    "enemy" => "an enemy",
                    one => one,
                }
            ));
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(format!("{} {id}", capitalised(self.category.one())));
            if ui.button("Delete…").clicked() {
                self.uses = Some((id.clone(), self.used_by(&id, project, catalog, elsewhere)));
            }
        });
        if let Some((what, uses)) = self.uses.clone()
            && what == id
        {
            if uses.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Nothing uses it.");
                    if ui.button("Delete it").clicked() {
                        let before = self.data.clone();
                        self.remove(&id);
                        self.record(before, self.this_file());
                        self.editing = None;
                        self.selected = None;
                        self.uses = None;
                    }
                    if ui.button("Keep it").clicked() {
                        self.uses = None;
                    }
                });
                if self.selected.is_none() {
                    return;
                }
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 190, 90),
                    format!("It cannot go while it is used by: {}.", uses.join(", ")),
                );
                if ui.button("OK").clicked() {
                    self.uses = None;
                }
            }
        }
        ui.separator();
        match self.category {
            Category::Items => self.item(ui, f, names, &id, project),
            Category::Movesets => self.moveset(ui, f, &id),
            Category::Enemies => self.enemy(ui, f, names, &id, catalog),
            Category::People => self.person(ui, f, names, &id),
            Category::Factions => self.faction(ui, f, names, &id),
            Category::Regions => self.region(ui, f, names, &id),
            Category::Titles => self.title(ui, f, names, &id),
            _ => {}
        }
    }

    fn item(
        &mut self,
        ui: &mut Ui,
        f: &mut Tracker,
        names: &mut Names,
        id: &str,
        project: &Project,
    ) {
        let Some(item) = self.data.life.items.get_mut(id) else {
            return;
        };
        names.field(ui, "Name", &item.name);
        // The icon: an image, a cell size, and which cell.
        let mut has_icon = item.icon.is_some();
        if f.track(ui.checkbox(&mut has_icon, "Has an icon")).changed() {
            item.icon = has_icon.then(|| IconDef {
                image: "Art/Icon01.png".into(),
                size: 48,
                cell: (0, 0),
            });
        }
        if let Some(icon) = &mut item.icon {
            ui.indent("icon", |ui| {
                f.text(ui, "Image", &mut icon.image, "An icon sheet in the project");
                f.number(
                    ui,
                    "Cell size",
                    &mut icon.size,
                    4..=512,
                    "Pixels per icon cell",
                );
                ui.horizontal(|ui| {
                    ui.label("Cell");
                    f.track(ui.add(egui::DragValue::new(&mut icon.cell.0).prefix("column ")));
                    f.track(ui.add(egui::DragValue::new(&mut icon.cell.1).prefix("row ")));
                });
                let key = (icon.image.clone(), icon.size, icon.cell);
                let texture = self.icons.entry(key).or_insert_with(|| {
                    project
                        .load_icon(&icon.image, icon.size, icon.cell, 48)
                        .ok()
                        .map(|image| {
                            ui.ctx().load_texture(
                                format!("icon {} {:?}", icon.image, icon.cell),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [image.width as usize, image.height as usize],
                                    &image.rgba,
                                ),
                                egui::TextureOptions::LINEAR,
                            )
                        })
                });
                match texture {
                    Some(t) => {
                        ui.image((t.id(), egui::vec2(48.0, 48.0)));
                    }
                    None => {
                        ui.colored_label(egui::Color32::from_rgb(255, 110, 100), "No such icon");
                    }
                }
            });
        }
        ui.separator();
        // What using it does.
        let kinds = [
            "Food or drink",
            "Clothing",
            "Warming magic",
            "Tent",
            "Campfire",
            "Soap",
        ];
        let kind = match &item.use_ {
            ItemUse::Consume(_) => 0,
            ItemUse::Wear { .. } => 1,
            ItemUse::Warm { .. } => 2,
            ItemUse::Place(Structure::Tent) => 3,
            ItemUse::Place(Structure::Campfire { .. }) => 4,
            ItemUse::Wash => 5,
        };
        let mut chosen = kind;
        ui.horizontal(|ui| {
            ui.label("Using it");
            ComboBox::from_id_salt(("use", id))
                .selected_text(kinds[kind])
                .show_ui(ui, |ui| {
                    for (i, k) in kinds.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, *k);
                    }
                });
        });
        if chosen != kind {
            item.use_ = match chosen {
                0 => ItemUse::Consume(Consumable::default()),
                1 => ItemUse::Wear { insulation: 1000 },
                2 => ItemUse::Warm {
                    bonus: 2000,
                    minutes: 60,
                },
                3 => ItemUse::Place(Structure::Tent),
                4 => ItemUse::Place(Structure::Campfire { minutes: 120 }),
                _ => ItemUse::Wash,
            };
            f.step(("use", id));
        }
        ui.indent("use", |ui| match &mut item.use_ {
            ItemUse::Consume(c) => {
                ui.weak("Needs from 0 (fine) to 1000 (failing); negative numbers relieve.");
                let r = -1000..=1000;
                f.number(ui, "Hunger", &mut c.hunger, r.clone(), "");
                f.number(ui, "Thirst", &mut c.thirst, r.clone(), "");
                f.number(ui, "Tiredness", &mut c.fatigue, r.clone(), "");
                f.number(ui, "Bladder", &mut c.bladder, r.clone(), "");
                f.number(ui, "Bowels", &mut c.bowel, r.clone(), "");
                f.number(ui, "Dirt", &mut c.hygiene, r, "");
                f.number(
                    ui,
                    "Alcohol",
                    &mut c.alcohol,
                    0..=10_000,
                    "1000 is one drink",
                );
            }
            ItemUse::Wear { insulation } => {
                degrees(ui, f, "Warmth", insulation, "Added to the air while worn");
            }
            ItemUse::Warm { bonus, minutes } => {
                degrees(ui, f, "Warmth", bonus, "Added to the air while it lasts");
                f.number(ui, "Minutes", minutes, 1..=24 * 60, "Game minutes it lasts");
            }
            ItemUse::Place(Structure::Campfire { minutes }) => {
                f.number(ui, "Burns (minutes)", minutes, 1..=24 * 60, "Game minutes");
            }
            ItemUse::Place(Structure::Tent) => {
                ui.weak("Set down to sleep in, warmer and more restful; packed up again with E.");
            }
            ItemUse::Wash => {
                ui.weak("Clean again. Used up.");
            }
        });
    }

    fn starting_kit(&mut self, ui: &mut Ui, f: &mut Tracker) {
        let display = &self.display;
        ui.heading("Starting kit");
        ui.label("What a new character carries, in hotbar order (keys 1 to 8).");
        let items: Vec<String> = self.data.life.items.keys().cloned().collect();
        let life = &mut self.data.life;
        let mut remove = None;
        let mut swap = None;
        let count = life.start.len();
        for (i, (item, n)) in life.start.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("{}", i + 1));
                f.pick(ui, ("kit", i), "", item, &items, |id| {
                    shown(display, Category::Items, id)
                });
                f.track(ui.add(egui::DragValue::new(n).range(1..=999).prefix("× ")));
                if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                    swap = Some((i - 1, i));
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("▼"))
                    .clicked()
                {
                    swap = Some((i, i + 1));
                }
                if ui.button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some((a, b)) = swap {
            life.start.swap(a, b);
            f.step(("kit swap", a));
        }
        if let Some(i) = remove {
            life.start.remove(i);
            f.step(("kit remove", i));
        }
        if ui.button("Add an item").clicked()
            && let Some(first) = items.first()
        {
            life.start.push((first.clone(), 1));
            f.step("kit add");
        }
        ui.separator();
        // What is worn must be carried, and be clothing.
        if let Some(worn) = &life.wear
            && (!life.start.iter().any(|(id, _)| id == worn)
                || !matches!(
                    life.items.get(worn).map(|i| &i.use_),
                    Some(ItemUse::Wear { .. })
                ))
        {
            life.wear = None;
            f.mark("wear gone");
        }
        let clothes: Vec<String> = life
            .start
            .iter()
            .map(|(id, _)| id.clone())
            .filter(|id| {
                matches!(
                    life.items.get(id).map(|i| &i.use_),
                    Some(ItemUse::Wear { .. })
                )
            })
            .collect();
        f.pick_optional(
            ui,
            "wear",
            "Wearing",
            &mut life.wear,
            &clothes,
            "nothing",
            |id| shown(display, Category::Items, id),
        );
    }

    fn climates(&mut self, ui: &mut Ui, f: &mut Tracker) {
        ui.heading("Climates");
        ui.label("The air, coldest at 05:00 and warmest at 17:00.");
        let regions = self.regions();
        let life = &mut self.data.life;
        ui.label("Anywhere not listed:");
        ui.indent("default climate", |ui| {
            degrees(ui, f, "Day", &mut life.climate.day, "");
            degrees(ui, f, "Night", &mut life.climate.night, "");
        });
        for region in &regions {
            let mut own = life.climates.contains_key(region);
            if f.track(ui.checkbox(&mut own, format!("{region} has its own")))
                .changed()
            {
                if own {
                    life.climates.insert(region.clone(), life.climate);
                } else {
                    life.climates.remove(region);
                }
            }
            if let Some(c) = life.climates.get_mut(region) {
                ui.indent(region, |ui| {
                    degrees(ui, f, "Day", &mut c.day, "");
                    degrees(ui, f, "Night", &mut c.night, "");
                });
            }
        }
    }

    fn moveset(&mut self, ui: &mut Ui, f: &mut Tracker, id: &str) {
        let Some(m) = self.data.combat.movesets.get_mut(id) else {
            return;
        };
        f.number(ui, "Health", &mut m.health, 1..=10_000, "");
        f.number(
            ui,
            "Poise",
            &mut m.poise,
            0..=1000,
            "Hits take poise; only a hit that empties it staggers",
        );
        f.number(
            ui,
            "Poise back after",
            &mut m.poise_recovery,
            0..=6000,
            "Ticks (60 a second) without being hit",
        );
        f.text(
            ui,
            "Hurt clip",
            &mut m.hurt_clip,
            "The animation when staggered",
        );
        f.text(
            ui,
            "Death clip",
            &mut m.death_clip,
            "The animation when dying",
        );
        ui.separator();
        ui.label("The combo: pressing attack again during or just after one goes on to the next.");
        let mut remove = None;
        for (i, a) in m.combo.iter_mut().enumerate() {
            egui::CollapsingHeader::new(format!("Attack {}  ({} damage)", i + 1, a.damage))
                .id_salt(("attack", id, i))
                .default_open(i == 0)
                .show(ui, |ui| {
                    attack(ui, f, a);
                    if ui.button("Remove this attack").clicked() {
                        remove = Some(i);
                    }
                });
        }
        if let Some(i) = remove {
            m.combo.remove(i);
            f.step(("remove attack", i));
        }
        if ui.button("Add an attack").clicked() {
            m.combo.push(m.combo.last().cloned().unwrap_or(AttackDef {
                clip: "attack".into(),
                startup: 8,
                active: 4,
                recovery: 14,
                chain_from: 4,
                damage: 10,
                poise_damage: 10,
                knockback: 90.0,
                hitstun: 18,
                reach: 16.0,
                radius: 12.0,
                lunge: 40.0,
                hitstop: 4,
            }));
            f.step("add attack");
        }
        ui.separator();
        let mut dodges = m.dodge.is_some();
        if f.track(ui.checkbox(&mut dodges, "Can dodge")).changed() {
            m.dodge = dodges.then(|| DodgeDef {
                clip: "dodge".into(),
                ticks: 22,
                moving: 14,
                speed: 220.0,
                invulnerable: (2, 12),
            });
        }
        if let Some(d) = &mut m.dodge {
            ui.indent("dodge", |ui| {
                f.text(ui, "Clip", &mut d.clip, "");
                f.number(
                    ui,
                    "Lasts (ticks)",
                    &mut d.ticks,
                    1..=600,
                    "Moving, then recovering",
                );
                f.number(ui, "Moving (ticks)", &mut d.moving, 0..=600, "");
                f.number(ui, "Speed", &mut d.speed, 0.0..=1000.0, "Pixels a second");
                f.number(
                    ui,
                    "Safe from tick",
                    &mut d.invulnerable.0,
                    0..=600,
                    "Nothing can hurt the dodger",
                );
                f.number(ui, "Safe to tick", &mut d.invulnerable.1, 0..=600, "");
            });
        }
    }

    fn enemy(
        &mut self,
        ui: &mut Ui,
        f: &mut Tracker,
        names: &mut Names,
        id: &str,
        catalog: &Catalog,
    ) {
        let movesets: Vec<String> = self.data.combat.movesets.keys().cloned().collect();
        let Some(e) = self.data.combat.enemies.get_mut(id) else {
            return;
        };
        names.field(ui, "Name", &e.name);
        f.pick(
            ui,
            ("enemy sheet", id),
            "Looks like",
            &mut e.sheet,
            &catalog.characters,
            |s| sheet_name(s).to_owned(),
        );
        let attacks: Vec<String> = catalog.attacks.clone();
        f.pick_optional(
            ui,
            ("enemy attack", id),
            "Attack sheet",
            &mut e.attack,
            &attacks,
            "(the same sheet)",
            |s| sheet_name(s).to_owned(),
        );
        f.pick(
            ui,
            ("enemy moveset", id),
            "Fights as",
            &mut e.moveset,
            &movesets,
            str::to_owned,
        );
        ui.separator();
        ui.label("How it thinks");
        let ai = &mut e.ai;
        f.number(ui, "Sees you from", &mut ai.sight, 1.0..=2000.0, "Pixels");
        f.number(
            ui,
            "Gives up past",
            &mut ai.leash,
            1.0..=4000.0,
            "Pixels from home, then walks back",
        );
        f.number(
            ui,
            "Attacks within",
            &mut ai.attack_range,
            1.0..=400.0,
            "Pixels",
        );
        f.number(ui, "Speed", &mut ai.speed, 0.05..=2.0, "1 walks, 2 runs");
        f.number(
            ui,
            "Backs off for",
            &mut ai.back_off,
            0..=600,
            "Ticks after an attack",
        );
        f.number(
            ui,
            "Comes back after",
            &mut ai.respawn,
            0..=60 * 60 * 60,
            "Ticks after dying (60 a second)",
        );
    }

    fn person(&mut self, ui: &mut Ui, f: &mut Tracker, names: &mut Names, id: &str) {
        let display = &self.display;
        let Some(w) = &mut self.data.world else {
            return;
        };
        let factions: Vec<String> = w.factions.iter().map(|x| x.id.clone()).collect();
        let regions: Vec<String> = w.regions.iter().map(|x| x.id.clone()).collect();
        let titles: Vec<String> = w.titles.iter().map(|x| x.id.clone()).collect();
        let mut roles: Vec<String> = w.actors.iter().map(|a| a.role.clone()).collect();
        roles.sort();
        roles.dedup();
        let Some(a) = w.actors.iter_mut().find(|a| a.id == id) else {
            return;
        };
        names.field(ui, "Name", &a.name);
        ui.horizontal(|ui| {
            ui.label("Role")
                .on_hover_text("warrior, mage, priest, commoner…");
            f.track(ui.text_edit_singleline(&mut a.role));
            ComboBox::from_id_salt(("roles", id))
                .selected_text("…")
                .width(30.0)
                .show_ui(ui, |ui| {
                    for role in &roles {
                        if ui.selectable_label(false, role).clicked() {
                            a.role = role.clone();
                            f.mark(("role", id));
                        }
                    }
                });
        });
        f.pick(
            ui,
            ("faction", id),
            "Faction",
            &mut a.faction,
            &factions,
            |id| shown(display, Category::Factions, id),
        );
        f.pick(ui, ("home", id), "Lives in", &mut a.home, &regions, |id| {
            shown(display, Category::Regions, id)
        });
        f.number(
            ui,
            "Strength",
            &mut a.power,
            0..=100_000,
            "Parties add theirs up",
        );
        f.number(
            ui,
            "Trust needed",
            &mut a.trust,
            -1000..=1000,
            "Standing with their faction before they follow anyone",
        );
        f.check(
            ui,
            "A boss",
            &mut a.boss,
            "Guards their home and never leaves",
        );
        f.pick_many(ui, ("titles", id), "Titles", &mut a.titles, &titles, |id| {
            shown(display, Category::Titles, id)
        });
    }

    fn faction(&mut self, ui: &mut Ui, f: &mut Tracker, names: &mut Names, id: &str) {
        let display = &self.display;
        let Some(w) = &mut self.data.world else {
            return;
        };
        let Some(x) = w.factions.iter_mut().find(|x| x.id == id) else {
            return;
        };
        names.field(ui, "Name", &x.name);
        f.check(ui, "Hostile to everyone", &mut x.hostile, "The demon army");
        let factions: Vec<String> = w.factions.iter().map(|x| x.id.clone()).collect();
        f.pick_optional(
            ui,
            "player faction",
            "Players join",
            &mut w.player_faction,
            &factions,
            "(the first friendly one)",
            |id| shown(display, Category::Factions, id),
        );
    }

    fn region(&mut self, ui: &mut Ui, f: &mut Tracker, names: &mut Names, id: &str) {
        let Some(w) = &mut self.data.world else {
            return;
        };
        let Some(r) = w.regions.iter_mut().find(|x| x.id == id) else {
            return;
        };
        names.field(ui, "Name", &r.name);
        let kinds = [
            (RegionKind::City, "City"),
            (RegionKind::Town, "Town"),
            (RegionKind::Village, "Village"),
            (RegionKind::Wilds, "Wilds"),
            (RegionKind::Dungeon, "Dungeon"),
            (RegionKind::Fortress, "Fortress"),
        ];
        ui.horizontal(|ui| {
            ui.label("Kind");
            let before = r.kind;
            let shown = kinds
                .iter()
                .find(|(k, _)| *k == r.kind)
                .map_or("", |(_, n)| n);
            ComboBox::from_id_salt(("kind", id))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (k, n) in kinds {
                        ui.selectable_value(&mut r.kind, k, n);
                    }
                });
            if r.kind != before {
                f.mark(("kind", id));
            }
        });
        f.number(
            ui,
            "Danger",
            &mut r.danger,
            0..=10,
            "0 safe to 10: how often monsters find travellers, and how strong",
        );
        f.check(
            ui,
            "Has an inn",
            &mut r.inn,
            "Somewhere to sleep safely and heal quickly",
        );
    }

    fn roads(&mut self, ui: &mut Ui, f: &mut Tracker, _names: &mut Names) {
        let display = &self.display;
        ui.heading("Roads");
        ui.label("Walking hours between regions.");
        let regions = self.regions();
        let Some(w) = &mut self.data.world else {
            return;
        };
        let mut remove = None;
        for (i, road) in w.roads.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                f.pick(ui, ("road a", i), "", &mut road.between.0, &regions, |id| {
                    shown(display, Category::Regions, id)
                });
                ui.label("to");
                f.pick(ui, ("road b", i), "", &mut road.between.1, &regions, |id| {
                    shown(display, Category::Regions, id)
                });
                f.track(
                    ui.add(
                        egui::DragValue::new(&mut road.hours)
                            .range(1..=500)
                            .suffix(" h"),
                    ),
                );
                if ui.button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            w.roads.remove(i);
            f.step(("remove road", i));
        }
        if ui.button("Add a road").clicked()
            && let (Some(a), Some(b)) = (regions.first(), regions.get(1))
        {
            w.roads.push(RoadDef {
                between: (a.clone(), b.clone()),
                hours: 10,
            });
            f.step("add road");
        }
    }

    fn title(&mut self, ui: &mut Ui, f: &mut Tracker, names: &mut Names, id: &str) {
        let display = &self.display;
        let Some(w) = &mut self.data.world else {
            return;
        };
        let factions: Vec<String> = w.factions.iter().map(|x| x.id.clone()).collect();
        let Some(t) = w.titles.iter_mut().find(|x| x.id == id) else {
            return;
        };
        names.field(ui, "Name", &t.name);
        ui.label("When its holder dies:");
        ui.indent("succession", |ui| {
            f.check(
                ui,
                "Their killer takes it",
                &mut t.succession.to_killer,
                "If the killer is a person, not a monster",
            );
            let mut appointed = t.succession.appointed.is_some();
            if f.track(ui.checkbox(&mut appointed, "A faction names someone"))
                .changed()
            {
                t.succession.appointed = appointed.then(|| Appointment {
                    faction: factions.first().cloned().unwrap_or_default(),
                    role: "warrior".into(),
                });
            }
            if let Some(a) = &mut t.succession.appointed {
                ui.indent("appointed", |ui| {
                    f.pick(
                        ui,
                        ("appointer", id),
                        "Faction",
                        &mut a.faction,
                        &factions,
                        |id| shown(display, Category::Factions, id),
                    );
                    f.text(
                        ui,
                        "Role",
                        &mut a.role,
                        "Its strongest living member of this role without a title",
                    );
                });
            }
            ui.weak("Otherwise the title falls vacant.");
        });
    }

    fn hero_party(&mut self, ui: &mut Ui, f: &mut Tracker, _names: &mut Names) {
        let display = &self.display;
        ui.heading("Hero party");
        let Some(w) = &mut self.data.world else {
            return;
        };
        let titles: Vec<String> = w.titles.iter().map(|x| x.id.clone()).collect();
        let actors: Vec<String> = w.actors.iter().map(|x| x.id.clone()).collect();
        let bosses: Vec<String> = w
            .actors
            .iter()
            .filter(|a| a.boss)
            .map(|x| x.id.clone())
            .collect();
        let party = &mut w.hero_party;
        f.pick(
            ui,
            "leader",
            "Led by the holder of",
            &mut party.leader_title,
            &titles,
            |id| shown(display, Category::Titles, id),
        );
        f.pick_many(
            ui,
            "members",
            "Members at the start",
            &mut party.members,
            &actors,
            |id| shown(display, Category::People, id),
        );
        ui.horizontal(|ui| {
            ui.label("Roles it needs (one member each)");
        });
        let mut remove = None;
        for (i, role) in party.roles.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                f.track(ui.text_edit_singleline(role));
                if ui.button("✖").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            party.roles.remove(i);
            f.step(("remove role", i));
        }
        if ui.button("Add a role").clicked() {
            party.roles.push("warrior".into());
            f.step("add role");
        }
        f.pick(ui, "goal", "Must defeat", &mut party.goal, &bosses, |id| {
            shown(display, Category::People, id)
        });
        f.pick_many(
            ui,
            "lieutenants",
            "Worth defeating first",
            &mut party.lieutenants,
            &bosses,
            |id| shown(display, Category::People, id),
        );
    }
}

impl Database {
    /// The year: a grid of its days coloured by season, events marked; then the seasons and the
    /// events, to change.
    fn calendar(&mut self, ui: &mut Ui, f: &mut Tracker, names: &mut Names, elsewhere: &Elsewhere) {
        ui.heading("Calendar");
        ui.label("365 days. A season warms or cools every region's air; storylets can wait for a season or an event.");
        let Some(w) = &mut self.data.world else {
            return;
        };
        let cal = &mut w.calendar;
        // The year, 30 days a row.
        let colours = [
            egui::Color32::from_rgb(90, 150, 90),
            egui::Color32::from_rgb(200, 170, 70),
            egui::Color32::from_rgb(170, 100, 60),
            egui::Color32::from_rgb(110, 140, 200),
            egui::Color32::from_rgb(150, 110, 170),
            egui::Color32::from_rgb(120, 120, 120),
        ];
        let cell = 22.0;
        let (area, response) = ui.allocate_exact_size(
            egui::vec2(30.0 * cell, (365.0_f32 / 30.0).ceil() * cell),
            egui::Sense::click(),
        );
        let painter = ui.painter_at(area);
        for day in 1..=365u32 {
            let (col, row) = ((day - 1) % 30, (day - 1) / 30);
            let rect = egui::Rect::from_min_size(
                area.min + egui::vec2(col as f32 * cell, row as f32 * cell),
                egui::vec2(cell - 2.0, cell - 2.0),
            );
            let season = cal
                .seasons
                .iter()
                .position(|s| cal.season(day).is_some_and(|t| t.id == s.id));
            let fill = season.map_or(egui::Color32::from_gray(50), |i| colours[i % colours.len()]);
            painter.rect_filled(rect, 2.0, fill.gamma_multiply(0.7));
            if cal
                .events
                .iter()
                .any(|e| (e.day..e.day.saturating_add(e.days.max(1))).contains(&day))
            {
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
            }
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                day.to_string(),
                egui::FontId::proportional(9.0),
                egui::Color32::from_gray(230),
            );
        }
        let day_at = |p: egui::Pos2| {
            let v = p - area.min;
            let day = (v.y / cell).floor() as u32 * 30 + (v.x / cell).floor() as u32 + 1;
            (1..=365).contains(&day).then_some(day)
        };
        if let Some(day) = response.hover_pos().and_then(day_at) {
            let season = cal
                .season(day)
                .map_or("no season".to_owned(), |s| s.id.clone());
            let on: Vec<&str> = cal
                .events
                .iter()
                .filter(|e| (e.day..e.day.saturating_add(e.days.max(1))).contains(&day))
                .map(|e| e.id.as_str())
                .collect();
            response
                .clone()
                .on_hover_text(format!("Day {day}: {season} {}", on.join(", ")));
        }
        let clicked = response
            .clicked()
            .then(|| response.interact_pointer_pos().and_then(day_at))
            .flatten();
        if let Some(day) = clicked {
            ui.data_mut(|d| d.insert_temp(egui::Id::new("calendar day"), day));
        }
        let chosen: u32 = ui
            .data(|d| d.get_temp(egui::Id::new("calendar day")))
            .unwrap_or(1);
        ui.horizontal(|ui| {
            ui.label(format!("Day {chosen} (click the year to choose another):"));
            let id_field = egui::Id::new("calendar new id");
            let mut id: String = ui.data(|d| d.get_temp(id_field)).unwrap_or_default();
            ui.add(
                egui::TextEdit::singleline(&mut id)
                    .hint_text("id, e.g. harvest_fair")
                    .desired_width(130.0),
            );
            tidy_id(&mut id);
            let free = valid_id(&id)
                && !cal.seasons.iter().any(|s| s.id == id)
                && !cal.events.iter().any(|e| e.id == id);
            let starts = cal.seasons.iter().any(|s| s.from_day == chosen);
            if ui
                .add_enabled(free && !starts, egui::Button::new("A season starts here"))
                .on_disabled_hover_text("Type an id; a day can start one season")
                .clicked()
            {
                let wanted = format!("season.{id}");
                let key = if names.strings.is_free(&wanted) {
                    wanted
                } else {
                    names.strings.fresh_key(&wanted, |_| false)
                };
                names
                    .strings
                    .set(&key, names.language, &id.replace('_', " "));
                cal.seasons.push(dark_sim::SeasonDef {
                    id: id.clone(),
                    name: key,
                    from_day: chosen,
                    warmth: 0,
                });
                cal.seasons.sort_by_key(|s| s.from_day);
                f.step(("add season", &id));
                id.clear();
            }
            if ui
                .add_enabled(free, egui::Button::new("An event here"))
                .clicked()
            {
                let wanted = format!("event.{id}");
                let key = if names.strings.is_free(&wanted) {
                    wanted
                } else {
                    names.strings.fresh_key(&wanted, |_| false)
                };
                names
                    .strings
                    .set(&key, names.language, &id.replace('_', " "));
                cal.events.push(dark_sim::EventDef {
                    id: id.clone(),
                    name: key,
                    day: chosen,
                    days: 1,
                });
                f.step(("add event", &id));
                id.clear();
            }
            ui.data_mut(|d| d.insert_temp(id_field, id));
        });
        ui.separator();
        ui.heading("Seasons");
        ui.weak(
            "Each runs from its first day to the next's; the days before the first are the last's.",
        );
        let mut remove = None;
        for (i, season) in cal.seasons.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.strong(&season.id);
                names.field(ui, "Name", &season.name);
                f.track(
                    ui.add(
                        egui::DragValue::new(&mut season.from_day)
                            .range(1..=365)
                            .prefix("from day "),
                    ),
                );
                degrees(
                    ui,
                    f,
                    "warmer by",
                    &mut season.warmth,
                    "Added to every region's air (negative: colder)",
                );
                let used = story_uses(elsewhere.story, Category::Calendar, &season.id);
                if ui
                    .add_enabled(used.is_none(), egui::Button::new("✖"))
                    .on_disabled_hover_text(format!("Used by {}", used.unwrap_or_default()))
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            cal.seasons.remove(i);
            f.step(("remove season", i));
        }
        ui.separator();
        ui.heading("Events");
        let mut remove = None;
        for (i, event) in cal.events.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.strong(&event.id);
                names.field(ui, "Name", &event.name);
                f.track(
                    ui.add(
                        egui::DragValue::new(&mut event.day)
                            .range(1..=365)
                            .prefix("day "),
                    ),
                );
                f.track(
                    ui.add(
                        egui::DragValue::new(&mut event.days)
                            .range(1..=365)
                            .suffix(" days"),
                    ),
                );
                let used = story_uses(elsewhere.story, Category::Calendar, &event.id);
                if ui
                    .add_enabled(used.is_none(), egui::Button::new("✖"))
                    .on_disabled_hover_text(format!("Used by {}", used.unwrap_or_default()))
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            cal.events.remove(i);
            f.step(("remove event", i));
        }
        if let Err(e) = cal.validate() {
            ui.colored_label(egui::Color32::from_rgb(255, 110, 100), e.to_string());
        }
    }

    /// The tuning numbers. Try changes out with `dark-cli simulate` (the world) or in play.
    fn balance(&mut self, ui: &mut Ui, f: &mut Tracker) {
        ui.heading("Balance");
        ui.weak("Needs run from 0 (fine) to 1000 (failing). Rates are per game hour.");
        let r = &mut self.data.life.rates;
        egui::CollapsingHeader::new("Bodies: needs")
            .default_open(true)
            .show(ui, |ui| {
                for (label, rates) in [("Awake", &mut r.awake), ("Asleep", &mut r.asleep)] {
                    ui.label(label);
                    ui.indent(label, |ui| {
                        let top = 0..=1000;
                        f.number(ui, "Hunger", &mut rates.hunger, top.clone(), "");
                        f.number(ui, "Thirst", &mut rates.thirst, top.clone(), "");
                        f.number(ui, "Tiredness", &mut rates.fatigue, top.clone(), "");
                        f.number(ui, "Bladder", &mut rates.bladder, top.clone(), "");
                        f.number(ui, "Bowels", &mut rates.bowel, top.clone(), "");
                        f.number(ui, "Dirt", &mut rates.hygiene, top, "");
                    });
                }
                ui.label("Tiredness slept off per hour");
                ui.indent("rest", |ui| {
                    f.number(ui, "In the open", &mut r.rest_open, 0..=1000, "");
                    f.number(ui, "In a tent", &mut r.rest_tent, 0..=1000, "");
                    f.number(ui, "At an inn", &mut r.rest_inn, 0..=1000, "");
                    f.number(
                        ui,
                        "Wakes below",
                        &mut r.come_to_fatigue,
                        0..=1000,
                        "Asleep or out cold, tiredness under this lets a body come to",
                    );
                });
            });
        egui::CollapsingHeader::new("Bodies: drink").show(ui, |ui| {
            ui.weak("1000 is one drink in the blood.");
            f.number(
                ui,
                "Sobering awake",
                &mut r.sober_awake,
                0..=10_000,
                "Per hour",
            );
            f.number(
                ui,
                "Sobering asleep",
                &mut r.sober_asleep,
                0..=10_000,
                "Per hour",
            );
            f.number(ui, "Tipsy from", &mut r.drunk.tipsy, 0..=20_000, "");
            f.number(ui, "Drunk from", &mut r.drunk.drunk, 0..=20_000, "");
            f.number(ui, "Wasted from", &mut r.drunk.wasted, 0..=20_000, "");
            f.number(ui, "Blackout from", &mut r.drunk.blackout, 0..=20_000, "");
            f.number(
                ui,
                "Drowsiness",
                &mut r.drunk.drowsy,
                0..=1000,
                "Extra tiredness per hour when drunk (twice when wasted)",
            );
        });
        egui::CollapsingHeader::new("Bodies: warmth").show(ui, |ui| {
            degrees(ui, f, "Comfortable from", &mut r.comfort.0, "Felt temperatures that keep the body warm");
            degrees(ui, f, "Comfortable to", &mut r.comfort.1, "");
            f.number(ui, "Chill", &mut r.chill, 1..=100_000, "Outside comfort the body's warmth moves by (felt − edge) / chill each minute: higher is slower");
            f.number(ui, "Recovery", &mut r.recover, 0..=1000, "Hundredths of a degree a minute, inside comfort");
            degrees(ui, f, "Cold below", &mut r.core.cold, "The body's own warmth");
            degrees(ui, f, "Freezing below", &mut r.core.freezing, "");
            degrees(ui, f, "Hypothermic below", &mut r.core.hypothermic, "");
            degrees(ui, f, "Hot above", &mut r.core.hot, "");
            degrees(ui, f, "Heatstroke above", &mut r.core.heatstroke, "");
            degrees(ui, f, "An inn's air", &mut r.indoor_air, "");
            degrees(ui, f, "A tent adds", &mut r.tent_warmth, "");
            degrees(ui, f, "A fire nearby adds", &mut r.fire_warmth, "");
        });
        egui::CollapsingHeader::new("Bodies: harm").show(ui, |ui| {
            ui.weak("Health lost per hour.");
            f.number(ui, "Starving", &mut r.harm.starving, 0..=1000, "");
            f.number(ui, "Parched", &mut r.harm.parched, 0..=1000, "");
            f.number(ui, "Freezing", &mut r.harm.freezing, 0..=1000, "");
            f.number(ui, "Hypothermic", &mut r.harm.hypothermic, 0..=1000, "");
            f.number(ui, "Heatstroke", &mut r.harm.heatstroke, 0..=1000, "");
        });
        let Some(w) = &mut self.data.world else {
            return;
        };
        let t = &mut w.tuning;
        egui::CollapsingHeader::new("The world's year").show(ui, |ui| {
            f.number(
                ui,
                "Training per day",
                &mut t.train_per_day,
                0..=1000,
                "Strength each member gains per day training in a settlement",
            );
            f.number(
                ui,
                "Healing at an inn",
                &mut t.rest_inn_per_hour,
                0..=100,
                "Health (of 100) per hour",
            );
            f.number(
                ui,
                "Healing elsewhere",
                &mut t.rest_wild_per_hour,
                0..=100,
                "Health (of 100) per hour",
            );
            f.number(
                ui,
                "Monsters met",
                &mut t.encounter_permille_per_danger,
                0..=1000,
                "Chance per hour, tenths of a percent per point of danger",
            );
            f.number(
                ui,
                "Monster strength",
                &mut t.monster_power_per_danger,
                0..=10_000,
                "Per point of danger (±50%)",
            );
            f.number(
                ui,
                "Experience divisor",
                &mut t.xp_divisor,
                1..=10_000,
                "After a win each member gains the enemy's strength divided by this",
            );
            f.number(
                ui,
                "Ready to attack at",
                &mut t.readiness_percent,
                1..=1000,
                "The party attacks a boss at this percentage of its strength",
            );
            f.number(
                ui,
                "Days in hand",
                &mut t.deadline_margin_days,
                0..=365,
                "Marches on the goal regardless when this few days are left",
            );
            f.number(
                ui,
                "Sharpness",
                &mut t.sharpness,
                1..=20,
                "How decisive strength is in a fight",
            );
            f.number(
                ui,
                "Wounds",
                &mut t.wound_percent,
                0..=100,
                "Health lost in a fight against an equal",
            );
            f.number(
                ui,
                "Lieutenants weaken by",
                &mut t.lieutenant_weakens_percent,
                0..=100,
                "Percent of the goal's strength, each",
            );
        });
        let s = &mut w.social;
        egui::CollapsingHeader::new("Standing and loyalty").show(ui, |ui| {
            ui.weak("Both run from −1000 to 1000.");
            let span = -1000..=1000;
            f.number(
                ui,
                "With one's own faction",
                &mut s.own_standing,
                span.clone(),
                "",
            );
            f.number(
                ui,
                "With the other side",
                &mut s.enemy_standing,
                span.clone(),
                "",
            );
            f.number(
                ui,
                "Killing costs",
                &mut s.kill_penalty,
                0..=2000,
                "With the victim's faction",
            );
            f.number(
                ui,
                "Murder costs",
                &mut s.murder_penalty,
                0..=2000,
                "Killing a friend: with every other friendly faction too",
            );
            f.number(
                ui,
                "Slaying earns",
                &mut s.slay_reward,
                0..=2000,
                "Killing the hostile: with every friendly faction",
            );
            f.number(
                ui,
                "A follower's loyalty",
                &mut s.loyalty_start,
                span.clone(),
                "At first, plus half the leader's standing",
            );
            f.number(
                ui,
                "Loyalty per day",
                &mut s.loyalty_per_day,
                -1000..=1000,
                "",
            );
            f.number(ui, "Leaves below", &mut s.desert_below, span.clone(), "");
            f.number(ui, "Betrays below", &mut s.betray_below, span, "");
            f.number(
                ui,
                "Betrayal costs",
                &mut s.betrayal_penalty,
                0..=2000,
                "With every faction in the party",
            );
            f.number(ui, "Party size", &mut s.max_party, 1..=64, "");
            f.number(ui, "Players at most", &mut s.max_players, 1..=4, "");
        });
    }
}

/// The ids in `category`, in order, with their names' keys.
fn entries(d: &Data, category: Category) -> Vec<(String, Option<String>)> {
    let world = d.world.as_ref();
    match category {
        Category::Items => d
            .life
            .items
            .iter()
            .map(|(id, i)| (id.clone(), Some(i.name.clone())))
            .collect(),
        Category::Movesets => d
            .combat
            .movesets
            .keys()
            .map(|id| (id.clone(), None))
            .collect(),
        Category::Enemies => d
            .combat
            .enemies
            .iter()
            .map(|(id, e)| (id.clone(), Some(e.name.clone())))
            .collect(),
        Category::People => world
            .iter()
            .flat_map(|w| &w.actors)
            .map(|a| (a.id.clone(), Some(a.name.clone())))
            .collect(),
        Category::Factions => world
            .iter()
            .flat_map(|w| &w.factions)
            .map(|f| (f.id.clone(), Some(f.name.clone())))
            .collect(),
        Category::Regions => world
            .iter()
            .flat_map(|w| &w.regions)
            .map(|r| (r.id.clone(), Some(r.name.clone())))
            .collect(),
        Category::Titles => world
            .iter()
            .flat_map(|w| &w.titles)
            .map(|t| (t.id.clone(), Some(t.name.clone())))
            .collect(),
        _ => Vec::new(),
    }
}

/// What the rest of the editor holds unsaved, for "used by".
pub struct Elsewhere<'a> {
    /// The open map, as edited.
    pub scene: Option<(&'a str, &'a SceneDef)>,
    pub story: &'a StoryDef,
}

/// Where the story names `id` of `category`, if it does.
fn story_uses(story: &StoryDef, category: Category, id: &str) -> Option<String> {
    use dark_story::{Condition, Effect};
    fn condition(c: &Condition, category: Category, id: &str) -> bool {
        match c {
            Condition::Standing { faction, .. }
            | Condition::StandingBelow { faction, .. }
            | Condition::Faction(faction) => category == Category::Factions && faction == id,
            Condition::Title(t) => category == Category::Titles && t == id,
            Condition::Alive(p) | Condition::Dead(p) => category == Category::People && p == id,
            Condition::Season(c) | Condition::During(c) => {
                category == Category::Calendar && c == id
            }
            Condition::Not(inner) => condition(inner, category, id),
            _ => false,
        }
    }
    let effect = |e: &Effect| match e {
        Effect::Standing { faction, .. } | Effect::Defect(faction) => {
            category == Category::Factions && faction == id
        }
        Effect::Give { item, .. } => category == Category::Items && item == id,
        _ => false,
    };
    for s in &story.storylets {
        let with = category == Category::People && s.with == id;
        let conditions = s.when.iter().chain(
            s.nodes
                .values()
                .flat_map(|n| n.choices.iter().flat_map(|c| &c.when)),
        );
        let effects = s
            .nodes
            .values()
            .flat_map(|n| n.then.iter().chain(n.choices.iter().flat_map(|c| &c.then)));
        if with
            || conditions.clone().any(|c| condition(c, category, id))
            || effects.clone().any(effect)
        {
            return Some(format!("conversation {}", s.id));
        }
    }
    story
        .endings
        .iter()
        .find(|e| e.when.iter().any(|c| condition(c, category, id)))
        .map(|e| format!("ending {}", e.id))
}

/// Which of life, combat and world differ between `a` and `b`.
fn changed_files(a: &Data, b: &Data) -> [bool; 3] {
    let differ = |x: Result<String, String>, y: Result<String, String>| x != y;
    [
        differ(to_ron(&a.life), to_ron(&b.life)),
        differ(to_ron(&a.combat), to_ron(&b.combat)),
        differ(
            a.world.as_ref().map_or(Ok(String::new()), to_ron),
            b.world.as_ref().map_or(Ok(String::new()), to_ron),
        ),
    ]
}

/// A thing as pickers show it: its name, then its id.
fn shown(display: &HashMap<(Category, String), String>, category: Category, id: &str) -> String {
    match display.get(&(category, id.to_owned())) {
        Some(name) if !name.is_empty() && name != id => format!("{name} ({id})"),
        _ => id.to_owned(),
    }
}

/// One attack's numbers.
fn attack(ui: &mut Ui, f: &mut Tracker, a: &mut AttackDef) {
    f.text(ui, "Clip", &mut a.clip, "Played as <clip>_<facing>");
    ui.weak("Ticks, 60 a second.");
    f.number(
        ui,
        "Wind-up",
        &mut a.startup,
        0..=600,
        "Before the hit can land: what an enemy's attack is read by",
    );
    f.number(
        ui,
        "Hitting",
        &mut a.active,
        1..=600,
        "Ticks the hit is out",
    );
    f.number(
        ui,
        "Recovery",
        &mut a.recovery,
        0..=600,
        "Before moving again",
    );
    f.number(
        ui,
        "Combo from",
        &mut a.chain_from,
        0..=600,
        "Ticks into recovery a next attack or a dodge can start",
    );
    f.number(ui, "Damage", &mut a.damage, 0..=10_000, "");
    f.number(ui, "Poise damage", &mut a.poise_damage, 0..=10_000, "");
    f.number(
        ui,
        "Knockback",
        &mut a.knockback,
        0.0..=2000.0,
        "Pixels a second, on a stagger",
    );
    f.number(
        ui,
        "Stagger",
        &mut a.hitstun,
        0..=600,
        "Ticks the victim is staggered",
    );
    f.number(
        ui,
        "Reach",
        &mut a.reach,
        0.0..=400.0,
        "Pixels in front of the feet",
    );
    f.number(
        ui,
        "Size",
        &mut a.radius,
        1.0..=400.0,
        "The hit's radius, pixels",
    );
    f.number(
        ui,
        "Lunge",
        &mut a.lunge,
        0.0..=1000.0,
        "Forward speed while winding up and striking",
    );
    f.number(
        ui,
        "Hit stop",
        &mut a.hitstop,
        0..=30,
        "Frames both freeze when it lands",
    );
}

/// A temperature kept in hundredths of a degree, shown in degrees.
fn degrees(ui: &mut Ui, f: &mut Tracker, label: &str, value: &mut i32, help: &str) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(help);
        let mut shown = *value as f32 / 100.0;
        let field = egui::DragValue::new(&mut shown)
            .range(-100.0..=100.0)
            .speed(0.1)
            .max_decimals(2)
            .suffix(" °C")
            .clamp_existing_to_range(false);
        if f.track(ui.add(field)).changed() {
            *value = (shown * 100.0).round() as i32;
        }
    });
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|first| first.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Translated names: typed as text in the language being written.
struct Names<'a> {
    strings: &'a mut Strings,
    language: &'a str,
    changed: bool,
}

impl Names<'_> {
    fn field(&mut self, ui: &mut Ui, label: &str, key: &str) {
        ui.horizontal(|ui| {
            ui.label(format!("{label} ({})", self.language));
            let mut text = self.strings.text(key, self.language);
            let edit = egui::TextEdit::singleline(&mut text).id(egui::Id::new(("name", key)));
            if ui.add(edit).changed() {
                self.strings.set(key, self.language, &text);
                self.changed = true;
            }
        });
    }
}

/// `value` as the editor writes a data file: pretty RON, optional fields plain.
pub(crate) fn to_ron<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let config = ron::ser::PrettyConfig::default()
        .extensions(ron::extensions::Extensions::IMPLICIT_SOME)
        .struct_names(false);
    ron::ser::to_string_pretty(value, config).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written and read back, every file says the same again.
    #[test]
    fn the_data_files_round_trip() {
        let life = LifeDef::parse(
            r#"(items: {"ale": (name: "item.ale", icon: (image: "i.png", size: 48, cell: (0, 16)),
                  use: Consume((thirst: -150, alcohol: 1000))),
                "cloak": (name: "item.cloak", use: Wear(insulation: 1500)),
                "fire": (name: "item.fire", use: Place(Campfire(minutes: 240))),
                "soap": (name: "item.soap", use: Wash)},
               start: [("ale", 2), ("cloak", 1)], wear: "cloak",
               climates: {"r": (day: 2000, night: 100)})"#,
        )
        .unwrap();
        let text = to_ron(&life).unwrap();
        assert_eq!(to_ron(&LifeDef::parse(&text).unwrap()).unwrap(), text);

        let combat = CombatDef::parse(
            r#"(movesets: {"m": (health: 50, combo: [(startup: 6, active: 4, recovery: 12,
                  damage: 12, reach: 16, radius: 12)], dodge: (ticks: 22, moving: 14, speed: 220,
                  invulnerable: (2, 12)))},
               enemies: {"e": (name: "enemy.e", sheet: "s", moveset: "m",
                  ai: (sight: 100, leash: 200, attack_range: 18))})"#,
        )
        .unwrap();
        let text = to_ron(&combat).unwrap();
        let again = CombatDef::parse(&text).unwrap();
        assert_eq!(to_ron(&again).unwrap(), text);
        assert_eq!(again.movesets["m"], combat.movesets["m"]);

        let world = WorldDef::parse(WORLD).unwrap();
        let text = to_ron(&world).unwrap();
        assert_eq!(to_ron(&WorldDef::parse(&text).unwrap()).unwrap(), text);
        dark_sim::WorldSim::new(&WorldDef::parse(&text).unwrap(), 1).unwrap();
    }

    /// The game project's own files (`DARK_TEST_PROJECT`) come back the same once written.
    #[test]
    fn the_projects_own_files_round_trip() {
        let Ok(root) = std::env::var("DARK_TEST_PROJECT") else {
            return;
        };
        let project = Project::open(root).unwrap();
        let db = Database::load(&project);
        assert!(db.broken.is_empty(), "{:?}", db.broken);
        let d = &db.data;
        let life = to_ron(&d.life).unwrap();
        assert_eq!(to_ron(&LifeDef::parse(&life).unwrap()).unwrap(), life);
        let combat = to_ron(&d.combat).unwrap();
        assert_eq!(to_ron(&CombatDef::parse(&combat).unwrap()).unwrap(), combat);
        if let Some(world) = &d.world {
            let text = to_ron(world).unwrap();
            assert_eq!(to_ron(&WorldDef::parse(&text).unwrap()).unwrap(), text);
        }
        db.check().unwrap();
    }

    const WORLD: &str = r#"(
        regions: [(id: "town", name: "region.town", kind: Town, danger: 0, inn: true),
                  (id: "keep", name: "region.keep", kind: Fortress, danger: 3)],
        roads: [(between: ("town", "keep"), hours: 5)],
        factions: [(id: "kingdom", name: "f.k"), (id: "demons", name: "f.d", hostile: true)],
        titles: [(id: "hero", name: "t.h", succession: (to_killer: true,
                    appointed: (faction: "kingdom", role: "warrior"))),
                 (id: "lord", name: "t.l")],
        actors: [(id: "hero", name: "a.h", role: "warrior", faction: "kingdom", power: 40,
                    home: "town", titles: ["hero"]),
                 (id: "lord", name: "a.l", role: "lord", faction: "demons", power: 500,
                    home: "keep", titles: ["lord"], boss: true)],
        hero_party: (leader_title: "hero", roles: ["warrior"], members: ["hero"], goal: "lord"),
    )"#;

    #[test]
    fn things_in_use_cannot_be_deleted_and_new_ones_start_valid() {
        let mut db = Database {
            data: Data {
                life: LifeDef::default(),
                combat: CombatDef::default(),
                world: Some(WorldDef::parse(WORLD).unwrap()),
            },
            broken: Vec::new(),
            unreadable: [false; 3],
            dirty: [false; 3],
            undo: Vec::new(),
            redo: Vec::new(),
            editing: None,
            category: Category::Regions,
            selected: None,
            new_id: String::new(),
            uses: None,
            icons: HashMap::new(),
            display: HashMap::new(),
        };
        let project = test_project();
        let catalog = Catalog::default();
        let story = StoryDef::parse(
            r#"(storylets: [(id: "talk", with: "hero", start: "a", nodes: {
                "a": (line: "l", then: [Give(item: "stew", count: 1)])})])"#,
        )
        .unwrap();
        let elsewhere = Elsewhere {
            scene: None,
            story: &story,
        };
        let uses = db.used_by("keep", &project, &catalog, &elsewhere);
        assert!(uses.iter().any(|u| u == "person lord"), "{uses:?}");
        assert!(uses.iter().any(|u| u == "a road"));
        // A new region, a person living there and a faction: the world still builds.
        let mut strings = Strings::load(&project);
        db.add("marsh", &mut strings, "en");
        db.category = Category::People;
        db.add("hermit", &mut strings, "en");
        db.category = Category::Factions;
        db.add("guild", &mut strings, "en");
        db.category = Category::Movesets;
        db.add("brawler", &mut strings, "en");
        db.category = Category::Enemies;
        db.add("wolf", &mut strings, "en");
        db.category = Category::Items;
        db.add("stew", &mut strings, "en");
        db.check().unwrap();
        assert_eq!(
            strings.text("region.marsh", "en"),
            "marsh",
            "named after its id"
        );
        assert!(
            db.used_by("guild", &project, &catalog, &elsewhere)
                .is_empty()
        );
        // The story (as edited, not as saved) names the stew; the hero only as a person.
        db.category = Category::Items;
        let uses = db.used_by("stew", &project, &catalog, &elsewhere);
        assert_eq!(uses, vec!["the story (conversation talk)".to_owned()]);
        db.category = Category::Titles;
        let uses = db.used_by("hero", &project, &catalog, &elsewhere);
        assert!(!uses.iter().any(|u| u.contains("story")), "{uses:?}");
        db.category = Category::Factions;
        db.remove("guild");
        db.check().unwrap();
        std::fs::remove_dir_all(project.root()).ok();
    }

    fn test_project() -> Project {
        let root = std::env::temp_dir().join(format!("dark-editor-db-{}", std::process::id()));
        std::fs::create_dir_all(root.join("locale")).unwrap();
        std::fs::write(
            root.join("project.ron"),
            r#"(name: "T", tile_size: 16, resolution: (320, 180), languages: ["en"])"#,
        )
        .unwrap();
        std::fs::write(root.join("locale/en.ron"), "{}").unwrap();
        Project::open(&root).unwrap()
    }
}
