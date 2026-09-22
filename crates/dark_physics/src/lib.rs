//! Top-down collision with height, for kinematic characters.
//!
//! Positions are on the ground plane (pixels); `elevation` is feet height above level 0.
//! A body's footprint is a circle. It stands on the highest tile its footprint overlaps and may
//! never overlap a tile higher than its feet (plus a small step-up), so it only walks off a ledge
//! once fully clear of it, and a jump that clears a ledge's rim lands on it.
//!
//! Sim crate: deterministic, no presentation dependencies.

mod terrain;

use glam::Vec2;
use serde::{Deserialize, Serialize};

pub use terrain::{Cell, Terrain};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Circle {
        radius: f32,
    },
    /// Axis-aligned box, by half extents.
    Rect {
        half: Vec2,
    },
}

/// A static obstacle: a prop's footprint that occupies elevations `base..base + height`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collider {
    pub center: Vec2,
    pub shape: Shape,
    pub base: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub position: Vec2,
    /// Feet height in pixels.
    pub elevation: f32,
    /// Vertical speed, pixels per second, up is positive.
    pub vz: f32,
    pub radius: f32,
    pub grounded: bool,
}

impl Body {
    pub fn new(position: Vec2, radius: f32) -> Self {
        assert!(radius > 0.0, "a body needs a positive radius");
        Self {
            position,
            elevation: 0.0,
            vz: 0.0,
            radius,
            grounded: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveParams {
    /// Pixels per second squared.
    pub gravity: f32,
    /// Initial upward speed of a jump. Apex height is `jump_speed² / (2 × gravity)`.
    pub jump_speed: f32,
    /// Ledges up to this high are walked onto without jumping.
    pub step_up: f32,
    /// Walking straight into a corner that the footprint clips by at most this many pixels
    /// slides around it instead of stopping dead.
    pub corner_slip: f32,
}

impl Default for MoveParams {
    fn default() -> Self {
        // Apex ≈ 22 px in theory, ≈ 20.6 px with 60 Hz steps: clears one 16 px level, not two.
        Self {
            gravity: 900.0,
            jump_speed: 200.0,
            step_up: 2.0,
            corner_slip: 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepEvents {
    pub jumped: bool,
    pub landed: bool,
    /// Walked off a ledge without jumping.
    pub fell: bool,
}

/// Collision for one map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub terrain: Terrain,
    pub colliders: Vec<Collider>,
}

impl World {
    pub fn new(terrain: Terrain) -> Self {
        Self {
            terrain,
            colliders: Vec::new(),
        }
    }

    /// Ground under a footprint: the highest tile it overlaps. Infinite if it overlaps a wall.
    pub fn ground_under(&self, center: Vec2, radius: f32) -> f32 {
        self.terrain
            .cells_under_circle(center, radius)
            .fold(f32::NEG_INFINITY, f32::max)
            .max(0.0)
    }

    /// Lowest and highest ground under a footprint (the highest is [`Self::ground_under`]).
    pub fn ground_span(&self, center: Vec2, radius: f32) -> (f32, f32) {
        self.terrain
            .cells_under_circle(center, radius)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), h| {
                (lo.min(h), hi.max(h))
            })
    }

    /// Whether a footprint with feet at `feet` fits at `center`.
    pub fn fits(&self, center: Vec2, radius: f32, feet: f32, params: &MoveParams) -> bool {
        self.ground_under(center, radius) <= feet + params.step_up
    }

    /// Moves `body` for one tick at `velocity` (pixels per second), sliding along obstacles.
    pub fn step(
        &self,
        body: &mut Body,
        velocity: Vec2,
        jump: bool,
        dt: f32,
        params: &MoveParams,
    ) -> StepEvents {
        let mut events = StepEvents::default();
        // Intents may come from the network: never let a bad one poison the body.
        let velocity = if velocity.is_finite() {
            velocity
        } else {
            Vec2::ZERO
        };
        if jump && body.grounded {
            body.vz = params.jump_speed;
            body.grounded = false;
            events.jumped = true;
        }

        // Horizontal: substeps no longer than half the radius, so thin walls are not skipped. The
        // 32-substep cap bounds this at 16 radii per tick: fine for walking, not for projectiles.
        let delta = velocity * dt;
        let max_step = (body.radius * 0.5).max(0.5);
        let steps = ((delta.length() / max_step).ceil() as u32).clamp(1, 32);
        let step = delta / steps as f32;
        for _ in 0..steps {
            self.substep(body, step, params);
        }

        // Vertical.
        let ground = self.support(body.position, body.radius, body.elevation, params);
        if !ground.is_finite() {
            // Overlapping a wall or the map edge: movement never enters one, so this only happens
            // to a body placed badly. Leave its height alone rather than snapping to infinity.
            return events;
        }
        if body.grounded {
            if ground < body.elevation - params.step_up {
                body.grounded = false;
                events.fell = true;
            } else {
                // Stepping up a small lip, or settling onto flat ground.
                body.elevation = ground;
            }
        }
        if !body.grounded {
            body.vz -= params.gravity * dt;
            body.elevation += body.vz * dt;
            if body.elevation <= ground {
                body.elevation = ground;
                body.vz = 0.0;
                body.grounded = true;
                events.landed = true;
            }
        }
        events
    }

    fn substep(&self, body: &mut Body, step: Vec2, params: &MoveParams) {
        let (r, feet) = (body.radius, body.elevation);
        let fits = |p: Vec2| self.fits(p, r, feet, params);
        // Full move; if blocked, go as far as possible along each axis in turn, which both
        // slides along walls and ends flush against them instead of a substep short.
        let mut next = body.position + step;
        if !fits(next) {
            next = body.position;
            for axis in [Vec2::X, Vec2::Y] {
                let along = step * axis;
                let t = furthest(|t| fits(next + along * t));
                next += along * t;
            }
            if next == body.position {
                next = self.slip_around_corner(body.position, step, &fits, params);
            }
        }
        // Push out of props whose height range the feet are inside. Two passes settle corners.
        let moved = next;
        for _ in 0..2 {
            for collider in self.blocking(feet, params) {
                if let Some(push) = penetration(collider, next, r) {
                    next += push;
                }
            }
        }
        if fits(next) {
            body.position = next;
        } else if self.prop_overlap(moved, r, feet, params)
            <= self.prop_overlap(body.position, r, feet, params) + 1e-4
        {
            // The push points into terrain (a prop against a wall). Keep the terrain-legal move
            // if it sinks no deeper into props, so a body wedged between the two can walk out.
            body.position = moved;
        }
    }

    /// What the feet rest on: the terrain under the footprint, or the top of a prop the body is
    /// over and already level with or above (a rock you jumped onto).
    pub fn support(&self, center: Vec2, radius: f32, feet: f32, params: &MoveParams) -> f32 {
        let terrain = self.ground_under(center, radius);
        self.supporting_prop(center, radius, feet, params)
            .map_or(terrain, |c| terrain.max(c.base + c.height))
    }

    /// The highest prop the body stands on, if any. Like terrain, a prop holds the body up while
    /// any of the footprint overlaps it, so walking off only drops once fully clear and the
    /// body is never shoved out of a prop it was just standing on.
    pub fn supporting_prop(
        &self,
        center: Vec2,
        radius: f32,
        feet: f32,
        params: &MoveParams,
    ) -> Option<&Collider> {
        self.colliders
            .iter()
            .filter(|c| c.base + c.height <= feet + params.step_up)
            .filter(|c| penetration(c, center, radius).is_some())
            .max_by(|a, b| (a.base + a.height).total_cmp(&(b.base + b.height)))
    }

    /// A move straight along an axis that is blocked only by a corner the footprint clips by a
    /// few pixels: step sideways towards the opening (at most the move's length per substep).
    fn slip_around_corner(
        &self,
        from: Vec2,
        step: Vec2,
        fits: &impl Fn(Vec2) -> bool,
        params: &MoveParams,
    ) -> Vec2 {
        let (major, minor) = if step.x.abs() >= step.y.abs() {
            (Vec2::new(step.x, 0.0), Vec2::Y)
        } else {
            (Vec2::new(0.0, step.y), Vec2::X)
        };
        // Only nearly straight moves: a diagonal already slides along the free axis.
        if (step - major).length() > major.length() * 0.1 {
            return from;
        }
        let limit = params.corner_slip.max(0.0).floor() as u32;
        for d in 1..=limit {
            for side in [minor, -minor] {
                let opening = side * d as f32;
                if fits(from + opening + major) {
                    let nudge = from + side * (d as f32).min(major.length());
                    if fits(nudge) {
                        return nudge;
                    }
                }
            }
        }
        from
    }

    /// Props whose height range the feet are inside.
    fn blocking(&self, feet: f32, params: &MoveParams) -> impl Iterator<Item = &Collider> {
        self.colliders
            .iter()
            .filter(move |c| feet < c.base + c.height && feet + params.step_up >= c.base)
    }

    /// Total penetration depth into blocking props at `center`.
    fn prop_overlap(&self, center: Vec2, radius: f32, feet: f32, params: &MoveParams) -> f32 {
        self.blocking(feet, params)
            .filter_map(|c| penetration(c, center, radius))
            .map(Vec2::length)
            .sum()
    }
}

/// Largest `t` in `[0, 1]` with `ok(t)`, assuming `ok(0)`; bisection to 1/256 of the step.
fn furthest(ok: impl Fn(f32) -> bool) -> f32 {
    if ok(1.0) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..8 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) { lo = mid } else { hi = mid }
    }
    lo
}

/// How far to move a circle at `center` to leave `collider`, if they overlap.
fn penetration(collider: &Collider, center: Vec2, radius: f32) -> Option<Vec2> {
    match collider.shape {
        Shape::Circle { radius: cr } => {
            let d = center - collider.center;
            let reach = radius + cr;
            let dist = d.length();
            (dist < reach).then(|| {
                // Dead centre: push down (towards the camera), a consistent, deterministic choice.
                let dir = if dist > 1e-4 { d / dist } else { Vec2::Y };
                dir * (reach - dist)
            })
        }
        Shape::Rect { half } => {
            let min = collider.center - half;
            let max = collider.center + half;
            let closest = center.clamp(min, max);
            let d = center - closest;
            let dist = d.length();
            if dist >= radius {
                return None;
            }
            if dist > 1e-4 {
                return Some(d / dist * (radius - dist));
            }
            // Centre inside the box: leave by the nearest side.
            let exits = [
                (center.x - min.x + radius, Vec2::NEG_X),
                (max.x - center.x + radius, Vec2::X),
                (center.y - min.y + radius, Vec2::NEG_Y),
                (max.y - center.y + radius, Vec2::Y),
            ];
            let (amount, dir) = exits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
            Some(dir * amount)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// 10×10 tiles of 16 px; level-1 plateau at columns 5.., level-2 block at columns 8..
    fn world() -> World {
        let mut t = Terrain::new(10, 10, 16.0, 16.0);
        t.fill(5, 0, 5, 10, Cell::Level(1));
        t.fill(8, 0, 2, 10, Cell::Level(2));
        World::new(t)
    }

    fn run(
        world: &World,
        body: &mut Body,
        velocity: Vec2,
        jump_at: Option<u32>,
        ticks: u32,
    ) -> Vec<StepEvents> {
        let params = MoveParams::default();
        (0..ticks)
            .map(|i| world.step(body, velocity, jump_at == Some(i), DT, &params))
            .collect()
    }

    #[test]
    fn walking_into_a_ledge_stops_at_it() {
        let w = world();
        let mut body = Body::new(Vec2::new(40.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(80.0, 0.0), None, 120);
        assert!(
            (body.position.x - 75.0).abs() < 0.05,
            "stopped at x={}",
            body.position.x
        );
        assert_eq!(body.elevation, 0.0);
    }

    #[test]
    fn diagonal_into_a_ledge_slides_along_it() {
        let w = world();
        let mut body = Body::new(Vec2::new(60.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(80.0, 40.0), None, 30);
        assert!(body.position.x <= 75.1);
        assert!(
            body.position.y > 95.0,
            "kept sliding down, y={}",
            body.position.y
        );
    }

    #[test]
    fn jumping_onto_one_level_works_but_not_two() {
        let w = world();
        let mut body = Body::new(Vec2::new(70.0, 80.0), 5.0);
        let events = run(&w, &mut body, Vec2::new(80.0, 0.0), Some(0), 60);
        assert!(events[0].jumped);
        assert!(events.iter().any(|e| e.landed));
        assert_eq!(body.elevation, 16.0);
        assert!(body.position.x > 80.0);
        // Level 1 to level 2 is one level: also a single jump.
        run(&w, &mut body, Vec2::new(80.0, 0.0), Some(0), 90);
        assert_eq!(body.elevation, 32.0);

        // But two levels straight from the floor are out of reach (apex ≈ 22 px < 32 px).
        let mut t = Terrain::new(10, 10, 16.0, 16.0);
        t.fill(5, 0, 5, 10, Cell::Level(2));
        let cliff = World::new(t);
        let mut body = Body::new(Vec2::new(70.0, 80.0), 5.0);
        run(&cliff, &mut body, Vec2::new(80.0, 0.0), Some(0), 90);
        assert!(
            body.position.x <= 75.01,
            "got onto level 2 at x={}",
            body.position.x
        );
        assert_eq!(body.elevation, 0.0);
    }

    #[test]
    fn walking_off_a_ledge_falls_and_lands() {
        let w = world();
        let mut body = Body::new(Vec2::new(100.0, 80.0), 5.0);
        body.elevation = 16.0;
        let events = run(&w, &mut body, Vec2::new(-80.0, 0.0), None, 60);
        assert!(events.iter().any(|e| e.fell));
        assert!(events.iter().any(|e| e.landed));
        assert_eq!(body.elevation, 0.0);
        assert!(body.grounded);
    }

    #[test]
    fn map_edge_blocks() {
        let w = world();
        let mut body = Body::new(Vec2::new(20.0, 20.0), 5.0);
        run(&w, &mut body, Vec2::new(-100.0, -100.0), None, 60);
        assert!(
            (body.position - Vec2::splat(5.0)).length() < 0.05,
            "at {}",
            body.position
        );
    }

    #[test]
    fn tall_prop_blocks_and_body_slides_around_it() {
        let mut w = World::new(Terrain::new(20, 20, 16.0, 16.0));
        w.colliders.push(Collider {
            center: Vec2::new(160.0, 160.0),
            shape: Shape::Circle { radius: 8.0 },
            base: 0.0,
            height: 1000.0,
        });
        // Walking straight at it, slightly off-centre: ends up past it rather than stuck.
        let mut body = Body::new(Vec2::new(100.0, 158.0), 5.0);
        let params = MoveParams::default();
        for tick in 0..120 {
            w.step(&mut body, Vec2::new(60.0, 0.0), false, DT, &params);
            let gap = body.position.distance(Vec2::new(160.0, 160.0));
            assert!(
                gap >= 13.0 - 0.01,
                "tick {tick}: inside the prop, gap {gap}"
            );
        }
        assert!(
            body.position.x > 170.0,
            "slid around, x={}",
            body.position.x
        );
    }

    #[test]
    fn low_prop_can_be_jumped_over() {
        let mut w = World::new(Terrain::new(20, 5, 16.0, 16.0));
        w.colliders.push(Collider {
            center: Vec2::new(100.0, 40.0),
            shape: Shape::Rect {
                half: Vec2::new(4.0, 30.0),
            },
            base: 0.0,
            height: 8.0,
        });
        let mut walker = Body::new(Vec2::new(70.0, 40.0), 5.0);
        run(&w, &mut walker, Vec2::new(80.0, 0.0), None, 60);
        assert!(walker.position.x < 92.0);
        let mut jumper = Body::new(Vec2::new(70.0, 40.0), 5.0);
        run(&w, &mut jumper, Vec2::new(80.0, 0.0), Some(8), 60);
        assert!(
            jumper.position.x > 110.0,
            "jumped over, x={}",
            jumper.position.x
        );
    }

    #[test]
    fn body_wedged_between_prop_and_wall_can_walk_out() {
        let mut t = Terrain::new(20, 20, 16.0, 16.0);
        t.fill(0, 0, 1, 20, Cell::Wall);
        let mut w = World::new(t);
        w.colliders.push(Collider {
            center: Vec2::new(30.0, 80.0),
            shape: Shape::Circle { radius: 8.0 },
            base: 0.0,
            height: 1000.0,
        });
        // Placed overlapping the prop, with the wall behind it: the push-out points into the wall.
        let mut body = Body::new(Vec2::new(22.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(0.0, 60.0), None, 60);
        assert!(body.position.y > 100.0, "walked out, y={}", body.position.y);
    }

    #[test]
    fn wall_under_a_badly_placed_body_never_makes_it_infinitely_high() {
        let mut t = Terrain::new(10, 10, 16.0, 16.0);
        t.fill(0, 0, 1, 10, Cell::Wall);
        let w = World::new(t);
        let mut body = Body::new(Vec2::new(18.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(-60.0, 0.0), Some(0), 60);
        assert!(body.elevation.is_finite());
        assert!(
            body.position.x >= 16.0,
            "did not walk into the wall, x={}",
            body.position.x
        );
    }

    #[test]
    fn non_finite_velocity_is_ignored() {
        let w = world();
        let mut body = Body::new(Vec2::new(40.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(f32::NAN, 0.0), None, 5);
        assert_eq!(body.position, Vec2::new(40.0, 80.0));
    }

    #[test]
    fn can_stand_on_a_low_prop_after_jumping_onto_it_and_walk_off() {
        let mut w = World::new(Terrain::new(20, 10, 16.0, 16.0));
        w.colliders.push(Collider {
            center: Vec2::new(100.0, 80.0),
            shape: Shape::Circle { radius: 12.0 },
            base: 0.0,
            height: 12.0,
        });
        let params = MoveParams::default();
        let mut body = Body::new(Vec2::new(70.0, 80.0), 5.0);
        // Jump and move until over the rock, then stop.
        for i in 0..40 {
            let v = if body.position.x < 100.0 {
                Vec2::new(80.0, 0.0)
            } else {
                Vec2::ZERO
            };
            w.step(&mut body, v, i == 0, DT, &params);
        }
        assert!(body.grounded);
        assert_eq!(body.elevation, 12.0, "standing on the rock");
        // Walk off the far side: falls back to the ground without being shoved. Each tick moves
        // exactly the walked distance (1.33 px), never an extra push out of the rock.
        let params = MoveParams::default();
        let mut events = Vec::new();
        for _ in 0..40 {
            let before = body.position;
            events.push(w.step(&mut body, Vec2::new(80.0, 0.0), false, DT, &params));
            let moved = body.position - before;
            assert!(
                (moved.x - 80.0 * DT).abs() < 1e-3 && moved.y.abs() < 1e-3,
                "shoved by {moved}"
            );
        }
        assert!(events.iter().any(|e| e.fell));
        assert_eq!(body.elevation, 0.0);
        assert!(body.position.x > 118.0);
    }

    #[test]
    fn walking_into_a_corner_clipped_by_a_few_pixels_slides_around_it() {
        let mut t = Terrain::new(10, 10, 16.0, 16.0);
        // A wall block to the north-west; the body clips its corner by 2 px.
        t.fill(0, 0, 3, 3, Cell::Wall);
        let w = World::new(t);
        let mut body = Body::new(Vec2::new(51.0, 80.0), 5.0);
        run(&w, &mut body, Vec2::new(0.0, -60.0), None, 60);
        assert!(
            body.position.y < 40.0,
            "got past the corner, y={}",
            body.position.y
        );
        assert!(body.position.x >= 53.0 - 0.01);
        // Clipped far beyond the slip distance (9 px): stays blocked. The round footprint passes a
        // square corner clipped by a little more than the slip distance.
        let mut deep = Body::new(Vec2::new(44.0, 80.0), 5.0);
        run(&w, &mut deep, Vec2::new(0.0, -60.0), None, 60);
        assert!(deep.position.y > 50.0, "blocked, y={}", deep.position.y);
    }

    #[test]
    fn same_inputs_same_result() {
        let w = world();
        let mut a = Body::new(Vec2::new(33.3, 71.7), 5.0);
        let mut b = a;
        run(&w, &mut a, Vec2::new(77.0, 13.0), Some(3), 200);
        run(&w, &mut b, Vec2::new(77.0, 13.0), Some(3), 200);
        assert_eq!(a, b);
    }
}
