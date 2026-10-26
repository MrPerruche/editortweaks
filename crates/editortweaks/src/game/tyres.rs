//! Wheels' tyres: which of a brick's material slots show one, and the textures it's drawn with.
//! All through reflection; found in BRMK 1.11.6's assets, see `docs/re/model-export.md` ("Wheels'
//! tyres").
//!
//! A wheel's mesh has two slots (`SM_Wheel_4x2`: `M_Brick`, then `M_Wheel_4x2`): the rim shows the
//! brick's material, the tyre an instance of `M_Tire_Master`, which ignores the brick's colour and
//! material: rubber (the texture [`RUBBER`], roughness 0.9 times the rubber's own), with the
//! instance's normal map (the tread) and, for some, its ambient occlusion map, all at the mesh's
//! UVs. The instance's textures are its `TextureParameterValues` (or its parents').

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::game::mesh_component::MeshComponents;
use crate::game::reflection::{self, Getter};

/// The base material of every tyre.
const TYRE_MATERIAL: &str = "M_Tire_Master";

/// The texture `M_Tire_Master` draws tyres with (its `bHasDiffuse` switch, which would take a
/// texture parameter instead, is off in every tyre).
pub const RUBBER: &str = "Rubber_ScratchedWorn_1k_basecolor";

/// `M_Tire_Master`'s texture parameters: the tread's normals, and the occlusion its `bHasAOMap`
/// switch multiplies the colour with (on only in the instances that set this map).
const NORMAL_MAP: &str = "NormalMap";
const OCCLUSION_MAP: &str = "AmbientOcclusion";

/// How many parents up a material instance's textures are looked for.
const MAX_PARENTS: usize = 8;

/// A tyre slot's own textures (null where it has none).
#[derive(Clone, Copy)]
pub struct Tyre {
    pub normal: *mut UObjectBase,
    pub occlusion: *mut UObjectBase,
}

/// Where a material instance's texture parameters are.
struct Parameters {
    instance_class: *const UStruct,
    /// `UMaterialInstance::Parent`.
    parent: usize,
    /// `UMaterialInstance::TextureParameterValues` (`TArray<FTextureParameterValue>`).
    values: usize,
    /// `FTextureParameterValue`'s size, its `ParameterInfo.Name` and `ParameterValue`.
    value_size: usize,
    name: usize,
    texture: usize,
}

/// Reads bricks' tyre slots (after engine init).
pub struct Tyres {
    components: MeshComponents,
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`.
    root: Getter<*mut UObjectBase>,
    /// `UMaterial* UMaterialInterface::GetBaseMaterial()`.
    base: Getter<*mut UObjectBase>,
    /// `None` if they weren't found: tyres are then exported without their own textures.
    parameters: Option<Parameters>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Tyres {}
unsafe impl Sync for Tyres {}

impl Parameters {
    fn find() -> Result<Self, String> {
        let instance =
            reflection::find_class("MaterialInstance").ok_or("class MaterialInstance not found")?;
        let values = reflection::field(instance, "TextureParameterValues", "ArrayProperty", 16)?;
        // SAFETY: live property records: the array's element and `ParameterInfo` are structs.
        unsafe {
            let value = reflection::struct_type(reflection::array_inner(values));
            if value.is_null() {
                return Err("TextureParameterValues: not an array of structs".into());
            }
            let info = reflection::find_property(value, "ParameterInfo");
            if info.is_null() || !reflection::is_kind(info, "StructProperty") {
                return Err("field 'ParameterInfo' not found".into());
            }
            let name = reflection::struct_field_offset(
                reflection::struct_type(info),
                "Name",
                "NameProperty",
                size_of::<FName>() as i32,
            )?;
            Ok(Self {
                instance_class: instance.cast(),
                parent: reflection::field_offset(instance, "Parent", "ObjectProperty", 8)?,
                values: (*values).offset_internal as usize,
                value_size: reflection::struct_size(value),
                name: (*info).offset_internal as usize + name,
                texture: reflection::struct_field_offset(
                    value,
                    "ParameterValue",
                    "ObjectProperty",
                    8,
                )?,
            })
        }
    }

    /// The texture `material` (or the first of its parents that sets one) gives the parameter
    /// `name`, null if none does.
    ///
    /// # Safety
    /// `material` must be a live material (or null). Game thread only.
    unsafe fn texture(&self, mut material: *mut UObjectBase, name: &str) -> *mut UObjectBase {
        // SAFETY: per the contract; instances' parents are live materials (or null), and the
        // array is read within its count.
        unsafe {
            for _ in 0..MAX_PARENTS {
                if material.is_null() || !reflection::is_a(material, self.instance_class) {
                    break;
                }
                let base = material.cast::<u8>();
                let values = base.add(self.values).cast::<TArray<u8>>().read();
                if !values.data.is_null() && (0..=0x1000).contains(&values.num) {
                    for index in 0..values.num as usize {
                        let value = values.data.add(index * self.value_size);
                        if reflection::name_is(&*value.add(self.name).cast::<FName>(), name) {
                            let texture = value.add(self.texture).cast::<*mut UObjectBase>().read();
                            if !texture.is_null() {
                                return texture;
                            }
                        }
                    }
                }
                material = base.add(self.parent).cast::<*mut UObjectBase>().read();
            }
            core::ptr::null_mut()
        }
    }
}

impl Tyres {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let parameters = Parameters::find()
            .map_err(|reason| crate::prelude::log!("tyres' own textures not exported: {reason}"))
            .ok();
        Ok(Self {
            components: MeshComponents::find()?,
            root: Getter::find(
                class("BrickEditorObject")?,
                "GetRootComponent",
                "ObjectProperty",
            )?,
            base: Getter::find(
                class("MaterialInterface")?,
                "GetBaseMaterial",
                "ObjectProperty",
            )?,
            parameters,
        })
    }

    /// For each of `brick`'s material slots, the tyre it shows if it shows one; empty if none
    /// does.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn slots(&self, brick: *mut UObjectBase) -> Vec<Option<Tyre>> {
        // SAFETY: per the contract; the root component and its materials are live objects (or
        // null).
        unsafe {
            let root = self.root.call(brick);
            if root.is_null() {
                return Vec::new();
            }
            let slots: Vec<Option<Tyre>> = (0..self.components.material_count(root))
                .map(|index| {
                    let material = self.components.material(root, index);
                    if material.is_null() {
                        return None;
                    }
                    let base = self.base.call(material);
                    if base.is_null() || !reflection::name_is(&(*base).name_private, TYRE_MATERIAL)
                    {
                        return None;
                    }
                    let texture = |name| match &self.parameters {
                        Some(parameters) => parameters.texture(material, name),
                        None => core::ptr::null_mut(),
                    };
                    Some(Tyre {
                        normal: texture(NORMAL_MAP),
                        occlusion: texture(OCCLUSION_MAP),
                    })
                })
                .collect();
            if slots.iter().any(Option::is_some) {
                slots
            } else {
                Vec::new()
            }
        }
    }

    /// The rubber texture tyres show, if it's loaded (it is once a tyre is).
    pub fn rubber() -> Option<*mut UObjectBase> {
        reflection::find_object(RUBBER, "Texture2D")
    }
}
