//! Sharing references over real sessions: a subhost with `Sharing` and members with `Receiving`,
//! on an in-memory network that delivers each pair's messages in order but interleaves pairs at
//! random, with a simulated clock. Whatever happens (lists changing while data flows, members
//! leaving, levels changing), every member ends with exactly the bytes the subhost holds for what
//! its level allows. A subhost that lies gets nowhere: what it sends is checked by the member.
//!
//! Everything is deterministic from the seed. Environment variables:
//!
//! - `COOP_REFS_SEEDS`: how many seeds the sweep runs (a small default),
//! - `COOP_REFS_FIRST`: the first seed, to repeat a failure with `COOP_REFS_SEEDS=1`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use coop::framing;
use coop::limits::Flood;
use coop::message::HEADER_LEN;
use coop::references::{
    CHUNK, Entry, Finished, ImageLook, Kind, Level, Receiving, Sharing, Size, Skip, Status, split,
};
use coop::transport::{MemoryEndpoint, MemoryNet, Transport};
use coop::{EndReason, Event, HostSettings, Message, PeerId, PeerInfo, Session};

const HOST: PeerId = 100;
const SESSION: u32 = 7;
const CODE: &str = "code";

fn info() -> PeerInfo {
    PeerInfo {
        protocol: (1, 1),
        mod_version: "refs".into(),
        game_version: "1".into(),
        hosting: false,
    }
}

fn settings() -> HostSettings {
    HostSettings {
        code: CODE.into(),
        max_members: 8,
        approve: false,
    }
}

// --- Randomness (no dependency, repeatable) ---

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.next() % (high - low + 1)
    }
}

/// `len` bytes that depend on `seed` and look random.
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| (rng.next() >> 20) as u8).collect()
}

fn mix(values: &[u64]) -> u64 {
    values.iter().fold(0xCBF2_9CE4_8422_2325, |hash, &v| {
        (hash ^ v).wrapping_mul(0x0100_0000_01B3).rotate_left(17)
    })
}

// --- What a subhost has and makes of it ---

/// A reference as the subhost's editor has it.
#[derive(Clone)]
struct Item {
    id: String,
    kind: Kind,
    triangles: u64,
    pixels: u64,
    /// The size of its data before it is fitted to a level.
    bytes: u32,
    seed: u64,
}

fn new_item(rng: &mut Rng, counter: &mut u64) -> Item {
    *counter += 1;
    let seed = *counter;
    let image = rng.below(3) == 0;
    Item {
        id: format!("ref_{seed}"),
        kind: if image { Kind::Image } else { Kind::Model },
        triangles: if image {
            0
        } else {
            rng.between(1_000, 200_000)
        },
        pixels: if image {
            rng.between(100_000, 8_000_000)
        } else {
            rng.between(0, 2_000_000)
        },
        bytes: rng.between(200, 12_000) as u32,
        seed,
    }
}

/// What the subhost makes of `items` for `level`: the list, and the data of each content. Like
/// the mod: fitted to the level by `split`, and the data is named by what it is.
fn cook(level: Level, items: &[Item]) -> (Vec<Entry>, Vec<(u64, Vec<u8>)>) {
    let caps = level.caps();
    let sizes: Vec<Size> = items
        .iter()
        .map(|item| Size {
            triangles: item.triangles,
            pixels: item.pixels,
        })
        .collect();
    let (mut entries, mut data) = (Vec::new(), Vec::new());
    for (item, target) in items.iter().zip(split(caps, &sizes)).take(caps.references) {
        let bytes = u64::from(item.bytes).min(target.bytes).max(1) as u32;
        let content = mix(&[
            item.seed,
            u64::from(Level::to_wire(Some(level))),
            target.triangles,
            target.pixels,
            u64::from(bytes),
        ]);
        entries.push(Entry {
            id: item.id.clone(),
            name: format!("Reference {}", item.seed),
            kind: item.kind,
            content,
            bytes,
            triangles: target.triangles as u32,
            pixels: target.pixels as u32,
            simplified: target.triangles < item.triangles || target.pixels < item.pixels,
            position: [item.seed as f64, -1.0, 2.5],
            rotation: [0.0, 90.0, 0.0],
            scale: [1.0; 3],
            mirror: [false; 3],
            opacity: 1.0,
            color: [1.0; 3],
            image: (item.kind == Kind::Image).then_some(ImageLook {
                width: 100.0,
                flip_x: false,
                flip_y: false,
                double_sided: true,
                pixelated: false,
                pivot: 0,
            }),
        });
        data.push((content, payload(content, bytes as usize)));
    }
    (entries, data)
}

/// What a level allows of `entries`, worked out here again, in list order.
fn allowed(level: Level, entries: &[Entry]) -> Vec<&Entry> {
    let caps = level.caps();
    let (mut triangles, mut pixels) = (0u64, 0u64);
    let mut taken = Vec::new();
    for entry in entries {
        let fits = taken.len() < caps.references
            && u64::from(entry.bytes) <= caps.burst_bytes
            && triangles + u64::from(entry.triangles) <= caps.triangles
            && pixels + u64::from(entry.pixels) <= caps.pixels;
        if fits {
            triangles += u64::from(entry.triangles);
            pixels += u64::from(entry.pixels);
            taken.push(entry);
        }
    }
    taken
}

// --- The world ---

struct HostSide {
    session: Session,
    transport: MemoryEndpoint,
    sharing: Sharing,
    /// The data of every content it ever cooked.
    contents: HashMap<u64, Vec<u8>>,
    /// The list at each level, as published.
    lists: HashMap<Level, Vec<Entry>>,
}

impl HostSide {
    fn publish(&mut self, items: &[Item]) {
        for level in Level::ALL {
            let (entries, data) = cook(level, items);
            self.contents.extend(data);
            self.sharing.publish(level, entries.clone());
            self.lists.insert(level, entries);
        }
    }
}

struct Member {
    peer: PeerId,
    session: Session,
    transport: MemoryEndpoint,
    receiving: Receiving,
    /// Contents the worker checked and loaded.
    loaded: HashMap<u64, Vec<u8>>,
    /// Finished downloads the worker is still on, and when it is done with each.
    checking: Vec<(u64, Finished)>,
    left: bool,
}

struct World {
    net: Rc<RefCell<MemoryNet>>,
    now: u64,
    rng: Rng,
    host: HostSide,
    members: Vec<Member>,
    /// Sizes of the frames in flight from one peer to another: a mirror of the net's queues, for
    /// the subhost to tell how much waits for a member.
    flights: HashMap<(PeerId, PeerId), VecDeque<usize>>,
    /// Frames ever sent, per pair.
    frames: HashMap<(PeerId, PeerId), usize>,
}

impl World {
    /// A subhost and members that joined, each at its level.
    fn new(seed: u64, levels: &[Option<Level>]) -> Self {
        let net = MemoryNet::new();
        let mut host = HostSide {
            session: Session::new(info()),
            transport: MemoryNet::endpoint(&net, HOST),
            sharing: Sharing::default(),
            contents: HashMap::new(),
            lists: HashMap::new(),
        };
        host.session
            .host(SESSION, "host".into(), settings(), 0)
            .unwrap();
        let members = levels
            .iter()
            .enumerate()
            .map(|(i, &level)| {
                let peer = HOST + 1 + i as u64;
                let mut session = Session::new(info());
                session.join(HOST, CODE.into(), format!("m{i}")).unwrap();
                Member {
                    peer,
                    session,
                    transport: MemoryNet::endpoint(&net, peer),
                    receiving: Receiving::new(level, 0),
                    loaded: HashMap::new(),
                    checking: Vec::new(),
                    left: false,
                }
            })
            .collect();
        let mut world = Self {
            net,
            now: 0,
            rng: Rng::new(seed),
            host,
            members,
            flights: HashMap::new(),
            frames: HashMap::new(),
        };
        for _ in 0..100 {
            world.step(50);
        }
        assert_eq!(world.host.session.members().len(), levels.len() + 1);
        world
    }

    fn live(&self) -> Vec<usize> {
        (0..self.members.len())
            .filter(|&i| !self.members[i].left)
            .collect()
    }

    /// Moves what the sessions want sent onto the network.
    fn flush(&mut self) {
        let mut out = vec![];
        out.extend(
            self.host
                .session
                .take_outgoing()
                .into_iter()
                .map(|o| (HOST, o)),
        );
        for member in &mut self.members {
            let peer = member.peer;
            out.extend(
                member
                    .session
                    .take_outgoing()
                    .into_iter()
                    .map(|o| (peer, o)),
            );
        }
        for (from, o) in out {
            let pair = (from, o.to);
            self.flights
                .entry(pair)
                .or_default()
                .push_back(o.bytes.len());
            *self.frames.entry(pair).or_default() += 1;
            let transport = match from {
                HOST => &mut self.host.transport,
                peer => &mut self.members[(peer - HOST - 1) as usize].transport,
            };
            transport.send(o.to, &o.bytes, o.reliable);
        }
    }

    /// Delivers up to `most` frames, from pairs picked at random.
    fn deliver(&mut self, most: usize) {
        for _ in 0..most {
            let pairs = self.net.borrow().busy_pairs();
            if pairs.is_empty() {
                return;
            }
            let pair = pairs[self.rng.below(pairs.len())];
            self.net.borrow_mut().deliver_one(pair);
            if let Some(sizes) = self.flights.get_mut(&pair) {
                sizes.pop_front();
            }
        }
    }

    /// `ms` milliseconds pass: some frames arrive, everyone does what the mod would, and what
    /// they send goes out.
    fn step(&mut self, ms: u64) {
        self.now += ms;
        self.host.session.tick(self.now);
        for member in &mut self.members {
            member.session.tick(self.now);
        }
        let most = 1 + self.rng.below(8);
        self.deliver(most);
        self.host_step();
        for i in 0..self.members.len() {
            self.member_step(i);
        }
        self.flush();
    }

    fn host_step(&mut self) {
        let host = &mut self.host;
        for (from, bytes) in host.transport.receive() {
            host.session.receive(from, &bytes);
        }
        for event in host.session.take_events() {
            match event {
                Event::ReferenceLevelAsked { peer, level } => {
                    host.sharing.level_asked(peer, level, self.now)
                }
                Event::ReferenceWanted { peer, content } => host.sharing.wanted(peer, content),
                Event::Members(_) => host.sharing.keep_members(&host.session.expected_peers()),
                Event::BadMessage { peer, error } => panic!("junk from {peer}: {error}"),
                _ => {}
            }
        }
        for (peer, entries) in host.sharing.lists(self.now) {
            host.session.send_reference_list(peer, entries);
        }
        let flights = &self.flights;
        let contents = &host.contents;
        let chunks = host.sharing.chunks(
            self.now,
            |peer| {
                let waiting = flights.get(&(HOST, peer));
                Some(waiting.map_or(0, |sizes| sizes.iter().sum()))
            },
            |content| contents.get(&content).map(Vec::as_slice),
        );
        for chunk in chunks {
            host.session.send_reference_data(chunk);
        }
    }

    fn member_step(&mut self, i: usize) {
        let member = &mut self.members[i];
        if member.left {
            return;
        }
        for (from, bytes) in member.transport.receive() {
            member.session.receive(from, &bytes);
        }
        for event in member.session.take_events() {
            match event {
                Event::Joined { .. } => member.session.reference_level(member.receiving.level()),
                Event::ReferenceList(entries) => member.receiving.list(entries),
                Event::ReferenceData {
                    content,
                    offset,
                    data,
                } => {
                    if let Some(done) = member.receiving.data(content, offset, &data, self.now) {
                        let ready_at = self.now + self.rng.below(500) as u64;
                        member.checking.push((ready_at, done));
                    }
                }
                Event::Ended(reason) => {
                    assert_eq!(reason, EndReason::Left);
                    member.left = true;
                }
                Event::BadMessage { peer, error } => panic!("junk from {peer}: {error}"),
                _ => {}
            }
        }
        // The worker: what it finished is exactly what the subhost has, then it's loaded.
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut member.checking)
            .into_iter()
            .partition(|(at, _)| *at <= self.now);
        member.checking = later;
        for (_, Finished { content, bytes }) in due {
            assert_eq!(
                self.host.contents.get(&content),
                Some(&bytes),
                "member {i} got other bytes than the subhost holds for {content:x}"
            );
            member.loaded.insert(content, bytes);
            member.receiving.ready(content);
        }
        if let Some(content) = member.receiving.next_want(self.now) {
            member.session.want_reference(content);
        }
    }

    /// Member `i` takes another level, as the mod does.
    fn set_level(&mut self, i: usize, level: Option<Level>) {
        let member = &mut self.members[i];
        member.receiving.set_level(level, self.now);
        member.session.reference_level(level);
    }

    fn leave(&mut self, i: usize) {
        self.members[i].session.leave();
    }

    /// What member `i` should show once everything arrived.
    fn expected(&self, i: usize) -> Vec<&Entry> {
        match self.members[i].receiving.level() {
            None => Vec::new(),
            Some(level) => allowed(level, &self.host.lists[&level]),
        }
    }

    fn is_done(&self) -> bool {
        self.live().into_iter().all(|i| {
            let member = &self.members[i];
            let shown: Vec<(&str, u64)> = member
                .receiving
                .shown()
                .iter()
                .map(|e| (e.id.as_str(), e.content))
                .collect();
            let expected: Vec<(&str, u64)> = self
                .expected(i)
                .iter()
                .map(|e| (e.id.as_str(), e.content))
                .collect();
            shown == expected && member.checking.is_empty()
        })
    }

    /// Runs until every live member shows what it should and stays that way for 20 seconds,
    /// which lets level changes that waited for their turn go through.
    fn settle(&mut self) {
        let mut good = 0;
        for _ in 0..40_000 {
            self.step(50);
            if self.is_done() {
                good += 1;
                if good == 400 {
                    return;
                }
            } else {
                good = 0;
            }
        }
        panic!("never settled; {}", self.describe());
    }

    fn describe(&self) -> String {
        self.live()
            .into_iter()
            .map(|i| {
                let member = &self.members[i];
                let list = member.receiving.entries();
                let waiting = list.iter().filter(|(_, s)| *s == Status::Waiting).count();
                format!(
                    "member {i} at {:?}: {} listed, {waiting} waiting, {} shown, {} expected",
                    member.receiving.level(),
                    list.len(),
                    member.receiving.shown().len(),
                    self.expected(i).len(),
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// What every live member holds is what the level allows, byte for byte, and the subhost
    /// never sent more than a member's level takes.
    fn check(&self) {
        assert!(self.is_done(), "{}", self.describe());
        for i in self.live() {
            let member = &self.members[i];
            let expected = self.expected(i);
            for (entry, status) in member.receiving.entries() {
                assert_eq!(status, Status::Ready, "{}", entry.id);
            }
            for entry in expected {
                assert_eq!(
                    member.loaded.get(&entry.content),
                    self.host.contents.get(&entry.content)
                );
            }
        }
    }
}

/// Items for `n` references.
fn new_items(rng: &mut Rng, counter: &mut u64, n: usize) -> Vec<Item> {
    (0..n).map(|_| new_item(rng, counter)).collect()
}

// --- Tests: the honest subhost ---

#[test]
fn members_get_what_their_level_allows() {
    let mut world = World::new(1, &[Some(Level::High), Some(Level::Extreme)]);
    let mut counter = 0;
    let items = new_items(&mut Rng::new(5), &mut counter, 90);
    world.host.publish(&items);
    world.settle();
    world.check();

    let high = world.members[0].receiving.shown();
    let extreme = world.members[1].receiving.shown();
    assert_eq!(high.len(), 64, "the level's number of references");
    assert_eq!(extreme.len(), 90);
    let triangles: u64 = high.iter().map(|e| u64::from(e.triangles)).sum();
    assert!(triangles <= Level::High.caps().triangles);
    assert!(
        high.iter().any(|e| e.simplified),
        "90 big references don't fit High unsimplified"
    );
    // Every byte was sent once and no more.
    for (i, shown) in [(0, &high), (1, &extreme)] {
        let bytes: u64 = shown.iter().map(|e| u64::from(e.bytes)).sum();
        assert_eq!(world.host.sharing.sent_to(world.members[i].peer), bytes);
    }
}

#[test]
fn lists_change_while_data_flows() {
    let mut world = World::new(2, &[Some(Level::High), Some(Level::VeryHigh)]);
    let mut rng = Rng::new(9);
    let mut counter = 0;
    let mut items = new_items(&mut rng, &mut counter, 60);
    world.host.publish(&items);
    for round in 0..6 {
        for _ in 0..rng.below(30) {
            world.step(50);
        }
        // Drop some, add some, move one.
        for _ in 0..rng.below(8) {
            let at = rng.below(items.len());
            items.remove(at);
        }
        items.extend(new_items(&mut rng, &mut counter, 1 + round));
        let (a, b) = (rng.below(items.len()), rng.below(items.len()));
        items.swap(a, b);
        world.host.publish(&items);
    }
    world.settle();
    world.check();
}

#[test]
fn a_member_leaving_midway_does_not_disturb_the_rest() {
    let mut world = World::new(3, &[Some(Level::VeryHigh), Some(Level::High)]);
    let mut counter = 0;
    let items = new_items(&mut Rng::new(11), &mut counter, 70);
    world.host.publish(&items);
    for _ in 0..40 {
        world.step(50);
    }
    let gone = world.members[0].peer;
    assert!(world.host.sharing.sent_to(gone) > 0, "it was mid-transfer");
    world.leave(0);
    for _ in 0..10 {
        world.step(50);
    }
    assert_eq!(world.host.sharing.sent_to(gone), 0, "forgotten");
    let sent = world.frames[&(HOST, gone)];
    world.settle();
    world.check();
    assert_eq!(world.frames[&(HOST, gone)], sent, "nothing more for it");
    assert!(!world.members[1].receiving.shown().is_empty());
}

#[test]
fn level_changes_midway_end_consistent() {
    let mut world = World::new(4, &[Some(Level::High), Some(Level::High)]);
    let mut counter = 0;
    let items = new_items(&mut Rng::new(13), &mut counter, 80);
    world.host.publish(&items);
    for _ in 0..30 {
        world.step(50);
    }
    // Up while receiving, off, on again: three changes in a row, the last waits its turn.
    world.set_level(0, Some(Level::Extreme));
    for _ in 0..20 {
        world.step(50);
    }
    world.set_level(0, None);
    world.set_level(0, Some(Level::VeryHigh));
    world.set_level(1, Some(Level::High));
    world.settle();
    world.check();
    assert_eq!(world.members[0].receiving.shown().len(), 80);
    assert_eq!(world.members[1].receiving.shown().len(), 64);
}

#[test]
fn off_gets_nothing_and_going_on_gets_it() {
    let mut world = World::new(5, &[None, Some(Level::High)]);
    let mut counter = 0;
    let items = new_items(&mut Rng::new(17), &mut counter, 30);
    world.host.publish(&items);
    world.settle();
    world.check();
    let off = world.members[0].peer;
    assert!(world.members[0].receiving.shown().is_empty());
    assert_eq!(world.host.sharing.sent_to(off), 0);
    world.set_level(0, Some(Level::High));
    world.settle();
    world.check();
    assert_eq!(world.members[0].receiving.shown().len(), 30);
    // And back off: the references go from view and the subhost sends no more.
    let sent = world.host.sharing.sent_to(off);
    world.set_level(0, None);
    world.settle();
    world.check();
    assert!(world.members[0].receiving.shown().is_empty());
    assert_eq!(world.host.sharing.sent_to(off), sent);
}

#[test]
fn going_back_to_an_earlier_list_needs_no_new_download() {
    let mut world = World::new(6, &[Some(Level::High)]);
    let mut counter = 0;
    let mut rng = Rng::new(19);
    let items = new_items(&mut rng, &mut counter, 20);
    world.host.publish(&items);
    world.settle();
    let peer = world.members[0].peer;
    let first = world.host.sharing.sent_to(peer);
    // Another list entirely, then the first again.
    let other = new_items(&mut rng, &mut counter, 20);
    world.host.publish(&other);
    world.settle();
    let second = world.host.sharing.sent_to(peer);
    assert!(second > first);
    world.host.publish(&items);
    world.settle();
    world.check();
    assert_eq!(
        world.host.sharing.sent_to(peer),
        second,
        "what it had is remembered"
    );
}

#[test]
fn random_sessions_converge() {
    let read = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let first = read("COOP_REFS_FIRST").unwrap_or(0);
    let seeds = read("COOP_REFS_SEEDS").unwrap_or(40);
    for seed in first..first + seeds {
        run(seed);
    }
    eprintln!("{seeds} runs");
}

/// One random session: members at random levels, lists changing, levels changing, someone
/// leaving, and then everything settles.
fn run(seed: u64) {
    let mut rng = Rng::new(seed + 1000);
    let levels: Vec<Option<Level>> = (0..1 + rng.below(3))
        .map(|_| {
            [
                None,
                Some(Level::High),
                Some(Level::VeryHigh),
                Some(Level::Extreme),
            ][rng.below(4)]
        })
        .collect();
    let mut world = World::new(seed, &levels);
    let mut counter = 0;
    let count = rng.between(10, 90) as usize;
    let mut items = new_items(&mut rng, &mut counter, count);
    world.host.publish(&items);
    let mut can_leave = levels.len() > 1;
    for _ in 0..400 {
        world.step(50);
        match rng.below(100) {
            0..=2 => {
                for _ in 0..rng.below(6) {
                    if items.len() > 3 {
                        let at = rng.below(items.len());
                        items.remove(at);
                    }
                }
                if items.len() < 100 {
                    let more = rng.below(8);
                    items.extend(new_items(&mut rng, &mut counter, more));
                }
                let (a, b) = (rng.below(items.len()), rng.below(items.len()));
                items.swap(a, b);
                world.host.publish(&items);
            }
            3..=5 => {
                let live = world.live();
                let i = live[rng.below(live.len())];
                let level = [
                    None,
                    Some(Level::High),
                    Some(Level::VeryHigh),
                    Some(Level::Extreme),
                ][rng.below(4)];
                world.set_level(i, level);
            }
            6 if can_leave && world.live().len() > 1 => {
                can_leave = false;
                let live = world.live();
                world.leave(live[rng.below(live.len())]);
            }
            _ => {}
        }
    }
    world.settle();
    world.check();
}

// --- Tests: a subhost that lies ---

/// A member's session that joined a real subhost; what the "subhost" says next is forged.
struct Duel {
    host: Session,
    guest: Session,
    receiving: Receiving,
    now: u64,
}

const GUEST: PeerId = 200;

/// Delivers everything both ways until quiet.
fn pump(host: &mut Session, guest: &mut Session) {
    loop {
        let (a, b) = (host.take_outgoing(), guest.take_outgoing());
        if a.is_empty() && b.is_empty() {
            return;
        }
        a.into_iter()
            .filter(|o| o.to == GUEST)
            .for_each(|o| guest.receive(HOST, &o.bytes));
        b.into_iter()
            .filter(|o| o.to == HOST)
            .for_each(|o| host.receive(GUEST, &o.bytes));
    }
}

impl Duel {
    fn new(level: Option<Level>) -> Self {
        let mut host = Session::new(info());
        host.host(SESSION, "host".into(), settings(), 0).unwrap();
        let mut guest = Session::new(info());
        guest.join(HOST, CODE.into(), "guest".into()).unwrap();
        pump(&mut host, &mut guest);
        assert!(guest.slot().is_some(), "joined");
        Self {
            host,
            guest,
            receiving: Receiving::new(level, 0),
            now: 0,
        }
    }

    /// The "subhost" sends `message`; what the member's mod does with it follows. What finished
    /// downloading comes back.
    fn forge(&mut self, message: Message) -> Vec<Finished> {
        for frame in framing::frames(SESSION, &message) {
            self.guest.receive(HOST, &frame);
        }
        self.drain()
    }

    fn drain(&mut self) -> Vec<Finished> {
        let mut finished = Vec::new();
        for event in self.guest.take_events() {
            match event {
                Event::ReferenceList(entries) => self.receiving.list(entries),
                Event::ReferenceData {
                    content,
                    offset,
                    data,
                } => finished.extend(self.receiving.data(content, offset, &data, self.now)),
                _ => {}
            }
        }
        finished
    }
}

fn entry(id: &str, content: u64, bytes: u32) -> Entry {
    Entry {
        id: id.into(),
        name: id.into(),
        kind: Kind::Model,
        content,
        bytes,
        triangles: 1,
        pixels: 0,
        simplified: false,
        position: [0.0; 3],
        rotation: [0.0; 3],
        scale: [1.0; 3],
        mirror: [false; 3],
        opacity: 1.0,
        color: [1.0; 3],
        image: None,
    }
}

fn data_message(content: u64, offset: u32, data: Vec<u8>) -> Message {
    Message::ReferenceData {
        content,
        offset,
        data,
    }
}

#[test]
fn a_subhost_lying_about_sizes_gets_only_what_it_declared() {
    let mut duel = Duel::new(Some(Level::High));
    duel.forge(Message::ReferenceList {
        entries: vec![entry("a", 1, 1000)],
    });
    assert_eq!(duel.receiving.next_want(0), Some(1));
    // More than declared, in one chunk and over several.
    assert!(duel.forge(data_message(1, 0, vec![7; 5000])).is_empty());
    assert!(duel.forge(data_message(1, 0, vec![7; CHUNK])).is_empty());
    assert_eq!(duel.receiving.ignored(), 2);
    let finished = duel.forge(data_message(1, 0, vec![7; 600]));
    assert!(finished.is_empty());
    assert!(duel.forge(data_message(1, 600, vec![7; 401])).is_empty());
    let finished = duel.forge(data_message(1, 600, vec![8; 400]));
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].bytes.len(), 1000);
    // And nothing after it finished.
    assert!(duel.forge(data_message(1, 1000, vec![7; 10])).is_empty());
    assert!(duel.forge(data_message(1, 0, vec![7; 1000])).is_empty());
}

#[test]
fn chunks_nobody_asked_for_and_in_the_wrong_order_are_ignored() {
    let mut duel = Duel::new(Some(Level::High));
    // Before any list.
    assert!(duel.forge(data_message(1, 0, vec![1; 100])).is_empty());
    duel.forge(Message::ReferenceList {
        entries: vec![entry("a", 1, 100), entry("b", 2, 100)],
    });
    // Listed but not asked for.
    assert!(duel.forge(data_message(1, 0, vec![1; 100])).is_empty());
    assert_eq!(duel.receiving.next_want(0), Some(1));
    // Another content than the asked one, even if listed; a chunk from the middle first.
    assert!(duel.forge(data_message(2, 0, vec![2; 100])).is_empty());
    assert!(duel.forge(data_message(1, 50, vec![1; 50])).is_empty());
    assert!(duel.forge(data_message(9, 0, vec![1; 100])).is_empty());
    assert_eq!(duel.receiving.ignored(), 5);
    assert_eq!(
        duel.receiving
            .entries()
            .iter()
            .map(|(_, s)| s.clone())
            .collect::<Vec<_>>(),
        vec![Status::Receiving { got: 0 }, Status::Waiting]
    );
    let finished = duel.forge(data_message(1, 0, vec![1; 100]));
    assert_eq!(finished.len(), 1);
}

#[test]
fn huge_lists_are_judged_against_the_level() {
    let mut duel = Duel::new(Some(Level::High));
    // 256 references, each claiming more than a burst of bytes: all skipped, nothing asked for.
    let huge: Vec<Entry> = (0..256)
        .map(|n| entry(&format!("h{n}"), n, u32::MAX))
        .collect();
    duel.forge(Message::ReferenceList { entries: huge });
    assert_eq!(duel.receiving.entries().len(), 256);
    let statuses: Vec<Status> = duel
        .receiving
        .entries()
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    assert!(
        statuses[..64]
            .iter()
            .all(|s| *s == Status::Skipped(Skip::Bytes))
    );
    assert!(
        statuses[64..]
            .iter()
            .all(|s| *s == Status::Skipped(Skip::Count))
    );
    assert_eq!(duel.receiving.next_want(0), None);

    // 256 small ones: the level's 64 only.
    let many: Vec<Entry> = (0..256)
        .map(|n| entry(&format!("m{n}"), 1000 + n, 100))
        .collect();
    duel.forge(Message::ReferenceList { entries: many });
    let statuses: Vec<Status> = duel
        .receiving
        .entries()
        .into_iter()
        .map(|(_, s)| s)
        .collect();
    assert!(statuses[..64].iter().all(|s| *s == Status::Waiting));
    assert!(
        statuses[64..]
            .iter()
            .all(|s| *s == Status::Skipped(Skip::Count))
    );

    // Each with more triangles than a share: they run out of the total.
    let heavy: Vec<Entry> = (0..60)
        .map(|n| Entry {
            triangles: 100_000,
            ..entry(&format!("t{n}"), 5000 + n, 100)
        })
        .collect();
    duel.forge(Message::ReferenceList { entries: heavy });
    let waiting = duel
        .receiving
        .entries()
        .iter()
        .filter(|(_, s)| *s == Status::Waiting)
        .count();
    assert_eq!(waiting, 2, "250,000 triangles are two of 100,000");

    // Four megabytes each, 60 of them: the allowance (16 MiB at High) stops the asking after four.
    let sized: Vec<Entry> = (0..60)
        .map(|n| entry(&format!("s{n}"), 9000 + n, 4 << 20))
        .collect();
    duel.forge(Message::ReferenceList { entries: sized });
    let mut asked = 0;
    while let Some(content) = duel.receiving.next_want(0) {
        asked += 1;
        let mut done = Vec::new();
        for offset in (0..4 << 20).step_by(CHUNK) {
            done = duel.forge(data_message(content, offset, vec![0; CHUNK]));
        }
        assert_eq!(done.len(), 1);
        duel.receiving.ready(content);
    }
    assert_eq!(asked, 4);
}

#[test]
fn lists_over_the_cap_or_broken_are_refused_and_change_nothing() {
    let mut duel = Duel::new(Some(Level::High));
    duel.forge(Message::ReferenceList {
        entries: vec![entry("a", 1, 100)],
    });
    // A count over 256 with data enough behind it, and one under with nonsense behind it.
    for count in [257u32, 300, 1000, 5] {
        let mut bytes = Message::ReferenceList {
            entries: Vec::new(),
        }
        .encode(SESSION);
        bytes[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&count.to_le_bytes());
        bytes.extend(vec![0; 2000]);
        duel.guest.receive(HOST, &bytes);
        let events = duel.guest.take_events();
        assert!(
            events.iter().any(|e| matches!(e, Event::BadMessage { .. })),
            "count {count}: {events:?}"
        );
        assert!(!events.iter().any(|e| matches!(e, Event::ReferenceList(_))));
    }
    assert_eq!(
        duel.receiving.entries().len(),
        1,
        "still the list from before"
    );

    // Numbers no honest host sends.
    let mut bad = entry("nan", 2, 10);
    bad.position[0] = f64::NAN;
    let mut bytes = Message::ReferenceList { entries: vec![bad] }.encode(SESSION);
    duel.guest.receive(HOST, &bytes);
    assert!(matches!(
        duel.guest.take_events()[..],
        [Event::BadMessage { .. }]
    ));
    // A path as an id.
    bytes = Message::ReferenceList {
        entries: vec![entry("..a", 2, 10)],
    }
    .encode(SESSION);
    duel.guest.receive(HOST, &bytes);
    assert!(matches!(
        duel.guest.take_events()[..],
        [Event::BadMessage { .. }]
    ));
}

#[test]
fn a_subhost_flooding_with_data_is_left() {
    let mut duel = Duel::new(Some(Level::High));
    duel.forge(Message::ReferenceList {
        entries: vec![entry("a", 1, 100)],
    });
    assert_eq!(duel.receiving.next_want(0), Some(1));
    let frame = data_message(1, 5, vec![1; 10]).encode(SESSION);
    let mut ended = None;
    for sent in 0..40_000 {
        duel.guest.receive(HOST, &frame);
        for event in duel.guest.take_events() {
            if let Event::Ended(reason) = event {
                ended = Some((sent, reason));
            }
        }
        if ended.is_some() {
            break;
        }
    }
    let (sent, reason) = ended.expect("it left a subhost sending without end");
    assert_eq!(reason, EndReason::HostFlooded(Flood::Messages));
    assert!(sent < 20_000, "{sent}");
    assert!(duel.guest.is_idle());
}

// --- Tests: the session's part ---

#[test]
fn level_and_wants_reach_the_subhost_from_its_members_only() {
    let mut duel = Duel::new(None);
    duel.guest.reference_level(Some(Level::VeryHigh));
    duel.guest.want_reference(0xABCD);
    duel.guest.reference_level(None);
    pump(&mut duel.host, &mut duel.guest);
    let events: Vec<Event> = duel
        .host
        .take_events()
        .into_iter()
        .filter(|e| !matches!(e, Event::SnapshotWanted(_) | Event::Members(_)))
        .collect();
    assert_eq!(
        events,
        vec![
            Event::ReferenceLevelAsked {
                peer: GUEST,
                level: Some(Level::VeryHigh)
            },
            Event::ReferenceWanted {
                peer: GUEST,
                content: 0xABCD
            },
            Event::ReferenceLevelAsked {
                peer: GUEST,
                level: None
            },
        ]
    );

    // A stranger's, a level that doesn't exist, and what only a subhost sends: nothing happens.
    let stranger: PeerId = 555;
    for message in [
        Message::ReferenceWant { content: 1 },
        Message::ReferenceLevel { level: 1 },
    ] {
        duel.host.receive(stranger, &message.encode(SESSION));
    }
    let mut level = Message::ReferenceLevel { level: 1 }.encode(SESSION);
    *level.last_mut().unwrap() = 9;
    duel.host.receive(GUEST, &level);
    for message in [
        Message::ReferenceList {
            entries: vec![entry("a", 1, 1)],
        },
        data_message(1, 0, vec![1]),
    ] {
        duel.host.receive(GUEST, &message.encode(SESSION));
    }
    let events = duel.host.take_events();
    assert!(
        matches!(events[..], [Event::BadMessage { .. }]),
        "only the bad level is noticed: {events:?}"
    );
}

#[test]
fn lists_and_data_go_only_from_a_subhost_to_its_members() {
    let mut duel = Duel::new(Some(Level::High));
    duel.guest.take_events();
    let chunk = |peer, len| coop::references::Chunk {
        peer,
        content: 1,
        offset: 0,
        data: vec![3; len],
    };
    // Not a member, and a chunk bigger than the protocol allows.
    duel.host.send_reference_data(chunk(555, 10));
    duel.host.send_reference_list(555, vec![entry("a", 1, 1)]);
    duel.host.send_reference_data(chunk(GUEST, CHUNK + 1));
    assert!(duel.host.take_outgoing().is_empty());
    // A member gets them.
    duel.host
        .send_reference_list(GUEST, vec![entry("a", 1, 10)]);
    duel.host.send_reference_data(chunk(GUEST, CHUNK));
    pump(&mut duel.host, &mut duel.guest);
    let events = duel.guest.take_events();
    assert!(matches!(
        events[..],
        [Event::ReferenceList(_), Event::ReferenceData { .. }]
    ));

    // And only in their roles: a member can't send lists, an idle session sends nothing.
    duel.guest.send_reference_list(HOST, vec![]);
    duel.guest.send_reference_data(chunk(HOST, 1));
    let mut idle = Session::new(info());
    idle.reference_level(Some(Level::High));
    idle.want_reference(1);
    duel.host.reference_level(Some(Level::High));
    duel.host.want_reference(1);
    assert!(duel.guest.take_outgoing().is_empty());
    assert!(idle.take_outgoing().is_empty());
    assert!(duel.host.take_outgoing().is_empty());
}

#[test]
fn a_member_flooding_wants_is_removed_and_gets_no_more_data() {
    let mut duel = Duel::new(Some(Level::High));
    duel.host.take_events();
    for n in 0..2_000 {
        duel.guest.want_reference(n);
    }
    pump(&mut duel.host, &mut duel.guest);
    let events = duel.host.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Flooded { peer: GUEST, .. })),
        "a member sending far more messages than a person can is removed"
    );
    duel.host.send_reference_data(coop::references::Chunk {
        peer: GUEST,
        content: 1,
        offset: 0,
        data: vec![1; 10],
    });
    assert!(duel.host.take_outgoing().is_empty());
}
