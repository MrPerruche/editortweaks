//! A brick's properties as values that can leave the game and come back (coop's `Set` ops, only in
//! builds with the `coop` feature). Built from the receiver's own reflection, so nothing from
//! outside decides where in memory anything is written. Each brick property gives one or more
//! entries, each with its own value:
//!
//! - **Data:** numbers, enums, bools and structs of them (colors, vectors, sizes, choices, an input
//!   channel's axis and value). The value is the property's reflected, editable fields, each as the
//!   bytes the game keeps it in, one after the other: nothing of a struct's padding or of its
//!   members that aren't reflected. Written back field by field, after checking every one: the
//!   exact size, floats finite, enums one of their entries (never the `_MAX` one), bools 0 or 1.
//! - **Assets:** an object or class property holding a data asset (a material, a pattern, a fuel
//!   type...) travels as the asset's path name. It's written only if the receiver already has a
//!   live object of that path and of the property's class; nothing is loaded. Empty means none,
//!   allowed only where the brick class' defaults have none.
//! - **Strings** (seat, switch and camera names, a text brick's text) travel as text, written with
//!   the property's own copy (the game allocates the copy): no character 0, at most [`MAX_TEXT`].
//! - **Bricks** (`FBrickEditorObjectPtr`: an input channel's `SourceBricks`, a camera's
//!   `OwningSeat`) travel as the bricks, which coop turns into its ids and back. Written as the
//!   game makes one: a weak pointer to the brick and its id, `GetEditorObjectID`.
//! - **Connector spacing** (`FScalableBrickConnectorSpacing`, no reflected fields) travels as one
//!   `EConnectorSpacing` per face, read and written with the game's own functions
//!   (`UBrickStatics::Get/SetScalableBrickConnectorSpacingAxis`), so its bits are never ours to
//!   know. Which axes are faces and which spacings a face holds is found out by calling them.
//!
//! A struct's strings and bricks are entries of their own, named by path (`InputChannel.
//! SourceBricks`); the rest of it is the entry named like the property. Anything else isn't synced:
//! [`ClassValues::skipped`] lists it, and [`BrickValues::inventory`] every brick class'. Game
//! 1.11.7 has nothing else (`docs/re/coop/brick-property-types.md`).
//!
//! UE 4.27 layouts used, each right after the `FProperty` fields (0x78 bytes): `FByteProperty::Enum`,
//! `FEnumProperty::UnderlyingProp` then `Enum`, `FObjectPropertyBase::PropertyClass`, then
//! `FClassProperty::MetaClass`; `UEnum::Names` at [`reflection::ENUM_NAMES`]. `FString` and
//! `TArray` are `{data, num, max}`. `FBrickEditorObjectPtr` (12 bytes, checked) is an
//! `FWeakObjectPtr` and the 2-byte `FBrickEditorObjectID` (checked), `docs/re/coop/protocol.md`.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::tarray::TArray;
use brickrust::ue::tshared::FWeakObjectPtr;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::game::brick_property;
use crate::game::property_copy;
use crate::game::reflection::{self, BoolField, EnumEntry, Getter, Parameters};
use crate::game::slider_text::OwnedFString;
use crate::game::weak_object;

/// Most entries an enum may have to be trusted as one.
const MAX_ENUM_ENTRIES: usize = 1024;
/// Deepest struct nesting followed (a struct can't hold itself, but a broken record could loop).
const MAX_DEPTH: usize = 8;
/// Longest string accepted, in UTF-16 units.
pub const MAX_TEXT: usize = 1 << 16;
/// Most bricks one property may refer to.
pub const MAX_BRICKS: usize = 4096;
/// `EPropertyFlags::CPF_Edit`: a struct's fields the panel shows (an output channel's current
/// value, say, isn't one).
const EDIT: u64 = 0x1;

/// One reflected field of a property's value.
#[derive(Clone)]
struct Leaf {
    /// Where the object keeps it.
    offset: usize,
    kind: LeafKind,
}

#[derive(Clone)]
enum LeafKind {
    Float,
    Double,
    /// An integer of this many bytes; any value is one.
    Int(usize),
    /// An enum's value, of this many bytes: one of `values`.
    Enum {
        size: usize,
        values: Arc<[i64]>,
    },
    /// One byte, 0 or 1, outside; inside the object maybe one bit of a bitfield.
    Bool(BoolField),
}

impl LeafKind {
    /// Bytes it takes in a value.
    fn size(&self) -> usize {
        match self {
            Self::Float => 4,
            Self::Double => 8,
            Self::Int(size) | Self::Enum { size, .. } => *size,
            Self::Bool(_) => 1,
        }
    }

    /// Whether `bytes` (of [`Self::size`]) are a value this field may hold.
    fn allows(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Float => f32::from_le_bytes(bytes.try_into().expect("4 bytes")).is_finite(),
            Self::Double => f64::from_le_bytes(bytes.try_into().expect("8 bytes")).is_finite(),
            Self::Int(_) => true,
            Self::Enum { values, .. } => {
                let mut wide = [0u8; 8];
                wide[..bytes.len()].copy_from_slice(bytes);
                let value = i64::from_le_bytes(wide);
                // Entries are stored as i64; an unsigned 8-byte enum's large values wrap alike.
                values.contains(&value)
            }
            Self::Bool(_) => bytes[0] <= 1,
        }
    }
}

/// How a property's value (or a part of it) leaves the game.
enum Shape {
    /// Its [`Leaf`]s' bytes, `size` in all.
    Data { leaves: Vec<Leaf>, size: usize },
    /// A data asset (an object pointer at `offset`), by path name.
    Asset {
        offset: usize,
        /// The objects it may hold are of this class.
        class: *const UStruct,
        /// For a class property: the classes it may hold are this one or its children.
        meta: Option<*const UStruct>,
        /// Whether it may hold none (the brick class' defaults do).
        nullable: bool,
    },
    /// An `FString` at `offset`, of `property`.
    Str {
        offset: usize,
        property: *const FProperty,
    },
    /// `FBrickEditorObjectPtr`s at `offset`: one (a `StructProperty`), or a `TArray` of them
    /// (`many`, an `ArrayProperty`), of `property`.
    Bricks {
        offset: usize,
        property: *const FProperty,
        many: bool,
    },
    /// An `FScalableBrickConnectorSpacing` at `offset`, of `property`.
    Spacing {
        offset: usize,
        property: *const FProperty,
    },
}

/// A property's value as it travels.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// [`Shape::Data`]'s bytes, or a connector spacing's, one per face.
    Data(Vec<u8>),
    /// A string, or an asset's path name (empty for none).
    Text(String),
    /// Editor objects of the same editor, live.
    Bricks(Vec<*mut UObjectBase>),
}

/// One brick property, or part of one, that can be synced.
pub struct PropertyValue {
    /// Its path: the property's name, then its fields' if it's a part (`InputChannel.SourceBricks`).
    pub name: String,
    /// The brick property's name, for change events.
    pub fname: FName,
    shape: Shape,
}

// SAFETY: reflection records and assets living as long as the game; only used on the game thread.
unsafe impl Send for PropertyValue {}
unsafe impl Sync for PropertyValue {}

impl PropertyValue {
    /// The float fields of `bytes`, a value of this property as it travels, that differ from
    /// `current` (the value held now, as it travels); none if it isn't plain data.
    fn changed_floats(&self, bytes: &[u8], current: &[u8]) -> Vec<f64> {
        let Shape::Data { leaves, size } = &self.shape else {
            return Vec::new();
        };
        if bytes.len() != *size || current.len() != *size {
            return Vec::new();
        }
        let mut floats = Vec::new();
        let mut at = 0;
        for leaf in leaves {
            let range = at..at + leaf.kind.size();
            at = range.end;
            let part = &bytes[range.clone()];
            if part == &current[range] {
                continue;
            }
            match leaf.kind {
                LeafKind::Float => floats.push(f64::from(f32::from_le_bytes(
                    part.try_into().expect("4 bytes"),
                ))),
                LeafKind::Double => {
                    floats.push(f64::from_le_bytes(part.try_into().expect("8 bytes")))
                }
                _ => {}
            }
        }
        floats
    }

    /// Appends the value `object` holds, as kept locally: data as it travels, an asset as its
    /// address and its slot in the object array (to tell later whether it's still there), a string
    /// as its characters, bricks as their addresses (followed only in [`Self::value`]) and a
    /// connector spacing as its bytes.
    ///
    /// # Safety
    /// `object` must be a live object of a class having the property. Game thread only.
    unsafe fn read(&self, object: *const UObjectBase, out: &mut Vec<u8>) {
        let base = object.cast::<u8>();
        // SAFETY: per the contract, every part lies within the object, as reflection says.
        unsafe {
            match &self.shape {
                Shape::Data { leaves, .. } => {
                    for leaf in leaves {
                        match &leaf.kind {
                            LeafKind::Bool(field) => out.push(u8::from(field.get(base))),
                            kind => out.extend_from_slice(core::slice::from_raw_parts(
                                base.add(leaf.offset),
                                kind.size(),
                            )),
                        }
                    }
                }
                Shape::Asset { offset, .. } => {
                    let asset = base
                        .add(*offset)
                        .cast::<*mut UObjectBase>()
                        .read_unaligned();
                    // A live brick's asset property holds null or a live object.
                    let index = if asset.is_null() {
                        0
                    } else {
                        (*asset).internal_index
                    };
                    out.extend_from_slice(&(asset as usize).to_le_bytes());
                    out.extend_from_slice(&index.to_le_bytes());
                }
                Shape::Str { offset, .. } => {
                    for unit in string_units(base.add(*offset)) {
                        out.extend_from_slice(&unit.to_le_bytes());
                    }
                }
                Shape::Bricks { offset, many, .. } => {
                    let at = base.add(*offset);
                    let (first, count) = if *many {
                        let array = &*at.cast::<TArray<u8>>();
                        match usize::try_from(array.num) {
                            Ok(count) if !array.data.is_null() => (array.data.cast_const(), count),
                            _ => (at, 0),
                        }
                    } else {
                        (at, 1)
                    };
                    for index in 0..count {
                        let element = first.add(index * BRICK_POINTER_SIZE);
                        let pointer = element.cast::<FWeakObjectPtr>().read_unaligned();
                        let address =
                            weak_object::resolve(pointer).map_or(0, |found| found as usize);
                        out.extend_from_slice(&address.to_le_bytes());
                    }
                }
                Shape::Spacing { offset, property } => {
                    out.extend_from_slice(core::slice::from_raw_parts(
                        base.add(*offset),
                        (**property).element_size as usize,
                    ))
                }
            }
        }
    }

    /// The value as it travels, from its local copy (see [`Self::read`]).
    ///
    /// # Safety
    /// `local` must have been read just now (addresses are followed). Game thread only.
    unsafe fn value(&self, local: &[u8], known: &Known) -> Value {
        match self.shape {
            Shape::Data { .. } => Value::Data(local.to_vec()),
            Shape::Asset { .. } => {
                let (address, _) = asset_of(local);
                let object = address as *const UObjectBase;
                // SAFETY: per the contract, the property holds null or a live object.
                Value::Text(if object.is_null() {
                    String::new()
                } else {
                    unsafe { path_of(object) }
                })
            }
            Shape::Str { .. } => {
                let units: Vec<u16> = local
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&unit| u16::from_le_bytes(unit))
                    .collect();
                Value::Text(String::from_utf16_lossy(&units))
            }
            Shape::Bricks { .. } => Value::Bricks(
                local
                    .as_chunks::<{ size_of::<usize>() }>()
                    .0
                    .iter()
                    .map(|&address| usize::from_le_bytes(address))
                    .filter(|&address| address != 0)
                    .map(|address| address as *mut UObjectBase)
                    .collect(),
            ),
            Shape::Spacing { .. } => match &known.spacing {
                // SAFETY: game thread, per the contract; `local` is a whole spacing.
                Ok(spacing) => Value::Data(
                    spacing
                        .faces
                        .iter()
                        .map(|&face| unsafe { spacing.get(local, face) })
                        .collect(),
                ),
                // Not reached: without the functions there's no such shape.
                Err(_) => Value::Data(Vec::new()),
            },
        }
    }

    /// The value as it travels, from a local copy read earlier (see [`Self::read`]): `None` if
    /// it's an asset that no longer exists. Bricks may no longer exist either: their addresses
    /// aren't followed.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn earlier_value(&self, local: &[u8], known: &Known) -> Option<Value> {
        if let Shape::Asset { .. } = self.shape {
            let (address, index) = asset_of(local);
            if address != 0
                && weak_object::WeakObject::at(address as *mut UObjectBase, index)
                    .get_live()
                    .is_none()
            {
                return None;
            }
        }
        // SAFETY: an asset was just checked to be live; nothing else is followed.
        Some(unsafe { self.value(local, known) })
    }

    /// Gives `object` the value `value` (not telling it: see [`brick_property::post_modify`]),
    /// once every part of it is checked; otherwise why not, with nothing written.
    ///
    /// # Safety
    /// `object` must be a live object of a class having the property, and bricks in `value` live
    /// objects. Game thread only.
    unsafe fn write(
        &self,
        object: *mut UObjectBase,
        value: &Value,
        known: &Known,
    ) -> Result<(), String> {
        let base = object.cast::<u8>();
        match (&self.shape, value) {
            (Shape::Data { leaves, size }, Value::Data(bytes)) => {
                if bytes.len() != *size {
                    return Err(format!("{} bytes, not {size}", bytes.len()));
                }
                let mut at = 0;
                for leaf in leaves {
                    let part = &bytes[at..at + leaf.kind.size()];
                    if !leaf.kind.allows(part) {
                        return Err(format!("an impossible field value {part:02x?}"));
                    }
                    at += part.len();
                }
                let mut at = 0;
                for leaf in leaves {
                    let part = &bytes[at..at + leaf.kind.size()];
                    // SAFETY: per the contract, every leaf lies within the object; its value was
                    // checked to be one the field may hold.
                    unsafe {
                        match &leaf.kind {
                            LeafKind::Bool(field) => field.set(base, part[0] == 1),
                            _ => core::ptr::copy_nonoverlapping(
                                part.as_ptr(),
                                base.add(leaf.offset),
                                part.len(),
                            ),
                        }
                    }
                    at += part.len();
                }
                Ok(())
            }
            (
                Shape::Asset {
                    offset,
                    class,
                    meta,
                    nullable,
                },
                Value::Text(path),
            ) => {
                let asset = if path.is_empty() {
                    if !nullable {
                        return Err("none, which this brick can't have".into());
                    }
                    core::ptr::null_mut()
                } else {
                    // SAFETY: game thread, per the contract.
                    unsafe { find_asset(path, *class, *meta) }
                        .ok_or_else(|| format!("no asset {path} of its kind here"))?
                };
                // SAFETY: per the contract, the property lies within the object; the asset is
                // null (allowed) or a live object of the property's class.
                unsafe {
                    base.add(*offset)
                        .cast::<*mut UObjectBase>()
                        .write_unaligned(asset)
                };
                Ok(())
            }
            (Shape::Str { offset, property }, Value::Text(text)) => {
                let units = text.encode_utf16().count();
                if units > MAX_TEXT {
                    return Err(format!(
                        "a text of {units} characters, more than {MAX_TEXT}"
                    ));
                }
                if text.contains('\0') {
                    return Err("a text with a character 0".into());
                }
                let source = OwnedFString::new(text);
                // SAFETY: per the contract, the property holds a string there; the source is a
                // valid string the copy only reads.
                unsafe {
                    property_copy::copy_values(
                        *property,
                        base.add(*offset),
                        source.as_ptr().cast(),
                        1,
                    )
                };
                Ok(())
            }
            (
                Shape::Bricks {
                    offset,
                    property,
                    many,
                },
                Value::Bricks(bricks),
            ) => {
                let references = known.references.as_ref().map_err(Clone::clone)?;
                if !many && bricks.len() > 1 {
                    return Err(format!("{} bricks where one fits", bricks.len()));
                }
                if bricks.len() > MAX_BRICKS {
                    return Err(format!("{} bricks, more than {MAX_BRICKS}", bricks.len()));
                }
                let mut elements = Vec::with_capacity(bricks.len() * BRICK_POINTER_SIZE);
                for &brick in bricks {
                    // SAFETY: per the contract, the bricks are live.
                    elements.extend_from_slice(&unsafe { references.pointer_to(brick) }?);
                }
                // SAFETY: per the contract, the property holds its value there; every source below
                // is a valid value of its type (elements of the checked size, made as the game
                // makes them), which the copy only reads.
                unsafe {
                    let target = base.add(*offset);
                    if *many {
                        let source = TArray::<u8> {
                            data: elements.as_mut_ptr(),
                            num: bricks.len() as i32,
                            max: bricks.len() as i32,
                        };
                        property_copy::copy_values(
                            *property,
                            target,
                            (&raw const source).cast(),
                            1,
                        );
                    } else if let Some(element) = elements.get(..BRICK_POINTER_SIZE) {
                        property_copy::copy_values(*property, target, element.as_ptr(), 1);
                    } else {
                        // None, as the brick class' defaults hold it.
                        let class = reflection::class_of(object.cast()).cast::<UClass>();
                        let defaults = reflection::default_object(class)
                            .ok_or("no brick defaults to take none from")?;
                        let none = defaults.cast::<u8>().add(*offset);
                        property_copy::copy_values(*property, target, none, 1);
                    }
                }
                Ok(())
            }
            (Shape::Spacing { offset, property }, Value::Data(faces)) => {
                let spacing = known.spacing.as_ref().map_err(Clone::clone)?;
                if faces.len() != spacing.faces.len() {
                    return Err(format!(
                        "{} faces, not {}",
                        faces.len(),
                        spacing.faces.len()
                    ));
                }
                if let Some(bad) = faces.iter().find(|face| !spacing.values.contains(face)) {
                    return Err(format!("an impossible spacing {bad}"));
                }
                // SAFETY: per the contract, the property holds a spacing there; the new one is the
                // game's own functions' work on a copy of it.
                unsafe {
                    let target = base.add(*offset);
                    let mut value = core::slice::from_raw_parts(target, spacing.size).to_vec();
                    for (&face, &face_value) in spacing.faces.iter().zip(faces) {
                        spacing.set(&mut value, face, face_value);
                    }
                    property_copy::copy_values(*property, target, value.as_ptr(), 1);
                }
                Ok(())
            }
            _ => Err("a value of another kind".into()),
        }
    }
}

/// The UTF-16 units of the `FString` at `string`, without its terminating zero.
///
/// # Safety
/// `string` must point to a live `FString`.
unsafe fn string_units<'a>(string: *const u8) -> &'a [u16] {
    // SAFETY: per the contract; `data` holds `num` units, the last one the terminating zero.
    unsafe {
        let string = &*string.cast::<TArray<u16>>();
        match usize::try_from(string.num) {
            Ok(count @ 1..) if !string.data.is_null() => {
                core::slice::from_raw_parts(string.data, count - 1)
            }
            _ => &[],
        }
    }
}

/// The brick properties of one brick class that can be synced.
pub struct ClassValues {
    pub properties: Vec<PropertyValue>,
    /// Brick properties (or parts) left out, each as `Class.Property (why)`, by declaring class.
    pub skipped: Vec<String>,
}

impl ClassValues {
    /// Every syncable property's value `object` holds, as kept locally (each one's size, then it).
    ///
    /// # Safety
    /// `object` must be a live object of the class these are of. Game thread only.
    pub unsafe fn read(&self, object: *const UObjectBase) -> Vec<u8> {
        let mut out = Vec::new();
        let mut value = Vec::new();
        for property in &self.properties {
            value.clear();
            // SAFETY: per the contract.
            unsafe { property.read(object, &mut value) };
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(&value);
        }
        out
    }

    /// The properties whose values differ between two of [`Self::read`]'s copies, each with its
    /// value as it travels from `now` and, to undo the change, from `was` (`None` if that's an
    /// asset that no longer exists). Every property, none to undo, if `was` isn't comparable.
    ///
    /// # Safety
    /// `now` must have been read just now, by these. Game thread only.
    pub unsafe fn differences(
        &self,
        values: &BrickValues,
        was: &[u8],
        now: &[u8],
    ) -> Vec<(&PropertyValue, Value, Option<Value>)> {
        let count = self.properties.len();
        let was = split(was).filter(|was| was.len() == count);
        let Some(now) = split(now).filter(|now| now.len() == count) else {
            return Vec::new();
        };
        self.properties
            .iter()
            .zip(&now)
            .enumerate()
            .filter_map(|(index, (property, now))| {
                let was = match &was {
                    Some(was) if was[index] == *now => return None,
                    Some(was) => Some(was[index]),
                    None => None,
                };
                // SAFETY: per the contract.
                unsafe {
                    Some((
                        property,
                        property.value(now, &values.known),
                        was.and_then(|was| property.earlier_value(was, &values.known)),
                    ))
                }
            })
            .collect()
    }

    /// The bricks `object` refers to, per property (part) that refers to any, as they travel: what
    /// a vehicle archive of only some bricks may leave out.
    ///
    /// # Safety
    /// `object` must be a live object of the class these are of. Game thread only.
    pub unsafe fn references(
        &self,
        values: &BrickValues,
        object: *const UObjectBase,
    ) -> Vec<(&PropertyValue, Value)> {
        self.properties
            .iter()
            .filter(|property| matches!(property.shape, Shape::Bricks { .. }))
            .filter_map(|property| {
                let mut local = Vec::new();
                // SAFETY: per the contract; the value is taken from what was just read.
                let value = unsafe {
                    property.read(object, &mut local);
                    property.value(&local, &values.known)
                };
                matches!(&value, Value::Bricks(bricks) if !bricks.is_empty())
                    .then_some((property, value))
            })
            .collect()
    }

    /// The property (or part) called `name`.
    pub fn find(&self, name: &str) -> Option<&PropertyValue> {
        self.properties
            .iter()
            .find(|property| property.name == name)
    }
}

/// An asset's local copy (see [`PropertyValue::read`]): its address and its slot in the object
/// array.
fn asset_of(local: &[u8]) -> (usize, i32) {
    let (address, index) = local.split_at(size_of::<usize>());
    (
        usize::from_le_bytes(address.try_into().expect("an address")),
        i32::from_le_bytes(index.try_into().expect("an index")),
    )
}

/// The values of a [`ClassValues::read`] copy, or `None` if it isn't one.
fn split(mut local: &[u8]) -> Option<Vec<&[u8]>> {
    let mut values = Vec::new();
    while !local.is_empty() {
        let (size, rest) = local.split_first_chunk::<4>()?;
        let size = u32::from_le_bytes(*size) as usize;
        if rest.len() < size {
            return None;
        }
        let (value, rest) = rest.split_at(size);
        values.push(value);
        local = rest;
    }
    Some(values)
}

/// What every brick class can sync, for the log (see [`BrickValues::inventory`]).
pub struct Inventory {
    pub classes: usize,
    /// Property paths synced (`InputChannel.SourceBricks`).
    pub synced: BTreeSet<String>,
    /// `Class.Property (why)`, by declaring class.
    pub skipped: BTreeSet<String>,
}

/// The game code needed by values whose types have no reflected fields, found once.
struct Known {
    references: Result<References, String>,
    spacing: Result<Spacing, String>,
}

/// The syncable brick properties of every brick class met, found once per class.
pub struct BrickValues {
    /// `Brick`.
    brick: *const UStruct,
    classes: Mutex<HashMap<usize, Arc<ClassValues>>>,
    /// Where bricks keep their `IBrickPropertyInterface`, once found on one.
    interface: Mutex<Option<usize>>,
    known: Known,
}

// SAFETY: see `PropertyValue`.
unsafe impl Send for BrickValues {}
unsafe impl Sync for BrickValues {}

/// Finds what syncing brick properties needs. Calls game functions: after engine init, on the game
/// thread.
pub fn find() -> Result<BrickValues, String> {
    let brick = reflection::find_class("Brick").ok_or("class Brick not found")?;
    Ok(BrickValues {
        brick: brick.cast(),
        classes: Mutex::new(HashMap::new()),
        interface: Mutex::new(None),
        known: Known {
            references: References::find(),
            spacing: Spacing::find(),
        },
    })
}

impl BrickValues {
    /// `object`'s syncable brick properties, or `None` if it isn't a brick.
    ///
    /// # Safety
    /// `object` must be a live object. Game thread only.
    pub unsafe fn of(&self, object: *const UObjectBase) -> Option<Arc<ClassValues>> {
        // SAFETY: per the contract.
        unsafe {
            if !reflection::is_a(object, self.brick) {
                return None;
            }
            Some(self.of_class(reflection::class_of(object.cast())))
        }
    }

    /// The syncable brick properties of `class`, a brick class.
    ///
    /// # Safety
    /// `class` must be a live, linked brick class. Game thread only.
    unsafe fn of_class(&self, class: *const UStruct) -> Arc<ClassValues> {
        let mut classes = self.classes.lock().unwrap_or_else(PoisonError::into_inner);
        classes
            .entry(class as usize)
            // SAFETY: per the contract.
            .or_insert_with(|| Arc::new(unsafe { self.class_values(class) }))
            .clone()
    }

    /// What every C++ brick class can and can't sync (Blueprint ones add no brick properties: see
    /// [`brick_property::is_brick_property`]).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn inventory(&self) -> Inventory {
        let mut inventory = Inventory {
            classes: 0,
            synced: BTreeSet::new(),
            skipped: BTreeSet::new(),
        };
        // SAFETY: GObjects' slots hold live objects or null; an object is checked to be a C++ class
        // (linked at startup) before it's walked as one.
        unsafe {
            for object in GObjects().iter() {
                let class = object.cast::<UStruct>().cast_const();
                if object.is_null()
                    || !reflection::is_native_class(class)
                    || !reflection::with_parents(class).any(|parent| parent == self.brick)
                {
                    continue;
                }
                inventory.classes += 1;
                let values = self.of_class(class);
                for property in &values.properties {
                    inventory.synced.insert(property.name.clone());
                }
                inventory.skipped.extend(values.skipped.iter().cloned());
            }
        }
        inventory
    }

    /// Why bricks' references or connector spacings can't be synced, if they can't.
    pub fn problems(&self) -> Vec<&str> {
        [
            &self.known.references.as_ref().err(),
            &self.known.spacing.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        .map(|reason| reason.as_str())
        .collect()
    }

    /// Gives `object` the value `value` of its property (or part) `name` and tells it, as the
    /// panel does (in the caller's editor operation); otherwise why not, with nothing written.
    ///
    /// # Safety
    /// `object` must be a live brick of the editor, and bricks in `value` live objects of it. Game
    /// thread only.
    pub unsafe fn set(
        &self,
        object: *mut UObjectBase,
        name: &str,
        value: &Value,
    ) -> Result<(), String> {
        // SAFETY: per the contract.
        unsafe {
            let values = self.of(object).ok_or("not a brick")?;
            let property = values
                .find(name)
                .ok_or_else(|| format!("{name}: not a property synced here"))?;
            let interface = self
                .interface(object)
                .ok_or("bricks' property interface wasn't found")?;
            property
                .write(object, value, &self.known)
                .map_err(|reason| format!("{name}: {reason}"))?;
            brick_property::ChangedEvent::with_property(property.fname, false, |event| {
                brick_property::post_modify(object.cast::<u8>().add(interface).cast(), event);
            });
        }
        Ok(())
    }

    /// The float fields of `bytes`, a value of `object`'s property (or part) `name` as it
    /// travels, that differ from what `object` holds now.
    ///
    /// # Safety
    /// `object` must be a live brick. Game thread only.
    pub unsafe fn changed_floats(
        &self,
        object: *mut UObjectBase,
        name: &str,
        bytes: &[u8],
    ) -> Vec<f64> {
        // SAFETY: per the contract.
        let Some(values) = (unsafe { self.of(object) }) else {
            return Vec::new();
        };
        let Some(property) = values.find(name) else {
            return Vec::new();
        };
        let mut current = Vec::new();
        // SAFETY: per the contract; the class has the property (it's in its values).
        unsafe { property.read(object, &mut current) };
        property.changed_floats(bytes, &current)
    }

    /// Where `object`, a brick, keeps its property interface.
    ///
    /// # Safety
    /// `object` must be a live brick. Game thread only.
    unsafe fn interface(&self, object: *mut UObjectBase) -> Option<usize> {
        let mut known = self
            .interface
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if known.is_none() {
            // SAFETY: per the contract; any brick has it at the same place (it comes from
            // `UBrickEditorObject`).
            *known = unsafe { brick_property::interface_offset(self.brick, object) };
        }
        *known
    }

    /// The syncable brick properties of `class`, a brick class.
    ///
    /// # Safety
    /// `class` must be a live, linked brick class. Game thread only.
    unsafe fn class_values(&self, class: *const UStruct) -> ClassValues {
        // SAFETY: per the contract; the class' records live as long as it does.
        unsafe {
            let mut values = ClassValues {
                properties: Vec::new(),
                skipped: Vec::new(),
            };
            let defaults = reflection::default_object(class.cast());
            for property in reflection::properties(class) {
                if !brick_property::is_brick_property(property, self.brick) {
                    continue;
                }
                let fname = (*property).field.name_private;
                let name = reflection::name_text(&fname);
                let owner = reflection::declaring_struct(property);
                let owner = reflection::name_text(reflection::struct_name(owner));
                let mut walk = Walk {
                    known: &self.known,
                    defaults,
                    leaves: Vec::new(),
                    parts: Vec::new(),
                    skipped: Vec::new(),
                };
                walk.add(property, 0, 0, &name);
                let size: usize = walk.leaves.iter().map(|leaf| leaf.kind.size()).sum();
                if size > 0 {
                    let leaves = walk.leaves;
                    walk.parts
                        .insert(0, (name.clone(), Shape::Data { leaves, size }));
                }
                if walk.parts.is_empty() && walk.skipped.is_empty() {
                    walk.skipped.push(format!("{name} (no reflected fields)"));
                }
                for (path, shape) in walk.parts {
                    values.properties.push(PropertyValue {
                        name: path,
                        fname,
                        shape,
                    });
                }
                for skipped in walk.skipped {
                    values.skipped.push(format!("{owner}.{skipped}"));
                }
            }
            values
        }
    }
}

/// Splitting one brick property into what travels: its plain fields together, its other parts
/// each on its own, and what can't travel.
struct Walk<'a> {
    known: &'a Known,
    /// The brick class' default object.
    defaults: Option<*mut UObjectBase>,
    leaves: Vec<Leaf>,
    /// Each with its path.
    parts: Vec<(String, Shape)>,
    /// Each as `path (why)`.
    skipped: Vec<String>,
}

impl Walk<'_> {
    /// Adds `property` (every element of a fixed size array), called `path`, of a struct or object
    /// starting `base` bytes into the object.
    ///
    /// # Safety
    /// `property` must be a live property record.
    unsafe fn add(&mut self, property: *const FProperty, base: usize, depth: usize, path: &str) {
        // SAFETY: per the contract.
        if let Err(reason) = unsafe { self.try_add(property, base, depth, path) } {
            self.skipped.push(format!("{path} ({reason})"));
        }
    }

    /// See [`Self::add`]; why not, if it can't travel.
    ///
    /// # Safety
    /// As for [`Self::add`].
    unsafe fn try_add(
        &mut self,
        property: *const FProperty,
        base: usize,
        depth: usize,
        path: &str,
    ) -> Result<(), String> {
        // SAFETY: per the contract; type records follow the `FProperty` fields (module
        // documentation).
        unsafe {
            let count = (*property).array_dim;
            let size = (*property).element_size;
            let (Ok(count @ 1..=64), Ok(size @ 1..)) =
                (usize::try_from(count), usize::try_from(size))
            else {
                return Err("an unexpected size".into());
            };
            let start = base + (*property).offset_internal as usize;
            let single = |what: &str| {
                if count == 1 {
                    Ok(())
                } else {
                    Err(format!("an array of {what}"))
                }
            };
            let kind_of = |expected: usize, kind: LeafKind| {
                if size == expected {
                    Ok(kind)
                } else {
                    Err(format!("a {size}-byte value of a {expected}-byte type"))
                }
            };
            let part = if reflection::is_kind(property, "ObjectProperty")
                || reflection::is_kind(property, "ClassProperty")
            {
                single("objects")?;
                if size != 8 {
                    return Err("an unexpected object property".into());
                }
                let class = *property.add(1).cast::<*const UStruct>();
                let meta = if reflection::is_kind(property, "ClassProperty") {
                    Some(*property.add(1).cast::<*const UStruct>().add(1))
                } else {
                    None
                };
                if class.is_null() || meta.is_some_and(|meta| meta.is_null()) {
                    return Err("an object property of no class".into());
                }
                let nullable = self.defaults.is_none_or(|defaults| {
                    defaults
                        .cast::<u8>()
                        .add(start)
                        .cast::<*const u8>()
                        .read_unaligned()
                        .is_null()
                });
                Shape::Asset {
                    offset: start,
                    class,
                    meta,
                    nullable,
                }
            } else if reflection::is_kind(property, "StrProperty") {
                single("strings")?;
                Shape::Str {
                    offset: start,
                    property,
                }
            } else if reflection::is_kind(property, "ArrayProperty") {
                single("arrays")?;
                let inner = reflection::array_inner(property);
                if inner.is_null() {
                    return Err("an array of nothing".into());
                }
                let inner_struct = reflection::struct_type(inner);
                if inner_struct.is_null() || !self.known.is_brick_pointer(inner_struct) {
                    return Err(format!("arrays of {} not synced yet", kind_name(inner)));
                }
                self.known.references.as_ref().map_err(Clone::clone)?;
                Shape::Bricks {
                    offset: start,
                    property,
                    many: true,
                }
            } else if reflection::is_kind(property, "StructProperty") {
                let inner = reflection::struct_type(property);
                if inner.is_null() || depth >= MAX_DEPTH {
                    return Err("an unexpected struct".into());
                }
                if self.known.is_brick_pointer(inner) {
                    single("bricks")?;
                    self.known.references.as_ref().map_err(Clone::clone)?;
                    Shape::Bricks {
                        offset: start,
                        property,
                        many: false,
                    }
                } else if reflection::name_is(reflection::struct_name(inner), Spacing::STRUCT) {
                    single("connector spacings")?;
                    let spacing = self.known.spacing.as_ref().map_err(Clone::clone)?;
                    kind_of(spacing.size, LeafKind::Int(spacing.size))?;
                    Shape::Spacing {
                        offset: start,
                        property,
                    }
                } else {
                    return self.add_fields(inner, start, count, size, depth, path);
                }
            } else {
                let kind = if reflection::is_kind(property, "BoolProperty") {
                    single("bools")?;
                    let field = BoolField::of(property).ok_or("an unexpected bool")?;
                    LeafKind::Bool(field.shifted(base))
                } else if reflection::is_kind(property, "FloatProperty") {
                    kind_of(4, LeafKind::Float)?
                } else if reflection::is_kind(property, "DoubleProperty") {
                    kind_of(8, LeafKind::Double)?
                } else if ["IntProperty", "UInt32Property"]
                    .iter()
                    .any(|kind| reflection::is_kind(property, kind))
                {
                    kind_of(4, LeafKind::Int(4))?
                } else if ["Int16Property", "UInt16Property"]
                    .iter()
                    .any(|kind| reflection::is_kind(property, kind))
                {
                    kind_of(2, LeafKind::Int(2))?
                } else if ["Int64Property", "UInt64Property"]
                    .iter()
                    .any(|kind| reflection::is_kind(property, kind))
                {
                    kind_of(8, LeafKind::Int(8))?
                } else if reflection::is_kind(property, "Int8Property") {
                    kind_of(1, LeafKind::Int(1))?
                } else if reflection::is_kind(property, "ByteProperty") {
                    let uenum = enum_of(property);
                    if uenum.is_null() {
                        kind_of(1, LeafKind::Int(1))?
                    } else {
                        kind_of(1, enum_kind(uenum, 1)?)?
                    }
                } else if reflection::is_kind(property, "EnumProperty") {
                    let uenum = enum_of(property);
                    if uenum.is_null() || !matches!(size, 1 | 2 | 4 | 8) {
                        return Err("an unexpected enum".into());
                    }
                    enum_kind(uenum, size)?
                } else {
                    return Err(format!("{} not synced yet", kind_name(property)));
                };
                for index in 0..count {
                    self.leaves.push(Leaf {
                        offset: start + index * size,
                        kind: kind.clone(),
                    });
                }
                return Ok(());
            };
            self.parts.push((path.into(), part));
            Ok(())
        }
    }

    /// Adds the editable fields of `count` structs `inner` of `size` bytes, the first at `start`,
    /// called `path`.
    ///
    /// # Safety
    /// `inner` must be a live struct.
    unsafe fn add_fields(
        &mut self,
        inner: *const UStruct,
        start: usize,
        count: usize,
        size: usize,
        depth: usize,
        path: &str,
    ) -> Result<(), String> {
        // SAFETY: per the contract; its property records live as long as it.
        unsafe {
            let fields: Vec<_> = reflection::properties(inner).collect();
            if fields.is_empty() {
                return Err(format!(
                    "{} has no reflected fields",
                    reflection::name_text(reflection::struct_name(inner))
                ));
            }
            for index in 0..count {
                for &field in &fields {
                    if (*field).property_flags & EDIT == 0 {
                        continue;
                    }
                    let name = reflection::name_text(&(*field).field.name_private);
                    let path = if count == 1 {
                        format!("{path}.{name}")
                    } else {
                        format!("{path}[{index}].{name}")
                    };
                    self.add(field, start + index * size, depth + 1, &path);
                }
            }
        }
        Ok(())
    }
}

/// The name of `property`'s type, e.g. `MapProperty`.
///
/// # Safety
/// `property` must be a live property record.
unsafe fn kind_name(property: *const FProperty) -> String {
    // SAFETY: per the contract; a property's class record lives as long as the program.
    unsafe {
        let class = (*property).field.class_private;
        if class.is_null() {
            "an unknown type".into()
        } else {
            reflection::name_text(&(*class).name)
        }
    }
}

/// An enum field of `size` bytes holding entries of `uenum`, but its `_MAX`.
///
/// # Safety
/// `uenum` must be a live `UEnum`.
unsafe fn enum_kind(uenum: *const UObjectBase, size: usize) -> Result<LeafKind, String> {
    // SAFETY: per the contract.
    let values = unsafe { enum_values(uenum) }?;
    Ok(LeafKind::Enum {
        size,
        values: values.into(),
    })
}

/// The values of `uenum`'s entries, but its `_MAX`.
///
/// # Safety
/// `uenum` must be a live `UEnum`.
unsafe fn enum_values(uenum: *const UObjectBase) -> Result<Vec<i64>, String> {
    // SAFETY: per the contract; the entries are live names.
    unsafe {
        let names = &*uenum
            .cast::<u8>()
            .add(reflection::ENUM_NAMES)
            .cast::<TArray<EnumEntry>>();
        let count = usize::try_from(names.num)
            .ok()
            .filter(|&count| (1..=MAX_ENUM_ENTRIES).contains(&count) && !names.data.is_null())
            .ok_or("an enum with unexpected entries")?;
        Ok(core::slice::from_raw_parts(names.data, count)
            .iter()
            .filter(|entry| !reflection::name_text(&entry.name).ends_with("_MAX"))
            .map(|entry| entry.value)
            .collect())
    }
}

/// The enum an enum or byte property holds, or null.
///
/// # Safety
/// `property` must be a live property record.
unsafe fn enum_of(property: *const FProperty) -> *const UObjectBase {
    // SAFETY: per the contract; see the module documentation for the layouts.
    unsafe {
        if reflection::is_kind(property, "EnumProperty") {
            *property.add(1).cast::<*const UObjectBase>().add(1)
        } else if reflection::is_kind(property, "ByteProperty") {
            *property.add(1).cast::<*const UObjectBase>()
        } else {
            core::ptr::null()
        }
    }
}

/// Bytes of an `FBrickEditorObjectPtr` (checked against reflection in [`References::find`]).
const BRICK_POINTER_SIZE: usize = 12;
/// Where its `FBrickEditorObjectID` is, after the weak pointer.
const BRICK_ID_OFFSET: usize = 8;

/// Making `FBrickEditorObjectPtr`s.
struct References {
    /// `FBrickEditorObjectPtr`.
    pointer: *const UStruct,
    /// `UBrickEditorObject`: what one may point to.
    editor_object: *const UStruct,
    /// `const FBrickEditorObjectID& UBrickEditorObject::GetEditorObjectID()`, 2 bytes.
    id: Getter<u16>,
}

impl References {
    fn find() -> Result<Self, String> {
        let pointer = reflection::find_object("BrickEditorObjectPtr", "ScriptStruct")
            .ok_or("struct BrickEditorObjectPtr not found")?
            .cast::<UStruct>()
            .cast_const();
        // SAFETY: a live struct (found by its class' name).
        let size = unsafe { reflection::struct_size(pointer) };
        if size != BRICK_POINTER_SIZE {
            return Err(format!(
                "BrickEditorObjectPtr is {size} bytes, not {BRICK_POINTER_SIZE}"
            ));
        }
        let editor_object = reflection::find_class("BrickEditorObject")
            .ok_or("class BrickEditorObject not found")?;
        Ok(Self {
            pointer,
            editor_object: editor_object.cast(),
            id: Getter::find(editor_object, "GetEditorObjectID", "StructProperty")?,
        })
    }

    /// An `FBrickEditorObjectPtr` to `object`, as the game makes one; otherwise why not.
    ///
    /// # Safety
    /// `object` must be a live object. Game thread only.
    unsafe fn pointer_to(
        &self,
        object: *mut UObjectBase,
    ) -> Result<[u8; BRICK_POINTER_SIZE], String> {
        // SAFETY: per the contract.
        unsafe {
            if !reflection::is_a(object, self.editor_object) {
                return Err("a reference to something else than a brick".into());
            }
            let weak = weak_object::new_game_pointer(object)?;
            let id = self.id.call(object);
            if id == 0 {
                return Err("a brick without an editor id".into());
            }
            let mut element = [0u8; BRICK_POINTER_SIZE];
            element
                .as_mut_ptr()
                .cast::<FWeakObjectPtr>()
                .write_unaligned(weak);
            element[BRICK_ID_OFFSET..BRICK_ID_OFFSET + 2].copy_from_slice(&id.to_le_bytes());
            Ok(element)
        }
    }
}

impl Known {
    /// Whether `ustruct` is `FBrickEditorObjectPtr` (by name when it wasn't found usable).
    ///
    /// # Safety
    /// `ustruct` must be a live struct.
    unsafe fn is_brick_pointer(&self, ustruct: *const UStruct) -> bool {
        match &self.references {
            Ok(references) => ustruct == references.pointer,
            // SAFETY: per the contract.
            Err(_) => unsafe {
                reflection::name_is(reflection::struct_name(ustruct), "BrickEditorObjectPtr")
            },
        }
    }
}

/// `FScalableBrickConnectorSpacing` through the game's functions (module documentation).
struct Spacing {
    /// `UBrickStatics`' default object, to call its static functions on.
    statics: *mut UObjectBase,
    /// `EConnectorSpacing GetScalableBrickConnectorSpacingAxis(const
    /// FScalableBrickConnectorSpacing&, EFluAxisSigned)` and where its parameters go.
    get: *const UFunction,
    get_spacing: usize,
    get_axis: usize,
    get_result: usize,
    /// `void SetScalableBrickConnectorSpacingAxis(FScalableBrickConnectorSpacing&, EFluAxisSigned,
    /// EConnectorSpacing)` and where its parameters go.
    set: *const UFunction,
    set_spacing: usize,
    set_axis: usize,
    set_value: usize,
    /// The struct's bytes.
    size: usize,
    /// The `EFluAxisSigned`s that are faces: setting them changes a spacing.
    faces: Vec<u8>,
    /// The `EConnectorSpacing`s a face holds: set, it reads back the same.
    values: Vec<u8>,
}

impl Spacing {
    const STRUCT: &str = "ScalableBrickConnectorSpacing";

    /// Finds the functions and checks them by calling them. Game thread only.
    fn find() -> Result<Self, String> {
        let class = reflection::find_class("BrickStatics").ok_or("class BrickStatics not found")?;
        // SAFETY: a live class; its functions' records live as long as it. Each call gets
        // parameters at their reflected offsets (checked to fit), on the class' default object.
        unsafe {
            let statics =
                reflection::default_object(class).ok_or("BrickStatics has no default object")?;
            let get = reflection::find_function(class, "GetScalableBrickConnectorSpacingAxis")?;
            let set = reflection::find_function(class, "SetScalableBrickConnectorSpacingAxis")?;
            let (get_spacing, size) = struct_parameter(get)?;
            let (set_spacing, set_size) = struct_parameter(set)?;
            if size != set_size || size == 0 || size > 16 {
                return Err("unexpected connector spacing parameters".into());
            }
            let (get_axis, axes) = enum_parameter(get, "EFluAxisSigned", false)?;
            let (get_result, values) = enum_parameter(get, "EConnectorSpacing", true)?;
            let (set_axis, _) = enum_parameter(set, "EFluAxisSigned", false)?;
            let (set_value, _) = enum_parameter(set, "EConnectorSpacing", false)?;
            let mut found = Self {
                statics,
                get,
                get_spacing,
                get_axis,
                get_result,
                set,
                set_spacing,
                set_axis,
                set_value,
                size,
                faces: Vec::new(),
                values: Vec::new(),
            };
            let bytes = |values: Vec<i64>| -> Vec<u8> {
                values
                    .into_iter()
                    .filter_map(|value| u8::try_from(value).ok())
                    .collect()
            };
            let (axes, values) = (bytes(axes), bytes(values));
            let zero = vec![0u8; size];
            let probe = *values
                .iter()
                .find(|&&value| value != 0)
                .ok_or("EConnectorSpacing has no spacing")?;
            for &axis in &axes {
                let mut changed = zero.clone();
                found.set(&mut changed, axis, probe);
                if changed != zero {
                    found.faces.push(axis);
                }
            }
            let first = *found.faces.first().ok_or("no axis is a face")?;
            for &value in &values {
                let mut spacing = zero.clone();
                found.set(&mut spacing, first, value);
                if found.get(&spacing, first) == value {
                    found.values.push(value);
                }
            }
            // Every face its own: all set, each reads back.
            let mut all = zero;
            for &face in &found.faces {
                found.set(&mut all, face, probe);
            }
            if found
                .faces
                .iter()
                .any(|&face| found.get(&all, face) != probe)
            {
                return Err("connector spacing faces overlap".into());
            }
            Ok(found)
        }
    }

    /// The spacing of `face` in `spacing` (a whole struct).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn get(&self, spacing: &[u8], face: u8) -> u8 {
        let mut parameters = Parameters::new();
        // SAFETY: the parameters fit (checked in `find`), at their reflected offsets.
        unsafe {
            let base = parameters.as_mut_ptr();
            let size = self.size.min(spacing.len());
            core::ptr::copy_nonoverlapping(spacing.as_ptr(), base.add(self.get_spacing), size);
            base.add(self.get_axis).write(face);
            reflection::call(self.statics, self.get, base);
            base.add(self.get_result).read()
        }
    }

    /// Gives `face` of `spacing` (a whole struct) the spacing `value`.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn set(&self, spacing: &mut [u8], face: u8, value: u8) {
        let mut parameters = Parameters::new();
        // SAFETY: as for `get`; the struct is the function's out parameter.
        unsafe {
            let base = parameters.as_mut_ptr();
            let size = self.size.min(spacing.len());
            core::ptr::copy_nonoverlapping(spacing.as_ptr(), base.add(self.set_spacing), size);
            base.add(self.set_axis).write(face);
            base.add(self.set_value).write(value);
            reflection::call(self.statics, self.set, base);
            core::ptr::copy_nonoverlapping(base.add(self.set_spacing), spacing.as_mut_ptr(), size);
        }
    }
}

/// `function`'s one `FScalableBrickConnectorSpacing` parameter: its offset and size.
///
/// # Safety
/// `function` must be a live function record.
unsafe fn struct_parameter(function: *const UFunction) -> Result<(usize, usize), String> {
    // SAFETY: per the contract.
    unsafe {
        if !Parameters::fit(function) {
            return Err("connector spacing functions with too many parameters".into());
        }
        let mut found = reflection::properties(function.cast()).filter(|&parameter| {
            let inner = reflection::struct_type(parameter);
            !inner.is_null() && reflection::name_is(reflection::struct_name(inner), Spacing::STRUCT)
        });
        match (found.next(), found.next()) {
            (Some(parameter), None) => Ok((
                (*parameter).offset_internal as usize,
                (*parameter).element_size as usize,
            )),
            _ => Err("connector spacing functions without one spacing parameter".into()),
        }
    }
}

/// `function`'s one 1-byte parameter (the return value if `result`) holding the enum `name`: its
/// offset and the enum's values.
///
/// # Safety
/// `function` must be a live function record.
unsafe fn enum_parameter(
    function: *const UFunction,
    name: &str,
    result: bool,
) -> Result<(usize, Vec<i64>), String> {
    // SAFETY: per the contract.
    unsafe {
        let mut found = reflection::properties(function.cast()).filter(|&parameter| {
            let uenum = enum_of(parameter);
            !uenum.is_null()
                && reflection::name_is(&(*uenum).name_private, name)
                && ((*parameter).property_flags & reflection::RETURN_PARAMETER != 0) == result
        });
        match (found.next(), found.next()) {
            (Some(parameter), None) if (*parameter).element_size == 1 => Ok((
                (*parameter).offset_internal as usize,
                enum_values(enum_of(parameter))?,
            )),
            _ => Err(format!("connector spacing functions without one {name}")),
        }
    }
}

/// `name` with its number, as Unreal prints it (`Steel_2` is `Steel` numbered 3).
///
/// # Safety
/// `name` must be a valid name (of the live name pool).
unsafe fn full_name(name: &FName) -> String {
    // SAFETY: per the contract.
    let text = unsafe { reflection::name_text(name) };
    match name.number {
        0 => text,
        number => format!("{text}_{}", number - 1),
    }
}

/// `object`'s path name: its outers' names and its own, outermost first, joined by dots (e.g.
/// `/Game/Bricks/Materials/Steel.Steel`).
///
/// # Safety
/// `object` must be a live object. Game thread only.
unsafe fn path_of(object: *const UObjectBase) -> String {
    let mut names = Vec::new();
    let mut at = object;
    // SAFETY: per the contract; a live object's outers are live.
    unsafe {
        while !at.is_null() && names.len() < 32 {
            names.push(full_name(&(*at).name_private));
            at = (*at).outer_private;
        }
    }
    names.reverse();
    names.join(".")
}

/// The live object with path name `path` (see [`path_of`]) that is of `class`, or, for a class
/// property (`meta`), is a class that is `meta` or one of its children. Never a class' default
/// object or one being destroyed. Loads nothing.
///
/// # Safety
/// `class` and `meta` must be live classes. Game thread only.
unsafe fn find_asset(
    path: &str,
    class: *const UStruct,
    meta: Option<*const UStruct>,
) -> Option<*mut UObjectBase> {
    let leaf = path.rsplit('.').next()?;
    let excluded = EObjectFlags::RF_ClassDefaultObject | EObjectFlags::RF_BeginDestroyed;
    // SAFETY: per the contract; GObjects' slots hold live objects or null, and every object has a
    // class.
    unsafe {
        GObjects().iter().find(|&object| {
            !object.is_null()
                && !(*object).object_flags.intersects(excluded)
                && reflection::is_a(object, class)
                && full_name(&(*object).name_private) == leaf
                && meta.is_none_or(|meta| {
                    reflection::with_parents(object.cast::<UStruct>()).any(|parent| parent == meta)
                })
                && path_of(object) == path
        })
    }
}
