//! The local test: two game instances on one PC, linked over localhost instead of Steam (both are
//! signed in to the same account, so Steam can't tell them apart; `docs/re/coop/testing.md`).
//! The one hosting listens; the other connects and joins.

use std::io;
use std::time::Duration;

use ::coop::PeerId;
use ::coop::tcp::TcpTransport;

/// The localhost port: "ET", like the Steam channel.
const PORT: u16 = 0x4554;
/// The listening instance's peer id.
pub const HOST: PeerId = 1;
/// What joining asks for; the subhost checks it.
pub const CODE: &str = "local";
/// A listener on localhost answers within a millisecond; Windows takes two seconds to give up on
/// a port nobody listens on.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Member,
}

impl Role {
    pub fn name(&self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Member => "member",
        }
    }
}

/// Our peer id as a member: unique among the instances on this PC.
fn member_id() -> PeerId {
    1000 + PeerId::from(std::process::id())
}

/// Connects to the instance hosting.
pub fn connect() -> io::Result<TcpTransport> {
    TcpTransport::connect(member_id(), PORT, HOST, CONNECT_TIMEOUT)
}

/// Listens, to host.
pub fn listen() -> io::Result<TcpTransport> {
    TcpTransport::listen(HOST, PORT)
}
