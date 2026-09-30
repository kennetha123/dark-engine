//! What a sky *looks* like: the project's `fx/weather.ron` (`journals/engine/05` phase 2).
//!
//! Which sky is overhead is world truth and lives in `dark_sim::WeatherDef` — it is what the
//! simulation will read the day rain is to make anyone cold. This half is only the picture of
//! one: the effect it draws and how it tints the light.

use std::collections::BTreeMap;
use std::path::Path;

use dark_assets::Project;
use serde::{Deserialize, Serialize};

use dark_render::DAYLIGHT;

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
    /// What the world is multiplied by through the day, by the hour it is reached: white is full
    /// daylight and a dim blue is midnight (`journals/engine/05` phase 5). Between two hours the
    /// colour runs evenly from one to the other, and it wraps around midnight. Empty is daylight
    /// all day, so a project that does not ask for a night does not get one.
    ///
    /// **These are multipliers on light, not colours as they look.** Half here is half the light,
    /// which the eye reads as about three quarters as bright; a night that should look a fifth as
    /// bright is about `0.03`. Written this way because it *is* a multiplication of light, and a
    /// number that did not mean that would be a number nobody could reason about.
    #[serde(default)]
    pub night: Vec<(f32, [f32; 3])>,
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
            night: Vec::new(),
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

    /// What the world is multiplied by at `hour` (0 to 24).
    ///
    /// The entries are read in the order they are written and the day wraps, so the last one runs
    /// round midnight into the first — which is how a night is written at all, since it begins on
    /// one day and ends on the next.
    pub fn darkness(&self, hour: f32) -> [f32; 3] {
        if self.night.is_empty() {
            return DAYLIGHT;
        }
        let hour = hour.rem_euclid(24.0);
        // The entry in force: the last one whose hour has come, or — before the first — the last
        // of the day, which is still running from yesterday.
        let (n, (at, from)) = match self
            .night
            .iter()
            .enumerate()
            .rfind(|(_, (at, _))| *at <= hour)
        {
            Some((n, entry)) => (n, *entry),
            None => (self.night.len() - 1, self.night[self.night.len() - 1]),
        };
        let (next_at, to) = self.night[(n + 1) % self.night.len()];
        // How far from this entry to the next, around the clock.
        let span = (next_at - at).rem_euclid(24.0);
        if span <= 0.0 {
            return from;
        }
        let gone = (hour - at).rem_euclid(24.0) / span;
        std::array::from_fn(|i| from[i] + (to[i] - from[i]) * gone.clamp(0.0, 1.0))
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

    /// The night is why lights exist, so the curve has to mean what a project writes.
    #[test]
    fn the_night_runs_from_hour_to_hour_and_round_midnight() {
        // Deliberately starting at six rather than at midnight: a curve whose first entry is
        // hour zero can never ask what happens *before* the first one, which is the whole of the
        // night and the only place the wrap shows.
        let art = art(r#"(night: [(6.0, (1.0, 1.0, 1.0)), (18.0, (0.2, 0.2, 0.4))])"#);
        // The hours written are exactly what they say.
        assert_eq!(art.darkness(6.0), [1.0; 3]);
        assert_eq!(art.darkness(18.0), [0.2, 0.2, 0.4]);
        // Between two of them, evenly: noon is halfway from dawn to dusk.
        let noon = art.darkness(12.0);
        assert!((noon[0] - 0.6).abs() < 1e-5, "{noon:?}");
        // And round midnight, which is the whole point. From 18:00 to 06:00 is twelve hours, so
        // midnight is halfway back to daylight — reached by carrying the evening entry forward
        // through the small hours, not by falling back on the first entry of the list.
        let midnight = art.darkness(0.0);
        assert!(
            (midnight[0] - 0.6).abs() < 1e-5,
            "midnight came out {midnight:?}"
        );
        let small_hours = art.darkness(3.0);
        assert!(
            (small_hours[0] - 0.8).abs() < 1e-5,
            "03:00 came out {small_hours:?}"
        );
        assert_eq!(art.darkness(24.0), art.darkness(0.0), "the day wraps");
    }

    /// A game that does not ask for a night must be the game it was: lit, all day.
    #[test]
    fn no_night_written_is_daylight_all_day() {
        let art = art(r#"(skies: {})"#);
        for hour in [0.0, 3.0, 12.0, 23.9] {
            assert_eq!(art.darkness(hour), DAYLIGHT, "at {hour}");
        }
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
