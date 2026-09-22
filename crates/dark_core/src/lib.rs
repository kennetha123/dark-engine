//! App shell: ECS world, schedules and the fixed-timestep loop.
//!
//! Gameplay runs in [`FixedUpdate`] at a fixed rate and never reads frame delta.
//! [`FrameUpdate`] runs once per rendered frame for presentation and reads [`FrameTime`].

mod timestep;

use std::time::Duration;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_ecs::system::ScheduleSystem;

pub use timestep::FixedTimestep;

pub const DEFAULT_TICK_RATE: u32 = 60;

/// Runs zero or more times per frame, once per simulation tick.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixedUpdate;

/// Runs once per frame after all simulation ticks for that frame.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameUpdate;

/// Number of simulation ticks run since start.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimTick(pub u64);

/// Length of one simulation tick.
#[derive(Resource, Clone, Copy, Debug)]
pub struct FixedDelta(pub Duration);

#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct FrameTime {
    pub delta: Duration,
    /// Progress towards the next tick in `[0, 1)`, for render interpolation.
    pub alpha: f32,
}

/// Consumed on install, so a plugin can hand owned resources (sockets, handles) to the world.
pub trait Plugin {
    fn build(self, app: &mut App);
}

pub struct App {
    pub world: World,
    timestep: FixedTimestep,
}

impl App {
    pub fn new(tick_rate: u32) -> Self {
        let timestep = FixedTimestep::new(tick_rate);
        let mut world = World::new();
        world.add_schedule(Schedule::new(FixedUpdate));
        world.add_schedule(Schedule::new(FrameUpdate));
        world.insert_resource(SimTick::default());
        world.insert_resource(FixedDelta(timestep.step()));
        world.insert_resource(FrameTime::default());
        Self { world, timestep }
    }

    pub fn add_plugin(&mut self, plugin: impl Plugin) -> &mut Self {
        plugin.build(self);
        self
    }

    pub fn add_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.world
            .resource_mut::<Schedules>()
            .add_systems(schedule, systems);
        self
    }

    /// Orders system sets within a schedule, e.g. `(A, B, C).chain()`.
    pub fn configure_sets<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        sets: impl IntoScheduleConfigs<bevy_ecs::intern::Interned<dyn bevy_ecs::schedule::SystemSet>, M>,
    ) -> &mut Self {
        self.world
            .resource_mut::<Schedules>()
            .configure_sets(schedule, sets);
        self
    }

    pub fn insert_resource(&mut self, resource: impl Resource) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    /// Advances real time by `real_delta`: runs the due simulation ticks, then one frame.
    pub fn update(&mut self, real_delta: Duration) {
        let ticks = self.timestep.accumulate(real_delta);
        for _ in 0..ticks {
            self.tick();
        }
        *self.world.resource_mut::<FrameTime>() = FrameTime {
            delta: real_delta,
            alpha: self.timestep.alpha(),
        };
        self.world.run_schedule(FrameUpdate);
    }

    /// Runs exactly one simulation tick. Headless tools (fast-forward) call this directly.
    pub fn tick(&mut self) {
        self.world.run_schedule(FixedUpdate);
        self.world.resource_mut::<SimTick>().0 += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Counter {
        fixed: u32,
        frames: u32,
    }

    #[test]
    fn fixed_and_frame_schedules_run_at_their_rates() {
        let mut app = App::new(60);
        app.insert_resource(Counter::default())
            .add_systems(FixedUpdate, |mut c: ResMut<Counter>| c.fixed += 1)
            .add_systems(FrameUpdate, |mut c: ResMut<Counter>| c.frames += 1);

        for _ in 0..30 {
            app.update(Duration::from_secs(1) / 30);
        }
        let counter = app.world.resource::<Counter>();
        assert_eq!(counter.frames, 30);
        assert_eq!(counter.fixed, 60);
        assert_eq!(app.world.resource::<SimTick>().0, 60);
    }
}
