//! The host: renet server, optional UDP transport, optional in-process local player.

use std::collections::HashMap;
use std::time::Duration;

use renet::{Bytes, RenetClient, RenetServer, ServerEvent};

use crate::protocol::{ClientMessage, PROTOCOL_VERSION, ServerMessage, decode, encode};
use crate::session::{Accepted, RejectReason, SessionEvent, SessionRegistry, SessionState};
use crate::{Channel, ClientId, NetError, PlayerId, connection_config};

/// Connection id reserved for the in-process player. Remote ids are random u64s below it.
const LOCAL_CLIENT_ID: ClientId = u64::MAX;
/// A connection that has not identified itself by then is closed, so it cannot hold a slot.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Rejected connections stay open this long so the rejection reason reaches the client.
const REJECT_LINGER: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct HostConfig {
    /// UDP address to accept remote players on. `None` hosts a single-player session.
    #[cfg(feature = "udp")]
    pub bind: Option<std::net::SocketAddr>,
    /// The world this host is playing in, as `dark_assets::Project::fingerprint` gives it. A
    /// player who holds another one is turned away with [`RejectReason::DifferentWorld`] rather
    /// than left to walk on ground the host does not have (docs/PLAN.md §5).
    ///
    /// Zero means a host that does not care — the tests, and the world simulation, which has no
    /// project to fingerprint. A player is never turned away for it.
    pub world: u64,
}

/// The host's own player. It gets a server of its own: renet keeps acknowledgements queued for
/// every connection, and a local connection inside the UDP server would have the transport try,
/// and fail, to send them over the network every tick.
struct LocalPlayer {
    server: RenetServer,
    client: RenetClient,
    player: PlayerId,
}

pub struct Host {
    /// Remote players, over the UDP transport.
    server: RenetServer,
    #[cfg(feature = "udp")]
    udp: Option<renet_netcode::NetcodeServerTransport>,
    local: Option<LocalPlayer>,
    sessions: SessionRegistry,
    /// The world this host plays in; see [`HostConfig::world`].
    world: u64,
    /// Connections to close when their timer runs out: unidentified (hello timeout), rejected
    /// (linger), or leaving (closed next tick, after gameplay has read their final messages).
    closing: HashMap<ClientId, Duration>,
    /// Why players were turned away, in order, for the tests that ask whether it was the right
    /// reason. A rejection otherwise leaves nothing behind but a line in the log.
    #[cfg(test)]
    refused: Vec<RejectReason>,
}

impl Host {
    pub fn new(config: HostConfig) -> Result<Self, NetError> {
        Ok(Self {
            server: RenetServer::new(connection_config()),
            #[cfg(feature = "udp")]
            udp: config.bind.map(bind_udp).transpose()?,
            local: None,
            sessions: SessionRegistry::default(),
            closing: HashMap::new(),
            #[cfg(test)]
            refused: Vec::new(),
            world: config.world,
        })
    }

    /// The UDP address remote players connect to, if this host accepts them.
    #[cfg(feature = "udp")]
    pub fn udp_addr(&self) -> Option<std::net::SocketAddr> {
        self.udp
            .as_ref()
            .and_then(|t| t.addresses().first().copied())
    }

    /// The in-process player, if one is connected.
    pub fn local_player(&self) -> Option<PlayerId> {
        self.local.as_ref().map(|l| l.player)
    }

    pub fn sessions(&self) -> &SessionRegistry {
        &self.sessions
    }

    /// Connects the in-process player. It goes through the same handshake as remote players.
    pub fn connect_local(&mut self, player: PlayerId) {
        let mut server = RenetServer::new(connection_config());
        let mut client = server.new_local_client(LOCAL_CLIENT_ID);
        client.send_message(
            Channel::Control,
            encode(&ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                player,
                // The host's own player is in the host's own world, by definition.
                world: self.world,
            }),
        );
        self.local = Some(LocalPlayer {
            server,
            client,
            player,
        });
    }

    /// The server a connection belongs to.
    fn server_of(&mut self, client: ClientId) -> Option<&mut RenetServer> {
        if client == LOCAL_CLIENT_ID {
            self.local.as_mut().map(|l| &mut l.server)
        } else {
            Some(&mut self.server)
        }
    }

    /// Every connection on both servers.
    fn clients(&self) -> Vec<ClientId> {
        let mut clients = self.server.clients_id();
        if let Some(local) = &self.local {
            clients.extend(local.server.clients_id());
        }
        clients
    }

    /// Receives packets, runs the session handshake and advances grace timers.
    /// Call once per simulation tick with the fixed timestep.
    pub fn update(&mut self, dt: Duration) -> Vec<SessionEvent> {
        // Close due connections first: a leaving client's final messages were read last tick.
        let mut due = Vec::new();
        for (client, left) in &mut self.closing {
            *left = left.saturating_sub(dt);
            if left.is_zero() {
                due.push(*client);
            }
        }
        for client in due {
            self.closing.remove(&client);
            if let Some(server) = self.server_of(client) {
                server.disconnect(client);
            }
        }

        self.server.update(dt);
        #[cfg(feature = "udp")]
        if let Some(udp) = &mut self.udp
            && let Err(err) = udp.update(dt, &mut self.server)
        {
            tracing::warn!("udp transport: {err}");
        }
        if let Some(local) = &mut self.local {
            local.server.update(dt);
            local.client.update(dt);
        }
        self.exchange_local();

        let mut server_events = Vec::new();
        while let Some(event) = self.server.get_event() {
            server_events.push(event);
        }
        if let Some(local) = &mut self.local {
            while let Some(event) = local.server.get_event() {
                server_events.push(event);
            }
        }
        let mut events = Vec::new();
        for event in server_events {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    tracing::debug!("client {client_id} connected");
                    if client_id != LOCAL_CLIENT_ID {
                        self.closing.insert(client_id, HELLO_TIMEOUT);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    tracing::debug!("client {client_id} disconnected: {reason}");
                    self.closing.remove(&client_id);
                    events.extend(self.sessions.disconnected(client_id));
                }
            }
        }
        for client in self.clients() {
            while let Some(server) = self.server_of(client) {
                let Some(bytes) = server.receive_message(client, Channel::Control) else {
                    break;
                };
                match decode::<ClientMessage>(&bytes) {
                    Some(message) => events.extend(self.handle_control(client, message)),
                    None => tracing::warn!("client {client} sent an undecodable control message"),
                }
            }
        }
        events.extend(self.sessions.update(dt));
        events
    }

    fn handle_control(&mut self, client: ClientId, message: ClientMessage) -> Option<SessionEvent> {
        match message {
            ClientMessage::Hello {
                protocol_version,
                player,
                world,
            } => {
                let local_player = self.local_player();
                let result = if protocol_version != PROTOCOL_VERSION {
                    Err(RejectReason::ProtocolMismatch)
                } else if self.world != 0 && world != self.world {
                    Err(RejectReason::DifferentWorld)
                } else if client != LOCAL_CLIENT_ID && local_player == Some(player) {
                    Err(RejectReason::IdentityInUse)
                } else {
                    self.sessions.hello(client, player)
                };
                match result {
                    Ok(Accepted { event, displaced }) => {
                        self.closing.remove(&client);
                        if let Some(stale) = displaced {
                            tracing::info!(
                                "player {player} reconnected; closing stale client {stale}"
                            );
                            self.closing.remove(&stale);
                            if let Some(server) = self.server_of(stale) {
                                server.disconnect(stale);
                            }
                        }
                        if let Some(server) = self.server_of(client) {
                            server.send_message(
                                client,
                                Channel::Control,
                                encode(&ServerMessage::Welcome),
                            );
                        }
                        Some(event)
                    }
                    Err(reason) => {
                        tracing::info!("rejected client {client}: {reason}");
                        #[cfg(test)]
                        self.refused.push(reason);
                        if let Some(server) = self.server_of(client) {
                            server.send_message(
                                client,
                                Channel::Control,
                                encode(&ServerMessage::Rejected(reason)),
                            );
                        }
                        self.closing.insert(client, REJECT_LINGER);
                        None
                    }
                }
            }
            ClientMessage::Goodbye => {
                // The host closes the connection, next tick, so the goodbye is never raced by the
                // client's own disconnect and gameplay still reads messages sent before it.
                self.closing.insert(client, Duration::ZERO);
                self.sessions.goodbye(client)
            }
        }
    }

    /// Moves packets both ways between the local server and the in-process player.
    fn exchange_local(&mut self) {
        if let Some(local) = &mut self.local
            && let Err(err) = local
                .server
                .process_local_client(LOCAL_CLIENT_ID, &mut local.client)
        {
            tracing::warn!("local client: {err}");
        }
    }

    /// Flushes queued messages. Call once per tick after gameplay has sent.
    pub fn send_packets(&mut self) {
        self.exchange_local();
        #[cfg(feature = "udp")]
        if let Some(udp) = &mut self.udp {
            udp.send_packets(&mut self.server);
        }
    }

    /// Ends the session for everyone. Remote players are told immediately instead of timing out.
    pub fn shutdown(&mut self) {
        #[cfg(feature = "udp")]
        if let Some(udp) = &mut self.udp {
            udp.disconnect_all(&mut self.server);
        }
    }

    /// Sends to a player that has completed the handshake. Messages to unknown players are dropped.
    pub fn send(&mut self, player: PlayerId, channel: Channel, message: impl Into<Bytes>) {
        if let Some(SessionState::InGame { client }) = self.sessions.state(player)
            && let Some(server) = self.server_of(client)
        {
            server.send_message(client, channel, message);
        }
    }

    /// Sends to every player that has completed the handshake.
    pub fn broadcast(&mut self, channel: Channel, message: impl Into<Bytes>) {
        let message = message.into();
        let clients: Vec<ClientId> = self
            .sessions
            .sessions()
            .filter_map(|(_, state)| match state {
                SessionState::InGame { client } => Some(client),
                _ => None,
            })
            .collect();
        for client in clients {
            if let Some(server) = self.server_of(client) {
                server.send_message(client, channel, message.clone());
            }
        }
    }

    /// Drains gameplay messages from identified players, including a leaving player's last ones.
    pub fn receive(&mut self, channel: Channel) -> Vec<(PlayerId, Bytes)> {
        assert_ne!(
            channel,
            Channel::Control,
            "the control channel is owned by the host"
        );
        let mut out = Vec::new();
        for client in self.clients() {
            let Some(player) = self.sessions.player_of(client) else {
                continue;
            };
            while let Some(bytes) = self
                .server_of(client)
                .and_then(|s| s.receive_message(client, channel))
            {
                out.push((player, bytes));
            }
        }
        out
    }

    /// Gameplay side of the in-process player.
    pub fn local_send(&mut self, channel: Channel, message: impl Into<Bytes>) {
        if let Some(local) = &mut self.local {
            local.client.send_message(channel, message);
        }
    }

    pub fn local_receive(&mut self, channel: Channel) -> Option<Bytes> {
        assert_ne!(
            channel,
            Channel::Control,
            "the control channel is owned by the host"
        );
        self.local.as_mut()?.client.receive_message(channel)
    }
}

#[cfg(feature = "udp")]
fn bind_udp(addr: std::net::SocketAddr) -> Result<renet_netcode::NetcodeServerTransport, NetError> {
    use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};

    let socket = std::net::UdpSocket::bind(addr)?;
    let public = socket.local_addr()?;
    let config = ServerConfig {
        current_time: crate::client::unix_now(),
        // Transport slots, not players: the session registry enforces the player limit
        // (including an in-process host player) and closes rejected connections.
        max_clients: crate::MAX_PLAYERS,
        protocol_id: crate::PROTOCOL_ID,
        public_addresses: vec![public],
        // TODO(M9): secure connect tokens once Steam relay lands.
        authentication: ServerAuthentication::Unsecure,
    };
    Ok(NetcodeServerTransport::new(config, socket)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player holding a different world is turned away, and told why; one holding the same
    /// world is let in.
    ///
    /// Two machines with different scene files are not in the same game at all: the maps are
    /// numbered by walking the ways out from the starting scene, so the same number can mean two
    /// different maps, and a scene made from a seed is a whole country in one number. Before this
    /// the handshake had nothing to say about any of it (docs/PLAN.md §5, §24.4).
    #[test]
    fn a_player_from_another_world_is_turned_away() {
        let mut host = Host::new(HostConfig {
            world: 0xDEAD_BEEF,
            ..HostConfig::default()
        })
        .unwrap();
        // The host's own player is in the host's own world, whatever that is.
        host.connect_local(PlayerId::random());
        let dt = Duration::from_millis(16);
        let events: Vec<_> = (0..3).flat_map(|_| host.update(dt)).collect();
        assert!(matches!(events[..], [SessionEvent::Joined { .. }]));

        // A stranger who holds another world, and one who holds this one. A client that sends
        // no world at all is turned away too: only the host may be the one that does not care.
        for (world, welcome) in [(0xDEAD_BEEF, true), (0x0BAD_F00D, false), (0, false)] {
            let stranger = PlayerId::random();
            let reply = host.handle_control(
                7,
                ClientMessage::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    player: stranger,
                    world,
                },
            );
            assert_eq!(
                reply.is_some(),
                welcome,
                "a player holding world {world:#x} should{} have been let in",
                if welcome { "" } else { " not" }
            );
            if welcome {
                host.sessions.disconnected(7);
            } else {
                // And turned away for the right reason: told to fix their copy of the project,
                // not that the game is full or that they are somebody else.
                assert_eq!(
                    host.refused.last(),
                    Some(&RejectReason::DifferentWorld),
                    "a player holding world {world:#x} was refused for the wrong reason"
                );
            }
        }
    }

    /// A host that does not care which world it is in — the tests, and a host with no project —
    /// lets anybody in, whatever they hold.
    #[test]
    fn a_host_without_a_world_of_its_own_asks_nobody_about_theirs() {
        let mut host = Host::new(HostConfig::default()).unwrap();
        let reply = host.handle_control(
            7,
            ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                player: PlayerId::random(),
                world: 0x0BAD_F00D,
            },
        );
        assert!(
            reply.is_some(),
            "nobody is turned away by a host with no world"
        );
    }

    #[test]
    fn local_player_goes_through_handshake() {
        let mut host = Host::new(HostConfig::default()).unwrap();
        let player = PlayerId::random();
        host.connect_local(player);

        let dt = Duration::from_millis(16);
        let events: Vec<_> = (0..3).flat_map(|_| host.update(dt)).collect();
        assert_eq!(
            events,
            vec![SessionEvent::Joined {
                player,
                first_time: true
            }]
        );

        host.local_send(Channel::Events, "ping");
        host.update(dt);
        let received = host.receive(Channel::Events);
        assert_eq!(received, vec![(player, Bytes::from("ping"))]);

        host.send(player, Channel::Events, "pong");
        host.send_packets();
        assert_eq!(
            host.local_receive(Channel::Events),
            Some(Bytes::from("pong"))
        );
    }

    #[cfg(feature = "udp")]
    #[test]
    fn local_player_still_receives_when_udp_is_active() {
        let mut host = Host::new(HostConfig {
            world: 0,
            bind: Some("127.0.0.1:0".parse().unwrap()),
        })
        .unwrap();
        let player = PlayerId::random();
        host.connect_local(player);
        let dt = Duration::from_millis(16);
        for _ in 0..3 {
            host.update(dt);
            host.send_packets();
        }

        // The in-process player lives on its own server, never on the one the UDP transport flushes.
        assert!(!host.server.clients_id().contains(&LOCAL_CLIENT_ID));
        let mut received = 0;
        for tick in 0..10u8 {
            host.update(dt);
            host.broadcast(Channel::State, vec![tick]);
            host.send_packets();
            while host.local_receive(Channel::State).is_some() {
                received += 1;
            }
        }
        assert_eq!(received, 10);
    }
}
