//! Finding games to join on this network (docs/PLAN.md §23).
//!
//! A host answers a small question on a port of its own, beside the game's: who is playing, and
//! how many of them. Anyone looking shouts the question to the whole network and listens for a
//! moment; what comes back is the list a player picks from, each with `2/4` beside it.
//!
//! This is only for finding games. Playing one goes through the game's own port as before, and
//! a host that nobody can reach (another network, a router in the way) is simply not on the
//! list; [`Search::ask`] puts the question to one host directly, for an address a player was
//! given rather than found.
//!
//! Nothing here trusts what arrives. Everyone on the network can send anything to these ports,
//! so answers are read for their shape, cut to a length a menu can show, and counted: a flood
//! costs one frame's work at most, and a host answers only the machines on its own network.

use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{MAX_PLAYERS, PROTOCOL_VERSION, decode, encode};

/// The question, and the answer's first bytes: anything else on the port is not ours.
const ASKED: &[u8] = b"dark-wagon?1";
const ANSWERED: &[u8] = b"dark-wagon!1";
/// A host listens for questions here: the game's own port and one more.
pub const BESIDE: u16 = 1;
/// Answers are small; a bigger packet is not one of ours.
const MOST: usize = 512;
/// Packets taken in one call. A frame is worth more than answering a flood.
const AT_ONCE: usize = 32;
/// Games kept on the list. More than this on one network is somebody making them up; the one
/// that answered longest ago gives way, so a burst of them cannot hold a real game off the list.
/// A steady stream still can: nothing here proves who a game belongs to.
const MOST_GAMES: usize = 32;
/// Letters of a game's name that reach the menu; the rest would not be read anyway.
const NAME_LETTERS: usize = 40;

/// How long a game stays on the list after it last answered: long enough to outlive a lost
/// packet or two, short enough that a game someone closed goes away while they are still looking.
pub const FORGET: Duration = Duration::from_secs(6);

/// The game on the list that answered longest ago.
fn oldest(found: &[(SocketAddr, Status, Instant)]) -> Option<usize> {
    found
        .iter()
        .enumerate()
        .min_by_key(|(_, (.., heard))| *heard)
        .map(|(nth, _)| nth)
}

/// The port a game is asked on, beside the one it is played on.
fn beside(port: u16) -> io::Result<u16> {
    port.checked_add(BESIDE).ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("a game on port {port} has no port beside it to be asked on"),
        )
    })
}

/// A name as a menu can show it: one line, no longer than it can hold.
fn tidy(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(NAME_LETTERS)
        .collect()
}

/// Whether this is a machine near enough to be asking: a home or office network, a link-local
/// address, this machine itself, or the shared range a mesh network (Tailscale and the like)
/// hands out. A question from further off is somebody else's business, and answering it would
/// make this game a way to shout at strangers. The shared range is also what an internet
/// provider hands out behind its own big network, so a player there answers their provider's
/// other customers; a short answer to a short question, but not nobody.
fn nearby(who: IpAddr) -> bool {
    match who {
        // `is_private` is the three old ranges; 100.64/10 is what a mesh network (Tailscale,
        // and the like) hands out, and those are as near as any machine in the house.
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            let meshed = first == 100 && (64..128).contains(&second);
            ip.is_private() || ip.is_link_local() || ip.is_loopback() || ip.is_broadcast() || meshed
        }
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unicast_link_local() || ip.is_unique_local(),
    }
}

/// What a host says about itself when asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// The wire version both sides must share to play together.
    pub version: u32,
    /// What to call it in the list: the game's name.
    pub name: String,
    /// How many are in it now, the host included.
    pub players: u8,
    /// How many it holds.
    pub most: u8,
    /// The port to join on; the question was asked on another.
    pub port: u16,
}

impl Status {
    /// Whether this game can be played with at all: it speaks the same wire version.
    pub fn understood(&self) -> bool {
        self.version == PROTOCOL_VERSION
    }

    /// Whether this game can be joined: the same version, and room for one more.
    pub fn joinable(&self) -> bool {
        self.understood() && self.players < self.most
    }
}

/// A host answering questions about itself.
pub struct Beacon {
    socket: UdpSocket,
    status: Status,
}

impl Beacon {
    /// Answers on `port + BESIDE` for a game hosted on `port`.
    pub fn new(port: u16, name: String) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, beside(port)?))?;
        socket.set_nonblocking(true)?;
        let status = Status {
            version: PROTOCOL_VERSION,
            name: tidy(&name),
            players: 0,
            most: MAX_PLAYERS as u8,
            port,
        };
        // An answer that will not fit is one nobody can read: say so here rather than going
        // quietly missing from every list on the network.
        let size = encode(&status).len() + ANSWERED.len();
        if size > MOST {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("{size} bytes is too much to say about a game (at most {MOST})"),
            ));
        }
        Ok(Self { socket, status })
    }

    /// How many are playing now, for the next answer.
    pub fn playing(&mut self, players: usize) {
        self.status.players = players.min(u8::MAX as usize) as u8;
    }

    /// Answers everyone nearby who has asked since the last call. Never blocks.
    pub fn answer(&mut self) {
        let mut buffer = [0u8; MOST];
        for _ in 0..AT_ONCE {
            let (read, from) = match self.socket.recv_from(&mut buffer) {
                Ok(heard) => heard,
                Err(err) if err.kind() == ErrorKind::WouldBlock => return,
                // Not an empty queue: Windows reports an oversized packet, and a host that has
                // gone, as errors on the socket. The next packet is still worth reading.
                Err(err) => {
                    tracing::debug!("cannot hear who is asking: {err}");
                    continue;
                }
            };
            if &buffer[..read] != ASKED || !nearby(from.ip()) {
                continue;
            }
            let mut packet = ANSWERED.to_vec();
            packet.extend(encode(&self.status));
            if let Err(err) = self.socket.send_to(&packet, from) {
                tracing::debug!("cannot answer {from}: {err}");
            }
        }
    }
}

/// Looking for games to join.
pub struct Search {
    socket: UdpSocket,
    found: Vec<(SocketAddr, Status, Instant)>,
}

impl Search {
    pub fn new() -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            found: Vec::new(),
        })
    }

    /// Asks the whole network, on the ports games are hosted on. What has already answered stays
    /// on the list until it has been quiet for [`FORGET`], so asking again does not empty it.
    pub fn ask_around(&mut self, ports: &[u16]) {
        for &port in ports {
            match beside(port) {
                Ok(port) => self.shout(SocketAddr::from((Ipv4Addr::BROADCAST, port))),
                Err(err) => tracing::debug!("cannot ask about port {port}: {err}"),
            }
        }
    }

    /// Asks one host, wherever it is: an address a player was given rather than found.
    pub fn ask(&mut self, host: SocketAddr) {
        match beside(host.port()) {
            Ok(port) => {
                let mut to = host;
                to.set_port(port);
                self.shout(to);
            }
            Err(err) => tracing::debug!("cannot ask {host}: {err}"),
        }
    }

    fn shout(&self, to: SocketAddr) {
        if let Err(err) = self.socket.send_to(ASKED, to) {
            tracing::debug!("cannot ask {to}: {err}");
        }
    }

    /// Takes in whatever has answered since the last call, and forgets whoever has gone quiet.
    /// Never blocks.
    pub fn listen(&mut self) {
        let mut buffer = [0u8; MOST];
        for _ in 0..AT_ONCE {
            let (read, from) = match self.socket.recv_from(&mut buffer) {
                Ok(heard) => heard,
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                // As in [`Beacon::answer`]: an error here is about one packet, not the queue.
                Err(err) => {
                    tracing::debug!("cannot hear the answers: {err}");
                    continue;
                }
            };
            let Some(mut status) = buffer[..read]
                .strip_prefix(ANSWERED)
                .and_then(decode::<Status>)
            else {
                continue;
            };
            status.name = tidy(&status.name);
            // The game is on its own port, wherever the answer came from.
            let mut at = from;
            at.set_port(status.port);
            let now = Instant::now();
            match self.found.iter().position(|(known, ..)| *known == at) {
                Some(nth) => self.found[nth] = (at, status, now),
                None if self.found.len() < MOST_GAMES => self.found.push((at, status, now)),
                // The list is full: the one that answered longest ago gives way, so a burst of
                // made-up games cannot keep a real one off it.
                None => {
                    if let Some(oldest) = oldest(&self.found) {
                        self.found[oldest] = (at, status, now);
                    }
                }
            }
        }
        self.found.retain(|(.., heard)| heard.elapsed() < FORGET);
    }

    /// The games heard from lately, in the order their places on the list were taken.
    pub fn found(&self) -> Vec<(SocketAddr, Status)> {
        self.found
            .iter()
            .map(|(at, status, _)| (*at, status.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A beacon on a port this machine has free, with the game's port beside it.
    fn beacon(name: &str) -> (Beacon, u16) {
        // Ports are taken and given back all the time, so a free one is found by taking it.
        for port in 41000..41100 {
            if let Ok(beacon) = Beacon::new(port, name.to_owned()) {
                return (beacon, port);
            }
        }
        panic!("no free port to answer on");
    }

    /// A host answers the question, and says how full it is.
    #[test]
    fn a_game_is_found_and_says_how_many_are_playing() {
        let (mut beacon, port) = beacon("Adventurer");
        beacon.playing(2);
        let mut search = Search::new().unwrap();
        // Asked directly: broadcasting is not something a test can rely on.
        search.ask(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
        for _ in 0..50 {
            beacon.answer();
            search.listen();
            if !search.found().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let found = search.found();
        let (at, status) = found.first().expect("the game answers");
        assert_eq!(at.port(), port, "joined on the game's own port");
        assert_eq!((status.players, status.most), (2, MAX_PLAYERS as u8));
        assert_eq!(status.name, "Adventurer");
        assert!(status.joinable(), "two of four is room for more");
    }

    /// Whatever else is on the port, the game goes on and the list stays clean.
    #[test]
    fn nonsense_on_the_port_is_ignored() {
        let (mut beacon, port) = beacon("Adventurer");
        let noise = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let at = SocketAddr::from((Ipv4Addr::LOCALHOST, beside(port).unwrap()));
        noise.send_to(b"hello?", at).unwrap();
        noise.send_to(&[0xFF; 400], at).unwrap();
        beacon.answer();

        let mut search = Search::new().unwrap();
        let answering = search.socket.local_addr().unwrap().port();
        let to = SocketAddr::from((Ipv4Addr::LOCALHOST, answering));
        noise.send_to(b"dark-wagon!1 and then rubbish", to).unwrap();
        noise.send_to(ANSWERED, to).unwrap();
        search.listen();
        assert!(search.found().is_empty(), "no game said any of that");
    }

    /// A name from the network is cut to what a menu can show, on one line.
    #[test]
    fn a_shouted_name_is_cut_down() {
        let long = format!("A{}\nB", "a".repeat(200));
        assert_eq!(tidy(&long).chars().count(), NAME_LETTERS);
        assert!(!tidy(&long).contains('\n'), "one line");
        // A name too long to say is cut down, not a beacon nobody can open.
        let (_, port) = beacon(&"x".repeat(1000));
        assert!(port > 0);
    }

    #[test]
    fn a_full_game_and_one_of_another_version_are_not_joined() {
        let full = Status {
            version: PROTOCOL_VERSION,
            name: "Full".into(),
            players: 4,
            most: 4,
            port: 7777,
        };
        assert!(!full.joinable(), "4/4 is full");
        assert!(full.understood(), "but it is this game");
        let older = Status {
            version: PROTOCOL_VERSION.wrapping_sub(1),
            players: 1,
            ..full.clone()
        };
        assert!(!older.joinable(), "another version cannot be played with");
        assert!(!older.understood(), "and the player is told which it is");
    }

    /// The last port has nothing beside it to be asked on, and says so.
    #[test]
    fn a_game_on_the_last_port_cannot_be_asked_about() {
        assert!(Beacon::new(u16::MAX, "Edge".into()).is_err());
    }
}
