//! A localhost stand-in for Steam's messages, so two game instances on one PC can test coop
//! (`testing.md`): both are signed in to the same Steam account, so Steam can't tell them apart.
//!
//! One instance listens, the others connect to it: a star around the subhost, which is all the
//! session needs (members only ever talk to the subhost). Each connection carries frames in
//! order, like Steam's reliable messages; unreliable frames take the same way. Everything is
//! non-blocking and polled, from the game thread.
//!
//! On the wire: the connecting side first sends [`HANDSHAKE`] and its peer id (8 bytes, little
//! endian), then both sides send frames as a `u32` length (little endian) and the bytes. The
//! listener's peer id is agreed beforehand ([`TcpTransport::connect`] takes it), so connecting
//! never waits for the listener to answer.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use crate::framing::MAX_FRAME;
use crate::message::PeerId;
use crate::transport::Transport;

/// What a connection starts with: not a coop frame (those start with `ET`), so nothing else
/// listening on the port mistakes it for one, and we notice anything else that connects.
pub const HANDSHAKE: [u8; 4] = *b"ETtc";
/// Bigger frames end the connection: the session never makes them.
const MAX_WIRE_FRAME: usize = MAX_FRAME + 1024;
/// Data a connection may hold back before it's dropped, when the other side stops reading (e.g.
/// its game froze): a few snapshots' worth.
const MAX_BACKLOG: usize = 64 << 20;

pub struct TcpTransport {
    own: PeerId,
    listener: Option<TcpListener>,
    connections: Vec<Connection>,
    /// Peers whose connection closed since the last [`TcpTransport::take_lost`].
    lost: Vec<PeerId>,
}

struct Connection {
    stream: TcpStream,
    /// Known once its handshake arrived (at once for connections we opened).
    peer: Option<PeerId>,
    /// Bytes read but not yet a whole frame.
    received: Vec<u8>,
    /// Bytes the socket didn't take yet.
    unsent: Vec<u8>,
}

impl TcpTransport {
    /// Listens on localhost `port` (0: any free port, see [`TcpTransport::port`]) as `own`.
    pub fn listen(own: PeerId, port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            own,
            listener: Some(listener),
            connections: Vec::new(),
            lost: Vec::new(),
        })
    }

    /// Connects to the instance listening on localhost `port`, which is `host`. If nothing listens
    /// there it fails after `timeout` (Windows retries refused connections for about two seconds,
    /// so keep it short: a listener answers within a millisecond).
    pub fn connect(own: PeerId, port: u16, host: PeerId, timeout: Duration) -> io::Result<Self> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        let mut connection = Connection::new(stream)?;
        connection.peer = Some(host);
        connection.unsent.extend_from_slice(&HANDSHAKE);
        connection.unsent.extend_from_slice(&own.to_le_bytes());
        connection.flush()?;
        Ok(Self {
            own,
            listener: None,
            connections: vec![connection],
            lost: Vec::new(),
        })
    }

    pub fn own_id(&self) -> PeerId {
        self.own
    }

    /// The port we listen on, if we do.
    pub fn port(&self) -> Option<u16> {
        let address = self.listener.as_ref()?.local_addr().ok()?;
        Some(address.port())
    }

    pub fn is_connected(&self, peer: PeerId) -> bool {
        self.connections.iter().any(|c| c.peer == Some(peer))
    }

    /// Peers whose connection closed since the last call.
    pub fn take_lost(&mut self) -> Vec<PeerId> {
        std::mem::take(&mut self.lost)
    }

    fn accept(&mut self) {
        let Some(listener) = &self.listener else {
            return;
        };
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Ok(connection) = Connection::new(stream) {
                        self.connections.push(connection);
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                // `WouldBlock`: nobody else is waiting. Anything else: try again next time.
                Err(_) => return,
            }
        }
    }
}

impl Connection {
    fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        // Small frames (selections, pings) shouldn't wait for more to fill a packet.
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            peer: None,
            received: Vec::new(),
            unsent: Vec::new(),
        })
    }

    /// Writes what the socket takes now.
    fn flush(&mut self) -> io::Result<()> {
        while !self.unsent.is_empty() {
            match self.stream.write(&self.unsent) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(written) => {
                    self.unsent.drain(..written);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        if self.unsent.len() > MAX_BACKLOG {
            return Err(io::Error::other("the other side stopped reading"));
        }
        Ok(())
    }

    /// Reads what arrived and appends the whole frames to `out`. An error closes the connection.
    fn read(&mut self, out: &mut Vec<(PeerId, Vec<u8>)>) -> io::Result<()> {
        let mut buffer = [0; 64 * 1024];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(read) => self.received.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        let mut start = 0;
        if self.peer.is_none() {
            let Some(handshake) = self.received.get(..12) else {
                return Ok(());
            };
            if handshake[..4] != HANDSHAKE {
                return Err(io::Error::other("not a coop connection"));
            }
            let id = PeerId::from_le_bytes(handshake[4..12].try_into().expect("8 bytes"));
            self.peer = Some(id);
            start = 12;
        }
        let peer = self.peer.expect("set above");
        while let Some(header) = self.received.get(start..start + 4) {
            let len = u32::from_le_bytes(header.try_into().expect("4 bytes")) as usize;
            if len > MAX_WIRE_FRAME {
                return Err(io::Error::other("frame too long"));
            }
            let Some(frame) = self.received.get(start + 4..start + 4 + len) else {
                break;
            };
            out.push((peer, frame.to_vec()));
            start += 4 + len;
        }
        self.received.drain(..start);
        Ok(())
    }
}

impl Transport for TcpTransport {
    /// Frames for peers we have no connection to are dropped, as Steam would fail to send them.
    fn send(&mut self, to: PeerId, bytes: &[u8], _reliable: bool) {
        let Ok(len) = u32::try_from(bytes.len()) else {
            return;
        };
        let Some(index) = self.connections.iter().position(|c| c.peer == Some(to)) else {
            return;
        };
        let connection = &mut self.connections[index];
        connection.unsent.extend_from_slice(&len.to_le_bytes());
        connection.unsent.extend_from_slice(bytes);
        if connection.flush().is_err() {
            self.connections.swap_remove(index);
            self.lost.push(to);
        }
    }

    fn receive(&mut self) -> Vec<(PeerId, Vec<u8>)> {
        self.accept();
        let mut received = Vec::new();
        let lost = &mut self.lost;
        self.connections.retain_mut(|connection| {
            let alive = connection.flush().is_ok() && connection.read(&mut received).is_ok();
            if !alive && let Some(peer) = connection.peer {
                lost.push(peer);
            }
            alive
        });
        received
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: PeerId = 1;
    const WAIT: Duration = Duration::from_secs(2);

    /// Polls until `wanted` frames arrived (localhost is quick, but not instant).
    fn receive_n(transport: &mut TcpTransport, wanted: usize) -> Vec<(PeerId, Vec<u8>)> {
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        while got.len() < wanted && start.elapsed() < WAIT {
            got.extend(transport.receive());
            std::thread::sleep(Duration::from_millis(1));
        }
        got
    }

    #[test]
    fn frames_both_ways_in_order() {
        let mut host = TcpTransport::listen(HOST, 0).unwrap();
        let port = host.port().unwrap();
        let mut a = TcpTransport::connect(10, port, HOST, WAIT).unwrap();
        let mut b = TcpTransport::connect(11, port, HOST, WAIT).unwrap();
        a.send(HOST, b"first", true);
        a.send(HOST, b"", true);
        let big = vec![7; MAX_FRAME];
        a.send(HOST, &big, true);
        b.send(HOST, b"from b", false);
        let got = receive_n(&mut host, 4);
        let from_a: Vec<_> = got
            .iter()
            .filter(|(p, _)| *p == 10)
            .map(|(_, f)| f)
            .collect();
        assert_eq!(from_a, [&b"first".to_vec(), &Vec::new(), &big]);
        assert!(got.contains(&(11, b"from b".to_vec())));
        assert!(host.is_connected(10) && host.is_connected(11));

        host.send(11, b"to b", true);
        host.send(12, b"nobody", true);
        assert_eq!(receive_n(&mut b, 1), [(HOST, b"to b".to_vec())]);
        assert!(a.receive().is_empty());
    }

    #[test]
    fn closed_connections_are_reported() {
        let mut host = TcpTransport::listen(HOST, 0).unwrap();
        let mut a = TcpTransport::connect(10, host.port().unwrap(), HOST, WAIT).unwrap();
        a.send(HOST, b"hi", true);
        assert_eq!(receive_n(&mut host, 1).len(), 1);
        drop(a);
        let start = std::time::Instant::now();
        while host.is_connected(10) && start.elapsed() < WAIT {
            host.receive();
        }
        assert!(!host.is_connected(10));
        assert_eq!(host.take_lost(), [10]);
    }

    #[test]
    fn strangers_are_dropped() {
        let mut host = TcpTransport::listen(HOST, 0).unwrap();
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, host.port().unwrap()));
        let mut stranger = TcpStream::connect(address).unwrap();
        stranger.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            assert!(host.receive().is_empty());
        }
        assert!(host.connections.is_empty());
    }

    /// Windows retries a refused localhost connection for about two seconds, so callers pass a
    /// short timeout: a listener there answers within a millisecond.
    #[test]
    fn nothing_listening_fails_within_the_timeout() {
        // Bind and drop to find a port that's (very probably) free.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let start = std::time::Instant::now();
        assert!(TcpTransport::connect(10, port, HOST, Duration::from_millis(200)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
