//! The messages members send each other (`protocol.md`, "Messages"), and their encoding.
//!
//! Every message starts with a header: `"ET"`, the protocol version, the message kind and the
//! session id (0 before joining), so leftovers from another session are dropped.

use crate::audit::{Audit, Digest};
use crate::ids::{Check, CoopId, Slot};
use crate::ops::{Op, read_ids, read_ops, write_ids, write_ops};
use crate::presence::{MapPlace, View};
use crate::references::{
    CHUNK, Edit, Entry, Level, read_edits, read_entries, write_edits, write_entries,
};
use crate::wire::{DecodeError, Reader, Result, Writer};

/// A peer's address on the transport: a SteamID64 over Steam, any made-up number elsewhere.
pub type PeerId = u64;

pub const MAGIC: [u8; 2] = *b"ET";
/// Bumped whenever a message changes.
pub const PROTOCOL: u16 = 9;
/// magic + protocol + kind + session.
pub const HEADER_LEN: usize = 2 + 2 + 1 + 4;

pub mod caps {
    pub const NAME: usize = 64;
    pub const CODE: usize = 32;
    pub const VERSION: usize = 64;
    /// What a subhost's player gave as the reason of a ban, in bytes: cut to it when banning.
    pub const BAN_REASON: usize = 200;
    pub const MEMBERS: usize = 256;
    pub const SELECTION: usize = 4096;
    /// Unpacked snapshot (`SaveToArchive` bytes).
    pub const SNAPSHOT_BYTES: usize = 16 << 20;
    /// Packed snapshot.
    pub const PACKED_SNAPSHOT_BYTES: usize = 16 << 20;
    /// Bricks in a snapshot (the archive counts them in a `u16`).
    pub const SNAPSHOT_IDS: usize = 65_535;
    /// Bricks one audit describes.
    pub const AUDIT_BRICKS: usize = 256;
}

/// What a peer tells others about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// The oldest and newest protocol it speaks.
    pub protocol: (u16, u16),
    pub mod_version: String,
    /// The game's version: members must match, since values travel as the game's raw bytes.
    pub game_version: String,
    pub hosting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub slot: Slot,
    pub peer: PeerId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    WrongCode,
    /// The subhost said no.
    Refused,
    /// Another game version.
    GameVersion,
    Full,
    /// Not hosting (any more).
    NotHosting,
    /// The subhost banned us, for this many more minutes (`u16::MAX`: at least that many), for
    /// the reason its player gave (may be empty).
    Banned {
        minutes: u16,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hello(PeerInfo),
    HelloReply(PeerInfo),
    Join {
        code: String,
        name: String,
        game_version: String,
    },
    JoinAccepted {
        slot: Slot,
    },
    JoinRefused(RefuseReason),
    Members(Vec<Member>),
    /// The whole vehicle, as of commit `commit`. `ids` are the bricks' coop ids in object order.
    Snapshot {
        commit: u32,
        ids: Vec<CoopId>,
        vehicle: Vec<u8>,
    },
    /// A member's edit, `local` counting its own proposals.
    Propose {
        local: u32,
        ops: Vec<Op>,
    },
    /// An edit in the session's order. `check` describes the bricks after it.
    Commit {
        number: u32,
        author: Slot,
        local: u32,
        ops: Vec<Op>,
        check: Check,
    },
    /// "I drifted, send me a snapshot."
    Resync,
    /// The subhost's editor in short (see `audit`).
    Audit(Audit),
    /// "My bricks are in another order, send me yours."
    OrderWanted,
    /// The subhost's bricks in object order, as of commit `commit`.
    Order {
        commit: u32,
        ids: Vec<CoopId>,
    },
    /// Which bricks a member has selected. `slot` is filled in by the subhost when relaying.
    Selection {
        slot: Slot,
        ids: Vec<CoopId>,
    },
    /// Where a member's camera is (`None`: not in the editor's view). `slot` as for
    /// [`Self::Selection`].
    Camera {
        slot: Slot,
        view: Option<View>,
    },
    Leave,
    Kick,
    Close,
    Ping,
    /// A subhost invites us to its session, calling itself `name`.
    Invite {
        name: String,
    },
    /// A piece of a message too big for one transport message (see `framing`).
    Part {
        index: u32,
        count: u32,
        data: Vec<u8>,
    },
    /// Member to subhost: how much of the subhost's references it takes (`Level::to_wire`).
    ReferenceLevel {
        level: u8,
    },
    /// Subhost to member: all the references, fitted to the member's level.
    ReferenceList {
        entries: Vec<Entry>,
    },
    /// Member to subhost: "send me this reference's data from its start."
    ReferenceWant {
        content: u64,
    },
    /// Subhost to member: a piece of a reference's data, at most [`CHUNK`] bytes.
    ReferenceData {
        content: u64,
        offset: u32,
        data: Vec<u8>,
    },
    /// Member to subhost: changes of the subhost's references, which the subhost makes as its own.
    ReferenceEdit {
        edits: Vec<Edit>,
    },
    /// Subhost to member: where the subhost's map shows (`None`: it doesn't).
    MapPlace {
        place: Option<MapPlace>,
    },
}

impl Message {
    fn kind(&self) -> u8 {
        match self {
            Self::Hello(_) => 0,
            Self::HelloReply(_) => 1,
            Self::Join { .. } => 2,
            Self::JoinAccepted { .. } => 3,
            Self::JoinRefused(_) => 4,
            Self::Members(_) => 5,
            Self::Snapshot { .. } => 6,
            Self::Propose { .. } => 7,
            Self::Commit { .. } => 8,
            Self::Resync => 9,
            Self::Selection { .. } => 10,
            Self::Leave => 11,
            Self::Kick => 12,
            Self::Close => 13,
            Self::Ping => 14,
            Self::Part { .. } => 15,
            Self::Audit(_) => 16,
            Self::OrderWanted => 17,
            Self::Order { .. } => 18,
            Self::Camera { .. } => 19,
            Self::Invite { .. } => 20,
            Self::ReferenceLevel { .. } => 21,
            Self::ReferenceList { .. } => 22,
            Self::ReferenceWant { .. } => 23,
            Self::ReferenceData { .. } => 24,
            Self::ReferenceEdit { .. } => 25,
            Self::MapPlace { .. } => 26,
        }
    }

    /// Whether it may be lost: only selections, cameras and pings, which are resent anyway.
    pub fn reliable(&self) -> bool {
        !matches!(
            self,
            Self::Selection { .. } | Self::Camera { .. } | Self::Ping
        )
    }

    pub fn encode(&self, session: u32) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(&MAGIC);
        w.u16(PROTOCOL);
        w.u8(self.kind());
        w.u32(session);
        match self {
            Self::Hello(info) | Self::HelloReply(info) => {
                w.u16(info.protocol.0);
                w.u16(info.protocol.1);
                w.str(&info.mod_version);
                w.str(&info.game_version);
                w.bool(info.hosting);
            }
            Self::Join {
                code,
                name,
                game_version,
            } => {
                w.str(code);
                w.str(name);
                w.str(game_version);
            }
            Self::JoinAccepted { slot } => w.u8(*slot),
            Self::JoinRefused(reason) => match reason {
                RefuseReason::WrongCode => w.u8(0),
                RefuseReason::Refused => w.u8(1),
                RefuseReason::GameVersion => w.u8(2),
                RefuseReason::Full => w.u8(3),
                RefuseReason::NotHosting => w.u8(4),
                RefuseReason::Banned { minutes, reason } => {
                    w.u8(5);
                    w.u16(*minutes);
                    w.str(reason);
                }
            },
            Self::Members(members) => {
                w.count(members.len());
                for m in members {
                    w.u8(m.slot);
                    w.u64(m.peer);
                    w.str(&m.name);
                }
            }
            Self::Snapshot {
                commit,
                ids,
                vehicle,
            } => {
                w.u32(*commit);
                write_ids(&mut w, ids);
                w.count(vehicle.len());
                // Level 6: zlib's default; snapshots are rare.
                w.bytes(&miniz_oxide::deflate::compress_to_vec_zlib(vehicle, 6));
            }
            Self::Propose { local, ops } => {
                w.u32(*local);
                write_ops(&mut w, ops);
            }
            Self::Commit {
                number,
                author,
                local,
                ops,
                check,
            } => {
                w.u32(*number);
                w.u8(*author);
                w.u32(*local);
                write_ops(&mut w, ops);
                w.u32(check.count);
                w.u64(check.hash);
            }
            Self::Selection { slot, ids } => {
                w.u8(*slot);
                write_ids(&mut w, ids);
            }
            Self::Audit(audit) => {
                w.u32(audit.commit);
                w.u32(audit.count);
                w.u32(audit.order);
                w.count(audit.bricks.len());
                for (id, digest) in &audit.bricks {
                    w.u32(id.raw());
                    w.u32(digest.place);
                    w.u32(digest.values);
                    w.u32(digest.groups);
                }
            }
            Self::Order { commit, ids } => {
                w.u32(*commit);
                write_ids(&mut w, ids);
            }
            Self::Camera { slot, view } => {
                w.u8(*slot);
                w.bool(view.is_some());
                if let Some(view) = view {
                    view.location
                        .iter()
                        .chain(&view.forward)
                        .for_each(|&v| w.f32(v));
                }
            }
            Self::Resync
            | Self::OrderWanted
            | Self::Leave
            | Self::Kick
            | Self::Close
            | Self::Ping => {}
            Self::Part { index, count, data } => {
                w.u32(*index);
                w.u32(*count);
                w.raw(data);
            }
            Self::Invite { name } => w.str(name),
            Self::ReferenceLevel { level } => w.u8(*level),
            Self::ReferenceList { entries } => write_entries(&mut w, entries),
            Self::ReferenceWant { content } => w.u64(*content),
            Self::ReferenceData {
                content,
                offset,
                data,
            } => {
                w.u64(*content);
                w.u32(*offset);
                w.bytes(data);
            }
            Self::ReferenceEdit { edits } => write_edits(&mut w, edits),
            Self::MapPlace { place } => {
                w.bool(place.is_some());
                if let Some(place) = place {
                    place
                        .location
                        .iter()
                        .chain(&place.rotation)
                        .chain(&place.spawn_point)
                        .for_each(|&v| w.f32(v));
                    w.bool(place.spawn_box.is_some());
                    if let Some(half) = place.spawn_box {
                        half.iter().for_each(|&v| w.f32(v));
                    }
                }
            }
        }
        w.finish()
    }

    /// Returns the session id from the header and the message.
    pub fn decode(bytes: &[u8]) -> Result<(u32, Self)> {
        let mut r = Reader::new(bytes);
        if r.u8()? != MAGIC[0] || r.u8()? != MAGIC[1] {
            return Err(DecodeError::NotOurs);
        }
        let protocol = r.u16()?;
        let kind = r.u8()?;
        let session = r.u32()?;
        // Hellos keep their layout across versions, so peers can tell each other they differ.
        if protocol != PROTOCOL && kind > 1 {
            return Err(DecodeError::WrongProtocol(protocol));
        }
        let message = match kind {
            0 | 1 => {
                let info = PeerInfo {
                    protocol: (r.u16()?, r.u16()?),
                    mod_version: r.str("mod version", caps::VERSION)?,
                    game_version: r.str("game version", caps::VERSION)?,
                    hosting: r.bool()?,
                };
                if kind == 0 {
                    Self::Hello(info)
                } else {
                    Self::HelloReply(info)
                }
            }
            2 => Self::Join {
                code: r.str("code", caps::CODE)?,
                name: r.str("name", caps::NAME)?,
                game_version: r.str("game version", caps::VERSION)?,
            },
            3 => Self::JoinAccepted { slot: r.u8()? },
            4 => Self::JoinRefused(match r.u8()? {
                0 => RefuseReason::WrongCode,
                1 => RefuseReason::Refused,
                2 => RefuseReason::GameVersion,
                3 => RefuseReason::Full,
                4 => RefuseReason::NotHosting,
                5 => RefuseReason::Banned {
                    minutes: r.u16()?,
                    reason: r.str("ban reason", caps::BAN_REASON)?,
                },
                tag => {
                    return Err(DecodeError::BadTag {
                        what: "refuse reason",
                        tag,
                    });
                }
            }),
            5 => {
                let n = r.count("members", caps::MEMBERS)?;
                let members = (0..n)
                    .map(|_| {
                        Ok(Member {
                            slot: r.u8()?,
                            peer: r.u64()?,
                            name: r.str("name", caps::NAME)?,
                        })
                    })
                    .collect::<Result<_>>()?;
                Self::Members(members)
            }
            6 => {
                let commit = r.u32()?;
                let n = r.count("snapshot ids", caps::SNAPSHOT_IDS)?;
                let ids = (0..n)
                    .map(|_| crate::ops::read_id(&mut r))
                    .collect::<Result<_>>()?;
                let unpacked_len = r.u32()? as usize;
                if unpacked_len > caps::SNAPSHOT_BYTES {
                    return Err(DecodeError::TooLong {
                        what: "snapshot",
                        len: unpacked_len,
                        max: caps::SNAPSHOT_BYTES,
                    });
                }
                let packed = r.bytes("packed snapshot", caps::PACKED_SNAPSHOT_BYTES)?;
                let vehicle =
                    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, unpacked_len)
                        .map_err(|_| DecodeError::BadCompression)?;
                if vehicle.len() != unpacked_len {
                    return Err(DecodeError::BadCompression);
                }
                Self::Snapshot {
                    commit,
                    ids,
                    vehicle,
                }
            }
            7 => Self::Propose {
                local: r.u32()?,
                ops: read_ops(&mut r)?,
            },
            8 => Self::Commit {
                number: r.u32()?,
                author: r.u8()?,
                local: r.u32()?,
                ops: read_ops(&mut r)?,
                check: Check {
                    count: r.u32()?,
                    hash: r.u64()?,
                },
            },
            9 => Self::Resync,
            10 => Self::Selection {
                slot: r.u8()?,
                ids: read_ids(&mut r)?,
            },
            11 => Self::Leave,
            12 => Self::Kick,
            13 => Self::Close,
            14 => Self::Ping,
            20 => Self::Invite {
                name: r.str("name", caps::NAME)?,
            },
            15 => Self::Part {
                index: r.u32()?,
                count: r.u32()?,
                data: r.rest().to_vec(),
            },
            16 => {
                let commit = r.u32()?;
                let count = r.u32()?;
                let order = r.u32()?;
                let n = r.count("audited bricks", caps::AUDIT_BRICKS)?;
                let bricks = (0..n)
                    .map(|_| {
                        Ok((
                            crate::ops::read_id(&mut r)?,
                            Digest {
                                place: r.u32()?,
                                values: r.u32()?,
                                groups: r.u32()?,
                            },
                        ))
                    })
                    .collect::<Result<_>>()?;
                Self::Audit(Audit {
                    commit,
                    count,
                    order,
                    bricks,
                })
            }
            17 => Self::OrderWanted,
            18 => {
                let commit = r.u32()?;
                let n = r.count("order ids", caps::SNAPSHOT_IDS)?;
                let ids = (0..n)
                    .map(|_| crate::ops::read_id(&mut r))
                    .collect::<Result<_>>()?;
                Self::Order { commit, ids }
            }
            19 => {
                let slot = r.u8()?;
                let view = if r.bool()? {
                    let mut v = [0f32; 6];
                    for value in &mut v {
                        *value = r.f32()?;
                    }
                    let view = View {
                        location: [v[0], v[1], v[2]],
                        forward: [v[3], v[4], v[5]],
                    };
                    if !view.is_sane() {
                        return Err(DecodeError::BadValue("camera"));
                    }
                    Some(view)
                } else {
                    None
                };
                Self::Camera { slot, view }
            }
            21 => {
                let level = r.u8()?;
                if Level::from_wire(level).is_none() {
                    return Err(DecodeError::BadValue("reference level"));
                }
                Self::ReferenceLevel { level }
            }
            22 => Self::ReferenceList {
                entries: read_entries(&mut r)?,
            },
            23 => Self::ReferenceWant { content: r.u64()? },
            24 => Self::ReferenceData {
                content: r.u64()?,
                offset: r.u32()?,
                data: r.bytes("reference data", CHUNK)?.to_vec(),
            },
            25 => Self::ReferenceEdit {
                edits: read_edits(&mut r)?,
            },
            26 => {
                let place = if r.bool()? {
                    let mut three = || -> Result<[f32; 3]> { Ok([r.f32()?, r.f32()?, r.f32()?]) };
                    let (location, rotation, spawn_point) = (three()?, three()?, three()?);
                    let spawn_box = if r.bool()? {
                        Some([r.f32()?, r.f32()?, r.f32()?])
                    } else {
                        None
                    };
                    let place = MapPlace {
                        location,
                        rotation,
                        spawn_point,
                        spawn_box,
                    };
                    if !place.is_sane() {
                        return Err(DecodeError::BadValue("map place"));
                    }
                    Some(place)
                } else {
                    None
                };
                Self::MapPlace { place }
            }
            tag => {
                return Err(DecodeError::BadTag {
                    what: "message",
                    tag,
                });
            }
        };
        r.finish()?;
        Ok((session, message))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ops::{Transform, Value};
    use crate::references::{ImageLook, Kind};

    fn id(slot: Slot, n: u32) -> CoopId {
        CoopId::new(slot, n).unwrap()
    }

    pub(crate) fn samples() -> Vec<Message> {
        let info = PeerInfo {
            protocol: (1, 1),
            mod_version: "0.1.0".into(),
            game_version: "1.11.7".into(),
            hosting: true,
        };
        vec![
            Message::Hello(info.clone()),
            Message::HelloReply(PeerInfo {
                hosting: false,
                ..info
            }),
            Message::Join {
                code: "ABCD".into(),
                name: "Perru".into(),
                game_version: "1.11.7".into(),
            },
            Message::JoinAccepted { slot: 3 },
            Message::JoinRefused(RefuseReason::Full),
            Message::JoinRefused(RefuseReason::Banned {
                minutes: 600,
                reason: "griefing".into(),
            }),
            Message::Invite {
                name: "Perru".into(),
            },
            Message::Members(vec![Member {
                slot: 1,
                peer: 76561198000000000,
                name: "a".into(),
            }]),
            Message::Snapshot {
                commit: 9,
                ids: vec![id(0, 1), id(2, 7)],
                vehicle: vec![5; 10_000],
            },
            Message::Propose {
                local: 4,
                ops: vec![Op::Delete { id: id(0, 1) }],
            },
            Message::Commit {
                number: 10,
                author: 2,
                local: 4,
                ops: vec![
                    Op::Move {
                        id: id(2, 7),
                        transform: Transform {
                            location: [1.0; 3],
                            rotation: [2.0; 3],
                        },
                    },
                    Op::Set {
                        id: id(2, 7),
                        property: "Color".into(),
                        value: Value::Text("x".into()),
                    },
                ],
                check: Check {
                    count: 1,
                    hash: 0xDEAD,
                },
            },
            Message::Resync,
            Message::Selection {
                slot: 2,
                ids: vec![id(2, 7)],
            },
            Message::Camera {
                slot: 3,
                view: Some(View {
                    location: [1.0, -2.5, 300.0],
                    forward: [0.0, 0.6, -0.8],
                }),
            },
            Message::Camera {
                slot: 1,
                view: None,
            },
            Message::Leave,
            Message::Kick,
            Message::Close,
            Message::Ping,
            Message::Part {
                index: 1,
                count: 3,
                data: vec![1, 2, 3],
            },
            Message::Audit(Audit {
                commit: 12,
                count: 40,
                order: 0xABCD,
                bricks: vec![(
                    id(2, 7),
                    Digest {
                        place: 1,
                        values: 2,
                        groups: 3,
                    },
                )],
            }),
            Message::OrderWanted,
            Message::Order {
                commit: 12,
                ids: vec![id(0, 1), id(2, 7)],
            },
            Message::ReferenceLevel { level: 2 },
            Message::ReferenceList {
                entries: vec![
                    Entry {
                        id: "a_1".into(),
                        name: "Reference".into(),
                        kind: Kind::Model,
                        content: 0xFEED,
                        bytes: 5000,
                        triangles: 120,
                        pixels: 64,
                        simplified: true,
                        position: [1.0, 2.0, -3.0],
                        rotation: [0.0, 90.0, 0.0],
                        scale: [1.0; 3],
                        mirror: [false, true, false],
                        opacity: 0.5,
                        color: [1.0, 1.0, 0.0],
                        image: None,
                    },
                    Entry {
                        id: "b-2".into(),
                        name: "Photo".into(),
                        kind: Kind::Image,
                        content: 7,
                        bytes: 90,
                        triangles: 0,
                        pixels: 4096,
                        simplified: false,
                        position: [0.0; 3],
                        rotation: [0.0; 3],
                        scale: [1.0; 3],
                        mirror: [false; 3],
                        opacity: 1.0,
                        color: [1.0; 3],
                        image: Some(ImageLook {
                            width: 200.0,
                            flip_x: false,
                            flip_y: true,
                            double_sided: true,
                            pixelated: false,
                            pivot: 4,
                        }),
                    },
                ],
            },
            Message::ReferenceList {
                entries: Vec::new(),
            },
            Message::ReferenceWant { content: 0xFEED },
            Message::ReferenceData {
                content: 0xFEED,
                offset: 16384,
                data: vec![9; 100],
            },
            Message::ReferenceEdit {
                edits: vec![crate::references::Edit::Remove { id: "ab".into() }],
            },
            Message::MapPlace { place: None },
            Message::MapPlace {
                place: Some(sample_map_place()),
            },
            Message::MapPlace {
                place: Some(MapPlace {
                    spawn_box: None,
                    ..sample_map_place()
                }),
            },
        ]
    }

    pub(crate) fn sample_map_place() -> MapPlace {
        MapPlace {
            location: [123_456.5, -98_765.25, 4_321.0],
            rotation: [1.5, -90.0, 0.25],
            spawn_point: [10.0, -20.0, 30.0],
            spawn_box: Some([400.0, 250.0, 300.0]),
        }
    }

    #[test]
    fn nonsense_map_places_are_refused() {
        let good = sample_map_place();
        let bad = [
            MapPlace {
                location: [f32::NAN, 0.0, 0.0],
                ..good
            },
            MapPlace {
                location: [0.0, 0.0, 2e7],
                ..good
            },
            MapPlace {
                rotation: [0.0, f32::INFINITY, 0.0],
                ..good
            },
            MapPlace {
                spawn_point: [0.0, 2e6, 0.0],
                ..good
            },
            MapPlace {
                spawn_box: Some([-1.0, 1.0, 1.0]),
                ..good
            },
        ];
        for place in bad {
            let bytes = Message::MapPlace { place: Some(place) }.encode(3);
            assert!(
                matches!(Message::decode(&bytes), Err(DecodeError::BadValue(_))),
                "{place:?}"
            );
        }
    }

    #[test]
    fn round_trip() {
        for message in samples() {
            let bytes = message.encode(0x1234_5678);
            assert_eq!(Message::decode(&bytes), Ok((0x1234_5678, message)));
        }
    }

    #[test]
    fn junk_is_refused_without_panicking() {
        assert_eq!(Message::decode(b"HTTP/1.1"), Err(DecodeError::NotOurs));
        // Every prefix of every message, and every message with one byte flipped.
        for message in samples() {
            let bytes = message.encode(1);
            for len in 0..bytes.len() {
                let _ = Message::decode(&bytes[..len]);
            }
            for i in 0..bytes.len() {
                let mut flipped = bytes.clone();
                flipped[i] ^= 0xA5;
                let _ = Message::decode(&flipped);
            }
        }
    }

    #[test]
    fn reference_messages_refuse_what_no_honest_peer_sends() {
        let level = |level| Message::ReferenceLevel { level }.encode(1);
        assert!(Message::decode(&level(0)).is_ok());
        assert!(Message::decode(&level(3)).is_ok());
        assert_eq!(
            Message::decode(&level(4)),
            Err(DecodeError::BadValue("reference level"))
        );

        let data = |len| {
            Message::ReferenceData {
                content: 1,
                offset: 0,
                data: vec![0; len],
            }
            .encode(1)
        };
        assert!(Message::decode(&data(CHUNK)).is_ok());
        assert!(matches!(
            Message::decode(&data(CHUNK + 1)),
            Err(DecodeError::TooLong { .. })
        ));
        assert!(Message::ReferenceWant { content: 1 }.reliable());
        assert!(data(1).len() > HEADER_LEN);
    }

    #[test]
    fn other_protocol() {
        let mut bytes = Message::Resync.encode(1);
        bytes[2] = 99;
        assert_eq!(Message::decode(&bytes), Err(DecodeError::WrongProtocol(99)));
        // Hellos still read, so a newer peer can be told apart from junk.
        let mut hello = samples()[0].encode(0);
        hello[2] = 99;
        assert!(matches!(
            Message::decode(&hello),
            Ok((_, Message::Hello(_)))
        ));
    }
}
