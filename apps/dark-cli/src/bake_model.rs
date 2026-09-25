//! `dark-cli bake-model` — turns an artist's export into what the game loads
//! (`journals/engine/04`, phase 1).
//!
//! Blender does the reading. It is the one tool that opens every format an artist might hand
//! over, it is already needed to author the models, and writing an FBX reader in Rust to avoid
//! shelling out to it would be a poor trade. The conversion happens here, offline, and the game
//! only ever sees glTF — the same shape as `bake-spine`, which measures a skeleton once so the
//! host never has to run Spine.

use std::path::{Path, PathBuf};
use std::process::Command;

use dark_assets::{ModelMeasure, Project, source_hash};

/// Where Blender is. `DARK_BLENDER` wins; otherwise the usual Windows install, newest first;
/// otherwise whatever is on the path.
fn blender() -> Result<PathBuf, String> {
    if let Some(set) = std::env::var_os("DARK_BLENDER") {
        let path = PathBuf::from(set);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!(
            "DARK_BLENDER points at {}, which is not there",
            path.display()
        ));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for root in [
        "C:/Program Files/Blender Foundation",
        "C:/Program Files (x86)/Blender Foundation",
    ] {
        for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let exe = entry.path().join("blender.exe");
            if exe.exists() {
                found.push(exe);
            }
        }
    }
    found.sort();
    if let Some(newest) = found.pop() {
        return Ok(newest);
    }
    // Linux, and anyone who put it on the path themselves.
    if Command::new("blender").arg("--version").output().is_ok() {
        return Ok(PathBuf::from("blender"));
    }
    Err("cannot find Blender: set DARK_BLENDER to its executable".into())
}

/// The bake script, which lives beside the engine rather than in the game project.
fn script() -> Result<PathBuf, String> {
    // From the built binary, walk up out of `target/<profile>/`; from `cargo run`, the manifest
    // directory is the crate's, so the repository root is two above it. The walk stops at the
    // checkout it finds rather than running on to the drive root, where a stray
    // `C:/tools/bake_model.py` would otherwise be picked up and run.
    let repo = |dir: &Path| dir.join("Cargo.toml").exists() || dir.join(".git").exists();
    let mut roots: Vec<PathBuf> = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")];
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            roots.push(dir.to_path_buf());
            if repo(dir) {
                break;
            }
        }
    }
    roots.push(PathBuf::from("."));
    for root in roots {
        let path = root.join("tools/bake_model.py");
        if path.exists() {
            return Ok(path);
        }
    }
    Err("cannot find tools/bake_model.py: run it from the engine checkout".into())
}

pub fn bake_model(project: &str, model: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project = Project::open(project)?;
    let def = project.load_model_def(model)?;
    let source = project.path(&def.source);
    if !source.exists() {
        return Err(format!("{} is not there", source.display()).into());
    }
    let mesh_out = project.path(&def.mesh);
    let baked_out = project.path(&def.baked);
    // Blender writes its measurements here and the engine turns them into the bake. A failed run
    // must not leave a stale bake looking fresh, so the file it writes is never the bake itself.
    let measure_out = baked_out.with_extension("measure.ron");
    let _ = std::fs::remove_file(&measure_out);
    let (blender, script) = (blender()?, script()?);

    let mut command = Command::new(&blender);
    command
        .arg("--background")
        .arg("--factory-startup")
        // Without this Blender exits 0 on any exception that is not a `SystemExit`: a locked
        // output file, a corrupt export, a changed API. The bake would then be judged on
        // whatever the script had managed to write.
        .arg("--python-exit-code")
        .arg("1")
        .arg("--python")
        .arg(&script)
        .arg("--")
        .arg(&source)
        .arg(&mesh_out)
        .arg(&measure_out)
        .arg(def.scale.to_string());
    for (action, animation, _) in def.clips() {
        // The engine's name for it travels with the animation's own, so the script writes clips
        // the engine can look up and can say which animation it could not find.
        command.arg(format!("{action}={animation}"));
    }
    let output = command
        .output()
        .map_err(|e| format!("running {}: {e}", blender.display()))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let why = text
            .lines()
            .find(|l| l.contains("bake-model:"))
            .unwrap_or("see Blender's output above");
        eprintln!("{text}");
        return Err(why.trim().to_owned().into());
    }

    // Blender measured it; the engine owns the arithmetic and what only it knows. Reading the
    // measurement back is also the check that the script and `ModelMeasure` still agree.
    let measure = ModelMeasure::read(&measure_out)?;
    let bake = measure.bake(&def, source_hash(&std::fs::read(&source)?));
    let missing: Vec<String> = def
        .clips()
        .into_iter()
        .map(|(action, ..)| action)
        .filter(|action| !bake.clips.contains_key(action))
        .collect();
    if !missing.is_empty() {
        return Err(format!("Blender measured no {}", missing.join(", ")).into());
    }
    bake.write(&baked_out)?;
    let _ = std::fs::remove_file(&measure_out);

    let events: usize = bake.clips.values().map(|c| c.events.len()).sum();
    let looping = bake.clips.values().filter(|c| c.looping).count();
    println!(
        "{}: {} clips ({looping} looping, {events} events), {} bones, {:.0} px tall",
        baked_out.display(),
        bake.clips.len(),
        bake.bones,
        bake.height
    );
    println!("{}: mesh written", mesh_out.display());
    Ok(())
}
