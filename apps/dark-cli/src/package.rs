//! Packaging a project into a folder to hand to someone who does not have the engine
//! (docs/PLAN.md §19): the game exe, and only the files the game actually loads.
//!
//! The project folder holds whole art packs, of which a game uses a few files; a package
//! collects them by following what the scenes, the database and the sheets refer to, so what
//! is handed over is small enough to send.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use dark_assets::{LookDef, Project, SceneDef};

/// Where the packaged game keeps its content, beside the exe. `dark-player` started with no
/// `--project` plays the project it finds here.
pub const GAME_DIR: &str = "game";
/// What the package tells whoever receives it; also the mark of a folder made by packaging.
const README: &str = "README.txt";

pub struct Options {
    pub project: PathBuf,
    pub out: PathBuf,
    /// The player to copy in; by default `dark-player` beside this tool.
    pub exe: Option<PathBuf>,
}

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut plain = Vec::new();
    let mut exe = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--exe" => exe = Some(PathBuf::from(rest.next().ok_or("--exe needs a path")?)),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => plain.push(other.to_owned()),
        }
    }
    let [project, out] = plain.as_slice() else {
        return Err("usage: package <project-dir> <out-dir> [--exe <path>]".into());
    };
    Ok(Options {
        project: PathBuf::from(project),
        out: PathBuf::from(out),
        exe,
    })
}

pub fn run(options: &Options) -> Result<(), String> {
    let project = Project::open(&options.project).map_err(|e| e.to_string())?;
    // The scene the game starts in is the project's own: a package plays as the game does.
    let scene = project
        .settings
        .start_scene
        .clone()
        .unwrap_or_else(|| dark_assets::DEFAULT_SCENE.to_owned());
    let needed = collect(&project, &scene)?;
    let exe = player_exe(options)?;

    let game = options.out.join(GAME_DIR);
    // Only a folder that is empty, or holds a package already, is written into: `<out>` is
    // given on a command line, and this deletes what is in it.
    if options.out.exists() {
        let empty = options
            .out
            .read_dir()
            .map_err(|e| format!("{}: {e}", options.out.display()))?
            .next()
            .is_none();
        if !empty && !options.out.join(README).exists() {
            return Err(format!(
                "{} is not empty and is not a package (no {README}); give an empty folder",
                options.out.display()
            ));
        }
    }
    if game.exists() {
        std::fs::remove_dir_all(&game).map_err(|e| format!("{}: {e}", game.display()))?;
    }
    let mut bytes = 0;
    for file in &needed {
        bytes += copy(&project.path(file), &game.join(file))?;
    }
    let name = format!("{}{}", exe_name(&project), std::env::consts::EXE_SUFFIX);
    bytes += copy(&exe, &options.out.join(&name))?;
    let readme = options.out.join(README);
    std::fs::write(&readme, readme_text(&project, &name))
        .map_err(|e| format!("{}: {e}", readme.display()))?;

    println!("the game: {}", exe.display());
    if exe.components().any(|c| c.as_os_str() == "debug") {
        println!(
            "note: that is a debug build, which runs slowly; \
             `cargo build --release -p dark-player` and package again for one to hand over"
        );
    }
    println!(
        "packaged {} files ({:.1} MB) into {}",
        needed.len() + 1,
        bytes as f64 / (1024.0 * 1024.0),
        options.out.display()
    );
    println!("double-click {name} to play; see README.txt for playing together");
    Ok(())
}

/// The player to package: the one asked for, else `dark-player` beside this tool.
fn player_exe(options: &Options) -> Result<PathBuf, String> {
    if let Some(exe) = &options.exe {
        return exe
            .is_file()
            .then(|| exe.clone())
            .ok_or_else(|| format!("{}: no such file", exe.display()));
    }
    let beside = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(format!("dark-player{}", std::env::consts::EXE_SUFFIX));
    beside.is_file().then_some(beside).ok_or_else(|| {
        "the game is not built beside this tool: `cargo build --release -p dark-player`, or pass \
         --exe <path>"
            .to_owned()
    })
}

/// A file name for the packaged game, from the project's name.
fn exe_name(project: &Project) -> String {
    let name: String = project
        .settings
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() { "game".into() } else { name }
}

fn copy(from: &Path, to: &Path) -> Result<u64, String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))
}

/// Every file the game reads, as paths inside the project. Anything referred to but missing is
/// an error: the package would be broken in a way only playing it would show.
fn collect(project: &Project, start: &str) -> Result<BTreeSet<String>, String> {
    let mut needed = BTreeSet::new();
    let mut sheets = BTreeSet::new();
    needed.insert(Project::FILE.to_owned());
    if let Some(font) = &project.settings.font {
        needed.insert(font.path.clone());
        // A font is handed on under its licence, so its licence goes with it.
        let dir = Path::new(&font.path).parent().unwrap_or(Path::new(""));
        if let Ok(beside) = std::fs::read_dir(project.path(dir)) {
            for entry in beside.flatten() {
                let name = entry.file_name().to_string_lossy().to_uppercase();
                if name.starts_with("OFL")
                    || name.starts_with("LICEN")
                    || name.starts_with("COPYING")
                {
                    needed.insert(normalise(&dir.join(entry.file_name()).to_string_lossy()));
                }
            }
        }
    }
    if let Some(audio) = &project.settings.audio {
        // Every platform's runtime library the project has, so one package plays on both;
        // a platform without one is silent there, which is not worth refusing to package for.
        for (platform, library) in &audio.library {
            if project.path(library).exists() {
                needed.insert(library.clone());
            } else {
                println!("note: no FMOD runtime for {platform} ({library}); silent there");
            }
        }
        needed.extend(audio.banks.iter().cloned());
    }
    for language in &project.settings.languages {
        needed.insert(format!("locale/{language}.ron"));
        // Text written in the editor lives beside the hand-written table and wins over it;
        // without it a packaged game shows keys where the lines should be.
        let edited = format!("locale/{language}.editor.ron");
        if project.path(&edited).exists() {
            needed.insert(edited);
        }
    }
    for file in ["world.ron", "story.ron", "life.ron", "combat.ron"] {
        if project.path(file).exists() {
            needed.insert(file.to_owned());
        }
    }

    let combat = dark_combat::CombatDef::load_or_default(&project.path("combat.ron"))
        .map_err(|e| e.to_string())?;
    let life = dark_life::LifeDef::load_or_default(&project.path("life.ron"))
        .map_err(|e| e.to_string())?;
    for item in life.items.values() {
        if let Some(icon) = &item.icon {
            needed.insert(icon.image.clone());
        }
    }
    // Tents and campfires, which are set down rather than placed in a scene.
    sheets.extend(life.art.tent.iter().chain(&life.art.campfire).cloned());

    // The starting scene and every scene its exits lead to, and so on.
    let mut queue = VecDeque::from([normalise(start)]);
    let mut scenes = BTreeSet::new();
    while let Some(scene) = queue.pop_front() {
        if !scenes.insert(scene.clone()) {
            continue;
        }
        stays_in(&scene)?;
        let def: SceneDef = project.load_scene(&scene).map_err(|e| e.to_string())?;
        needed.insert(scene.clone());
        for exit in &def.exits {
            queue.push_back(normalise(&exit.to));
        }
        sheets.insert(def.ground.sheet.clone());
        sheets.extend(def.props.iter().map(|p| p.sheet.clone()));
        sheets.extend(def.scatter.iter().map(|s| s.sheet.clone()));
        let looks = def
            .npcs
            .iter()
            .map(|npc| npc.look())
            .chain(def.player.as_ref().map(|p| p.look()))
            .chain(def.enemies.iter().filter_map(|placed| {
                combat.enemies.get(&placed.kind).map(|enemy| LookDef {
                    sheet: enemy.sheet.clone(),
                    attack: enemy.attack.clone(),
                    face: None,
                    name: Some(enemy.name.clone()),
                    moveset: Some(enemy.moveset.clone()),
                })
            }));
        for look in looks {
            sheets.insert(look.sheet.clone());
            sheets.extend(look.attack.clone());
            if let Some(face) = &look.face {
                needed.insert(face.image.clone());
            }
        }
    }

    // What each sheet draws: an image, or a skeleton with its atlas, pages and baked clips.
    for sheet in &sheets {
        stays_in(sheet)?;
        needed.insert(sheet.clone());
        if sheet.ends_with(".spine.ron") {
            let def = project.load_spine_def(sheet).map_err(|e| e.to_string())?;
            needed.insert(def.skeleton.clone());
            needed.insert(def.atlas.clone());
            needed.insert(def.baked.clone());
            // Named by Spine's own atlas reader, as loading the skeleton names them.
            let dir = Path::new(&def.atlas).parent().unwrap_or(Path::new(""));
            let pages = dark_spine::atlas_pages(project, &def).map_err(|e| e.to_string())?;
            needed.extend(
                pages
                    .iter()
                    .map(|page| normalise(&dir.join(page).to_string_lossy())),
            );
        } else {
            let def = project.load_sheet_def(sheet).map_err(|e| e.to_string())?;
            needed.insert(def.image.clone());
        }
    }

    for file in &needed {
        stays_in(file)?;
    }
    let missing: Vec<&String> = needed
        .iter()
        .filter(|file| !project.path(file).exists())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the project refers to {} file(s) that are not there: {}",
            missing.len(),
            missing
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(needed)
}

/// What the package says for the person who receives it.
fn readme_text(project: &Project, exe: &str) -> String {
    format!(
        "{name}\n\n\
         Play: double-click {exe}. The world is kept only if you start it with --save (below),\n\
         so a plain double-click begins a new year every time.\n\n\
         Keeping your world:\n\
         - {exe} --save my-world.sav\n\n\
         Play together (up to four). There is no menu for this yet: it is typed at a command\n\
         line (in this folder, type cmd in the address bar and press enter).\n\
         - The host:  {exe} --host 7777 --save our-world.sav\n\
         - Everyone else:  {exe} --join HOST:7777   (HOST is the host's address)\n\
         On the same network the host's address is their local IP (ipconfig / ip addr). Over the\n\
         internet the host must open UDP port 7777 on their router, or you can all use a private\n\
         network such as Tailscale and join that address. Everyone needs this same build.\n\n\
         Controls: WASD or arrows walk, Shift runs, Space jumps, J attacks (again to combo),\n\
         K dodges, E talks (number keys answer), Q asks someone along, Z sleeps, R relieves\n\
         yourself, 1-8 use the hotbar, Esc opens the menu, F1 shows collision, F2 switches\n\
         language.\n\n\
         The {game} folder beside the exe holds the game's content; keep them together.\n",
        name = project.settings.name,
        game = GAME_DIR,
    )
}

/// `./a\b.ron` is `a/b.ron`, as scenes are named when maps load.
fn normalise(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

/// Refuses a path that leads out of the project: packaging copies the project's own files, and
/// a path of its own would be read from, or written over, somewhere else entirely.
fn stays_in(file: &str) -> Result<(), String> {
    inside(file).then_some(()).ok_or_else(|| {
        format!("{file} is not a path inside the project; packaging copies the project's own files")
    })
}

/// Whether `file` stays inside the project: relative, and never climbing out.
fn inside(file: &str) -> bool {
    use std::path::Component;
    let path = Path::new(file);
    path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project of two scenes joined by an exit, of which the second has the only enemy.
    fn project(root: &Path) -> Project {
        let write = |name: &str, text: &str| {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            Project::FILE,
            r#"(name: "Test Game", tile_size: 16, resolution: (320, 180), languages: ["en", "ja"],
                start_scene: "scenes/one.ron",
                font: (path: "fonts/f.ttf", size: 16, line_height: 18))"#,
        );
        write(
            "scenes/one.ron",
            r#"(size: (320.0, 180.0), ground: (sheet: "sheets/ground.sheet.ron", frame: 0),
                player: (sheet: "sheets/hero.sheet.ron", face: (image: "art/faces.png"),
                    spawn: (10.0, 10.0)),
                props: [(sheet: "sheets/props.sheet.ron", frame: 1, position: (10.0, 10.0))],
                exits: [(area: (0.0, 0.0, 8.0, 8.0), to: "./scenes\\two.ron", spawn: (1.0, 1.0))])"#,
        );
        write(
            "scenes/two.ron",
            r#"(size: (320.0, 180.0), ground: (sheet: "sheets/ground.sheet.ron", frame: 0),
                enemies: [(kind: "grunt", position: (20.0, 20.0))])"#,
        );
        write(
            "combat.ron",
            r#"(movesets: {"fists": (health: 10, combo: [])},
                enemies: {"grunt": (name: "e.grunt", sheet: "sheets/grunt.sheet.ron",
                    moveset: "fists", ai: (sight: 80.0, leash: 200.0, attack_range: 20.0))})"#,
        );
        write(
            "life.ron",
            r#"(items: {"bread": (name: "i.bread", icon: (image: "art/icons.png", size: 16,
                cell: (0, 0)), use: Wash)},
               art: (tent: "sheets/tent.sheet.ron"))"#,
        );
        for sheet in ["ground", "props", "hero", "grunt", "tent"] {
            write(
                &format!("sheets/{sheet}.sheet.ron"),
                &format!(r#"(image: "art/{sheet}.png", slicing: Grid(cell: (16, 16)))"#),
            );
            write(&format!("art/{sheet}.png"), "");
        }
        for file in [
            "fonts/f.ttf",
            "fonts/OFL.txt",
            "locale/en.ron",
            "locale/ja.ron",
            // Lines written in the editor, which win over the hand-written table.
            "locale/en.editor.ron",
            "art/faces.png",
            "art/icons.png",
        ] {
            write(file, "");
        }
        Project::open(root).unwrap()
    }

    /// A temporary project folder of this test's own.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dark-package-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_package_holds_what_the_game_loads_and_nothing_else() {
        let root = temp_dir("whole");
        let project = project(&root);
        // Art the project holds but nothing refers to stays behind.
        std::fs::write(root.join("art/unused.png"), "").unwrap();
        let needed = collect(&project, "scenes/one.ron").unwrap();

        for file in [
            "project.ron",
            "fonts/f.ttf",
            "locale/en.ron",
            "locale/ja.ron",
            // The editor's own text, and the font's licence with the font.
            "locale/en.editor.ron",
            "fonts/OFL.txt",
            "combat.ron",
            "life.ron",
            "scenes/one.ron",
            // Reached through the first scene's exit, however that exit spells it.
            "scenes/two.ron",
            "sheets/hero.sheet.ron",
            "art/hero.png",
            "art/faces.png",
            "art/icons.png",
            // The enemy of the second scene, through combat.ron, and the tent from life.ron.
            "sheets/grunt.sheet.ron",
            "art/grunt.png",
            "sheets/tent.sheet.ron",
        ] {
            assert!(needed.contains(file), "{file} is missing from {needed:?}");
        }
        assert!(!needed.contains("art/unused.png"), "unused art is packaged");
        // A file the project names but does not have breaks the package, not the player's game.
        std::fs::remove_file(root.join("art/grunt.png")).unwrap();
        let err = collect(&project, "scenes/one.ron").unwrap_err();
        assert!(err.contains("art/grunt.png"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_path_leading_out_of_the_project_is_refused() {
        let root = temp_dir("stray");
        let project = project(&root);
        // An exit written as a path of its own would be copied over whatever it names.
        let stray = if cfg!(windows) {
            "D:/elsewhere/two.ron"
        } else {
            "/elsewhere/two.ron"
        };
        std::fs::write(
            root.join("scenes/one.ron"),
            format!(
                r#"(size: (320.0, 180.0), ground: (sheet: "sheets/ground.sheet.ron", frame: 0),
                    exits: [(area: (0.0, 0.0, 8.0, 8.0), to: "{stray}", spawn: (1.0, 1.0))])"#
            ),
        )
        .unwrap();

        let err = collect(&project, "scenes/one.ron").unwrap_err();
        assert!(err.contains("inside the project"), "{err}");
        assert!(!inside("../outside.png") && !inside(stray));
        assert!(inside("art/x.png") && inside("./art/x.png"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_package_is_named_after_the_project() {
        let root = temp_dir("name");
        assert_eq!(exe_name(&project(&root)), "Test-Game");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
