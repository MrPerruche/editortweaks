//! The session: who's in it, and the order everyone applies edits in. Pure logic: it takes what
//! peers sent and what the local player did, and gives back frames to send ([`Session::take_outgoing`])
//! and things for the editor to do ([`Session::take_events`]). The mod and the tests feed it
//! the same way.
//!
//! The subhost puts every edit in one order and numbers it (a commit); members apply commits in
//! that order. A member's own edits show at once and go to the subhost as proposals; while one is
//! pending, remote commits that it will overwrite are skipped (see [`Op::overridden_by`]), so
//! every member ends up where the subhost is. Each commit carries a [`Check`] of the bricks after
//! it; a member whose own count of them differs asks for a snapshot.
//!
//! Each member can undo and redo its own edits (`undo`): the session keeps them with what undoes
//! them, and tells them which of their changes someone else overwrote since.
//!
//! Every few seconds the subhost describes its editor in short (`audit`); a member at that commit
//! with nothing of its own on the way compares its editor, and asks for a snapshot (or only the
//! subhost's object order) if it differs.
//!
//! Peers are untrusted: what each may send is limited (`limits`), on a clock the mod gives
//! ([`Session::tick`]).

use std::collections::{HashMap, VecDeque};

use crate::audit::{Audit, Digest};
use crate::framing::{self, Assembler};
use crate::ids::{CoopId, IdAllocator, LiveSet, ORIGINAL_SLOT, SUBHOST_SLOT, Slot};
use crate::limits::{Flood, Limits};
use crate::message::{Member, Message, PeerId, PeerInfo, RefuseReason};
use crate::ops::{Op, Value};
use crate::undo::{Change, History, StepBack};
use crate::wire::DecodeError;

/// A frame for the transport to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub to: PeerId,
    pub bytes: Vec<u8>,
    pub reliable: bool,
}

/// Why the session ended for us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// We left, or closed the session we hosted.
    Left,
    Kicked,
    /// The subhost closed the session.
    Closed,
}

/// What the mod should do in the editor, or show.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A peer answered or sent a hello.
    Hello {
        peer: PeerId,
        info: PeerInfo,
    },
    /// We're in, as `slot`; the vehicle follows as [`Event::Load`].
    Joined {
        slot: Slot,
    },
    Refused(RefuseReason),
    /// Replace the whole vehicle with `vehicle`; `ids` are its bricks' coop ids in object order.
    /// Own edits not yet committed are lost from the editor here; they come back with their
    /// commits.
    Load {
        ids: Vec<CoopId>,
        vehicle: Vec<u8>,
    },
    /// Apply someone else's edit.
    Apply {
        author: Slot,
        ops: Vec<Op>,
    },
    /// Subhost only: save the vehicle and pass it to [`Session::send_snapshot`] with this
    /// request, once the editor is up to date: see there.
    SnapshotWanted(SnapshotRequest),
    Members(Vec<Member>),
    Selection {
        slot: Slot,
        ids: Vec<CoopId>,
    },
    /// The subhost's editor in short: compare ours (see `audit`). We're at the same commit with
    /// nothing of ours on the way. If a brick differs, [`Session::request_resync`]; if only the
    /// object order, [`Session::request_order`].
    Audit(Audit),
    /// Subhost only: send this member the editor's object order ([`Session::send_order`]), once the
    /// editor is up to date, as for a snapshot.
    OrderWanted(PeerId),
    /// Put the bricks in this object order (the subhost's, as of our last commit; the same
    /// bricks as ours): not an edit of ours.
    Reorder(Vec<CoopId>),
    /// Our bricks differed from the subhost's; a snapshot was asked for.
    Drifted,
    /// Someone else's edit and ours still on their way both touched a brick one of them makes
    /// again (undoing a delete): a snapshot was asked for.
    UndoConflict,
    Ended(EndReason),
    /// Subhost: a member sent far too much (`limits`) and was kicked; it can't join again.
    Flooded {
        peer: PeerId,
        what: Flood,
    },
    /// A peer sent something we couldn't read (logged by the mod, otherwise ignored).
    BadMessage {
        peer: PeerId,
        error: DecodeError,
    },
}

/// Which snapshot a member waits for. A member can leave and join again (or ask twice) before
/// an older request is answered; a snapshot only counts for the request it was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRequest {
    pub peer: PeerId,
    slot: Slot,
    commit: u32,
}

/// Misuse of the session's API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// Hosting or joining while already in a session.
    Busy,
    NotInSession,
    NotSubhost,
    /// Editing before the vehicle arrived.
    NotLoaded,
    /// More bricks than the archive can count, or the slot ran out of ids.
    TooManyBricks,
}

#[derive(Debug, Clone)]
pub struct HostSettings {
    /// What joiners must type.
    pub code: String,
    /// Members besides the subhost.
    pub max_members: usize,
}

struct HostedMember {
    member: Member,
    /// While it waits for a snapshot: the commit it was asked at, which tells this request from
    /// older ones. It gets no commits meanwhile: the snapshot will hold them.
    waiting: Option<u32>,
}

impl HostedMember {
    fn ready(&self) -> bool {
        self.waiting.is_none()
    }

    /// Waits for a snapshot asked for at `commit`; `None` if it was waiting already.
    fn wait(&mut self, commit: u32) -> Option<SnapshotRequest> {
        if self.waiting.is_some() {
            return None;
        }
        self.waiting = Some(commit);
        Some(SnapshotRequest {
            peer: self.member.peer,
            slot: self.member.slot,
            commit,
        })
    }
}

struct Host {
    settings: HostSettings,
    members: Vec<HostedMember>,
    /// Slots aren't reused within a session, so an old member's ids never collide with a new one's.
    next_slot: u16,
    commit: u32,
    ids: IdAllocator,
    live: LiveSet,
    /// Bricks that existed and were deleted, with who deleted them: only they may make them again
    /// (undoing the delete).
    retired: HashMap<CoopId, Slot>,
    name: String,
    history: History,
    /// Snapshots past a member's allowance, with when they may go.
    deferred: Vec<(u64, SnapshotRequest)>,
}

struct Pending {
    local: u32,
    ops: Vec<Op>,
}

struct Guest {
    host: PeerId,
    slot: Slot,
    ids: IdAllocator,
    next_local: u32,
    /// Own proposals not committed yet, oldest first.
    pending: VecDeque<Pending>,
    /// The bricks as of the last commit, which the commit's check must match.
    confirmed: LiveSet,
    loaded: bool,
    resync_asked: bool,
    order_asked: bool,
    /// The last commit applied (or folded into the snapshot loaded).
    commit: u32,
    members: Vec<Member>,
    history: History,
}

impl Guest {
    /// Whether the editor holds exactly the subhost's vehicle as of commit `commit`, as far as
    /// the session knows: nothing of ours on the way, no snapshot coming.
    fn settled_at(&self, commit: u32) -> bool {
        self.loaded && !self.resync_asked && self.pending.is_empty() && self.commit == commit
    }

    /// The bricks as the editor has them: the last commit's, with our pending edits.
    fn view(&self) -> LiveSet {
        let mut view = self.confirmed.clone();
        for op in self.pending.iter().flat_map(|p| &p.ops) {
            op.apply_to(&mut view);
        }
        view
    }
}

enum Role {
    Idle,
    Joining { host: PeerId },
    Host(Host),
    Guest(Guest),
}

/// Frames waiting for the transport. Its own field, so sending works while the role is borrowed.
#[derive(Default)]
struct Outbox {
    session: u32,
    frames: Vec<Outgoing>,
}

impl Outbox {
    fn send(&mut self, to: PeerId, message: &Message) {
        let reliable = message.reliable();
        for bytes in framing::frames(self.session, message) {
            self.frames.push(Outgoing {
                to,
                bytes,
                reliable,
            });
        }
    }
}

pub struct Session {
    info: PeerInfo,
    role: Role,
    assembler: Assembler,
    out: Outbox,
    events: Vec<Event>,
    /// Milliseconds, from [`Session::tick`].
    now: u64,
    limits: Limits,
}

impl Session {
    /// `info` is what we tell peers in hellos (`hosting` is filled in).
    pub fn new(info: PeerInfo) -> Self {
        Self {
            info,
            role: Role::Idle,
            assembler: Assembler::default(),
            out: Outbox::default(),
            events: Vec::new(),
            now: 0,
            limits: Limits::default(),
        }
    }

    /// The time in milliseconds, from any start: the mod calls this every frame. Sends the
    /// snapshots that waited for a member's allowance ([`Event::SnapshotWanted`]).
    pub fn tick(&mut self, now: u64) {
        self.now = self.now.max(now);
        if let Role::Host(host) = &mut self.role {
            let now = self.now;
            let (due, later): (Vec<_>, Vec<_>) =
                host.deferred.drain(..).partition(|(at, _)| *at <= now);
            host.deferred = later;
            self.events
                .extend(due.into_iter().map(|(_, r)| Event::SnapshotWanted(r)));
        }
    }

    // --- State ---

    pub fn is_idle(&self) -> bool {
        matches!(self.role, Role::Idle)
    }

    pub fn is_subhost(&self) -> bool {
        matches!(self.role, Role::Host(_))
    }

    /// Whether we have the vehicle and can edit.
    pub fn is_loaded(&self) -> bool {
        match &self.role {
            Role::Host(_) => true,
            Role::Guest(guest) => guest.loaded,
            Role::Idle | Role::Joining { .. } => false,
        }
    }

    pub fn slot(&self) -> Option<Slot> {
        match &self.role {
            Role::Host(_) => Some(SUBHOST_SLOT),
            Role::Guest(guest) => Some(guest.slot),
            Role::Idle | Role::Joining { .. } => None,
        }
    }

    /// Everyone in the session, subhost first.
    pub fn members(&self) -> Vec<Member> {
        match &self.role {
            Role::Host(host) => host_member_list(host, 0),
            Role::Guest(guest) => guest.members.clone(),
            Role::Idle | Role::Joining { .. } => Vec::new(),
        }
    }

    /// Peers we expect messages from (Steam needs each accepted).
    pub fn expected_peers(&self) -> Vec<PeerId> {
        match &self.role {
            Role::Host(host) => host.members.iter().map(|m| m.member.peer).collect(),
            Role::Guest(Guest { host, .. }) | Role::Joining { host } => vec![*host],
            Role::Idle => Vec::new(),
        }
    }

    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.out.frames)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    // --- Starting and ending ---

    /// Starts hosting the vehicle in the editor, which has `bricks` bricks. Returns their coop
    /// ids in object order. `session` should be random, so leftovers from another session drop.
    pub fn host(
        &mut self,
        session: u32,
        name: String,
        settings: HostSettings,
        bricks: usize,
    ) -> Result<Vec<CoopId>, SessionError> {
        if !self.is_idle() {
            return Err(SessionError::Busy);
        }
        if bricks > crate::message::caps::SNAPSHOT_IDS {
            return Err(SessionError::TooManyBricks);
        }
        let mut original = IdAllocator::new(ORIGINAL_SLOT);
        let ids: Vec<CoopId> = (0..bricks)
            .map(|_| original.next_id().expect("under the cap"))
            .collect();
        self.out.session = session.max(1);
        self.limits = Limits::default();
        self.role = Role::Host(Host {
            settings,
            members: Vec::new(),
            next_slot: u16::from(SUBHOST_SLOT) + 1,
            commit: 0,
            ids: IdAllocator::new(SUBHOST_SLOT),
            live: LiveSet::from_ids(ids.iter().copied()),
            retired: HashMap::new(),
            name,
            history: History::default(),
            deferred: Vec::new(),
        });
        Ok(ids)
    }

    /// Asks `host` to let us in. Answered by [`Event::Joined`] or [`Event::Refused`].
    pub fn join(&mut self, host: PeerId, code: String, name: String) -> Result<(), SessionError> {
        if !self.is_idle() {
            return Err(SessionError::Busy);
        }
        self.out.session = 0;
        self.role = Role::Joining { host };
        let game_version = self.info.game_version.clone();
        self.out.send(
            host,
            &Message::Join {
                code,
                name,
                game_version,
            },
        );
        Ok(())
    }

    /// Leaves the session, or closes it if we host it.
    pub fn leave(&mut self) {
        match std::mem::replace(&mut self.role, Role::Idle) {
            Role::Idle => return,
            Role::Joining { host } | Role::Guest(Guest { host, .. }) => {
                self.out.send(host, &Message::Leave)
            }
            Role::Host(host) => {
                for m in &host.members {
                    self.out.send(m.member.peer, &Message::Close);
                }
            }
        }
        self.assembler = Assembler::default();
        self.events.push(Event::Ended(EndReason::Left));
    }

    /// Subhost: removes a member, which can't join again.
    pub fn kick(&mut self, peer: PeerId) -> Result<(), SessionError> {
        self.remove(peer, true)
    }

    /// Subhost: the connection to a member was lost. It may join again.
    pub fn lost(&mut self, peer: PeerId) -> Result<(), SessionError> {
        self.remove(peer, false)
    }

    fn remove(&mut self, peer: PeerId, kick: bool) -> Result<(), SessionError> {
        let Role::Host(host) = &mut self.role else {
            return Err(SessionError::NotSubhost);
        };
        if kick {
            self.limits.banned.insert(peer);
        }
        let before = host.members.len();
        host.members.retain(|m| m.member.peer != peer);
        if host.members.len() != before {
            if kick {
                self.out.send(peer, &Message::Kick);
            }
            self.assembler.forget(peer);
            self.broadcast_members();
        }
        Ok(())
    }

    pub fn hello(&mut self, peer: PeerId) {
        let info = self.own_info();
        self.out.send(peer, &Message::Hello(info));
    }

    // --- Local actions ---

    /// Coop ids for `n` bricks the local player is creating.
    pub fn new_ids(&mut self, n: usize) -> Result<Vec<CoopId>, SessionError> {
        let ids = match &mut self.role {
            Role::Host(host) => &mut host.ids,
            Role::Guest(guest) => &mut guest.ids,
            Role::Idle | Role::Joining { .. } => return Err(SessionError::NotInSession),
        };
        (0..n)
            .map(|_| ids.next_id().ok_or(SessionError::TooManyBricks))
            .collect()
    }

    /// An edit the local player finished (already applied in their editor), which they can't
    /// undo.
    pub fn edit(&mut self, ops: Vec<Op>) -> Result<(), SessionError> {
        self.send_edit(ops, |history, _| history.forget_redo())
    }

    /// An edit the local player finished (already applied in their editor): its changes, each
    /// with what undoes it ([`Change`]).
    pub fn edit_undoable(&mut self, changes: Vec<Change>) -> Result<(), SessionError> {
        let ops = changes
            .iter()
            .flat_map(|c| c.done.iter().cloned())
            .collect();
        self.send_edit(ops, |history, local| history.record(changes, local))
    }

    /// Undoes the local player's newest step still to undo, as a new edit: what it makes, for the
    /// editor to make too. `None` if there's nothing to undo.
    pub fn undo(&mut self) -> Result<Option<StepBack>, SessionError> {
        self.step_back(false)
    }

    /// Redoes the step undone last, as [`Session::undo`] does.
    pub fn redo(&mut self) -> Result<Option<StepBack>, SessionError> {
        self.step_back(true)
    }

    fn step_back(&mut self, redo: bool) -> Result<Option<StepBack>, SessionError> {
        let (history, view) = match &mut self.role {
            Role::Host(host) => (&mut host.history, host.live.clone()),
            Role::Guest(guest) if guest.loaded => {
                let view = guest.view();
                (&mut guest.history, view)
            }
            Role::Guest(_) => return Err(SessionError::NotLoaded),
            Role::Idle | Role::Joining { .. } => return Err(SessionError::NotInSession),
        };
        let Some((back, other_way)) = history.take(redo, view) else {
            return Ok(None);
        };
        if back.ops.is_empty() {
            // Nothing to send (only skipped changes, or ones only the other way does something
            // for), so nothing waits for a commit either.
            history.push(!redo, other_way, 0);
            return Ok(Some(back));
        }
        self.send_edit(back.ops.clone(), |history, local| {
            history.push(!redo, other_way, local)
        })?;
        Ok(Some(back))
    }

    /// Sends a local edit, then `record`s it in the history with the proposal number it went as.
    /// Nothing for no ops.
    fn send_edit(
        &mut self,
        ops: Vec<Op>,
        record: impl FnOnce(&mut History, u32),
    ) -> Result<(), SessionError> {
        if ops.is_empty() {
            return Ok(());
        }
        match &mut self.role {
            Role::Host(_) => {
                self.commit(SUBHOST_SLOT, 0, ops);
                let Role::Host(host) = &mut self.role else {
                    unreachable!()
                };
                record(&mut host.history, 0);
                Ok(())
            }
            Role::Guest(guest) if guest.loaded => {
                guest.next_local += 1;
                let local = guest.next_local;
                let host = guest.host;
                let message = Message::Propose {
                    local,
                    ops: ops.clone(),
                };
                guest.pending.push_back(Pending { local, ops });
                record(&mut guest.history, local);
                self.out.send(host, &message);
                Ok(())
            }
            Role::Guest(_) => Err(SessionError::NotLoaded),
            Role::Idle | Role::Joining { .. } => Err(SessionError::NotInSession),
        }
    }

    /// Subhost, after [`Event::SnapshotWanted`]: the vehicle as the editor has it now, `ids` in
    /// object order. It's labelled with the latest commit, so the editor must hold exactly what
    /// was committed: every [`Event::Apply`] taken so far applied, every finished local edit
    /// passed to [`Session::edit`], and nothing of an edit still in progress (a brick still being
    /// placed). The mod answers once its editor is idle, after applying the events: any number of
    /// commits later than the request is fine.
    pub fn send_snapshot(
        &mut self,
        request: SnapshotRequest,
        ids: Vec<CoopId>,
        vehicle: Vec<u8>,
    ) -> Result<(), SessionError> {
        let Role::Host(host) = &mut self.role else {
            return Err(SessionError::NotSubhost);
        };
        let SnapshotRequest { peer, slot, commit } = request;
        let Some(member) = host
            .members
            .iter_mut()
            .find(|m| m.member.peer == peer && m.member.slot == slot)
        else {
            return Ok(()); // Left in the meantime.
        };
        if member.waiting.take_if(|asked| *asked == commit).is_none() {
            return Ok(()); // Not the request it waits for.
        }
        let commit = host.commit;
        self.out.send(
            peer,
            &Message::Snapshot {
                commit,
                ids,
                vehicle,
            },
        );
        Ok(())
    }

    /// Subhost: describes the editor to the members that have the vehicle (see `audit`): it has
    /// `count` bricks, in the order `order` ([`crate::audit::order_hash`]), and `bricks` are some
    /// of them with their digests. Labelled with the latest commit, so the editor must hold exactly
    /// what was committed, as for [`Session::send_snapshot`].
    pub fn audit(
        &mut self,
        count: u32,
        order: u32,
        mut bricks: Vec<(CoopId, Digest)>,
    ) -> Result<(), SessionError> {
        let Role::Host(host) = &self.role else {
            return Err(SessionError::NotSubhost);
        };
        bricks.truncate(crate::message::caps::AUDIT_BRICKS);
        let message = Message::Audit(Audit {
            commit: host.commit,
            count,
            order,
            bricks,
        });
        let peers: Vec<PeerId> = host
            .members
            .iter()
            .filter(|m| m.ready())
            .map(|m| m.member.peer)
            .collect();
        peers
            .into_iter()
            .for_each(|peer| self.out.send(peer, &message));
        Ok(())
    }

    /// Subhost, after [`Event::OrderWanted`]: the editor's bricks in object order, labelled with
    /// the latest commit as for [`Session::send_snapshot`].
    pub fn send_order(&mut self, peer: PeerId, ids: Vec<CoopId>) -> Result<(), SessionError> {
        let Role::Host(host) = &self.role else {
            return Err(SessionError::NotSubhost);
        };
        if host
            .members
            .iter()
            .any(|m| m.member.peer == peer && m.ready())
        {
            let commit = host.commit;
            self.out.send(peer, &Message::Order { commit, ids });
        }
        Ok(())
    }

    /// The mod found the editor's bricks in another object order than the subhost's (and nothing
    /// else different): ask for its order (once until it comes).
    pub fn request_order(&mut self) {
        if let Role::Guest(guest) = &mut self.role
            && guest.loaded
            && !guest.resync_asked
            && !guest.order_asked
        {
            guest.order_asked = true;
            let host = guest.host;
            self.out.send(host, &Message::OrderWanted);
        }
    }

    /// The mod noticed the editor differs from what the session expects: ask for a snapshot.
    pub fn request_resync(&mut self) {
        self.resync(Event::Drifted);
    }

    /// Asks for a snapshot (once until it comes), telling the mod why with `event`.
    fn resync(&mut self, event: Event) {
        if let Role::Guest(guest) = &mut self.role
            && guest.loaded
            && !guest.resync_asked
        {
            guest.resync_asked = true;
            let host = guest.host;
            self.out.send(host, &Message::Resync);
            self.events.push(event);
        }
    }

    /// What the local player has selected, for others to see.
    pub fn select(&mut self, mut ids: Vec<CoopId>) {
        ids.truncate(crate::message::caps::SELECTION);
        match &self.role {
            Role::Host(host) => {
                let peers: Vec<PeerId> = host
                    .members
                    .iter()
                    .filter(|m| m.ready())
                    .map(|m| m.member.peer)
                    .collect();
                let message = Message::Selection {
                    slot: SUBHOST_SLOT,
                    ids,
                };
                peers
                    .into_iter()
                    .for_each(|peer| self.out.send(peer, &message));
            }
            Role::Guest(guest) => {
                let (host, slot) = (guest.host, guest.slot);
                self.out.send(host, &Message::Selection { slot, ids });
            }
            Role::Idle | Role::Joining { .. } => {}
        }
    }

    // --- Receiving ---

    /// A frame from the transport.
    pub fn receive(&mut self, peer: PeerId, frame: &[u8]) {
        if !self.admit(peer, frame.len()) {
            return;
        }
        match self.assembler.receive(peer, frame) {
            Ok(Some((session, message))) => self.handle(peer, session, message),
            // Strangers only send what fits one frame (hellos, joins): their parts aren't kept.
            Ok(None) if !self.expected_peers().contains(&peer) => self.assembler.forget(peer),
            Ok(None) => {}
            // Not ours at all: some other mod's traffic on the channel. Dropped quietly.
            Err(DecodeError::NotOurs) => {}
            Err(error) => self.events.push(Event::BadMessage { peer, error }),
        }
    }

    /// Whether to read a frame of `len` bytes from `peer`: not from one that can't join, nor past
    /// its allowance of bytes (a member past it is kicked). Our subhost's are always read.
    fn admit(&mut self, peer: PeerId, len: usize) -> bool {
        if self.limits.banned.contains(&peer) {
            return false;
        }
        if let Role::Joining { host } | Role::Guest(Guest { host, .. }) = &self.role
            && *host == peer
        {
            return true;
        }
        if self.limits.crowded() {
            let keep = self.expected_peers();
            self.limits.prune(&keep);
        }
        if self.limits.of(peer, self.now).bytes(self.now, len) {
            return true;
        }
        if self.is_subhost() && self.expected_peers().contains(&peer) {
            self.flood(peer, Flood::Bytes);
        } else {
            self.assembler.forget(peer);
        }
        false
    }

    fn handle(&mut self, peer: PeerId, session: u32, message: Message) {
        if matches!(message, Message::Hello(_) | Message::HelloReply(_))
            && !self.limits.of(peer, self.now).hello(self.now)
        {
            return;
        }
        match message {
            Message::Hello(info) => {
                self.events.push(Event::Hello { peer, info });
                let own = self.own_info();
                self.out.send(peer, &Message::HelloReply(own));
                return;
            }
            Message::HelloReply(info) => {
                self.events.push(Event::Hello { peer, info });
                return;
            }
            _ => {}
        }
        match &self.role {
            Role::Idle => {}
            Role::Host(_) => self.handle_as_host(peer, session, message),
            Role::Joining { host } => {
                if peer == *host {
                    self.handle_as_joiner(session, message);
                }
            }
            Role::Guest(guest) => {
                if peer == guest.host && session == self.out.session {
                    self.handle_as_guest(message);
                }
            }
        }
    }

    fn handle_as_host(&mut self, peer: PeerId, session: u32, message: Message) {
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        if let Message::Join {
            code,
            name,
            game_version,
        } = message
        {
            let refusal = if code != host.settings.code {
                if self.limits.of(peer, self.now).wrong_code() {
                    self.limits.banned.insert(peer);
                }
                Some(RefuseReason::WrongCode)
            } else if game_version != self.info.game_version {
                Some(RefuseReason::GameVersion)
            } else if host.members.len() >= host.settings.max_members
                || host.next_slot > u16::from(Slot::MAX)
            {
                Some(RefuseReason::Full)
            } else {
                None
            };
            if let Some(reason) = refusal {
                self.out.send(peer, &Message::JoinRefused(reason));
                return;
            }
            // Joining again (its game restarted): the old membership goes.
            host.members.retain(|m| m.member.peer != peer);
            let slot = host.next_slot as Slot;
            host.next_slot += 1;
            let mut member = HostedMember {
                member: Member { slot, peer, name },
                waiting: None,
            };
            let request = member
                .wait(host.commit)
                .expect("new members aren't waiting yet");
            host.members.push(member);
            self.assembler.forget(peer);
            self.out.send(peer, &Message::JoinAccepted { slot });
            self.want_snapshot(request);
            self.broadcast_members();
            return;
        }

        if session != self.out.session {
            return;
        }
        if !host.members.iter().any(|m| m.member.peer == peer) {
            return;
        }
        let now = self.now;
        let allowance = self.limits.of(peer, now);
        let flood = match &message {
            // Only shown: dropped past their allowance.
            Message::Selection { .. } if !allowance.selection(now) => return,
            Message::Selection { .. } => None,
            Message::Propose { ops, .. } => allowance.message(now, ops).err(),
            _ => allowance.message(now, &[]).err(),
        };
        if let Some(what) = flood {
            return self.flood(peer, what);
        }
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        let commit = host.commit;
        let Some(author) = host.members.iter_mut().find(|m| m.member.peer == peer) else {
            return;
        };
        let slot = author.member.slot;
        match message {
            // Also while it waits for a resync snapshot: those commits come after the snapshot.
            Message::Propose { local, ops } => {
                if valid_proposal(&host.live, &host.retired, slot, &ops) {
                    self.commit(slot, local, ops);
                } else if let Some(request) = author.wait(commit) {
                    // Its editor and ours disagree: it gets the truth, which drops its pending
                    // edits.
                    self.want_snapshot(request);
                }
            }
            Message::Resync => {
                if let Some(request) = author.wait(commit) {
                    self.want_snapshot(request);
                }
            }
            // One waiting for a snapshot gets the order with it. Past its allowance, it asks
            // again after the next audit.
            Message::OrderWanted if author.ready() && self.limits.of(peer, now).order(now) => {
                self.events.push(Event::OrderWanted(peer))
            }
            Message::Selection { mut ids, .. } => {
                ids.truncate(crate::message::caps::SELECTION);
                let message = Message::Selection {
                    slot,
                    ids: ids.clone(),
                };
                let others: Vec<PeerId> = host
                    .members
                    .iter()
                    .filter(|m| m.ready() && m.member.peer != peer)
                    .map(|m| m.member.peer)
                    .collect();
                others.into_iter().for_each(|p| self.out.send(p, &message));
                self.events.push(Event::Selection { slot, ids });
            }
            Message::Leave => {
                host.members.retain(|m| m.member.peer != peer);
                self.assembler.forget(peer);
                self.broadcast_members();
            }
            _ => {}
        }
    }

    fn handle_as_joiner(&mut self, session: u32, message: Message) {
        let Role::Joining { host } = self.role else {
            unreachable!()
        };
        match message {
            Message::JoinAccepted { slot } => {
                self.out.session = session;
                self.role = Role::Guest(Guest {
                    host,
                    slot,
                    ids: IdAllocator::new(slot),
                    next_local: 0,
                    pending: VecDeque::new(),
                    confirmed: LiveSet::default(),
                    loaded: false,
                    resync_asked: false,
                    order_asked: false,
                    commit: 0,
                    members: Vec::new(),
                    history: History::default(),
                });
                self.events.push(Event::Joined { slot });
            }
            Message::JoinRefused(reason) => {
                self.role = Role::Idle;
                self.events.push(Event::Refused(reason));
            }
            _ => {}
        }
    }

    fn handle_as_guest(&mut self, message: Message) {
        let Role::Guest(guest) = &mut self.role else {
            unreachable!()
        };
        match message {
            Message::Snapshot {
                commit,
                ids,
                vehicle,
            } => {
                guest.confirmed = LiveSet::from_ids(ids.iter().copied());
                guest.pending.clear();
                guest.commit = commit;
                guest.order_asked = false;
                // Commits folded into the snapshot can't tell which changes of ours they
                // overwrote.
                guest.history = History::default();
                guest.loaded = true;
                guest.resync_asked = false;
                self.events.push(Event::Load { ids, vehicle });
            }
            Message::Commit {
                number,
                author,
                local,
                ops,
                check,
            } if guest.loaded => {
                guest.commit = number;
                let mut conflict = false;
                let own = author == guest.slot && guest.pending.iter().any(|p| p.local == local);
                // Ops on bricks that didn't exist did nothing at the subhost, and references to
                // them were dropped there; here such a brick may exist again, made by an undo of
                // ours that comes later.
                let mut done = Vec::new();
                for mut op in ops {
                    if !op.is_effective(&guest.confirmed) {
                        continue;
                    }
                    if let Op::Set {
                        value: Value::Bricks(ids),
                        ..
                    } = &mut op
                    {
                        ids.retain(|&id| guest.confirmed.contains(id));
                    }
                    op.apply_to(&mut guest.confirmed);
                    done.push(op);
                }
                if author != guest.slot {
                    let pending: Vec<u32> = guest.pending.iter().map(|p| p.local).collect();
                    guest
                        .history
                        .foreign(&done, |local| pending.contains(&local));
                }
                if own {
                    // Already in our editor. Older pending ones were refused; a snapshot follows.
                    guest.pending.retain(|p| p.local > local);
                } else {
                    let pending: Vec<&Op> = guest.pending.iter().flat_map(|p| &p.ops).collect();
                    let ops: Vec<Op> = done
                        .into_iter()
                        .filter(|op| !op.overridden_by(pending.iter().copied()))
                        .collect();
                    // Edits of others are made here after our pending ones, which come after
                    // them at the subhost; that's the same, but for bricks made again (undoing
                    // a delete) by either side: making them undoes the other side's changes to
                    // them. Rare (both edited or deleted the same brick at about the same time):
                    // the subhost's vehicle sorts it out.
                    let made = |ops: &mut dyn Iterator<Item = &Op>| -> Vec<CoopId> {
                        ops.flat_map(|op| match op {
                            Op::Create { ids, .. } => ids.as_slice(),
                            _ => &[],
                        })
                        .copied()
                        .collect()
                    };
                    let theirs = made(&mut ops.iter());
                    let ours = made(&mut pending.iter().copied());
                    conflict = pending.iter().any(|op| op.mentions(&theirs))
                        || ops.iter().any(|op| op.mentions(&ours));
                    if !ops.is_empty() {
                        self.events.push(Event::Apply { author, ops });
                    }
                }
                if conflict {
                    self.resync(Event::UndoConflict);
                } else if guest.confirmed.check() != check {
                    self.request_resync();
                }
            }
            Message::Audit(audit) => {
                // An order asked for and not sent (or not yet) may be asked for again.
                guest.order_asked = false;
                if guest.settled_at(audit.commit) {
                    self.events.push(Event::Audit(audit));
                }
            }
            Message::Order { commit, ids } => {
                guest.order_asked = false;
                // The same bricks only: otherwise the next audit finds what differs.
                let same = ids.len() == guest.confirmed.len()
                    && ids.iter().all(|&id| guest.confirmed.contains(id))
                    && LiveSet::from_ids(ids.iter().copied()).len() == ids.len();
                if guest.settled_at(commit) && same {
                    self.events.push(Event::Reorder(ids));
                }
            }
            Message::Members(members) => {
                guest.members = members.clone();
                self.events.push(Event::Members(members));
            }
            Message::Selection { slot, ids } => self.events.push(Event::Selection { slot, ids }),
            Message::Kick | Message::Close => {
                let reason = if matches!(message, Message::Kick) {
                    EndReason::Kicked
                } else {
                    EndReason::Closed
                };
                self.role = Role::Idle;
                self.assembler = Assembler::default();
                self.events.push(Event::Ended(reason));
            }
            _ => {}
        }
    }

    // --- Helpers ---

    /// Subhost: asks the mod for `request`'s snapshot, now or once the member's allowance has
    /// room (making one takes the editor a while on a big vehicle).
    fn want_snapshot(&mut self, request: SnapshotRequest) {
        let due = self
            .limits
            .of(request.peer, self.now)
            .snapshot_due(self.now);
        if due <= self.now {
            self.events.push(Event::SnapshotWanted(request));
        } else if let Role::Host(host) = &mut self.role {
            host.deferred.push((due, request));
        }
    }

    /// Subhost: kicks a member that sent far too much.
    fn flood(&mut self, peer: PeerId, what: Flood) {
        let _ = self.kick(peer);
        self.events.push(Event::Flooded { peer, what });
    }

    /// Subhost: numbers an edit and sends it to everyone with the vehicle. The subhost's own
    /// editor already has its own edits; others' it applies here.
    fn commit(&mut self, author: Slot, local: u32, ops: Vec<Op>) {
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        host.commit += 1;
        let mut done = Vec::new();
        for op in &ops {
            if !op.is_effective(&host.live) {
                continue;
            }
            done.push(op);
            match op {
                Op::Create { ids, .. } => ids.iter().for_each(|id| {
                    host.retired.remove(id);
                }),
                Op::Delete { id } if host.live.contains(*id) => {
                    host.retired.insert(*id, author);
                }
                _ => {}
            }
            op.apply_to(&mut host.live);
        }
        if author != SUBHOST_SLOT {
            host.history.foreign(done, |_| false);
        }
        let message = Message::Commit {
            number: host.commit,
            author,
            local,
            ops,
            check: host.live.check(),
        };
        // Members waiting for a snapshot get this one in it.
        for member in host.members.iter().filter(|m| m.ready()) {
            self.out.send(member.member.peer, &message);
        }
        if author != SUBHOST_SLOT {
            let Message::Commit { ops, .. } = message else {
                unreachable!()
            };
            self.events.push(Event::Apply { author, ops });
        }
    }

    fn broadcast_members(&mut self) {
        let Role::Host(host) = &self.role else { return };
        let list = host_member_list(host, 0);
        let peers: Vec<PeerId> = host.members.iter().map(|m| m.member.peer).collect();
        let message = Message::Members(list.clone());
        peers
            .into_iter()
            .for_each(|peer| self.out.send(peer, &message));
        self.events.push(Event::Members(list));
    }

    fn own_info(&self) -> PeerInfo {
        PeerInfo {
            hosting: self.is_subhost(),
            ..self.info.clone()
        }
    }
}

/// The subhost first (`self_peer` is a placeholder: peers know the subhost's address already).
fn host_member_list(host: &Host, self_peer: PeerId) -> Vec<Member> {
    std::iter::once(Member {
        slot: SUBHOST_SLOT,
        peer: self_peer,
        name: host.name.clone(),
    })
    .chain(host.members.iter().map(|m| m.member.clone()))
    .collect()
}

/// A member may only create bricks with its own slot's new ids, or bricks it deleted again
/// (undoing the delete), and not ones that exist. A brick someone else deleted first can't come
/// back through another member, whose own delete did nothing: that member's editor still applies
/// the first delete when it arrives, after making the brick again.
fn valid_proposal(live: &LiveSet, retired: &HashMap<CoopId, Slot>, slot: Slot, ops: &[Op]) -> bool {
    ops.iter().all(|op| match op {
        Op::Create { ids, .. } => ids.iter().all(|&id| {
            !live.contains(id) && retired.get(&id).map_or(id.slot() == slot, |&by| by == slot)
        }),
        _ => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> PeerInfo {
        PeerInfo {
            protocol: (1, 1),
            mod_version: "test".into(),
            game_version: "1".into(),
            hosting: false,
        }
    }

    fn settings() -> HostSettings {
        HostSettings {
            code: "1234".into(),
            max_members: 4,
        }
    }

    /// Delivers everything both ways until quiet.
    fn pump(host: &mut Session, host_peer: PeerId, guest: &mut Session, guest_peer: PeerId) {
        loop {
            let a = host.take_outgoing();
            let b = guest.take_outgoing();
            if a.is_empty() && b.is_empty() {
                break;
            }
            a.into_iter()
                .filter(|o| o.to == guest_peer)
                .for_each(|o| guest.receive(host_peer, &o.bytes));
            b.into_iter()
                .filter(|o| o.to == host_peer)
                .for_each(|o| host.receive(guest_peer, &o.bytes));
        }
    }

    fn snapshot_request(host: &mut Session) -> SnapshotRequest {
        host.take_events()
            .into_iter()
            .find_map(|e| match e {
                Event::SnapshotWanted(request) => Some(request),
                _ => None,
            })
            .expect("a snapshot request")
    }

    #[test]
    fn join_refusals() {
        let mut host = Session::new(info());
        host.host(7, "h".into(), settings(), 3).unwrap();
        let mut guest = Session::new(info());
        guest.join(1, "nope".into(), "g".into()).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        assert_eq!(
            guest.take_events(),
            vec![Event::Refused(RefuseReason::WrongCode)]
        );
        assert!(guest.is_idle());

        let mut other_build = Session::new(PeerInfo {
            game_version: "2".into(),
            ..info()
        });
        other_build.join(1, "1234".into(), "g".into()).unwrap();
        pump(&mut host, 1, &mut other_build, 3);
        assert_eq!(
            other_build.take_events(),
            vec![Event::Refused(RefuseReason::GameVersion)]
        );
    }

    #[test]
    fn join_load_edit_leave() {
        let mut host = Session::new(info());
        let original = host.host(7, "h".into(), settings(), 2).unwrap();
        let mut guest = Session::new(info());
        guest.join(1, "1234".into(), "g".into()).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        let request = snapshot_request(&mut host);
        assert_eq!(request.peer, 2);
        host.send_snapshot(request, original.clone(), b"vehicle".to_vec())
            .unwrap();
        pump(&mut host, 1, &mut guest, 2);
        let events = guest.take_events();
        assert!(events.contains(&Event::Joined { slot: 2 }));
        assert!(events.contains(&Event::Load {
            ids: original.clone(),
            vehicle: b"vehicle".to_vec()
        }));

        let ids = guest.new_ids(1).unwrap();
        assert_eq!(ids[0].slot(), 2);
        guest
            .edit(vec![Op::Create {
                ids: ids.clone(),
                archive: vec![1],
            }])
            .unwrap();
        pump(&mut host, 1, &mut guest, 2);
        assert_eq!(
            host.take_events(),
            vec![Event::Apply {
                author: 2,
                ops: vec![Op::Create {
                    ids: ids.clone(),
                    archive: vec![1]
                }]
            }]
        );
        // Our own commit isn't applied twice, and its check matches.
        assert_eq!(guest.take_events(), vec![]);

        guest.leave();
        pump(&mut host, 1, &mut guest, 2);
        assert_eq!(host.members().len(), 1);
    }

    #[test]
    fn only_who_deleted_a_brick_makes_it_again() {
        let mut host = Session::new(info());
        let original = host.host(7, "h".into(), settings(), 2).unwrap();
        let mut guest = Session::new(info());
        guest.join(1, "1234".into(), "g".into()).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        let request = snapshot_request(&mut host);
        host.send_snapshot(request, original.clone(), vec![])
            .unwrap();
        pump(&mut host, 1, &mut guest, 2);
        host.take_events();
        guest.take_events();
        let create = |id| Op::Create {
            ids: vec![id],
            archive: vec![],
        };

        // The guest deletes brick 0 and makes it again (undoing): fine.
        guest.edit(vec![Op::Delete { id: original[0] }]).unwrap();
        guest.edit(vec![create(original[0])]).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        assert!(
            host.take_events()
                .iter()
                .all(|e| matches!(e, Event::Apply { .. }))
        );

        // The subhost deletes brick 1: the guest can't make it again.
        host.edit(vec![Op::Delete { id: original[1] }]).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        guest.edit(vec![create(original[1])]).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        assert!(matches!(
            &host.take_events()[..],
            [Event::SnapshotWanted(SnapshotRequest { peer: 2, .. })]
        ));
    }

    #[test]
    fn foreign_creates_are_refused() {
        let mut host = Session::new(info());
        let original = host.host(7, "h".into(), settings(), 1).unwrap();
        let mut guest = Session::new(info());
        guest.join(1, "1234".into(), "g".into()).unwrap();
        pump(&mut host, 1, &mut guest, 2);
        let request = snapshot_request(&mut host);
        host.send_snapshot(request, original.clone(), vec![])
            .unwrap();
        pump(&mut host, 1, &mut guest, 2);
        host.take_events();
        // Re-creating a brick that exists (slot 0): the guest's editor is off, so it gets a snapshot.
        guest
            .edit(vec![Op::Create {
                ids: original,
                archive: vec![],
            }])
            .unwrap();
        pump(&mut host, 1, &mut guest, 2);
        assert!(matches!(
            &host.take_events()[..],
            [Event::SnapshotWanted(SnapshotRequest { peer: 2, .. })]
        ));
    }
}
