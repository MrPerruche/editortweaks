//! Reading the front of the editor's vehicle archive (the `.brv` format) before the game gets it.
//! A `Create` op carries bytes from another player, and the game's loader trusts a vehicle file
//! about as much as it trusts a file the player saved themselves. So the receiver checks the
//! layout first, with the same limits the loader enforces, and only then hands the bytes over.
//! Everything read is bounds-checked against the bytes that are really there before anything is
//! allocated for it.
//!
//! The layout (little-endian; `docs/re/coop/findings.md`, "The format"):
//!
//! ```text
//! header        u8  version            7..=18 (the loader refuses > 18; < 7 is its legacy path)
//!               u16 object count
//!               u16 class count        <= object count, <= 50000
//!               u16 property count     <= class count * 32
//! class table   per class: name         u8 length (<= 127), then that many ASCII bytes
//! property      per property: name      as a class name
//! table                       u16 n     how many distinct values the file holds of it
//!                             u32 len   bytes of those values, skipped here
//!                             values    len bytes
//!                             if n > 1: u16 k; if k == 0: n * u16 (the values' sizes)
//! objects       per object: u16 class index (into the class table)
//!                           u32 len, then len bytes of the object's own data
//! tail          whatever follows (e.g. the main seat's index for formats 7..=14)
//! ```
//!
//! An archive saved with `bSkipHeader` (the editor's duplicate does that) has no 7-byte header and
//! one more `u16` per object (the object's old index + 1), so it can't be read without its counts.
//! Coop sends whole archives (`SaveToArchive`), which do start with the header.

use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

/// The oldest format the loader reads without its legacy path (`LoadObjectsFromArchive` sends
/// anything older to a separate loader, which coop doesn't want to depend on).
pub const OLDEST_VERSION: u8 = 7;
/// The newest format: what game 1.11.7 writes, and the most its loader accepts (it answers
/// "newer format" to anything above).
pub const NEWEST_VERSION: u8 = 18;
/// The header's object count is a `u16`, so no archive can hold more. Kept as a name so the cap
/// reads as a rule and not as an accident of the field.
pub const MAX_OBJECTS: usize = u16::MAX as usize;
/// The loader's cap on the class count.
pub const MAX_CLASSES: usize = 50_000;
/// The loader's cap on the property count, per class in the table.
pub const MAX_PROPERTIES_PER_CLASS: usize = 32;
/// The longest name the loader reads (class and property names).
pub const MAX_NAME_LEN: usize = 127;

/// The least bytes a property table entry takes: a name's length byte, `n` and `len`.
const MIN_PROPERTY_BYTES: usize = 1 + 2 + 4;
/// The least bytes an object takes: its class index and `len`.
const MIN_OBJECT_BYTES: usize = 2 + 4;

/// What the front of an archive says, once it checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveHeader {
    pub version: u8,
    /// Class names, without the `BP_` prefix, in table order.
    pub class_names: Vec<String>,
    /// Names of the properties the objects hold, in table order. (The game saves properties by
    /// name, so these are what a later check can compare against what it knows.)
    pub property_names: Vec<String>,
    /// For each object, its position in `class_names`. Always in range.
    pub class_indices: Vec<u16>,
    /// For each object, where its own data lies in the archive's bytes.
    pub bodies: Vec<Range<usize>>,
    /// Where the last object ends: the archive's tail, if any, starts here.
    pub objects_end: usize,
}

impl ArchiveHeader {
    pub fn object_count(&self) -> usize {
        self.class_indices.len()
    }

    /// The class name of object `object`.
    pub fn class_of(&self, object: usize) -> Option<&str> {
        let index = *self.class_indices.get(object)?;
        self.class_names.get(usize::from(index)).map(String::as_str)
    }

    /// Which of the objects the game's loader made, given the classes of what it made, in order.
    /// It skips objects of classes this game doesn't have (a mod's brick), and only those: `None`
    /// if `made` can't come of that, so what's missing can't be told.
    pub fn made_objects<S: AsRef<str>>(&self, made: &[S]) -> Option<Vec<usize>> {
        let mut objects = 0..self.object_count();
        let found: Vec<usize> = made
            .iter()
            .map(|class| {
                let class = bare_class(class.as_ref());
                objects.find(|&object| self.class_of(object).map(bare_class) == Some(class))
            })
            .collect::<Option<_>>()?;
        // A class is there or it isn't: an object of a class that was made wasn't skipped for it.
        let classes: HashSet<&str> = made.iter().map(|c| bare_class(c.as_ref())).collect();
        let of_those = (0..self.object_count())
            .filter(|&object| {
                self.class_of(object)
                    .is_some_and(|c| classes.contains(bare_class(c)))
            })
            .count();
        (of_those == found.len()).then_some(found)
    }
}

/// A class's name as archives and the game's objects share it: `BP_Brick_C` and `BP_Brick` are
/// `Brick`.
fn bare_class(name: &str) -> &str {
    let name = name.strip_prefix("BP_").unwrap_or(name);
    name.strip_suffix("_C").unwrap_or(name)
}

/// Why an archive was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The bytes ended in the middle of `what`.
    Truncated { what: &'static str },
    /// A format the loader doesn't read (too old, or from a newer game).
    UnsupportedVersion(u8),
    /// A count over the loader's cap, or one the other counts can't be true with.
    BadCount {
        what: &'static str,
        count: usize,
        max: usize,
    },
    /// A name that is empty, longer than the loader reads, or not printable ASCII.
    BadName { what: &'static str },
    /// Object `object` names a class the table doesn't have.
    ClassOutOfRange {
        object: usize,
        index: u16,
        classes: usize,
    },
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what } => write!(f, "archive ends in the middle of {what}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "archive format {v} isn't one coop accepts ({OLDEST_VERSION}..={NEWEST_VERSION})"
            ),
            Self::BadCount { what, count, max } => {
                write!(f, "{what}: {count} is impossible (at most {max})")
            }
            Self::BadName { what } => write!(f, "{what}: not a valid name"),
            Self::ClassOutOfRange {
                object,
                index,
                classes,
            } => write!(
                f,
                "object {object} has class {index}, but the archive has {classes} classes"
            ),
        }
    }
}

impl std::error::Error for ArchiveError {}

type Result<T> = std::result::Result<T, ArchiveError>;

/// Checks the front of `bytes` as the game's loader would read it (header, class table, property
/// table, and every object's class and length) and describes it. The objects' own data is not
/// read, only measured. Bytes after the last object are fine: they are the archive's tail.
pub fn read_header(bytes: &[u8]) -> Result<ArchiveHeader> {
    let mut cursor = Cursor { bytes, at: 0 };

    let version = cursor.u8("the version")?;
    if !(OLDEST_VERSION..=NEWEST_VERSION).contains(&version) {
        return Err(ArchiveError::UnsupportedVersion(version));
    }
    let objects = usize::from(cursor.u16("the object count")?);
    let classes = usize::from(cursor.u16("the class count")?);
    let properties = usize::from(cursor.u16("the property count")?);

    // The loader's own limits...
    if classes > objects {
        return Err(bad_count("class count", classes, objects));
    }
    if classes > MAX_CLASSES {
        return Err(bad_count("class count", classes, MAX_CLASSES));
    }
    let max_properties = classes * MAX_PROPERTIES_PER_CLASS;
    if properties > max_properties {
        return Err(bad_count("property count", properties, max_properties));
    }
    // ...and the bytes' own: every item takes at least a few, so counts the rest of the bytes
    // couldn't hold are refused before anything is reserved for them.
    cursor.expect_room("class table", classes, 1)?;
    cursor.expect_room("property table", properties, MIN_PROPERTY_BYTES)?;
    cursor.expect_room("objects", objects, MIN_OBJECT_BYTES)?;

    let mut class_names = Vec::with_capacity(classes);
    for _ in 0..classes {
        class_names.push(cursor.name("a class name")?);
    }

    let mut property_names = Vec::with_capacity(properties);
    for _ in 0..properties {
        property_names.push(cursor.name("a property name")?);
        cursor.skip_property_values()?;
    }

    let mut class_indices = Vec::with_capacity(objects);
    let mut bodies = Vec::with_capacity(objects);
    for object in 0..objects {
        let index = cursor.u16("an object's class")?;
        if usize::from(index) >= classes {
            return Err(ArchiveError::ClassOutOfRange {
                object,
                index,
                classes,
            });
        }
        let len = cursor.len32("an object's length")?;
        bodies.push(cursor.take_range(len, "an object's data")?);
        class_indices.push(index);
    }

    Ok(ArchiveHeader {
        version,
        class_names,
        property_names,
        class_indices,
        bodies,
        objects_end: cursor.at,
    })
}

fn bad_count(what: &'static str, count: usize, max: usize) -> ArchiveError {
    ArchiveError::BadCount { what, count, max }
}

/// A position in the archive's bytes. Every read checks what is left first.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&[u8]> {
        if n > self.left() {
            return Err(ArchiveError::Truncated { what });
        }
        let taken = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(taken)
    }

    /// Steps over `n` bytes and says where they were.
    fn take_range(&mut self, n: usize, what: &'static str) -> Result<Range<usize>> {
        let start = self.at;
        self.take(n, what)?;
        Ok(start..self.at)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// A `u32` length, as a `usize`.
    fn len32(&mut self, what: &'static str) -> Result<usize> {
        let bytes = self.take(4, what)?;
        let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        // A length that doesn't fit `usize` can't fit the bytes either.
        usize::try_from(len).map_err(|_| ArchiveError::Truncated { what })
    }

    /// Refuses `count` items of at least `each` bytes when fewer bytes than that are left.
    fn expect_room(&self, what: &'static str, count: usize, each: usize) -> Result<()> {
        // `count` is a `u16` and `each` tiny: this can't overflow.
        let needed = count * each;
        if needed > self.left() {
            return Err(ArchiveError::Truncated { what });
        }
        Ok(())
    }

    /// A name: a length byte, then that many printable ASCII characters. The loader reads it the
    /// same way, with the same cap (a negative length is accepted by it, but the game never
    /// writes one).
    fn name(&mut self, what: &'static str) -> Result<String> {
        let len = usize::from(self.u8(what)?);
        if len == 0 || len > MAX_NAME_LEN {
            return Err(ArchiveError::BadName { what });
        }
        let chars = self.take(len, what)?;
        if !chars.iter().all(|c| (0x20..0x7F).contains(c)) {
            return Err(ArchiveError::BadName { what });
        }
        Ok(chars.iter().map(|&c| char::from(c)).collect())
    }

    /// Steps over what follows a property's name: see the table in the module docs.
    fn skip_property_values(&mut self) -> Result<()> {
        let values = usize::from(self.u16("a property's value count")?);
        let len = self.len32("a property's values length")?;
        self.take(len, "a property's values")?;
        if values > 1 {
            let each_size = self.u16("a property's value size")?;
            if each_size == 0 {
                // One size per value, which the loader reads and we only have to step over.
                self.take(values * 2, "a property's value sizes")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an archive the way the game lays it out, with objects of the given classes and a
    /// few bytes of data each.
    #[derive(Default)]
    struct Archive {
        version: u8,
        classes: Vec<&'static str>,
        /// (name, number of values, values' bytes, then the sizes part as written)
        properties: Vec<(&'static str, u16, Vec<u8>, Vec<u8>)>,
        /// (class index, data)
        objects: Vec<(u16, Vec<u8>)>,
        tail: Vec<u8>,
    }

    impl Archive {
        fn new() -> Self {
            Self {
                version: NEWEST_VERSION,
                ..Self::default()
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![self.version];
            out.extend((self.objects.len() as u16).to_le_bytes());
            out.extend((self.classes.len() as u16).to_le_bytes());
            out.extend((self.properties.len() as u16).to_le_bytes());
            for name in &self.classes {
                push_name(&mut out, name);
            }
            for (name, values, data, sizes) in &self.properties {
                push_name(&mut out, name);
                out.extend(values.to_le_bytes());
                out.extend((data.len() as u32).to_le_bytes());
                out.extend(data);
                out.extend(sizes);
            }
            for (class, data) in &self.objects {
                out.extend(class.to_le_bytes());
                out.extend((data.len() as u32).to_le_bytes());
                out.extend(data);
            }
            out.extend(&self.tail);
            out
        }
    }

    fn push_name(out: &mut Vec<u8>, name: &str) {
        out.push(name.len() as u8);
        out.extend(name.as_bytes());
    }

    /// Two classes, the three kinds of property tables entry, three objects and a tail.
    fn vehicle() -> Archive {
        let mut archive = Archive::new();
        archive.classes = vec!["ScalableBrick", "Light_1sx1sx1s"];
        archive.properties = vec![
            // One value of 12 bytes: nothing after the values.
            ("BrickSize", 1, vec![0; 12], vec![]),
            // Two values, all of 4 bytes: a non-zero size stands for all.
            ("BrickColor", 2, vec![7; 8], 4u16.to_le_bytes().to_vec()),
            // Two values of different sizes: a zero, then each one's.
            (
                "BrickMaterial",
                2,
                b"\x07Plastic\x04Glow".to_vec(),
                [0u16, 8, 5].iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
        ];
        archive.objects = vec![(0, vec![1; 33]), (1, vec![2; 5]), (0, vec![])];
        archive.tail = vec![0, 0];
        archive
    }

    #[test]
    fn reads_a_vehicle() {
        let bytes = vehicle().bytes();
        let header = read_header(&bytes).unwrap();
        assert_eq!(header.version, 18);
        assert_eq!(header.class_names, ["ScalableBrick", "Light_1sx1sx1s"]);
        assert_eq!(
            header.property_names,
            ["BrickSize", "BrickColor", "BrickMaterial"]
        );
        assert_eq!(header.class_indices, [0, 1, 0]);
        assert_eq!(header.object_count(), 3);
        assert_eq!(header.class_of(1), Some("Light_1sx1sx1s"));
        assert_eq!(header.class_of(3), None);
        // What the loader made of it: all, or without a class this game lacks.
        let (brick, light) = ("BP_ScalableBrick_C", "BP_Light_1sx1sx1s_C");
        assert_eq!(
            header.made_objects(&[brick, light, brick]),
            Some(vec![0, 1, 2])
        );
        assert_eq!(header.made_objects(&[brick, brick]), Some(vec![0, 2]));
        assert_eq!(header.made_objects(&[light]), Some(vec![1]));
        assert_eq!(header.made_objects::<&str>(&[]), Some(vec![]));
        // Not what skipping classes makes: another order, one of a class's objects, another class.
        assert_eq!(header.made_objects(&[light, brick]), None);
        assert_eq!(header.made_objects(&[brick]), None);
        assert_eq!(header.made_objects(&["BP_Other_C"]), None);
        // The bodies are where the objects' data really is, and the tail is what follows.
        assert_eq!(bytes[header.bodies[0].clone()], [1; 33]);
        assert_eq!(bytes[header.bodies[1].clone()], [2; 5]);
        assert!(header.bodies[2].is_empty());
        assert_eq!(&bytes[header.objects_end..], [0, 0]);
    }

    #[test]
    fn reads_the_empty_vehicle() {
        // What the game saves for a vehicle with no bricks: the header alone.
        let header = read_header(&[0x12, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(header.object_count(), 0);
        assert!(header.class_names.is_empty());
        assert_eq!(header.objects_end, 7);
    }

    #[test]
    fn reads_a_real_vehicles_start() {
        // A vehicle of one brick as the game wrote it (format 14, whose seven-byte header is
        // followed by a class and no properties).
        let mut bytes = vec![0x0e, 1, 0, 1, 0, 0, 0];
        bytes.extend(b"\x0aAircraftR4");
        bytes.extend([0, 0, 25, 0, 0, 0]);
        bytes.extend([0; 25]);
        let header = read_header(&bytes).unwrap();
        assert_eq!(header.class_names, ["AircraftR4"]);
        assert_eq!(header.class_indices, [0]);
        assert_eq!(header.objects_end, bytes.len());
    }

    #[test]
    fn accepts_the_formats_the_loader_reads() {
        for version in OLDEST_VERSION..=NEWEST_VERSION {
            let mut archive = vehicle();
            archive.version = version;
            assert!(read_header(&archive.bytes()).is_ok(), "format {version}");
        }
    }

    #[test]
    fn refuses_other_formats() {
        for version in [0, 1, 6, 19, 255] {
            let mut archive = vehicle();
            archive.version = version;
            assert_eq!(
                read_header(&archive.bytes()),
                Err(ArchiveError::UnsupportedVersion(version))
            );
        }
    }

    #[test]
    fn refuses_every_cut_short_archive() {
        // Cut anywhere before the last object ends, the archive is refused, never a panic. (The
        // tail may be cut: it isn't read.)
        let archive = vehicle();
        let bytes = archive.bytes();
        let end = read_header(&bytes).unwrap().objects_end;
        for cut in 0..end {
            assert!(
                matches!(
                    read_header(&bytes[..cut]),
                    Err(ArchiveError::Truncated { .. })
                ),
                "cut at {cut}"
            );
        }
        assert!(read_header(&bytes[..end]).is_ok());
    }

    #[test]
    fn refuses_class_counts_the_loader_refuses() {
        // More classes than objects.
        let mut bytes = vec![18, 1, 0, 2, 0, 0, 0];
        bytes.extend(b"\x01a\x01b");
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::BadCount {
                what: "class count",
                count: 2,
                max: 1
            })
        ));
    }

    #[test]
    fn refuses_more_properties_than_32_a_class() {
        let mut archive = Archive::new();
        archive.classes = vec!["A"];
        archive.objects = vec![(0, vec![])];
        // 33 one-byte-valued properties, one class: over the loader's cap, though the bytes fit.
        archive.properties = (0..33).map(|_| ("P", 1, vec![0], vec![])).collect();
        assert!(matches!(
            read_header(&archive.bytes()),
            Err(ArchiveError::BadCount {
                what: "property count",
                count: 33,
                max: 32
            })
        ));
        archive.properties.pop();
        assert!(read_header(&archive.bytes()).is_ok());
    }

    #[test]
    fn counts_the_bytes_cant_hold_are_refused_without_reserving() {
        // 65535 objects and classes claimed with nothing behind them: over the loader's cap of
        // classes...
        let bytes = [18, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::BadCount {
                what: "class count",
                ..
            })
        ));
        // ...and 65535 objects and 16 classes, which the bytes can't hold: refused as cut short,
        // before a vector is reserved for the claim.
        let bytes = [18, 0xFF, 0xFF, 0x10, 0x00, 0x00, 0x00];
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::Truncated { .. })
        ));
        // The most objects there can be, with a few classes and properties.
        let bytes = [18, 0xFF, 0xFF, 0x01, 0x00, 0x20, 0x00, 1, b'a'];
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::Truncated { .. })
        ));
        assert_eq!(MAX_OBJECTS, 65_535);
    }

    #[test]
    fn refuses_a_class_index_outside_the_table() {
        let mut archive = vehicle();
        archive.objects[1].0 = 2;
        assert_eq!(
            read_header(&archive.bytes()),
            Err(ArchiveError::ClassOutOfRange {
                object: 1,
                index: 2,
                classes: 2
            })
        );
        archive.objects[1].0 = u16::MAX;
        assert!(matches!(
            read_header(&archive.bytes()),
            Err(ArchiveError::ClassOutOfRange {
                index: u16::MAX,
                ..
            })
        ));
    }

    #[test]
    fn refuses_lengths_past_the_end() {
        // An object longer than what follows.
        let mut bytes = vehicle().bytes();
        let end = read_header(&bytes).unwrap().objects_end;
        bytes.truncate(end);
        let last = vehicle().objects.len() - 1;
        let last_len_at = read_header(&bytes).unwrap().bodies[last].start - 4;
        bytes[last_len_at..last_len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::Truncated {
                what: "an object's data"
            })
        ));

        // A property's values longer than the archive.
        let mut archive = Archive::new();
        archive.classes = vec!["A"];
        archive.properties = vec![("P", 1, vec![0; 4], vec![])];
        archive.objects = vec![(0, vec![])];
        let mut bytes = archive.bytes();
        // Header 7, class 2, property name 2, its value count 2, then the length.
        let len_at = 7 + 2 + 2 + 2;
        bytes[len_at..len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::Truncated { .. })
        ));
    }

    #[test]
    fn refuses_a_list_of_value_sizes_that_does_not_fit() {
        let mut archive = Archive::new();
        archive.classes = vec!["A"];
        // 65535 values claimed, size part says "a size each" but isn't there.
        archive.properties = vec![("P", u16::MAX, vec![], 0u16.to_le_bytes().to_vec())];
        archive.objects = vec![(0, vec![])];
        assert!(matches!(
            read_header(&archive.bytes()),
            Err(ArchiveError::Truncated { .. })
        ));
    }

    #[test]
    fn refuses_bad_names() {
        for name in [
            &b""[..],
            &[b'x'; MAX_NAME_LEN + 1],
            b"caf\xC3\xA9",
            b"tab\there",
            b"nul\0",
        ] {
            let mut bytes = vec![18, 0, 0, 0, 0, 0, 0];
            // One object of class 0, one class named `name`.
            bytes[1] = 1;
            bytes[3] = 1;
            bytes.push(name.len() as u8);
            bytes.extend(name);
            bytes.extend([0, 0, 0, 0, 0, 0]);
            assert!(
                matches!(read_header(&bytes), Err(ArchiveError::BadName { .. })),
                "{name:?}"
            );
        }
        // The longest name is fine.
        let long = "x".repeat(MAX_NAME_LEN);
        let mut bytes = vec![18, 1, 0, 1, 0, 0, 0, MAX_NAME_LEN as u8];
        bytes.extend(long.as_bytes());
        bytes.extend([0, 0, 0, 0, 0, 0]);
        assert_eq!(read_header(&bytes).unwrap().class_names, [long]);
        // A negative length byte (accepted by the loader as its absolute value) is refused here.
        let mut bytes = vec![18, 1, 0, 1, 0, 0, 0, 0xFE];
        bytes.extend(b"ab");
        bytes.extend([0, 0, 0, 0, 0, 0]);
        assert!(matches!(
            read_header(&bytes),
            Err(ArchiveError::BadName { .. })
        ));
    }

    #[test]
    fn never_panics_on_garbage() {
        // A cheap sweep: every prefix of a real archive with each byte replaced in turn by
        // extreme values must give an answer, not a panic or an allocation of the claimed size.
        let bytes = vehicle().bytes();
        for at in 0..bytes.len() {
            for value in [0x00, 0x01, 0x7F, 0x80, 0xFF] {
                let mut changed = bytes.clone();
                changed[at] = value;
                let _ = read_header(&changed);
            }
        }
    }
}
