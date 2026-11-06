//! The materials bricks show, and plain versions of them for meshes of our own. All through
//! reflection; found by name in game 1.11.7 (build 25831956), see `docs/re/collision-shapes.md`.
//!
//! A brick's material is a dynamic instance of its type's base material (its static info's
//! `DefaultMaterial`, or `TranslucentMaterial` for see-through brick materials) with the
//! parameters of its brick material (wood, glass, ...) and pattern
//! (`UVehicleResourceSubsystem::FindOrCreateVehicleMID`); its color is in custom primitive data.
//!
//! Some base materials also deform the mesh into the brick's shape (wheels' tires, spinners'
//! blades, flaps), so they would deform any other mesh too. Those brick types are the ones with a
//! hidden material of their own (`HiddenMaterial`: it has to deform the same way, or hidden bricks
//! would lose their shape), which is how they're told apart here, without naming any type. The
//! plain version of their material is a new instance of the plain brick type's base material
//! (`ScalableBrickStaticInfo`'s defaults) with the same parameters.

use std::sync::{Mutex, Once};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{self, Function, Parameters, put, take};
use crate::game::weak_object::WeakObject;
use crate::prelude::log;

/// `EMIDCreationFlags::Transient`: the new instance goes in the transient package, like the
/// game's own brick instances.
const TRANSIENT: u8 = 1;

/// `static UMaterialInstanceDynamic* UKismetMaterialLibrary::CreateDynamicMaterialInstance(UObject*
/// WorldContextObject, UMaterialInterface* Parent, FName OptionalName, EMIDCreationFlags
/// CreationFlags)`.
struct Create {
    function: Function,
    context: usize,
    parent: usize,
    flags: usize,
    result: usize,
}

/// `UMaterialInstanceDynamic::K2_CopyMaterialInstanceParameters(UMaterialInterface* Source, bool
/// bQuickParametersOnly)`. Not quick: each parameter of the instance gets the source's value, if
/// the source has that parameter (`UMaterialInstance::CopyMaterialInstanceParameters`).
struct Copy {
    function: Function,
    source: usize,
    quick: usize,
}

/// Sets a material instance's own parameter values on another, each under its exact
/// `FMaterialParameterInfo`, as the game fills a brick's instance (BRMK 0x479960:
/// `SetScalarParameterValueByInfo` and `SetTextureParameterValueByInfo` with the layer parameter 0).
/// `K2_CopyMaterialInstanceParameters` only keeps parameters of the source's base material as it was
/// compiled, which may leave out the brick material's.
struct OwnValues {
    scalars: ValueList,
    textures: ValueList,
}

/// One kind of a material instance's values.
struct ValueList {
    /// Offset of the `TArray` in `UMaterialInstance`, and an element's size.
    array: usize,
    stride: usize,
    /// Offsets of `ParameterInfo` and `ParameterValue` in an element.
    info: usize,
    value: usize,
    /// `Set...ParameterValueByInfo(const FMaterialParameterInfo&, Value)`.
    set: Function,
    set_info: usize,
    set_value: usize,
    /// The size of `FMaterialParameterInfo`.
    info_size: usize,
}

impl ValueList {
    fn find(
        instance: *const UClass,
        dynamic: *const UClass,
        array: &'static str,
        element: &'static str,
        value_kind: &'static str,
        value_size: i32,
        set: &'static str,
    ) -> Result<Self, String> {
        let element: *const UStruct = reflection::find_object(element, "ScriptStruct")
            .ok_or(format!("struct {element} not found"))?
            .cast_const()
            .cast();
        let set = Function::find(dynamic, set)?;
        // SAFETY: a live struct and a live class property record: the struct `ParameterInfo` is.
        let (stride, info, info_size) = unsafe {
            let info = reflection::find_property(element, "ParameterInfo");
            if info.is_null() || !reflection::is_kind(info, "StructProperty") {
                return Err("a parameter value has no ParameterInfo".into());
            }
            (
                reflection::struct_size(element),
                (*info).offset_internal as usize,
                reflection::struct_size(reflection::struct_type(info)),
            )
        };
        Ok(Self {
            array: reflection::field_offset(instance, array, "ArrayProperty", 16)?,
            stride,
            info,
            value: reflection::struct_field_offset(
                element,
                "ParameterValue",
                value_kind,
                value_size,
            )?,
            set_info: set.parameter(
                "ParameterInfo",
                "StructProperty",
                i32::try_from(info_size).map_err(|_| "ParameterInfo is too big")?,
            )?,
            set_value: set.parameter("Value", value_kind, value_size)?,
            set,
            info_size,
        })
    }

    /// Sets every value of `source` on `target`, how many there were. `T` is the value's type.
    ///
    /// # Safety
    /// `source` must be a live material instance and `target` a live dynamic one, `T` the value's
    /// type. Game thread only.
    unsafe fn set_from<T: core::marker::Copy>(
        &self,
        source: *mut UObjectBase,
        target: *mut UObjectBase,
    ) -> usize {
        // SAFETY: per the contract; the array's fields are a `TArray`'s, its elements laid out as
        // their reflected struct, and the function reads its parameters at their reflected offsets.
        unsafe {
            let array = source.cast::<u8>().add(self.array).cast_const();
            let data = array.cast::<*const u8>().read();
            let count = usize::try_from(array.add(8).cast::<i32>().read()).unwrap_or(0);
            for index in 0..count {
                let element = data.add(index * self.stride);
                let mut parameters = Parameters::new();
                core::ptr::copy_nonoverlapping(
                    element.add(self.info),
                    parameters.as_mut_ptr().add(self.set_info),
                    self.info_size,
                );
                let value = element.add(self.value).cast::<T>().read_unaligned();
                put(&mut parameters, self.set_value, value);
                self.set.call(target, &mut parameters);
            }
            count
        }
    }
}

impl OwnValues {
    fn find(instance: *const UClass, dynamic: *const UClass) -> Result<Self, String> {
        Ok(Self {
            scalars: ValueList::find(
                instance,
                dynamic,
                "ScalarParameterValues",
                "ScalarParameterValue",
                "FloatProperty",
                4,
                "SetScalarParameterValueByInfo",
            )?,
            textures: ValueList::find(
                instance,
                dynamic,
                "TextureParameterValues",
                "TextureParameterValue",
                "ObjectProperty",
                8,
                "SetTextureParameterValueByInfo",
            )?,
        })
    }

    /// Sets `source`'s own values on `target`, and logs how many the first time.
    ///
    /// # Safety
    /// `source` must be a live material instance and `target` a live dynamic one. Game thread only.
    unsafe fn set_from(&self, source: *mut UObjectBase, target: *mut UObjectBase) {
        static LOG_FIRST: Once = Once::new();
        // SAFETY: per the contract; a scalar is a float, a texture an object pointer.
        let (scalars, textures) = unsafe {
            (
                self.scalars.set_from::<f32>(source, target),
                self.textures.set_from::<*mut UObjectBase>(source, target),
            )
        };
        LOG_FIRST.call_once(|| {
            log!("a plain material took {scalars} scalars and {textures} textures from its brick's")
        });
    }
}

/// A brick type's materials (`UBrickStaticInfo`).
#[derive(Clone, Copy)]
struct TypeMaterials {
    opaque: *mut UObjectBase,
    translucent: *mut UObjectBase,
    hidden: *mut UObjectBase,
}

pub struct BrickMaterials {
    /// Offset of `UBrickEditorObject::StaticInfoClass`: the brick's type, whose default object is
    /// its static info.
    static_info_class: usize,
    /// Offsets of `UBrickStaticInfo::DefaultMaterial`, `TranslucentMaterial`, `HiddenMaterial`.
    opaque: usize,
    translucent: usize,
    hidden: usize,
    /// `ScalableBrickStaticInfo`'s default object: the plain brick type.
    plain: *const u8,
    /// `MaterialInstance`, and the offset of its `Parent`.
    instance: *const UStruct,
    parent: usize,
    /// `KismetMaterialLibrary`'s default object, which its static functions are called on.
    library: *mut UObjectBase,
    create: Create,
    copy: Copy,
    /// Without it, a plain version only has what `copy` gives it.
    values: Option<OwnValues>,
    /// The plain versions made, each with the material it was made from. Weak pointers: the
    /// engine frees an instance once no component shows it.
    made: Mutex<Vec<(WeakObject, WeakObject)>>,
}

// SAFETY: reflection records and default objects living as long as the game, and weak pointers;
// only used on the game thread.
unsafe impl Send for BrickMaterials {}
unsafe impl Sync for BrickMaterials {}

impl BrickMaterials {
    pub fn find() -> Result<Self, String> {
        let class = |name| -> Result<*const UClass, String> {
            reflection::find_class(name).ok_or(format!("class {name} not found"))
        };
        let info = class("BrickStaticInfo")?;
        let material = |name| reflection::field_offset(info, name, "ObjectProperty", 8);
        let default_object = |name| {
            // SAFETY: a live class.
            unsafe { reflection::default_object(class(name)?) }
                .ok_or(format!("{name} has no default object"))
        };
        let instance = class("MaterialInstance")?;

        let library = "KismetMaterialLibrary";
        let function = Function::find(class(library)?, "CreateDynamicMaterialInstance")?;
        let create = Create {
            context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
            parent: function.parameter("Parent", "ObjectProperty", 8)?,
            // `EMIDCreationFlags` is an `enum class : uint8`.
            flags: function
                .parameter("CreationFlags", "EnumProperty", 1)
                .or_else(|_| function.parameter("CreationFlags", "ByteProperty", 1))?,
            result: function.parameter("ReturnValue", "ObjectProperty", 8)?,
            function,
        };
        // `OptionalName` stays `NAME_None` (zero): the engine names the instance.
        create
            .function
            .parameter("OptionalName", "NameProperty", 8)?;

        let function = Function::find(
            class("MaterialInstanceDynamic")?,
            "K2_CopyMaterialInstanceParameters",
        )?;
        let copy = Copy {
            source: function.parameter("Source", "ObjectProperty", 8)?,
            quick: function.parameter("bQuickParametersOnly", "BoolProperty", 1)?,
            function,
        };
        Ok(Self {
            static_info_class: reflection::field_offset(
                class("BrickEditorObject")?,
                "StaticInfoClass",
                "ClassProperty",
                8,
            )?,
            opaque: material("DefaultMaterial")?,
            translucent: material("TranslucentMaterial")?,
            hidden: material("HiddenMaterial")?,
            plain: default_object("ScalableBrickStaticInfo")?
                .cast_const()
                .cast(),
            instance: instance.cast(),
            parent: reflection::field_offset(instance, "Parent", "ObjectProperty", 8)?,
            library: default_object(library)?,
            create,
            copy,
            values: OwnValues::find(instance, class("MaterialInstanceDynamic")?)
                .inspect_err(|reason| {
                    log!("plain materials may not show their brick's textures: {reason}")
                })
                .ok(),
            made: Mutex::new(Vec::new()),
        })
    }

    /// The material hidden plain bricks show: what hidden bricks' meshes of our own show.
    pub fn plain_hidden(&self) -> *mut UObjectBase {
        // SAFETY: the plain type's default object lives as long as the game.
        unsafe { self.materials(self.plain).hidden }
    }

    /// `material` (`brick`'s own) as a mesh of our own should show it: itself, unless `brick`'s
    /// type deforms the mesh in its material (see the module's comment); then its plain version,
    /// made the first time it's needed (`context` gives the engine a world). Falls back to
    /// `material` itself if the engine makes no instance.
    ///
    /// # Safety
    /// `brick` must be a live brick, `material` a live material and `context` a live object in the
    /// brick's world. Game thread only.
    pub unsafe fn plain_version(
        &self,
        brick: *mut UObjectBase,
        material: *mut UObjectBase,
        context: *mut UObjectBase,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract; materials read from static infos are live (their default
        // objects hold them).
        unsafe {
            let plain = self.materials(self.plain);
            let Some(own) = self.type_of(brick) else {
                return material;
            };
            if own.hidden.is_null() || own.hidden == plain.hidden || plain.opaque.is_null() {
                return material;
            }
            if let Some(made) = self.made_from(material) {
                return made;
            }
            // The translucent base for a see-through brick material, as the game picks it.
            let base = if self.base_of(material) == own.translucent && !plain.translucent.is_null()
            {
                plain.translucent
            } else {
                plain.opaque
            };
            let made = self.instance_of(base, material, context);
            if made.is_null() {
                return material;
            }
            if let Ok(mut all) = self.made.lock() {
                all.retain(|(source, made)| source.get().is_some() && made.get().is_some());
                all.push((WeakObject::new(material), WeakObject::new(made)));
            }
            made
        }
    }

    /// The plain version made from `material` before, if the engine didn't free it since.
    fn made_from(&self, material: *mut UObjectBase) -> Option<*mut UObjectBase> {
        let all = self.made.lock().ok()?;
        all.iter()
            .find(|(source, _)| source.is(material) && source.get().is_some())
            .and_then(|(_, made)| made.get())
    }

    /// A new dynamic instance of `base` with the parameters of `source`, or null.
    ///
    /// # Safety
    /// `base` and `source` must be live materials, `context` a live object. Game thread only.
    unsafe fn instance_of(
        &self,
        base: *mut UObjectBase,
        source: *mut UObjectBase,
        context: *mut UObjectBase,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract; each call gets its parameters at their reflected offsets.
        unsafe {
            let call = &self.create;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.context, context);
            put(&mut parameters, call.parent, base);
            put(&mut parameters, call.flags, TRANSIENT);
            call.function.call(self.library, &mut parameters);
            let made = take::<*mut UObjectBase>(&parameters, call.result);
            if made.is_null() {
                return made;
            }
            let call = &self.copy;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.source, source);
            put(&mut parameters, call.quick, false);
            call.function.call(made, &mut parameters);
            if let Some(values) = &self.values {
                values.set_from(source, made);
            }
            made
        }
    }

    /// The material `material` is an instance of (its `Parent`), or itself if it isn't one.
    ///
    /// # Safety
    /// `material` must be a live material.
    pub unsafe fn base_of(&self, material: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the offset comes from reflection, checked against the class.
        unsafe {
            if !reflection::with_parents(reflection::class_of(material.cast()))
                .any(|class| class == self.instance)
            {
                return material;
            }
            let parent = material
                .cast::<u8>()
                .add(self.parent)
                .cast::<*mut UObjectBase>()
                .read();
            if parent.is_null() { material } else { parent }
        }
    }

    /// The materials of `brick`'s type, if it has one.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    unsafe fn type_of(&self, brick: *mut UObjectBase) -> Option<TypeMaterials> {
        // SAFETY: per the contract; the offset comes from reflection, and the class is null or a
        // live static info class.
        unsafe {
            let class = brick
                .cast::<u8>()
                .add(self.static_info_class)
                .cast::<*const UClass>()
                .read();
            if class.is_null() {
                return None;
            }
            let info = reflection::default_object(class)?;
            Some(self.materials(info.cast_const().cast()))
        }
    }

    /// The materials of the static info at `info`.
    ///
    /// # Safety
    /// `info` must point to a live `UBrickStaticInfo`.
    unsafe fn materials(&self, info: *const u8) -> TypeMaterials {
        // SAFETY: per the contract; the offsets come from reflection.
        let read = |offset| unsafe { info.add(offset).cast::<*mut UObjectBase>().read() };
        TypeMaterials {
            opaque: read(self.opaque),
            translucent: read(self.translucent),
            hidden: read(self.hidden),
        }
    }
}
