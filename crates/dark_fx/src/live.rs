//! What is running: emitters and their particles.
//!
//! Nothing here touches files or the GPU. An effect's pictures are handed in as [`Art`] by
//! whoever loaded the sheet, so this half can be tested without either.

use std::f32::consts::TAU;

use dark_render::{Mesh, MeshVertex, TextureId};
use dark_sprite::Rect;
use glam::Vec2;

use crate::{EffectDef, EffectError};

/// An effect's pictures, once its sheet is loaded: the page, its size in texels, and one
/// rectangle for each entry of [`EffectDef::frames`].
#[derive(Clone, Debug, PartialEq)]
pub struct Art {
    pub texture: TextureId,
    /// The page's size in texels, which is what turns a frame rectangle into uvs.
    pub page: Vec2,
    /// One per entry of the def's `frames`, in the same order.
    pub frames: Vec<Rect>,
}

/// An effect [`Effects`] knows how to play. Given out by, and only meaningful in, the [`Effects`]
/// that was taught it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(u32);

/// One playing emitter. Stays valid until the emitter is gone; calls naming a handle that has
/// finished are ignored rather than being an error, because a caller cannot know the moment a
/// one-shot's last particle dies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Handle(u64);

/// One particle. Nothing about it touches the world: no collision, no query, no callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// Where on the ground it is.
    pub at: Vec2,
    /// How high off the ground, in pixels.
    pub lift: f32,
    /// Pixels a second across the ground.
    pub speed: Vec2,
    /// Pixels a second upwards.
    pub rise: f32,
    pub age: f32,
    pub life: f32,
    /// Turns a second.
    pub spin: f32,
    /// Turns so far.
    pub turn: f32,
    /// Index into [`Art::frames`], picked at birth and kept.
    pub frame: u16,
}

impl Particle {
    /// How far through its life it is, 0 to 1.
    pub fn fraction(&self) -> f32 {
        (self.age / self.life.max(1e-6)).clamp(0.0, 1.0)
    }
}

/// xorshift64*, so each emitter's randomness is its own and a frame redrawn is a frame repeated.
/// Effects are not networked and not deterministic across machines (`journals/engine/05`); this
/// is here to keep one emitter's variation from depending on how many others are alive.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0 to 1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// -1 to 1.
    fn spread(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

struct Kind {
    def: EffectDef,
    art: Art,
}

struct Emitter {
    handle: Handle,
    kind: EffectId,
    at: Vec2,
    /// While true it keeps giving birth. A one-shot is false from the start, and [`Effects::stop`]
    /// is what makes a standing one false — its particles then live out their lives.
    standing: bool,
    /// Births carried over between frames, so a rate is not rounded away by a short frame.
    owed: f32,
    rng: Rng,
    particles: Vec<Particle>,
}

/// Every effect that is playing. One of these lives in the renderer's half of an app.
#[derive(Default)]
pub struct Effects {
    kinds: Vec<Kind>,
    live: Vec<Emitter>,
    next: u64,
    seeds: u64,
}

impl Effects {
    pub fn new() -> Self {
        Self::default()
    }

    /// Teaches it an effect. `art.frames` must hold one rectangle for each of `def.frames`, and
    /// the def itself must be one that could be seen — both refused here rather than drawing
    /// nothing later. A def does not only arrive from a file, so the check cannot live in
    /// [`EffectDef::load`] alone.
    pub fn add(&mut self, def: EffectDef, art: Art) -> Result<EffectId, EffectError> {
        Self::agree(&def, &art)?;
        self.kinds.push(Kind { def, art });
        Ok(EffectId(self.kinds.len() as u32 - 1))
    }

    /// Swaps an effect's definition under its live particles.
    ///
    /// For tuning: dragging a number should change the fall being watched, not start it again
    /// from an empty screen, or there is nothing to judge the number by. What is already in the
    /// air keeps the speed and lifetime it was born with; everything born from here on follows
    /// the new definition.
    ///
    /// Refused, and nothing changed, if the definition and its art disagree — an effect must
    /// never be left half-swapped.
    pub fn retune(&mut self, kind: EffectId, def: EffectDef, art: Art) -> Result<(), EffectError> {
        Self::agree(&def, &art)?;
        self.kinds[kind.0 as usize] = Kind { def, art };
        Ok(())
    }

    /// What [`Self::add`] and [`Self::retune`] both insist on.
    fn agree(def: &EffectDef, art: &Art) -> Result<(), EffectError> {
        def.check(&def.sheet)?;
        if art.frames.len() != def.frames.len() {
            return Err(EffectError::Invalid {
                path: def.sheet.clone(),
                what: format!(
                    "it names {} frame(s) but {} were found in the sheet",
                    def.frames.len(),
                    art.frames.len()
                ),
            });
        }
        if art.page.x <= 0.0 || art.page.y <= 0.0 {
            return Err(EffectError::Invalid {
                path: def.sheet.clone(),
                what: "the sheet's page has no size".into(),
            });
        }
        Ok(())
    }

    pub fn def(&self, kind: EffectId) -> &EffectDef {
        &self.kinds[kind.0 as usize].def
    }

    /// A one-shot at a place: sparks, a footfall. It births at once and never again.
    ///
    /// A file with no `burst` is a standing effect being asked for as a one-shot, so it gets a
    /// second of its rate — or one lifetime's worth, if it lives less than a second. Better than
    /// showing nothing at all for a number an artist left out.
    pub fn burst(&mut self, kind: EffectId, at: Vec2) -> Handle {
        self.start(kind, at, false)
    }

    /// One that keeps going until [`Self::stop`]: weather, a campfire.
    pub fn standing(&mut self, kind: EffectId, at: Vec2) -> Handle {
        self.start(kind, at, true)
    }

    fn start(&mut self, kind: EffectId, at: Vec2, standing: bool) -> Handle {
        self.next += 1;
        self.seeds = self.seeds.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let handle = Handle(self.next);
        let def = &self.kinds[kind.0 as usize].def;
        let mut emitter = Emitter {
            handle,
            kind,
            at,
            standing,
            owed: 0.0,
            rng: Rng::new(self.seeds ^ self.next.wrapping_mul(0x2545_F491_4F6C_DD1D)),
            particles: Vec::new(),
        };
        let opening = match (def.burst, standing) {
            (0, false) => (def.rate * def.life.high.min(1.0)).round() as u32,
            (n, _) => n,
        };
        for _ in 0..opening.min(def.cap) {
            let born = emitter.conceive(def);
            emitter.particles.push(born);
        }
        self.live.push(emitter);
        handle
    }

    /// Where its *new* particles are born. Ones already alive keep going where they are, which is
    /// what makes a torch carried across a room leave its smoke behind.
    pub fn move_to(&mut self, handle: Handle, at: Vec2) {
        if let Some(emitter) = self.live.iter_mut().find(|e| e.handle == handle) {
            emitter.at = at;
        }
    }

    /// Stops the births. What is alive lives out its life, so rain stops by thinning out.
    pub fn stop(&mut self, handle: Handle) {
        if let Some(emitter) = self.live.iter_mut().find(|e| e.handle == handle) {
            emitter.standing = false;
        }
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    /// Advances everything by `dt` seconds.
    ///
    /// Frame delta, deliberately: an effect is presentation, nothing in the simulation can see one,
    /// and so rule 3 (gameplay never reads frame delta) does not reach here.
    pub fn update(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let kinds = &self.kinds;
        for emitter in &mut self.live {
            let def = &kinds[emitter.kind.0 as usize].def;
            if emitter.standing {
                emitter.owed += def.rate * dt;
                let born = emitter.owed.floor().max(0.0);
                emitter.owed -= born;
                for _ in 0..born as u32 {
                    // The cap drops newcomers rather than growing without limit: a wrong number
                    // in a file must not be able to take the frame rate with it.
                    if emitter.particles.len() >= def.cap as usize {
                        break;
                    }
                    let particle = emitter.conceive(def);
                    emitter.particles.push(particle);
                }
            }
            let keep = (1.0 - def.drag).powf(dt);
            emitter.particles.retain_mut(|p| {
                p.age += dt;
                if p.age >= p.life {
                    return false;
                }
                p.rise += def.gravity * dt;
                p.speed *= keep;
                p.rise *= keep;
                p.at += p.speed * dt;
                p.lift += p.rise * dt;
                p.turn += p.spin * dt;
                // Rain stops at the ground rather than falling on through it.
                !(def.lands && p.lift < 0.0)
            });
        }
        // An emitter is gone once it has stopped giving birth and buried its last particle.
        self.live.retain(|e| e.standing || !e.particles.is_empty());
    }

    /// Appends one mesh per emitter, ready for `Renderer::render_scene`.
    ///
    /// One mesh per effect means one `sort_y` per effect, which is right for a thing standing in
    /// a place. Weather covers the screen and sits on a high layer instead of being sorted.
    pub fn meshes(&self, out: &mut Vec<Mesh>) {
        for emitter in &self.live {
            if emitter.particles.is_empty() {
                continue;
            }
            let kind = &self.kinds[emitter.kind.0 as usize];
            let (def, art) = (&kind.def, &kind.art);
            let mut vertices = Vec::with_capacity(emitter.particles.len() * 6);
            for particle in &emitter.particles {
                let Some(rect) = art.frames.get(particle.frame as usize) else {
                    continue;
                };
                let t = particle.fraction();
                let half = Vec2::new(rect.w as f32, rect.h as f32) * 0.5 * def.size_at(t);
                let centre = Vec2::new(particle.at.x, particle.at.y - particle.lift);
                let colour = def.colour_at(t);
                let (sin, cos) = (particle.turn * TAU).sin_cos();
                let uv_min = Vec2::new(rect.x as f32, rect.y as f32) / art.page;
                let uv_max = Vec2::new(rect.right() as f32, rect.bottom() as f32) / art.page;
                let corner = |x: f32, y: f32| MeshVertex {
                    position: centre
                        + Vec2::new(
                            x * half.x * cos - y * half.y * sin,
                            x * half.x * sin + y * half.y * cos,
                        ),
                    uv: Vec2::new(
                        if x < 0.0 { uv_min.x } else { uv_max.x },
                        if y < 0.0 { uv_min.y } else { uv_max.y },
                    ),
                    color: colour,
                };
                vertices.extend([
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ]);
            }
            if vertices.is_empty() {
                continue;
            }
            out.push(Mesh {
                texture: art.texture,
                vertices,
                layer: def.layer,
                sort_y: emitter.at.y,
                // Particles are not bodies: nothing draws a silhouette of smoke.
                body: false,
            });
        }
    }

    /// How many particles are alive, across every emitter.
    pub fn count(&self) -> usize {
        self.live.iter().map(|e| e.particles.len()).sum()
    }

    /// How many emitters are playing.
    pub fn playing(&self) -> usize {
        self.live.len()
    }

    /// One emitter's particles, for inspection and for tests.
    pub fn particles(&self, handle: Handle) -> Option<&[Particle]> {
        self.live
            .iter()
            .find(|e| e.handle == handle)
            .map(|e| e.particles.as_slice())
    }
}

impl Emitter {
    fn conceive(&mut self, def: &EffectDef) -> Particle {
        let rng = &mut self.rng;
        let frame = if def.frames.len() > 1 {
            (rng.next() % def.frames.len() as u64) as u16
        } else {
            0
        };
        Particle {
            at: self.at + Vec2::new(rng.spread() * def.area.0, rng.spread() * def.area.1),
            lift: def.lift.pick(rng.unit()),
            speed: Vec2::new(def.drift.x.pick(rng.unit()), def.drift.y.pick(rng.unit())),
            rise: def.drift.lift.pick(rng.unit()),
            age: 0.0,
            life: def.life.pick(rng.unit()).max(1e-3),
            spin: def.spin.pick(rng.unit()),
            turn: 0.0,
            frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(rest: &str) -> EffectDef {
        ron::from_str(&format!(r#"(sheet: "s.sheet.ron", frames: [0], {rest})"#))
            .expect("the test's own effect parses")
    }

    fn art(frames: usize) -> Art {
        Art {
            texture: TextureId::FIRST,
            page: Vec2::splat(64.0),
            frames: (0..frames)
                .map(|i| Rect::new(i as u32 * 8, 0, 8, 8))
                .collect(),
        }
    }

    fn with(extra: &str) -> (Effects, EffectId) {
        let mut fx = Effects::new();
        let def = def(extra);
        let frames = def.frames.len();
        let id = fx.add(def, art(frames)).expect("art matches the def");
        (fx, id)
    }

    /// A second at the file's rate has to put the file's number of particles in the air, whatever
    /// the frame rate is — the fractional births carried between frames are the whole point.
    #[test]
    fn a_rate_births_what_it_says_over_a_second() {
        for steps in [60u32, 30, 17] {
            let (mut fx, id) = with("rate: 90.0, life: 100.0");
            fx.standing(id, Vec2::ZERO);
            for _ in 0..steps {
                fx.update(1.0 / steps as f32);
            }
            let born = fx.count();
            assert!(
                (89..=91).contains(&born),
                "{steps} steps of a second at 90 a second gave {born}"
            );
        }
    }

    /// A particle is gone when its life is up, and not before.
    #[test]
    fn a_particle_dies_at_its_lifetime() {
        let (mut fx, id) = with("burst: 4, life: 0.5");
        let handle = fx.standing(id, Vec2::ZERO);
        assert_eq!(
            fx.count(),
            4,
            "its burst is born at once, before any update"
        );
        for _ in 0..29 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(
            fx.count(),
            4,
            "at 29 frames of 60 it is not yet half a second"
        );
        fx.update(1.0 / 60.0);
        fx.update(1.0 / 60.0);
        assert_eq!(fx.count(), 0, "past half a second they are all gone");
        assert!(
            fx.particles(handle).is_some(),
            "but it is standing, so it stays"
        );
    }

    /// A wrong number in a file must not be able to take the frame rate with it.
    #[test]
    fn the_cap_holds_however_much_a_file_asks_for() {
        let (mut fx, id) = with("rate: 100000.0, life: 100.0, cap: 32");
        let def_wants = fx.def(id).wanted();
        fx.standing(id, Vec2::ZERO);
        for _ in 0..120 {
            fx.update(1.0 / 60.0);
        }
        assert!(def_wants > 1e6, "the file is asking for {def_wants}");
        assert_eq!(fx.count(), 32, "but the cap is what it gets");

        // And an opening burst larger than the cap is capped too.
        let (mut fx, id) = with("burst: 500, cap: 8");
        fx.burst(id, Vec2::ZERO);
        assert_eq!(fx.count(), 8);
    }

    /// A one-shot has to end by itself, or the world fills up with dead emitters.
    #[test]
    fn a_one_shot_ends_and_a_standing_one_does_not() {
        let (mut fx, id) = with("burst: 6, life: 0.2, rate: 10.0");
        fx.burst(id, Vec2::ZERO);
        let standing = fx.standing(id, Vec2::new(100.0, 0.0));
        assert_eq!(fx.playing(), 2);
        for _ in 0..60 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.playing(), 1, "the one-shot is gone");
        assert!(fx.particles(standing).is_some(), "the standing one is not");

        // Stopping it lets what is alive live out its life, and then it goes.
        fx.stop(standing);
        assert!(fx.count() > 0, "rain does not vanish the instant it stops");
        for _ in 0..30 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.playing(), 0, "and then the emitter is gone too");
    }

    /// Asking a standing effect for a one-shot should show *something*, not nothing, when the
    /// file never names a burst.
    #[test]
    fn a_one_shot_of_an_effect_with_no_burst_still_shows_something() {
        let (mut fx, id) = with("rate: 20.0, life: 0.5");
        fx.burst(id, Vec2::ZERO);
        assert_eq!(fx.count(), 10, "half a second of twenty a second");
        for _ in 0..60 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.playing(), 0, "and it is a one-shot, so it ends");
    }

    /// Where a particle goes is the file's drift, gravity and drag — checked against the numbers
    /// rather than against itself.
    #[test]
    fn a_particle_goes_where_the_file_sends_it() {
        let (mut fx, id) =
            with("burst: 1, life: 10.0, drift: (x: 60.0, lift: 30.0), gravity: -60.0");
        let handle = fx.burst(id, Vec2::new(10.0, 20.0));
        for _ in 0..60 {
            fx.update(1.0 / 60.0);
        }
        let p = fx.particles(handle).expect("it is alive")[0];
        assert!(
            (p.at.x - 70.0).abs() < 1e-3,
            "a second at 60 px across: {p:?}"
        );
        assert!((p.at.y - 20.0).abs() < 1e-3, "nothing sent it down the map");
        // Thrown up at 30 with gravity taking 60 a second off it: falling at 30 by now, and back
        // on the ground. Stepped rather than solved, so it is within a pixel and not exact.
        assert!((p.rise + 30.0).abs() < 1e-3, "it is falling by now: {p:?}");
        assert!(p.lift.abs() < 1.0, "and it is back where it started: {p:?}");
    }

    /// Rain has to stop at the ground; smoke and dust must not.
    #[test]
    fn what_lands_is_gone_and_what_does_not_keeps_going() {
        let falling = "burst: 1, life: 10.0, lift: 20.0, drift: (lift: -40.0)";
        let (mut fx, id) = with(&format!("{falling}, lands: true"));
        let rain = fx.burst(id, Vec2::ZERO);
        for _ in 0..29 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.count(), 1, "half a second at 40 px/s is only 20 px down");
        for _ in 0..4 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.count(), 0, "and then it has landed");
        assert!(
            fx.particles(rain).is_none(),
            "a one-shot that landed is over"
        );

        // The same fall, with nothing said about landing.
        let (mut fx, id) = with(falling);
        let dust = fx.burst(id, Vec2::ZERO);
        for _ in 0..60 {
            fx.update(1.0 / 60.0);
        }
        let p = fx.particles(dust).expect("still going")[0];
        assert!(p.lift < -15.0, "it sank through the ground: {p:?}");
    }

    /// Drag is a fraction of speed lost a second, so it must not depend on the frame rate.
    #[test]
    fn drag_costs_the_same_whatever_the_frame_rate() {
        let speed_after_a_second = |steps: u32| {
            let (mut fx, id) = with("burst: 1, life: 10.0, drift: (x: 100.0), drag: 0.75");
            let handle = fx.burst(id, Vec2::ZERO);
            for _ in 0..steps {
                fx.update(1.0 / steps as f32);
            }
            fx.particles(handle).expect("alive")[0].speed.x
        };
        let sixty = speed_after_a_second(60);
        assert!((sixty - 25.0).abs() < 0.5, "three quarters lost: {sixty}");
        let ten = speed_after_a_second(10);
        assert!((sixty - ten).abs() < 0.5, "{sixty} at 60 fps, {ten} at 10");
    }

    /// Particles are born in the box the file gives, around where the emitter stands — and
    /// `move_to` moves where the *next* ones are born, not the ones already in the air.
    #[test]
    fn births_follow_the_emitter_and_what_is_airborne_does_not() {
        let (mut fx, id) = with("rate: 600.0, life: 10.0, area: (20.0, 4.0)");
        let handle = fx.standing(id, Vec2::new(1000.0, 500.0));
        fx.update(0.1);
        let first: Vec<Vec2> = fx.particles(handle).unwrap().iter().map(|p| p.at).collect();
        assert!(first.len() > 20, "enough to see a spread: {}", first.len());
        for at in &first {
            assert!(
                (at.x - 1000.0).abs() <= 20.0 + 1e-3,
                "inside the box: {at:?}"
            );
            assert!((at.y - 500.0).abs() <= 4.0 + 1e-3, "inside the box: {at:?}");
        }
        assert!(
            first.iter().any(|at| (at.x - first[0].x).abs() > 1e-3),
            "and spread out, not all in one spot"
        );

        fx.move_to(handle, Vec2::new(-1000.0, 500.0));
        fx.update(0.1);
        let all = fx.particles(handle).unwrap();
        assert!(
            all.iter().any(|p| p.at.x > 900.0) && all.iter().any(|p| p.at.x < -900.0),
            "the old ones stayed and the new ones are born at the new place"
        );
    }

    /// Two emitters of the same effect must not vary in lockstep, and one must not change what
    /// the other does.
    #[test]
    fn each_emitter_varies_on_its_own() {
        let (mut fx, id) = with("burst: 16, life: (1.0, 2.0)");
        let a = fx.burst(id, Vec2::ZERO);
        let b = fx.burst(id, Vec2::ZERO);
        let lives = |fx: &Effects, h| {
            fx.particles(h)
                .unwrap()
                .iter()
                .map(|p| p.life)
                .collect::<Vec<_>>()
        };
        assert_ne!(lives(&fx, a), lives(&fx, b), "two bursts, two seeds");
    }

    /// The quad a particle draws: six vertices at its place, the size the file asks for, with the
    /// frame's corner of the page on it.
    #[test]
    fn a_particle_draws_one_quad_of_its_frame() {
        let (mut fx, id) =
            with("burst: 1, life: 10.0, lift: 7.0, size: (from: 2.0, to: 2.0), layer: 100");
        fx.burst(id, Vec2::new(30.0, 40.0));
        let mut out = Vec::new();
        fx.meshes(&mut out);
        assert_eq!(out.len(), 1, "one mesh for the emitter");
        let mesh = &out[0];
        assert_eq!(mesh.vertices.len(), 6, "two triangles");
        assert_eq!(mesh.layer, 100, "the layer the file named");
        assert_eq!(mesh.sort_y, 40.0, "sorted where the emitter stands");
        assert!(!mesh.body, "smoke casts no silhouette");
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.position.x).collect();
        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.position.y).collect();
        let (lo_x, hi_x) = (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        );
        let (lo_y, hi_y) = (
            ys.iter().copied().fold(f32::MAX, f32::min),
            ys.iter().copied().fold(f32::MIN, f32::max),
        );
        // An 8 px frame at twice the size, centred on its place, seven pixels up the screen.
        assert!((hi_x - lo_x - 16.0).abs() < 1e-3, "{lo_x}..{hi_x}");
        assert!((hi_y - lo_y - 16.0).abs() < 1e-3, "{lo_y}..{hi_y}");
        assert!(((lo_x + hi_x) / 2.0 - 30.0).abs() < 1e-3);
        assert!(((lo_y + hi_y) / 2.0 - 33.0).abs() < 1e-3, "lift raises it");
        for v in &mesh.vertices {
            assert!(
                (0.0..=1.0).contains(&v.uv.x) && (0.0..=1.0).contains(&v.uv.y),
                "uvs are across the page, not in texels: {:?}",
                v.uv
            );
        }
        // An 8 px frame on a 64 px page.
        let u: Vec<f32> = mesh.vertices.iter().map(|v| v.uv.x).collect();
        assert!(u.contains(&0.0) && u.contains(&0.125), "{u:?}");
    }

    /// Spin has to actually turn the quad, or `spin` in a file does nothing.
    #[test]
    fn spin_turns_the_quad() {
        let (mut fx, id) = with("burst: 1, life: 10.0, spin: 0.125");
        fx.burst(id, Vec2::ZERO);
        let mut still = Vec::new();
        fx.meshes(&mut still);
        fx.update(1.0);
        let mut turned = Vec::new();
        fx.meshes(&mut turned);
        // An eighth of a turn a second, so after a second the corners are on the axes.
        let corner = |m: &Vec<Mesh>| m[0].vertices[0].position;
        assert!(
            (corner(&still) - corner(&turned)).length() > 1.0,
            "{:?} did not turn into {:?}",
            corner(&still),
            corner(&turned)
        );
        let after = corner(&turned);
        assert!(
            after.x.abs() < 1e-3 || after.y.abs() < 1e-3,
            "an eighth of a turn puts a corner on an axis: {after:?}"
        );
    }

    /// Colour and size are read from the def at the particle's age, so an effect that fades
    /// actually fades.
    #[test]
    fn a_particle_fades_and_grows_as_it_ages() {
        let (mut fx, id) = with(
            "burst: 1, life: 1.0, size: (from: 1.0, to: 3.0), \
             colour: (from: (1.0, 1.0, 1.0, 1.0), to: (1.0, 0.0, 0.0, 0.0))",
        );
        fx.burst(id, Vec2::ZERO);
        let span = |fx: &Effects| {
            let mut out = Vec::new();
            fx.meshes(&mut out);
            let xs: Vec<f32> = out[0].vertices.iter().map(|v| v.position.x).collect();
            let alpha = out[0].vertices[0].color[3];
            let hi = xs.iter().copied().fold(f32::MIN, f32::max);
            let lo = xs.iter().copied().fold(f32::MAX, f32::min);
            (hi - lo, alpha)
        };
        let (young_size, young_alpha) = span(&fx);
        assert!((young_size - 8.0).abs() < 1e-3, "born at its frame's size");
        assert!((young_alpha - 1.0).abs() < 1e-3, "born solid");
        for _ in 0..59 {
            fx.update(1.0 / 60.0);
        }
        let (old_size, old_alpha) = span(&fx);
        assert!(
            old_size > 22.0,
            "nearly three times the size by now: {old_size}"
        );
        assert!(old_alpha < 0.05, "and nearly gone: {old_alpha}");
    }

    /// A def and its art have to agree, and be told so rather than drawing nothing.
    #[test]
    fn art_that_does_not_match_the_def_is_refused() {
        let mut fx = Effects::new();
        let three: EffectDef =
            ron::from_str(r#"(sheet: "s.sheet.ron", frames: [0, 1, 2], burst: 1)"#)
                .expect("parses");
        let err = fx
            .add(three.clone(), art(2))
            .expect_err("three frames named, two found");
        assert!(err.to_string().contains("s.sheet.ron"), "{err}");
        let mut blank = art(3);
        blank.page = Vec2::ZERO;
        let err = fx
            .add(three.clone(), blank)
            .expect_err("a page with no size");
        assert!(err.to_string().contains("no size"), "{err}");
        fx.add(three, art(3)).expect("three and three");

        // A def built in memory — by the editor — is held to what one read from a file is.
        let err = fx
            .add(def("rate: 0.0"), art(1))
            .expect_err("nothing would ever be born");
        assert!(err.to_string().contains("ever born"), "{err}");
    }

    /// Tuning a number must change the fall being watched, not start it over: an empty screen on
    /// every drag tick is nothing to judge a number by.
    #[test]
    fn retuning_changes_what_is_born_without_disturbing_what_is_flying() {
        let (mut fx, id) = with("burst: 3, life: 100.0, rate: 0.0, drift: (x: 10.0)");
        let handle = fx.standing(id, Vec2::ZERO);
        fx.update(1.0);
        let flying: Vec<Vec2> = fx.particles(handle).unwrap().iter().map(|p| p.at).collect();
        assert_eq!(flying.len(), 3);

        // Twice as fast across, and now giving birth.
        fx.retune(
            id,
            def("burst: 3, life: 100.0, rate: 60.0, drift: (x: 20.0)"),
            art(1),
        )
        .expect("the art still fits");
        fx.update(1.0);
        let all = fx.particles(handle).unwrap();
        assert_eq!(
            all.len(),
            63,
            "sixty born in the second, and the three still up"
        );
        for (old, now) in flying.iter().zip(all) {
            assert!(
                (now.at.x - old.x - 10.0).abs() < 1e-3,
                "one already flying kept the speed it was born with: {old:?} -> {:?}",
                now.at
            );
        }
        assert!(
            all[3..].iter().all(|p| (p.speed.x - 20.0).abs() < 1e-3),
            "and the new ones took the new speed"
        );
    }

    /// A refused retune must leave the effect exactly as it was. Half-swapping it would draw a
    /// definition against the wrong pictures.
    #[test]
    fn a_retune_that_does_not_fit_changes_nothing() {
        let (mut fx, id) = with("burst: 1, life: 10.0");
        fx.standing(id, Vec2::ZERO);
        let before = fx.def(id).clone();
        let three: EffectDef =
            ron::from_str(r#"(sheet: "s.sheet.ron", frames: [0, 1, 2], burst: 1)"#)
                .expect("parses");
        fx.retune(id, three, art(1))
            .expect_err("three frames, one rectangle");
        assert_eq!(fx.def(id), &before, "it is as it was");
        fx.retune(id, def("rate: 0.0"), art(1))
            .expect_err("nothing would ever be born");
        assert_eq!(fx.def(id), &before, "still as it was");
    }

    /// Nothing is drawn or advanced for an effect that is not playing, and a handle that has
    /// finished is ignored rather than panicking — a caller cannot know the moment a one-shot's
    /// last particle died.
    #[test]
    fn nothing_playing_draws_nothing_and_a_stale_handle_is_harmless() {
        let (mut fx, id) = with("burst: 1, life: 0.1");
        let handle = fx.burst(id, Vec2::ZERO);
        for _ in 0..12 {
            fx.update(1.0 / 60.0);
        }
        assert_eq!(fx.playing(), 0);
        fx.move_to(handle, Vec2::ONE);
        fx.stop(handle);
        assert!(fx.particles(handle).is_none());
        let mut out = Vec::new();
        fx.meshes(&mut out);
        assert!(out.is_empty(), "no emitters, no meshes");
        fx.update(0.0);
        fx.update(-1.0);
        assert_eq!(fx.count(), 0);
    }
}
