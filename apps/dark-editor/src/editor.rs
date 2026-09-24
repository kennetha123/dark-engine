//! The editor's window: maps on the left with the picture palette, the map in the middle as
//! the game draws it, what is selected on the right, and tools along the top. Everything is
//! made by pointing and typing; nothing needs writing by hand.

use std::collections::HashMap;
use std::sync::Arc;

use dark_assets::{LineDef, Project, SceneDef};
use dark_physics::Cell;
use dark_sprite::Facing;
use dark_world::{Map, Maps};
use egui::{
    Align2, CentralPanel, Color32, ComboBox, DragValue, FontId, Key, KeyboardShortcut, Modifiers,
    Panel, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2,
};
use glam::Vec2;

use crate::catalog::{Catalog, scene_name, sheet_name};
use crate::database::{Database, Elsewhere};
use crate::minimap::{self, Fit};
use crate::scene_ops::{self as ops, History, Thing};
use crate::sheets::SheetEditor;
use crate::story::StoryEditor;
use crate::strings::Strings;
use crate::viewport::{Figure, MAX_ZOOM, Mapping, View, Viewport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Terrain,
    Prop,
    Npc,
    Enemy,
    Exit,
    Inn,
    Start,
    Place,
    Erase,
}

impl Tool {
    const ALL: [(Tool, &'static str, &'static str); 10] = [
        (
            Tool::Select,
            "Select",
            "Click something to select it, drag it to move it. Delete removes it.",
        ),
        (
            Tool::Terrain,
            "Terrain",
            "Paint the ground: the left button paints the brush, the right button plain ground.",
        ),
        (
            Tool::Prop,
            "Props",
            "Pick a picture in the palette, then click the map to place it.",
        ),
        (
            Tool::Npc,
            "Villager",
            "Click to place a villager; write what they say on the right.",
        ),
        (Tool::Enemy, "Enemy", "Click to place an enemy."),
        (
            Tool::Exit,
            "Exit",
            "Drag a box: walking into it takes you to another map.",
        ),
        (
            Tool::Inn,
            "Inn",
            "Drag a box over an inn: sleeping inside is sleeping indoors.",
        ),
        (
            Tool::Start,
            "Player start",
            "Click where the player begins.",
        ),
        (
            Tool::Place,
            "Stamp a place",
            "Click to stamp the chosen town, camp or ruin here. Everything in it becomes part \
             of this map.",
        ),
        (Tool::Erase, "Erase", "Click something to remove it."),
    ];
}

/// What the terrain tool paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Brush {
    Ground,
    Hill(u8),
    Wall,
}

impl Brush {
    const ALL: [(Brush, &'static str); 5] = [
        (Brush::Ground, "Ground"),
        (Brush::Hill(1), "Hill 1"),
        (Brush::Hill(2), "Hill 2"),
        (Brush::Hill(3), "Hill 3"),
        (Brush::Wall, "Wall"),
    ];

    fn cell(self) -> Cell {
        match self {
            Brush::Ground => Cell::Floor,
            Brush::Hill(n) => Cell::Level(n),
            Brush::Wall => Cell::Wall,
        }
    }
}

/// A press on the map being held.
enum Drag {
    Move {
        thing: Thing,
        grab: Vec2,
    },
    /// Painting terrain: where the pointer was last frame, and whether the stroke has been
    /// recorded for undo yet.
    Paint {
        last: Vec2,
        recorded: bool,
    },
    Area {
        from: Vec2,
    },
    Pan,
}

struct OpenScene {
    path: String,
    def: SceneDef,
    dirty: bool,
    /// Where other maps' exits arrive in this one.
    landings: Vec<(f32, f32)>,
}

/// Something waiting for an answer about unsaved changes.
#[derive(Clone)]
enum Pending {
    Open(String),
    NewMap { name: String, cols: u32, rows: u32 },
    Quit,
}

/// What the window is for: one of the editor's parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Workspace {
    Maps,
    Database,
    Story,
    Sheets,
}

struct NewMap {
    name: String,
    cols: u32,
    rows: u32,
    /// The name field has been given the keyboard.
    focused: bool,
}

pub struct Editor {
    project: Project,
    workspace: Workspace,
    database: Database,
    story: StoryEditor,
    sheets: SheetEditor,
    tile: u32,
    catalog: Catalog,
    pub viewport: Viewport,
    strings: Strings,
    /// The language text is written in.
    language: String,
    scene: Option<OpenScene>,
    history: History,
    tool: Tool,
    brush: Brush,
    /// The palette picture the prop tool places.
    prop: Option<(String, u32)>,
    palette_sheet: String,
    prop_solid: bool,
    npc_sheet: String,
    enemy_kind: String,
    /// The scene the place tool stamps.
    place_scene: String,
    selected: Option<Thing>,
    drag: Option<Drag>,
    /// The inspector field being typed into, so one burst of typing is one undo step.
    editing: Option<egui::Id>,
    view: View,
    mapping: Option<Mapping>,
    wanted: (u32, u32),
    overlay: bool,
    grid: bool,
    status: (String, bool),
    /// The map must be built again before it is drawn.
    stale: bool,
    thumbnails: HashMap<String, egui::TextureHandle>,
    pending: Option<Pending>,
    new_map: Option<NewMap>,
    hover: Option<Vec2>,
    /// Asked to close, and nothing is left unsaved.
    pub quit: bool,
    /// How many play a playtest, each in a window of their own.
    players: u8,
    /// Picking, on the map an exit leads to, where it arrives: the exit's map and number.
    picking: Option<(String, usize)>,
}

/// Where a playtest with more than one player is hosted.
const PLAYTEST_PORT: &str = "7777";

/// The largest map the editor will make or resize to, in tiles a side. The engine carries far
/// more (`SceneDef::MAX_TILES_PER_SIDE`), but painting one tile here rebuilds the whole grid of
/// them, so a map this size is already a slow brush. A world larger than this is made from a
/// seed rather than painted (docs/PLAN.md §24.4, §24.5).
const PAINTABLE_TILES: u32 = 4096;

/// How tall the minimap is, in points. Enough to read a map's shape without crowding out the
/// prop palette below it.
const MINIMAP_HEIGHT: f32 = 130.0;

impl Editor {
    pub fn new(
        project: Project,
        mut viewport: Viewport,
        ctx: &egui::Context,
        start: Option<String>,
    ) -> Self {
        let tile = project.settings.tile_size;
        let catalog = Catalog::load(&project, &mut viewport);
        let strings = Strings::load(&project);
        let database = Database::load(&project);
        let story = StoryEditor::load(&project);
        use_project_font(&project, ctx);
        let language = strings.languages.first().cloned().unwrap_or_default();
        let problem = strings
            .broken
            .as_ref()
            .or(database.broken.first())
            .or(story.broken.as_ref())
            .or(catalog.problems.first());
        let status = match problem {
            Some(problem) => (format!("Could not read {problem}"), true),
            None => (
                format!("{}: pick a map on the left.", project.settings.name),
                false,
            ),
        };
        let first = |list: &[String], like: &str| {
            list.iter()
                .find(|s| s.contains(like))
                .or(list.first())
                .cloned()
                .unwrap_or_default()
        };
        let mut editor = Self {
            workspace: Workspace::Maps,
            database,
            story,
            sheets: SheetEditor::new(Vec::new()),
            tile,
            palette_sheet: first(&catalog.props, "town_props"),
            npc_sheet: first(&catalog.characters, "villager"),
            place_scene: String::new(),
            enemy_kind: catalog
                .enemies
                .first()
                .map(|(k, _)| k.clone())
                .unwrap_or_default(),
            catalog,
            viewport,
            strings,
            language,
            scene: None,
            history: History::default(),
            tool: Tool::Select,
            brush: Brush::Hill(1),
            prop: None,
            prop_solid: true,
            selected: None,
            drag: None,
            editing: None,
            view: View {
                center: Vec2::ZERO,
                zoom: 2,
            },
            mapping: None,
            wanted: (1, 1),
            overlay: false,
            grid: false,
            status,
            stale: false,
            thumbnails: HashMap::new(),
            pending: None,
            new_map: None,
            hover: None,
            quit: false,
            players: 1,
            picking: None,
            project,
        };
        // The map the game starts on, which is the one to open first.
        let start = start.or_else(|| {
            let scenes = &editor.catalog.scenes;
            let first = editor
                .project
                .settings
                .start_scene
                .clone()
                .unwrap_or_else(|| dark_assets::DEFAULT_SCENE.to_owned());
            scenes
                .iter()
                .find(|s| **s == first)
                .or(scenes.first())
                .cloned()
        });
        if let Some(start) = start {
            editor.open(start);
        }
        editor.list_sheets();
        editor
    }

    pub fn title(&self) -> String {
        let star = if self.unsaved() { " *" } else { "" };
        match &self.scene {
            Some(s) => format!(
                "{}{star} - {} - Dark Editor",
                scene_name(&s.path),
                self.project.settings.name
            ),
            None => format!("{}{star} - Dark Editor", self.project.settings.name),
        }
    }

    /// Anything not yet saved: the map, the database or text.
    fn unsaved(&self) -> bool {
        self.scene.as_ref().is_some_and(|s| s.dirty)
            || self.database.dirty()
            || self.story.dirty()
            || self.sheets.dirty()
            || self.strings.dirty()
    }

    /// The window was asked to close.
    pub fn request_quit(&mut self) {
        if self.unsaved() {
            self.pending = Some(Pending::Quit);
        } else {
            self.quit = true;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // Names on the map show in the language being written.
        self.strings.shown.set_language(&self.language);
        // Maps offer what the database holds now, new kinds of enemy and people included.
        self.catalog.enemies = self.database.enemies();
        self.catalog.actors = self.database.actors();
        self.catalog.regions = self.database.regions();
        self.shortcuts(&ctx);
        Panel::top("tools").show(ui, |ui| self.toolbar(ui));
        Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        match self.workspace {
            Workspace::Maps => {
                Panel::left("maps")
                    .resizable(true)
                    .default_size(240.0)
                    .min_size(200.0)
                    .show(ui, |ui| self.left_panel(ui));
                // Wide enough for any selection, so the map does not shift as it changes.
                Panel::right("inspector")
                    .resizable(true)
                    .default_size(320.0)
                    .min_size(320.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
                    });
                CentralPanel::no_frame().show(ui, |ui| self.map_view(ui));
            }
            Workspace::Database => {
                // "Used by" sees the open map and the story as they are now, saved or not.
                let elsewhere = Elsewhere {
                    scene: self.scene.as_ref().map(|s| (s.path.as_str(), &s.def)),
                    story: self.story.def(),
                };
                self.database.ui(
                    ui,
                    &self.project,
                    &self.catalog,
                    &mut self.strings,
                    &self.language,
                    &elsewhere,
                );
            }
            Workspace::Sheets => {
                self.sheets.ui(ui, &self.project);
                // A sheet just made reaches the maps at once.
                self.reload_sheets();
            }
            Workspace::Story => {
                let lists = self.database.lists(&mut self.strings, &self.language);
                self.story.ui(ui, &lists, &mut self.strings, &self.language);
            }
        }
        self.dialogs(&ctx);
    }

    /// Draws the map for this frame, after the interface has laid it out.
    pub fn render(&mut self, egui: &mut egui_wgpu::Renderer) {
        if self.workspace != Workspace::Maps {
            return;
        }
        let Some(scene) = &self.scene else {
            return;
        };
        if self.stale {
            self.stale = false;
            if let Err(err) = self.viewport.rebuild(
                &self.project,
                &scene.path,
                &scene.def,
                self.tile,
                &scene.landings,
            ) {
                self.status = (format!("This map cannot be built: {err}"), true);
            }
        }
        let def = &scene.def;
        // What this map holds, and what the places stamped on it brought with them: the built
        // map has both, and a stamped town's villagers must be seen even though they are not
        // this scene's to move (docs/PLAN.md §24.5).
        // Copied out rather than borrowed, because the viewport is drawn into just below and a
        // stamped villager is only a few numbers.
        let people: Vec<(String, (f32, f32), Facing)> = {
            let built = self.viewport.map.as_ref().map_or(def, |map| &map.def);
            built
                .npcs
                .iter()
                .map(|n| (n.sheet.clone(), n.position, n.facing))
                .chain(built.enemies.iter().filter_map(|e| {
                    Some((
                        self.catalog.enemy_sheet(&e.kind)?.to_owned(),
                        e.position,
                        e.facing,
                    ))
                }))
                .collect()
        };
        let mut figures: Vec<Figure> = people
            .iter()
            .map(|(sheet, at, facing)| Figure {
                sheet,
                at: *at,
                facing: *facing,
            })
            .collect();
        figures.extend(def.player.iter().map(|p| Figure {
            sheet: &p.sheet,
            at: p.spawn,
            facing: Facing::Down,
        }));
        let origin = self.mapping.map_or(Vec2::ZERO, |m| m.origin);
        self.viewport
            .render(egui, self.wanted, origin, &figures, self.overlay);
    }

    // --- Files -----------------------------------------------------------------------------

    fn request_open(&mut self, path: String) {
        // Nothing could be changed while picking, so the map being picked on is left as is.
        self.picking = None;
        if self.scene.as_ref().is_some_and(|s| s.dirty) {
            self.pending = Some(Pending::Open(path));
        } else {
            self.open(path);
        }
    }

    fn open(&mut self, path: String) {
        match self.project.load_scene(&path) {
            Ok(def) => {
                self.view.center = (Vec2::from(def.size) / 2.0).round();
                // Where the other maps' exits arrive here: the game keeps them clear.
                let others: Vec<(&String, SceneDef)> = self
                    .catalog
                    .scenes
                    .iter()
                    .filter(|s| **s != path)
                    .filter_map(|s| Some((s, self.project.load_scene(s).ok()?)))
                    .collect();
                let landings = Map::landings(&path, others.iter().map(|(p, d)| (p.as_str(), d)));
                self.scene = Some(OpenScene {
                    path: path.clone(),
                    def,
                    dirty: false,
                    landings,
                });
                self.history.clear();
                self.selected = None;
                self.drag = None;
                self.editing = None;
                self.stale = true;
                self.status = (format!("Opened {}.", scene_name(&path)), false);
            }
            Err(err) => self.status = (format!("Cannot open {path}: {err}"), true),
        }
    }

    /// Saves everything changed: the text, the database, the map; then checks the game will
    /// load them. False if something could not be written.
    fn save(&mut self) -> bool {
        // The text first: data naming text that was never written would show its keys.
        if let Err(err) = self.strings.save(&self.project) {
            self.status = (
                format!("Not saved: the text cannot be written: {err}"),
                true,
            );
            return false;
        }
        // Each part is saved on its own: one that cannot be written does not keep the others.
        let mut failed = Vec::new();
        let mut warnings = Vec::new();
        if self.database.dirty() {
            match self.database.save(&self.project) {
                Ok(refused) => warnings.extend(
                    refused.map(|e| format!("the game will not accept the database yet: {e}")),
                ),
                Err(err) => failed.push(format!("the database is not saved: {err}")),
            }
        }
        if self.story.dirty() {
            let lists = self.database.lists(&mut self.strings, &self.language);
            match self.story.save(&self.project, &lists) {
                Ok(refused) => warnings.extend(
                    refused.map(|e| format!("the game will not accept the story yet: {e}")),
                ),
                Err(err) => failed.push(format!("the story is not saved: {err}")),
            }
        }
        if self.sheets.dirty() {
            match self.sheets.save(&self.project) {
                Ok(refused) => warnings.extend(refused),
                Err(err) => failed.push(format!("the sheet is not saved: {err}")),
            }
            self.reload_sheets();
        }
        let mut saved = "Saved.".to_owned();
        if let Some(scene) = self.scene.as_mut().filter(|s| s.dirty) {
            ops::compact_terrain(&mut scene.def, self.tile);
            match self.project.save_scene(&scene.path, &scene.def) {
                Ok(()) => {
                    scene.dirty = false;
                    saved = format!("Saved {}.", scene_name(&scene.path));
                }
                Err(err) => failed.push(format!("the map is not saved: {err}")),
            }
        }
        if let Err(e) = self.check() {
            warnings.push(format!("the game will not load this map yet: {e}"));
        }
        self.status = if !failed.is_empty() {
            (capitalised(&failed.join("; ")), true)
        } else if !warnings.is_empty() {
            (format!("{saved} But {}.", warnings.join("; ")), true)
        } else {
            (saved, false)
        };
        failed.is_empty()
    }

    /// Sheets written since the last look: the maps load them again, and the lists follow.
    fn reload_sheets(&mut self) {
        let saved = std::mem::take(&mut self.sheets.saved);
        if saved.is_empty() {
            return;
        }
        for path in &saved {
            self.viewport.forget_sheet(path);
            self.thumbnails.remove(path);
        }
        let catalog = Catalog::load(&self.project, &mut self.viewport);
        self.catalog = catalog;
        self.list_sheets();
        // What the tools had chosen may be gone (a sheet that no longer loads).
        if !self.catalog.props.contains(&self.palette_sheet) {
            self.palette_sheet = self.catalog.props.first().cloned().unwrap_or_default();
        }
        if !self.catalog.characters.contains(&self.npc_sheet) {
            self.npc_sheet = self.catalog.characters.first().cloned().unwrap_or_default();
        }
        if self
            .prop
            .as_ref()
            .is_some_and(|(sheet, _)| !self.catalog.props.contains(sheet))
        {
            self.prop = None;
        }
        self.stale = true;
    }

    /// Every sheet file of the project, loading or not (one that does not is to be fixed).
    fn list_sheets(&mut self) {
        let loaded = |path: &String| {
            self.catalog.characters.contains(path)
                || self.catalog.props.contains(path)
                || self.catalog.attacks.contains(path)
        };
        let list = crate::catalog::sheet_files(&self.project);
        self.sheets.broken = list.iter().filter(|p| !loaded(p)).cloned().collect();
        self.sheets.list = list;
    }

    /// Opens the map exit `exit` leads to, to click where it arrives.
    fn start_picking(&mut self, exit: usize) {
        let Some(scene) = &self.scene else {
            return;
        };
        if scene.dirty {
            self.status = ("Save this map first (Ctrl+S), then pick.".into(), true);
            return;
        }
        let Some(to) = scene.def.exits.get(exit).map(|e| e.to.clone()) else {
            return;
        };
        let from = scene.path.clone();
        self.open(to.clone());
        // A map that did not open (its status says why) cannot be picked on.
        if self.scene.as_ref().is_none_or(|s| s.path != to) {
            return;
        }
        self.picking = Some((from.clone(), exit));
        self.tool = Tool::Select;
        self.status = (
            format!(
                "Click where people arriving from {} appear (Esc to stop).",
                scene_name(&from)
            ),
            false,
        );
    }

    /// Sets the picked arrival in the exit's own map (saved at once), and goes back to it.
    fn finish_picking(&mut self, at: Vec2) {
        let Some((from, exit)) = self.picking.take() else {
            return;
        };
        let result = self
            .project
            .load_scene(&from)
            .map_err(|e| e.to_string())
            .and_then(|mut def| {
                let e = def.exits.get_mut(exit).ok_or("the exit is gone")?;
                e.spawn = (at.x.round(), at.y.round());
                self.project
                    .save_scene(&from, &def)
                    .map_err(|e| e.to_string())
            });
        self.open(from.clone());
        self.selected = Some(Thing::Exit(exit));
        self.status = match result.and_then(|()| self.check()) {
            Ok(()) => (
                format!(
                    "Set where people arriving from {} appear.",
                    scene_name(&from)
                ),
                false,
            ),
            Err(e) => (format!("Set, but the game will not load it yet: {e}"), true),
        };
    }

    /// Undoes the last change in what is open: the map or the database.
    fn undo_any(&mut self) {
        match self.workspace {
            Workspace::Maps => self.undo(),
            Workspace::Database => self.database.undo_step(),
            Workspace::Story => self.story.undo_step(),
            Workspace::Sheets => self.sheets.undo_step(&self.project),
        }
    }

    fn redo_any(&mut self) {
        match self.workspace {
            Workspace::Maps => self.redo(),
            Workspace::Database => self.database.redo_step(),
            Workspace::Story => self.story.redo_step(),
            Workspace::Sheets => self.sheets.redo_step(&self.project),
        }
    }

    /// Whether the game loads the saved map (and every map it leads to).
    fn check(&self) -> Result<(), String> {
        let Some(scene) = &self.scene else {
            return Ok(());
        };
        Maps::load(&self.project, &scene.path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Saves, then starts the game on this map (from its player start, if it has one).
    fn play(&mut self) {
        if self.unsaved() && !self.save() {
            return;
        }
        if let Err(err) = self.check() {
            self.status = (format!("The game will not load this map yet: {err}"), true);
            return;
        }
        let exe = std::env::current_exe()
            .ok()
            .map(|p| p.with_file_name(format!("dark-player{}", std::env::consts::EXE_SUFFIX)));
        let Some(exe) = exe.filter(|p| p.exists()) else {
            self.status = (
                "The game is not built next to the editor: run `cargo build -p dark-player`."
                    .into(),
                true,
            );
            return;
        };
        let mut command = std::process::Command::new(exe);
        command
            .arg("--project")
            .arg(self.project.root())
            .args(["--lang", &self.language]);
        // More than one player: a host, and a window of its own for each other player, joined.
        if self.players > 1 {
            let others = (self.players - 1).to_string();
            command.args(["--host", PLAYTEST_PORT, "--clients", &others]);
        }
        let here = self
            .scene
            .as_ref()
            .filter(|s| s.def.player.is_some())
            .map(|s| s.path.clone());
        match &here {
            Some(path) => {
                command.args(["--scene", path]);
            }
            None => {
                self.status = (
                    "This map has no player start, so the game starts on the first map.".into(),
                    false,
                );
            }
        }
        match command.spawn() {
            Ok(_) => {
                if here.is_some() {
                    self.status = ("Playing.".into(), false);
                }
            }
            Err(err) => self.status = (format!("Cannot start the game: {err}"), true),
        }
    }

    fn create_map(&mut self, name: &str, cols: u32, rows: u32) {
        let path = format!("scenes/{name}.ron");
        if self.project.path(&path).exists() {
            self.status = (format!("There is already a map called {name}."), true);
            return;
        }
        let ground = self
            .scene
            .as_ref()
            .map(|s| s.def.ground.sheet.clone())
            .or_else(|| {
                self.catalog
                    .props
                    .iter()
                    .find(|p| p.contains("ground"))
                    .cloned()
            })
            .or_else(|| self.catalog.props.first().cloned());
        let Some(ground) = ground else {
            self.status = (
                "The project has no sheets to make ground from.".into(),
                true,
            );
            return;
        };
        let def = ops::blank_scene(&ground, cols, rows, self.tile);
        if let Err(err) = self.project.save_scene(&path, &def) {
            self.status = (format!("Cannot create {name}: {err}"), true);
            return;
        }
        self.catalog.scenes.push(path.clone());
        self.catalog.scenes.sort();
        self.open(path);
        self.status = (
            format!("Made {name}. Add an exit from another map to reach it."),
            false,
        );
    }

    // --- Editing ---------------------------------------------------------------------------

    /// Records the map as it is, before a change.
    fn record(&mut self) {
        if let Some(scene) = &mut self.scene {
            self.history.record(&scene.def);
            scene.dirty = true;
        }
        self.editing = None;
        self.stale = true;
    }

    fn undo(&mut self) {
        if let Some(scene) = &mut self.scene
            && self.history.undo(&mut scene.def)
        {
            scene.dirty = true;
            self.selected = None;
            self.stale = true;
            self.editing = None;
        }
    }

    fn redo(&mut self) {
        if let Some(scene) = &mut self.scene
            && self.history.redo(&mut scene.def)
        {
            scene.dirty = true;
            self.selected = None;
            self.stale = true;
            self.editing = None;
        }
    }

    fn remove_selected(&mut self) {
        let Some(thing) = self.selected else {
            return;
        };
        if thing == Thing::PlayerStart {
            self.status = (
                "The player start can be moved, not removed (use Remove on the right).".into(),
                false,
            );
            return;
        }
        self.record();
        if let Some(scene) = &mut self.scene {
            ops::remove(&mut scene.def, thing);
        }
        self.selected = None;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let pressed = |ctx: &egui::Context, modifiers, key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
        };
        if pressed(ctx, Modifiers::COMMAND, Key::S) {
            self.save();
        }
        // While typing, Ctrl+Z undoes the typing (the field's own undo), not the last change.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.picking.is_some() {
            if pressed(ctx, Modifiers::NONE, Key::Escape)
                && let Some((from, _)) = self.picking.take()
            {
                self.open(from);
            }
            return;
        }
        if pressed(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
            || pressed(ctx, Modifiers::COMMAND, Key::Y)
        {
            self.redo_any();
        }
        if pressed(ctx, Modifiers::COMMAND, Key::Z) {
            self.undo_any();
        }
        if self.workspace != Workspace::Maps {
            return;
        }
        if pressed(ctx, Modifiers::NONE, Key::Delete)
            || pressed(ctx, Modifiers::NONE, Key::Backspace)
        {
            self.remove_selected();
        }
        if pressed(ctx, Modifiers::NONE, Key::Escape) {
            self.selected = None;
            self.drag = None;
        }
    }

    // --- Panels ----------------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (workspace, label, help) in [
                (
                    Workspace::Maps,
                    "Maps",
                    "Make maps: ground, props, villagers, enemies, exits",
                ),
                (
                    Workspace::Database,
                    "Database",
                    "Items, enemies, movesets, people, factions, regions",
                ),
                (
                    Workspace::Story,
                    "Story",
                    "Conversations with the people of the world, and the year's endings",
                ),
                (
                    Workspace::Sheets,
                    "Sheets",
                    "How pictures are cut into frames and clips, and where attacks hit",
                ),
            ] {
                let chosen = self.workspace == workspace;
                let tab = egui::Button::new(egui::RichText::new(label).strong()).selected(chosen);
                if ui.add(tab).on_hover_text(help).clicked() {
                    self.workspace = workspace;
                }
            }
            ui.separator();
            if ui
                .add_enabled(self.unsaved(), egui::Button::new("Save"))
                .on_hover_text("Save everything (Ctrl+S)")
                .clicked()
            {
                self.save();
            }
            let (can_undo, can_redo) = match self.workspace {
                Workspace::Maps => (self.history.can_undo(), self.history.can_redo()),
                Workspace::Database => (self.database.can_undo(), self.database.can_redo()),
                Workspace::Story => (self.story.can_undo(), self.story.can_redo()),
                Workspace::Sheets => (self.sheets.can_undo(), self.sheets.can_redo()),
            };
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                self.undo_any();
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Y")
                .clicked()
            {
                self.redo_any();
            }
            ui.separator();
            let maps = self.workspace == Workspace::Maps;
            for (tool, label, help) in Tool::ALL.into_iter().filter(|_| maps) {
                if ui
                    .selectable_label(self.tool == tool, label)
                    .on_hover_text(help)
                    .clicked()
                {
                    self.tool = tool;
                    self.drag = None;
                }
            }
            if maps {
                ui.separator();
            }
            if ui
                .add_enabled(self.scene.is_some(), egui::Button::new("▶ Play"))
                .on_hover_text("Save, then play the open map from its player start")
                .clicked()
            {
                self.play();
            }
            ComboBox::from_id_salt("players")
                .selected_text(match self.players {
                    1 => "1 player".to_owned(),
                    n => format!("{n} players"),
                })
                .width(80.0)
                .show_ui(ui, |ui| {
                    for n in 1..=4u8 {
                        let label = if n == 1 {
                            "1 player".to_owned()
                        } else {
                            format!("{n} players")
                        };
                        ui.selectable_value(&mut self.players, n, label);
                    }
                })
                .response
                .on_hover_text("Play together on this computer: one window per player");
            ui.separator();
            if maps {
                ui.checkbox(&mut self.grid, "Grid");
                ui.checkbox(&mut self.overlay, "Collision").on_hover_text(
                    "Show what blocks walking, hills and exits as the game sees them",
                );
            }
            ComboBox::from_id_salt("language")
                .selected_text(format!("Text: {}", self.language))
                .show_ui(ui, |ui| {
                    for code in &self.strings.languages {
                        ui.selectable_value(&mut self.language, code.clone(), code.as_str());
                    }
                })
                .response
                .on_hover_text("The language you are writing text in");
        });
        if self.workspace != Workspace::Maps {
            ui.add_space(2.0);
            return;
        }
        // The chosen tool's options.
        ui.horizontal_wrapped(|ui| {
            let help = Tool::ALL
                .iter()
                .find(|(t, ..)| *t == self.tool)
                .map_or("", |(_, _, h)| h);
            match self.tool {
                Tool::Terrain => {
                    ui.label("Brush:");
                    for (brush, label) in Brush::ALL {
                        ui.selectable_value(&mut self.brush, brush, label);
                    }
                }
                Tool::Prop => {
                    ui.checkbox(&mut self.prop_solid, "Solid")
                        .on_hover_text("Nobody can walk through it");
                    match &self.prop {
                        Some((sheet, frame)) => {
                            ui.label(format!("Placing {} #{frame}", sheet_name(sheet)));
                        }
                        None => {
                            ui.label("Pick a picture in the palette on the left.");
                        }
                    }
                }
                Tool::Npc => {
                    ui.label("Looks like:");
                    sheet_combo(
                        ui,
                        "npc sheet",
                        &mut self.npc_sheet,
                        &self.catalog.characters,
                    );
                }
                Tool::Place => {
                    ui.label("Stamping:");
                    let here = self.scene.as_ref().map(|s| s.path.clone());
                    ComboBox::from_id_salt("place scene")
                        .selected_text(if self.place_scene.is_empty() {
                            "(choose one)".to_owned()
                        } else {
                            scene_name(&self.place_scene).to_owned()
                        })
                        .show_ui(ui, |ui| {
                            // Every scene but this one: a map cannot be stamped on itself.
                            for scene in self
                                .catalog
                                .scenes
                                .iter()
                                .filter(|scene| Some(*scene) != here.as_ref())
                            {
                                ui.selectable_value(
                                    &mut self.place_scene,
                                    scene.clone(),
                                    scene_name(scene),
                                );
                            }
                        });
                }
                Tool::Enemy => {
                    ui.label("Kind:");
                    ComboBox::from_id_salt("enemy kind")
                        .selected_text(self.enemy_kind.as_str())
                        .show_ui(ui, |ui| {
                            for (kind, _) in &self.catalog.enemies {
                                ui.selectable_value(&mut self.enemy_kind, kind.clone(), kind);
                            }
                        });
                }
                _ => {}
            }
            ui.weak(help);
        });
        ui.add_space(2.0);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let (text, error) = &self.status;
            if *error {
                ui.colored_label(Color32::from_rgb(255, 110, 100), text);
            } else {
                ui.label(text);
            }
            if self.workspace != Workspace::Maps {
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("zoom {}x", self.view.zoom));
                if let Some(at) = self.hover {
                    let tile = self.tile as f32;
                    ui.label(format!(
                        "x {:.0}  y {:.0}  (tile {}, {})",
                        at.x,
                        at.y,
                        (at.x / tile).floor(),
                        (at.y / tile).floor()
                    ));
                }
            });
        });
    }

    fn left_panel(&mut self, ui: &mut Ui) {
        ui.heading("Maps");
        let current = self.scene.as_ref().map(|s| s.path.clone());
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("maps")
            .max_height(160.0)
            .show(ui, |ui| {
                for path in &self.catalog.scenes {
                    let here = current.as_deref() == Some(path.as_str());
                    if ui.selectable_label(here, scene_name(path)).clicked() && !here {
                        open = Some(path.clone());
                    }
                }
            });
        if let Some(path) = open {
            self.request_open(path);
        }
        if ui.button("New map…").clicked() {
            self.new_map = Some(NewMap {
                name: String::new(),
                cols: 40,
                rows: 30,
                focused: false,
            });
        }
        ui.separator();
        self.minimap(ui);
        ui.separator();
        ui.heading("Palette");
        sheet_combo(ui, "palette", &mut self.palette_sheet, &self.catalog.props);
        let sheet = self.palette_sheet.clone();
        let Some(texture) = self.thumbnail(ui.ctx(), &sheet) else {
            return;
        };
        let Some(loaded) = self.viewport.sheets.get(&sheet) else {
            return;
        };
        let size = vec2(loaded.image.width as f32, loaded.image.height as f32);
        let frames: Vec<Rect> = loaded
            .sheet
            .frames
            .iter()
            .map(|f| {
                Rect::from_min_size(
                    pos2(f.rect.x as f32, f.rect.y as f32),
                    vec2(f.rect.w as f32, f.rect.h as f32),
                )
            })
            .collect();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("palette")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                    for (i, frame) in frames.iter().enumerate() {
                        let uv = Rect::from_min_max(
                            (frame.min.to_vec2() / size).to_pos2(),
                            (frame.max.to_vec2() / size).to_pos2(),
                        );
                        let fit = 44.0 / frame.width().max(frame.height()).max(1.0);
                        let shown = frame.size() * fit.min(3.0);
                        let image = egui::Image::new((texture.id(), shown)).uv(uv);
                        let chosen = self.prop.as_ref() == Some(&(sheet.clone(), i as u32));
                        let button = egui::Button::image(image)
                            .selected(chosen)
                            .min_size(vec2(48.0, 48.0));
                        if ui
                            .add(button)
                            .on_hover_text(format!("#{i} ({}x{})", frame.width(), frame.height()))
                            .clicked()
                        {
                            picked = Some(i as u32);
                        }
                    }
                });
            });
        if let Some(frame) = picked {
            self.prop = Some((sheet, frame));
            self.tool = Tool::Prop;
        }
    }

    /// The whole map in a small box, with the part being worked on outlined: on a map larger
    /// than a few screens it is the only way to tell where the view is (docs/PLAN.md §24).
    /// A click or a drag inside it looks there.
    fn minimap(&mut self, ui: &mut Ui) {
        // Nothing to show before a map is open: the panel keeps its room for the palette.
        let Some(scene) = &self.scene else {
            return;
        };
        let def = &scene.def;
        ui.heading("Minimap");
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), MINIMAP_HEIGHT),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, Color32::from_rgb(12, 12, 14));
        let fit = Fit::new(rect.shrink(4.0), Vec2::from(def.size));
        // The ground, then the land raised above it: a shape a designer recognises at a glance.
        painter.rect_filled(fit.shown, 0.0, Color32::from_rgb(38, 46, 38));
        let tile = self.tile as f32;
        for fill in &def.terrain.fill {
            let (col, row, cols, rows) = fill.tiles;
            let at = (
                col as f32 * tile,
                row as f32 * tile,
                cols as f32 * tile,
                rows as f32 * tile,
            );
            let ink = match fill.cell {
                Cell::Wall => Color32::from_rgb(90, 80, 74),
                Cell::Floor => Color32::from_rgb(38, 46, 38),
                Cell::Level(n) => {
                    let lift = 28 + 22 * u32::from(n).min(5) as u8;
                    Color32::from_rgb(52 + lift, 58 + lift / 2, 44 + lift / 3)
                }
            };
            painter.rect_filled(fit.rect(at), 0.0, ink);
        }
        // The props a designer placed by hand, faintly: landmarks, not the scattered undergrowth.
        for prop in &def.props {
            painter.circle_filled(
                fit.to_screen(Vec2::from(prop.position)),
                1.0,
                Color32::from_rgb(120, 116, 104),
            );
        }
        for inn in &def.inns {
            painter.rect_filled(fit.rect(inn.area), 0.0, Color32::from_rgb(110, 170, 255));
        }
        for exit in &def.exits {
            painter.rect_filled(fit.rect(exit.area), 0.0, Color32::from_rgb(255, 220, 60));
        }
        let dot = |at: (f32, f32), ink: Color32| {
            painter.circle_filled(fit.to_screen(Vec2::from(at)), 1.5, ink);
        };
        for npc in &def.npcs {
            dot(npc.position, Color32::WHITE);
        }
        for enemy in &def.enemies {
            dot(enemy.position, Color32::from_rgb(255, 110, 100));
        }
        if let Some(player) = &def.player {
            dot(player.spawn, Color32::from_rgb(120, 230, 120));
        }
        // What the map view is looking at. On a large map that is a sliver, so it is drawn with
        // the same least size as everything else here: an outline too thin to see is no use to
        // the one person who needs it.
        let seen = Vec2::new(self.wanted.0 as f32, self.wanted.1 as f32);
        let corner = self.view.center - seen / 2.0;
        let looking = fit.rect((corner.x, corner.y, seen.x, seen.y));
        painter.rect_stroke(
            looking,
            0.0,
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
        painter.rect_stroke(
            fit.shown,
            0.0,
            Stroke::new(1.0, Color32::from_white_alpha(50)),
            StrokeKind::Outside,
        );
        // Dragging inside it carries the view along; the map itself is not edited from here, and
        // the other buttons are left alone, as they are on the map itself.
        if let Some(at) = response.interact_pointer_pos()
            && (response.dragged_by(PointerButton::Primary) || response.clicked())
        {
            self.look_at(fit.to_world(at));
        }
    }

    /// Looks at a place on the map. The middle of the view stays on the map, so the edge of one
    /// can be worked on with the edge down the middle of the screen, and panning can no longer
    /// wander off into nothing and leave a designer hunting for their own map.
    fn look_at(&mut self, at: Vec2) {
        let size = self
            .scene
            .as_ref()
            .map_or(Vec2::ZERO, |scene| Vec2::from(scene.def.size));
        self.view.center = minimap::keep_inside(at, size).round();
    }

    /// The whole image of `sheet`, for egui to draw pieces of.
    fn thumbnail(&mut self, ctx: &egui::Context, sheet: &str) -> Option<egui::TextureHandle> {
        if let Some(handle) = self.thumbnails.get(sheet) {
            return Some(handle.clone());
        }
        let loaded = self.viewport.sheets.get(sheet)?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [loaded.image.width as usize, loaded.image.height as usize],
            &loaded.image.rgba,
        );
        let handle = ctx.load_texture(sheet, image, egui::TextureOptions::NEAREST);
        self.thumbnails.insert(sheet.to_owned(), handle.clone());
        Some(handle)
    }

    fn inspector(&mut self, ui: &mut Ui) {
        if let Some((from, _)) = &self.picking {
            ui.heading("Picking where people arrive");
            ui.label(format!(
                "Click on this map where people coming from {} appear.",
                scene_name(from)
            ));
            if ui.button("Stop").clicked() {
                let from = from.clone();
                self.picking = None;
                self.open(from);
            }
            return;
        }
        let Some(scene) = &mut self.scene else {
            ui.label("No map is open.");
            return;
        };
        let before = scene.def.clone();
        let name = scene_name(&scene.path).to_owned();
        let mut form = Form {
            changed: None,
            step: false,
            text_changed: false,
            palette: self.prop.clone(),
            pick_arrival: None,
            strings: &mut self.strings,
            language: &self.language,
            catalog: &self.catalog,
            scene: &name,
            tile: self.tile,
        };
        let mut remove = false;
        match self.selected {
            None => {
                ui.heading(format!("Map: {name}"));
                form.map(ui, &mut scene.def);
            }
            Some(thing) => {
                ui.horizontal(|ui| {
                    ui.heading(thing_label(thing));
                    if ui.button("Remove").clicked() {
                        remove = true;
                    }
                    if ui.button("Done").clicked() {
                        self.selected = None;
                    }
                });
                if let Some(thing) = self.selected {
                    form.thing(ui, &mut scene.def, thing, &self.viewport);
                }
            }
        }
        let (changed, text_changed) = (form.changed, form.text_changed);
        let (step, pick_arrival) = (form.step, form.pick_arrival);
        if let Some(id) = changed {
            if self.editing != Some(id) || step {
                self.history.record(&before);
            }
            self.editing = (!step).then_some(id);
            scene.dirty = true;
            self.stale = true;
        }
        if text_changed {
            scene.dirty = true;
        }
        if let Some(exit) = pick_arrival {
            self.start_picking(exit);
            return;
        }
        if remove {
            match self.selected {
                Some(Thing::PlayerStart) => {
                    self.record();
                    if let Some(scene) = &mut self.scene {
                        scene.def.player = None;
                    }
                    self.selected = None;
                }
                _ => self.remove_selected(),
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(pending) = self.pending.clone() {
            let name = self
                .scene
                .as_ref()
                .map(|s| scene_name(&s.path).to_owned())
                .unwrap_or_default();
            let mut answer = None;
            egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label(match pending {
                    Pending::Quit => "Save your changes before closing?".to_owned(),
                    _ => format!("Save the changes to {name}?"),
                });
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        answer = Some(Some(true));
                    }
                    if ui.button("Don't save").clicked() {
                        answer = Some(Some(false));
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(None);
                    }
                });
            });
            if let Some(answer) = answer {
                self.pending = None;
                let go = match answer {
                    Some(true) => self.save(),
                    Some(false) => {
                        // What was typed goes too, or the next save would write it; unless
                        // the database or the story, still open, names things with it.
                        if !self.database.dirty() && !self.story.dirty() {
                            self.strings = Strings::load(&self.project);
                        }
                        true
                    }
                    None => false,
                };
                // A warning from that save outlives the "Opened" or "Made" that follows.
                let warning = (answer == Some(true) && self.status.1).then(|| self.status.clone());
                if go {
                    match pending {
                        Pending::Open(path) => self.open(path),
                        Pending::NewMap { name, cols, rows } => self.create_map(&name, cols, rows),
                        Pending::Quit => self.quit = true,
                    }
                }
                if let Some(warning) = warning {
                    self.status = warning;
                }
            }
        }
        if let Some(new) = &mut self.new_map {
            let mut create = None;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("new map")).show(ctx, |ui| {
                ui.heading("New map");
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    let field = ui.text_edit_singleline(&mut new.name);
                    if !new.focused {
                        field.request_focus();
                        new.focused = true;
                    }
                });
                new.name
                    .retain(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                new.name.make_ascii_lowercase();
                ui.horizontal(|ui| {
                    ui.label("Size in tiles:");
                    let most = PAINTABLE_TILES;
                    ui.add(DragValue::new(&mut new.cols).range(8..=most));
                    ui.label("by");
                    ui.add(DragValue::new(&mut new.rows).range(8..=most));
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!new.name.is_empty(), egui::Button::new("Create"))
                        .clicked()
                    {
                        create = Some((new.name.clone(), new.cols, new.rows));
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                self.new_map = None;
            }
            if let Some((name, cols, rows)) = create {
                self.new_map = None;
                if self.scene.as_ref().is_some_and(|s| s.dirty) {
                    self.pending = Some(Pending::NewMap { name, cols, rows });
                } else {
                    self.create_map(&name, cols, rows);
                }
            }
        }
    }

    // --- The map ---------------------------------------------------------------------------

    fn map_view(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 14));
        if self.scene.is_none() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Pick a map on the left, or make a new one.",
                FontId::proportional(18.0),
                Color32::GRAY,
            );
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let (mapping, size) = Mapping::new(rect, ppp, self.view);
        self.mapping = Some(mapping);
        self.wanted = size;
        let shown =
            Rect::from_min_size(rect.min, vec2(size.0 as f32, size.1 as f32) * mapping.scale);
        painter.image(
            self.viewport.image,
            shown,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        self.hover = response.hover_pos().map(|p| mapping.to_world(p));
        self.pointer(ui, &response, &mapping);
        self.markers(&painter, &mapping);
        if let Some((from, _)) = &self.picking {
            let text = format!(
                "Click where people arriving from {} appear",
                scene_name(from)
            );
            let galley = painter.layout_no_wrap(text, FontId::proportional(16.0), Color32::WHITE);
            let at =
                Align2::CENTER_TOP.anchor_size(rect.center_top() + vec2(0.0, 12.0), galley.size());
            painter.rect_filled(
                at.expand(6.0),
                4.0,
                Color32::from_rgba_unmultiplied(20, 60, 20, 220),
            );
            painter.galley(at.min, galley, Color32::WHITE);
        }
    }

    fn pointer(&mut self, ui: &Ui, response: &egui::Response, mapping: &Mapping) {
        // Zoom about the cursor.
        if let Some(at) = self.hover {
            let scroll: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                        _ => None,
                    })
                    .sum()
            });
            let zoom = self.view.zoom;
            let next = if scroll > 0.0 {
                (zoom + 1).min(MAX_ZOOM)
            } else if scroll < 0.0 {
                zoom.saturating_sub(1).max(1)
            } else {
                zoom
            };
            if next != zoom {
                self.look_at(at + (self.view.center - at) * zoom as f32 / next as f32);
                self.view.zoom = next;
            }
        }
        // Pan with the middle button, or the right one where it has no other use.
        let pans_right = !matches!(self.tool, Tool::Terrain);
        if response.dragged_by(PointerButton::Middle)
            || (pans_right && response.dragged_by(PointerButton::Secondary))
        {
            let delta = response.drag_delta() / mapping.scale;
            self.look_at(self.view.center - Vec2::new(delta.x, delta.y));
            self.drag = Some(Drag::Pan);
            return;
        }
        if matches!(self.drag, Some(Drag::Pan)) {
            self.drag = None;
        }
        let Some(at) = self.hover else {
            if response.drag_stopped() {
                self.finish_drag(None);
            }
            return;
        };
        // Picking where an exit arrives: a click here is the answer.
        if self.picking.is_some() {
            if response.clicked() {
                self.finish_picking(at);
            }
            return;
        }
        let primary_down = response.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.button_down(PointerButton::Primary));
        let secondary_down = response.is_pointer_button_down_on()
            && ui.input(|i| i.pointer.button_down(PointerButton::Secondary));
        match self.tool {
            Tool::Select => {
                if response.drag_started_by(PointerButton::Primary) {
                    let from = ui
                        .input(|i| i.pointer.press_origin())
                        .map_or(at, |p| mapping.to_world(p));
                    self.selected = self.pick(from);
                    if let Some(thing) = self.selected
                        && let Some(scene) = &self.scene
                        && let Some(position) = ops::position_of(&scene.def, thing)
                    {
                        self.record();
                        self.drag = Some(Drag::Move {
                            thing,
                            grab: position - from,
                        });
                    }
                } else if response.clicked() {
                    self.selected = self.pick(at);
                    self.editing = None;
                }
                if let Some(Drag::Move { thing, grab }) = self.drag
                    && response.dragged_by(PointerButton::Primary)
                    && let Some(scene) = &mut self.scene
                {
                    ops::move_to(&mut scene.def, thing, at + grab);
                    self.stale = true;
                }
            }
            Tool::Terrain => {
                if primary_down || secondary_down {
                    let (from, recorded) = match self.drag {
                        Some(Drag::Paint { last, recorded }) => (last, recorded),
                        _ => (at, false),
                    };
                    let cell = if primary_down {
                        self.brush.cell()
                    } else {
                        Cell::Floor
                    };
                    let mut changed = false;
                    if let Some(scene) = &mut self.scene {
                        // One undo step per stroke, and none for a stroke that changed nothing.
                        let before = (!recorded).then(|| scene.def.clone());
                        changed = ops::paint_line(&mut scene.def, self.tile, from, at, cell);
                        if changed && let Some(before) = before {
                            self.history.record(&before);
                            scene.dirty = true;
                            self.editing = None;
                        }
                    }
                    self.stale |= changed;
                    self.drag = Some(Drag::Paint {
                        last: at,
                        recorded: recorded || changed,
                    });
                } else if matches!(self.drag, Some(Drag::Paint { .. })) {
                    self.finish_drag(Some(at));
                }
            }
            Tool::Exit | Tool::Inn => {
                if response.drag_started_by(PointerButton::Primary) {
                    let from = ui
                        .input(|i| i.pointer.press_origin())
                        .map_or(at, |p| mapping.to_world(p));
                    self.drag = Some(Drag::Area { from });
                }
            }
            Tool::Prop | Tool::Npc | Tool::Enemy | Tool::Start | Tool::Place | Tool::Erase => {
                if response.clicked() {
                    self.click(at);
                }
            }
        }
        if response.drag_stopped() {
            self.finish_drag(Some(at));
        }
    }

    fn finish_drag(&mut self, at: Option<Vec2>) {
        match self.drag.take() {
            Some(Drag::Paint { .. }) => {
                if let Some(scene) = &mut self.scene {
                    ops::compact_terrain(&mut scene.def, self.tile);
                }
            }
            Some(Drag::Area { from }) => {
                let Some(to) = at else {
                    return;
                };
                let area = ops::area(from, to, self.tile as f32);
                self.record();
                let destination = self.other_scene();
                let Some(scene) = &mut self.scene else {
                    return;
                };
                let thing = if self.tool == Tool::Inn {
                    scene.def.inns.push(ops::new_inn(area));
                    Thing::Inn(scene.def.inns.len() - 1)
                } else {
                    let (to, spawn) = destination;
                    scene.def.exits.push(ops::new_exit(area, &to, spawn));
                    Thing::Exit(scene.def.exits.len() - 1)
                };
                self.selected = Some(thing);
            }
            Some(Drag::Move { .. } | Drag::Pan) | None => {}
        }
    }

    /// Another map for a new exit to lead to, and where it lands there.
    fn other_scene(&self) -> (String, (f32, f32)) {
        let here = self.scene.as_ref().map(|s| s.path.as_str());
        let to = self
            .catalog
            .scenes
            .iter()
            .find(|s| Some(s.as_str()) != here)
            .or(self.catalog.scenes.first())
            .cloned()
            .unwrap_or_default();
        let spawn = self
            .project
            .load_scene(&to)
            .map(|d| (Vec2::from(d.size) / 2.0).round().into())
            .unwrap_or((0.0, 0.0));
        (to, spawn)
    }

    /// A click with a placing tool.
    fn click(&mut self, at: Vec2) {
        let scene_path = match &self.scene {
            Some(s) => s.path.clone(),
            None => return,
        };
        let tool = self.tool;
        let placed = match tool {
            Tool::Prop => {
                let Some((sheet, frame)) = self.prop.clone() else {
                    self.status = ("Pick a picture in the palette first.".into(), false);
                    return;
                };
                let width = self
                    .viewport
                    .sheets
                    .get(&sheet)
                    .and_then(|s| s.sheet.frames.get(frame as usize))
                    .map_or(self.tile, |f| f.rect.w);
                self.record();
                let scene = self.scene.as_mut().expect("a scene is open");
                let prop = ops::new_prop(&sheet, frame, at, width, self.prop_solid);
                scene.def.props.push(prop);
                Some(Thing::Prop(scene.def.props.len() - 1))
            }
            Tool::Place => {
                if self.place_scene.is_empty() {
                    self.status = ("Choose which place to stamp first.".into(), false);
                    return;
                }
                if self.place_scene == scene_path {
                    self.status = ("A map cannot be stamped on itself.".into(), true);
                    return;
                }
                // A place is put down on a whole tile: its ground is drawn in tiles, and the
                // engine refuses anything else (§24.5).
                let tile = self.tile as f32;
                let at = Vec2::new((at.x / tile).floor() * tile, (at.y / tile).floor() * tile);
                self.record();
                let scene = self.scene.as_mut().expect("a scene is open");
                scene.def.places.push(dark_assets::PlaceDef {
                    scene: self.place_scene.clone(),
                    at: (at.x, at.y),
                });
                Some(Thing::Place(scene.def.places.len() - 1))
            }
            Tool::Npc => {
                let used = self.scene.as_ref().map(|s| ops::keys_in(&s.def));
                let key = self
                    .strings
                    .fresh_key(&format!("npc.{}", scene_name(&scene_path)), |k| {
                        used.as_ref().is_some_and(|u| u.contains(k))
                    });
                self.record();
                let scene = self.scene.as_mut().expect("a scene is open");
                scene.def.npcs.push(ops::new_npc(&self.npc_sheet, at, key));
                Some(Thing::Npc(scene.def.npcs.len() - 1))
            }
            Tool::Enemy => {
                if self.enemy_kind.is_empty() {
                    self.status = (
                        "The project has no kinds of enemy (combat.ron).".into(),
                        true,
                    );
                    return;
                }
                self.record();
                let scene = self.scene.as_mut().expect("a scene is open");
                scene.def.enemies.push(ops::new_enemy(&self.enemy_kind, at));
                Some(Thing::Enemy(scene.def.enemies.len() - 1))
            }
            Tool::Start => {
                let look = self
                    .scene
                    .as_ref()
                    .and_then(|s| s.def.player.clone())
                    .or_else(|| self.catalog.player.clone());
                let Some(mut player) = look else {
                    self.status = ("No map has a player to copy the look of.".into(), true);
                    return;
                };
                player.spawn = (at.x.round(), at.y.round());
                self.record();
                let scene = self.scene.as_mut().expect("a scene is open");
                scene.def.player = Some(player);
                Some(Thing::PlayerStart)
            }
            Tool::Erase => {
                if let Some(thing) = self.pick(at) {
                    self.selected = Some(thing);
                    self.remove_selected();
                }
                None
            }
            _ => None,
        };
        if placed.is_some() {
            self.selected = placed;
        }
    }

    fn pick(&self, at: Vec2) -> Option<Thing> {
        let scene = self.scene.as_ref()?;
        ops::pick(
            &scene.def,
            at,
            |p| {
                self.picture(&p.sheet, p.frame, p.position)
                    .map(|r| (r.min.x, r.min.y, r.width(), r.height()))
            },
            |scene| self.catalog.sizes.get(scene).copied(),
        )
    }

    /// Where a prop's picture is drawn, in world pixels.
    fn picture(&self, sheet: &str, frame: u32, at: (f32, f32)) -> Option<Rect> {
        let frame = self
            .viewport
            .sheets
            .get(sheet)?
            .sheet
            .frames
            .get(frame as usize)?;
        let at = Vec2::from(at);
        let top_left = at - frame.pivot - Vec2::new(0.0, self.viewport.ground(at));
        Some(Rect::from_min_size(
            pos2(top_left.x, top_left.y),
            vec2(frame.rect.w as f32, frame.rect.h as f32),
        ))
    }

    /// What the game does not draw: exits, inns, the player start, names, and the selection.
    fn markers(&self, painter: &egui::Painter, m: &Mapping) {
        let Some(scene) = &self.scene else {
            return;
        };
        let def = &scene.def;
        let font = FontId::proportional(13.0);
        let label = |at: Pos2, text: &str, color: Color32| {
            let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
            let rect = Align2::CENTER_BOTTOM
                .anchor_size(at, galley.size())
                .expand(2.0);
            painter.rect_filled(rect, 3.0, Color32::from_black_alpha(160));
            painter.galley(rect.min + vec2(2.0, 2.0), galley, color);
        };
        let world_rect = |r: Rect| {
            Rect::from_min_max(
                m.to_screen(Vec2::new(r.min.x, r.min.y)),
                m.to_screen(Vec2::new(r.max.x, r.max.y)),
            )
        };
        if self.grid {
            let tile = self.tile as f32;
            let size = Vec2::from(def.size);
            let line = Stroke::new(1.0, Color32::from_white_alpha(28));
            let mut x = 0.0;
            while x <= size.x {
                painter.line_segment(
                    [
                        m.to_screen(Vec2::new(x, 0.0)),
                        m.to_screen(Vec2::new(x, size.y)),
                    ],
                    line,
                );
                x += tile;
            }
            let mut y = 0.0;
            while y <= size.y {
                painter.line_segment(
                    [
                        m.to_screen(Vec2::new(0.0, y)),
                        m.to_screen(Vec2::new(size.x, y)),
                    ],
                    line,
                );
                y += tile;
            }
        }
        let map_edge = m.rect((0.0, 0.0, def.size.0, def.size.1));
        painter.rect_stroke(
            map_edge,
            0.0,
            Stroke::new(1.0, Color32::from_white_alpha(60)),
            StrokeKind::Outside,
        );
        // Everything a stamped place brought, drawn faintly and named after it: it cannot be
        // moved here — it is moved by moving the place, or by opening the place itself.
        if let Some(built) = self.viewport.map.as_ref().map(|map| &map.def) {
            let faint = Color32::from_white_alpha(70);
            for exit in built.exits.iter().skip(def.exits.len()) {
                let r = m.rect(exit.area);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, faint), StrokeKind::Inside);
                label(
                    r.center_top(),
                    &format!("to {}", scene_name(&exit.to)),
                    faint,
                );
            }
            for inn in built.inns.iter().skip(def.inns.len()) {
                let r = m.rect(inn.area);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, faint), StrokeKind::Inside);
                label(r.center_top(), "Inn", faint);
            }
        }
        let yellow = Color32::from_rgb(255, 220, 60);
        for exit in &def.exits {
            let r = m.rect(exit.area);
            painter.rect_filled(r, 0.0, yellow.gamma_multiply(0.25));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, yellow), StrokeKind::Inside);
            label(
                r.center_top(),
                &format!("to {}", scene_name(&exit.to)),
                yellow,
            );
        }
        // The towns, camps and ruins stamped on this map: where each covers, and what it is
        // called (docs/PLAN.md §24.5). Drawn under the rest, since everything a place holds is
        // drawn inside it.
        let violet = Color32::from_rgb(200, 150, 255);
        for (nth, place) in def.places.iter().enumerate() {
            let Some(size) = self.catalog.sizes.get(&place.scene).copied() else {
                continue;
            };
            let r = m.rect((place.at.0, place.at.1, size.0, size.1));
            let chosen = self.selected == Some(Thing::Place(nth));
            painter.rect_filled(
                r,
                0.0,
                violet.gamma_multiply(if chosen { 0.22 } else { 0.10 }),
            );
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(if chosen { 2.5 } else { 1.5 }, violet),
                StrokeKind::Inside,
            );
            label(r.center_top(), scene_name(&place.scene), violet);
        }
        let blue = Color32::from_rgb(110, 170, 255);
        for inn in &def.inns {
            let r = m.rect(inn.area);
            painter.rect_filled(r, 0.0, blue.gamma_multiply(0.18));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, blue), StrokeKind::Inside);
            label(r.center_top(), "Inn", blue);
            painter.circle_stroke(
                m.to_screen(Vec2::from(inn.bed)),
                5.0,
                Stroke::new(1.5, blue),
            );
        }
        let head = |at: (f32, f32)| {
            let at = Vec2::from(at);
            m.to_screen(at - Vec2::new(0.0, 40.0 + self.viewport.ground(at)))
        };
        for npc in &def.npcs {
            let name = npc
                .name
                .as_deref()
                .map(|k| self.strings.shown.text(k).to_owned())
                .unwrap_or_else(|| "villager".into());
            label(head(npc.position), &name, Color32::WHITE);
        }
        let red = Color32::from_rgb(255, 110, 100);
        for enemy in &def.enemies {
            let drawn = self
                .catalog
                .enemy_sheet(&enemy.kind)
                .is_some_and(|s| self.viewport.draws(s));
            if !drawn {
                painter.circle_filled(m.to_screen(Vec2::from(enemy.position)), 6.0, red);
            }
            label(head(enemy.position), &enemy.kind, red);
        }
        let green = Color32::from_rgb(120, 230, 120);
        if let Some(player) = &def.player {
            let at = m.to_screen(Vec2::from(player.spawn));
            painter.circle_stroke(at, 8.0, Stroke::new(2.0, green));
            label(head(player.spawn), "Player start", green);
        }
        // The selection.
        let white = Stroke::new(2.0, Color32::WHITE);
        match self.selected {
            Some(Thing::Prop(i)) => {
                if let Some(p) = def.props.get(i)
                    && let Some(r) = self.picture(&p.sheet, p.frame, p.position)
                {
                    painter.rect_stroke(world_rect(r), 0.0, white, StrokeKind::Outside);
                }
            }
            Some(Thing::Exit(i)) => {
                if let Some(e) = def.exits.get(i) {
                    painter.rect_stroke(m.rect(e.area), 0.0, white, StrokeKind::Outside);
                    let spawn = format!("arrives at {:.0}, {:.0}", e.spawn.0, e.spawn.1);
                    label(
                        m.rect(e.area).center_bottom() + vec2(0.0, 18.0),
                        &spawn,
                        yellow,
                    );
                }
            }
            Some(Thing::Inn(i)) => {
                if let Some(inn) = def.inns.get(i) {
                    painter.rect_stroke(m.rect(inn.area), 0.0, white, StrokeKind::Outside);
                }
            }
            Some(thing) => {
                if let Some(at) = ops::position_of(def, thing) {
                    painter.circle_stroke(m.to_screen(at), 11.0, white);
                }
            }
            None => {}
        }
        // What the tool would do here.
        let Some(at) = self.hover else {
            return;
        };
        match (self.tool, &self.drag) {
            (Tool::Terrain, _) => {
                let tile = self.tile as f32;
                let corner = (at / tile).floor() * tile;
                let r = m.rect((corner.x, corner.y, tile, tile));
                painter.rect_stroke(r, 0.0, white, StrokeKind::Inside);
            }
            (Tool::Exit | Tool::Inn, Some(Drag::Area { from })) => {
                let (x, y, w, h) = ops::area(*from, at, self.tile as f32);
                let color = if self.tool == Tool::Inn { blue } else { yellow };
                painter.rect_stroke(
                    m.rect((x, y, w, h)),
                    0.0,
                    Stroke::new(2.0, color),
                    StrokeKind::Inside,
                );
            }
            (Tool::Prop, _) => {
                if let Some((sheet, frame)) = &self.prop
                    && let Some(r) = self.picture(sheet, *frame, (at.x.round(), at.y.round()))
                    && let Some(texture) = self.thumbnails.get(sheet)
                    && let Some(loaded) = self.viewport.sheets.get(sheet)
                    && let Some(f) = loaded.sheet.frames.get(*frame as usize)
                {
                    let size = vec2(loaded.image.width as f32, loaded.image.height as f32);
                    let uv = Rect::from_min_size(
                        pos2(f.rect.x as f32 / size.x, f.rect.y as f32 / size.y),
                        vec2(f.rect.w as f32 / size.x, f.rect.h as f32 / size.y),
                    );
                    painter.image(
                        texture.id(),
                        world_rect(r),
                        uv,
                        Color32::from_white_alpha(150),
                    );
                }
            }
            _ => {}
        }
    }
}

/// The inspector's fields for one frame: which field changed (for undo), and where text goes.
struct Form<'a> {
    changed: Option<egui::Id>,
    /// The change was a press: an undo step of its own.
    step: bool,
    text_changed: bool,
    /// The palette's picture, for scattering.
    palette: Option<(String, u32)>,
    /// Asked to pick, on the map it leads to, where exit `n` arrives.
    pick_arrival: Option<usize>,
    strings: &'a mut Strings,
    language: &'a str,
    catalog: &'a Catalog,
    scene: &'a str,
    tile: u32,
}

impl Form<'_> {
    /// A press that changed the map: an undo step of its own.
    fn press(&mut self, id: impl std::hash::Hash + std::fmt::Debug) {
        self.changed = Some(egui::Id::new(id));
        self.step = true;
    }

    fn track(&mut self, response: egui::Response) -> egui::Response {
        if response.changed() {
            self.changed = Some(response.id);
        }
        response
    }

    fn point(&mut self, ui: &mut Ui, label: &str, at: &mut (f32, f32)) {
        ui.horizontal(|ui| {
            ui.label(label);
            self.track(ui.add(DragValue::new(&mut at.0).prefix("x ").speed(1.0)));
            self.track(ui.add(DragValue::new(&mut at.1).prefix("y ").speed(1.0)));
        });
    }

    fn area(&mut self, ui: &mut Ui, area: &mut (f32, f32, f32, f32)) {
        ui.horizontal(|ui| {
            ui.label("Box");
            self.track(ui.add(DragValue::new(&mut area.0).prefix("x ")));
            self.track(ui.add(DragValue::new(&mut area.1).prefix("y ")));
        });
        ui.horizontal(|ui| {
            ui.label("Size");
            self.track(
                ui.add(
                    DragValue::new(&mut area.2)
                        .prefix("w ")
                        .range(1.0..=8192.0)
                        .clamp_existing_to_range(false),
                ),
            );
            self.track(
                ui.add(
                    DragValue::new(&mut area.3)
                        .prefix("h ")
                        .range(1.0..=8192.0)
                        .clamp_existing_to_range(false),
                ),
            );
        });
    }

    fn facing(&mut self, ui: &mut Ui, facing: &mut Facing) {
        ui.horizontal(|ui| {
            ui.label("Faces");
            for f in Facing::CARDINAL {
                self.track(ui.selectable_value(facing, f, f.name()));
            }
        });
    }

    /// Text in the language being written, stored under `key`, in the field `id` (kept across
    /// frames, so typing carries on when the field is rebuilt).
    fn text(&mut self, ui: &mut Ui, id: egui::Id, key: &str, multiline: bool) {
        let mut text = self.strings.text(key, self.language);
        let edit = if multiline {
            TextEdit::multiline(&mut text).desired_rows(2)
        } else {
            TextEdit::singleline(&mut text)
        };
        if ui.add(edit.id(id).desired_width(f32::INFINITY)).changed() {
            self.strings.set(key, self.language, &text);
            self.text_changed = true;
        }
    }

    fn map(&mut self, ui: &mut Ui, def: &mut SceneDef) {
        let tile = self.tile as f32;
        // Tiles as the game counts them (a part tile is a tile).
        let (mut cols, mut rows) = (
            (def.size.0 / tile).ceil() as u32,
            (def.size.1 / tile).ceil() as u32,
        );
        // A map painted by hand is held to what a brush can cover; one made from a seed is not
        // painted at all, so it may be as large as the engine carries (§24.4).
        let most = if def.land.is_some() {
            dark_assets::SceneDef::MAX_TILES_PER_SIDE
        } else {
            PAINTABLE_TILES
        };
        // Values outside the range are left as they are until someone changes them.
        fn tiles(value: &mut u32, most: u32) -> DragValue<'_> {
            DragValue::new(value)
                .range(8..=most)
                .clamp_existing_to_range(false)
        }
        ui.horizontal(|ui| {
            ui.label("Size in tiles");
            if self.track(ui.add(tiles(&mut cols, most))).changed() {
                def.size.0 = cols as f32 * tile;
            }
            ui.label("by");
            if self.track(ui.add(tiles(&mut rows, most))).changed() {
                def.size.1 = rows as f32 * tile;
            }
            // What that is in the world, since a made map is counted in kilometres rather than
            // in tiles: at 16 px to the tile and a tile to the metre.
            let km = |tiles: u32| tiles as f32 * tile / 1_000.0;
            if def.land.is_some() {
                ui.weak(format!("{:.1} × {:.1} km", km(cols), km(rows)));
            }
        });
        self.land(ui, def);
        ui.horizontal(|ui| {
            ui.label("Region");
            let shown = def.region.clone().unwrap_or_else(|| "(none)".into());
            let before = def.region.clone();
            ComboBox::from_id_salt("region")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut def.region, None, "(none)");
                    for region in &self.catalog.regions {
                        ui.selectable_value(&mut def.region, Some(region.clone()), region);
                    }
                });
            if def.region != before {
                self.changed = Some(egui::Id::new("region"));
            }
        })
        .response
        .on_hover_text("Where in the world this map is (world.ron)");
        ui.separator();
        ui.label("Ground pictures (frames of the ground sheet)");
        ui.horizontal(|ui| {
            ui.label("Sheet");
            let before = def.ground.sheet.clone();
            sheet_combo(
                ui,
                "ground sheet",
                &mut def.ground.sheet,
                &self.catalog.props,
            );
            if def.ground.sheet != before {
                self.changed = Some(egui::Id::new("ground sheet"));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Ground");
            self.track(ui.add(frame_number(&mut def.ground.frame, 9999)));
        });
        let ground = def.ground.frame;
        let mut optional = |ui: &mut Ui, label: &str, value: &mut Option<u32>| {
            ui.horizontal(|ui| {
                ui.label(label);
                let mut frame = value.unwrap_or(ground);
                if self.track(ui.add(frame_number(&mut frame, 9999))).changed() {
                    *value = Some(frame);
                }
            });
        };
        optional(ui, "Hill tops", &mut def.terrain.top_frame);
        optional(ui, "Cliff faces", &mut def.terrain.face_frame);
        ui.separator();
        ui.weak(format!(
            "{} props placed, {} villagers, {} enemies, {} exits.",
            def.props.len(),
            def.npcs.len(),
            def.enemies.len(),
            def.exits.len()
        ));
        ui.separator();
        self.scatter(ui, def);
    }

    /// Whether this map's ground is made from a seed rather than drawn, and which seed
    /// (docs/PLAN.md §24.4). A made map is a country: hills, plains and water, worked out as the
    /// players walk, with what a designer draws laid on top of it.
    fn land(&mut self, ui: &mut Ui, def: &mut SceneDef) {
        let mut made = def.land.is_some();
        ui.horizontal(|ui| {
            if self
                .track(ui.checkbox(&mut made, "Made from a seed"))
                .changed()
            {
                // A seed nobody chose is still a country; this one is the day this was built.
                def.land = made.then_some(dark_assets::LandDef { seed: 20_260_923 });
            }
            if let Some(land) = &mut def.land {
                ui.label("Seed");
                let mut seed = land.seed;
                if self
                    .track(ui.add(DragValue::new(&mut seed).speed(1.0)))
                    .changed()
                {
                    land.seed = seed;
                }
                if self.track(ui.button("Another")).clicked() {
                    // Stirred rather than counted up: neighbouring seeds make unlike countries.
                    land.seed = land
                        .seed
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                }
            }
        })
        .response
        .on_hover_text(
            "The ground is worked out from this number as players walk, so a map may be \
             kilometres across. What you draw is laid on top of it.",
        );
        if def.land.is_some() {
            ui.weak(
                "Hills, plains and water come from the seed. Draw over it where it matters, \
                 and stamp towns on it below.",
            );
        }
    }

    /// Groups of props the game strews over the map (grass, flowers, trees): which pictures,
    /// how many, how far apart, solid or not, on which ground.
    fn scatter(&mut self, ui: &mut Ui, def: &mut SceneDef) {
        ui.strong("Scattered props");
        ui.weak("Strewn by the game over flat ground, clear of paths and people.");
        let mut remove = None;
        for (g, group) in def.scatter.iter_mut().enumerate() {
            let title = format!("{} × {}", group.count, sheet_name(&group.sheet));
            egui::CollapsingHeader::new(title)
                .id_salt(("scatter", g))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Pictures");
                        let mut drop = None;
                        for (k, frame) in group.frames.iter().enumerate() {
                            if ui.small_button(format!("#{frame} ✖")).clicked() {
                                drop = Some(k);
                            }
                        }
                        if let Some(k) = drop {
                            group.frames.remove(k);
                            self.press(("scatter drop", g, k));
                        }
                    });
                    match &self.palette {
                        Some((sheet, frame)) if *sheet == group.sheet => {
                            if !group.frames.contains(frame)
                                && ui.button(format!("Add the palette's #{frame}")).clicked()
                            {
                                group.frames.push(*frame);
                                self.press(("scatter add", g, *frame));
                            }
                        }
                        _ => {
                            ui.weak(format!(
                                "Pick a picture of {} in the palette to add it.",
                                sheet_name(&group.sheet)
                            ));
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("How many");
                        self.track(ui.add(DragValue::new(&mut group.count).range(0..=5000)));
                        ui.label("apart");
                        self.track(
                            ui.add(
                                DragValue::new(&mut group.min_spacing)
                                    .range(0.0..=2000.0)
                                    .suffix(" px"),
                            ),
                        );
                    });
                    if ui
                        .button("Shuffle")
                        .on_hover_text("Strew them somewhere else")
                        .clicked()
                    {
                        group.seed = group
                            .seed
                            .wrapping_mul(6_364_136_223_846_793_005)
                            .wrapping_add(1_442_695_040_888_963_407);
                        self.press(("shuffle", g));
                    }
                    let mut solid = group.collider.is_some();
                    if self
                        .track(ui.checkbox(&mut solid, "Solid (a round footprint)"))
                        .changed()
                    {
                        group.collider = solid.then(|| dark_assets::ColliderDef {
                            shape: dark_physics::Shape::Circle { radius: 6.0 },
                            offset: (0.0, -4.0),
                            height: 1000.0,
                        });
                    }
                    if let Some(c) = &mut group.collider {
                        collider(self, ui, ("scatter collider", g), c);
                    }
                    ui.horizontal(|ui| {
                        ui.label("On");
                        let mut anywhere = group.levels.is_none();
                        if self
                            .track(ui.checkbox(&mut anywhere, "any ground"))
                            .changed()
                        {
                            group.levels = (!anywhere).then(|| vec![0]);
                        }
                        if let Some(levels) = &mut group.levels {
                            for level in 0..=3u8 {
                                let mut on = levels.contains(&level);
                                let label = if level == 0 {
                                    "ground".to_owned()
                                } else {
                                    format!("hill {level}")
                                };
                                if ui.checkbox(&mut on, label).changed() {
                                    if on {
                                        levels.push(level);
                                        levels.sort_unstable();
                                    } else {
                                        levels.retain(|l| *l != level);
                                    }
                                    self.press(("levels", g, level));
                                }
                            }
                        }
                    });
                    if ui.button("Remove this group").clicked() {
                        remove = Some(g);
                    }
                });
        }
        if let Some(g) = remove {
            def.scatter.remove(g);
            self.press(("scatter remove", g));
        }
        match self.palette.clone() {
            Some((sheet, frame)) => {
                if ui
                    .button(format!(
                        "Scatter the palette's picture ({} #{frame})",
                        sheet_name(&sheet)
                    ))
                    .clicked()
                {
                    def.scatter.push(dark_assets::ScatterDef {
                        sheet,
                        frames: vec![frame],
                        count: 20,
                        min_spacing: 24.0,
                        seed: def.scatter.len() as u64 * 7919 + 17,
                        collider: None,
                        levels: None,
                    });
                    self.press(("scatter new", def.scatter.len()));
                }
            }
            None => {
                ui.weak("Pick a picture in the palette to scatter it.");
            }
        }
    }

    fn thing(&mut self, ui: &mut Ui, def: &mut SceneDef, thing: Thing, viewport: &Viewport) {
        // Keys the map names already: new text never shares one.
        let used = ops::keys_in(def);
        match thing {
            Thing::Place(i) => {
                let Some(place) = def.places.get_mut(i) else {
                    return;
                };
                ui.label("A place stamped on this map");
                ui.horizontal(|ui| {
                    ui.label("Which");
                    let before = place.scene.clone();
                    ComboBox::from_id_salt("place scene")
                        .selected_text(scene_name(&place.scene))
                        .show_ui(ui, |ui| {
                            for scene in &self.catalog.scenes {
                                ui.selectable_value(
                                    &mut place.scene,
                                    scene.clone(),
                                    scene_name(scene),
                                );
                            }
                        });
                    if place.scene != before {
                        self.changed = Some(egui::Id::new(("place scene", i)));
                    }
                });
                let tile = self.tile as f32;
                let mut at = (place.at.0 / tile, place.at.1 / tile);
                ui.horizontal(|ui| {
                    ui.label("At tile");
                    let mut moved = false;
                    for value in [&mut at.0, &mut at.1] {
                        moved |= self
                            .track(ui.add(DragValue::new(value).speed(1.0)))
                            .changed();
                    }
                    if moved {
                        place.at = ((at.0).round() * tile, (at.1).round() * tile);
                    }
                });
                if let Some((w, h)) = self.catalog.sizes.get(&place.scene) {
                    ui.weak(format!(
                        "{} × {} tiles of ground, people and doors, laid into this map.",
                        (w / tile).ceil() as u32,
                        (h / tile).ceil() as u32
                    ));
                }
                ui.weak("The ground under it is levelled into the land, and nothing grows on it.");
            }
            Thing::Prop(i) => {
                let Some(p) = def.props.get_mut(i) else {
                    return;
                };
                ui.label(format!("Picture {} #{}", sheet_name(&p.sheet), p.frame));
                let frames = viewport
                    .sheets
                    .get(&p.sheet)
                    .map_or(1, |s| s.sheet.frames.len().max(1)) as u32;
                ui.horizontal(|ui| {
                    ui.label("Picture number");
                    self.track(ui.add(frame_number(&mut p.frame, frames - 1)));
                });
                self.point(ui, "At", &mut p.position);
                let mut solid = !p.colliders.is_empty();
                if self
                    .track(ui.checkbox(&mut solid, "Solid (nobody walks through)"))
                    .changed()
                {
                    let width = viewport
                        .sheets
                        .get(&p.sheet)
                        .and_then(|s| s.sheet.frames.get(p.frame as usize))
                        .map_or(self.tile, |f| f.rect.w);
                    p.colliders =
                        ops::new_prop(&p.sheet, p.frame, Vec2::ZERO, width, solid).colliders;
                }
                let count = p.colliders.len();
                let mut remove = None;
                for (k, c) in p.colliders.iter_mut().enumerate() {
                    ui.indent(("prop collider", i, k), |ui| {
                        collider(self, ui, ("prop collider", i, k), c);
                        if count > 1 && ui.small_button("Remove this part").clicked() {
                            remove = Some(k);
                        }
                    });
                }
                if let Some(k) = remove {
                    p.colliders.remove(k);
                    self.press(("remove part", i, k));
                }
                if !p.colliders.is_empty()
                    && ui
                        .button("Add a part")
                        .on_hover_text("A footprint of several shapes (a house with a porch)")
                        .clicked()
                {
                    let last = p.colliders[count - 1];
                    p.colliders.push(last);
                    self.press(("add part", i, count));
                }
            }
            Thing::Npc(i) => {
                let Some(npc) = def.npcs.get_mut(i) else {
                    return;
                };
                ui.label("Name");
                let field = egui::Id::new(("npc name", i));
                match npc.name.clone() {
                    Some(key) => self.text(ui, field, &key, false),
                    None => {
                        let mut name = String::new();
                        let edit = TextEdit::singleline(&mut name)
                            .id(field)
                            .hint_text("(no name)")
                            .desired_width(f32::INFINITY);
                        if ui.add(edit).changed() {
                            let key = self
                                .strings
                                .fresh_key(&format!("name.{}", self.scene), |k| used.contains(k));
                            self.strings.set(&key, self.language, &name);
                            npc.name = Some(key);
                            self.changed = Some(egui::Id::new(("npc name", i)));
                            self.text_changed = true;
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Looks like");
                    let before = npc.sheet.clone();
                    sheet_combo(
                        ui,
                        ("npc look", i),
                        &mut npc.sheet,
                        &self.catalog.characters,
                    );
                    if npc.sheet != before {
                        self.changed = Some(egui::Id::new(("npc look", i)));
                    }
                });
                self.facing(ui, &mut npc.facing);
                self.point(ui, "Stands at", &mut npc.position);
                ui.horizontal(|ui| {
                    ui.label("Person of the world");
                    let before = npc.actor.clone();
                    ComboBox::from_id_salt(("actor", i))
                        .selected_text(npc.actor.clone().unwrap_or_else(|| "(nobody)".into()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut npc.actor, None, "(nobody)");
                            for actor in &self.catalog.actors {
                                ui.selectable_value(&mut npc.actor, Some(actor.clone()), actor);
                            }
                        });
                    if npc.actor != before {
                        self.changed = Some(egui::Id::new(("actor", i)));
                    }
                })
                .response
                .on_hover_text("A person in world.ron: one who lives the year, and can join you");
                ui.separator();
                ui.label(format!("What they say ({})", self.language));
                ui.weak("Each time someone talks to them, the next line.");
                let mut remove = None;
                let lines = npc.lines.len();
                for (n, line) in npc.lines.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let reply = matches!(line, LineDef::Reply { .. });
                        let mut who = reply;
                        ComboBox::from_id_salt(("who", i, n))
                            .selected_text(if reply { "Player" } else { "Villager" })
                            .width(80.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut who, false, "Villager");
                                ui.selectable_value(&mut who, true, "Player");
                            });
                        if who != reply {
                            let key = line.key().to_owned();
                            *line = if who {
                                LineDef::Reply { reply: key }
                            } else {
                                LineDef::Says(key)
                            };
                            self.changed = Some(egui::Id::new(("who", i, n)));
                        }
                        if lines > 1 && ui.small_button("✖").on_hover_text("Remove").clicked() {
                            remove = Some(n);
                        }
                    });
                    let key = line.key().to_owned();
                    self.text(ui, egui::Id::new(("line", &key)), &key, true);
                }
                if let Some(n) = remove {
                    npc.lines.remove(n);
                    self.changed = Some(egui::Id::new(("remove line", i, n)));
                }
                if ui.button("Add a line").clicked() {
                    let key = self
                        .strings
                        .fresh_key(&format!("npc.{}", self.scene), |k| used.contains(k));
                    npc.lines.push(LineDef::Says(key));
                    self.changed = Some(egui::Id::new(("add line", i, npc.lines.len())));
                }
            }
            Thing::Enemy(i) => {
                let Some(enemy) = def.enemies.get_mut(i) else {
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label("Kind");
                    let before = enemy.kind.clone();
                    ComboBox::from_id_salt(("enemy", i))
                        .selected_text(enemy.kind.as_str())
                        .show_ui(ui, |ui| {
                            for (kind, _) in &self.catalog.enemies {
                                ui.selectable_value(&mut enemy.kind, kind.clone(), kind);
                            }
                        });
                    if enemy.kind != before {
                        self.changed = Some(egui::Id::new(("enemy", i)));
                    }
                });
                self.facing(ui, &mut enemy.facing);
                self.point(ui, "Guards", &mut enemy.position);
            }
            Thing::Exit(i) => {
                let Some(exit) = def.exits.get_mut(i) else {
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label("Leads to");
                    let before = exit.to.clone();
                    ComboBox::from_id_salt(("exit", i))
                        .selected_text(scene_name(&exit.to))
                        .show_ui(ui, |ui| {
                            for scene in &self.catalog.scenes {
                                ui.selectable_value(&mut exit.to, scene.clone(), scene_name(scene));
                            }
                        });
                    if exit.to != before {
                        self.changed = Some(egui::Id::new(("exit", i)));
                    }
                });
                self.point(ui, "Arrives at", &mut exit.spawn);
                ui.horizontal(|ui| {
                    ui.weak("Where you appear on the other map.");
                    if ui.button("Pick it there…").clicked() {
                        self.pick_arrival = Some(i);
                    }
                });
                self.area(ui, &mut exit.area);
            }
            Thing::Inn(i) => {
                let Some(inn) = def.inns.get_mut(i) else {
                    return;
                };
                self.area(ui, &mut inn.area);
                self.point(ui, "Bed", &mut inn.bed);
                ui.weak("Where someone who left the game sleeps; inside the box.");
            }
            Thing::PlayerStart => {
                let Some(player) = &mut def.player else {
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label("Looks like");
                    let before = player.sheet.clone();
                    sheet_combo(
                        ui,
                        "player look",
                        &mut player.sheet,
                        &self.catalog.characters,
                    );
                    if player.sheet != before {
                        self.changed = Some(egui::Id::new("player look"));
                    }
                });
                self.point(ui, "Starts at", &mut player.spawn);
                ui.weak("Playing this map starts here.");
            }
        }
    }
}

/// A picture-number field up to `last`. A number out of range is left as it is until someone
/// changes it: showing a field never edits the map.
fn frame_number(value: &mut u32, last: u32) -> DragValue<'_> {
    DragValue::new(value)
        .range(0..=last)
        .clamp_existing_to_range(false)
}

/// A footprint at most this tall is jumped over and stood on.
const LOW: f32 = 18.0;

/// A footprint: round or a box, where it stands from the prop's foot, and how tall.
fn collider(
    form: &mut Form,
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    c: &mut dark_assets::ColliderDef,
) {
    use dark_physics::Shape;
    ui.horizontal(|ui| {
        let round = matches!(c.shape, Shape::Circle { .. });
        if ui.selectable_label(round, "round").clicked() && !round {
            c.shape = Shape::Circle { radius: 8.0 };
            form.press((salt, "round"));
        }
        if ui.selectable_label(!round, "a box").clicked() && round {
            c.shape = Shape::Rect {
                half: glam::Vec2::new(8.0, 6.0),
            };
            form.press((salt, "box"));
        }
        match &mut c.shape {
            Shape::Circle { radius } => {
                form.track(ui.add(DragValue::new(radius).range(1.0..=1000.0).prefix("radius ")));
            }
            Shape::Rect { half } => {
                let (mut w, mut h) = (half.x * 2.0, half.y * 2.0);
                let a = form.track(ui.add(DragValue::new(&mut w).range(1.0..=2000.0).prefix("w ")));
                let b = form.track(ui.add(DragValue::new(&mut h).range(1.0..=2000.0).prefix("h ")));
                if a.changed() || b.changed() {
                    *half = glam::Vec2::new(w / 2.0, h / 2.0);
                }
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("From the foot");
        form.track(ui.add(DragValue::new(&mut c.offset.0).prefix("x ")));
        form.track(ui.add(DragValue::new(&mut c.offset.1).prefix("y ")));
        // Low enough to jump onto: below the jump's height (about 20 px).
        let mut low = c.height <= LOW;
        if form
            .track(ui.checkbox(&mut low, "low (jumped over, stood on)"))
            .changed()
        {
            c.height = if low { LOW.min(12.0) } else { 1000.0 };
        }
    });
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|first| first.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn thing_label(thing: Thing) -> &'static str {
    match thing {
        Thing::Prop(_) => "Prop",
        Thing::Npc(_) => "Villager",
        Thing::Enemy(_) => "Enemy",
        Thing::Exit(_) => "Exit",
        Thing::Inn(_) => "Inn",
        Thing::Place(_) => "Place",
        Thing::PlayerStart => "Player start",
    }
}

fn sheet_combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    sheets: &[String],
) {
    ComboBox::from_id_salt(egui::Id::new(id))
        .selected_text(sheet_name(value))
        .show_ui(ui, |ui| {
            for sheet in sheets {
                ui.selectable_value(value, sheet.clone(), sheet_name(sheet));
            }
        });
}

/// The project's font (for Japanese and the rest) after egui's own.
fn use_project_font(project: &Project, ctx: &egui::Context) {
    let Some(def) = &project.settings.font else {
        return;
    };
    let Ok(data) = std::fs::read(project.path(&def.path)) else {
        tracing::warn!("cannot read the project font {}", def.path);
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("project".into(), Arc::new(egui::FontData::from_owned(data)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("project".into());
    }
    ctx.set_fonts(fonts);
}
