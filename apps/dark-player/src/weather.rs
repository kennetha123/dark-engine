//! The sky overhead (`journals/engine/05` phase 2).
//!
//! Which sky it is comes from `dark_sim::WeatherDef` in the project's `world.ron`: a pure
//! function of the day, the hour and the region, so every player holding the same project sees
//! the same weather without a word of it being sent. What that sky *looks* like comes from
//! `fx/weather.ron` through `dark_fx::WeatherArt`.
//!
//! All of it lives here rather than in `demo.rs`, which gains only a field, a line in each of its
//! two constructors and one call — a whole feature that can be committed on its own.

use std::collections::HashMap;

use dark_assets::{LoadedSheet, Project};
use dark_fx::{Art, EffectDef, EffectId, Effects, Handle};
use dark_render::{Mesh, RenderError, Renderer, Sprite, TextureId, layer};
use dark_sim::{CLEAR, CalendarDef, WeatherDef, WorldDef};
use glam::Vec2;

/// The weather as read off disk, before anything is on the GPU. The same split the sheets use:
/// [`crate::demo::DemoScene`] holds this, and `build_view` turns it into a [`Weather`].
pub struct WeatherLoad {
    art: dark_fx::WeatherArt,
    def: WeatherDef,
    calendar: CalendarDef,
    /// Each sky that draws something: its id, its effect, and the sheet that effect names.
    skies: Vec<(String, EffectDef, LoadedSheet)>,
}

impl WeatherLoad {
    /// Reads the project's weather. **Never fails**: a game whose sky will not load is a game
    /// with a plain sky, not a game that will not start, and what went wrong is logged.
    pub fn read(project: &Project) -> Self {
        let (def, calendar) = match WorldDef::load(&project.path("world.ron")) {
            Ok(world) => (world.weather, world.calendar),
            // No world at all is the ordinary case for a game started without a project.
            Err(err) => {
                tracing::debug!("no world weather: {err}");
                (WeatherDef::default(), CalendarDef::default())
            }
        };
        let art = dark_fx::WeatherArt::load_or_default(project, "fx/weather.ron")
            .inspect_err(|err| tracing::error!("fx/weather.ron: {err}"))
            .unwrap_or_default();

        let mut skies = Vec::new();
        for (id, sky) in &art.skies {
            let Some(path) = &sky.effect else { continue };
            match Self::one(project, path) {
                Ok((effect, sheet)) => skies.push((id.clone(), effect, sheet)),
                Err(err) => tracing::error!("the {id} sky draws nothing: {err}"),
            }
        }
        Self {
            art,
            def,
            calendar,
            skies,
        }
    }

    fn one(
        project: &Project,
        path: &str,
    ) -> Result<(EffectDef, LoadedSheet), Box<dyn std::error::Error>> {
        let effect = EffectDef::load(project, path)?;
        let sheet = project.load_sheet(&effect.sheet)?;
        Ok((effect, sheet))
    }

    /// Puts the sheets on the GPU.
    pub fn upload(self, renderer: &mut Renderer) -> Result<Weather, RenderError> {
        let mut fx = Effects::new();
        let mut kinds = HashMap::new();
        for (id, effect, sheet) in self.skies {
            let texture = renderer.create_texture(
                &format!("sky {id}"),
                sheet.image.width,
                sheet.image.height,
                &sheet.image.rgba,
            )?;
            let frames: Vec<_> = effect
                .frames
                .iter()
                .filter_map(|f| sheet.sheet.frames.get(*f as usize).map(|frame| frame.rect))
                .collect();
            let art = Art {
                texture,
                page: Vec2::new(sheet.image.width as f32, sheet.image.height as f32),
                frames,
            };
            match fx.add(effect, art) {
                Ok(kind) => {
                    kinds.insert(id, kind);
                }
                Err(err) => tracing::error!("the {id} sky draws nothing: {err}"),
            }
        }
        Ok(Weather {
            art: self.art,
            def: self.def,
            calendar: self.calendar,
            fx,
            kinds,
            showing: None,
            tint: [0.0; 4],
            forced: None,
            dark: dark_render::DAYLIGHT,
            lights: Vec::new(),
            seconds: 0.0,
        })
    }
}

/// The sky, playing.
pub struct Weather {
    art: dark_fx::WeatherArt,
    def: WeatherDef,
    calendar: CalendarDef,
    fx: Effects,
    /// The effect each sky plays, by sky id. A sky that would not load is simply missing.
    kinds: HashMap<String, EffectId>,
    /// The sky now, and the emitter playing it if it has particles at all — fog has none.
    showing: Option<(String, Option<Handle>)>,
    /// The wash over the world as it is at this moment, sliding towards the sky's own.
    tint: [f32; 4],
    forced: Option<String>,
    /// What the world is multiplied by at this hour, and the glows that push back against it
    /// (`journals/engine/05` phase 5). Worked out in [`Weather::draw`] and read by the renderer.
    dark: [f32; 3],
    lights: Vec<Sprite>,
    /// Seconds drawn, for the fire's flicker.
    seconds: f32,
}

impl Weather {
    /// What the world is multiplied by this frame: white by day, dim and blue at night.
    pub fn darkness(&self) -> [f32; 3] {
        self.dark
    }

    /// The glows drawn over the darkened world.
    pub fn lights(&self) -> &[Sprite] {
        &self.lights
    }

    /// `--weather`: one sky, whatever the calendar says, so a picture can be taken of it.
    pub fn force(&mut self, sky: Option<String>) {
        self.forced = sky;
    }

    /// The sky over `region` at this day and hour, or what `--weather` insists on.
    ///
    /// `time` is the day (counted from zero, as the clock counts) and the hour. The calendar
    /// counts days from one, which is where the `+ 1` goes and nowhere else.
    pub fn sky(&self, time: Option<(u32, f32)>, region: Option<&str>) -> &str {
        if let Some(forced) = &self.forced {
            return forced;
        }
        let (day, hour) = time.unwrap_or((0, 12.0));
        self.def.at(&self.calendar, day + 1, hour, region)
    }

    /// Advances the sky and draws it: the particles into `meshes`, the wash into `sprites`.
    ///
    /// The wash goes on the interface's layer, below everything drawn there, so it lies over the
    /// whole world — canopies and all — and under the writing.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        time: Option<(u32, f32)>,
        region: Option<&str>,
        camera: Vec2,
        (view_min, view_size): (Vec2, Vec2),
        white: TextureId,
        dt: f32,
        structures: &[dark_world::StructureSnapshot],
        meshes: &mut Vec<Mesh>,
        sprites: &mut Vec<Sprite>,
    ) {
        self.seconds += dt;
        self.night(time, structures, white);
        let wanted = self.sky(time, region).to_owned();
        let changed = self.showing.as_ref().is_none_or(|(now, _)| *now != wanted);
        if changed {
            // What was falling finishes falling; nothing is cut off mid-air.
            if let Some((_, Some(handle))) = self.showing.take() {
                self.fx.stop(handle);
            }
            // A sky with a tint and no particles — fog — is still the sky that is showing.
            let playing = self
                .kinds
                .get(&wanted)
                .map(|k| self.fx.standing(*k, camera));
            if playing.is_some() || (wanted != CLEAR && self.art.sky(&wanted).is_some()) {
                self.showing = Some((wanted.clone(), playing));
            }
        }
        // Weather stands on the camera: it is the screen's, not a place's.
        if let Some((_, Some(handle))) = &self.showing {
            self.fx.move_to(*handle, camera);
        }
        self.fx.update(dt);
        self.fx.meshes(meshes);

        let want = self
            .art
            .sky(&wanted)
            .map_or([0.0; 4], |sky| sky.tint)
            .map(|c| c.clamp(0.0, 1.0));
        self.ease(want, dt);
        if self.tint[3] > 0.002 {
            let mut wash = Sprite::fill(white, view_min, view_size, self.tint);
            wash.layer = layer::UI;
            // Below everything else drawn on that layer: the wash is weather, not writing.
            wash.sort_y = -2e9;
            sprites.push(wash);
        }
    }

    /// How dark it is, and what is burning against it.
    ///
    /// A campfire is the only thing that gives light today. Its glow flickers, because a fire
    /// that does not is a lamp; the flicker is a pair of sines whose periods do not divide into
    /// one another, so it never repeats on a beat the eye can catch.
    fn night(
        &mut self,
        time: Option<(u32, f32)>,
        structures: &[dark_world::StructureSnapshot],
        white: TextureId,
    ) {
        let hour = time.map_or(12.0, |(_, hour)| hour);
        self.dark = self.art.darkness(hour);
        self.lights.clear();
        // Nothing to light, and nothing would show: a glow added to a fully lit world is a
        // bright smudge on the grass.
        if self.dark == dark_render::DAYLIGHT {
            return;
        }
        for structure in structures {
            let dark_life::Structure::Campfire { minutes } = structure.structure else {
                continue;
            };
            if minutes == 0 {
                continue;
            }
            let flicker =
                1.0 + 0.06 * (self.seconds * 5.3).sin() + 0.04 * (self.seconds * 11.7).sin();
            // It burns down: the last of a fire lights less than a new one, over the final hour.
            let left = (minutes as f32 / 60.0).clamp(0.25, 1.0);
            let reach = 88.0 * flicker * (0.6 + 0.4 * left);
            let mut glow = Sprite::fill(
                white,
                structure.at - Vec2::splat(reach),
                Vec2::splat(reach * 2.0),
                // Firelight. The alpha is how strong it is, and it is kept low on purpose: the
                // pass adds, so anything near full blows the middle out to white and takes the
                // fire's own picture with it.
                [1.0, 0.66, 0.34, 0.30 * left],
            );
            glow.kind = dark_render::SpriteKind::Light;
            self.lights.push(glow);
        }
    }

    /// Slides the wash towards `want` at a steady rate, so a sky takes `change_secs` to arrive
    /// however far it has to travel. Evenly rather than easing, because a designer setting six
    /// seconds should get six seconds.
    fn ease(&mut self, want: [f32; 4], dt: f32) {
        let step = if self.art.change_secs > 0.0 {
            dt / self.art.change_secs
        } else {
            1.0
        };
        for (now, want) in self.tint.iter_mut().zip(want) {
            let gap = want - *now;
            *now += gap.clamp(-step, step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weather(forced: Option<&str>) -> Weather {
        let art: dark_fx::WeatherArt =
            ron::from_str(r#"(change_secs: 2.0, skies: {"rain": (tint: (0.5, 0.5, 1.0, 0.4))})"#)
                .expect("the test's own art parses");
        // Built rather than parsed: what a whole `world.ron` must contain is not this test's
        // business, and `WeatherDef` is the only part of one it reads.
        let def = WeatherDef {
            spell_hours: 24,
            chances: vec![dark_sim::SkyChance {
                season: None,
                region: Some("wilds".into()),
                skies: [("rain".to_owned(), 1)].into_iter().collect(),
            }],
        };
        Weather {
            art,
            def,
            calendar: CalendarDef::default(),
            fx: Effects::new(),
            kinds: HashMap::new(),
            showing: None,
            tint: [0.0; 4],
            forced: forced.map(str::to_owned),
            dark: dark_render::DAYLIGHT,
            lights: Vec::new(),
            seconds: 0.0,
        }
    }

    /// The clock counts days from zero and the calendar from one. Getting that wrong would put
    /// every sky one day out all year, which nothing on the screen would ever give away.
    ///
    /// Pinned with a season that begins on the *second* day, so day zero on the clock and day one
    /// on the clock have to land on different sides of it. A calendar with no seasons could not
    /// tell the two apart, and neither could a test built on one.
    #[test]
    fn the_clocks_first_day_is_the_calendars_first_day() {
        let mut sky = weather(None);
        sky.calendar = CalendarDef {
            seasons: vec![
                dark_sim::SeasonDef {
                    id: "opening".into(),
                    name: "Opening".into(),
                    from_day: 1,
                    warmth: 0,
                },
                dark_sim::SeasonDef {
                    id: "after".into(),
                    name: "After".into(),
                    from_day: 2,
                    warmth: 0,
                },
            ],
            events: Vec::new(),
        };
        sky.def.chances = vec![
            dark_sim::SkyChance {
                season: Some("opening".into()),
                region: None,
                skies: [("rain".to_owned(), 1)].into_iter().collect(),
            },
            dark_sim::SkyChance {
                season: Some("after".into()),
                region: None,
                skies: [("snow".to_owned(), 1)].into_iter().collect(),
            },
        ];
        assert_eq!(
            sky.sky(Some((0, 12.0)), None),
            "rain",
            "day zero on the clock is the year's first day"
        );
        assert_eq!(
            sky.sky(Some((1, 12.0)), None),
            "snow",
            "and day one on the clock is its second"
        );
    }

    /// `--weather` is for taking a picture of a sky, so it has to overrule the calendar
    /// everywhere and at every hour.
    #[test]
    fn the_flag_overrules_the_calendar() {
        let sky = weather(Some("snow"));
        assert_eq!(sky.sky(Some((0, 12.0)), Some("wilds")), "snow");
        assert_eq!(sky.sky(None, None), "snow");
        // And without it, a place with no region still has a sky rather than a panic.
        assert_eq!(weather(None).sky(None, None), CLEAR);
    }

    /// A sky takes the time its file asks for, whatever the frame rate and however far the wash
    /// has to travel.
    #[test]
    fn a_sky_takes_the_time_it_says_to_arrive() {
        for steps in [60u32, 15] {
            let mut sky = weather(Some("rain"));
            let mut meshes = Vec::new();
            let mut sprites = Vec::new();
            // Two seconds of frames, which is this art's `change_secs`.
            for _ in 0..(steps * 2) {
                sky.draw(
                    None,
                    None,
                    Vec2::ZERO,
                    (Vec2::ZERO, Vec2::new(320.0, 180.0)),
                    TextureId::FIRST,
                    1.0 / steps as f32,
                    &[],
                    &mut meshes,
                    &mut sprites,
                );
            }
            assert!(
                (sky.tint[3] - 0.4).abs() < 1e-3,
                "at {steps} a second the wash reached {:?}",
                sky.tint
            );
            let wash = sprites.last().expect("a wash was drawn");
            assert_eq!(wash.layer, layer::UI);
            assert!(wash.sort_y < -1e9, "under everything else on that layer");
        }
    }

    /// Clear weather draws nothing at all — no wash, no sprite, nothing to pay for.
    #[test]
    fn a_clear_sky_draws_nothing() {
        let mut sky = weather(None);
        let (mut meshes, mut sprites) = (Vec::new(), Vec::new());
        for _ in 0..120 {
            sky.draw(
                Some((0, 12.0)),
                Some("nowhere"),
                Vec2::ZERO,
                (Vec2::ZERO, Vec2::new(320.0, 180.0)),
                TextureId::FIRST,
                1.0 / 60.0,
                &[],
                &mut meshes,
                &mut sprites,
            );
        }
        assert!(
            sprites.is_empty() && meshes.is_empty(),
            "a clear sky is free"
        );
    }
}
