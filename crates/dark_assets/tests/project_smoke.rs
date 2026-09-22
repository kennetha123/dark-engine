//! Loads a real game project when `DARK_TEST_PROJECT` points at one (e.g. `../adventurer`).
//! Skipped otherwise, because game art is not part of the engine repository.

use dark_assets::Project;

#[test]
fn every_sheet_and_scene_in_the_project_loads() {
    let Ok(dir) = std::env::var("DARK_TEST_PROJECT") else {
        eprintln!("DARK_TEST_PROJECT not set; skipping");
        return;
    };
    let project = Project::open(dir).expect("project.ron");
    let mut checked = 0;
    for folder in ["sheets", "scenes"] {
        let Ok(entries) = std::fs::read_dir(project.path(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = format!("{folder}/{}", entry.file_name().to_string_lossy());
            if folder == "sheets" {
                let sheet = project.load_sheet(&name).unwrap_or_else(|e| panic!("{e}"));
                assert!(!sheet.sheet.frames.is_empty(), "{name} has no frames");
                // A skeleton exported again must be baked again.
                if let Some(spine) = &sheet.spine {
                    let json = std::fs::read_to_string(project.path(&spine.def.skeleton))
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(
                        dark_assets::skeleton_hash(&json).as_deref(),
                        Some(spine.bake.skeleton_hash.as_str()),
                        "{name}: the skeleton changed since it was baked"
                    );
                }
            } else {
                let scene = project.load_scene(&name).unwrap_or_else(|e| panic!("{e}"));
                // What the editor saves loads back as the same scene.
                let copy = std::env::temp_dir().join(format!(
                    "dark_smoke_{}_{}",
                    std::process::id(),
                    entry.file_name().to_string_lossy()
                ));
                project
                    .save_scene(&copy, &scene)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                let again = project
                    .load_scene(&copy)
                    .unwrap_or_else(|e| panic!("{name} as saved: {e}"));
                assert_eq!(
                    format!("{again:?}"),
                    format!("{scene:?}"),
                    "{name}: save round trip"
                );
                let _ = std::fs::remove_file(&copy);
                for prop in scene.placed_props(0.0, &[], |_, _| true) {
                    let sheet = project
                        .load_sheet(&prop.sheet)
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert!(
                        (prop.frame as usize) < sheet.sheet.frames.len(),
                        "{name}: {} has no frame {}",
                        prop.sheet,
                        prop.frame
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no sheets or scenes found");
}

#[test]
fn every_line_and_name_has_text_in_every_language_and_faces_and_font_load() {
    let Ok(dir) = std::env::var("DARK_TEST_PROJECT") else {
        return;
    };
    let project = Project::open(dir).expect("project.ron");
    if let Some(font) = &project.settings.font {
        let data = std::fs::read(project.path(&font.path)).expect("font file");
        assert!(!data.is_empty());
    }
    let mut strings = dark_assets::Localization::load(&project).unwrap_or_else(|e| panic!("{e}"));
    let entries = std::fs::read_dir(project.path("scenes")).expect("scenes folder");
    let mut keys = vec![
        "ui.talk".to_owned(),
        "ui.language".to_owned(),
        "ui.day".to_owned(),
        "ui.paused".to_owned(),
        "ui.others_playing".to_owned(),
        "ui.standing".to_owned(),
        "ui.feelings".to_owned(),
        "ui.resume".to_owned(),
    ];
    for entry in entries.flatten() {
        let name = format!("scenes/{}", entry.file_name().to_string_lossy());
        let scene = project.load_scene(&name).unwrap_or_else(|e| panic!("{e}"));
        let mut looks: Vec<dark_assets::LookDef> = scene.player.iter().map(|p| p.look()).collect();
        for npc in &scene.npcs {
            assert!(!npc.lines.is_empty(), "{name}: an NPC has no lines");
            keys.extend(npc.lines.iter().map(|l| l.key().to_owned()));
            looks.push(npc.look());
        }
        for look in looks {
            for sheet in std::iter::once(&look.sheet).chain(&look.attack) {
                project
                    .load_sheet(sheet)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            if let Some(face) = &look.face {
                project
                    .load_face(face, 72)
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
            }
            keys.extend(look.name);
        }
    }
    // Combat: the definitions validate, every enemy's sheets load and its name has text.
    let combat = dark_combat::CombatDef::load_or_default(&project.path("combat.ron"))
        .unwrap_or_else(|e| panic!("{e}"));
    for (id, enemy) in &combat.enemies {
        for sheet in std::iter::once(&enemy.sheet).chain(&enemy.attack) {
            project
                .load_sheet(sheet)
                .unwrap_or_else(|e| panic!("enemy {id}: {e}"));
        }
        keys.push(enemy.name.clone());
    }
    // Life: the definitions validate, icons and structure art load, and every item, status and
    // line the body can say has text.
    let life = dark_life::LifeDef::load_or_default(&project.path("life.ron"))
        .unwrap_or_else(|e| panic!("{e}"));
    for (id, item) in &life.items {
        if let Some(icon) = &item.icon {
            project
                .load_icon(&icon.image, icon.size, icon.cell, 16)
                .unwrap_or_else(|e| panic!("item {id}: {e}"));
        }
        keys.push(item.name.clone());
    }
    for sheet in life.art.tent.iter().chain(&life.art.campfire) {
        project.load_sheet(sheet).unwrap_or_else(|e| panic!("{e}"));
    }
    keys.extend(dark_life::Status::ALL.map(|s| s.key().to_owned()));
    keys.push("life.accident".to_owned());
    // What the engine has people say about parties (dark_world::party).
    for key in [
        "party.yes",
        "party.no",
        "party.no.distrust",
        "party.no.grudge",
        "party.no.enemy",
        "party.no.busy",
        "party.no.full",
        "party.farewell",
        "party.invite",
        "party.together",
        "party.leave",
        "party.betrayed",
        "react.accident",
        "react.drunk",
        "react.filthy",
    ] {
        keys.push(key.to_owned());
    }
    // The story: it validates, every storylet is with a person of the world, and every line,
    // choice and ending has text.
    let story = dark_story::StoryDef::load_or_default(&project.path("story.ron"))
        .unwrap_or_else(|e| panic!("{e}"));
    keys.extend(story.keys());
    // Audio: the banks the project names are there.
    if let Some(audio) = &project.settings.audio {
        for bank in &audio.banks {
            assert!(project.path(bank).exists(), "missing bank {bank}");
        }
    }
    // The world simulation's names too, and the world itself must build.
    let world_path = project.path("world.ron");
    if world_path.exists() {
        let def = dark_sim::WorldDef::load(&world_path).unwrap_or_else(|e| panic!("{e}"));
        let sim = dark_sim::WorldSim::new(&def, 0).unwrap_or_else(|e| panic!("{e}"));
        let world = sim.world();
        keys.extend(world.regions.iter().map(|r| r.name.clone()));
        keys.extend(world.factions.iter().map(|f| f.name.clone()));
        keys.extend(world.titles.iter().map(|t| t.name.clone()));
        keys.extend(world.actors.iter().map(|a| a.name.clone()));
        for storylet in &story.storylets {
            assert!(
                world.actor(&storylet.with).is_some(),
                "story.ron: {} is with unknown actor {}",
                storylet.id,
                storylet.with
            );
        }
        for region in life.climates.keys() {
            assert!(
                world.region(region).is_some(),
                "life.ron: unknown region {region}"
            );
        }
        // And every scene's region is in it.
        for entry in std::fs::read_dir(project.path("scenes"))
            .expect("scenes")
            .flatten()
        {
            let name = format!("scenes/{}", entry.file_name().to_string_lossy());
            let scene = project.load_scene(&name).unwrap_or_else(|e| panic!("{e}"));
            if let Some(region) = &scene.region {
                assert!(
                    world.region(region).is_some(),
                    "{name}: unknown region {region}"
                );
            }
            for npc in &scene.npcs {
                if let Some(actor) = &npc.actor {
                    assert!(
                        world.actor(actor).is_some(),
                        "{name}: unknown actor {actor}"
                    );
                }
            }
        }
    }
    for language in project.settings.languages.clone() {
        assert!(strings.set_language(&language));
        for key in &keys {
            assert!(strings.has(key), "{language}: no text for {key}");
        }
    }
}
