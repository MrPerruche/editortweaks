//! The static info of each kind of brick (`UBrickEditorObjectStaticInfo`), where the editor gets a
//! brick's name. See `docs/re/brick-names.md`.
//!
//! A brick's `StaticInfoClass` is a Blueprint class (`BP_ScalableWedge_C`) whose default object
//! holds what is the same for every brick of the kind. The name comes from there: the editor's
//! `GetBrickEditorObjectDisplayName` calls the static info's virtual `GetDisplayName`, which for
//! bricks is the reflected `ObjectTypeDisplayName`, followed by the size if
//! `bAppendBrickSizeToDisplayName` is set. Scalable and light bricks don't set their own, so they
//! all show the name of their C++ class ("Scalable brick").

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::UStruct;

use crate::game::kismet::Texts;
use crate::game::reflection::{self, BoolField};
use crate::game::text::Text;

/// The static info classes' fields that name bricks (after engine init).
pub struct StaticInfos {
    /// `BrickEditorObjectStaticInfo`.
    class: *const UStruct,
    /// Offset of its `ObjectTypeDisplayName`.
    type_name: usize,
    /// How bricks show their size in their name, if that was found.
    size_in_name: Option<SizeInName>,
}

/// `BrickStaticInfo`'s fields for the size shown after a brick's name.
struct SizeInName {
    /// `bAppendBrickSizeToDisplayName`.
    appended: BoolField,
    /// Offset of the `FVector` size.
    size: usize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for StaticInfos {}
unsafe impl Sync for StaticInfos {}

/// One kind of brick: the default object of its static info class.
pub struct Kind {
    object: *mut UObjectBase,
    /// The Blueprint's name without its `_C`, e.g. `BP_ScalableWedge`.
    pub class: String,
}

pub fn find() -> Result<StaticInfos, String> {
    let class = reflection::find_class("BrickEditorObjectStaticInfo")
        .ok_or("class BrickEditorObjectStaticInfo not found")?;
    let type_name = reflection::field_offset(
        class,
        "ObjectTypeDisplayName",
        "TextProperty",
        size_of::<Text>() as i32,
    )?;
    // Only needed to tell bricks apart that show their size: a missing part is no reason to stop.
    let size_in_name = reflection::find_class("BrickStaticInfo").and_then(|bricks| {
        let bricks = bricks.cast::<UStruct>();
        Some(SizeInName {
            appended: reflection::bool_field(bricks, "bAppendBrickSizeToDisplayName").ok()?,
            size: reflection::struct_field_offset(bricks, "BrickSize", "StructProperty", 12)
                .ok()?,
        })
    });
    Ok(StaticInfos {
        class: class.cast(),
        type_name,
        size_in_name,
    })
}

impl StaticInfos {
    /// The kinds of brick (and other editor objects) made as Blueprints that are loaded. Walks
    /// every live object. Game thread only.
    pub fn blueprints(&self) -> Vec<Kind> {
        // SAFETY: on the game thread, GObjects slots hold live objects or null, and every object
        // has a valid class.
        unsafe {
            GObjects()
                .iter()
                .filter(|&object| {
                    !object.is_null()
                        && (*object)
                            .object_flags
                            .contains(EObjectFlags::RF_ClassDefaultObject)
                })
                .filter_map(|object| {
                    let class = reflection::class_of(object.cast());
                    if !reflection::with_parents(class).any(|parent| parent == self.class) {
                        return None;
                    }
                    let name = reflection::name_text(reflection::struct_name(class));
                    let class = name.strip_suffix("_C")?.to_owned();
                    Some(Kind { object, class })
                })
                .collect()
        }
    }

    /// What `kind` is called, without the size.
    ///
    /// # Safety
    /// `kind` must come from [`Self::blueprints`] and still be live. Game thread only.
    pub unsafe fn name(&self, texts: &Texts, kind: &Kind) -> String {
        // SAFETY: per the contract; `type_name` is the place of its FText `ObjectTypeDisplayName`.
        unsafe {
            let field = kind.object.byte_add(self.type_name).cast::<Text>();
            texts.string(Text::copy_of(field))
        }
    }

    /// Makes `kind` called `name` (the size, if it shows one, still follows).
    ///
    /// # Safety
    /// As [`Self::name`].
    pub unsafe fn set_name(&self, texts: &Texts, kind: &Kind, name: &str) {
        // SAFETY: per the contract; the field takes its own reference to our new text.
        unsafe {
            let field = kind.object.byte_add(self.type_name).cast::<Text>();
            let text = texts.text(name);
            text.assign_to(field);
            text.release();
        }
    }

    /// The size `kind`'s name ends with, if it does.
    ///
    /// # Safety
    /// As [`Self::name`].
    pub unsafe fn size_in_name(&self, kind: &Kind) -> Option<[f32; 3]> {
        let shown = self.size_in_name.as_ref()?;
        // SAFETY: per the contract; `size` is the place of an FVector.
        unsafe {
            shown.appended.get(kind.object.cast()).then(|| {
                kind.object
                    .byte_add(shown.size)
                    .cast::<[f32; 3]>()
                    .read_unaligned()
            })
        }
    }
}
