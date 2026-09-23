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

/// How wide the squares of the collider grid are, in pixels (docs/PLAN.md §24.3). A few tiles:
/// small enough that a footprint asks about a handful of props, large enough that a prop rarely
/// sits in many squares at once.
const CELL: f32 = 64.0;

/// Where the colliders are, so a query asks about what is near it instead of about all of them.
/// A collider is listed in every square its footprint touches, so a square holds everything that
/// could reach into it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Grid {
    /// Which colliders are in each square, by their place in the world's own list.
    squares: std::collections::HashMap<(i32, i32), Vec<u32>>,
    /// Set by a test to make every query look at every collider, as the world did before it had
    /// a grid. Never written down: a world is the same world either way.
    #[cfg(test)]
    #[serde(skip)]
    everything: bool,
    /// The longest push any collider here can give: a body inside one leaves by its nearest
    /// side, so it is the *narrowest* half of a box, not the widest, and a circle's radius.
    /// It says how far beyond itself a query must look while a body is being pushed about.
    reach: f32,
}

impl Grid {
    /// The squares a rectangle touches. Nothing of it is asked for unless both corners are real
    /// numbers: an infinite corner would be every square there is, which is a game that never
    /// draws another frame. (Looking at every collider in turn answered such a question with
    /// nonsense rather than nothing, so this is the better answer as well as the quicker one.)
    fn over(min: Vec2, max: Vec2) -> impl Iterator<Item = (i32, i32)> {
        let real = min.is_finite() && max.is_finite();
        let (first, last) = if real {
            ((min / CELL).floor(), (max / CELL).floor())
        } else {
            (Vec2::ZERO, Vec2::NEG_ONE)
        };
        let (rows, cols) = (
            first.y as i32..=last.y as i32,
            first.x as i32..=last.x as i32,
        );
        rows.flat_map(move |row| cols.clone().map(move |col| (col, row)))
    }

    fn put(&mut self, nth: u32, collider: &Collider) {
        let half = Self::half_of(collider);
        self.reach = self.reach.max(half.min_element());
        for square in Self::over(collider.center - half, collider.center + half) {
            self.squares.entry(square).or_default().push(nth);
        }
    }

    /// Takes a collider out of every square it was put in. `reach` is left as it was: it only
    /// says how far a query must look, and looking a little further than needed is right where
    /// forgetting to look far enough is not.
    fn take(&mut self, nth: u32, collider: &Collider) {
        let half = Self::half_of(collider);
        for square in Self::over(collider.center - half, collider.center + half) {
            if let Some(here) = self.squares.get_mut(&square) {
                here.retain(|&held| held != nth);
                if here.is_empty() {
                    self.squares.remove(&square);
                }
            }
        }
    }

    /// How far a collider reaches from its middle, each way.
    fn half_of(collider: &Collider) -> Vec2 {
        match collider.shape {
            Shape::Circle { radius } => Vec2::splat(radius),
            Shape::Rect { half } => half,
        }
    }

    /// Everything that might reach into `min..max`, each named once and in the order they were
    /// added, so a query answers exactly as it did when it looked at all of them in turn.
    fn near(&self, min: Vec2, max: Vec2) -> Vec<u32> {
        let mut near: Vec<u32> = Vec::new();
        for square in Self::over(min, max) {
            if let Some(here) = self.squares.get(&square) {
                near.extend_from_slice(here);
            }
        }
        near.sort_unstable();
        near.dedup();
        near
    }
}

/// Collision for one map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub terrain: Terrain,
    /// Props' footprints, each in a place of its own so that one can be taken away without
    /// renaming the others: the grid, and anything else that has asked, hold these numbers.
    colliders: Vec<Option<Collider>>,
    /// Places whose collider has been taken away, ready for the next one.
    #[serde(default)]
    free: Vec<u32>,
    grid: Grid,
}

impl World {
    pub fn new(terrain: Terrain) -> Self {
        Self {
            terrain,
            colliders: Vec::new(),
            free: Vec::new(),
            grid: Grid::default(),
        }
    }

    /// Adds a prop's footprint and says where it is held, so it can be taken away again. The
    /// grid is kept as they arrive, so no query can find a stale one.
    ///
    /// A place left by a collider that has been taken away is used again before the list grows:
    /// what grows on made land comes and goes with the land it grows on (docs/PLAN.md §24.5), and
    /// a world walked across for an hour would otherwise hold a list of every tree ever seen.
    pub fn add(&mut self, collider: Collider) -> u32 {
        let nth = match self.free.pop() {
            Some(nth) => {
                self.colliders[nth as usize] = Some(collider);
                nth
            }
            None => {
                self.colliders.push(Some(collider));
                self.colliders.len() as u32 - 1
            }
        };
        let held = self.colliders[nth as usize]
            .as_ref()
            .expect("just put there");
        self.grid.put(nth, held);
        nth
    }

    /// Takes a collider away: out of the grid, and its place kept for the next one.
    pub fn remove(&mut self, nth: u32) {
        let Some(gone) = self.colliders.get_mut(nth as usize).and_then(Option::take) else {
            return;
        };
        self.grid.take(nth, &gone);
        self.free.push(nth);
    }

    /// Every collider in the map with where it is held, for drawing them and for counting them.
    pub fn colliders(&self) -> impl Iterator<Item = (u32, &Collider)> {
        self.colliders
            .iter()
            .enumerate()
            .filter_map(|(nth, held)| held.as_ref().map(|c| (nth as u32, c)))
    }

    /// One collider by where it is held, if anything is held there.
    pub fn collider(&self, nth: u32) -> Option<&Collider> {
        self.colliders.get(nth as usize).and_then(Option::as_ref)
    }

    /// How many places the list of colliders has, held or free. Whoever walks them by number
    /// walks this far.
    pub fn collider_places(&self) -> u32 {
        self.colliders.len() as u32
    }

    /// The colliders whose footprint a circle overlaps, in the order they were added.
    pub fn overlapping(&self, center: Vec2, radius: f32) -> impl Iterator<Item = &Collider> {
        self.near(center, radius)
            .into_iter()
            .filter_map(|nth| self.collider(nth))
            .filter(move |c| penetration(c, center, radius).is_some())
    }

    /// The colliders that could touch a circle at `center`.
    fn near(&self, center: Vec2, radius: f32) -> Vec<u32> {
        #[cfg(test)]
        if self.grid.everything {
            return self.colliders().map(|(nth, _)| nth).collect();
        }
        self.grid
            .near(center - Vec2::splat(radius), center + Vec2::splat(radius))
    }

    /// Makes this world look at every collider in turn, as it did before it had a grid. Only for
    /// the test that plays the same moves both ways and compares them.
    #[cfg(test)]
    fn scan_everything(&mut self) {
        self.grid.everything = true;
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

    /// Slides `body` sideways by `delta` pixels, along obstacles as walking does, and touches
    /// nothing else: no gravity, no jump, no ground. Bodies pushed out of one another
    /// (docs/PLAN.md §10) move this way, so a push can never shove anyone through a wall.
    pub fn slide(&self, body: &mut Body, delta: Vec2, params: &MoveParams) {
        if !delta.is_finite() || delta == Vec2::ZERO {
            return;
        }
        let max_step = (body.radius * 0.5).max(0.5);
        let steps = ((delta.length() / max_step).ceil() as u32).clamp(1, 32);
        let step = delta / steps as f32;
        for _ in 0..steps {
            self.substep(body, step, params);
        }
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
        // How much further than its own footprint the query looks. One push is never longer than
        // this, so a body that has moved less than it since the props were asked for cannot have
        // come upon one that was not asked for.
        let slack = r + self.grid.reach;
        for _ in 0..2 {
            let mut from = next;
            let mut props = self.blocking(from, r, slack, feet, params);
            let mut nth = 0;
            while nth < props.len() {
                if let Some(push) = self
                    .collider(props[nth])
                    .and_then(|collider| penetration(collider, next, r))
                {
                    next += push;
                }
                nth += 1;
                // Pushed out of one prop and into the reach of others: ask again, and take in
                // whatever is new. Each prop still pushes at most once in a pass.
                if next.distance(from) > slack {
                    from = next;
                    // Only props the pass has not gone past yet. One it went past cannot have
                    // been missed: the body had moved less than the slack when its turn came, so
                    // it was in that asking, and it was passed over because nothing touched it.
                    // Letting it back in now would push out of a prop a full scan walked past.
                    let done = props[..nth].iter().copied().max().unwrap_or(0);
                    let more = self.blocking(from, r, slack, feet, params);
                    let fresh: Vec<u32> = more
                        .into_iter()
                        .filter(|&m| m > done && !props.contains(&m))
                        .collect();
                    props.extend(fresh);
                    props[nth..].sort_unstable();
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
        self.near(center, radius)
            .into_iter()
            .filter_map(|nth| self.collider(nth))
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

    /// Props near `center` whose height range the feet are inside. `slack` is how much further
    /// than its own footprint the query looks: a body being pushed out of one prop moves while
    /// the pushing goes on, and must still find the next one.
    fn blocking(
        &self,
        center: Vec2,
        radius: f32,
        slack: f32,
        feet: f32,
        params: &MoveParams,
    ) -> Vec<u32> {
        let mut near = self.near(center, radius + slack);
        near.retain(|&nth| {
            self.collider(nth)
                .is_some_and(|c| feet < c.base + c.height && feet + params.step_up >= c.base)
        });
        near
    }

    /// Total penetration depth into blocking props at `center`.
    fn prop_overlap(&self, center: Vec2, radius: f32, feet: f32, params: &MoveParams) -> f32 {
        self.blocking(center, radius, 0.0, feet, params)
            .into_iter()
            .filter_map(|nth| penetration(self.collider(nth)?, center, radius))
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
        w.add(Collider {
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

    /// A world of props scattered over `across` pixels, the same every time.
    fn scattered(across: f32, many: u32) -> World {
        let mut w = World::new(Terrain::new(
            (across / 16.0) as u32,
            (across / 16.0) as u32,
            16.0,
            16.0,
        ));
        let mut seed = 0x9E3779B97F4A7C15u64;
        let mut roll = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (u32::MAX / 2) as f32).fract()
        };
        for n in 0..many {
            // A quarter of them off the map: west and north of it, and past its far corner.
            let at = match n % 4 {
                0 => Vec2::new(
                    roll() * across - across / 8.0,
                    roll() * across - across / 8.0,
                ),
                _ => Vec2::new(roll() * across, roll() * across),
            };
            // One in nine is wider than a square of the grid, as the game's own great props are.
            let size = if n % 9 == 0 {
                CELL + roll() * 128.0
            } else {
                4.0 + roll() * 28.0
            };
            let shape = if n % 2 == 0 {
                Shape::Circle { radius: size }
            } else {
                Shape::Rect {
                    half: Vec2::new(size, 4.0 + roll() * 20.0),
                }
            };
            w.add(Collider {
                center: at,
                shape,
                base: (roll() * 3.0).floor() * 8.0,
                height: 4.0 + roll() * 40.0,
            });
        }
        w
    }

    /// A collider taken away is gone from every query, its place is used again, and the props
    /// that stayed answer exactly as they did before — by the same numbers.
    ///
    /// What grows on made land comes and goes with the land it grows on (§24.5), so this happens
    /// while people are standing on it. A stale number left in the grid would be a tree nobody
    /// can see and nobody can walk through.
    #[test]
    fn a_prop_taken_away_is_gone_from_every_query_and_its_place_is_used_again() {
        let params = MoveParams::default();
        let mut w = scattered(512.0, 60);
        let before: Vec<(u32, Collider)> = w.colliders().map(|(nth, c)| (nth, *c)).collect();
        let asked: Vec<Vec2> = (0..40)
            .map(|n| Vec2::new((n % 8) as f32 * 64.0, (n / 8) as f32 * 64.0))
            .collect();
        let answers = |w: &World| -> Vec<Vec<u32>> {
            asked
                .iter()
                .map(|at| {
                    w.blocking(*at, 8.0, 0.0, 0.0, &params)
                        .into_iter()
                        .collect()
                })
                .collect()
        };
        let was = answers(&w);

        // Half of them go.
        let gone: Vec<u32> = before
            .iter()
            .filter(|(nth, _)| nth % 2 == 0)
            .map(|(nth, _)| *nth)
            .collect();
        for nth in &gone {
            w.remove(*nth);
        }
        assert_eq!(w.colliders().count(), before.len() - gone.len());
        for nth in &gone {
            assert!(w.collider(*nth).is_none(), "{nth} is still held");
        }
        for (at, was) in asked.iter().zip(&was) {
            let now = w.blocking(*at, 8.0, 0.0, 0.0, &params);
            let kept: Vec<u32> = was.iter().copied().filter(|n| n % 2 == 1).collect();
            assert_eq!(now, kept, "what is left at {at} is what was not taken away");
        }

        // Their places are used again rather than the list growing, and the new props answer.
        let places = w.collider_places();
        let mut back = Vec::new();
        for n in 0..gone.len() {
            back.push(w.add(Collider {
                center: Vec2::new(16.0 + n as f32 * 3.0, 16.0),
                shape: Shape::Circle { radius: 6.0 },
                base: 0.0,
                height: 32.0,
            }));
        }
        // Every one of them landed in a place something had left.
        assert!(back.iter().all(|nth| gone.contains(nth)), "{back:?}");
        assert_eq!(w.collider_places(), places, "the list did not grow");
        assert_eq!(w.colliders().count(), before.len());
        for nth in back {
            assert!(w.collider(nth).is_some());
        }
    }

    /// The grid must answer exactly as looking at every collider in turn did — the same prop
    /// held up to, the same props overlapped, in the same order. Anything else is a body that
    /// stands somewhere different than it used to (docs/PLAN.md §24.3).
    #[test]
    fn the_grid_answers_as_a_full_scan_does() {
        let params = MoveParams::default();
        let w = scattered(2048.0, 400);
        let mut asked = 0;
        let mut found = 0;
        for row in 0..64 {
            for col in 0..64 {
                let at = Vec2::new(col as f32 * 32.0, row as f32 * 32.0);
                for (radius, feet) in [(5.0, 0.0), (9.0, 8.0), (16.0, 24.0)] {
                    // Every collider in turn, as the world used to do it.
                    let scan: Vec<&Collider> = w
                        .colliders()
                        .map(|(_, c)| c)
                        .filter(|c| penetration(c, at, radius).is_some())
                        .collect();
                    let grid: Vec<&Collider> = w.overlapping(at, radius).collect();
                    assert_eq!(scan, grid, "overlapping at {at} with radius {radius}");

                    let held_by = w
                        .colliders()
                        .map(|(_, c)| c)
                        .filter(|c| c.base + c.height <= feet + params.step_up)
                        .filter(|c| penetration(c, at, radius).is_some())
                        .max_by(|a, b| (a.base + a.height).total_cmp(&(b.base + b.height)));
                    assert_eq!(
                        held_by,
                        w.supporting_prop(at, radius, feet, &params),
                        "what holds up feet at {feet} at {at}"
                    );
                    asked += 1;
                    found += grid.len();
                }
            }
        }
        assert!(found > 1000, "only {found} overlaps in {asked} questions");
    }

    /// Walking about with the grid must put a body in exactly the place that looking at every
    /// collider in turn put it — the whole move, not just the questions it asks on the way:
    /// blocked steps, sliding along a wall, corner slips, and being pushed out of props, which
    /// moves the body while the props are being asked about (docs/PLAN.md §24.3).
    #[test]
    fn walking_about_lands_where_a_full_scan_lands() {
        let params = MoveParams::default();
        let (with_grid, mut scanning) = (scattered(1024.0, 300), scattered(1024.0, 300));
        scanning.scan_everything();
        let mut seed = 0x243F_6A88_85A3_08D3u64;
        let mut roll = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (u32::MAX / 2) as f32).fract()
        };
        let mut walked = 0.0f32;
        for walk in 0..40 {
            let start = Vec2::new(roll() * 1024.0, roll() * 1024.0);
            let mut here = Body::new(start, 4.0 + roll() * 6.0);
            here.elevation = (roll() * 3.0).floor() * 8.0;
            let mut there = here;
            for step in 0..60 {
                let push = Vec2::new(roll() * 2.0 - 1.0, roll() * 2.0 - 1.0) * 90.0;
                let jump = step % 17 == 0;
                with_grid.step(&mut here, push, jump, 1.0 / 60.0, &params);
                scanning.step(&mut there, push, jump, 1.0 / 60.0, &params);
                assert_eq!(
                    (here.position, here.elevation),
                    (there.position, there.elevation),
                    "walk {walk} step {step}: the grid took the body somewhere else"
                );
                walked += here.position.distance(start);
            }
        }
        assert!(walked > 10_000.0, "the walks went nowhere ({walked} px)");
    }

    /// How far beyond its own footprint one asking covers.
    fn slack_of(world: &World, radius: f32) -> f32 {
        radius + world.grid.reach
    }

    /// A body in a row of props that overlap one another is pushed along the row, moving while
    /// the props are being asked about — so the ones further along were not near it when the
    /// asking began. The grid must take them in as the body reaches them, or the body is left
    /// sitting inside a prop the full scan would have pushed it out of.
    #[test]
    fn being_pushed_along_a_row_of_props_finds_the_far_ones() {
        let params = MoveParams::default();
        let row = |world: &mut World| {
            for n in 0..120 {
                world.add(Collider {
                    center: Vec2::new(0.0, n as f32 * 4.0),
                    shape: Shape::Circle { radius: 14.0 },
                    base: 0.0,
                    height: 40.0,
                });
            }
        };
        let mut with_grid = World::new(Terrain::new(64, 64, 16.0, 16.0));
        let mut scanning = World::new(Terrain::new(64, 64, 16.0, 16.0));
        row(&mut with_grid);
        row(&mut scanning);
        scanning.scan_everything();
        let mut here = Body::new(Vec2::new(0.5, 240.0), 6.0);
        let mut there = here;
        for _ in 0..3 {
            with_grid.step(&mut here, Vec2::new(4.0, 2.0), false, 1.0 / 60.0, &params);
            scanning.step(&mut there, Vec2::new(4.0, 2.0), false, 1.0 / 60.0, &params);
        }
        assert_eq!(here.position, there.position, "pushed along the row");
        // The pushing really did carry the body a long way down the row — much further than the
        // slack one asking covers — or this proves nothing.
        assert!(
            here.position.y - 240.0 > slack_of(&with_grid, 6.0) * 2.0,
            "the body was only pushed to {}",
            here.position.y
        );
    }

    /// Bodies shoved about inside tight knots of props, with the grid and with a full scan,
    /// landing in the same place every time.
    ///
    /// This is the guard for the push-out loop, where the body moves while the props are being
    /// asked about: the props asked for change under it, and the order they are dealt with in
    /// decides where it ends up. Built cases cannot find the trouble there — a knot has to be
    /// dense enough, and a body unlucky enough, for a prop to arrive late — so the knots and the
    /// shoves are rolled instead, and there are a great many of them.
    #[test]
    fn knots_of_props_push_a_body_the_same_way_a_full_scan_does() {
        let params = MoveParams::default();
        let mut seed = 0xD1B5_4A32_D192_ED03u64;
        let mut roll = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (u32::MAX / 2) as f32).fract()
        };
        let mut pushed = 0.0f32;
        for knot in 0..8_000 {
            // A knot of props overlapping one another, in a space a few of them wide.
            let (mut with_grid, mut scanning) = (
                World::new(Terrain::new(16, 16, 16.0, 16.0)),
                World::new(Terrain::new(16, 16, 16.0, 16.0)),
            );
            let middle = Vec2::splat(128.0);
            let many = 10 + (roll() * 30.0) as u32;
            for n in 0..many {
                let at = middle + Vec2::new(roll() * 60.0 - 30.0, roll() * 60.0 - 30.0);
                let size = 4.0 + roll() * 7.0;
                let shape = if n % 3 == 0 {
                    Shape::Rect {
                        half: Vec2::new(size, 4.0 + roll() * 7.0),
                    }
                } else {
                    Shape::Circle { radius: size }
                };
                let prop = Collider {
                    center: at,
                    shape,
                    base: 0.0,
                    height: 20.0 + roll() * 30.0,
                };
                with_grid.add(prop);
                scanning.add(prop);
            }
            scanning.scan_everything();
            // A body dropped into the knot and shoved about in it.
            let start = middle + Vec2::new(roll() * 40.0 - 20.0, roll() * 40.0 - 20.0);
            let mut here = Body::new(start, 5.0 + roll() * 6.0);
            let mut there = here;
            for step in 0..12 {
                let shove = Vec2::new(roll() * 2.0 - 1.0, roll() * 2.0 - 1.0) * 70.0;
                with_grid.step(&mut here, shove, false, 1.0 / 60.0, &params);
                scanning.step(&mut there, shove, false, 1.0 / 60.0, &params);
                assert_eq!(
                    (here.position, here.elevation),
                    (there.position, there.elevation),
                    "knot {knot} of {many} props, shove {step}: the grid put the body elsewhere"
                );
            }
            pushed += here.position.distance(start);
        }
        // The knots really did shove the bodies about, or this proves nothing.
        assert!(
            pushed > 100_000.0,
            "the bodies barely moved ({pushed} px in all)"
        );
    }

    /// And it must ask about a handful of props, however many the map holds: this is the whole
    /// point of it, and a body's tick asks tens of times.
    #[test]
    fn a_question_is_about_what_is_near_it() {
        let at = Vec2::splat(1000.0);
        // The same props, spread over a map four times as wide in each direction.
        let close = scattered(2048.0, 400);
        let far = scattered(8192.0, 400);
        let (close_asked, far_asked) = (close.near(at, 16.0).len(), far.near(at, 16.0).len());
        assert!(
            close_asked <= 24 && far_asked <= 24,
            "asked about {close_asked} and {far_asked} props"
        );
        // A map with a hundred times the props, asked in the same place, asks about what is in
        // that place — not about the hundred times.
        let many = scattered(8192.0, 40_000);
        let asked = many.near(at, 16.0).len();
        assert!(
            asked < many.colliders().count() / 100,
            "asked about {asked} of {}",
            many.colliders().count()
        );
    }

    #[test]
    fn low_prop_can_be_jumped_over() {
        let mut w = World::new(Terrain::new(20, 5, 16.0, 16.0));
        w.add(Collider {
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
        w.add(Collider {
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
        w.add(Collider {
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
