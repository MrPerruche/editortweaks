//! The brick properties the vehicle editor gives new bricks: the values edited last. After each
//! final edit of a property declared by `UBrick` (its color, material, pattern, ...), the editor
//! keeps the edited brick's value of it, and gives it to every brick created afterwards that has
//! that property. Found through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/editor-operations.md`.
//!
//! `virtual void AVehicleEditor::OnAnyPropertyModified(const FBrickPropertyChangedEvent&)` (BRMK
//! 0xC0A10, game RVA 0xCEE860) keeps it:
//!
//! ```text
//! if (Event.Type == Interactive || Event.Objects.Num() == 0) return;
//! Brick = Cast<UBrick>(Event.Objects.Last().Get());
//! Name = Event.Path.Num() ? Event.Path.Last() : NAME_None;
//! if (!Brick || !UBrick::StaticClass()->FindPropertyByName(Name)) return;
//! Brick->ReflectBrickProperties(Reflection);           // its properties as the panel lists them
//! if (Property = Reflection.Find(Name.ToString())) {
//!     Saved = &this->NewBrickProperties.FindOrAdd(Name);   // a TMap<FName, TArray<uint8>>
//!     Saved->Reset();
//!     Property->Serialize(FMemoryWriter(*Saved), Brick);   // the brick's value
//! }
//! ```
//!
//! Nothing else: no undo step, no unsaved changes. `AVehicleEditor::OnBrickEditorObjectsCreated`
//! (BRMK 0xC18D0) reads the map back into each new brick's properties of the same names.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::{self, FunctionSig};
use crate::game::brick_property::ChangedEvent;
use crate::game::{reflection, weak_object};

/// `AVehicleEditor::OnAnyPropertyModified`, by its start: the interactive check (`Type` at +0x48)
/// while saving its arguments, then the check for edited objects (their count at +0x38).
pub const ON_ANY_PROPERTY_MODIFIED: FunctionSig = FunctionSig {
    name: "AVehicleEditor::OnAnyPropertyModified",
    pattern: "80 7a 48 02 48 8b da 4c 8b f9 0f 84 ?? ?? ?? ?? 8b 42 38 85 c0 0f 84",
};

type OnAnyPropertyModifiedFn =
    unsafe extern "C" fn(editor: *mut UObjectBase, event: *const ChangedEvent);

pub struct NewBrickProperties {
    on_any_property_modified: OnAnyPropertyModifiedFn,
    /// `VehicleEditor`, the class the function belongs to.
    vehicle_editor: *const UStruct,
    /// `Brick`, whose properties it keeps.
    brick: *const UStruct,
}

// SAFETY: game code and class records living as long as the game; only used on the game thread.
unsafe impl Send for NewBrickProperties {}
unsafe impl Sync for NewBrickProperties {}

/// Finds the function (after engine init: reflection).
pub fn find() -> Result<NewBrickProperties, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let vehicle_editor = class("VehicleEditor")?;
    let brick = class("Brick")?;
    let function = ON_ANY_PROPERTY_MODIFIED.resolve()?;
    // A virtual function: some vtable holds it.
    if sig::pointers_to(function).is_empty() {
        return Err(format!(
            "{}: not a virtual function",
            ON_ANY_PROPERTY_MODIFIED.name
        ));
    }
    Ok(NewBrickProperties {
        // SAFETY: the start of OnAnyPropertyModified, which the type matches.
        on_any_property_modified: unsafe {
            core::mem::transmute::<*const u8, OnAnyPropertyModifiedFn>(function)
        },
        vehicle_editor: vehicle_editor.cast(),
        brick: brick.cast(),
    })
}

impl NewBrickProperties {
    /// Whether `editor` is a vehicle editor, which keeps properties for new bricks.
    ///
    /// # Safety
    /// `editor` must be a live object.
    pub unsafe fn is_vehicle_editor(&self, editor: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(editor, self.vehicle_editor) }
    }

    /// Whether `object` is a brick.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_brick(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.brick) }
    }

    /// Makes `editor` give new bricks `brick`'s value of `property` (one `UBrick` declares: the
    /// editor keeps no other), as after an edit of it on that brick in the panel. Whether the brick
    /// could be named to the editor (it can't if the game never referred to it weakly).
    ///
    /// # Safety
    /// `editor` must be a live vehicle editor and `brick` a live brick of it. Game thread only.
    pub unsafe fn keep(
        &self,
        editor: *mut UObjectBase,
        brick: *mut UObjectBase,
        property: FName,
    ) -> bool {
        // SAFETY: per the contract.
        let Some(weak) = (unsafe { weak_object::game_pointer(brick) }) else {
            return false;
        };
        ChangedEvent::with_object(property, weak, |event| {
            // SAFETY: per the contract; the event names one property of the brick and the brick
            // alone, and lives through the call.
            unsafe { (self.on_any_property_modified)(editor, event) }
        });
        true
    }
}
