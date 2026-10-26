//! Unreal pak files (`.pak`), versions 3 to 11: their index, and their files' bytes (stored or Zlib
//! compressed, not encrypted). The game's own pak is version 11; mods are made with whatever tool
//! their author had (version 3 and 11 seen). Layouts: UE 4.27 `IPlatformFilePak.h` (`FPakInfo`,
//! `FPakEntry`) and `FPakFile::LoadIndex` / `DecodePakEntry`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::bytes::Bytes;

/// What a pak's footer (`FPakInfo`) starts its fixed part with.
const MAGIC: u32 = 0x5A6F_12E1;

/// The versions read: 3 added compression blocks; 5 made their offsets relative to the entry;
/// 8 named compression methods; 9 added a frozen-index flag; 10 encoded the index's entries.
const OLDEST: i32 = 3;
const NEWEST: i32 = 11;

/// The fixed part of the footer from its magic on: magic, version, index offset and size, index
/// SHA-1.
const FOOTER_FIXED: usize = 4 + 4 + 8 + 8 + 20;

/// The largest index and file read (a brick mesh's package is kilobytes, a big mod's index a few
/// megabytes).
const MAX_INDEX: u64 = 256 << 20;
const MAX_FILE: u64 = 256 << 20;

/// A file in a pak.
#[derive(Clone, Debug)]
struct Entry {
    /// Where its in-pak header starts.
    offset: u64,
    /// Its size stored and uncompressed.
    size: u64,
    uncompressed: u64,
    /// 0 for stored; else the compression method (a name index from version 8, a flag before).
    method: u32,
    encrypted: bool,
    /// Compressed blocks: start and end, relative to `offset` from version 5, absolute before.
    blocks: Vec<(u64, u64)>,
}

/// An open pak: where it is, and its files by their path from the game's folder (e.g.
/// `BrickRigs/Mods/Foo/Content/Bricks/Bar.uasset`).
pub struct Pak {
    path: PathBuf,
    version: i32,
    /// From version 8, compression methods by name, for `Entry::method - 1`.
    methods: Vec<String>,
    files: HashMap<String, Entry>,
}

impl Pak {
    /// Reads `path`'s index.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        let length = file
            .seek(SeekFrom::End(0))
            .map_err(|error| error.to_string())?;
        let tail_length = length.min(1024);
        let tail = read_at(&mut file, length - tail_length, tail_length)?;
        let footer = Footer::find(&tail)?;
        if footer.index_size > MAX_INDEX
            || footer
                .index_offset
                .checked_add(footer.index_size)
                .is_none_or(|end| end > length)
        {
            return Err(format!(
                "index out of the file ({} bytes at {})",
                footer.index_size, footer.index_offset
            ));
        }
        let index = read_at(&mut file, footer.index_offset, footer.index_size)?;
        let mut pak = Self {
            path: path.to_owned(),
            version: footer.version,
            methods: footer.methods,
            files: HashMap::new(),
        };
        pak.read_index(&mut file, &index, length)?;
        Ok(pak)
    }

    /// Every file's path.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    /// The bytes of the file `name`.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        let entry = self
            .files
            .get(name)
            .ok_or_else(|| format!("{name} isn't in the pak"))?;
        if entry.encrypted {
            return Err(format!("{name} is encrypted"));
        }
        if entry.uncompressed > MAX_FILE {
            return Err(format!("{name} is too big ({} bytes)", entry.uncompressed));
        }
        let mut file = File::open(&self.path).map_err(|error| error.to_string())?;
        if entry.method == 0 {
            let header = entry_header_size(self.version, 0);
            return read_at(&mut file, entry.offset + header, entry.uncompressed);
        }
        let method = self.method_name(entry.method);
        if !method.eq_ignore_ascii_case("zlib") {
            return Err(format!(
                "{name} is compressed with {method}, which isn't read"
            ));
        }
        let mut bytes = Vec::with_capacity(entry.uncompressed as usize);
        for &(start, end) in &entry.blocks {
            let (start, end) = if self.version >= 5 {
                (entry.offset + start, entry.offset + end)
            } else {
                (start, end)
            };
            let size = end
                .checked_sub(start)
                .filter(|&size| size <= entry.size)
                .ok_or_else(|| format!("{name}: bad compression block"))?;
            let block = read_at(&mut file, start, size)?;
            let left = entry.uncompressed as usize - bytes.len();
            let inflated = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&block, left)
                .map_err(|error| format!("{name}: can't decompress ({error:?})"))?;
            bytes.extend(inflated);
        }
        if bytes.len() as u64 != entry.uncompressed {
            return Err(format!(
                "{name}: {} bytes decompressed, {} expected",
                bytes.len(),
                entry.uncompressed
            ));
        }
        Ok(bytes)
    }

    fn method_name(&self, method: u32) -> String {
        if self.version >= 8 {
            let name = self.methods.get(method as usize - 1);
            name.cloned().unwrap_or_else(|| format!("method {method}"))
        } else {
            // `ECompressionFlags`: COMPRESS_ZLIB is 1, the bias flags (0x10, 0x20) don't matter.
            match method & 0x0F {
                1 => "Zlib".into(),
                other => format!("method {other}"),
            }
        }
    }

    fn read_index(&mut self, file: &mut File, index: &[u8], length: u64) -> Result<(), String> {
        let mut bytes = Bytes::new(index);
        let mount = mount_prefix(&bytes.string()?);
        let count = bytes.count(1)?;
        if self.version < 10 {
            for _ in 0..count {
                let name = bytes.string()?;
                let entry = read_entry(&mut bytes, self.version)?;
                self.files.insert(format!("{mount}{name}"), entry);
            }
            return Ok(());
        }
        let _path_hash_seed = bytes.u64()?;
        if bytes.bool32()? {
            // The path hash index: offset, size, SHA-1.
            bytes.skip(8 + 8 + 20)?;
        }
        if !bytes.bool32()? {
            return Err("the pak has no directory index".into());
        }
        let directory_offset = bytes.u64()?;
        let directory_size = bytes.u64()?;
        bytes.skip(20)?;
        let encoded_size = bytes.count(1)?;
        let encoded = bytes.take(encoded_size)?;
        let plain_count = bytes.count(1)?;
        let plain = (0..plain_count)
            .map(|_| read_entry(&mut bytes, self.version))
            .collect::<Result<Vec<_>, _>>()?;
        if directory_size > MAX_INDEX
            || directory_offset
                .checked_add(directory_size)
                .is_none_or(|end| end > length)
        {
            return Err("directory index out of the file".into());
        }
        let directory = read_at(file, directory_offset, directory_size)?;
        let mut bytes = Bytes::new(&directory);
        for _ in 0..bytes.count(4)? {
            let folder = bytes.string()?;
            let folder = folder.trim_start_matches('/');
            for _ in 0..bytes.count(4)? {
                let name = bytes.string()?;
                let location = bytes.i32()?;
                let entry = if location >= 0 {
                    decode_entry(encoded, location as usize, self.version)?
                } else {
                    plain
                        .get((-(location + 1)) as usize)
                        .cloned()
                        .ok_or("entry index out of range")?
                };
                self.files.insert(format!("{mount}{folder}{name}"), entry);
            }
        }
        Ok(())
    }
}

/// What the pak's footer says.
struct Footer {
    version: i32,
    index_offset: u64,
    index_size: u64,
    methods: Vec<String>,
}

impl Footer {
    /// Finds the footer at the end of `tail` (the file's last bytes): its fixed part must end
    /// exactly where the version's footer ends.
    fn find(tail: &[u8]) -> Result<Self, String> {
        let magic = MAGIC.to_le_bytes();
        for at in (0..tail.len().saturating_sub(FOOTER_FIXED - 1)).rev() {
            if tail[at..at + 4] != magic {
                continue;
            }
            let mut bytes = Bytes::at(tail, at + 4)?;
            let version = bytes.i32()?;
            if !(OLDEST..=NEWEST).contains(&version) {
                if (1..=20).contains(&version) {
                    return Err(format!("pak version {version} isn't read"));
                }
                continue;
            }
            let index_offset = bytes.u64()?;
            let index_size = bytes.u64()?;
            bytes.skip(20)?;
            if version == 9 {
                // bIndexIsFrozen
                bytes.skip(1)?;
            }
            let names_bytes = tail.len() - bytes.position();
            let methods = if version >= 8 {
                if !names_bytes.is_multiple_of(32) || !(4..=5).contains(&(names_bytes / 32)) {
                    continue;
                }
                (0..names_bytes / 32)
                    .map(|_| {
                        let name = bytes.take(32)?;
                        let end = name.iter().position(|&byte| byte == 0).unwrap_or(32);
                        Ok(String::from_utf8_lossy(&name[..end]).into_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()?
            } else {
                if names_bytes != 0 {
                    continue;
                }
                Vec::new()
            };
            // bEncryptedIndex is the byte before the magic (from version 4).
            if version >= 4 && at > 0 && tail[at - 1] != 0 {
                return Err("the pak's index is encrypted".into());
            }
            return Ok(Self {
                version,
                index_offset,
                index_size,
                methods,
            });
        }
        Err("not a pak file".into())
    }
}

/// The path files are below, from the pak's mount point: relative to the game's folder, which the
/// game's mount points (`../../../`, from its executable's folder) are.
fn mount_prefix(mount: &str) -> String {
    let mut prefix = mount.replace('\\', "/");
    while let Some(rest) = prefix.strip_prefix("../") {
        prefix = rest.to_owned();
    }
    let prefix = prefix.trim_start_matches('/');
    if prefix.is_empty() || prefix.ends_with('/') {
        prefix.to_owned()
    } else {
        format!("{prefix}/")
    }
}

/// The size of the header an entry's data starts with in the pak (an `FPakEntry`, serialized).
fn entry_header_size(version: i32, blocks: usize) -> u64 {
    let mut size = 8 + 8 + 8 + 4 + 20;
    if version >= 3 {
        if blocks > 0 {
            size += 4 + 16 * blocks as u64;
        }
        size += 1 + 4;
    }
    size
}

/// An `FPakEntry` as the index serializes it.
fn read_entry(bytes: &mut Bytes, version: i32) -> Result<Entry, String> {
    let offset = bytes.u64()?;
    let size = bytes.u64()?;
    let uncompressed = bytes.u64()?;
    let method = bytes.u32()?;
    if version < 2 {
        // Timestamp.
        bytes.skip(8)?;
    }
    bytes.skip(20)?;
    let mut blocks = Vec::new();
    if method != 0 {
        for _ in 0..bytes.count(16)? {
            blocks.push((bytes.u64()?, bytes.u64()?));
        }
    }
    let flags = bytes.u8()?;
    let _block_size = bytes.u32()?;
    Ok(Entry {
        offset,
        size,
        uncompressed,
        method,
        encrypted: flags & 1 != 0,
        blocks,
    })
}

/// An entry of a version 10+ index, encoded at `at` of `encoded` (`FPakFile::DecodePakEntry`): a
/// bit field, then the numbers it says are there.
fn decode_entry(encoded: &[u8], at: usize, version: i32) -> Result<Entry, String> {
    let mut bytes = Bytes::at(encoded, at)?;
    let bits = bytes.u32()?;
    let method = (bits >> 23) & 0x3F;
    let encrypted = bits & (1 << 22) != 0;
    let block_count = ((bits >> 6) & 0xFFFF) as usize;
    if bits & 0x3F == 0x3F {
        // The block size, when it isn't a multiple of 2 KiB.
        bytes.u32()?;
    }
    let mut number = |bit: u32| -> Result<u64, String> {
        if bits & (1 << bit) != 0 {
            bytes.u32().map(u64::from)
        } else {
            bytes.u64()
        }
    };
    let offset = number(31)?;
    let uncompressed = number(30)?;
    let size = if method != 0 {
        number(29)?
    } else {
        uncompressed
    };
    let mut blocks = Vec::with_capacity(block_count);
    let mut start = entry_header_size(version, block_count);
    if block_count == 1 && !encrypted {
        blocks.push((start, start + size));
    } else {
        for _ in 0..block_count {
            let block_size = u64::from(bytes.u32()?);
            blocks.push((start, start + block_size));
            // Encrypted blocks are padded to the AES block size.
            start += if encrypted {
                block_size.next_multiple_of(16)
            } else {
                block_size
            };
        }
    }
    Ok(Entry {
        offset,
        size,
        uncompressed,
        method,
        encrypted,
        blocks,
    })
}

/// `size` bytes of `file` from `offset`.
fn read_at(file: &mut File, offset: u64, size: u64) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(size)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != size {
        return Err(format!("unexpected end of the pak file at {offset}"));
    }
    Ok(bytes)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A version 3 pak holding `files` (name, bytes, whether to compress them), mounted at
    /// `../../../`.
    pub fn version_3(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        fn string(out: &mut Vec<u8>, text: &str) {
            out.extend((text.len() as i32 + 1).to_le_bytes());
            out.extend(text.as_bytes());
            out.push(0);
        }
        fn entry(out: &mut Vec<u8>, offset: u64, size: u64, raw: u64, blocks: &[(u64, u64)]) {
            out.extend(offset.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend(raw.to_le_bytes());
            out.extend(u32::from(!blocks.is_empty()).to_le_bytes());
            out.extend([0; 20]);
            if !blocks.is_empty() {
                out.extend((blocks.len() as i32).to_le_bytes());
                for (start, end) in blocks {
                    out.extend(start.to_le_bytes());
                    out.extend(end.to_le_bytes());
                }
            }
            out.push(0);
            out.extend(65536u32.to_le_bytes());
        }
        let mut pak = Vec::new();
        let mut index = Vec::new();
        string(&mut index, "../../../");
        index.extend((files.len() as i32).to_le_bytes());
        for (name, bytes, compress) in files {
            let offset = pak.len() as u64;
            let stored = if *compress {
                miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6)
            } else {
                bytes.to_vec()
            };
            let size = stored.len() as u64;
            let raw = bytes.len() as u64;
            let blocks: Vec<(u64, u64)> = if *compress {
                // Absolute offsets before version 5.
                let start = offset + entry_header_size(3, 1);
                vec![(start, start + size)]
            } else {
                Vec::new()
            };
            entry(&mut pak, offset, size, raw, &blocks);
            pak.extend(&stored);
            string(&mut index, name);
            entry(&mut index, offset, size, raw, &blocks);
        }
        let index_offset = pak.len() as u64;
        pak.extend(&index);
        pak.extend(MAGIC.to_le_bytes());
        pak.extend(3i32.to_le_bytes());
        pak.extend(index_offset.to_le_bytes());
        pak.extend((index.len() as u64).to_le_bytes());
        pak.extend([0; 20]);
        pak
    }

    #[test]
    fn reads_version_3() {
        let folder = std::env::temp_dir().join(format!("model_io-pak-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("test.pak");
        let text = b"hello hello hello hello".repeat(40);
        std::fs::write(
            &path,
            version_3(&[
                ("Game/Content/a.uasset", b"stored", false),
                ("Game/Content/b.uexp", &text, true),
            ]),
        )
        .unwrap();
        let pak = Pak::open(&path).unwrap();
        let mut files: Vec<&str> = pak.files().collect();
        files.sort_unstable();
        assert_eq!(files, ["Game/Content/a.uasset", "Game/Content/b.uexp"]);
        assert_eq!(pak.read("Game/Content/a.uasset").unwrap(), b"stored");
        assert_eq!(pak.read("Game/Content/b.uexp").unwrap(), text);
        assert!(pak.read("missing").is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn mount_points() {
        assert_eq!(mount_prefix("../../../"), "");
        assert_eq!(
            mount_prefix("../../../BrickRigs/Mods/Foo/"),
            "BrickRigs/Mods/Foo/"
        );
        assert_eq!(mount_prefix("../../../Engine"), "Engine/");
    }

    #[test]
    fn decodes_entries() {
        // Offset, sizes in 32 bits, Zlib (method 1), one block of 64 KiB.
        let bits: u32 = (1 << 31) | (1 << 30) | (1 << 29) | (1 << 23) | (1 << 6) | 32;
        let mut encoded = bits.to_le_bytes().to_vec();
        encoded.extend(1000u32.to_le_bytes());
        encoded.extend(500u32.to_le_bytes());
        encoded.extend(200u32.to_le_bytes());
        let entry = decode_entry(&encoded, 0, 11).unwrap();
        assert_eq!(
            (entry.offset, entry.uncompressed, entry.size),
            (1000, 500, 200)
        );
        let header = entry_header_size(11, 1);
        assert_eq!(entry.blocks, [(header, header + 200)]);
    }
}
