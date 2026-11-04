//! Coop over Steam's messages, between Steam friends playing Brick Rigs and players of the same
//! match. The subhost opens to both; a joiner says hello to each, and lists those answering as
//! hosting. The subhost's player approves each one.
//!
//! Steam only relays (`Steam::force_relay`, before anything is sent), so no one learns anyone's
//! address. Sessions are only accepted from friends playing the game, players of our match and
//! the session's peers: other strangers' messages never arrive.

use std::time::{Duration, Instant};

use ::coop::PeerId;
use ::coop::transport::Transport;

use super::log::coop_log;
use crate::game::steam::{Friend, Steam};

/// Our channel of Steam's messages ("ET"). The game itself talks over SteamSockets, not messages.
pub const CHANNEL: i32 = 0x4554;
/// How often the list of friends playing is read again.
const FRIENDS_EVERY: Duration = Duration::from_secs(10);
/// How often the connections to the session's peers are checked.
const LOST_EVERY: Duration = Duration::from_secs(1);
/// `ESteamNetworkingConnectionState`: closed by the peer, and a problem detected locally (it timed
/// out).
const CLOSED_BY_PEER: i32 = 4;
const PROBLEM: i32 = 5;

/// A session's peer whose connection ended.
pub struct Closed {
    pub peer: PeerId,
    /// It ended it on purpose (its game closed, say), rather than the connection failing.
    pub by_peer: bool,
}

/// Steam's messages as a coop transport.
pub struct SteamTransport {
    pub steam: Steam,
    app_id: u32,
    /// The session's peers (our subhost, or our members).
    expected: Vec<PeerId>,
    /// Whether friends playing may open sessions (to say hello, ask to join).
    open_to_friends: bool,
    friends: Vec<Friend>,
    friends_at: Option<Instant>,
    /// The other players of our match (SteamID, name), read every few seconds.
    server: Vec<(PeerId, String)>,
    server_at: Option<Instant>,
    own_id: PeerId,
    lost_at: Instant,
    /// Peers reported lost, so they're reported once.
    lost: Vec<PeerId>,
}

impl SteamTransport {
    /// Steam's messages, relayed only; `Err` if Steam is missing or won't relay only, and then
    /// nothing may be sent.
    pub fn new(open_to_friends: bool) -> Result<Self, String> {
        let steam = Steam::get()?;
        // Before anything is sent: a peer must never learn our IP address.
        if !steam.force_relay() {
            return Err("Steam refused relay-only mode, so an address could leak".into());
        }
        Ok(Self {
            steam,
            app_id: steam.app_id(),
            expected: Vec::new(),
            open_to_friends,
            friends: Vec::new(),
            friends_at: None,
            server: Vec::new(),
            server_at: None,
            own_id: steam.own_id(),
            lost_at: Instant::now(),
            lost: Vec::new(),
        })
    }

    /// The session's peers from now on (a peer again expected may be reported lost again).
    pub fn expect(&mut self, peers: Vec<PeerId>) {
        self.lost.retain(|peer| !peers.contains(peer));
        self.expected = peers;
    }

    /// Friends playing Brick Rigs now (read again every few seconds).
    pub fn friends(&mut self) -> &[Friend] {
        if self
            .friends_at
            .is_none_or(|at| at.elapsed() > FRIENDS_EVERY)
        {
            self.friends = self.steam.friends_playing(self.app_id);
            self.friends_at = Some(Instant::now());
        }
        &self.friends
    }

    /// The other players of our match (SteamID, name): `read` gives them all, called again every
    /// few seconds.
    pub fn server(&mut self, read: impl FnOnce() -> Vec<(PeerId, String)>) -> &[(PeerId, String)] {
        if self.server_at.is_none_or(|at| at.elapsed() > FRIENDS_EVERY) {
            let own = self.own_id;
            self.server = read().into_iter().filter(|(id, _)| *id != own).collect();
            self.server_at = Some(Instant::now());
        }
        &self.server
    }

    /// Whether `peer` is a friend playing Brick Rigs.
    pub fn is_friend(&mut self, peer: PeerId) -> bool {
        self.friends().iter().any(|f| f.id == peer)
    }

    /// What to call `peer`: Steam's name for a friend playing, else the name it has in our match,
    /// if it's in it.
    pub fn name_of(&mut self, peer: PeerId) -> Option<String> {
        if let Some(friend) = self.friends().iter().find(|f| f.id == peer) {
            return Some(friend.name.clone());
        }
        self.server
            .iter()
            .find(|(id, name)| *id == peer && !name.is_empty())
            .map(|(_, name)| name.clone())
    }

    /// The session's peers whose connection Steam closed since the last call (checked every
    /// second).
    pub fn take_lost(&mut self) -> Vec<Closed> {
        if self.lost_at.elapsed() < LOST_EVERY {
            return Vec::new();
        }
        self.lost_at = Instant::now();
        let mut newly = Vec::new();
        for &peer in &self.expected {
            let Some(state) = self.steam.peer_connection(peer).map(|c| c.state) else {
                continue;
            };
            let closed = state == CLOSED_BY_PEER || state == PROBLEM;
            if closed && !self.lost.contains(&peer) {
                self.lost.push(peer);
                newly.push(Closed {
                    peer,
                    by_peer: state == CLOSED_BY_PEER,
                });
            }
        }
        newly
    }

    /// Logs how Steam reaches `peer`, to see in game that the traffic is relayed.
    pub fn log_connection(&self, peer: PeerId) {
        match self.steam.peer_connection(peer) {
            Some(connection) => coop_log!(
                "Steam connection to {peer}: state {}, relayed {}, relay {}, flags {:#x}",
                connection.state,
                connection.relayed,
                connection.relay_pop.as_deref().unwrap_or("none"),
                connection.flags
            ),
            None => coop_log!("Steam has no connection to {peer}"),
        }
    }

    fn allowed(&self, peer: PeerId) -> bool {
        self.expected.contains(&peer)
            || (self.open_to_friends
                && (self.friends.iter().any(|f| f.id == peer)
                    || self.server.iter().any(|(id, _)| *id == peer)))
    }
}

impl Transport for SteamTransport {
    fn send(&mut self, to: PeerId, bytes: &[u8], reliable: bool) {
        if !self.steam.send(to, CHANNEL, bytes, reliable) {
            coop_log!("Steam refused a message to {to}");
        }
    }

    fn receive(&mut self) -> Vec<(PeerId, Vec<u8>)> {
        if self.open_to_friends {
            self.friends();
        }
        let mut peers = self.expected.clone();
        if self.open_to_friends {
            peers.extend(self.friends.iter().map(|f| f.id));
            peers.extend(self.server.iter().map(|(id, _)| *id));
        }
        for peer in peers {
            self.steam.accept(peer);
        }
        // Steam only delivers from sessions we accepted or opened; checking the sender anyway
        // keeps that from depending on Steam's behaviour.
        let mut messages = self.steam.receive(CHANNEL);
        messages.retain(|(sender, _)| self.allowed(*sender));
        messages
    }
}
