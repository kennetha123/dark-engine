//! What a sky *looks* like: the project's `fx/weather.ron` (`journals/engine/05` phase 2).
//!
//! Which sky is overhead is world truth and lives in `dark_sim::WeatherDef` — it is what the
//! simulation will read the day rain is to make anyone cold. This half is only the picture of
//! one: the effect it draws and how it tints the light.

use std::collections::BTreeMap;
use std::path::Path;

use dark_assets::Project;
use serde::{Deserialize, Serialize};

use crate::EffectError;

/// How one sky is drawn.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sky {
    /// The `*.fx.ron` it plays overhead. A sky with none is tint alone — fog, a close grey day.
    #[serde(default)]
    pub effect: Option<String>,
    /// Laid over the whole world, linear RGBA. The alpha is how much of it there is.
    #[serde(default)]
    pub tint: [f32; 4],
}

/// The project's `fx/weather.ron`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherArt {
    /// How long one sky takes to come on, and the one before it to go off.
    #[serde(default = "a_few_seconds")]
    pub change_secs: f32,
    /// By the sky's id in `world.ron`. A sky nothing here names draws nothing, which is how
    /// `clear` works: a project says nothing at all to have clear weather.
    #[serde(default)]
    pub skies: BTreeMap<String, Sky>,
}

fn a_few_seconds() -> f32 {
    6.0
}

impl Default for WeatherArt {
    fn default() -> Self {
        Self {
            change_secs: a_few_seconds(),
            skies: BTreeMap::new(),
        }
    }
}

impl WeatherArt {
    /// Reads `fx/weather.ron`, or the default if the project has none — a game without weather
    /// is a game, and it must not be an error.
    pub fn load_or_default(project: &Project, path: impl AsRef<Path>) -> Result<Self, EffectError> {
        let shown = path.as_ref().to_string_lossy().into_owned();
        let full = project.path(&path);
        if !full.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&full).map_err(|e| EffectError::Invalid {
            path: shown.clone(),
            what: e.to_string(),
        })?;
        let art: WeatherArt = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(&text)
            .map_err(|source| EffectError::Ron {
                path: shown.clone(),
                source: Box::new(source),
            })?;
        if !(art.change_secs.is_finite() && art.change_secs >= 0.0) {
            return Err(EffectError::Invalid {
                path: shown,
                what: "a sky cannot take less than no time to come on".into(),
            });
        }
        Ok(art)
    }

    pub fn sky(&self, id: &str) -> Option<&Sky> {
        self.skies.get(id)
    }

    /// Every `*.fx.ron` named here, once each, for loading them up front.
    pub fn effects(&self) -> impl Iterator<Item = &str> {
        self.skies.values().filter_map(|s| s.effect.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(text: &str) -> WeatherArt {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .expect("the test's own weather parses")
    }

    /// A sky need not have particles at all: fog is a colour over the world.
    #[test]
    fn a_sky_can_be_tint_alone() {
        let art = art(r#"(skies: {
                "rain": (effect: "fx/rain.fx.ron", tint: (0.7, 0.8, 0.9, 0.2)),
                "fog": (tint: (0.8, 0.8, 0.8, 0.3)),
            })"#);
        assert_eq!(art.sky("fog").expect("fog").effect, None);
        assert_eq!(art.sky("rain").expect("rain").tint[3], 0.2);
        assert_eq!(art.change_secs, 6.0, "a sky takes a moment to arrive");
        let named: Vec<&str> = art.effects().collect();
        assert_eq!(named, vec!["fx/rain.fx.ron"], "only the ones that draw");
    }

    /// `clear` is the absence of a sky, so nothing has to say so.
    #[test]
    fn a_sky_nothing_names_draws_nothing() {
        let art = art(r#"(skies: {"rain": (effect: "fx/rain.fx.ron")})"#);
        // `clear` is `dark_sim::CLEAR`, spelled out rather than depending on a simulation crate
        // for a word: nothing here should ever draw it.
        assert!(art.sky("clear").is_none());
        assert!(art.sky("hurricane").is_none());
        assert_eq!(art.sky("rain").expect("rain").tint, [0.0; 4], "and no tint");
    }

    /// A misspelt field would otherwise be read as its default and draw nothing.
    #[test]
    fn a_field_that_is_not_a_field_is_refused() {
        let bad: Result<WeatherArt, _> =
            ron::from_str(r#"(skys: {"rain": (effect: "fx/rain.fx.ron")})"#);
        assert!(bad.is_err(), "`skys` is not `skies`");
        let bad: Result<WeatherArt, _> =
            ron::from_str(r#"(skies: {"rain": (fx: "fx/rain.fx.ron")})"#);
        assert!(bad.is_err(), "`fx` is not `effect`");
    }
}
