//! Ops: the changes members send each other. A batch of ops is one finished editor operation
//! (`design.md`, "Syncing").

use crate::ids::{CoopId, LiveSet};
use crate::wire::{DecodeError, Reader, Result, Writer};

/// Caps on what a peer may send (first guesses, `protocol.md`).
pub mod caps {
    /// A member's whole allowance of changes at once (`limits::rates::CHANGES`). 10,000 was too few:
    /// one edit on a vehicle of 1,760 bricks came to 14,353 ops, and a proposal the subhost can't
    /// read is an edit lost.
    pub const OPS_PER_BATCH: usize = 300_000;
    /// The editor's archive counts bricks in a `u16`.
    pub const IDS_PER_OP: usize = 65_535;
    pub const ARCHIVE_BYTES: usize = 16 << 20;
    pub const PROPERTY_NAME: usize = 256;
    pub const VALUE_BYTES: usize = 1 << 20;
}

/// Where a brick is, exactly as the editor keeps it, so no rounding creeps in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub location: [f32; 3],
    pub rotation: [f32; 3],
}

impl Transform {
    /// Farthest a brick may be from the vehicle's origin, in cm (10 km): far beyond any editor
    /// build area, but no infinity or 1e38 that could upset the game's math.
    pub const MAX_LOCATION: f32 = 1e6;
    /// Largest rotation angle, in degrees: the editor keeps them within a turn or so.
    pub const MAX_ROTATION: f32 = 1e4;

    /// Whether every number is finite and within the caps above. Peers' moves are refused
    /// otherwise, before anything reaches the game.
    pub fn is_sane(&self) -> bool {
        let within = |values: &[f32; 3], max: f32| values.iter().all(|v| v.abs() <= max);
        within(&self.location, Self::MAX_LOCATION) && within(&self.rotation, Self::MAX_ROTATION)
    }
}

/// A property's new value (`protocol.md`, "Values").
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Plain data, copied as the bytes the game keeps it in.
    Raw(Vec<u8>),
    /// The property's own text export.
    Text(String),
    /// References to other bricks, as coop ids.
    Bricks(Vec<CoopId>),
}

/// Which of a brick's two groups (the game's `EBrickGroupType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    /// The editor's groups, for selecting bricks together.
    Editor = 0,
    /// Weld groups.
    Weld = 1,
}

impl GroupKind {
    pub const ALL: [Self; 2] = [Self::Editor, Self::Weld];
}

/// A brick's group of each kind, by [`GroupKind`] (0: in none). Group numbers are labels: bricks
/// with the same number are in the same group.
pub type Groups = [u16; 2];

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// New bricks, saved by the editor into one archive with `ids` in its order (references
    /// between them are archive positions).
    Create {
        ids: Vec<CoopId>,
        archive: Vec<u8>,
    },
    Delete {
        id: CoopId,
    },
    Move {
        id: CoopId,
        transform: Transform,
    },
    /// `property` is the property's path as the editor names it.
    Set {
        id: CoopId,
        property: String,
        value: Value,
    },
    /// Puts a brick in group `group` of kind `kind` (0: out of its group). One brick at a time,
    /// like a property, so members grouping different bricks at once don't undo each other's.
    Group {
        id: CoopId,
        kind: GroupKind,
        group: u16,
    },
    /// Bricks in their new object order. Bricks missing from the list keep their order after
    /// the listed ones (`order::reordered`).
    Order {
        ids: Vec<CoopId>,
    },
}

/// What an op writes, to tell whether two ops touch the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key<'a> {
    /// The whole brick (creating or deleting it).
    Brick(CoopId),
    Placement(CoopId),
    Property(CoopId, &'a str),
    Group(CoopId, GroupKind),
    Order,
}

impl Key<'_> {
    pub(crate) fn brick(self) -> Option<CoopId> {
        match self {
            Self::Brick(id) | Self::Placement(id) | Self::Property(id, _) | Self::Group(id, _) => {
                Some(id)
            }
            Self::Order => None,
        }
    }

    pub(crate) fn overlaps(self, other: Key<'_>) -> bool {
        match (self.brick(), other.brick()) {
            (Some(a), Some(b)) => {
                a == b
                    && (matches!(self, Self::Brick(_))
                        || matches!(other, Key::Brick(_))
                        || self == other)
            }
            _ => self == other,
        }
    }
}

impl Op {
    pub(crate) fn keys(&self) -> Vec<Key<'_>> {
        match self {
            Self::Create { ids, .. } => ids.iter().map(|&id| Key::Brick(id)).collect(),
            Self::Delete { id } => vec![Key::Brick(*id)],
            Self::Move { id, .. } => vec![Key::Placement(*id)],
            Self::Set { id, property, .. } => vec![Key::Property(*id, property)],
            Self::Group { id, kind, .. } => vec![Key::Group(*id, *kind)],
            Self::Order { .. } => vec![Key::Order],
        }
    }

    /// Whether a member that has `pending` edits of its own (sent, not committed yet) should skip
    /// this remote op. Its own edit will be committed after this one and overwrite it, so applying
    /// it would only show the wrong value until then, and worse, stick when the own edit's commit
    /// is skipped as already applied. Creates and deletes always apply: ids are unique, and a
    /// delete wins over any change to the brick.
    pub fn overridden_by<'a>(&self, pending: impl IntoIterator<Item = &'a Op>) -> bool {
        if matches!(self, Self::Create { .. } | Self::Delete { .. }) {
            return false;
        }
        let mine = self.keys();
        pending
            .into_iter()
            .flat_map(Op::keys)
            .any(|theirs| mine.iter().any(|key| key.overlaps(theirs)))
    }

    /// Whether this op does anything with the bricks in `live`: one on a brick that doesn't
    /// exist does nothing.
    pub fn is_effective(&self, live: &LiveSet) -> bool {
        match self {
            Self::Create { .. } | Self::Order { .. } => true,
            Self::Delete { id }
            | Self::Move { id, .. }
            | Self::Set { id, .. }
            | Self::Group { id, .. } => live.contains(*id),
        }
    }

    /// Whether this op and `other` write something in common.
    pub(crate) fn touches(&self, other: &Op) -> bool {
        let theirs = other.keys();
        self.keys()
            .iter()
            .any(|key| theirs.iter().any(|&other| key.overlaps(other)))
    }

    /// Whether this op makes or changes one of `ids`, or refers to one (a brick reference).
    pub(crate) fn mentions(&self, ids: &[CoopId]) -> bool {
        match self {
            Self::Set {
                value: Value::Bricks(bricks),
                ..
            } if bricks.iter().any(|id| ids.contains(id)) => true,
            _ => self
                .keys()
                .iter()
                .any(|key| key.brick().is_some_and(|id| ids.contains(&id))),
        }
    }

    /// Updates which bricks exist.
    pub fn apply_to(&self, live: &mut LiveSet) {
        match self {
            Self::Create { ids, .. } => ids.iter().for_each(|&id| {
                live.insert(id);
            }),
            Self::Delete { id } => {
                live.remove(*id);
            }
            _ => {}
        }
    }

    pub fn encode(&self, w: &mut Writer) {
        match self {
            Self::Create { ids, archive } => {
                w.u8(0);
                write_ids(w, ids);
                w.bytes(archive);
            }
            Self::Delete { id } => {
                w.u8(1);
                w.u32(id.raw());
            }
            Self::Move { id, transform } => {
                w.u8(2);
                w.u32(id.raw());
                transform
                    .location
                    .iter()
                    .chain(&transform.rotation)
                    .for_each(|&v| w.f32(v));
            }
            Self::Set {
                id,
                property,
                value,
            } => {
                w.u8(3);
                w.u32(id.raw());
                w.str(property);
                write_value(w, value);
            }
            Self::Group { id, kind, group } => {
                w.u8(4);
                w.u32(id.raw());
                w.u8(*kind as u8);
                w.u16(*group);
            }
            Self::Order { ids } => {
                w.u8(5);
                write_ids(w, ids);
            }
        }
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Self> {
        Ok(match r.u8()? {
            0 => Self::Create {
                ids: read_ids(r)?,
                archive: r.bytes("archive", caps::ARCHIVE_BYTES)?.to_vec(),
            },
            1 => Self::Delete { id: read_id(r)? },
            2 => {
                let id = read_id(r)?;
                let mut v = [0f32; 6];
                for slot in &mut v {
                    *slot = r.f32()?;
                }
                let transform = Transform {
                    location: [v[0], v[1], v[2]],
                    rotation: [v[3], v[4], v[5]],
                };
                // NaN fails every comparison, so it's refused too.
                if !transform.is_sane() {
                    return Err(DecodeError::BadValue("move"));
                }
                Self::Move { id, transform }
            }
            3 => {
                let id = read_id(r)?;
                let property = r.str("property name", caps::PROPERTY_NAME)?;
                let value = match r.u8()? {
                    0 => Value::Raw(r.bytes("value", caps::VALUE_BYTES)?.to_vec()),
                    1 => Value::Text(r.str("value", caps::VALUE_BYTES)?),
                    2 => Value::Bricks(read_ids(r)?),
                    tag => return Err(DecodeError::BadTag { what: "value", tag }),
                };
                Self::Set {
                    id,
                    property,
                    value,
                }
            }
            4 => Self::Group {
                id: read_id(r)?,
                kind: match r.u8()? {
                    0 => GroupKind::Editor,
                    1 => GroupKind::Weld,
                    tag => {
                        return Err(DecodeError::BadTag {
                            what: "group kind",
                            tag,
                        });
                    }
                },
                group: r.u16()?,
            },
            5 => Self::Order { ids: read_ids(r)? },
            tag => return Err(DecodeError::BadTag { what: "op", tag }),
        })
    }
}

pub(crate) fn write_value(w: &mut Writer, value: &Value) {
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
            write_ids(w, ids);
        }
    }
}

pub(crate) fn write_ids(w: &mut Writer, ids: &[CoopId]) {
    w.count(ids.len());
    ids.iter().for_each(|id| w.u32(id.raw()));
}

pub(crate) fn read_id(r: &mut Reader<'_>) -> Result<CoopId> {
    let raw = r.u32()?;
    CoopId::from_raw(raw).ok_or(DecodeError::BadTag {
        what: "coop id",
        tag: 0,
    })
}

pub(crate) fn read_ids(r: &mut Reader<'_>) -> Result<Vec<CoopId>> {
    let n = r.count("ids", caps::IDS_PER_OP)?;
    (0..n).map(|_| read_id(r)).collect()
}

pub(crate) fn write_ops(w: &mut Writer, ops: &[Op]) {
    w.count(ops.len());
    ops.iter().for_each(|op| op.encode(w));
}

pub(crate) fn read_ops(r: &mut Reader<'_>) -> Result<Vec<Op>> {
    let n = r.count("ops", caps::OPS_PER_BATCH)?;
    (0..n).map(|_| Op::decode(r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> CoopId {
        CoopId::new(2, n).unwrap()
    }

    fn set(n: u32, property: &str) -> Op {
        Op::Set {
            id: id(n),
            property: property.into(),
            value: Value::Raw(vec![1]),
        }
    }

    fn moved(n: u32) -> Op {
        Op::Move {
            id: id(n),
            transform: Transform {
                location: [0.0; 3],
                rotation: [0.0; 3],
            },
        }
    }

    #[test]
    fn overrides() {
        let pending = [set(1, "Color"), moved(2), Op::Delete { id: id(3) }];
        assert!(set(1, "Color").overridden_by(&pending));
        assert!(!set(1, "Size").overridden_by(&pending));
        assert!(moved(2).overridden_by(&pending));
        assert!(!set(2, "Color").overridden_by(&pending));
        // Anything on a brick deleted here is moot.
        assert!(set(3, "Color").overridden_by(&pending));
        assert!(moved(3).overridden_by(&pending));
        // Deletes always apply, even over our pending changes to the brick.
        assert!(!Op::Delete { id: id(1) }.overridden_by(&pending));
        let group = |n, kind| Op::Group {
            id: id(n),
            kind,
            group: 7,
        };
        assert!(!group(1, GroupKind::Weld).overridden_by(&pending));
        assert!(group(3, GroupKind::Weld).overridden_by(&pending));
        let grouped = [group(1, GroupKind::Weld)];
        assert!(group(1, GroupKind::Weld).overridden_by(&grouped));
        assert!(!group(1, GroupKind::Editor).overridden_by(&grouped));
        assert!(!group(2, GroupKind::Weld).overridden_by(&grouped));
    }

    #[test]
    fn round_trip() {
        let ops = vec![
            Op::Create {
                ids: vec![id(1), id(2)],
                archive: vec![9; 100],
            },
            Op::Delete { id: id(5) },
            Op::Move {
                id: id(1),
                transform: Transform {
                    location: [1.0, -2.5, 3e5],
                    rotation: [0.1, 0.2, -180.0],
                },
            },
            Op::Set {
                id: id(1),
                property: "Color".into(),
                value: Value::Raw(vec![1, 2, 3, 4]),
            },
            Op::Set {
                id: id(1),
                property: "Name".into(),
                value: Value::Text("hi".into()),
            },
            Op::Set {
                id: id(1),
                property: "Inputs.SourceBricks".into(),
                value: Value::Bricks(vec![id(2)]),
            },
            Op::Group {
                id: id(2),
                kind: GroupKind::Weld,
                group: 65_535,
            },
            Op::Order {
                ids: vec![id(2), id(1)],
            },
        ];
        let mut w = Writer::new();
        write_ops(&mut w, &ops);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(read_ops(&mut r), Ok(ops));
        assert_eq!(r.finish(), Ok(()));
    }

    #[test]
    fn impossible_moves_are_refused() {
        for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e7, f32::MAX] {
            for at in 0..6 {
                let mut v = [0.0; 6];
                v[at] = bad;
                let op = Op::Move {
                    id: id(1),
                    transform: Transform {
                        location: [v[0], v[1], v[2]],
                        rotation: [v[3], v[4], v[5]],
                    },
                };
                let mut w = Writer::new();
                op.encode(&mut w);
                let bytes = w.finish();
                assert_eq!(
                    Op::decode(&mut Reader::new(&bytes)),
                    Err(DecodeError::BadValue("move")),
                    "{bad} at {at}"
                );
            }
        }
    }
}
