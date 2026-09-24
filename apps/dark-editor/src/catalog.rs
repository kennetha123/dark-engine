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
    /// Faceset pictures: the portraits shown beside what somebody says.
    pub faces: Vec<String>,
    /// Enemy kinds (`combat.ron`), with the sheet each wears.
    pub enemies: Vec<(String, String)>,
    /// How a character fights (`combat.ron`): what an NPC's "Moves like" can be.
    pub movesets: Vec<String>,
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
        catalog.faces = faceset_files(project);
        match dark_combat::CombatDef::load_or_default(&project.path("combat.ron")) {
            Ok(combat) => {
                catalog.enemies = combat
                    .enemies
                    .iter()
                    .map(|(id, e)| (id.clone(), e.sheet.clone()))
                    .collect();
                catalog.movesets = combat.movesets.keys().cloned().collect();
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

/// The faceset pictures in the project's art: every `.png` in a folder called `faces`, or named
/// like one (`FacesetActor01.png`), which is how the art they are drawn from arrives. A portrait
/// is not a sheet — the engine cuts it into a 4×2 grid rather than reading a sheet's frames — so
/// these are found by looking rather than listed anywhere.
fn faceset_files(project: &Project) -> Vec<String> {
    /// Deep enough for the art as it is packed, and a stop: `Art/` may be a junction to the real
    /// folder, and following one in a circle must end.
    const DEEP: usize = 8;
    fn walk(dir: &Path, prefix: &str, depth: usize, found: &mut Vec<String>) {
        if depth > DEEP {
            return;
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let path = format!("{prefix}/{name}");
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&entry.path(), &path, depth + 1, found);
            } else if name.to_lowercase().ends_with(".png") {
                let in_faces = dir
                    .file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.eq_ignore_ascii_case("faces"));
                if in_faces || name.to_lowercase().starts_with("faceset") {
                    found.push(path);
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(&project.path("Art"), "Art", 0, &mut found);
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

/// A faceset's short name: `Art/Pitaya/faces/FacesetActor01.png` is "Pitaya/Actor01".
///
/// Every folder between `Art` and the `faces` the picture sits in is kept, because a set of art
/// names its facesets the same way every other set does, and `Grasslands/16x1` and
/// `Grasslands/16x3` both hold a `FacesetActor01.png`: keeping only the first folder would give
/// a designer two entries with one name and no way to tell them apart.
pub fn face_name(path: &str) -> String {
    let name = Path::new(path)
        .file_stem()
        .and_then(|f| f.to_str())
        .unwrap_or(path);
    // "FacesetActor01" is "Actor01"; a picture called just "Faceset" keeps its own name, there
    // being nothing else to call it.
    let name = name
        .strip_prefix("Faceset")
        .filter(|n| !n.is_empty())
        .unwrap_or(name);
    let folders: Vec<&str> = path
        .strip_prefix("Art/")
        .unwrap_or(path)
        .split('/')
        .rev()
        .skip(1)
        .filter(|folder| !folder.eq_ignore_ascii_case("faces"))
        .collect();
    if folders.is_empty() {
        return name.to_owned();
    }
    let set = folders.into_iter().rev().collect::<Vec<_>>().join("/");
    format!("{set}/{name}")
}

/// A sheet's short name: `sheets/town_props.sheet.ron` is "town_props".
pub fn sheet_name(path: &str) -> &str {
    let file = Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(path);
    file.split('.').next().unwrap_or(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two sets of art name their facesets alike, and the adventurer project really does hold
    /// `Grasslands/16x1` and `Grasslands/16x3`: a designer must be able to tell one list entry
    /// from the other.
    #[test]
    fn facesets_in_different_folders_are_named_differently() {
        let a = face_name("Art/Grasslands/16x1/faces/FacesetActor01.png");
        let b = face_name("Art/Grasslands/16x3/faces/FacesetActor01.png");
        assert_eq!(a, "Grasslands/16x1/Actor01");
        assert_ne!(a, b, "two pictures, one name");
        assert_eq!(
            face_name("Art/Pitaya/faces/FacesetActor01.png"),
            "Pitaya/Actor01"
        );
        // Nothing to trim, nothing to say where it came from, and nothing left over.
        assert_eq!(face_name("Art/faces/Faceset.png"), "Faceset");
        assert_eq!(face_name("portrait.png"), "portrait");
    }
}
