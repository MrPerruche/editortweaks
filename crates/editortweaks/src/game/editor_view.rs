//! The vehicle editor's view modes (`EBrickEditorViewMode`, cycled with Z): how they color bricks,
//! how bricks are paired by mirror mode, and adding a view mode. Found through BRMK 1.11.6's
//! exports, then in game 1.11.7 (build 25831956); see `docs/re/editor-view-modes.md`.
//!
//! A brick in the editor renders with its own materials, or with an override material that depends
//! on the view mode ([`MATERIAL_OVERRIDE`]); its custom primitive data carry its colors
//! ([`PRIMITIVE_DATA`]). The group view modes (welds, groups) use its static info's
//! `ColorVisualizationMaterial`, which shows the color in data slots 0 to 2 (and a second color, for
//! more distinct groups, in slots 3 to 5).

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fmalloc;
use brickrust::ue::tarray::TArray;
use brickrust::ue::tshared::FWeakObjectPtr;
use brickrust::ue::uclass::{UFunction, UStruct};

use crate::framework::sig::{FunctionSig, SigMatch, StringRefSig};
use crate::game::kismet;
use crate::game::property_reflection::{ITEM_NAME, ITEM_SIZE, RawArray};
use crate::game::reflection::{self, ENUM_NAMES, EnumEntry, Getter, Parameters};
use crate::game::text::Text;
use crate::game::weak_object;

/// `UMaterialInterface* UBrick::GetEditorMaterialOverride() const` (BRMK 0xACBF0, game RVA
/// 0xCE27D0): the material replacing all of a brick's materials in the editor, or null:
///
/// ```text
/// StaticInfo = GetBrickStaticInfo();
/// if (EditorParams && EditorParams->Flags & 0x18) return StaticInfo->HiddenMaterial;
/// switch (GetViewMode()) {
///     case Physics: return StaticInfo->PhysicsVisualizationMaterial;
///     case WeldGroups: case EditorGroups: return StaticInfo->ColorVisualizationMaterial;
///     default: return nullptr;   // the brick's own materials
/// }
/// ```
///
/// `UBrick::UpdateBrickMaterial` (game 0xD0F0A0) sets every material of the brick's mesh to it, then
/// calls [`PRIMITIVE_DATA`]. One more caller (0xCB8810).
pub const MATERIAL_OVERRIDE: FunctionSig = FunctionSig {
    name: "UBrick::GetEditorMaterialOverride",
    // GetBrickStaticInfo() (call at +3), the hidden check on EditorParams (its offset at +14, the
    // flags' at +22), the early return, GetViewMode() (call at +0x2F), then the switch.
    pattern: "48 8b f9 e8 ?? ?? ?? ?? 48 8b d8 48 8b 47 ?? 48 85 c0 74 ?? f6 40 ?? 18 74 ?? 48 8b 83 ?? ?? ?? ?? 48 8b 5c 24 ?? 48 83 c4 ?? 5f c3 48 8b cf e8 ?? ?? ?? ?? 0f b6 c8 83 e9 02 74",
};

pub type MaterialOverrideFn = unsafe extern "C" fn(brick: *mut UObjectBase) -> *mut UObjectBase;

/// Our name for the `UBrick` function writing a brick's custom primitive data (BRMK 0x42C40, game
/// RVA 0xCB8110), called by `UpdateBrickMaterial` and `OnEditorMoveUpdate`:
///
/// ```text
/// if ((Flags & 0x18) == 8) {                         // a brick in the editor
///     switch (GetViewMode()) {
///     case Physics: ...center of mass data...; return;
///     case WeldGroups: case EditorGroups:
///         Group = weld group, or EditorParams->Group;
///         // channels 1.0 for set bits, else 0.2 (all 0.05 for group 0)
///         SetCustomPrimitiveDataVector4(0, {RGB from bits 3 to 5, 1});
///         SetCustomPrimitiveDataVector4(3, {RGB from bits 0 to 2, 1});
///         return;
///     }
/// }
/// SetCustomPrimitiveDataVector4(0, BrickColor); ...   // then slots 4 to 17: transform, pattern
/// ```
pub const PRIMITIVE_DATA: FunctionSig = FunctionSig {
    name: "UBrick::UpdatePrimitiveData",
    // The editor check (the flags' offset at +3) while saving the arguments, then GetViewMode()
    // (call at +0x1A) compared with Physics.
    pattern: "0f b6 41 ?? 49 8b f1 24 18 4d 8b f0 48 8b fa 48 8b d9 3c 08 0f 85 ?? ?? ?? ?? e8 ?? ?? ?? ?? 3c 02 0f 85",
};

/// `component` points to the brick's shared weak pointer to its mesh; the two transforms are passed
/// through untouched.
pub type PrimitiveDataFn = unsafe extern "C" fn(
    brick: *mut UObjectBase,
    component: *const c_void,
    transform: *const c_void,
    relative: *const c_void,
);

/// `void ABrickEditor::UpdateMirroredObjects()` (BRMK 0xFA450, game RVA 0xD11EA0): with mirror mode
/// on, pairs each brick on the positive side of the mirror axis with the brick at its mirror image
/// (same mirrored class, location, rotation and mirrored properties) and sets their mirrored objects
/// to each other; with mirror mode off, clears them all. Called after every change (`PostModify`),
/// when objects are created and when the mirror settings change.
pub const UPDATE_MIRRORED_OBJECTS: FunctionSig = FunctionSig {
    name: "ABrickEditor::UpdateMirroredObjects",
    // The editor interface component, then the user settings' bEditorMirrorAxisEnabled and
    // EditorMirrorAxis.
    pattern: "4c 8b e9 e8 ?? ?? ?? ?? 33 ff 49 8d b5 ?? ?? ?? ?? 40 38 b8 ?? ?? ?? ?? 0f 84 ?? ?? ?? ?? 0f b6 80 ?? ?? ?? ?? 89 44 24 ?? 85 c0 0f 84",
};

pub type UpdateMirroredObjectsFn = unsafe extern "C" fn(editor: *mut UObjectBase);

/// `static void UBrickUserSettings::GetEditorViewModeItems(const FBrickPropertyContainer&,
/// TArray<FEnumPropertyItem>& OutItems)` (BRMK 0x320630, game RVA 0xE66EF0): appends the view
/// modes as the settings show them, a fixed list of the game's six, named from the `BrickEditor`
/// string table (`Name_ViewMode_Default`, ...). Used for the view mode list of the editor's panel
/// (when nothing is selected) and for the name in the notification after Z
/// (`GetEnumPropertyItemText`, game 0xDBEF20: the name of the item with the setting's value, or
/// "Invalid"). Only called through function pointers.
pub const VIEW_MODE_ITEMS: StringRefSig = StringRefSig {
    name: "UBrickUserSettings::GetEditorViewModeItems",
    text: "Name_ViewMode_Default",
};

/// `items` is a `TArray<FEnumPropertyItem>` (see [`ViewModeItems::append`]).
pub type ViewModeItemsFn = unsafe extern "C" fn(container: *const c_void, items: *mut c_void);

/// `FEnumPropertyItem`'s copy constructor (game RVA 0xCC4380, 132 callers, including
/// [`VIEW_MODE_ITEMS`] adding each item): copies the value, the display info (name, icon) and three
/// more texts, adding references to them.
const ITEM_COPY: FunctionSig = FunctionSig {
    name: "FEnumPropertyItem copy constructor",
    // The value, the display info's copy (call), then the next text with its reference.
    pattern: "8b 02 48 8b fa 89 01 48 8b d9 48 83 c1 08 48 83 c2 08 e8 ?? ?? ?? ?? 48 8b 47 48 48 89 43 48 48 8b 47 50",
};

type ItemCopyFn = unsafe extern "C" fn(target: *mut u8, source: *const u8) -> *mut u8;

/// Adds view modes to the game's view mode lists ([`VIEW_MODE_ITEMS`]).
pub struct ViewModeItems {
    /// [`VIEW_MODE_ITEMS`]'s start, to hook.
    pub function: *const u8,
    copy: ItemCopyFn,
}

// SAFETY: points into the game's code; only used on the game thread (and while loading).
unsafe impl Send for ViewModeItems {}
unsafe impl Sync for ViewModeItems {}

pub fn view_mode_items() -> Result<ViewModeItems, String> {
    let function = VIEW_MODE_ITEMS.resolve()?;
    let copy = ITEM_COPY.resolve()?;
    Ok(ViewModeItems {
        function,
        // SAFETY: the copy constructor has this signature (see ITEM_COPY).
        copy: unsafe { core::mem::transmute::<*const u8, ItemCopyFn>(copy) },
    })
}

impl ViewModeItems {
    /// Appends the view mode `value`, shown as `name`, to the list [`VIEW_MODE_ITEMS`] just filled:
    /// a copy of the last item (made by the game's copy constructor, so its texts keep the right
    /// reference counts) with our value and name. Lists that aren't the game's (values 0, 1, 2, ...,
    /// below `value`) are left alone.
    ///
    /// # Safety
    /// `items` must be the `TArray<FEnumPropertyItem>` the game passed to [`VIEW_MODE_ITEMS`].
    /// Game thread only.
    pub unsafe fn append(&self, items: *mut c_void, value: u8, name: &Text) {
        // SAFETY: per the contract; items are relocatable like every `TArray` element, so growing
        // the array with the game's allocator is what `TArray` does too.
        unsafe {
            let array = &mut *items.cast::<RawArray>();
            let count = match usize::try_from(array.num) {
                Ok(count) if count > 0 && count <= usize::from(value) && !array.data.is_null() => {
                    count
                }
                _ => return,
            };
            let item = |data: *mut u8, index: usize| data.add(index * ITEM_SIZE);
            if !(0..count).all(|index| item(array.data, index).cast::<i32>().read() == index as i32)
            {
                return;
            }
            if array.max as usize <= count {
                let grown =
                    fmalloc::realloc(array.data.cast(), (count + 1) * ITEM_SIZE).cast::<u8>();
                if grown.is_null() {
                    return;
                }
                array.data = grown;
                array.max = count as i32 + 1;
            }
            let added = item(array.data, count);
            (self.copy)(added, item(array.data, count - 1));
            added.cast::<i32>().write(i32::from(value));
            name.assign_to(added.add(ITEM_NAME).cast());
            array.num = count as i32 + 1;
        }
    }
}

type ViewModeFn = unsafe extern "C" fn(object: *const UObjectBase) -> u8;
type StaticInfoFn = unsafe extern "C" fn(brick: *const UObjectBase) -> *const u8;

/// `EditorParams` flags of a brick in the editor, in the byte at [`EditorCode::flags`]:
/// `(flags & EDITOR_MASK) == IN_EDITOR`, the primitive data's own check.
const EDITOR_MASK: u8 = 0x18;
const IN_EDITOR: u8 = 0x08;

/// `EditorParams` flags of a hidden editor object (`GetEditorMaterialOverride`'s check, the byte's
/// offset read from it).
const HIDDEN: u8 = 0x18;

/// Offset of the mirrored object (an `FWeakObjectPtr`) in an editor object's `EditorParams`: where
/// `UpdateMirroredObjects` assigns it (game 0xCB6D68: `mov rcx, [object + 0x40]` straight into
/// `FWeakObjectPtr::operator=`). 8 in BRMK, whose `EditorParams` has an editor-only field first.
const MIRRORED_OBJECT: usize = 0;

/// The game code the view modes need, found by signature.
pub struct EditorCode {
    pub material_override: *const u8,
    pub primitive_data: *const u8,
    pub update_mirrored_objects: *const u8,
    /// `UBrickEditorObject::GetViewMode()`: the user settings' `EditorViewMode`, 0 outside the editor.
    view_mode: ViewModeFn,
    /// `UBrick::GetBrickStaticInfo()`: the default object of the brick's static info class.
    static_info: StaticInfoFn,
    /// Offset of a brick's flags byte (see [`IN_EDITOR`]).
    flags: usize,
    /// Offset of an editor object's `EditorParams` pointer.
    editor_params: usize,
    /// Offset of the flags byte in `EditorParams` (see [`EditorCode::is_hidden`]).
    params_flags: usize,
}

// SAFETY: points into the game's code; only used on the game thread (and while loading).
unsafe impl Send for EditorCode {}
unsafe impl Sync for EditorCode {}

pub fn editor_code() -> Result<EditorCode, String> {
    let material = MATERIAL_OVERRIDE.locate()?;
    let data = PRIMITIVE_DATA.locate()?;
    let mirrored = UPDATE_MIRRORED_OBJECTS.locate()?;
    let call = |found: &SigMatch, offset, sig: &FunctionSig, what: &str| {
        // SAFETY: each offset is a call inside its pattern (see the patterns' comments).
        unsafe { found.call_target(offset) }
            .ok_or_else(|| format!("{}: no call to {what}", sig.name))
    };
    let static_info = call(&material, 3, &MATERIAL_OVERRIDE, "GetBrickStaticInfo")?;
    let view_mode = call(&material, 0x2F, &MATERIAL_OVERRIDE, "GetViewMode")?;
    if call(&data, 0x1A, &PRIMITIVE_DATA, "GetViewMode")? != view_mode {
        return Err(format!(
            "{} and {} call different functions",
            MATERIAL_OVERRIDE.name, PRIMITIVE_DATA.name
        ));
    }
    // SAFETY: both are displacement bytes inside their patterns; the call targets are game
    // functions with these signatures (see the patterns' comments).
    unsafe {
        Ok(EditorCode {
            material_override: material.function,
            primitive_data: data.function,
            update_mirrored_objects: mirrored.function,
            view_mode: core::mem::transmute::<*const u8, ViewModeFn>(view_mode),
            static_info: core::mem::transmute::<*const u8, StaticInfoFn>(static_info),
            flags: usize::from(*data.address.add(3)),
            editor_params: usize::from(*material.address.add(14)),
            params_flags: usize::from(*material.address.add(22)),
        })
    }
}

impl EditorCode {
    /// The editor's view mode, if `brick` is a brick in the editor (the others have none).
    ///
    /// # Safety
    /// `brick` must be a live brick (`UBrick`). Game thread only.
    pub unsafe fn view_mode(&self, brick: *const UObjectBase) -> Option<u8> {
        // SAFETY: per the contract; the offset comes from the game's own check.
        unsafe {
            let flags = brick.cast::<u8>().add(self.flags).read();
            (flags & EDITOR_MASK == IN_EDITOR).then(|| (self.view_mode)(brick))
        }
    }

    /// The brick mirror mode pairs `object` with, on the other side of the mirror plane, if any
    /// (nothing on the plane is paired).
    ///
    /// # Safety
    /// `object` must be a live editor object (`UBrickEditorObject`). Game thread only.
    pub unsafe fn mirrored_object(&self, object: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; `EditorParams` is null or points to the object's parameters.
        unsafe {
            let params = object
                .cast::<u8>()
                .add(self.editor_params)
                .cast::<*const u8>()
                .read();
            if params.is_null() {
                return None;
            }
            let pointer = params.add(MIRRORED_OBJECT).cast::<FWeakObjectPtr>().read();
            weak_object::resolve(pointer)
        }
    }

    /// Whether `object` is hidden in the editor (H, or hidden by the view mode): the flags
    /// `GetEditorMaterialOverride` checks to give it the hidden material.
    ///
    /// # Safety
    /// `object` must be a live editor object (`UBrickEditorObject`). Game thread only.
    pub unsafe fn is_hidden(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract; `EditorParams` is null or points to the object's parameters,
        // and both offsets come from the game's own check.
        unsafe {
            let params = object
                .cast::<u8>()
                .add(self.editor_params)
                .cast::<*const u8>()
                .read();
            !params.is_null() && params.add(self.params_flags).read() & HIDDEN != 0
        }
    }

    /// The static info of `brick` (a `UBrickStaticInfo`), or null.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    pub unsafe fn static_info(&self, brick: *const UObjectBase) -> *const u8 {
        // SAFETY: per the contract.
        unsafe { (self.static_info)(brick) }
    }
}

/// Bricks and the editor's objects through reflection (after engine init).
pub struct Bricks {
    /// `Brick`.
    brick: *const UStruct,
    /// Offset of `UBrickStaticInfo::ColorVisualizationMaterial`.
    color_material: usize,
    /// Offset of `UBrickStaticInfo::HiddenMaterial`.
    hidden_material: usize,
    /// Offset of `ABrickEditor::EditorInterfaceComponent`.
    interface: usize,
    /// Offset of `UBrickEditorInterfaceComponent::BrickEditorObjects` (a `TArray`).
    objects: usize,
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`: the brick's mesh.
    root_component: Getter<*mut UObjectBase>,
    /// `ABrickEditor* UBrickEditorObject::GetBrickEditor()`.
    editor: Getter<*mut UObjectBase>,
    set_data: SetCustomPrimitiveData,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Bricks {}
unsafe impl Sync for Bricks {}

pub fn bricks() -> Result<Bricks, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let brick = class("Brick")?;
    let info = class("BrickStaticInfo")?;
    let editor = class("BrickEditor")?;
    let interface = class("BrickEditorInterfaceComponent")?;
    let object = class("BrickEditorObject")?;
    let component = class("PrimitiveComponent")?;
    Ok(Bricks {
        brick: brick.cast(),
        color_material: reflection::field_offset(
            info,
            "ColorVisualizationMaterial",
            "ObjectProperty",
            8,
        )?,
        hidden_material: reflection::field_offset(info, "HiddenMaterial", "ObjectProperty", 8)?,
        interface: reflection::field_offset(
            editor,
            "EditorInterfaceComponent",
            "ObjectProperty",
            8,
        )?,
        objects: reflection::field_offset(interface, "BrickEditorObjects", "ArrayProperty", 0x10)?,
        root_component: Getter::find(object, "GetRootComponent", "ObjectProperty")?,
        editor: Getter::find(object, "GetBrickEditor", "ObjectProperty")?,
        set_data: SetCustomPrimitiveData::find(reflection::find_function(
            component,
            "SetCustomPrimitiveDataVector4",
        )?)?,
    })
}

impl Bricks {
    /// Whether `object` is a brick.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_brick(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|class| class == self.brick)
    }

    /// The editor `object` (an editor object, e.g. a brick) belongs to, or null.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn editor_of(&self, object: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe { self.editor.call(object) }
    }

    /// The editor's interface component (`UBrickEditorInterfaceComponent`), holding its objects, or
    /// null.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    pub unsafe fn interface(&self, editor: *const UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            editor
                .cast::<u8>()
                .add(self.interface)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }

    /// The editor's objects (bricks and others; entries may be null).
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`, and the slice is only valid until its objects change.
    /// Game thread only.
    pub unsafe fn editor_objects<'a>(&self, editor: *const UObjectBase) -> &'a [*mut UObjectBase] {
        // SAFETY: per the contract; both offsets come from reflection.
        unsafe {
            let interface = self.interface(editor).cast::<u8>();
            if interface.is_null() {
                return &[];
            }
            let objects = &*interface
                .add(self.objects)
                .cast::<TArray<*mut UObjectBase>>();
            match usize::try_from(objects.num) {
                Ok(count) if count > 0 && !objects.data.is_null() => {
                    core::slice::from_raw_parts(objects.data, count)
                }
                _ => &[],
            }
        }
    }

    /// The material the group view modes give `brick` (`ColorVisualizationMaterial`), or null.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    pub unsafe fn color_material(
        &self,
        code: &EditorCode,
        brick: *const UObjectBase,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe { self.static_info_material(code, brick, self.color_material) }
    }

    /// The material hidden bricks show (`HiddenMaterial`: dithered, see-through), or null.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    pub unsafe fn hidden_material(
        &self,
        code: &EditorCode,
        brick: *const UObjectBase,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe { self.static_info_material(code, brick, self.hidden_material) }
    }

    /// The material at `offset` in `brick`'s static info, or null.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor, and `offset` that of a material field of
    /// `UBrickStaticInfo`. Game thread only.
    unsafe fn static_info_material(
        &self,
        code: &EditorCode,
        brick: *const UObjectBase,
        offset: usize,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract; the static info is a UBrickStaticInfo.
        unsafe {
            let info = code.static_info(brick);
            if info.is_null() {
                return core::ptr::null_mut();
            }
            info.add(offset).cast::<*mut UObjectBase>().read()
        }
    }

    /// Shows two colors (linear RGB) through the group view modes' material, which mixes them in a
    /// pattern (data slots 0 to 2 and 3 to 5, like a group's two colors); the same color twice shows
    /// as a plain color.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn paint(&self, brick: *mut UObjectBase, colors: [[f32; 3]; 2]) {
        let value = |[red, green, blue]: [f32; 3]| [red, green, blue, 1.0];
        // SAFETY: per the contract; the mesh is a live primitive component if not null.
        unsafe {
            let mesh = self.root_component.call(brick);
            if !mesh.is_null() {
                self.set_data.call(mesh, 0, value(colors[0]));
                self.set_data.call(mesh, 3, value(colors[1]));
            }
        }
    }
}

/// `UPrimitiveComponent::SetCustomPrimitiveDataVector4(int32 DataIndex, FVector4 Value)`: sets four
/// consecutive custom primitive data slots, which the component's materials read.
struct SetCustomPrimitiveData {
    function: *const UFunction,
    index: usize,
    value: usize,
}

impl SetCustomPrimitiveData {
    fn find(function: *const UFunction) -> Result<Self, String> {
        let parameters = function.cast::<UStruct>();
        let field = |name, kind, size| {
            reflection::struct_field_offset(parameters, name, kind, size)
                .map_err(|reason| format!("SetCustomPrimitiveDataVector4: {reason}"))
        };
        let found = Self {
            function,
            index: field("DataIndex", "IntProperty", 4)?,
            value: field("Value", "StructProperty", 16)?,
        };
        // SAFETY: a live function record.
        if !unsafe { Parameters::fit(function) } || !found.value.is_multiple_of(16) {
            return Err("SetCustomPrimitiveDataVector4: unexpected parameters".into());
        }
        Ok(found)
    }

    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    unsafe fn call(&self, component: *mut UObjectBase, index: i32, value: [f32; 4]) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; both fields lie within the checked parameter size.
        unsafe {
            let base = parameters.as_mut_ptr();
            base.add(self.index).cast::<i32>().write(index);
            base.add(self.value).cast::<[f32; 4]>().write(value);
            reflection::call(component, self.function, base);
        }
    }
}

/// Adds the view mode `EBrickEditorViewMode::<name>` and returns its value. The view mode cycle
/// (`UBrickEditorDefaultMode::CycleViewMode`) steps through the enum's entries except the last
/// (`_MAX`), so the new one goes right before it, with `_MAX`'s value; `_MAX` grows by one.
/// Adding a name that is already there returns its value.
pub fn add_view_mode(name: &str) -> Result<u8, String> {
    let view_modes = reflection::find_object("EBrickEditorViewMode", "Enum")
        .ok_or("enum EBrickEditorViewMode not found")?;
    let full_name = kismet::string_to_name(&format!("EBrickEditorViewMode::{name}"))?;
    // SAFETY: a live UEnum (checked by class name); its `Names` array is read and grown with the
    // game's allocator, like `TArray` does, on the game thread.
    unsafe {
        let names = &mut *view_modes
            .cast::<u8>()
            .add(ENUM_NAMES)
            .cast::<TArray<EnumEntry>>();
        let count = usize::try_from(names.num)
            .ok()
            .filter(|count| (2..=64).contains(count) && !names.data.is_null())
            .ok_or("EBrickEditorViewMode: unexpected entries")?;
        let entries = core::slice::from_raw_parts(names.data, count);
        let max = entries[count - 1];
        let in_order = entries
            .iter()
            .enumerate()
            .all(|(index, entry)| entry.value == index as i64);
        if !in_order || !max.name.to_string().to_ascii_uppercase().ends_with("_MAX") {
            return Err("EBrickEditorViewMode: unexpected entries".into());
        }
        let same = |entry: &EnumEntry| {
            entry.name.comparison_index == full_name.comparison_index
                && entry.name.number == full_name.number
        };
        if let Some(existing) = entries.iter().find(|entry| same(entry)) {
            return u8::try_from(existing.value).map_err(|_| "view mode value too big".into());
        }
        let value = u8::try_from(max.value).map_err(|_| "view mode value too big")?;
        let grown = fmalloc::realloc(names.data.cast(), (count + 1) * size_of::<EnumEntry>())
            .cast::<EnumEntry>();
        if grown.is_null() {
            return Err("EBrickEditorViewMode: out of memory".into());
        }
        grown.add(count - 1).write(EnumEntry {
            name: full_name,
            value: max.value,
        });
        grown.add(count).write(EnumEntry {
            name: max.name,
            value: max.value + 1,
        });
        names.data = grown;
        names.num = (count + 1) as i32;
        names.max = names.num;
        Ok(value)
    }
}
