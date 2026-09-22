use std::time::Duration;

/// Converts variable real time into a whole number of fixed simulation ticks.
#[derive(Clone, Debug)]
pub struct FixedTimestep {
    step: Duration,
    accumulator: Duration,
    max_ticks_per_update: u32,
}

impl FixedTimestep {
    pub fn new(tick_rate: u32) -> Self {
        assert!(tick_rate > 0, "tick rate must be positive");
        Self {
            step: Duration::from_secs(1) / tick_rate,
            accumulator: Duration::ZERO,
            // Past this the sim falls behind real time instead of spiralling.
            max_ticks_per_update: 8,
        }
    }

    pub fn step(&self) -> Duration {
        self.step
    }

    /// Adds real time and returns how many ticks are due.
    pub fn accumulate(&mut self, real_delta: Duration) -> u32 {
        self.accumulator += real_delta;
        let mut ticks = 0;
        while self.accumulator >= self.step && ticks < self.max_ticks_per_update {
            self.accumulator -= self.step;
            ticks += 1;
        }
        if ticks == self.max_ticks_per_update {
            self.accumulator = self.accumulator.min(self.step);
        }
        ticks
    }

    /// Progress towards the next tick in `[0, 1)`.
    pub fn alpha(&self) -> f32 {
        (self.accumulator.as_secs_f64() / self.step.as_secs_f64()).min(1.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_partial_steps() {
        let mut ts = FixedTimestep::new(60);
        assert_eq!(ts.accumulate(Duration::from_millis(10)), 0);
        assert_eq!(ts.accumulate(Duration::from_millis(10)), 1);
        assert!(ts.alpha() > 0.1 && ts.alpha() < 0.3);
    }

    #[test]
    fn long_stall_is_clamped() {
        let mut ts = FixedTimestep::new(60);
        assert_eq!(ts.accumulate(Duration::from_secs(5)), 8);
        assert!(ts.accumulate(Duration::ZERO) <= 1);
    }
}
