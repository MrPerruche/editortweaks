//! Simulated members editing a fake vehicle over an in-memory network that delivers each pair's
//! messages in order but interleaves pairs at random. Whatever the order, every member must end
//! with the subhost's vehicle, without needing a resync. Members also undo and redo their own
//! edits, which they record as the mod does (`Player::make`). The subhost audits its editor at
//! random moments: a member must never find a brick that differs (a false alarm), and an object
//! order that differs (bricks made at the same moment) is mended from the subhost's order, so every
//! member ends with the subhost's order too.

use std::collections::{BTreeMap, HashMap};

use coop::audit::{Digest, Rounds, order_hash};
use coop::message::caps::SNAPSHOT_IDS;
use coop::transport::{self, MemoryEndpoint, MemoryNet};
use coop::vehicle::Vehicle;
use coop::wire::{Reader, Writer};
use coop::{
    Change, CoopId, Difference, Event, GroupKind, Groups, HostSettings, Message, Op, PeerId,
    PeerInfo, Session, SnapshotRequest, Transform, Value,
};

// --- A fake editor ---

#[derive(Debug, Clone, PartialEq)]
struct Brick {
    transform: Transform,
    props: BTreeMap<String, Value>,
    /// Not in the fake archive: the game's loader renumbers groups, so the mod clears them and
    /// sends them as ops (and with snapshots).
    groups: Groups,
}

/// Bricks in object order.
#[derive(Debug, Clone, Default)]
struct Editor {
    bricks: Vec<(CoopId, Brick)>,
}

impl Editor {
    fn find(&mut self, id: CoopId) -> Option<&mut Brick> {
        self.bricks
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, b)| b)
    }

    fn exists(&self, id: CoopId) -> bool {
        self.bricks.iter().any(|(i, _)| *i == id)
    }

    fn ids(&self) -> Vec<CoopId> {
        self.bricks.iter().map(|(id, _)| *id).collect()
    }

    /// The brick's digest, as the mod makes it from the game's editor.
    fn digest(&self, id: CoopId) -> Option<Digest> {
        let (_, brick) = self.bricks.iter().find(|(i, _)| *i == id)?;
        Some(Digest::of(
            &brick.transform,
            brick
                .props
                .iter()
                .map(|(name, value)| (name.as_str(), value)),
            brick.groups,
        ))
    }

    /// What every member must agree on (object order of concurrent creates may differ).
    fn state(&self) -> State {
        self.bricks.iter().cloned().collect()
    }

    /// Like the game, references to missing bricks are dropped.
    fn existing_refs(&self, value: &Value) -> Value {
        match value {
            Value::Bricks(ids) => {
                Value::Bricks(ids.iter().copied().filter(|&id| self.exists(id)).collect())
            }
            other => other.clone(),
        }
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Create { ids, archive } => {
                for (id, brick) in ids.iter().zip(decode_bricks(archive)) {
                    if !self.exists(*id) {
                        self.bricks.push((*id, brick));
                    }
                }
            }
            Op::Delete { id } => {
                self.bricks.retain(|(i, _)| i != id);
                for (_, brick) in &mut self.bricks {
                    for value in brick.props.values_mut() {
                        if let Value::Bricks(refs) = value {
                            refs.retain(|r| r != id);
                        }
                    }
                }
            }
            Op::Move { id, transform } => {
                if let Some(brick) = self.find(*id) {
                    brick.transform = *transform;
                }
            }
            Op::Set {
                id,
                property,
                value,
            } => {
                let value = self.existing_refs(value);
                if let Some(brick) = self.find(*id) {
                    brick.props.insert(property.clone(), value);
                }
            }
            Op::Group { id, kind, group } => {
                if let Some(brick) = self.find(*id) {
                    brick.groups[*kind as usize] = *group;
                }
            }
            Op::Order { ids } => {
                let current: Vec<CoopId> = self.bricks.iter().map(|(id, _)| *id).collect();
                let mut bricks: HashMap<CoopId, Brick> = self.bricks.drain(..).collect();
                self.bricks = coop::order::reordered(&current, ids)
                    .into_iter()
                    .map(|id| (id, bricks.remove(&id).unwrap()))
                    .collect();
            }
        }
    }

    fn save(&self) -> (Vec<CoopId>, Vec<u8>) {
        let ids = self.bricks.iter().map(|(id, _)| *id).collect();
        let vehicle = Vehicle {
            archive: encode_bricks(self.bricks.iter().map(|(_, b)| b)),
            groups: self.bricks.iter().map(|(_, b)| b.groups).collect(),
        };
        (ids, vehicle.encode())
    }

    fn load(ids: &[CoopId], vehicle: &[u8]) -> Self {
        let vehicle = Vehicle::decode(vehicle).unwrap();
        let mut bricks = decode_bricks(&vehicle.archive);
        assert_eq!(bricks.len(), ids.len());
        assert_eq!(vehicle.groups.len(), ids.len());
        for (brick, groups) in bricks.iter_mut().zip(vehicle.groups) {
            brick.groups = groups;
        }
        Self {
            bricks: ids.iter().copied().zip(bricks).collect(),
        }
    }
}

/// The fake archive: bricks without ids, like the game's.
fn encode_bricks<'a>(bricks: impl ExactSizeIterator<Item = &'a Brick>) -> Vec<u8> {
    let mut w = Writer::new();
    w.count(bricks.len());
    for brick in bricks {
        brick
            .transform
            .location
            .iter()
            .chain(&brick.transform.rotation)
            .for_each(|&v| w.f32(v));
        w.count(brick.props.len());
        for (name, value) in &brick.props {
            w.str(name);
            match value {
                Value::Raw(bytes) => {
                    w.u8(0);
                    w.bytes(bytes);
                }
                Value::Text(text) => {
                    w.u8(1);
                    w.str(text);
                }
                Value::Bricks(ids) => {
                    w.u8(2);
                    w.count(ids.len());
                    ids.iter().for_each(|id| w.u32(id.raw()));
                }
            }
        }
    }
    w.finish()
}

fn decode_bricks(bytes: &[u8]) -> Vec<Brick> {
    let mut r = Reader::new(bytes);
    let n = r.count("bricks", usize::MAX).unwrap();
    (0..n)
        .map(|_| {
            let v: Vec<f32> = (0..6).map(|_| r.f32().unwrap()).collect();
            let transform = Transform {
                location: [v[0], v[1], v[2]],
                rotation: [v[3], v[4], v[5]],
            };
            let props = (0..r.count("props", usize::MAX).unwrap())
                .map(|_| {
                    let name = r.str("name", usize::MAX).unwrap();
                    let value = match r.u8().unwrap() {
                        0 => Value::Raw(r.bytes("raw", usize::MAX).unwrap().to_vec()),
                        1 => Value::Text(r.str("text", usize::MAX).unwrap()),
                        _ => Value::Bricks(
                            (0..r.count("ids", usize::MAX).unwrap())
                                .map(|_| CoopId::from_raw(r.u32().unwrap()).unwrap())
                                .collect(),
                        ),
                    };
                    (name, value)
                })
                .collect();
            Brick {
                transform,
                props,
                groups: [0; 2],
            }
        })
        .collect()
}

/// `COOP_SIM_TRACE=1` prints what every member does (run one seed with `COOP_SIM_FIRST`).
fn trace(line: impl FnOnce() -> String) {
    if std::env::var_os("COOP_SIM_TRACE").is_some() {
        eprintln!("{}", line());
    }
}

// --- Randomness (no dependency, repeatable) ---

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn f32(&mut self) -> f32 {
        (self.next() % 2001) as f32 / 10.0 - 100.0
    }

    fn transform(&mut self) -> Transform {
        Transform {
            location: [self.f32(), self.f32(), self.f32()],
            rotation: [self.f32(), self.f32(), self.f32()],
        }
    }

    fn raw(&mut self) -> Value {
        Value::Raw(self.next().to_le_bytes()[..1 + self.below(8)].to_vec())
    }
}

/// Every property a brick has (a game brick always has all of its class').
const PROPERTIES: [&str; 4] = ["Color", "Size", "Name", REFERENCES];
const REFERENCES: &str = "Inputs.SourceBricks";

fn new_brick(rng: &mut Rng) -> Brick {
    Brick {
        transform: rng.transform(),
        props: PROPERTIES
            .iter()
            .map(|&name| {
                let value = if name == REFERENCES {
                    Value::Bricks(Vec::new())
                } else {
                    rng.raw()
                };
                (name.to_string(), value)
            })
            .collect(),
        groups: [rng.below(3) as u16, rng.below(3) as u16],
    }
}

// --- Members ---

const HOST: PeerId = 100;

fn info() -> PeerInfo {
    PeerInfo {
        protocol: (1, 1),
        mod_version: "sim".into(),
        game_version: "1".into(),
        hosting: false,
    }
}

struct Player {
    peer: PeerId,
    session: Session,
    transport: MemoryEndpoint,
    editor: Editor,
    /// Times a commit's check showed this member had drifted.
    drifts: usize,
    /// Times bricks someone else made again clashed with this member's edits on their way.
    conflicts: usize,
    /// Like the game's editor while it moves or places bricks: only sends, reads nothing, and
    /// finishes no edit (`features/coop`).
    busy: bool,
    /// Differences audits found that only a snapshot mends (none, unless the editor drifted).
    differences: Vec<Difference>,
    /// Times an audit found only the object order different.
    misordered: usize,
    /// Subhost: which bricks it audits next.
    rounds: Rounds,
}

impl Player {
    fn new(net: &std::rc::Rc<std::cell::RefCell<MemoryNet>>, peer: PeerId) -> Self {
        Self {
            peer,
            session: Session::new(info()),
            transport: MemoryNet::endpoint(net, peer),
            editor: Editor::default(),
            drifts: 0,
            conflicts: 0,
            busy: false,
            differences: Vec::new(),
            misordered: 0,
            rounds: Rounds::default(),
        }
    }

    /// Subhost: describes its editor to the members, with `n` bricks, as the mod does.
    fn audit(&mut self, n: usize) {
        let ids = self.editor.ids();
        let bricks = self
            .rounds
            .pick(&ids, &[], n)
            .into_iter()
            .map(|id| (id, self.editor.digest(id).unwrap()))
            .collect();
        trace(|| format!("{} audits {n} bricks", self.peer));
        self.session
            .audit(ids.len() as u32, order_hash(ids), bricks)
            .unwrap();
    }

    /// Sends what's queued, reads what arrived, and does what the session asks. Snapshots go
    /// out last, once every event is applied, as the mod does.
    fn pump(&mut self) {
        self.session.tick(now());
        if self.busy {
            for out in self.session.take_outgoing() {
                transport::Transport::send(&mut self.transport, out.to, &out.bytes, out.reliable);
            }
            return;
        }
        let mut snapshots: Vec<SnapshotRequest> = Vec::new();
        let mut orders: Vec<PeerId> = Vec::new();
        loop {
            transport::pump(&mut self.session, &mut self.transport);
            let events = self.session.take_events();
            if events.is_empty() {
                break;
            }
            for event in events {
                trace(|| format!("{} event {event:?}", self.peer));
                match event {
                    Event::Load { ids, vehicle } => self.editor = Editor::load(&ids, &vehicle),
                    Event::Apply { ops, .. } => ops.iter().for_each(|op| self.editor.apply(op)),
                    Event::SnapshotWanted(request) => snapshots.push(request),
                    Event::OrderWanted(peer) => orders.push(peer),
                    Event::Audit(audit) => {
                        let ids = self.editor.ids();
                        let differences = audit.compare(ids.len() as u32, order_hash(ids), |id| {
                            self.editor.digest(id)
                        });
                        if differences.iter().any(|d| d.needs_snapshot()) {
                            self.differences.extend(differences);
                            self.session.request_resync();
                        } else if !differences.is_empty() {
                            self.misordered += 1;
                            self.session.request_order();
                        }
                    }
                    Event::Reorder(ids) => {
                        let mut bricks: HashMap<CoopId, Brick> =
                            self.editor.bricks.drain(..).collect();
                        self.editor.bricks = ids
                            .iter()
                            .map(|id| (*id, bricks.remove(id).unwrap()))
                            .collect();
                    }
                    Event::Drifted => self.drifts += 1,
                    Event::UndoConflict => self.conflicts += 1,
                    Event::BadMessage { peer, error } => {
                        panic!("{} got junk from {peer}: {error}", self.peer)
                    }
                    _ => {}
                }
            }
        }
        for request in snapshots {
            let (ids, vehicle) = self.editor.save();
            self.session.send_snapshot(request, ids, vehicle).unwrap();
        }
        for peer in orders {
            self.session.send_order(peer, self.editor.ids()).unwrap();
        }
        // Answering may have queued frames.
        transport::pump(&mut self.session, &mut self.transport);
        assert!(self.session.take_events().is_empty());
    }

    /// A random edit, applied here at once and handed to the session; whether it changed anything.
    fn random_edit(&mut self, rng: &mut Rng) -> bool {
        let mut changes = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let existing: Vec<CoopId> = self.editor.bricks.iter().map(|(id, _)| *id).collect();
            let pick = |rng: &mut Rng| existing[rng.below(existing.len())];
            let ops = match rng.below(11) {
                _ if existing.is_empty() => {
                    changes.push(self.random_create(rng));
                    continue;
                }
                0 | 1 => {
                    changes.push(self.random_create(rng));
                    continue;
                }
                2 => vec![Op::Delete { id: pick(rng) }],
                3 | 4 => vec![Op::Move {
                    id: pick(rng),
                    transform: rng.transform(),
                }],
                5..=7 => vec![Op::Set {
                    id: pick(rng),
                    property: PROPERTIES[rng.below(3)].into(),
                    value: rng.raw(),
                }],
                8 => vec![Op::Set {
                    id: pick(rng),
                    property: REFERENCES.into(),
                    value: Value::Bricks((0..1 + rng.below(2)).map(|_| pick(rng)).collect()),
                }],
                9 => vec![Op::Order {
                    ids: (0..1 + rng.below(3)).map(|_| pick(rng)).collect(),
                }],
                // Few group numbers, so members often pick the same one.
                _ => (0..1 + rng.below(3))
                    .map(|_| Op::Group {
                        id: pick(rng),
                        kind: GroupKind::ALL[rng.below(2)],
                        group: rng.below(4) as u16,
                    })
                    .collect(),
            };
            for op in ops {
                changes.extend(self.make(op));
            }
        }
        trace(|| format!("{} edits {changes:?}", self.peer));
        let changed = changes.iter().any(|change| !change.done.is_empty());
        self.session.edit_undoable(changes).unwrap();
        changed
    }

    /// Makes `op` here, returning it as changes with what undoes them, worked out as the mod does
    /// from the vehicle just before. A delete also changes the bricks that referred to the deleted
    /// one (the game drops those references), and undoing it puts the brick back in its place in
    /// the object order.
    fn make(&mut self, op: Op) -> Vec<Change> {
        let order: Vec<CoopId> = self.editor.bricks.iter().map(|(id, _)| *id).collect();
        let mut changes = Vec::new();
        match &op {
            Op::Delete { id } => {
                let id = *id;
                let mut brick = self.editor.find(id).unwrap().clone();
                // Like a game archive of the brick alone, which loses its references and groups:
                // they follow it.
                let mut undo = Vec::new();
                let mut later = Vec::new();
                for (name, value) in &mut brick.props {
                    if let Value::Bricks(refs) = value {
                        let refs = std::mem::take(refs);
                        if !refs.is_empty() {
                            later.push(Op::Set {
                                id,
                                property: name.clone(),
                                value: Value::Bricks(refs),
                            });
                        }
                    }
                }
                for kind in GroupKind::ALL {
                    let group = brick.groups[kind as usize];
                    if group != 0 {
                        later.push(Op::Group { id, kind, group });
                    }
                }
                brick.groups = [0; 2];
                undo.push(Op::Create {
                    ids: vec![id],
                    archive: encode_bricks([&brick].into_iter()),
                });
                undo.extend(later);
                changes.push(Change {
                    done: vec![op.clone()],
                    undo,
                });
                for (other, brick) in &self.editor.bricks {
                    for (name, value) in &brick.props {
                        if *other != id
                            && let Value::Bricks(refs) = value
                            && refs.contains(&id)
                        {
                            let set = |refs: Vec<CoopId>| Op::Set {
                                id: *other,
                                property: name.clone(),
                                value: Value::Bricks(refs),
                            };
                            changes.push(Change {
                                done: vec![set(refs
                                    .iter()
                                    .copied()
                                    .filter(|r| *r != id)
                                    .collect())],
                                undo: vec![set(refs.clone())],
                            });
                        }
                    }
                }
                if let Some(ids) = coop::undo::restore_order(&order, &[id]) {
                    changes.push(Change {
                        done: Vec::new(),
                        undo: vec![Op::Order { ids }],
                    });
                }
            }
            Op::Move { id, .. } => {
                let transform = self.editor.find(*id).unwrap().transform;
                changes.push(Change {
                    done: vec![op.clone()],
                    undo: vec![Op::Move { id: *id, transform }],
                });
            }
            Op::Set { id, property, .. } => {
                let value = self.editor.find(*id).unwrap().props[property].clone();
                changes.push(Change {
                    done: vec![op.clone()],
                    undo: vec![Op::Set {
                        id: *id,
                        property: property.clone(),
                        value,
                    }],
                });
            }
            Op::Group { id, kind, .. } => {
                let group = self.editor.find(*id).unwrap().groups[*kind as usize];
                changes.push(Change {
                    done: vec![op.clone()],
                    undo: vec![Op::Group {
                        id: *id,
                        kind: *kind,
                        group,
                    }],
                });
            }
            Op::Order { .. } => {
                self.editor.apply(&op);
                let now: Vec<CoopId> = self.editor.bricks.iter().map(|(id, _)| *id).collect();
                // As the mod sends a reorder: the shortest list.
                if let Some(ids) = coop::order::change(&order, &now) {
                    changes.push(Change {
                        done: vec![Op::Order { ids }],
                        undo: vec![Op::Order {
                            ids: coop::order::change(&now, &order).unwrap(),
                        }],
                    });
                }
                return changes;
            }
            Op::Create { .. } => unreachable!("see random_create"),
        }
        // The references the game drops with a deleted brick go with the changes above.
        self.editor.apply(&op);
        changes
    }

    /// New bricks, then their groups (as the mod sends them: the archive loses them), made here.
    fn random_create(&mut self, rng: &mut Rng) -> Change {
        let n = 1 + rng.below(3);
        let ids = self.session.new_ids(n).unwrap();
        let bricks: Vec<Brick> = (0..n).map(|_| new_brick(rng)).collect();
        let mut done = vec![Op::Create {
            ids: ids.clone(),
            archive: encode_bricks(bricks.iter()),
        }];
        for (&id, brick) in ids.iter().zip(&bricks) {
            for kind in GroupKind::ALL {
                let group = brick.groups[kind as usize];
                if group != 0 {
                    done.push(Op::Group { id, kind, group });
                }
            }
        }
        done.iter().for_each(|op| self.editor.apply(op));
        Change {
            done,
            undo: ids.into_iter().map(|id| Op::Delete { id }).collect(),
        }
    }

    /// Undoes (or redoes) its newest step: what that makes, it makes here too. Whether there was
    /// one.
    fn step_back(&mut self, redo: bool) -> bool {
        let back = if redo {
            self.session.redo()
        } else {
            self.session.undo()
        };
        let Some(back) = back.unwrap() else {
            return false;
        };
        trace(|| {
            format!(
                "{} {} {back:?}",
                self.peer,
                if redo { "redoes" } else { "undoes" }
            )
        });
        back.ops.iter().for_each(|op| self.editor.apply(op));
        true
    }
}

/// The simulated time in milliseconds: rate limits (`coop::limits`) follow it.
fn now() -> u64 {
    CLOCK.with(|clock| clock.get())
}

fn pass(ms: u64) {
    CLOCK.with(|clock| clock.set(clock.get() + ms));
}

thread_local! {
    static CLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Delivers everything, waiting out snapshots held back by members' allowances.
fn settle(net: &std::rc::Rc<std::cell::RefCell<MemoryNet>>, players: &mut [Player]) {
    players.iter_mut().for_each(|p| p.busy = false);
    let mut quiet = 0;
    // Quiet for over a minute of simulated time: nothing is held back any more.
    while quiet < 8 {
        players.iter_mut().for_each(Player::pump);
        if net.borrow().busy_pairs().is_empty() {
            quiet += 1;
            pass(10_000);
        } else {
            quiet = 0;
            net.borrow_mut().deliver_all();
        }
    }
    players.iter_mut().for_each(Player::pump);
}

fn starting_vehicle(rng: &mut Rng, bricks: usize) -> Editor {
    Editor {
        bricks: (0..bricks)
            .map(|_| {
                let mut brick = new_brick(rng);
                brick.groups[GroupKind::Weld as usize] = 0;
                (CoopId::new(0, 1).unwrap(), brick)
            })
            .collect(),
    }
}

/// A subhost with a vehicle of `bricks` bricks and `guests` members that joined and have it.
fn session_of(
    seed: u64,
    guests: usize,
    bricks: usize,
) -> (
    std::rc::Rc<std::cell::RefCell<MemoryNet>>,
    Vec<Player>,
    Vec<CoopId>,
) {
    let net = MemoryNet::new();
    let mut players: Vec<Player> = (0..=guests)
        .map(|i| Player::new(&net, HOST + i as PeerId))
        .collect();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    players[0].editor = starting_vehicle(&mut rng, bricks);
    let ids = players[0]
        .session
        .host(
            1,
            "h".into(),
            HostSettings {
                code: "c".into(),
                max_members: 8,
            },
            bricks,
        )
        .unwrap();
    for ((id, _), new) in players[0].editor.bricks.iter_mut().zip(&ids) {
        *id = *new;
    }
    for p in &mut players[1..] {
        p.session.join(HOST, "c".into(), "g".into()).unwrap();
    }
    settle(&net, &mut players);
    (net, players, ids)
}

/// Snapshots asked for over undo conflicts, over every run (`members_converge` prints them).
static CONFLICTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Audits that found only the object order different (mended from the subhost's).
static MISORDERED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One random run: `guests` join over time, everyone edits, some leave and come back.
fn run(seed: u64, guests: usize, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let net = MemoryNet::new();
    let mut players = vec![Player::new(&net, HOST)];

    let bricks = rng.below(6);
    let mut vehicle = starting_vehicle(&mut rng, bricks);
    let settings = HostSettings {
        code: "code".into(),
        max_members: 8,
    };
    let ids = players[0]
        .session
        .host(seed as u32, "host".into(), settings, vehicle.bricks.len())
        .unwrap();
    for ((id, _), new) in vehicle.bricks.iter_mut().zip(ids) {
        *id = new;
    }
    players[0].editor = vehicle;

    for step in 0..steps {
        let roll = rng.below(100);
        if roll < 3 && players.len() <= guests {
            let mut joiner = Player::new(&net, HOST + players.len() as PeerId);
            joiner
                .session
                .join(HOST, "code".into(), format!("guest {}", players.len()))
                .unwrap();
            players.push(joiner);
        } else if roll < 4 && players.len() > 1 {
            // Leave, and join again later under the same address.
            let p = 1 + rng.below(players.len() - 1);
            if players[p].session.is_loaded() {
                players[p].session.leave();
                players[p].pump();
                players[p].editor = Editor::default();
                players[p]
                    .session
                    .join(HOST, "code".into(), "back".into())
                    .unwrap();
            }
        } else if roll < 6 && players.len() > 1 {
            // As if the mod noticed a drift: the snapshot must not lose commits made meanwhile.
            let p = 1 + rng.below(players.len() - 1);
            if players[p].busy {
                continue;
            }
            trace(|| format!("{} asks for a resync", players[p].peer));
            players[p].session.request_resync();
            // Its own `Drifted` isn't one the checks found.
            for event in players[p].session.take_events() {
                assert_eq!(event, Event::Drifted);
            }
        } else if roll < 10 {
            // Starts or ends moving bricks: meanwhile it reads nothing, and snapshot requests wait.
            let p = rng.below(players.len());
            players[p].busy = !players[p].busy;
        } else if roll < 42 {
            let p = rng.below(players.len());
            if players[p].session.is_loaded() && !players[p].busy {
                players[p].random_edit(&mut rng);
            }
        } else if roll < 50 {
            let p = rng.below(players.len());
            if players[p].session.is_loaded() && !players[p].busy {
                players[p].step_back(roll >= 47);
            }
        } else if roll < 56 {
            // The mod audits while its editor is idle.
            if !players[0].busy {
                let n = 1 + rng.below(8);
                players[0].audit(n);
            }
        } else {
            let pairs = net.borrow().busy_pairs();
            if !pairs.is_empty() {
                net.borrow_mut().deliver_one(pairs[rng.below(pairs.len())]);
            }
        }
        pass(50);
        players.iter_mut().for_each(Player::pump);
        let _ = step;
    }
    settle(&net, &mut players);
    // A last audit of every brick: orders that differ get mended.
    let all = players[0].editor.bricks.len();
    players[0].audit(all);
    settle(&net, &mut players);

    let conflicts: usize = players.iter().map(|p| p.conflicts).sum();
    CONFLICTS.fetch_add(conflicts, std::sync::atomic::Ordering::Relaxed);
    let misordered: usize = players.iter().map(|p| p.misordered).sum();
    MISORDERED.fetch_add(misordered, std::sync::atomic::Ordering::Relaxed);
    let truth = players[0].editor.state();
    for player in &players[1..] {
        assert!(
            player.session.is_loaded(),
            "seed {seed}: {} never got the vehicle",
            player.peer
        );
        assert_eq!(
            player.differences,
            vec![],
            "seed {seed}: {} found differences that weren't there",
            player.peer
        );
        assert_eq!(player.drifts, 0, "seed {seed}: {} drifted", player.peer);
        assert_eq!(
            player.editor.ids(),
            players[0].editor.ids(),
            "seed {seed}: {} has another object order",
            player.peer
        );
        let state = player.editor.state();
        if state != truth {
            panic!(
                "seed {seed}: {} differs from the subhost: {}",
                player.peer,
                describe_difference(&state, &truth)
            );
        }
    }
}

type State = BTreeMap<CoopId, Brick>;

fn describe_difference(bricks: &State, truth_bricks: &State) -> String {
    let ids: std::collections::BTreeSet<&CoopId> =
        bricks.keys().chain(truth_bricks.keys()).collect();
    for id in ids {
        let (a, b) = (bricks.get(id), truth_bricks.get(id));
        if a != b {
            return format!("brick {id:?}: {a:?}, subhost {b:?}");
        }
    }
    String::new()
}

#[test]
fn members_converge() {
    let seeds = std::env::var("COOP_SIM_SEEDS").map_or(300, |v| v.parse().unwrap());
    let first = std::env::var("COOP_SIM_FIRST").map_or(0, |v| v.parse().unwrap());
    for seed in first..first + seeds {
        run(seed, 1 + (seed as usize % 4), 400);
    }
    eprintln!(
        "{seeds} runs: {} snapshots over undo conflicts, {} object orders mended",
        CONFLICTS.load(std::sync::atomic::Ordering::Relaxed),
        MISORDERED.load(std::sync::atomic::Ordering::Relaxed)
    );
}

#[test]
fn many_members_long_session() {
    for seed in 1000..1010 {
        run(seed, 6, 3000);
    }
}

#[test]
fn same_property_last_commit_wins() {
    let net = MemoryNet::new();
    let mut players: Vec<Player> = (0..3).map(|i| Player::new(&net, HOST + i)).collect();
    let mut rng = Rng(7);
    players[0].editor = starting_vehicle(&mut rng, 1);
    let ids = players[0]
        .session
        .host(
            1,
            "h".into(),
            HostSettings {
                code: "c".into(),
                max_members: 4,
            },
            1,
        )
        .unwrap();
    players[0].editor.bricks[0].0 = ids[0];
    for p in &mut players[1..] {
        p.session.join(HOST, "c".into(), "g".into()).unwrap();
    }
    settle(&net, &mut players);

    // All three set the same property before hearing of each other.
    for (i, p) in players.iter_mut().enumerate() {
        let op = Op::Set {
            id: ids[0],
            property: "Color".into(),
            value: Value::Raw(vec![i as u8]),
        };
        p.editor.apply(&op);
        p.session.edit(vec![op]).unwrap();
        p.pump();
    }
    // Deliver the guests' proposals in reverse order, then everything else.
    net.borrow_mut().deliver_one((HOST + 2, HOST));
    net.borrow_mut().deliver_one((HOST + 1, HOST));
    settle(&net, &mut players);

    let values: Vec<&Value> = players
        .iter()
        .map(|p| &p.editor.bricks[0].1.props["Color"])
        .collect();
    assert_eq!(
        values,
        vec![&Value::Raw(vec![1]); 3],
        "guest 1's proposal was committed last"
    );
}

#[test]
fn order_converges_without_concurrent_creates() {
    let net = MemoryNet::new();
    let mut players: Vec<Player> = (0..2).map(|i| Player::new(&net, HOST + i)).collect();
    let mut rng = Rng(3);
    players[0].editor = starting_vehicle(&mut rng, 4);
    let ids = players[0]
        .session
        .host(
            1,
            "h".into(),
            HostSettings {
                code: "c".into(),
                max_members: 4,
            },
            4,
        )
        .unwrap();
    for ((id, _), new) in players[0].editor.bricks.iter_mut().zip(&ids) {
        *id = *new;
    }
    players[1]
        .session
        .join(HOST, "c".into(), "g".into())
        .unwrap();
    settle(&net, &mut players);

    let order = Op::Order {
        ids: vec![ids[3], ids[1]],
    };
    players[1].editor.apply(&order);
    players[1].session.edit(vec![order]).unwrap();
    let delete = Op::Delete { id: ids[1] };
    players[0].editor.apply(&delete);
    players[0].session.edit(vec![delete]).unwrap();
    settle(&net, &mut players);

    let order_of = |p: &Player| {
        p.editor
            .bricks
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order_of(&players[0]), vec![ids[3], ids[0], ids[2]]);
    assert_eq!(order_of(&players[1]), order_of(&players[0]));
}

// --- Audits ---

#[test]
fn audit_finds_a_drifted_editor() {
    let (net, mut players, ids) = session_of(21, 1, 3);
    // Changes the mod's hooks missed, each in only one editor.
    let drifts = [
        Op::Move {
            id: ids[1],
            transform: place(50.0),
        },
        Op::Set {
            id: ids[2],
            property: "Color".into(),
            value: Value::Raw(vec![99]),
        },
        Op::Group {
            id: ids[0],
            kind: GroupKind::Editor,
            group: 7,
        },
        Op::Delete { id: ids[2] },
    ];
    for (round, drift) in drifts.iter().enumerate() {
        players[1].editor.apply(drift);
        // Audits go round: it may take a few to come to the brick.
        for _ in 0..3 {
            players[0].audit(1);
            settle(&net, &mut players);
        }
        assert_eq!(players[1].drifts, round + 1, "{drift:?}");
        assert!(players[1].editor.state() == players[0].editor.state());
        assert_eq!(players[1].editor.ids(), players[0].editor.ids());
    }
    let differences = &players[1].differences;
    assert!(differences.contains(&Difference::Place(ids[1])));
    assert!(differences.contains(&Difference::Values(ids[2])));
    assert!(differences.contains(&Difference::Groups(ids[0])));
    assert!(
        differences
            .iter()
            .any(|d| matches!(d, Difference::Count { .. }))
    );
}

#[test]
fn audit_mends_the_order_of_bricks_made_at_once() {
    let (net, mut players, _) = session_of(22, 1, 2);
    let mut rng = Rng(5);
    // Both make a brick before hearing of the other's: each has its own first.
    for player in &mut players {
        let change = player.random_create(&mut rng);
        player.session.edit_undoable(vec![change]).unwrap();
        player.pump();
    }
    settle(&net, &mut players);
    assert!(players[1].editor.state() == players[0].editor.state());
    assert_ne!(players[1].editor.ids(), players[0].editor.ids());
    players[0].audit(4);
    settle(&net, &mut players);
    assert_eq!(players[1].misordered, 1);
    assert_eq!(players[1].drifts, 0, "no snapshot for an order");
    assert_eq!(players[1].editor.ids(), players[0].editor.ids());
    // Its own steps survive: undoing its brick still works.
    assert!(players[1].step_back(false));
    settle(&net, &mut players);
    all_equal(&players);
}

#[test]
fn audits_wait_for_own_edits() {
    let (net, mut players, ids) = session_of(23, 1, 2);
    // The member's move is on its way when the subhost audits: no comparing then.
    edit(
        &mut players,
        1,
        vec![Op::Move {
            id: ids[0],
            transform: place(3.0),
        }],
    );
    players[0].audit(2);
    net.borrow_mut().deliver_one((HOST, HOST + 1));
    players[1].pump();
    settle(&net, &mut players);
    assert_eq!(players[1].differences, vec![]);
    assert_eq!(players[1].drifts, 0);
    all_equal(&players);
}

#[test]
fn wrong_check_asks_for_a_snapshot() {
    // A hand-driven subhost that sends a commit whose check doesn't match.
    let mut guest = Session::new(info());
    guest.join(HOST, "c".into(), "g".into()).unwrap();
    let id = CoopId::new(0, 1).unwrap();
    for message in [
        Message::JoinAccepted { slot: 2 },
        Message::Snapshot {
            commit: 0,
            ids: vec![id],
            vehicle: vec![],
        },
        Message::Commit {
            number: 1,
            author: 1,
            local: 0,
            ops: vec![],
            check: coop::Check { count: 5, hash: 1 },
        },
    ] {
        guest.receive(HOST, &message.encode(42));
    }
    assert!(guest.take_events().contains(&Event::Drifted));
    let sent: Vec<Message> = guest
        .take_outgoing()
        .iter()
        .map(|o| Message::decode(&o.bytes).unwrap().1)
        .collect();
    assert_eq!(sent.last(), Some(&Message::Resync));

    // Then the snapshot clears it, and the next good commit is fine.
    guest.receive(
        HOST,
        &Message::Snapshot {
            commit: 1,
            ids: vec![id],
            vehicle: vec![],
        }
        .encode(42),
    );
    let check = coop::Check::of([id]);
    guest.receive(
        HOST,
        &Message::Commit {
            number: 2,
            author: 1,
            local: 0,
            ops: vec![],
            check,
        }
        .encode(42),
    );
    assert!(!guest.take_events().contains(&Event::Drifted));
}

#[test]
fn resync_after_missed_change() {
    let net = MemoryNet::new();
    let mut players: Vec<Player> = (0..2).map(|i| Player::new(&net, HOST + i)).collect();
    let mut rng = Rng(11);
    players[0].editor = starting_vehicle(&mut rng, 3);
    let ids = players[0]
        .session
        .host(
            1,
            "h".into(),
            HostSettings {
                code: "c".into(),
                max_members: 4,
            },
            3,
        )
        .unwrap();
    for ((id, _), new) in players[0].editor.bricks.iter_mut().zip(&ids) {
        *id = *new;
    }
    players[1]
        .session
        .join(HOST, "c".into(), "g".into())
        .unwrap();
    settle(&net, &mut players);

    // A change the mod's hooks missed: only the guest's editor has it.
    players[1].editor.apply(&Op::Delete { id: ids[0] });
    players[1].session.request_resync();
    settle(&net, &mut players);
    assert_eq!(players[1].drifts, 1);
    assert!(players[1].editor.state() == players[0].editor.state());
}

#[test]
fn big_vehicle_snapshot_in_parts() {
    let net = MemoryNet::new();
    let mut players: Vec<Player> = (0..2).map(|i| Player::new(&net, HOST + i)).collect();
    let mut rng = Rng(5);
    let n = SNAPSHOT_IDS.min(40_000);
    players[0].editor = starting_vehicle(&mut rng, n);
    let ids = players[0]
        .session
        .host(
            1,
            "h".into(),
            HostSettings {
                code: "c".into(),
                max_members: 4,
            },
            n,
        )
        .unwrap();
    for ((id, _), new) in players[0].editor.bricks.iter_mut().zip(&ids) {
        *id = *new;
    }
    players[1]
        .session
        .join(HOST, "c".into(), "g".into())
        .unwrap();
    settle(&net, &mut players);
    assert!(players[1].editor.state() == players[0].editor.state());
}

// --- Undo ---

/// One member edits, undoes and redoes at random while another watches: every undo brings back
/// exactly the vehicle before the step (object order included), every redo the one after, and both
/// members end equal.
#[test]
fn undo_and_redo_restore_own_steps() {
    let seeds = std::env::var("COOP_SIM_SEEDS").map_or(300, |v| v.parse().unwrap());
    let first = std::env::var("COOP_SIM_FIRST").map_or(0, |v| v.parse().unwrap());
    for seed in first..first + seeds {
        let mut rng = Rng((seed as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
        let (net, mut players, _) = session_of(seed as u64, 1, rng.below(6));
        // The subhost or the member.
        let actor = (seed % 2) as usize;
        type Bricks = Vec<(CoopId, Brick)>;
        let mut done: Vec<(Bricks, Bricks)> = Vec::new();
        let mut undone: Vec<(Bricks, Bricks)> = Vec::new();
        for step in 0..150 {
            let roll = rng.below(10);
            let player = &mut players[actor];
            if roll < 5 {
                let before = player.editor.bricks.clone();
                if !player.random_edit(&mut rng) {
                    continue;
                }
                done.push((before, player.editor.bricks.clone()));
                if done.len() > coop::undo::caps::STEPS {
                    done.remove(0);
                }
                undone.clear();
            } else if roll < 8 {
                let had = player.step_back(false);
                assert_eq!(had, !done.is_empty(), "seed {seed}, step {step}");
                if let Some((before, after)) = done.pop() {
                    assert!(
                        player.editor.bricks == before,
                        "seed {seed}, step {step}: undo gave {:?}, not {before:?}",
                        player.editor.bricks
                    );
                    undone.push((before, after));
                }
            } else if roll < 9 {
                let had = player.step_back(true);
                assert_eq!(had, !undone.is_empty(), "seed {seed}, step {step}");
                if let Some((before, after)) = undone.pop() {
                    assert!(
                        player.editor.bricks == after,
                        "seed {seed}, step {step}: redo gave {:?}, not {after:?}",
                        player.editor.bricks
                    );
                    done.push((before, after));
                }
            } else {
                let pairs = net.borrow().busy_pairs();
                if !pairs.is_empty() {
                    net.borrow_mut().deliver_one(pairs[rng.below(pairs.len())]);
                }
            }
            players.iter_mut().for_each(Player::pump);
        }
        settle(&net, &mut players);
        assert_eq!(players[1].drifts, 0, "seed {seed}");
        assert!(
            players[0].editor.state() == players[1].editor.state(),
            "seed {seed}: {}",
            describe_difference(&players[1].editor.state(), &players[0].editor.state())
        );
    }
}

fn place(x: f32) -> Transform {
    Transform {
        location: [x, 0.0, 0.0],
        rotation: [0.0; 3],
    }
}

fn all_equal(players: &[Player]) {
    for player in &players[1..] {
        assert_eq!(player.drifts, 0, "{} drifted", player.peer);
        assert!(
            player.editor.state() == players[0].editor.state(),
            "{}: {}",
            player.peer,
            describe_difference(&player.editor.state(), &players[0].editor.state())
        );
    }
}

/// Makes `ops` at `p` as one undoable edit.
fn edit(players: &mut [Player], p: usize, ops: Vec<Op>) {
    let changes = ops.into_iter().flat_map(|op| players[p].make(op)).collect();
    players[p].session.edit_undoable(changes).unwrap();
    players[p].pump();
}

#[test]
fn undo_keeps_what_others_changed_since() {
    let (net, mut players, ids) = session_of(1, 1, 2);
    let x = ids[0];
    let color = players[0].editor.bricks[0].1.props["Color"].clone();
    edit(
        &mut players,
        1,
        vec![
            Op::Move {
                id: x,
                transform: place(1.0),
            },
            Op::Set {
                id: x,
                property: "Color".into(),
                value: Value::Raw(vec![7]),
            },
        ],
    );
    settle(&net, &mut players);
    edit(
        &mut players,
        0,
        vec![Op::Move {
            id: x,
            transform: place(2.0),
        }],
    );
    settle(&net, &mut players);
    assert!(players[1].step_back(false));
    settle(&net, &mut players);
    all_equal(&players);
    let brick = players[0].editor.find(x).unwrap();
    assert_eq!(brick.transform, place(2.0), "the subhost moved it last");
    assert_eq!(brick.props["Color"], color, "nobody else changed the color");
}

#[test]
fn undo_brings_back_what_it_overwrote() {
    let (net, mut players, ids) = session_of(2, 1, 1);
    let x = ids[0];
    // The member's move is still on its way when the subhost's is committed: the member's comes
    // after it, so undoing the member's gives the subhost's back.
    edit(
        &mut players,
        1,
        vec![Op::Move {
            id: x,
            transform: place(1.0),
        }],
    );
    edit(
        &mut players,
        0,
        vec![Op::Move {
            id: x,
            transform: place(2.0),
        }],
    );
    net.borrow_mut().deliver_one((HOST, HOST + 1));
    players.iter_mut().for_each(Player::pump);
    settle(&net, &mut players);
    assert_eq!(players[0].editor.find(x).unwrap().transform, place(1.0));
    assert!(players[1].step_back(false));
    settle(&net, &mut players);
    all_equal(&players);
    assert_eq!(players[0].editor.find(x).unwrap().transform, place(2.0));
}

#[test]
fn undo_delete_brings_the_same_brick_back() {
    let (net, mut players, ids) = session_of(3, 1, 4);
    let (x, y) = (ids[1], ids[3]);
    // Brick y refers to x; x is in groups, and not last in the object order.
    edit(
        &mut players,
        0,
        vec![
            Op::Set {
                id: y,
                property: REFERENCES.into(),
                value: Value::Bricks(vec![x, ids[0]]),
            },
            Op::Group {
                id: x,
                kind: GroupKind::Weld,
                group: 3,
            },
        ],
    );
    settle(&net, &mut players);
    let start = players[0].editor.bricks.clone();

    // The member deletes the subhost's brick, then undoes it.
    edit(&mut players, 1, vec![Op::Delete { id: x }]);
    settle(&net, &mut players);
    assert!(players[0].editor.find(x).is_none());
    assert!(players[1].step_back(false));
    settle(&net, &mut players);
    all_equal(&players);
    assert!(
        players[0].editor.bricks == start,
        "{:?}",
        players[0].editor.bricks
    );
    assert!(players[1].editor.bricks == start);

    // And redoes it.
    assert!(players[1].step_back(true));
    settle(&net, &mut players);
    all_equal(&players);
    assert!(players[0].editor.find(x).is_none());
    assert_eq!(
        players[0].editor.find(y).unwrap().props[REFERENCES],
        Value::Bricks(vec![ids[0]])
    );
}

#[test]
fn undo_create_leaves_bricks_others_changed() {
    let (net, mut players, _) = session_of(4, 1, 1);
    let mut rng = Rng(9);
    let change = players[1].random_create(&mut rng);
    let Op::Create { ids, .. } = &change.done[0] else {
        unreachable!()
    };
    let z = ids[0];
    players[1].session.edit_undoable(vec![change]).unwrap();
    settle(&net, &mut players);
    edit(
        &mut players,
        0,
        vec![Op::Set {
            id: z,
            property: "Color".into(),
            value: Value::Raw(vec![5]),
        }],
    );
    settle(&net, &mut players);
    let back = players[1].session.undo().unwrap().unwrap();
    assert_eq!(back.skipped, 1);
    assert!(
        back.ops
            .iter()
            .all(|op| !matches!(op, Op::Delete { id } if *id == z))
    );
    settle(&net, &mut players);
    all_equal(&players);
    assert!(players[0].editor.find(z).is_some());
}

// --- Limits ---

/// A subhost of one brick (session 1) that `peer` joined: the join's snapshot request, and the
/// brick's id.
fn hosting_with(peer: PeerId) -> (Session, SnapshotRequest, CoopId) {
    let mut host = Session::new(info());
    let settings = HostSettings {
        code: "c".into(),
        max_members: 8,
    };
    let ids = host.host(1, "h".into(), settings, 1).unwrap();
    host.receive(peer, &join("c").encode(0));
    let request = host
        .take_events()
        .into_iter()
        .find_map(|event| match event {
            Event::SnapshotWanted(request) => Some(request),
            _ => None,
        })
        .unwrap();
    host.take_outgoing();
    (host, request, ids[0])
}

fn join(code: &str) -> Message {
    Message::Join {
        code: code.into(),
        name: "g".into(),
        game_version: info().game_version,
    }
}

fn sent(session: &mut Session) -> Vec<Message> {
    session
        .take_outgoing()
        .iter()
        .map(|o| Message::decode(&o.bytes).unwrap().1)
        .collect()
}

#[test]
fn a_flooding_member_is_kicked_for_good() {
    let (mut host, _, id) = hosting_with(5);
    let propose = |local: u32| Message::Propose {
        local,
        ops: vec![Op::Move {
            id,
            transform: place(local as f32),
        }],
    };
    for local in 1..=coop::limits::rates::MESSAGES.burst as u32 {
        host.receive(5, &propose(local).encode(1));
    }
    assert!(
        !host
            .take_events()
            .iter()
            .any(|e| matches!(e, Event::Flooded { .. }))
    );
    host.take_outgoing();
    host.receive(5, &propose(1000).encode(1));
    assert!(host.take_events().contains(&Event::Flooded {
        peer: 5,
        what: coop::limits::Flood::Messages
    }));
    assert_eq!(host.members().len(), 1);
    assert!(sent(&mut host).contains(&Message::Kick));
    // It can't come back, even much later.
    host.tick(3_600_000);
    host.receive(5, &join("c").encode(0));
    assert_eq!(host.take_events(), vec![]);
    assert_eq!(host.take_outgoing(), vec![]);
}

#[test]
fn bricks_made_past_the_allowance_get_a_member_kicked() {
    let (mut host, _, _) = hosting_with(5);
    let made = |first: u32, n: u32| Message::Propose {
        local: first,
        ops: vec![Op::Create {
            ids: (first..first + n)
                .map(|c| CoopId::new(2, c).unwrap())
                .collect(),
            archive: vec![],
        }],
    };
    host.receive(5, &made(1, 60_000).encode(1));
    host.tick(1000);
    host.receive(5, &made(60_001, 10_000).encode(1));
    assert!(
        !host
            .take_events()
            .iter()
            .any(|e| matches!(e, Event::Flooded { .. }))
    );
    host.receive(5, &made(70_001, 10_000).encode(1));
    assert!(host.take_events().contains(&Event::Flooded {
        peer: 5,
        what: coop::limits::Flood::Made
    }));
}

#[test]
fn snapshots_past_the_allowance_wait_their_turn() {
    let (mut host, first, id) = hosting_with(5);
    host.send_snapshot(first, vec![id], vec![]).unwrap();
    // The join took one; two more go at once.
    for _ in 0..2 {
        host.receive(5, &Message::Resync.encode(1));
        let Some(Event::SnapshotWanted(request)) = host.take_events().pop() else {
            panic!("no snapshot asked for");
        };
        host.send_snapshot(request, vec![id], vec![]).unwrap();
    }
    host.take_outgoing();
    // The next one waits five seconds.
    host.receive(5, &Message::Resync.encode(1));
    assert_eq!(host.take_events(), vec![]);
    host.tick(4_999);
    assert_eq!(host.take_events(), vec![]);
    host.tick(5_000);
    let Some(Event::SnapshotWanted(request)) = host.take_events().pop() else {
        panic!("the snapshot never went");
    };
    host.send_snapshot(request, vec![id], vec![]).unwrap();
    assert!(
        sent(&mut host)
            .iter()
            .any(|m| matches!(m, Message::Snapshot { .. }))
    );
    // Meanwhile it got no commits: the snapshot holds them.
    host.receive(5, &Message::Resync.encode(1));
    host.edit(vec![Op::Move {
        id,
        transform: place(1.0),
    }])
    .unwrap();
    assert_eq!(host.take_outgoing(), vec![]);
}

#[test]
fn guessing_the_code_runs_out() {
    let mut host = Session::new(info());
    let settings = HostSettings {
        code: "c".into(),
        max_members: 8,
    };
    host.host(1, "h".into(), settings, 0).unwrap();
    for _ in 0..coop::limits::rates::WRONG_CODES {
        host.receive(7, &join("x").encode(0));
        assert_eq!(
            sent(&mut host),
            vec![Message::JoinRefused(coop::RefuseReason::WrongCode)]
        );
    }
    host.receive(7, &join("c").encode(0));
    assert_eq!(host.take_outgoing(), vec![]);
    // Others still can.
    host.receive(8, &join("c").encode(0));
    assert_eq!(host.members().len(), 2);
}

#[test]
fn hellos_past_the_allowance_go_unanswered() {
    let mut session = Session::new(info());
    for _ in 0..20 {
        session.receive(9, &Message::Hello(info()).encode(0));
    }
    assert_eq!(session.take_outgoing().len(), 5);
    session.tick(2_000);
    session.receive(9, &Message::Hello(info()).encode(0));
    assert_eq!(session.take_outgoing().len(), 1);
}
