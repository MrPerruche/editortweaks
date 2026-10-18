//! Textures made from image files at run time, and materials showing them, through the engine's
//! Blueprint libraries (all reflected, UE 4.27):
//!
//! - `UKismetRenderingLibrary::ImportFileAsTexture2D(UObject* WorldContextObject, const FString&
//!   Filename)` and `ImportBufferAsTexture2D(UObject* WorldContextObject, const TArray<uint8>&
//!   Buffer)`: decode a PNG or JPEG (the engine's image wrappers) into a new transient `UTexture2D`.
//! - `UKismetMaterialLibrary::CreateDynamicMaterialInstance(UObject* WorldContextObject,
//!   UMaterialInterface* Parent, ...)`: a material instance whose parameters can be set, with
//!   `UMaterialInstanceDynamic::SetTextureParameterValue`, `SetVectorParameterValue` and
//!   `SetScalarParameterValue`; a layered material's layers keep their own parameters, which only
//!   `SetScalarParameterValueByInfo(const FMaterialParameterInfo&, float)` reaches (the info's
//!   association `LayerParameter` and the layer's index).
//!
//! Materials can't be made at run time, only instances of existing ones: see `docs/model-io.md`
//! ("Reference images") for which parent materials the game has.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;

use crate::game::kismet;
use crate::game::reflection::{self, Function, Getter, Parameters, put, take};
use crate::game::slider_text::{FString, OwnedFString};
use crate::prelude::log;

/// `FLinearColor`.
pub type LinearColor = [f32; 4];

/// A reflected call and where its parameters go.
struct Call<const N: usize> {
    function: Function,
    parameters: [usize; N],
}

impl<const N: usize> Call<N> {
    /// `name` of `class`, with its parameters `(name, kind, size)`.
    fn find(
        class: &'static str,
        name: &'static str,
        parameters: [(&'static str, &'static str, i32); N],
    ) -> Result<Self, String> {
        let class = reflection::find_class(class).ok_or(format!("class {class} not found"))?;
        let function = Function::find(class, name)?;
        let mut offsets = [0; N];
        for (offset, (parameter, kind, size)) in offsets.iter_mut().zip(parameters) {
            *offset = function.parameter(parameter, kind, size)?;
        }
        Ok(Self {
            function,
            parameters: offsets,
        })
    }
}

const POINTER: i32 = size_of::<usize>() as i32;

/// Makes textures and material instances (after engine init).
pub struct Textures {
    /// The libraries' default objects, which their static functions are called on.
    rendering: *mut UObjectBase,
    materials: *mut UObjectBase,
    import_file: Call<3>,
    import_buffer: Call<3>,
    create_instance: Call<3>,
    set_texture: Call<2>,
    set_vector: Call<2>,
    set_scalar: Call<2>,
    /// `None` if not found (logged): layers' parameters can't be set.
    set_layer_scalar: Option<LayerScalar>,
    /// `int32 UTexture2D::Blueprint_GetSizeX() const`, `...Y`.
    width: Getter<i32>,
    height: Getter<i32>,
}

/// `UMaterialInstanceDynamic::SetScalarParameterValueByInfo`, where its `FMaterialParameterInfo`'s
/// fields go, and the association of a layer's parameters.
struct LayerScalar {
    call: Call<2>,
    name: usize,
    association: usize,
    index: usize,
    /// `EMaterialParameterAssociation::LayerParameter`.
    layer: u8,
}

// SAFETY: reflection records and default objects living as long as the game; only used on the
// game thread.
unsafe impl Send for Textures {}
unsafe impl Sync for Textures {}

/// Finds [`LayerScalar`]'s function, fields and value by reflection.
fn find_layer_scalar() -> Result<LayerScalar, String> {
    const CLASS: &str = "MaterialInstanceDynamic";
    const NAME: &str = "SetScalarParameterValueByInfo";
    const INFO: &str = "ParameterInfo";
    let call = Call::find(
        CLASS,
        NAME,
        [(INFO, "StructProperty", 16), ("Value", "FloatProperty", 4)],
    )?;
    let class = reflection::find_class(CLASS).ok_or(format!("class {CLASS} not found"))?;
    let function = reflection::find_function(class, NAME)?;
    // SAFETY: a live function record; its parameter, if found, is a live struct property.
    let info = unsafe {
        let parameter = reflection::find_property(function.cast(), INFO);
        if parameter.is_null() {
            return Err(format!("{NAME}: no {INFO}"));
        }
        reflection::struct_type(parameter)
    };
    let field = |name, kind, size| reflection::struct_field_offset(info, name, kind, size);
    let enumeration = "EMaterialParameterAssociation";
    let layer = reflection::enum_value(enumeration, "LayerParameter").or_else(|_| {
        reflection::enum_value(enumeration, "EMaterialParameterAssociation::LayerParameter")
    })?;
    Ok(LayerScalar {
        call,
        name: field("Name", "NameProperty", size_of::<FName>() as i32)?,
        association: field("Association", "ByteProperty", 1)
            .or_else(|_| field("Association", "EnumProperty", 1))?,
        index: field("Index", "IntProperty", 4)?,
        layer: u8::try_from(layer)
            .map_err(|_| format!("{enumeration}: unexpected value {layer}"))?,
    })
}

impl Textures {
    pub fn find() -> Result<Self, String> {
        let defaults = |name: &'static str, class: &'static str| {
            reflection::find_object(name, class)
                .ok_or(format!("{class}'s default object not found"))
        };
        let texture = reflection::find_class("Texture2D").ok_or("class Texture2D not found")?;
        let instance = "MaterialInstanceDynamic";
        Ok(Self {
            rendering: defaults("Default__KismetRenderingLibrary", "KismetRenderingLibrary")?,
            materials: defaults("Default__KismetMaterialLibrary", "KismetMaterialLibrary")?,
            import_file: Call::find(
                "KismetRenderingLibrary",
                "ImportFileAsTexture2D",
                [
                    ("WorldContextObject", "ObjectProperty", POINTER),
                    ("Filename", "StrProperty", size_of::<FString>() as i32),
                    ("ReturnValue", "ObjectProperty", POINTER),
                ],
            )?,
            import_buffer: Call::find(
                "KismetRenderingLibrary",
                "ImportBufferAsTexture2D",
                [
                    ("WorldContextObject", "ObjectProperty", POINTER),
                    ("Buffer", "ArrayProperty", size_of::<TArray<u8>>() as i32),
                    ("ReturnValue", "ObjectProperty", POINTER),
                ],
            )?,
            create_instance: Call::find(
                "KismetMaterialLibrary",
                "CreateDynamicMaterialInstance",
                [
                    ("WorldContextObject", "ObjectProperty", POINTER),
                    ("Parent", "ObjectProperty", POINTER),
                    ("ReturnValue", "ObjectProperty", POINTER),
                ],
            )?,
            set_texture: Call::find(
                instance,
                "SetTextureParameterValue",
                [
                    ("ParameterName", "NameProperty", size_of::<FName>() as i32),
                    ("Value", "ObjectProperty", POINTER),
                ],
            )?,
            set_vector: Call::find(
                instance,
                "SetVectorParameterValue",
                [
                    ("ParameterName", "NameProperty", size_of::<FName>() as i32),
                    ("Value", "StructProperty", size_of::<LinearColor>() as i32),
                ],
            )?,
            set_scalar: Call::find(
                instance,
                "SetScalarParameterValue",
                [
                    ("ParameterName", "NameProperty", size_of::<FName>() as i32),
                    ("Value", "FloatProperty", 4),
                ],
            )?,
            set_layer_scalar: find_layer_scalar()
                .map_err(|reason| log!("layers' parameters can't be set: {reason}"))
                .ok(),
            width: Getter::find(texture, "Blueprint_GetSizeX", "IntProperty")?,
            height: Getter::find(texture, "Blueprint_GetSizeY", "IntProperty")?,
        })
    }

    /// A new texture of the PNG or JPEG file at `path`, decoded by the engine; null if it couldn't.
    ///
    /// # Safety
    /// `context` must be a live object in a world (e.g. an actor). Game thread only.
    pub unsafe fn import_file(&self, context: *mut UObjectBase, path: &str) -> *mut UObjectBase {
        let call = &self.import_file;
        let name = OwnedFString::new(path);
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the function only reads the string (`const FString&`), which
        // outlives the call, and ProcessEvent doesn't destroy parameters.
        unsafe {
            put(&mut parameters, call.parameters[0], context);
            put(&mut parameters, call.parameters[1], name.as_ptr().read());
            call.function.call(self.rendering, &mut parameters);
            take(&parameters, call.parameters[2])
        }
    }

    /// A new texture of `bytes` (a PNG or JPEG file's contents); null if the engine couldn't.
    ///
    /// # Safety
    /// As for [`Self::import_file`].
    pub unsafe fn import_buffer(
        &self,
        context: *mut UObjectBase,
        bytes: &[u8],
    ) -> *mut UObjectBase {
        let call = &self.import_buffer;
        let buffer = TArray {
            data: bytes.as_ptr().cast_mut(),
            num: bytes.len() as i32,
            max: bytes.len() as i32,
        };
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the function only reads the array (`const TArray&`), which
        // points into `bytes` for the length of the call.
        unsafe {
            put(&mut parameters, call.parameters[0], context);
            put(&mut parameters, call.parameters[1], buffer);
            call.function.call(self.rendering, &mut parameters);
            take(&parameters, call.parameters[2])
        }
    }

    /// The size of `texture` in pixels.
    ///
    /// # Safety
    /// `texture` must be a live `UTexture2D`. Game thread only.
    pub unsafe fn size(&self, texture: *mut UObjectBase) -> [i32; 2] {
        // SAFETY: per the contract.
        unsafe { [self.width.call(texture), self.height.call(texture)] }
    }

    /// A new material instance of `parent` whose parameters can be set; null if none was made.
    ///
    /// # Safety
    /// `context` as for [`Self::import_file`], `parent` a live material. Game thread only.
    pub unsafe fn instance(
        &self,
        context: *mut UObjectBase,
        parent: *mut UObjectBase,
    ) -> *mut UObjectBase {
        let call = &self.create_instance;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the other parameters (name, flags) stay zero: none, default.
        unsafe {
            put(&mut parameters, call.parameters[0], context);
            put(&mut parameters, call.parameters[1], parent);
            call.function.call(self.materials, &mut parameters);
            take(&parameters, call.parameters[2])
        }
    }

    /// Sets the texture parameter `name` of `instance` (one [`Self::instance`] made).
    ///
    /// # Safety
    /// `instance` must be a live dynamic material instance, `texture` null or a live texture.
    /// Game thread only.
    pub unsafe fn set_texture(
        &self,
        instance: *mut UObjectBase,
        name: &str,
        texture: *mut UObjectBase,
    ) {
        // SAFETY: per the contract.
        unsafe { self.set(&self.set_texture, instance, name, texture) }
    }

    /// Sets the vector (color) parameter `name` of `instance`.
    ///
    /// # Safety
    /// As for [`Self::set_texture`].
    pub unsafe fn set_color(&self, instance: *mut UObjectBase, name: &str, color: LinearColor) {
        // SAFETY: per the contract.
        unsafe { self.set(&self.set_vector, instance, name, color) }
    }

    /// Sets the scalar parameter `name` of `instance`.
    ///
    /// # Safety
    /// As for [`Self::set_texture`].
    pub unsafe fn set_scalar(&self, instance: *mut UObjectBase, name: &str, value: f32) {
        // SAFETY: per the contract.
        unsafe { self.set(&self.set_scalar, instance, name, value) }
    }

    /// Sets the scalar parameter `name` of layer `layer` of `instance` (of a layered material).
    /// Whether it could (the setter was found).
    ///
    /// # Safety
    /// As for [`Self::set_texture`].
    pub unsafe fn set_layer_scalar(
        &self,
        instance: *mut UObjectBase,
        layer: i32,
        name: &str,
        value: f32,
    ) -> bool {
        let (Some(setter), Ok(name)) = (&self.set_layer_scalar, kismet::string_to_name(name))
        else {
            return false;
        };
        let mut info = [0u8; 16];
        // SAFETY: the fields' reflected offsets lie within the 16-byte struct.
        unsafe {
            info.as_mut_ptr()
                .add(setter.name)
                .cast::<FName>()
                .write_unaligned(name);
            info.as_mut_ptr()
                .add(setter.association)
                .write(setter.layer);
            info.as_mut_ptr()
                .add(setter.index)
                .cast::<i32>()
                .write_unaligned(layer);
        }
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameters are an `FMaterialParameterInfo` and a float.
        unsafe {
            put(&mut parameters, setter.call.parameters[0], info);
            put(&mut parameters, setter.call.parameters[1], value);
            setter.call.function.call(instance, &mut parameters);
        }
        true
    }

    /// Calls a parameter setter: the parameter's name, then its value of type `T`.
    ///
    /// # Safety
    /// As for [`Self::set_texture`], `T` the setter's value type.
    unsafe fn set<T>(&self, call: &Call<2>, instance: *mut UObjectBase, name: &str, value: T) {
        let Ok(name) = kismet::string_to_name(name) else {
            return;
        };
        let mut parameters = Parameters::new();
        // SAFETY: per the contract.
        unsafe {
            put(&mut parameters, call.parameters[0], name);
            put(&mut parameters, call.parameters[1], value);
            call.function.call(instance, &mut parameters);
        }
    }
}
