//! Audio (docs/PLAN.md §1, §13): FMOD Studio events, played at positions in the world.
//!
//! Presentation only: the simulation never waits on sound, and a game runs silently (with a
//! warning) when FMOD or its banks are missing. Sounds are FMOD Studio events by path
//! (`event:/combat/swing`), authored and mixed by sound designers in FMOD Studio and built into
//! banks; the game says what happened and where, FMOD decides how it sounds.
//!
//! World positions are pixels on the ground plane, +y down the screen. FMOD hears them in metres
//! (`pixels_per_metre`, usually the tile size), with north (screen up) ahead of the listener.

mod fmod;

use std::path::PathBuf;

use glam::Vec2;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("cannot load the FMOD library {path}: {source}")]
    Library {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("the FMOD library has no {name}: {source}")]
    Symbol {
        name: &'static str,
        source: libloading::Error,
    },
    #[error("FMOD {call} failed with error {code}")]
    Fmod { call: &'static str, code: i32 },
    #[error("bank {path}: {source}")]
    Bank {
        path: PathBuf,
        source: Box<AudioError>,
    },
    #[error("path {0} cannot be passed to FMOD")]
    BadPath(PathBuf),
    #[error("FMOD version {0:?} is not like 2.02.30")]
    Version(String),
}

/// Where FMOD and the game's banks are.
#[derive(Clone, Debug)]
pub struct AudioConfig {
    /// The FMOD Studio runtime library (`fmodstudio.dll`, `libfmodstudio.so`).
    pub library: PathBuf,
    /// Banks in load order: the master bank and its strings bank first.
    pub banks: Vec<PathBuf>,
    /// The runtime's version as FMOD numbers it (see [`parse_version`]); the API refuses a
    /// mismatch between the library and the header version the caller claims.
    pub version: u32,
    pub pixels_per_metre: f32,
}

/// "2.02.30" as FMOD's version number, 0x00020230.
pub fn parse_version(text: &str) -> Result<u32, AudioError> {
    let bad = || AudioError::Version(text.to_owned());
    let parts: Vec<u32> = text
        .split('.')
        .map(|p| p.parse::<u32>().map_err(|_| bad()))
        .collect::<Result<_, _>>()?;
    let [major, minor, patch] = parts[..] else {
        return Err(bad());
    };
    if major > 9999 || minor > 99 || patch > 99 {
        return Err(bad());
    }
    // FMOD writes each part's decimal digits as hex digits: 2.02.30 is 0x0002_02_30.
    u32::from_str_radix(&format!("{major:04}{minor:02}{patch:02}"), 16).map_err(|_| bad())
}

/// The game's sound: FMOD Studio when it could start, else silence.
pub struct Audio {
    studio: Option<fmod::Studio>,
    pixels_per_metre: f32,
}

impl Audio {
    /// No sound at all (no project audio, or tools).
    pub fn silent() -> Self {
        Self {
            studio: None,
            pixels_per_metre: 16.0,
        }
    }

    /// FMOD Studio with the banks in `config`, or silence (with a warning saying why).
    pub fn open(config: &AudioConfig) -> Self {
        match fmod::Studio::open(&config.library, &config.banks, config.version) {
            Ok(studio) => {
                tracing::info!(
                    "FMOD Studio running ({} banks from {})",
                    config.banks.len(),
                    config.library.display()
                );
                Self {
                    studio: Some(studio),
                    pixels_per_metre: config.pixels_per_metre.max(1.0),
                }
            }
            Err(err) => {
                tracing::warn!("no sound: {err}");
                Self::silent()
            }
        }
    }

    pub fn is_live(&self) -> bool {
        self.studio.is_some()
    }

    /// Plays `event` (e.g. `event:/combat/hit`) once, at `at` in the world.
    pub fn play(&mut self, event: &str, at: Vec2) {
        let position = self.to_fmod(at);
        if let Some(studio) = &mut self.studio
            && let Err(err) = studio.play(event, position)
        {
            tracing::warn!("{event}: {err}");
        }
    }

    /// Where the listener is: usually what the camera follows.
    pub fn set_listener(&mut self, at: Vec2) {
        let position = self.to_fmod(at);
        if let Some(studio) = &mut self.studio
            && let Err(err) = studio.set_listener(position)
        {
            tracing::warn!("{err}");
        }
    }

    /// Once a frame: FMOD mixes and plays what was asked for.
    pub fn update(&mut self) {
        if let Some(studio) = &mut self.studio
            && let Err(err) = studio.update()
        {
            tracing::warn!("{err}");
        }
    }

    fn to_fmod(&self, at: Vec2) -> fmod::Vector {
        fmod::Vector {
            x: at.x / self.pixels_per_metre,
            y: 0.0,
            z: -at.y / self.pixels_per_metre,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_as_fmod_numbers_them() {
        assert_eq!(parse_version("2.02.30").unwrap(), 0x0002_0230);
        assert_eq!(parse_version("2.3.9").unwrap(), 0x0002_0309);
        assert!(parse_version("2.02").is_err());
        assert!(parse_version("2.100.1").is_err());
        assert!(parse_version("two").is_err());
    }

    #[test]
    fn without_fmod_the_game_is_silent_not_broken() {
        let mut audio = Audio::open(&AudioConfig {
            library: PathBuf::from("no/such/fmodstudio.dll"),
            banks: Vec::new(),
            version: 0x0002_0230,
            pixels_per_metre: 16.0,
        });
        assert!(!audio.is_live());
        audio.play("event:/combat/hit", Vec2::ZERO);
        audio.set_listener(Vec2::ONE);
        audio.update();
    }

    #[test]
    fn north_is_ahead_and_east_is_right() {
        let audio = Audio {
            studio: None,
            pixels_per_metre: 16.0,
        };
        let v = audio.to_fmod(Vec2::new(32.0, -48.0));
        assert_eq!((v.x, v.y, v.z), (2.0, 0.0, 3.0));
    }

    /// Plays a real bank when `DARK_TEST_PROJECT` points at a project with FMOD (skipped
    /// otherwise): proves the library, the version and the banks agree.
    #[test]
    fn a_real_project_bank_plays() {
        let Ok(project) = std::env::var("DARK_TEST_PROJECT") else {
            return;
        };
        let root = PathBuf::from(project).join("audio");
        let library = root.join(if cfg!(windows) {
            "lib/windows-x86_64/fmodstudio.dll"
        } else {
            "lib/linux-x86_64/libfmodstudio.so"
        });
        if !library.exists() {
            return;
        }
        let banks = ["Master.bank", "Master.strings.bank"]
            .map(|b| root.join("fmod/Build/Desktop").join(b))
            .to_vec();
        let mut studio = fmod::Studio::open(&library, &banks, 0x0002_0230).expect("FMOD starts");
        studio
            .play("event:/combat/hit", fmod::Vector::default())
            .expect("the event plays");
        assert!(studio.found("event:/combat/hit"));
        studio.update().expect("FMOD updates");
    }
}
