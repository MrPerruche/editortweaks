//! Text bricks' text as their text render components draw it, and the offline fonts they use. All
//! reflected (UE 4.27 `UTextRenderComponent`, `UFont`, `FFontCharacter`), except a font's
//! character remap; found in BRMK 1.11.6, see `docs/re/model-export.md` ("Text bricks").
//!
//! `UTextBrick::UpdateText` keeps a `UBrickEditorTextRenderComponent` (a `UTextRenderComponent`)
//! attached to the brick's root component, on top of the brick, its text the brick's `Text`
//! string, its size (`WorldSize`) the brick's `FontSize` shrunk to fit the brick's top, its font
//! the brick font's offline `UFont`.
//!
//! A remapped font (`IsRemapped`) finds its glyphs through `TMap<uint16, uint16> CharRemap`, the
//! first field after the reflected `CompositeFont` (not reflected itself). It's read as UE 4.27's
//! `TSet` lays it out, checked against the font's glyphs.

use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use model_io::export::text::{Font, Glyph, Horizontal, Settings, Vertical};

use crate::game::editor_object::{Transform, Vector};
use crate::game::reflection::{self, Getter};
use crate::game::slider_text::FString;
use crate::platform::memory;

/// A text render component's text, as drawn.
pub struct Text {
    pub text: String,
    /// Its `UFont`, null without one.
    pub font: *mut UObjectBase,
    /// Its settings at the brick's `FontSize`, before `UpdateText` shrinks it to fit.
    pub settings: Settings,
    /// The brick's `FontSize`, and the component's `WorldSize`: the size it was given.
    pub font_size: f32,
    pub world_size: f32,
    /// The text's size at `world_size` as the engine measures it (`GetTextLocalSize`: X across
    /// the quads, Y along the lines, Z across them), `None` if that function wasn't found.
    pub measured: Option<Vector>,
    /// `TextRenderColor`'s red, green, blue (linear, the brick's colour made linear) and alpha (the
    /// brick's outline thickness).
    pub color: [u8; 4],
    /// The component's place in the world.
    pub place: Transform,
    /// How many text render components the brick's root component has (the one read is the
    /// nearest to where it was asked for).
    pub components: usize,
}

/// Where a `UFont`'s fields are.
struct FontFields {
    characters: usize,
    textures: usize,
    is_remapped: usize,
    kerning: usize,
    /// Where `CharRemap` is expected: right after `CompositeFont`.
    remap: usize,
    /// `FFontCharacter`'s size and fields.
    glyph_size: usize,
    start_u: usize,
    start_v: usize,
    u_size: usize,
    v_size: usize,
    texture_index: usize,
    vertical_offset: usize,
}

/// Reads text bricks' text (after engine init).
pub struct TextRenders {
    /// `TextBrick`, its `Text` (`FString`) and `FontSize` (`float`).
    brick_class: *const UStruct,
    brick_text: usize,
    font_size: usize,
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`.
    root: Getter<*mut UObjectBase>,
    /// `USceneComponent::AttachChildren` (`TArray<USceneComponent*>`).
    children: usize,
    /// `FTransform USceneComponent::K2_GetComponentToWorld() const`.
    to_world: Getter<Transform>,
    /// `FVector UTextRenderComponent::GetTextLocalSize() const`: what `UTextBrick::UpdateText`
    /// fits to the brick (it includes the component's `BoundsScale`, unlike our own layout).
    local_size: Option<Getter<Vector>>,
    /// `TextRenderComponent` and its fields.
    component_class: *const UStruct,
    font: usize,
    horizontal: usize,
    vertical: usize,
    color: usize,
    x_scale: usize,
    y_scale: usize,
    world_size: usize,
    inv_default_size: usize,
    horizontal_spacing: usize,
    vertical_spacing: usize,
    /// `EHorizTextAligment`'s and `EVerticalTextAligment`'s values, in our enums' order.
    horizontal_values: [u8; 3],
    vertical_values: [u8; 4],
    fonts: FontFields,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for TextRenders {}
unsafe impl Sync for TextRenders {}

fn class(name: &'static str) -> Result<*const UClass, String> {
    reflection::find_class(name).ok_or(format!("class {name} not found"))
}

/// The values of `entries` of the enum `name` (by their full names, `EVRTA_TextTop`), each a byte;
/// their index if the enum isn't found.
fn enum_values<const N: usize>(name: &'static str, entries: [&str; N]) -> [u8; N] {
    core::array::from_fn(|index| {
        reflection::enum_value(name, entries[index])
            .ok()
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(index as u8)
    })
}

impl FontFields {
    fn find() -> Result<Self, String> {
        let font = class("Font")?;
        let characters = reflection::field(font, "Characters", "ArrayProperty", 16)?;
        // SAFETY: live property records: the array's element is a struct property, and
        // `CompositeFont` a struct property of the font.
        let (glyph, remap) = unsafe {
            let glyph = reflection::struct_type(reflection::array_inner(characters));
            if glyph.is_null() {
                return Err("Font::Characters: not an array of structs".into());
            }
            let composite = reflection::find_property(font.cast(), "CompositeFont");
            if composite.is_null() || !reflection::is_kind(composite, "StructProperty") {
                return Err("field 'CompositeFont' not found".into());
            }
            let end = (*composite).offset_internal as usize + (*composite).element_size as usize;
            (glyph, end.next_multiple_of(8))
        };
        let int = |name| reflection::struct_field_offset(glyph, name, "IntProperty", 4);
        Ok(Self {
            // SAFETY: a live property record.
            characters: unsafe { (*characters).offset_internal as usize },
            textures: reflection::field_offset(font, "Textures", "ArrayProperty", 16)?,
            is_remapped: reflection::field_offset(font, "IsRemapped", "IntProperty", 4)?,
            kerning: reflection::field_offset(font, "Kerning", "IntProperty", 4)?,
            remap,
            // SAFETY: a live struct record.
            glyph_size: unsafe { reflection::struct_size(glyph) },
            start_u: int("StartU")?,
            start_v: int("StartV")?,
            u_size: int("USize")?,
            v_size: int("VSize")?,
            texture_index: reflection::struct_field_offset(
                glyph,
                "TextureIndex",
                "ByteProperty",
                1,
            )?,
            vertical_offset: int("VerticalOffset")?,
        })
    }
}

impl TextRenders {
    pub fn find() -> Result<Self, String> {
        let brick = class("TextBrick")?;
        let scene = class("SceneComponent")?;
        let component = class("TextRenderComponent")?;
        let float = |name| reflection::field_offset(component, name, "FloatProperty", 4);
        let byte = |name| reflection::field_offset(component, name, "ByteProperty", 1);
        Ok(Self {
            brick_class: brick.cast(),
            brick_text: reflection::field_offset(
                brick,
                "Text",
                "StrProperty",
                size_of::<FString>() as i32,
            )?,
            font_size: reflection::field_offset(brick, "FontSize", "FloatProperty", 4)?,
            root: Getter::find(
                class("BrickEditorObject")?,
                "GetRootComponent",
                "ObjectProperty",
            )?,
            children: reflection::field_offset(scene, "AttachChildren", "ArrayProperty", 16)?,
            to_world: Getter::find(scene, "K2_GetComponentToWorld", "StructProperty")?,
            local_size: Getter::find(component, "GetTextLocalSize", "StructProperty").ok(),
            component_class: component.cast(),
            font: reflection::field_offset(component, "Font", "ObjectProperty", 8)?,
            horizontal: byte("HorizontalAlignment")?,
            vertical: byte("VerticalAlignment")?,
            color: reflection::field_offset(component, "TextRenderColor", "StructProperty", 4)?,
            x_scale: float("XScale")?,
            y_scale: float("YScale")?,
            world_size: float("WorldSize")?,
            inv_default_size: float("InvDefaultSize")?,
            horizontal_spacing: float("HorizSpacingAdjust")?,
            vertical_spacing: float("VertSpacingAdjust")?,
            horizontal_values: enum_values(
                "EHorizTextAligment",
                ["EHTA_Left", "EHTA_Center", "EHTA_Right"],
            ),
            vertical_values: enum_values(
                "EVerticalTextAligment",
                [
                    "EVRTA_TextTop",
                    "EVRTA_TextCenter",
                    "EVRTA_TextBottom",
                    "EVRTA_QuadTop",
                ],
            ),
            fonts: FontFields::find()?,
        })
    }

    /// `brick`'s text, if it's a text brick showing some: read from its root component's text
    /// render component nearest to `near` (a place in the world), if it has several.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn of(&self, brick: *mut UObjectBase, near: [f32; 3]) -> Option<Text> {
        // SAFETY: per the contract; the offsets come from reflection, the root component and its
        // children are live components (or null).
        unsafe {
            if !reflection::is_a(brick, self.brick_class) {
                return None;
            }
            let text = brick
                .cast::<u8>()
                .add(self.brick_text)
                .cast::<FString>()
                .read()
                .to_string();
            if text.trim().is_empty() {
                return None;
            }
            let root = self.root.call(brick);
            if root.is_null() {
                return None;
            }
            let children = pointers(root.cast::<u8>().add(self.children));
            let components: Vec<(*mut UObjectBase, Transform)> = children
                .into_iter()
                .filter(|&child| !child.is_null() && reflection::is_a(child, self.component_class))
                .map(|child| (child, self.to_world.call(child)))
                .collect();
            let distance = |place: &Transform| {
                let [x, y, z, _] = place.translation;
                [x - near[0], y - near[1], z - near[2]]
                    .iter()
                    .map(|axis| axis * axis)
                    .sum::<f32>()
            };
            let &(component, place) = components
                .iter()
                .min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))?;
            let base = component.cast::<u8>();
            let float = |offset: usize| base.add(offset).cast::<f32>().read();
            let font_size = brick.cast::<u8>().add(self.font_size).cast::<f32>().read();
            let size = font_size * float(self.inv_default_size);
            let [blue, green, red, alpha] = base.add(self.color).cast::<[u8; 4]>().read();
            let horizontal = base.add(self.horizontal).read();
            let vertical = base.add(self.vertical).read();
            let inverse = float(self.inv_default_size);
            Some(Text {
                text,
                font: base.add(self.font).cast::<*mut UObjectBase>().read(),
                settings: Settings {
                    scale: [size * float(self.x_scale), size * float(self.y_scale)],
                    spacing: [float(self.horizontal_spacing), float(self.vertical_spacing)],
                    horizontal: match self.horizontal_values.iter().position(|&v| v == horizontal) {
                        Some(1) => Horizontal::Center,
                        Some(2) => Horizontal::Right,
                        _ => Horizontal::Left,
                    },
                    vertical: match self.vertical_values.iter().position(|&v| v == vertical) {
                        Some(1) => Vertical::TextCenter,
                        Some(2) => Vertical::TextBottom,
                        Some(3) => Vertical::QuadTop,
                        _ => Vertical::TextTop,
                    },
                    empty_line: if inverse > 0.0 { 1.0 / inverse } else { 0.0 },
                },
                font_size,
                world_size: float(self.world_size),
                measured: self.local_size.as_ref().map(|size| size.call(component)),
                color: [red, green, blue, alpha],
                place,
                components: components.len(),
            })
        }
    }

    /// `font`'s glyphs and its pages (textures, null for missing ones). Page sizes are left
    /// `None`, for the caller to fill.
    ///
    /// # Safety
    /// `font` must be a live `UFont`. Game thread only.
    pub unsafe fn font(
        &self,
        font: *mut UObjectBase,
    ) -> Result<(Font, Vec<*mut UObjectBase>), String> {
        let fields = &self.fonts;
        // SAFETY: per the contract; the offsets come from reflection, the arrays are read within
        // their counts.
        unsafe {
            let base = font.cast::<u8>();
            let characters = base.add(fields.characters).cast::<TArray<u8>>().read();
            let count = usize::try_from(characters.num).unwrap_or(0);
            if count == 0 || characters.data.is_null() {
                return Err("the font has no characters (not an offline font)".into());
            }
            let glyphs: Vec<Glyph> = (0..count)
                .map(|index| {
                    let glyph = characters.data.add(index * fields.glyph_size);
                    let int = |offset: usize| glyph.add(offset).cast::<i32>().read();
                    Glyph {
                        start: [int(fields.start_u), int(fields.start_v)],
                        size: [int(fields.u_size), int(fields.v_size)],
                        page: usize::from(glyph.add(fields.texture_index).read()),
                        vertical_offset: int(fields.vertical_offset),
                    }
                })
                .collect();
            let textures = pointers(base.add(fields.textures));
            let remap = if base.add(fields.is_remapped).cast::<i32>().read() != 0 {
                Some(char_remap(base.add(fields.remap), glyphs.len())?)
            } else {
                None
            };
            Ok((
                Font {
                    pages: vec![None; textures.len()],
                    glyphs,
                    remap,
                    kerning: base.add(fields.kerning).cast::<i32>().read(),
                },
                textures,
            ))
        }
    }
}

/// The objects of the `TArray<UObject*>` at `array` (none if it doesn't look like one).
///
/// # Safety
/// `array` must point to a live `TArray` of object pointers.
unsafe fn pointers(array: *const u8) -> Vec<*mut UObjectBase> {
    // SAFETY: per the contract; read within its count.
    unsafe {
        let array = array.cast::<TArray<*mut UObjectBase>>().read();
        if array.data.is_null() || !(0..=0x10000).contains(&array.num) {
            return Vec::new();
        }
        core::slice::from_raw_parts(array.data, array.num as usize).to_vec()
    }
}

/// Reads a `TMap<uint16, uint16>` at `map`, checking that it maps into `glyphs` glyphs.
///
/// UE 4.27's layout: a `TSet` starts with its `TSparseArray` of elements: their `TArray` (12-byte
/// `TSetElement`s: the pair, then two `int32`s of hashing; free ones hold free-list links), then
/// its `TBitArray` of allocated elements (four inline `uint32`s, a pointer to heap words if more
/// are needed, `NumBits`, `MaxBits`).
///
/// # Safety
/// `map` must point to readable memory at least as long as a `TMap` (0x50 bytes).
unsafe fn char_remap(map: *const u8, glyphs: usize) -> Result<HashMap<u16, u16>, String> {
    const ELEMENT: usize = 12;
    const MAX_ELEMENTS: i32 = 0x10000;
    let unexpected = |what: &str| format!("the font's character remap doesn't read: {what}");
    // SAFETY: per the contract; the elements and bit words are read within the counts, after
    // checking them.
    unsafe {
        let elements = map.cast::<TArray<u8>>().read();
        let bits = map.add(0x10);
        let heap = bits.add(0x10).cast::<*const u32>().read();
        let bit_count = bits.add(0x18).cast::<i32>().read();
        if !(0..=MAX_ELEMENTS).contains(&elements.num) || elements.num > elements.max {
            return Err(unexpected("element count"));
        }
        if bit_count != elements.num {
            return Err(unexpected("allocation flags"));
        }
        // Inferred, so checked before following them.
        if !memory::is_readable(elements.data, elements.num as usize * ELEMENT) {
            return Err(unexpected("its elements aren't readable"));
        }
        let words = if heap.is_null() {
            if bit_count > 128 {
                return Err(unexpected("allocation flags"));
            }
            bits.cast::<u32>()
        } else {
            if !memory::is_readable(heap.cast(), (bit_count as usize).div_ceil(32) * 4) {
                return Err(unexpected("its allocation flags aren't readable"));
            }
            heap
        };
        let mut remap = HashMap::new();
        for index in 0..elements.num as usize {
            if words.add(index / 32).read() & (1 << (index % 32)) == 0 {
                continue;
            }
            let [key, value] = elements.data.add(index * ELEMENT).cast::<[u16; 2]>().read();
            if usize::from(value) >= glyphs {
                return Err(unexpected("a glyph past the font's"));
            }
            remap.insert(key, value);
        }
        if remap.is_empty() {
            return Err(unexpected("empty"));
        }
        Ok(remap)
    }
}
