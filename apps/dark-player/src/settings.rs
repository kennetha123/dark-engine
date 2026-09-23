//! What a player has set (docs/PLAN.md §22), kept in `<project>/saves/settings.ron` so it is
//! the same the next time they play.
//!
//! Settings are the player's, not the project's: a project says what languages there are, and
//! this says which one they read. A file that will not read is simply the settings a new player
//! would have, and is written over the next time anything changes.

use std::path::{Path, PathBuf};

use dark_assets::Project;
use serde::{Deserialize, Serialize};

/// How loud the game is, as the settings screen counts: tenths, 0 to 10.
pub const LOUDEST: u8 = 10;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The language read, one of the project's; none means its first.
    pub language: Option<String>,
    /// Loudness in tenths, 0 (silent) to [`LOUDEST`].
    pub volume: u8,
    pub fullscreen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: None,
            volume: LOUDEST,
            fullscreen: false,
        }
    }
}

impl Settings {
    /// Loudness as the mixer wants it: 0 to 1.
    pub fn loudness(&self) -> f32 {
        f32::from(self.volume.min(LOUDEST)) / f32::from(LOUDEST)
    }

    /// A tenth louder or quieter. Taking it (rather than nudging it left or right) goes up,
    /// and past the top comes round to silence, so one key is enough to reach any of it.
    pub fn change_volume(&mut self, delta: i8) {
        let volume = i16::from(self.volume) + i16::from(delta);
        self.volume = match volume {
            _ if volume > i16::from(LOUDEST) => 0,
            _ if volume < 0 => LOUDEST,
            volume => volume as u8,
        };
    }
}

fn file(project: &Project) -> PathBuf {
    project.path("saves/settings.ron")
}

/// What this player has set, or what a new one would have.
pub fn load(project: &Project) -> Settings {
    read(&file(project)).unwrap_or_default()
}

fn read(path: &Path) -> Option<Settings> {
    let text = std::fs::read_to_string(path).ok()?;
    match ron::from_str(&text) {
        Ok(settings) => Some(settings),
        Err(err) => {
            tracing::warn!(
                "{}: {err}; starting from the usual settings",
                path.display()
            );
            None
        }
    }
}

/// Keeps them for next time. Nothing a player can do about a failure, so it is only logged.
pub fn save(project: &Project, settings: &Settings) {
    let path = file(project);
    let written = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())
        .and_then(|text| {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
            }
            // Beside it first, then renamed over: a crash while the volume is being set (which
            // writes this file at every press) must not leave half a settings file behind.
            let partial = path.with_extension("ron.partial");
            std::fs::write(&partial, text).map_err(|e| e.to_string())?;
            std::fs::rename(&partial, &path).map_err(|e| e.to_string())
        });
    if let Err(err) = written {
        tracing::warn!("cannot keep the settings in {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_runs_from_silent_to_as_mixed_and_comes_round() {
        let mut settings = Settings::default();
        assert_eq!(settings.loudness(), 1.0, "a new player hears it all");
        settings.change_volume(1);
        assert_eq!(settings.volume, 0, "past the top is silence");
        settings.change_volume(1);
        assert!((settings.loudness() - 0.1).abs() < 0.001);
        settings.change_volume(-1);
        assert_eq!(settings.volume, 0, "quieter again");
        settings.change_volume(-1);
        assert_eq!(
            settings.volume, LOUDEST,
            "below silence is as loud as it goes"
        );
        settings.volume = 200;
        assert_eq!(settings.loudness(), 1.0, "never louder than mixed");
    }

    #[test]
    fn settings_are_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("dark-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("project.ron"),
            r#"(name: "t", tile_size: 16, resolution: (320, 180))"#,
        )
        .unwrap();
        let project = Project::open(&dir).unwrap();
        assert_eq!(load(&project), Settings::default(), "none kept yet");

        let mine = Settings {
            language: Some("ja".into()),
            volume: 3,
            fullscreen: true,
        };
        save(&project, &mine);
        assert_eq!(load(&project), mine);

        // Nonsense in the file is not worth refusing to play over.
        std::fs::write(file(&project), "not settings").unwrap();
        assert_eq!(load(&project), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
