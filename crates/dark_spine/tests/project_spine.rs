//! The game project's Spine sheets, when `DARK_TEST_PROJECT` points at one (e.g. `../adventurer`).
//! Skipped otherwise, because game art is not part of the engine repository.

use dark_assets::Project;

fn spine_sheets(project: &Project) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(project.path("sheets")) else {
        return Vec::new();
    };
    let mut sheets: Vec<String> = entries
        .flatten()
        .map(|e| format!("sheets/{}", e.file_name().to_string_lossy()))
        .filter(|name| name.ends_with(".spine.ron"))
        .collect();
    sheets.sort();
    sheets
}

#[test]
fn every_spine_sheet_is_baked_up_to_date_and_poses_into_triangles() {
    let Ok(dir) = std::env::var("DARK_TEST_PROJECT") else {
        eprintln!("DARK_TEST_PROJECT not set; skipping");
        return;
    };
    let project = Project::open(dir).expect("project.ron");
    for sheet in spine_sheets(&project) {
        let loaded = project.load_sheet(&sheet).unwrap_or_else(|e| panic!("{e}"));
        let spine = loaded.spine.expect("a Spine sheet");
        // Baking again gives exactly the committed bake: nothing was exported since.
        let fresh = dark_spine::bake(&project, &spine.def).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            fresh, spine.bake,
            "{sheet}: stale bake; run dark-cli bake-spine"
        );
        assert_eq!(
            dark_spine::bake(&project, &spine.def).unwrap(),
            fresh,
            "deterministic"
        );
        // Every clip poses into triangles on a real page, with texture coordinates on it.
        let rig = dark_spine::Rig::load(&project, &spine.def).unwrap();
        let mut pose = dark_spine::Pose::new(&rig);
        for (name, clip) in &spine.bake.clips {
            pose.pose(&clip.animation, clip.looping, clip.ticks as f32 / 120.0);
            let meshes = pose.meshes(&rig);
            assert!(!meshes.is_empty(), "{sheet}: {name} draws nothing");
            for mesh in &meshes {
                assert!(mesh.page < rig.pages.len());
                assert_eq!(mesh.vertices.len() % 3, 0);
                assert!(mesh.vertices.iter().all(|v| {
                    (-0.01..=1.01).contains(&v.uv.x) && (-0.01..=1.01).contains(&v.uv.y)
                }));
            }
        }
    }
}
