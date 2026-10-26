//! Cooked UE 4.27 packages: the `.uasset` header (`FPackageFileSummary`, then its names, imports and
//! exports) and where each export's data is (in the `.uexp` next to it once cooked). Only what
//! finding an export needs; layouts from UE 4.27 `PackageFileSummary.cpp`, `ObjectResource.cpp`.

use super::bytes::Bytes;

/// What every package starts with.
const TAG: u32 = 0x9E2A_83C1;

/// `PKG_FilterEditorOnly`: cooked, editor-only data left out.
const FILTER_EDITOR_ONLY: u32 = 0x8000_0000;
/// `PKG_UnversionedProperties`: properties saved without their names (read only with the classes'
/// layouts).
const UNVERSIONED_PROPERTIES: u32 = 0x2000;

/// `VER_UE4_64BIT_EXPORTMAP_SERIALSIZES`: the oldest file version read (UE 4.20). Cooked packages
/// usually say 0 ("this engine's"), taken as 4.27's.
const OLDEST_VERSION: i32 = 511;

/// An object a package uses from another package (or a class).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub class_name: String,
    /// `FPackageIndex`: 0 for none, -1 - n for import n, n + 1 for export n.
    pub outer: i32,
    pub name: String,
}

/// An object a package holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    /// Its class, as an `FPackageIndex`.
    pub class: i32,
    pub outer: i32,
    pub name: String,
    /// Where its data is: an offset into the package as if the `.uexp` followed the `.uasset`.
    pub serial_offset: u64,
    pub serial_size: u64,
}

/// A package's header.
pub struct Package {
    /// The `.uasset`'s size, which the `.uexp`'s offsets start from.
    pub header_size: u64,
    pub names: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}

impl Package {
    /// Reads the header in `asset` (a `.uasset`).
    pub fn parse(asset: &[u8]) -> Result<Self, String> {
        let mut bytes = Bytes::new(asset);
        if bytes.u32()? != TAG {
            return Err("not an Unreal package".into());
        }
        let legacy = bytes.i32()?;
        // -7 is 4.27's; -6 (4.25, 4.26) has the same layout. Older ones store custom versions
        // differently, newer ones are UE5's.
        if !(-7..=-6).contains(&legacy) {
            return Err(format!("package format {legacy} isn't read"));
        }
        let _ue3_version = bytes.i32()?;
        let version = bytes.i32()?;
        if version != 0 && version < OLDEST_VERSION {
            return Err(format!("package version {version} isn't read"));
        }
        let _licensee_version = bytes.i32()?;
        // Custom versions: a GUID and a version each.
        let customs = bytes.count(20)?;
        bytes.skip(customs * 20)?;
        let header_size = bytes.i32()?;
        let _folder = bytes.string()?;
        let flags = bytes.u32()?;
        if flags & UNVERSIONED_PROPERTIES != 0 {
            return Err("the package's properties are unversioned".into());
        }
        let name_count = bytes.i32()?;
        let name_offset = bytes.i32()?;
        if flags & FILTER_EDITOR_ONLY == 0 {
            let _localization_id = bytes.string()?;
        }
        // Gatherable text: count and offset.
        bytes.skip(8)?;
        let export_count = bytes.i32()?;
        let export_offset = bytes.i32()?;
        let import_count = bytes.i32()?;
        let import_offset = bytes.i32()?;
        // Depends offset, soft package references (count, offset), searchable names offset,
        // thumbnail table offset, GUID.
        bytes.skip(4 + 8 + 4 + 4 + 16)?;
        if flags & FILTER_EDITOR_ONLY == 0 {
            // Persistent GUID.
            bytes.skip(16)?;
        }
        let header_size = u64::try_from(header_size).map_err(|_| "bad header size")?;
        let mut package = Self {
            header_size,
            names: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
        };
        let mut names = Bytes::at(asset, offset(name_offset)?)?;
        for _ in 0..count(name_count)? {
            package.names.push(names.string()?);
            // Its hashes.
            names.skip(4)?;
        }
        let mut imports = Bytes::at(asset, offset(import_offset)?)?;
        for _ in 0..count(import_count)? {
            let _class_package = package.name(&mut imports)?;
            let class_name = package.name(&mut imports)?;
            let outer = imports.i32()?;
            let name = package.name(&mut imports)?;
            package.imports.push(Import {
                class_name,
                outer,
                name,
            });
        }
        let mut exports = Bytes::at(asset, offset(export_offset)?)?;
        for _ in 0..count(export_count)? {
            let class = exports.i32()?;
            let _super = exports.i32()?;
            let _template = exports.i32()?;
            let outer = exports.i32()?;
            let name = package.name(&mut exports)?;
            let _flags = exports.u32()?;
            let serial_size = exports.i64()?;
            let serial_offset = exports.i64()?;
            // Forced export, not for client, not for server, package GUID, package flags, not
            // always loaded for editor game, is asset, five preload dependency numbers.
            exports.skip(4 + 4 + 4 + 16 + 4 + 4 + 4 + 20)?;
            package.exports.push(Export {
                class,
                outer,
                name,
                serial_offset: u64::try_from(serial_offset).map_err(|_| "bad export offset")?,
                serial_size: u64::try_from(serial_size).map_err(|_| "bad export size")?,
            });
        }
        Ok(package)
    }

    /// An `FName`: an index into the names and a number (`Name_2` is number 3).
    pub fn name(&self, bytes: &mut Bytes) -> Result<String, String> {
        let index = bytes.i32()?;
        let number = bytes.i32()?;
        let name = usize::try_from(index)
            .ok()
            .and_then(|index| self.names.get(index))
            .ok_or_else(|| format!("name {index} out of range"))?;
        Ok(if number > 0 {
            format!("{name}_{}", number - 1)
        } else {
            name.clone()
        })
    }

    /// The name of the class `class` (an `FPackageIndex`) is, if it's an import (native classes
    /// are).
    pub fn class_name(&self, class: i32) -> Option<&str> {
        let import = usize::try_from(-(class.checked_add(1)?)).ok()?;
        self.imports.get(import).map(|import| import.name.as_str())
    }

    /// The export named `name` of the class `class`, at the top of the package.
    pub fn export(&self, name: &str, class: &str) -> Option<&Export> {
        self.exports.iter().find(|export| {
            export.outer == 0 && export.name == name && self.class_name(export.class) == Some(class)
        })
    }

    /// `export`'s data, from the `.uexp` (`exports`) or, for a package that isn't split, the
    /// `.uasset` (`asset`).
    pub fn data<'a>(
        &self,
        export: &Export,
        asset: &'a [u8],
        exports: Option<&'a [u8]>,
    ) -> Result<&'a [u8], String> {
        let (data, start) = match exports {
            Some(exports) => (
                exports,
                export
                    .serial_offset
                    .checked_sub(self.header_size)
                    .ok_or("export data in the header")?,
            ),
            None => (asset, export.serial_offset),
        };
        let end = start
            .checked_add(export.serial_size)
            .filter(|&end| end <= data.len() as u64)
            .ok_or_else(|| format!("export {} out of the file", export.name))?;
        Ok(&data[start as usize..end as usize])
    }
}

fn offset(value: i32) -> Result<usize, String> {
    usize::try_from(value).map_err(|_| format!("bad offset {value}"))
}

fn count(value: i32) -> Result<usize, String> {
    usize::try_from(value)
        .ok()
        .filter(|&count| count < 1 << 24)
        .ok_or_else(|| format!("bad count {value}"))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Builds a cooked package header (the `.uasset`) with `names`, `imports` (class name, outer,
    /// name) and `exports` (class, name, data size), the data in a `.uexp`.
    pub fn header(
        names: &[&str],
        imports: &[(&str, i32, &str)],
        exports: &[(i32, &str, u64)],
    ) -> Vec<u8> {
        let name_index = |name: &str| names.iter().position(|&known| known == name).unwrap() as i32;
        let fname = |out: &mut Vec<u8>, name: &str| {
            out.extend(name_index(name).to_le_bytes());
            out.extend(0i32.to_le_bytes());
        };
        let i32s = |out: &mut Vec<u8>, values: &[i32]| {
            for value in values {
                out.extend(value.to_le_bytes());
            }
        };
        // The summary's fixed size up to the name offset and after it, with the offsets patched in
        // below.
        let mut summary = Vec::new();
        summary.extend(TAG.to_le_bytes());
        i32s(&mut summary, &[-7, 864, 0, 0, 0]);
        let header_size_at = summary.len();
        i32s(&mut summary, &[0]);
        summary.extend(5i32.to_le_bytes());
        summary.extend(b"None\0");
        summary.extend(FILTER_EDITOR_ONLY.to_le_bytes());
        let tables_at = summary.len();
        // Names (count, offset), gatherable text, exports, imports.
        i32s(&mut summary, &[0; 8]);
        summary.extend([0; 4 + 8 + 4 + 4 + 16]);
        // The rest isn't read.
        summary.extend([0; 64]);

        let mut tables = Vec::new();
        let names_at = summary.len();
        for name in names {
            tables.extend((name.len() as i32 + 1).to_le_bytes());
            tables.extend(name.as_bytes());
            tables.push(0);
            tables.extend([0; 4]);
        }
        let imports_at = summary.len() + tables.len();
        for (class, outer, name) in imports {
            fname(&mut tables, "None");
            fname(&mut tables, class);
            tables.extend(outer.to_le_bytes());
            fname(&mut tables, name);
        }
        let exports_at = summary.len() + tables.len();
        let header_size = (exports_at + exports.len() * 104) as u64;
        let mut offset = header_size;
        for (class, name, size) in exports {
            i32s(&mut tables, &[*class, 0, 0, 0]);
            fname(&mut tables, name);
            tables.extend(0u32.to_le_bytes());
            tables.extend((*size as i64).to_le_bytes());
            tables.extend((offset as i64).to_le_bytes());
            tables.extend([0; 60]);
            offset += size;
        }
        let mut patch =
            |at: usize, value: i32| summary[at..at + 4].copy_from_slice(&value.to_le_bytes());
        patch(header_size_at, header_size as i32);
        patch(tables_at, names.len() as i32);
        patch(tables_at + 4, names_at as i32);
        patch(tables_at + 16, exports.len() as i32);
        patch(tables_at + 20, exports_at as i32);
        patch(tables_at + 24, imports.len() as i32);
        patch(tables_at + 28, imports_at as i32);
        summary.extend(tables);
        summary
    }

    #[test]
    fn reads_the_header() {
        let asset = header(
            &["None", "Class", "StaticMesh", "Wing", "BodySetup"],
            &[("Class", 0, "StaticMesh"), ("Class", 0, "BodySetup")],
            &[(-2, "BodySetup", 10), (-1, "Wing", 20)],
        );
        let package = Package::parse(&asset).unwrap();
        assert_eq!(package.header_size, asset.len() as u64);
        let wing = package.export("Wing", "StaticMesh").unwrap();
        assert_eq!(wing.serial_offset, package.header_size + 10);
        assert!(package.export("Wing", "BodySetup").is_none());
        let exports: Vec<u8> = (0..30).collect();
        let data = package.data(wing, &asset, Some(&exports)).unwrap();
        assert_eq!(data, &exports[10..30]);
    }

    #[test]
    fn refuses_other_files() {
        assert!(Package::parse(b"not a package at all").is_err());
    }
}
