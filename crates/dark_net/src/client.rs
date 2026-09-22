//! A remote player connecting to a [`crate::Host`] over UDP.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime};

use renet::{Bytes, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};

use crate::conditions::{Conditioner, NetConditions};
use crate::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, decode, encode};
use crate::{Channel, NetError, PROTOCOL_ID, PlayerId, RejectReason, connection_config};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientStatus {
    Connecting,
    /// Handshake accepted; gameplay traffic may flow.
    InGame,
    Rejected(RejectReason),
    /// Goodbye sent; waiting for the host to close the connection.
    Leaving,
    Disconnected,
}

/// How long to wait for the host to close the connection after a goodbye.
const LEAVE_TIMEOUT: Duration = Duration::from_secs(1);

pub struct RemoteClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
    player: PlayerId,
    hello_sent: bool,
    status: ClientStatus,
    leaving_for: Duration,
    /// Development network simulation, if enabled.
    conditions: Option<Conditioner>,
}

impl RemoteClient {
    pub fn connect(server: SocketAddr, player: PlayerId) -> Result<Self, NetError> {
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        }
        .parse()
        .expect("literal socket address");
        let socket = UdpSocket::bind(bind)?;
        let authentication = ClientAuthentication::Unsecure {
            protocol_id: PROTOCOL_ID,
            // Transport id only; identity is the PlayerId sent in Hello.
            client_id: rand_client_id(),
            server_addr: server,
            user_data: None,
        };
        let transport = NetcodeClientTransport::new(unix_now(), authentication, socket)
            .map_err(|e| NetError::Transport(e.to_string()))?;
        Ok(Self {
            client: RenetClient::new(connection_config()),
            transport,
            player,
            hello_sent: false,
            status: ClientStatus::Connecting,
            leaving_for: Duration::ZERO,
            conditions: None,
        })
    }

    pub fn status(&self) -> ClientStatus {
        self.status
    }

    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// Receives packets and runs the handshake. Call once per tick.
    pub fn update(&mut self, dt: Duration) {
        if self.client.is_disconnected() {
            // The transport errors on every call once closed; nothing left to do.
            return;
        }
        self.client.update(dt);
        if let Err(err) = self.transport.update(dt, &mut self.client)
            && self.status != ClientStatus::Leaving
        {
            // While leaving, the host closing the connection is the expected outcome.
            tracing::warn!("udp transport: {err}");
        }
        if self.client.is_disconnected() {
            if !matches!(self.status, ClientStatus::Rejected(_)) {
                self.status = ClientStatus::Disconnected;
            }
            return;
        }
        if self.status == ClientStatus::Leaving {
            self.leaving_for += dt;
            if self.leaving_for >= LEAVE_TIMEOUT {
                self.transport.disconnect();
                self.status = ClientStatus::Disconnected;
            }
            return;
        }
        if self.client.is_connected() && !self.hello_sent {
            self.client.send_message(
                Channel::Control,
                encode(&ClientMessage::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    player: self.player,
                }),
            );
            self.hello_sent = true;
        }
        while let Some(bytes) = self.client.receive_message(Channel::Control) {
            match decode::<ServerMessage>(&bytes) {
                Some(ServerMessage::Welcome) => self.status = ClientStatus::InGame,
                Some(ServerMessage::Rejected(reason)) => {
                    self.status = ClientStatus::Rejected(reason);
                    self.transport.disconnect();
                    return;
                }
                None => tracing::warn!("host sent an undecodable control message"),
            }
        }
        if let Some(sim) = &mut self.conditions {
            sim.advance(dt);
            for channel in [Channel::Events, Channel::State] {
                while let Some(bytes) = self.client.receive_message(channel) {
                    sim.incoming(channel, bytes);
                }
            }
            for (channel, bytes) in sim.due_outgoing() {
                self.client.send_message(channel, bytes);
            }
        }
    }

    pub fn send(&mut self, channel: Channel, message: impl Into<Bytes>) {
        assert_ne!(
            channel,
            Channel::Control,
            "the control channel is owned by the client"
        );
        if self.status == ClientStatus::InGame {
            match &mut self.conditions {
                Some(sim) => sim.outgoing(channel, message.into()),
                None => self.client.send_message(channel, message),
            }
        }
    }

    pub fn receive(&mut self, channel: Channel) -> Option<Bytes> {
        assert_ne!(
            channel,
            Channel::Control,
            "the control channel is owned by the client"
        );
        match &mut self.conditions {
            Some(sim) => sim.take_incoming(channel),
            None => self.client.receive_message(channel),
        }
    }

    /// Simulates latency, jitter and loss on gameplay messages (development only).
    pub fn set_conditions(&mut self, conditions: Option<NetConditions>) {
        self.conditions = conditions.map(Conditioner::new);
    }

    /// Flushes queued messages. Call once per tick after gameplay has sent.
    pub fn send_packets(&mut self) {
        if self.client.is_disconnected() {
            return;
        }
        if let Err(err) = self.transport.send_packets(&mut self.client)
            && self.status != ClientStatus::Leaving
        {
            tracing::warn!("udp transport: {err}");
        }
    }

    /// Starts a graceful quit. Keep calling [`Self::update`] and [`Self::send_packets`] until the
    /// status is `Disconnected`. If the goodbye is lost the host treats this as a crash, and the
    /// character still reaches an inn once the grace window expires.
    pub fn quit(&mut self) {
        if self.status == ClientStatus::InGame {
            // Whatever the conditioner still holds was sent before the goodbye; deliver it first.
            if let Some(sim) = &mut self.conditions {
                for (channel, bytes) in sim.drain_outgoing() {
                    self.client.send_message(channel, bytes);
                }
            }
            self.client
                .send_message(Channel::Control, encode(&ClientMessage::Goodbye));
            self.status = ClientStatus::Leaving;
        } else {
            self.transport.disconnect();
            self.status = ClientStatus::Disconnected;
        }
    }
}

pub(crate) fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock is after 1970")
}

fn rand_client_id() -> u64 {
    // Never collide with the host's reserved local id.
    uuid::Uuid::new_v4().as_u64_pair().0 & !(1 << 63)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Host, HostConfig, InnReason, SessionEvent};

    fn pump(
        host: &mut Host,
        client: &mut RemoteClient,
        until: impl Fn(&RemoteClient, &[SessionEvent]) -> bool,
    ) -> Vec<SessionEvent> {
        let dt = Duration::from_millis(16);
        let mut events = Vec::new();
        for _ in 0..300 {
            client.update(dt);
            client.send_packets();
            events.extend(host.update(dt));
            host.send_packets();
            if until(client, &events) {
                return events;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("condition not reached; events so far: {events:?}");
    }

    #[test]
    fn remote_player_joins_exchanges_and_quits_over_udp() {
        let mut host = Host::new(HostConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        })
        .unwrap();
        let addr = host.udp_addr().unwrap();
        let player = PlayerId::random();
        let mut client = RemoteClient::connect(addr, player).unwrap();

        let events = pump(&mut host, &mut client, |c, _| {
            c.status() == ClientStatus::InGame
        });
        assert_eq!(
            events,
            vec![SessionEvent::Joined {
                player,
                first_time: true
            }]
        );

        client.send(Channel::Events, "ping");
        let mut received = Vec::new();
        for _ in 0..300 {
            pump(&mut host, &mut client, |_, _| true);
            received.extend(host.receive(Channel::Events));
            if !received.is_empty() {
                break;
            }
        }
        assert_eq!(received, vec![(player, Bytes::from("ping"))]);

        // A message sent right before quitting must still reach gameplay.
        client.send(Channel::Events, "last words");
        client.quit();
        let dt = Duration::from_millis(16);
        let (mut events, mut received) = (Vec::new(), Vec::new());
        for _ in 0..300 {
            client.update(dt);
            client.send_packets();
            events.extend(host.update(dt));
            received.extend(host.receive(Channel::Events));
            host.send_packets();
            if client.status() == ClientStatus::Disconnected {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(client.status(), ClientStatus::Disconnected);
        assert_eq!(received, vec![(player, Bytes::from("last words"))]);
        assert_eq!(
            events,
            vec![SessionEvent::SendToInn {
                player,
                reason: InnReason::Quit
            }]
        );
    }

    #[test]
    fn rejected_client_is_told_why_and_closed() {
        let mut host = Host::new(HostConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        })
        .unwrap();
        let player = PlayerId::random();
        host.connect_local(player);
        // Same identity as the host's own player.
        let mut client = RemoteClient::connect(host.udp_addr().unwrap(), player).unwrap();
        let events = pump(&mut host, &mut client, |c, _| {
            matches!(c.status(), ClientStatus::Rejected(_))
        });
        assert_eq!(
            client.status(),
            ClientStatus::Rejected(RejectReason::IdentityInUse)
        );
        assert_eq!(
            events,
            vec![SessionEvent::Joined {
                player,
                first_time: true
            }],
            "only the local player joined"
        );
        assert_eq!(host.sessions().online_count(), 1);
    }

    #[test]
    fn dropped_client_enters_grace_on_host() {
        let mut host = Host::new(HostConfig {
            bind: Some("127.0.0.1:0".parse().unwrap()),
        })
        .unwrap();
        let player = PlayerId::random();
        let mut client = RemoteClient::connect(host.udp_addr().unwrap(), player).unwrap();
        pump(&mut host, &mut client, |c, _| {
            c.status() == ClientStatus::InGame
        });

        // Disconnect without a goodbye, as a crash or cable pull would.
        client.transport.disconnect();
        let events = pump(&mut host, &mut client, |_, e| !e.is_empty());
        assert_eq!(events, vec![SessionEvent::ConnectionLost { player }]);
        assert!(matches!(
            host.sessions().state(player),
            Some(crate::SessionState::Grace { .. })
        ));
    }
}
