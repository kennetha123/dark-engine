//! Network condition simulation for development: latency, jitter and loss on a client's
//! gameplay messages, both directions. Loss only hits the unreliable [`Channel::State`];
//! reliable [`Channel::Events`] are delayed but keep their order, as renet would deliver them.

use std::str::FromStr;
use std::time::Duration;

use renet::Bytes;

use crate::Channel;

/// One-way conditions; a round trip sees about twice the latency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetConditions {
    pub latency: Duration,
    /// Extra random delay per message, up to this much.
    pub jitter: Duration,
    /// Fraction of state messages dropped, 0..=1.
    pub loss: f32,
}

impl FromStr for NetConditions {
    type Err = String;

    /// `latency_ms,jitter_ms,loss_percent`, e.g. `80,20,5`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        let [latency, jitter, loss] = parts.as_slice() else {
            return Err(format!(
                "expected latency_ms,jitter_ms,loss_percent, got {s:?}"
            ));
        };
        let ms = |v: &str| {
            v.parse::<u64>()
                .map(Duration::from_millis)
                .map_err(|e| format!("{v:?}: {e}"))
        };
        let loss: f32 = loss.parse().map_err(|e| format!("{loss:?}: {e}"))?;
        if !(0.0..=100.0).contains(&loss) {
            return Err(format!("loss {loss}% must be 0..=100"));
        }
        Ok(Self {
            latency: ms(latency)?,
            jitter: ms(jitter)?,
            loss: loss / 100.0,
        })
    }
}

struct Delayed {
    at: Duration,
    channel: Channel,
    bytes: Bytes,
}

pub(crate) struct Conditioner {
    conditions: NetConditions,
    clock: Duration,
    rng: u64,
    outbox: Vec<Delayed>,
    inbox: Vec<Delayed>,
    /// Latest release time of a reliable message per direction, to keep them in order.
    last_out: Duration,
    last_in: Duration,
}

impl Conditioner {
    pub(crate) fn new(conditions: NetConditions) -> Self {
        Self {
            conditions,
            clock: Duration::ZERO,
            rng: 0x005E_ED0F_DA2C,
            outbox: Vec::new(),
            inbox: Vec::new(),
            last_out: Duration::ZERO,
            last_in: Duration::ZERO,
        }
    }

    pub(crate) fn advance(&mut self, dt: Duration) {
        self.clock += dt;
    }

    pub(crate) fn outgoing(&mut self, channel: Channel, bytes: Bytes) {
        if let Some(at) = self.schedule(channel, true) {
            self.outbox.push(Delayed { at, channel, bytes });
        }
    }

    pub(crate) fn incoming(&mut self, channel: Channel, bytes: Bytes) {
        if let Some(at) = self.schedule(channel, false) {
            self.inbox.push(Delayed { at, channel, bytes });
        }
    }

    /// Outgoing messages whose delay has passed, in send order.
    pub(crate) fn due_outgoing(&mut self) -> Vec<(Channel, Bytes)> {
        let clock = self.clock;
        let (due, waiting) = std::mem::take(&mut self.outbox)
            .into_iter()
            .partition(|d| d.at <= clock);
        self.outbox = waiting;
        let mut due: Vec<Delayed> = due;
        due.sort_by_key(|d| d.at);
        due.into_iter().map(|d| (d.channel, d.bytes)).collect()
    }

    /// Every outgoing message still held, in send order, delay or not.
    pub(crate) fn drain_outgoing(&mut self) -> Vec<(Channel, Bytes)> {
        let mut all = std::mem::take(&mut self.outbox);
        all.sort_by_key(|d| d.at);
        all.into_iter().map(|d| (d.channel, d.bytes)).collect()
    }

    /// The earliest incoming message on `channel` whose delay has passed.
    pub(crate) fn take_incoming(&mut self, channel: Channel) -> Option<Bytes> {
        let clock = self.clock;
        let index = self
            .inbox
            .iter()
            .enumerate()
            .filter(|(_, d)| d.channel == channel && d.at <= clock)
            .min_by_key(|(_, d)| d.at)
            .map(|(i, _)| i)?;
        Some(self.inbox.remove(index).bytes)
    }

    /// When a message sent now arrives, or `None` if it is lost.
    fn schedule(&mut self, channel: Channel, outgoing: bool) -> Option<Duration> {
        if channel == Channel::State && self.random() < self.conditions.loss {
            return None;
        }
        let jitter = self.conditions.jitter.mul_f32(self.random());
        let mut at = self.clock + self.conditions.latency + jitter;
        if channel != Channel::State {
            let last = if outgoing {
                &mut self.last_out
            } else {
                &mut self.last_in
            };
            at = at.max(*last);
            *last = at;
        }
        Some(at)
    }

    /// Uniform in [0, 1), deterministic (SplitMix64) so a session replays the same way.
    fn random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conditions(latency_ms: u64, jitter_ms: u64, loss: f32) -> NetConditions {
        NetConditions {
            latency: Duration::from_millis(latency_ms),
            jitter: Duration::from_millis(jitter_ms),
            loss,
        }
    }

    #[test]
    fn parses_the_command_line_form() {
        assert_eq!(
            "80,20,5".parse::<NetConditions>(),
            Ok(conditions(80, 20, 0.05))
        );
        assert!("80,20".parse::<NetConditions>().is_err());
        assert!("80,20,150".parse::<NetConditions>().is_err());
    }

    #[test]
    fn messages_wait_for_the_latency() {
        let mut c = Conditioner::new(conditions(100, 0, 0.0));
        c.incoming(Channel::State, Bytes::from("a"));
        c.advance(Duration::from_millis(99));
        assert_eq!(c.take_incoming(Channel::State), None);
        c.advance(Duration::from_millis(1));
        assert_eq!(c.take_incoming(Channel::State), Some(Bytes::from("a")));
    }

    #[test]
    fn loss_only_drops_state_messages() {
        let mut c = Conditioner::new(conditions(0, 0, 0.5));
        for _ in 0..1000 {
            c.outgoing(Channel::State, Bytes::from("s"));
            c.outgoing(Channel::Events, Bytes::from("e"));
        }
        let due = c.due_outgoing();
        let state = due.iter().filter(|(ch, _)| *ch == Channel::State).count();
        let events = due.iter().filter(|(ch, _)| *ch == Channel::Events).count();
        assert_eq!(events, 1000);
        assert!((400..600).contains(&state), "about half lost, kept {state}");
    }

    #[test]
    fn jitter_never_reorders_reliable_events() {
        let mut c = Conditioner::new(conditions(10, 50, 0.0));
        for i in 0..100u8 {
            c.incoming(Channel::Events, Bytes::from(vec![i]));
            c.advance(Duration::from_millis(1));
        }
        c.advance(Duration::from_secs(1));
        let got: Vec<u8> = std::iter::from_fn(|| c.take_incoming(Channel::Events))
            .map(|b| b[0])
            .collect();
        assert_eq!(got, (0..100).collect::<Vec<u8>>());
    }
}
