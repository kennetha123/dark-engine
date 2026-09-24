//! World calendar and sleep consensus.
//!
//! Time is counted in simulation ticks so the clock is deterministic and replicates exactly.
//! The clock only advances when the host ticks it; the host decides when to pause
//! (single-player only, see `docs/PLAN.md` §4.1).

use serde::{Deserialize, Serialize};

pub const DAYS_PER_YEAR: u32 = 365;
pub const HOURS_PER_DAY: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockConfig {
    /// Simulation ticks per real second.
    pub tick_rate: u32,
    /// Real seconds per in-game day.
    pub day_length_secs: u32,
    /// In-game hour sleepers wake at.
    pub wake_hour: u32,
    /// The hour the first day starts at, if somebody said. `None` starts at [`Self::wake_hour`],
    /// as it always did; `Some(0)` is midnight, which is exactly when a designer wants to watch
    /// somebody sleep (docs/PLAN.md §21.1).
    pub start_hour: Option<u32>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            tick_rate: 60,
            day_length_secs: 24 * 60,
            wake_hour: 6,
            start_hour: None,
        }
    }
}

impl ClockConfig {
    pub fn ticks_per_day(&self) -> u64 {
        u64::from(self.tick_rate) * u64::from(self.day_length_secs)
    }

    fn wake_tick(&self) -> u64 {
        self.ticks_per_day() * u64::from(self.wake_hour) / u64::from(HOURS_PER_DAY)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockEvent {
    /// A new day started. Carries the new zero-based day index.
    NewDay(u32),
    /// The last day of the year has ended.
    YearEnded,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameClock {
    config: ClockConfig,
    /// Zero-based day index; `DAYS_PER_YEAR` means the year is over.
    day: u32,
    tick_in_day: u64,
}

impl GameClock {
    /// A clock at the first morning of the year.
    pub fn new(config: ClockConfig) -> Self {
        assert!(
            config.ticks_per_day() > 0,
            "tick rate and day length must be positive"
        );
        assert!(config.wake_hour < HOURS_PER_DAY, "wake hour must be 0..24");
        Self {
            config,
            day: 0,
            tick_in_day: config.wake_tick(),
        }
    }

    pub fn config(&self) -> &ClockConfig {
        &self.config
    }

    pub fn day(&self) -> u32 {
        self.day
    }

    pub fn days_remaining(&self) -> u32 {
        DAYS_PER_YEAR.saturating_sub(self.day)
    }

    pub fn is_year_over(&self) -> bool {
        self.day >= DAYS_PER_YEAR
    }

    /// Fraction of the day elapsed, in `[0, 1)`.
    pub fn day_fraction(&self) -> f64 {
        self.tick_in_day as f64 / self.config.ticks_per_day() as f64
    }

    /// In-game hour as a fractional value in `[0, 24)`.
    pub fn hour(&self) -> f64 {
        self.day_fraction() * f64::from(HOURS_PER_DAY)
    }

    /// Sets the time to the start of `hour` on `day`: resuming a saved world.
    pub fn set_time(&mut self, day: u32, hour: u32) {
        assert!(hour < HOURS_PER_DAY, "hour must be 0..24");
        self.day = day.min(DAYS_PER_YEAR);
        self.tick_in_day = self.config.ticks_per_day() * u64::from(hour) / u64::from(HOURS_PER_DAY);
    }

    /// Advances one simulation tick.
    pub fn tick(&mut self) -> Option<ClockEvent> {
        if self.is_year_over() {
            return None;
        }
        self.tick_in_day += 1;
        if self.tick_in_day >= self.config.ticks_per_day() {
            self.tick_in_day = 0;
            return Some(self.next_day());
        }
        None
    }

    /// Jumps to the next wake-up time. Sleeping before dawn wakes the same day;
    /// sleeping after dawn wakes the next day.
    pub fn skip_to_morning(&mut self) -> Option<ClockEvent> {
        if self.is_year_over() {
            return None;
        }
        let wake = self.config.wake_tick();
        if self.tick_in_day < wake {
            self.tick_in_day = wake;
            return None;
        }
        self.tick_in_day = wake;
        Some(self.next_day())
    }

    fn next_day(&mut self) -> ClockEvent {
        self.day += 1;
        if self.is_year_over() {
            ClockEvent::YearEnded
        } else {
            ClockEvent::NewDay(self.day)
        }
    }
}

/// A player's state as far as sleep consensus is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    Awake,
    Asleep,
    /// Connection lost, inside the reconnect grace window. Counts as awake.
    InGrace,
    /// Not connected; the character sleeps at an inn or in a tent.
    Offline,
}

/// True when every online player is asleep and at least one is.
/// Offline characters never block the skip; players in grace always do.
pub fn everyone_asleep(players: impl IntoIterator<Item = Presence>) -> bool {
    let mut any_asleep = false;
    for presence in players {
        match presence {
            Presence::Asleep => any_asleep = true,
            Presence::Offline => {}
            Presence::Awake | Presence::InGrace => return false,
        }
    }
    any_asleep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config() -> ClockConfig {
        ClockConfig {
            tick_rate: 1,
            day_length_secs: 24,
            wake_hour: 6,
            start_hour: None,
        }
    }

    #[test]
    fn starts_at_first_morning() {
        let clock = GameClock::new(small_config());
        assert_eq!(clock.day(), 0);
        assert_eq!(clock.hour(), 6.0);
        assert_eq!(clock.days_remaining(), DAYS_PER_YEAR);
    }

    #[test]
    fn default_day_is_24_real_minutes_at_60hz() {
        assert_eq!(ClockConfig::default().ticks_per_day(), 60 * 24 * 60);
    }

    #[test]
    fn ticking_through_midnight_starts_new_day() {
        let mut clock = GameClock::new(small_config());
        let events: Vec<_> = (0..18).filter_map(|_| clock.tick()).collect();
        assert_eq!(events, vec![ClockEvent::NewDay(1)]);
        assert_eq!(clock.hour(), 0.0);
    }

    #[test]
    fn sleeping_after_dawn_wakes_next_day() {
        let mut clock = GameClock::new(small_config());
        for _ in 0..16 {
            clock.tick();
        }
        assert_eq!(clock.skip_to_morning(), Some(ClockEvent::NewDay(1)));
        assert_eq!(clock.hour(), 6.0);
    }

    #[test]
    fn sleeping_before_dawn_wakes_same_day() {
        let mut clock = GameClock::new(small_config());
        for _ in 0..20 {
            clock.tick();
        }
        assert_eq!(clock.day(), 1);
        assert_eq!(clock.skip_to_morning(), None);
        assert_eq!(clock.day(), 1);
        assert_eq!(clock.hour(), 6.0);
    }

    #[test]
    fn year_ends_after_last_day_and_clock_stops() {
        let mut clock = GameClock::new(small_config());
        for _ in 0..DAYS_PER_YEAR - 1 {
            clock.skip_to_morning();
        }
        assert_eq!(clock.days_remaining(), 1);
        assert_eq!(clock.skip_to_morning(), Some(ClockEvent::YearEnded));
        assert!(clock.is_year_over());
        assert_eq!(clock.tick(), None);
        assert_eq!(clock.skip_to_morning(), None);
    }

    #[test]
    fn a_resumed_clock_carries_on_from_the_saved_time() {
        let mut clock = GameClock::new(small_config());
        clock.set_time(40, 21);
        assert_eq!((clock.day(), clock.hour()), (40, 21.0));
        assert_eq!(clock.skip_to_morning(), Some(ClockEvent::NewDay(41)));
        clock.set_time(DAYS_PER_YEAR + 5, 0);
        assert!(clock.is_year_over());
    }

    #[test]
    fn consensus_rules() {
        use Presence::*;
        assert!(everyone_asleep([Asleep, Asleep]));
        assert!(everyone_asleep([Asleep, Offline, Offline]));
        assert!(!everyone_asleep([Asleep, Awake]));
        assert!(!everyone_asleep([Asleep, InGrace]));
        assert!(!everyone_asleep([Offline]));
        assert!(!everyone_asleep([]));
    }
}
