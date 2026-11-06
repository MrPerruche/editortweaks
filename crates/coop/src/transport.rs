//! Moving frames between peers. The mod implements [`Transport`] over Steam messages (and a
//! localhost stand-in for two game instances on one PC, `testing.md`); [`MemoryNet`] connects
//! sessions in one process, for tests.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::message::PeerId;
use crate::session::{Outgoing, Session};

pub trait Transport {
    fn send(&mut self, to: PeerId, bytes: &[u8], reliable: bool);
    /// Everything that arrived since the last call, with who sent it.
    fn receive(&mut self) -> Vec<(PeerId, Vec<u8>)>;
    /// Bytes queued for `to` and not yet delivered, if the transport can tell. Lets bulk data
    /// wait so edits don't queue behind it.
    fn backlog(&mut self, _to: PeerId) -> Option<usize> {
        None
    }
}

/// Sends what the session wants sent, then feeds it what arrived.
pub fn pump(session: &mut Session, transport: &mut impl Transport) {
    for Outgoing {
        to,
        bytes,
        reliable,
    } in session.take_outgoing()
    {
        transport.send(to, &bytes, reliable);
    }
    for (from, bytes) in transport.receive() {
        session.receive(from, &bytes);
    }
}

/// Frames in flight, one queue per (from, to) pair: like Steam's reliable messages, each pair's
/// messages arrive in order, but nothing is ordered across pairs.
#[derive(Default)]
pub struct MemoryNet {
    queues: HashMap<(PeerId, PeerId), VecDeque<Vec<u8>>>,
    /// Delivered frames waiting for their receiver's `receive`.
    inboxes: HashMap<PeerId, Vec<(PeerId, Vec<u8>)>>,
}

impl MemoryNet {
    pub fn new() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

    pub fn endpoint(net: &Rc<RefCell<Self>>, peer: PeerId) -> MemoryEndpoint {
        MemoryEndpoint {
            net: Rc::clone(net),
            peer,
        }
    }

    /// The pairs that have frames in flight, sorted so tests are repeatable.
    pub fn busy_pairs(&self) -> Vec<(PeerId, PeerId)> {
        let mut pairs: Vec<_> = self
            .queues
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .map(|(&k, _)| k)
            .collect();
        pairs.sort_unstable();
        pairs
    }

    /// Moves the oldest frame from `from` to `to` into `to`'s inbox.
    pub fn deliver_one(&mut self, (from, to): (PeerId, PeerId)) {
        if let Some(bytes) = self
            .queues
            .get_mut(&(from, to))
            .and_then(VecDeque::pop_front)
        {
            self.inboxes.entry(to).or_default().push((from, bytes));
        }
    }

    pub fn deliver_all(&mut self) {
        while let Some(&pair) = self.busy_pairs().first() {
            self.deliver_one(pair);
        }
    }
}

pub struct MemoryEndpoint {
    net: Rc<RefCell<MemoryNet>>,
    peer: PeerId,
}

impl Transport for MemoryEndpoint {
    /// Unreliable frames are delivered too: tests that want losses drop them themselves.
    fn send(&mut self, to: PeerId, bytes: &[u8], _reliable: bool) {
        self.net
            .borrow_mut()
            .queues
            .entry((self.peer, to))
            .or_default()
            .push_back(bytes.to_vec());
    }

    fn receive(&mut self) -> Vec<(PeerId, Vec<u8>)> {
        self.net
            .borrow_mut()
            .inboxes
            .remove(&self.peer)
            .unwrap_or_default()
    }
}
