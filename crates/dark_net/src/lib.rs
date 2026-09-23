//! Networking foundation: host-authoritative listen server over renet.
//!
//! Single-player is a [`Host`] with only its local player, so gameplay never has a separate
//! offline path. Remote players join over UDP (`udp` feature). See `docs/PLAN.md` §5.

mod host;
mod protocol;
mod session;

#[cfg(feature = "udp")]
mod beacon;
#[cfg(feature = "udp")]
mod client;
#[cfg(feature = "udp")]
mod conditions;

use std::fmt;
use std::time::Duration;

use renet::{ChannelConfig, ConnectionConfig, SendType};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use host::{Host, HostConfig};
pub use protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, decode, encode};
pub use session::{
    Accepted, InnReason, MAX_PLAYERS, RECONNECT_GRACE, RejectReason, SessionEvent, SessionRegistry,
    SessionState,
};

#[cfg(feature = "udp")]
pub use beacon::{Beacon, Search, Status};
#[cfg(feature = "udp")]
pub use client::{ClientStatus, RemoteClient};
#[cfg(feature = "udp")]
pub use conditions::NetConditions;

/// Transport-level connection id. Changes on every reconnect.
pub type ClientId = renet::ClientId;

/// Stable identity of a player across sessions. Dev builds use a random UUID stored locally;
/// shipped builds will use the platform account id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId(pub Uuid);

impl PlayerId {
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Message channels, identical in both directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Session handshake; consumed by [`Host`] and [`RemoteClient`], never by gameplay.
    Control,
    /// Reliable, ordered gameplay events (dialogue choices, trades, party invites).
    Events,
    /// Unreliable, latest-wins traffic (input, snapshots).
    State,
}

impl From<Channel> for u8 {
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Control => 0,
            Channel::Events => 1,
            Channel::State => 2,
        }
    }
}

/// Transport protocol id. Deliberately constant: netcode silently drops mismatched ids, so
/// version compatibility is checked in `Hello` instead, where the client gets a clear rejection.
pub const PROTOCOL_ID: u64 = 0xDA2C_E000_0000_0000;

pub fn connection_config() -> ConnectionConfig {
    let reliable = SendType::ReliableOrdered {
        resend_time: Duration::from_millis(200),
    };
    let channels = vec![
        ChannelConfig {
            channel_id: Channel::Control.into(),
            max_memory_usage_bytes: 64 * 1024,
            send_type: reliable.clone(),
        },
        ChannelConfig {
            channel_id: Channel::Events.into(),
            max_memory_usage_bytes: 4 * 1024 * 1024,
            send_type: reliable,
        },
        ChannelConfig {
            channel_id: Channel::State.into(),
            max_memory_usage_bytes: 4 * 1024 * 1024,
            send_type: SendType::Unreliable,
        },
    ];
    // Clients only send small input messages; the host need not buffer much from any of them.
    let mut client_channels = channels.clone();
    client_channels[2].max_memory_usage_bytes = 64 * 1024;
    ConnectionConfig {
        available_bytes_per_tick: 60_000,
        server_channels_config: channels,
        client_channels_config: client_channels,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(String),
}
