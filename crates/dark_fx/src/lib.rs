//! Effects written as data (`journals/engine/05`): rain, snow, smoke, sparks, dust.
//!
//! An effect is a `*.fx.ron` file — a handful of numbers an artist can change without a
//! compiler. What runs is in [`live`]; what a file says is here.
//!
//! This is **presentation**, and it is in `FORBIDDEN` in `tools/check-sim-deps.sh` so no
//! simulation crate can reach it. The rule that keeps it honest: *an effect the world reacts to
//! is not an effect.* A campfire's warmth is a climate in `life.ron`; the flame is this. Nothing
//! here is networked, queried, or visible to the simulation, which is also why it may use frame
//! delta where gameplay may not.

use std::path::Path;

use dark_assets::Project;
use dark_render::layer;
use serde::{Deserialize, Serialize};

mod live;
mod weather;
pub use live::{Art, EffectId, Effects, Handle, Particle};
pub use weather::{Sky, WeatherArt};

/// A span a value is picked from, evenly. A single number in the file means a fixed value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Range {
    pub low: f32,
    pub high: f32,
}

impl Range {
    pub const fn at(value: f32) -> Self {
        Self {
            low: value,
            high: value,
        }
    }

    /// `fraction` is 0 to 1.
    pub fn pick(self, fraction: f32) -> f32 {
        self.low + (self.high - self.low) * fraction
    }
}

impl Default for Range {
    fn default() -> Self {
        Self::at(0.0)
    }
}

/// Written as `1.5` or as `(1.0, 2.0)`, because most of these never vary and writing
/// `(1.5, 1.5)` for every one of them would bury the ones that do.
impl<'de> Deserialize<'de> for Range {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Fixed(f32),
            Span(f32, f32),
        }
        Ok(match Written::deserialize(d)? {
            Written::Fixed(value) => Range::at(value),
            Written::Span(low, high) => Range { low, high },
        })
    }
}

/// A value at birth and the same value at death; what happens between is even.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct Fade<T> {
    pub from: T,
    pub to: T,
}

impl<T> Fade<T> {
    pub const fn new(from: T, to: T) -> Self {
        Self { from, to }
    }
}

/// How fast a particle drifts, in pixels a second: across, up off the ground, and down the map.
///
/// Each one on its own, so `drift: (lift: (-260.0, -220.0))` is all falling rain has to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Drift {
    #[serde(default)]
    pub x: Range,
    #[serde(default)]
    pub lift: Range,
    #[serde(default)]
    pub y: Range,
}

/// A `*.fx.ron` file.
///
/// Unknown fields are refused. Nearly every field here has a default, so serde would otherwise
/// read a misspelt `color` as no colour at all and an artist would tune a number that was never
/// being read — which is the whole failure this crate exists to avoid.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectDef {
    /// The sheet its pictures come from.
    pub sheet: String,
    /// Which frames of it a particle may be. One is picked at birth and kept.
    pub frames: Vec<u32>,
    /// Born a second. Zero, with a `burst`, is a one-shot.
    #[serde(default)]
    pub rate: f32,
    /// Born at once when it starts.
    #[serde(default)]
    pub burst: u32,
    /// How long one lives, in seconds. A second unless said otherwise.
    #[serde(default = "a_second")]
    pub life: Range,
    /// Half-extents of the box they are born in, around where the effect stands.
    #[serde(default)]
    pub area: (f32, f32),
    /// How high off the ground they start.
    #[serde(default)]
    pub lift: Range,
    #[serde(default)]
    pub drift: Drift,
    /// Pulls `lift` down over time, in pixels a second squared. Negative falls.
    #[serde(default)]
    pub gravity: f32,
    /// How much of its speed a particle loses a second, 0 to 1.
    #[serde(default)]
    pub drag: f32,
    /// Whether a particle that falls below the ground is gone. Rain lands; smoke does not fall,
    /// and dust that settles wants to lie there for the rest of its life.
    #[serde(default)]
    pub lands: bool,
    /// Scale at birth and at death.
    #[serde(default = "one_to_one")]
    pub size: Fade<f32>,
    /// Turns a second.
    #[serde(default)]
    pub spin: Range,
    /// Colour at birth and at death, linear RGBA. Fading out is the alpha reaching zero.
    #[serde(default = "white_to_clear")]
    pub colour: Fade<[f32; 4]>,
    /// Where it sits in the world's order: `dark_render::layer`. Weather belongs above everything
    /// standing, a campfire's smoke among it.
    #[serde(default = "default_layer")]
    pub layer: i32,
    /// At most this many alive at once. A rate and a life that would ask for more drop the
    /// newcomers rather than growing without limit: a wrong number in a file must not be able to
    /// take the frame rate with it.
    #[serde(default = "default_cap")]
    pub cap: u32,
}

fn a_second() -> Range {
    Range::at(1.0)
}

fn one_to_one() -> Fade<f32> {
    Fade::new(1.0, 1.0)
}

fn white_to_clear() -> Fade<[f32; 4]> {
    Fade::new([1.0; 4], [1.0, 1.0, 1.0, 0.0])
}

fn default_cap() -> u32 {
    512
}

fn default_layer() -> i32 {
    layer::WORLD
}

#[derive(Debug, thiserror::Error)]
pub enum EffectError {
    #[error("{path}: {source}")]
    Ron {
        path: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },
    #[error("{path}: {what}")]
    Invalid { path: String, what: String },
}

impl EffectDef {
    /// Reads a `*.fx.ron`, checked. An effect that cannot make sense is refused here rather than
    /// drawing nothing later and leaving somebody to wonder why.
    pub fn load(project: &Project, path: impl AsRef<Path>) -> Result<Self, EffectError> {
        let shown = path.as_ref().to_string_lossy().into_owned();
        let text =
            std::fs::read_to_string(project.path(&path)).map_err(|e| EffectError::Invalid {
                path: shown.clone(),
                what: e.to_string(),
            })?;
        let def: EffectDef = ron::from_str(&text).map_err(|source| EffectError::Ron {
            path: shown.clone(),
            source: Box::new(source),
        })?;
        def.check(&shown)?;
        Ok(def)
    }

    /// Refuses an effect that could never be seen. `named` is what the message calls it: the file
    /// it was read from, or the sheet it draws, for one built in memory by the editor.
    ///
    /// Public because [`Effects::add`] calls it too. A def does not only arrive from a file.
    pub fn check(&self, named: &str) -> Result<(), EffectError> {
        let bad = |what: &str| EffectError::Invalid {
            path: named.to_owned(),
            what: what.to_owned(),
        };
        if self.frames.is_empty() {
            return Err(bad("no frames, so nothing would be drawn"));
        }
        if self.life.high <= 0.0 {
            return Err(bad("a life of no time, so nothing would be seen"));
        }
        if self.rate <= 0.0 && self.burst == 0 {
            return Err(bad("no rate and no burst, so nothing is ever born"));
        }
        if self.cap == 0 {
            return Err(bad("a cap of none, so nothing is ever born"));
        }
        if !(0.0..=1.0).contains(&self.drag) {
            return Err(bad("drag is how much speed is lost a second, from 0 to 1"));
        }
        Ok(())
    }

    /// How many can be alive at once, given the rate and how long one lives. The cap is what is
    /// actually enforced; this is what the file is asking for.
    pub fn wanted(&self) -> f32 {
        self.burst as f32 + self.rate * self.life.high
    }

    /// The scale a particle is drawn at, `fraction` of the way through its life.
    pub fn size_at(&self, fraction: f32) -> f32 {
        self.size.from + (self.size.to - self.size.from) * fraction
    }

    /// Its colour, `fraction` of the way through its life.
    pub fn colour_at(&self, fraction: f32) -> [f32; 4] {
        let Fade { from, to } = self.colour;
        std::array::from_fn(|i| from[i] + (to[i] - from[i]) * fraction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the two fields that have no sensible default are filled in; the test says the rest,
    /// so nothing here can quietly shadow what a test is trying to write.
    fn written(rest: &str) -> Result<EffectDef, ron::error::SpannedError> {
        ron::from_str(&format!(r#"(sheet: "s.sheet.ron", frames: [0], {rest})"#))
    }

    /// Most of these numbers never vary, so a single value has to be as writable as a span —
    /// otherwise the ones that *do* vary are buried in `(x, x)` pairs.
    #[test]
    fn a_number_and_a_span_are_both_a_range() {
        let def = written("lift: 40.0").expect("a fixed lift");
        assert_eq!(def.lift, Range::at(40.0));
        let def = written("lift: (10.0, 20.0)").expect("a span");
        assert_eq!(
            def.lift,
            Range {
                low: 10.0,
                high: 20.0
            }
        );
        assert_eq!(def.lift.pick(0.0), 10.0);
        assert_eq!(def.lift.pick(0.5), 15.0);
        assert_eq!(def.lift.pick(1.0), 20.0);
    }

    /// What a file leaves out has to be what it would have written anyway.
    #[test]
    fn what_is_left_out_is_still_sensible() {
        let def = written("burst: 1").expect("the smallest effect there is");
        assert_eq!(
            def.size,
            Fade::new(1.0, 1.0),
            "no size means no change of size"
        );
        assert_eq!(def.colour.from, [1.0; 4], "born white");
        assert_eq!(def.colour.to[3], 0.0, "and fading out");
        assert_eq!(def.life, Range::at(1.0), "and it lives a second");
        assert_eq!(def.gravity, 0.0);
        assert!(def.cap > 0, "there is always a cap");
        assert_eq!(def.layer, layer::WORLD, "among the things standing in it");
        assert_eq!(def.drift, Drift::default(), "and going nowhere");
    }

    /// Size and colour have to actually reach the values the file's second entry names, or the
    /// end of a fade is a number nobody can see the effect of.
    #[test]
    fn a_fade_reaches_the_values_it_names() {
        let def = written(
            "burst: 1, size: (from: 0.5, to: 4.0), \
             colour: (from: (1.0, 1.0, 1.0, 1.0), to: (0.0, 0.5, 0.0, 0.0))",
        )
        .expect("parses");
        assert_eq!(def.size_at(0.0), 0.5);
        assert_eq!(def.size_at(1.0), 4.0);
        assert_eq!(def.size_at(0.5), 2.25);
        assert_eq!(def.colour_at(0.0), [1.0; 4]);
        assert_eq!(def.colour_at(1.0), [0.0, 0.5, 0.0, 0.0]);
        assert_eq!(def.colour_at(0.5), [0.5, 0.75, 0.5, 0.5]);
    }

    /// Nearly every field has a default, so a misspelt one would otherwise be read as its default
    /// and an artist would sit there tuning a number nothing reads.
    #[test]
    fn a_field_that_is_not_a_field_is_refused() {
        let err =
            written("burst: 1, color: (from: (1.0, 1.0, 1.0, 1.0), to: (0.0, 0.0, 0.0, 0.0))")
                .expect_err("`color` is not a field; `colour` is");
        assert!(err.to_string().contains("color"), "{err}");
        // Including inside a nested one, where a typo is just as quiet.
        written("burst: 1, drift: (up: 20.0)").expect_err("it is `lift`, not `up`");
    }

    /// An effect that cannot be seen is a mistake in a file, and the file should say so rather
    /// than drawing nothing and leaving somebody to wonder.
    #[test]
    fn an_effect_that_could_never_be_seen_is_refused() {
        let refuses = |body: &str, because: &str| {
            let def: EffectDef = ron::from_str(body).expect("it parses");
            let err = def.check("t.fx.ron").expect_err(because);
            assert!(
                err.to_string().contains("t.fx.ron"),
                "the message names the file: {err}"
            );
        };
        refuses(
            r#"(sheet: "s.sheet.ron", frames: [], rate: 10.0)"#,
            "no frames",
        );
        refuses(
            r#"(sheet: "s.sheet.ron", frames: [0], rate: 10.0, life: 0.0)"#,
            "no time to live",
        );
        refuses(r#"(sheet: "s.sheet.ron", frames: [0])"#, "never born");
        refuses(
            r#"(sheet: "s.sheet.ron", frames: [0], rate: 10.0, cap: 0)"#,
            "no room to be born into",
        );
        refuses(
            r#"(sheet: "s.sheet.ron", frames: [0], rate: 10.0, drag: 2.0)"#,
            "drag out of range",
        );

        // And the smallest sensible one is accepted, as is a one-shot with no rate.
        written("rate: 10.0")
            .expect("parses")
            .check("t.fx.ron")
            .expect("the smallest effect is fine");
        written("burst: 8")
            .expect("parses")
            .check("t.fx.ron")
            .expect("a one-shot is fine");
    }

    /// The count a file is asking for, which is what a cap is judged against.
    #[test]
    fn what_a_file_asks_for_is_its_rate_times_its_life() {
        let def = written("life: (1.0, 2.0), rate: 50.0, burst: 5").expect("parses");
        assert_eq!(def.wanted(), 5.0 + 100.0);
    }
}
