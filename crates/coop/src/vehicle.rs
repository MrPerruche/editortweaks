//! The vehicle a snapshot carries (opaque to the session, `Session::send_snapshot`): the editor's
//! own archive of every brick, and each brick's groups, which the archive can't carry over (the
//! game's loader gives loaded bricks new group numbers, while ops name groups by number).

use crate::message::caps::{SNAPSHOT_BYTES, SNAPSHOT_IDS};
use crate::ops::Groups;
use crate::wire::{Reader, Result, Writer};

#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    /// The editor's archive of the bricks, in the snapshot's id order.
    pub archive: Vec<u8>,
    /// Each brick's groups, in the same order; empty if the subhost couldn't read them.
    pub groups: Vec<Groups>,
}

impl Vehicle {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&self.archive);
        w.count(self.groups.len());
        self.groups.iter().flatten().for_each(|&group| w.u16(group));
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let archive = r.bytes("archive", SNAPSHOT_BYTES)?.to_vec();
        let n = r.count("groups", SNAPSHOT_IDS)?;
        let groups = (0..n)
            .map(|_| Ok([r.u16()?, r.u16()?]))
            .collect::<Result<_>>()?;
        r.finish()?;
        Ok(Self { archive, groups })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let vehicle = Vehicle {
            archive: vec![1, 2, 3],
            groups: vec![[0, 4], [65_535, 0]],
        };
        assert_eq!(Vehicle::decode(&vehicle.encode()), Ok(vehicle));
    }

    #[test]
    fn junk_is_refused() {
        let mut bytes = Vehicle {
            archive: vec![1],
            groups: vec![[1, 2]],
        }
        .encode();
        bytes.push(0);
        assert!(Vehicle::decode(&bytes).is_err());
        assert!(Vehicle::decode(&bytes[..bytes.len() - 3]).is_err());
    }
}
