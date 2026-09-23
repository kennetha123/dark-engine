//! Player session lifecycle: `InGame → Grace → Offline`, reconnect inside grace resumes.
//!
//! Transport-agnostic: the host feeds it hello/goodbye messages and transport disconnects.
//! A disconnect without a prior goodbye is treated as a crash and starts the grace window.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{ClientId, PlayerId};

pub const MAX_PLAYERS: usize = 4;
pub const RECONNECT_GRACE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    InGame {
        client: ClientId,
    },
    /// Connection lost; the character stays in the world and can be hurt.
    Grace {
        remaining: Duration,
    },
    /// Character sleeps at an inn until the player returns.
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InnReason {
    Quit,
    GraceExpired,
}

/// What gameplay must do in response to a session change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// Wake the character where it slept, or spawn it when `first_time`.
    Joined { player: PlayerId, first_time: bool },
    /// Reconnected inside the grace window; hand control back.
    Resumed { player: PlayerId },
    /// Connection lost; the character stays put and the grace window starts.
    ConnectionLost { player: PlayerId },
    /// Move the character to the nearest inn and put it to sleep.
    SendToInn { player: PlayerId, reason: InnReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum RejectReason {
    #[error("client protocol version does not match the host")]
    ProtocolMismatch,
    #[error("session already has the maximum number of players")]
    SessionFull,
    #[error("connection already identified as another player")]
    DuplicateHello,
    #[error("this player id belongs to the host's own player")]
    IdentityInUse,
    // A new reason goes on the end: these travel by their place in this list, so one put in the
    // middle would rename every reason after it for anybody running an older build. That
    // `ProtocolMismatch` stays first is what lets two versions still tell each other apart.
    #[error(
        "client holds a different world from the host: different scenes, or a different scene to start in"
    )]
    DifferentWorld,
}

/// An accepted hello.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub event: SessionEvent,
    /// A stale connection of the same player that the host must now close. Happens when a
    /// player restarts after a crash before the transport has noticed the old connection died.
    pub displaced: Option<ClientId>,
}

#[derive(Debug)]
pub struct SessionRegistry {
    sessions: HashMap<PlayerId, SessionState>,
    by_client: HashMap<ClientId, PlayerId>,
    grace: Duration,
    max_online: usize,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new(RECONNECT_GRACE, MAX_PLAYERS)
    }
}

impl SessionRegistry {
    pub fn new(grace: Duration, max_online: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            by_client: HashMap::new(),
            grace,
            max_online,
        }
    }

    pub fn state(&self, player: PlayerId) -> Option<SessionState> {
        self.sessions.get(&player).copied()
    }

    pub fn player_of(&self, client: ClientId) -> Option<PlayerId> {
        self.by_client.get(&client).copied()
    }

    pub fn sessions(&self) -> impl Iterator<Item = (PlayerId, SessionState)> + '_ {
        self.sessions.iter().map(|(p, s)| (*p, *s))
    }

    /// Players holding a slot: in game or inside the grace window.
    pub fn online_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| !matches!(s, SessionState::Offline))
            .count()
    }

    pub fn hello(&mut self, client: ClientId, player: PlayerId) -> Result<Accepted, RejectReason> {
        if self.by_client.contains_key(&client) {
            return Err(RejectReason::DuplicateHello);
        }
        let mut displaced = None;
        let event = match self.sessions.get(&player) {
            // The new connection wins; the old one is stale.
            Some(SessionState::InGame { client: old }) => {
                self.by_client.remove(old);
                displaced = Some(*old);
                SessionEvent::Resumed { player }
            }
            Some(SessionState::Grace { .. }) => SessionEvent::Resumed { player },
            Some(SessionState::Offline) | None => {
                if self.online_count() >= self.max_online {
                    return Err(RejectReason::SessionFull);
                }
                SessionEvent::Joined {
                    player,
                    first_time: !self.sessions.contains_key(&player),
                }
            }
        };
        self.sessions
            .insert(player, SessionState::InGame { client });
        self.by_client.insert(client, player);
        Ok(Accepted { event, displaced })
    }

    /// Graceful quit: the character goes to an inn immediately. The connection stays mapped to
    /// the player until the transport closes, so messages sent before the goodbye still count.
    pub fn goodbye(&mut self, client: ClientId) -> Option<SessionEvent> {
        let player = self.player_of(client)?;
        if self.sessions.get(&player) != Some(&SessionState::InGame { client }) {
            return None;
        }
        self.sessions.insert(player, SessionState::Offline);
        Some(SessionEvent::SendToInn {
            player,
            reason: InnReason::Quit,
        })
    }

    /// Transport-level disconnect. Starts the grace window only if this connection was the
    /// player's live one; after a goodbye or a takeover it changes nothing.
    pub fn disconnected(&mut self, client: ClientId) -> Option<SessionEvent> {
        let player = self.by_client.remove(&client)?;
        if self.sessions.get(&player) != Some(&SessionState::InGame { client }) {
            return None;
        }
        self.sessions.insert(
            player,
            SessionState::Grace {
                remaining: self.grace,
            },
        );
        Some(SessionEvent::ConnectionLost { player })
    }

    /// Advances grace timers; expired players are sent to an inn.
    pub fn update(&mut self, dt: Duration) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        for (player, state) in &mut self.sessions {
            if let SessionState::Grace { remaining } = state {
                match remaining.checked_sub(dt) {
                    Some(left) if !left.is_zero() => *remaining = left,
                    _ => {
                        *state = SessionState::Offline;
                        events.push(SessionEvent::SendToInn {
                            player: *player,
                            reason: InnReason::GraceExpired,
                        });
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SessionRegistry {
        SessionRegistry::new(Duration::from_secs(60), 2)
    }

    /// Hello, keeping only the event.
    fn hello(
        r: &mut SessionRegistry,
        client: ClientId,
        player: PlayerId,
    ) -> Result<SessionEvent, RejectReason> {
        r.hello(client, player).map(|a| a.event)
    }

    #[test]
    fn first_join_then_rejoin_after_quit() {
        let mut r = registry();
        let p = PlayerId::random();
        assert_eq!(
            hello(&mut r, 1, p),
            Ok(SessionEvent::Joined {
                player: p,
                first_time: true
            })
        );
        assert_eq!(
            r.goodbye(1),
            Some(SessionEvent::SendToInn {
                player: p,
                reason: InnReason::Quit
            })
        );
        assert_eq!(r.state(p), Some(SessionState::Offline));
        assert_eq!(
            r.disconnected(1),
            None,
            "disconnect after goodbye is not a crash"
        );
        assert_eq!(
            hello(&mut r, 2, p),
            Ok(SessionEvent::Joined {
                player: p,
                first_time: false
            })
        );
    }

    #[test]
    fn crash_enters_grace_and_reconnect_resumes() {
        let mut r = registry();
        let p = PlayerId::random();
        hello(&mut r, 1, p).unwrap();
        assert_eq!(
            r.disconnected(1),
            Some(SessionEvent::ConnectionLost { player: p })
        );
        assert!(r.update(Duration::from_secs(59)).is_empty());
        assert_eq!(hello(&mut r, 2, p), Ok(SessionEvent::Resumed { player: p }));
        assert_eq!(r.state(p), Some(SessionState::InGame { client: 2 }));
    }

    #[test]
    fn grace_expiry_sends_to_inn() {
        let mut r = registry();
        let p = PlayerId::random();
        hello(&mut r, 1, p).unwrap();
        r.disconnected(1);
        r.update(Duration::from_secs(30));
        assert_eq!(
            r.update(Duration::from_secs(30)),
            vec![SessionEvent::SendToInn {
                player: p,
                reason: InnReason::GraceExpired
            }]
        );
        assert_eq!(r.state(p), Some(SessionState::Offline));
        assert!(r.update(Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn grace_holds_a_slot() {
        let mut r = registry();
        let (a, b, c) = (PlayerId::random(), PlayerId::random(), PlayerId::random());
        hello(&mut r, 1, a).unwrap();
        hello(&mut r, 2, b).unwrap();
        r.disconnected(2);
        assert_eq!(hello(&mut r, 3, c), Err(RejectReason::SessionFull));
        r.update(Duration::from_secs(60));
        assert!(hello(&mut r, 3, c).is_ok());
    }

    #[test]
    fn restart_before_crash_is_detected_takes_over() {
        let mut r = registry();
        let p = PlayerId::random();
        hello(&mut r, 1, p).unwrap();
        assert_eq!(
            r.hello(2, p),
            Ok(Accepted {
                event: SessionEvent::Resumed { player: p },
                displaced: Some(1),
            })
        );
        assert_eq!(r.state(p), Some(SessionState::InGame { client: 2 }));
        assert_eq!(
            r.disconnected(1),
            None,
            "late disconnect of the stale connection is ignored"
        );
        assert_eq!(r.state(p), Some(SessionState::InGame { client: 2 }));
    }

    #[test]
    fn rejoin_before_old_connection_closes_is_not_hurt_by_its_disconnect() {
        let mut r = registry();
        let p = PlayerId::random();
        hello(&mut r, 1, p).unwrap();
        r.goodbye(1);
        assert_eq!(
            r.player_of(1),
            Some(p),
            "still mapped until the transport closes"
        );
        hello(&mut r, 2, p).unwrap();
        assert_eq!(r.disconnected(1), None);
        assert_eq!(r.state(p), Some(SessionState::InGame { client: 2 }));
    }

    #[test]
    fn one_connection_cannot_claim_two_players() {
        let mut r = registry();
        hello(&mut r, 1, PlayerId::random()).unwrap();
        assert_eq!(
            hello(&mut r, 1, PlayerId::random()),
            Err(RejectReason::DuplicateHello)
        );
    }
}
