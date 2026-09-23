//! The games a player has going (docs/PLAN.md §22): save files in the project's `saves` folder,
//! listed for the title screen and named for new ones.
//!
//! A save is the host's whole world ([`dark_world::WorldSave`]); this only finds the files and
//! reads enough of each to show it in a list.

use std::path::{Path, PathBuf};

use dark_assets::Project;

/// Games listed on the title screen. Older ones are still in the folder, and still played by
/// putting their file back at the top; reading every world a player ever had would be slow.
const MOST_SHOWN: usize = 12;

/// What the title screen shows for one saved game.
pub struct Slot {
    pub path: PathBuf,
    /// The file's name without `.sav`, as the player will read it.
    pub name: String,
    /// The day the world had reached, counting from 1.
    pub day: u32,
}

/// Where a project's saves live. Beside the game's content, so a packaged game keeps its saves
/// in the folder it was unzipped into.
pub fn folder(project: &Project) -> PathBuf {
    project.path("saves")
}

/// Every saved game, newest first, at most [`MOST_SHOWN`] of them. A file that cannot be read is
/// left out of the list, not hidden from the folder: it stays where it is for the player.
pub fn list(project: &Project) -> Vec<Slot> {
    let Ok(entries) = std::fs::read_dir(folder(project)) else {
        return Vec::new();
    };
    // When each was written is cheap to ask; reading a whole world is not, so only the newest
    // few are read at all.
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "sav"))
        .map(|path| {
            let written = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (written, path)
        })
        .collect();
    files.sort_by_key(|(written, _)| std::cmp::Reverse(*written));
    files
        .into_iter()
        .filter_map(|(_, path)| read(&path))
        .take(MOST_SHOWN)
        .collect()
}

/// A file for a new game: `game`, then `game-2`, `game-3`, … so no game overwrites another.
pub fn fresh(project: &Project) -> PathBuf {
    let dir = folder(project);
    let free = |path: &Path| !path.exists();
    let first = dir.join("game.sav");
    if free(&first) {
        return first;
    }
    (2..1000)
        .map(|n| dir.join(format!("game-{n}.sav")))
        .find(|path| free(path))
        .unwrap_or(first)
}

/// Reads what the list shows of one save: its day and its name.
fn read(path: &Path) -> Option<Slot> {
    let text = std::fs::read_to_string(path).ok()?;
    let save = dark_world::WorldSave::parse(&text).ok()?;
    Some(Slot {
        path: path.to_path_buf(),
        name: path.file_stem()?.to_string_lossy().into_owned(),
        day: save.sim.day() + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("dark-saves-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("saves")).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        Project::open(dir).unwrap()
    }

    #[test]
    fn a_new_game_never_writes_over_one_already_there() {
        let project = project("fresh");
        let first = fresh(&project);
        assert!(first.ends_with("game.sav"));
        std::fs::write(&first, "").unwrap();
        assert!(fresh(&project).ends_with("game-2.sav"));
        std::fs::remove_dir_all(project.path("")).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_save_is_left_out_of_the_list() {
        let project = project("list");
        std::fs::write(project.path("saves/notes.txt"), "hello").unwrap();
        std::fs::write(project.path("saves/broken.sav"), "not a save").unwrap();
        assert!(list(&project).is_empty());
        std::fs::remove_dir_all(project.path("")).unwrap();
    }
}
