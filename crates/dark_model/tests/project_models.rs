//! Loads a real project's models when `DARK_TEST_PROJECT` points at one (e.g. `../adventurer`).
//! Skipped otherwise, because game art is not part of the engine repository.

use dark_assets::Project;
use dark_model::{Model, Pose};
use glam::{Mat4, Vec3};

/// The height of a model as it is actually posed, in world pixels.
fn posed_height(model: &Model, pose: &Pose) -> f32 {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for part in &model.parts {
        for vertex in &part.vertices {
            let mut skin = Mat4::ZERO;
            let mut used = 0.0;
            let weights: [f32; 4] = vertex.weights.into();
            for (joint, weight) in vertex.joints.iter().zip(weights) {
                if weight > 0.0
                    && let Some(bone) = pose.palette().get(*joint as usize)
                {
                    skin += *bone * weight;
                    used += weight;
                }
            }
            if used <= 1e-6 {
                skin = Mat4::IDENTITY;
            }
            let up = skin.transform_point3(vertex.position).dot(Model::UP);
            lo = lo.min(up);
            hi = hi.max(up);
        }
    }
    if lo > hi {
        0.0
    } else {
        (hi - lo) * model.scale
    }
}

/// **A model must come out the size it was measured at.**
///
/// This is the check that nothing else can make. Every preview frames a model on its own bounds,
/// so one a hundred times too large draws pixel-identically to one that is right — which is
/// exactly what happened: a skinning mistake made every export pose a hundredfold too big, and
/// it went unseen through two phases of work until a model finally stood in a real world beside
/// sprites that knew how large a person is.
///
/// Baking measures the model in Blender; loading measures what the engine will draw. They are
/// the same model through different tools, so they have to agree.
#[test]
fn a_model_poses_at_the_size_it_was_baked_at() {
    let Ok(dir) = std::env::var("DARK_TEST_PROJECT") else {
        eprintln!("DARK_TEST_PROJECT not set; skipping");
        return;
    };
    let project = Project::open(dir).expect("project.ron");
    let Ok(entries) = std::fs::read_dir(project.path("models")) else {
        return; // A project with no 3D models yet.
    };
    let mut checked = 0;
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !file.ends_with(".model.ron") {
            continue;
        }
        let name = format!("models/{file}");
        let loaded = project
            .load_model(&name)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let model = Model::load(&project, &loaded.def).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut pose = Pose::new(&model);

        for (clip, baked) in &loaded.bake.clips {
            // The first tick of every clip, because a pose is where a mistake shows.
            pose.pose(&model, &baked.animation, baked.looping, 0.0);
            let posed = posed_height(&model, &pose);
            let baked_height = loaded.bake.height;
            let off = (posed - baked_height).abs() / baked_height.max(1e-3);
            assert!(
                off < 0.15,
                "{name}: posed at {clip} it stands {posed:.1} px, but it was baked at \
                 {baked_height:.1} px — a skinning transform is wrong"
            );
        }

        // And it stands up: a character is taller than they are deep.
        pose.pose(&model, "", false, 0.0);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for part in &model.parts {
            for vertex in &part.vertices {
                lo = lo.min(vertex.position);
                hi = hi.max(vertex.position);
            }
        }
        let extent = hi - lo;
        assert!(
            extent.dot(Model::UP) >= extent.x && extent.dot(Model::UP) >= extent.z,
            "{name}: it is {extent:?}, which is not standing up — is the export Y-up?"
        );
        checked += 1;
    }
    eprintln!("checked {checked} model(s)");
}
