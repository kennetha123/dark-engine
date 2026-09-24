//! What the project has to build maps from, for the editor's lists: scenes, sheets, kinds of
//! enemy, the world's people and regions.

use std::path::Path;

use dark_assets::{PlayerDef, Project};

use crate::viewport::Viewport;

#[derive(Default)]
pub struct Catalog {
    /// Scene files, `scenes/<name>.ron`.
    pub scenes: Vec<String>,
    /// Sheets a character can wear (they have walk and idle clips, or a skeleton).
    pub characters: Vec<String>,
    /// Every other sheet: props and ground.
    pub props: Vec<String>,
    /// Attack sheets (a character's swings, beside its walking sheet).
    pub attacks: Vec<String>,
    /// Enemy kinds (`combat.ron`), with the sheet each wears.
    pub enemies: Vec<(String, String)>,
    /// The world's people (`world.ron`) an NPC can be.
    pub actors: Vec<String>,
    pub regions: Vec<String>,
    /// How big each scene is, in pixels: what a place stamped on a map covers (§24.5).
    pub sizes: std::collections::HashMap<String, (f32, f32)>,
    /// A player look from any scene, for maps that get their first player start.
    pub player: Option<PlayerDef>,
    /// Files that could not be read, to show once.
    pub problems: Vec<String>,
}

impl Catalog {
    /// Reads the project's lists and loads every sheet (into `viewport`, which draws them).
    pub fn load(project: &Project, viewport: &mut Viewport) -> Self {
        let mut catalog = Self {
            scenes: files(project, "scenes", ".ron"),
            ..Default::default()
        };
        for path in sheet_files(project) {
            match viewport.load_sheet(project, &path) {
                Ok(loaded) => {
                    let walks = loaded.sheet.clip_id("idle_down").is_some();
                    if walks || loaded.spine.is_some() {
                        catalog.characters.push(path);
                    } else if path.contains("_attack") {
                        catalog.attacks.push(path);
                    } else {
                        catalog.props.push(path);
                    }
                }
                Err(err) => catalog.problems.push(format!("{path}: {err}")),
            }
        }
        catalog.characters.sort();
        catalog.props.sort();
        catalog.attacks.sort();
        match dark_combat::CombatDef::load_or_default(&project.path("combat.ron")) {
            Ok(combat) => {
                catalog.enemies = combat
                    .enemies
                    .iter()
                    .map(|(id, e)| (id.clone(), e.sheet.clone()))
                    .collect();
            }
            Err(err) => catalog.problems.push(err.to_string()),
        }
        let world = project.path("world.ron");
        if world.exists() {
            match dark_sim::WorldDef::load(&world) {
                Ok(def) => {
                    catalog.actors = def.actors.iter().map(|a| a.id.clone()).collect();
                    catalog.regions = def.regions.iter().map(|r| r.id.clone()).collect();
                }
                Err(err) => catalog.problems.push(err.to_string()),
            }
        }
        catalog.sizes = catalog
            .scenes
            .iter()
            .filter_map(|s| Some((s.clone(), project.load_scene(s).ok()?.size)))
            .collect();
        catalog.player = catalog
            .scenes
            .iter()
            .filter_map(|s| project.load_scene(s).ok())
            .find_map(|s| s.player);
        catalog
    }

    pub fn enemy_sheet(&self, kind: &str) -> Option<&str> {
        self.enemies
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, sheet)| sheet.as_str())
    }
}

/// Files in `dir` ending in `suffix`, as project paths (`dir/name`), sorted.
fn files(project: &Project, dir: &str, suffix: &str) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(project.path(dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let is_file = entry.file_type().is_ok_and(|t| t.is_file());
            (is_file && name.ends_with(suffix)).then(|| format!("{dir}/{name}"))
        })
        .collect();
    found.sort();
    found
}

/// Every sheet file of the project: `sheets/*.sheet.ron` and `sheets/*.spine.ron`.
pub fn sheet_files(project: &Project) -> Vec<String> {
    let mut list = files(project, "sheets", ".sheet.ron");
    list.extend(files(project, "sheets", ".spine.ron"));
    list.sort();
    list
}

/// A scene's short name: `scenes/meadow.ron` is "meadow".
pub fn scene_name(path: &str) -> &str {
    let file = Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(path);
    file.strip_suffix(".ron").unwrap_or(file)
}

/// A sheet's short name: `sheets/town_props.sheet.ron` is "town_props".
pub fn sheet_name(path: &str) -> &str {
    let file = Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}
