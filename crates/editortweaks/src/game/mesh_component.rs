//! Extra meshes in the 3D view: `UStaticMeshComponent`s added to an actor at run time
//! (`AActor::AddComponentByClass`) and attached to another component (`K2_AttachToComponent`), so
//! they follow it; without collision. All through reflection; found by name in game 1.11.7 (build
//! 25831956), see `docs/re/collision-shapes.md`.
//!
//! They can copy what another component's materials read (its custom primitive data) and its
//! selection outline (custom depth), so a brick's own material renders on them as on the brick.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UClass;

use crate::game::editor_object::{Transform, Vector};
use crate::game::reflection::{self, BoolField, Function, Getter, Parameters, put, take};
use crate::prelude::log;

/// `ECollisionEnabled::NoCollision`.
const NO_COLLISION: u8 = 0;

/// `AActor::AddComponentByClass(TSubclassOf<UActorComponent> Class, bool bManualAttachment, const
/// FTransform& RelativeTransform, bool bDeferredFinish)`: makes, places and registers a component.
struct AddComponent {
    function: Function,
    class: usize,
    manual: usize,
    transform: usize,
    result: usize,
}

/// A function taking an index and an object or value, e.g. `SetMaterial(int32 ElementIndex,
/// UMaterialInterface* Material)`.
struct IndexedCall {
    function: Function,
    index: usize,
    value: usize,
}

/// A function taking one parameter.
struct SingleCall {
    function: Function,
    value: usize,
}

impl SingleCall {
    fn find(
        class: *const UClass,
        name: &'static str,
        parameter: &'static str,
        kind: &'static str,
        size: i32,
    ) -> Result<Self, String> {
        let function = Function::find(class, name)?;
        Ok(Self {
            value: function.parameter(parameter, kind, size)?,
            function,
        })
    }

    /// Calls it on `object` with `value`.
    ///
    /// # Safety
    /// `object` must be a live object whose class has the function, and `T` the parameter's type.
    /// Game thread only.
    unsafe fn call<T>(&self, object: *mut UObjectBase, value: T) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract.
        unsafe {
            put(&mut parameters, self.value, value);
            self.function.call(object, &mut parameters);
        }
    }
}

/// Makes and changes mesh components, and reads the components they copy (after engine init).
pub struct MeshComponents {
    /// `StaticMeshComponent`.
    class: *const UClass,
    /// `AActor* UActorComponent::GetOwner() const`.
    owner: Getter<*mut UObjectBase>,
    add_component: AddComponent,
    /// `USceneComponent::K2_AttachToComponent(USceneComponent* Parent, FName SocketName, ...)`:
    /// the attachment rules left at 0 keep the relative transform.
    attach: SingleCall,
    /// `USceneComponent::SetRelativeScale3D(FVector NewScale3D)`.
    set_scale: SingleCall,
    /// `bool UStaticMeshComponent::SetStaticMesh(UStaticMesh* NewMesh)`.
    set_mesh: SingleCall,
    /// `UPrimitiveComponent::SetCollisionEnabled(ECollisionEnabled::Type NewType)`.
    set_collision: SingleCall,
    /// `UActorComponent::K2_DestroyComponent(UObject* Object)`: only destroys if `Object` is the
    /// component's owner.
    destroy: SingleCall,
    /// `UPrimitiveComponent::SetMaterial(int32 ElementIndex, UMaterialInterface* Material)`.
    set_material: IndexedCall,
    /// `UMaterialInterface* UPrimitiveComponent::GetMaterial(int32 ElementIndex) const`.
    get_material: IndexedCall,
    /// `int32 UPrimitiveComponent::GetNumMaterials() const`.
    material_count: Getter<i32>,
    /// `UPrimitiveComponent::SetCustomPrimitiveDataVector4(int32 DataIndex, FVector4 Value)`.
    set_data: IndexedCall,
    /// Offset of `UPrimitiveComponent::CustomPrimitiveData.Data` (a `TArray<float>`).
    custom_data: AtomicUsize,
    /// Whether `custom_data` was checked against a live component.
    checked: Once,
    /// `UPrimitiveComponent::bRenderCustomDepth` and `CustomDepthStencilValue` (`int32`): the
    /// editor's selection outline.
    custom_depth: BoolField,
    stencil: usize,
    /// `SetRenderCustomDepth(bool bValue)`, `SetCustomDepthStencilValue(int32 Value)`.
    set_custom_depth: SingleCall,
    set_stencil: SingleCall,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for MeshComponents {}
unsafe impl Sync for MeshComponents {}

impl MeshComponents {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let mesh = class("StaticMeshComponent")?;
        let actor = class("Actor")?;
        let component = class("ActorComponent")?;
        let scene = class("SceneComponent")?;
        let primitive = class("PrimitiveComponent")?;

        let function = Function::find(actor, "AddComponentByClass")?;
        let add_component = AddComponent {
            class: function.parameter("Class", "ClassProperty", 8)?,
            manual: function.parameter("bManualAttachment", "BoolProperty", 1)?,
            transform: function.parameter("RelativeTransform", "StructProperty", 0x30)?,
            result: function.parameter("ReturnValue", "ObjectProperty", 8)?,
            function,
        };
        // `bDeferredFinish` stays false: the component is registered right away.
        add_component
            .function
            .parameter("bDeferredFinish", "BoolProperty", 1)?;

        let indexed = |name, index, value, kind, size| -> Result<IndexedCall, String> {
            let function = Function::find(primitive, name)?;
            Ok(IndexedCall {
                index: function.parameter(index, "IntProperty", 4)?,
                value: function.parameter(value, kind, size)?,
                function,
            })
        };
        let custom_data = {
            let property =
                reflection::field(primitive, "CustomPrimitiveData", "StructProperty", 16)?;
            // SAFETY: a live StructProperty record.
            let (offset, data) = unsafe {
                (
                    (*property).offset_internal as usize,
                    reflection::struct_type(property),
                )
            };
            offset + reflection::struct_field_offset(data, "Data", "ArrayProperty", 16)?
        };
        // `TEnumAsByte<ECollisionEnabled::Type>`: a byte property, or an enum one.
        let set_collision = SingleCall::find(
            primitive,
            "SetCollisionEnabled",
            "NewType",
            "ByteProperty",
            1,
        )
        .or_else(|_| {
            SingleCall::find(
                primitive,
                "SetCollisionEnabled",
                "NewType",
                "EnumProperty",
                1,
            )
        })?;
        Ok(Self {
            class: mesh,
            owner: Getter::find(component, "GetOwner", "ObjectProperty")?,
            add_component,
            attach: SingleCall::find(scene, "K2_AttachToComponent", "Parent", "ObjectProperty", 8)?,
            set_scale: SingleCall::find(
                scene,
                "SetRelativeScale3D",
                "NewScale3D",
                "StructProperty",
                12,
            )?,
            set_mesh: SingleCall::find(mesh, "SetStaticMesh", "NewMesh", "ObjectProperty", 8)?,
            set_collision,
            destroy: SingleCall::find(
                component,
                "K2_DestroyComponent",
                "Object",
                "ObjectProperty",
                8,
            )?,
            set_material: indexed(
                "SetMaterial",
                "ElementIndex",
                "Material",
                "ObjectProperty",
                8,
            )?,
            get_material: indexed(
                "GetMaterial",
                "ElementIndex",
                "ReturnValue",
                "ObjectProperty",
                8,
            )?,
            material_count: Getter::find(primitive, "GetNumMaterials", "IntProperty")?,
            set_data: indexed(
                "SetCustomPrimitiveDataVector4",
                "DataIndex",
                "Value",
                "StructProperty",
                16,
            )?,
            custom_data: AtomicUsize::new(custom_data),
            checked: Once::new(),
            custom_depth: reflection::bool_field(primitive.cast(), "bRenderCustomDepth")?,
            stencil: reflection::field_offset(
                primitive,
                "CustomDepthStencilValue",
                "IntProperty",
                4,
            )?,
            set_custom_depth: SingleCall::find(
                primitive,
                "SetRenderCustomDepth",
                "bValue",
                "BoolProperty",
                1,
            )?,
            set_stencil: SingleCall::find(
                primitive,
                "SetCustomDepthStencilValue",
                "Value",
                "IntProperty",
                4,
            )?,
        })
    }

    /// A new mesh component showing `mesh`, attached to `parent` (a scene component) without
    /// offset, scaled by `scale`, owned by `parent`'s actor, without collision; `None` if the engine
    /// made none.
    ///
    /// # Safety
    /// `parent` must be a live scene component and `mesh` a live static mesh. Game thread only.
    pub unsafe fn create(
        &self,
        parent: *mut UObjectBase,
        mesh: *mut UObjectBase,
        scale: Vector,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; each call gets its parameters at their reflected offsets.
        unsafe {
            let actor = self.owner.call(parent);
            if actor.is_null() {
                return None;
            }
            let call = &self.add_component;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.class, self.class);
            // Attached below, to `parent` rather than to the actor's root.
            put(&mut parameters, call.manual, true);
            let [x, y, z] = scale;
            put(
                &mut parameters,
                call.transform,
                Transform {
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    translation: [0.0; 4],
                    scale: [x, y, z, 0.0],
                },
            );
            call.function.call(actor, &mut parameters);
            let component = take::<*mut UObjectBase>(&parameters, call.result);
            if component.is_null() {
                return None;
            }
            self.set_collision.call(component, NO_COLLISION);
            self.attach.call(component, parent);
            self.set_mesh(component, mesh);
            self.check_custom_data(component);
            Some(component)
        }
    }

    /// Makes `component` (one [`MeshComponents::create`] made) show `mesh`.
    ///
    /// # Safety
    /// `component` must be a live static mesh component and `mesh` a live static mesh. Game thread
    /// only.
    pub unsafe fn set_mesh(&self, component: *mut UObjectBase, mesh: *mut UObjectBase) {
        // SAFETY: per the contract.
        unsafe { self.set_mesh.call(component, mesh) };
    }

    /// Sets the scale of `component` relative to what it's attached to.
    ///
    /// # Safety
    /// `component` must be a live scene component. Game thread only.
    pub unsafe fn set_scale(&self, component: *mut UObjectBase, scale: Vector) {
        // SAFETY: per the contract; the parameter is an `FVector`.
        unsafe { self.set_scale.call(component, scale) };
    }

    /// Destroys `component` (one this made).
    ///
    /// # Safety
    /// `component` must be a live component. Game thread only.
    pub unsafe fn destroy(&self, component: *mut UObjectBase) {
        // SAFETY: per the contract.
        unsafe { self.destroy.call(component, self.owner.call(component)) };
    }

    /// The number of material slots of `component` (a primitive component).
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn material_count(&self, component: *mut UObjectBase) -> usize {
        // SAFETY: per the contract.
        usize::try_from(unsafe { self.material_count.call(component) }).unwrap_or(0)
    }

    /// The material `component` renders slot `index` with (its override, else its mesh's), or null.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn material(&self, component: *mut UObjectBase, index: usize) -> *mut UObjectBase {
        let call = &self.get_material;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the return value is an object pointer.
        unsafe {
            put(&mut parameters, call.index, index as i32);
            call.function.call(component, &mut parameters);
            take(&parameters, call.value)
        }
    }

    /// Makes `component` render slot `index` with `material`.
    ///
    /// # Safety
    /// `component` must be a live primitive component and `material` null or a live material.
    /// Game thread only.
    pub unsafe fn set_material(
        &self,
        component: *mut UObjectBase,
        index: usize,
        material: *mut UObjectBase,
    ) {
        let call = &self.set_material;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract.
        unsafe {
            put(&mut parameters, call.index, index as i32);
            put(&mut parameters, call.value, material);
            call.function.call(component, &mut parameters);
        }
    }

    /// Checks the offset of the custom primitive data array against `component` (once): the
    /// reflected one was 0x10 too low in game 1.11.7, so this writes a probe value to unused slots
    /// 28 to 31, looks for the array holding it, and uses that offset if it finds exactly one. The
    /// probe is then set back to 0.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    unsafe fn check_custom_data(&self, component: *mut UObjectBase) {
        self.checked.call_once(|| {
            let mut parameters = Parameters::new();
            // SAFETY: per the contract; the scan reads 8-byte aligned words within the
            // component's class size, and an array only where its pointer is non-null and counts
            // are plausible.
            unsafe {
                put(&mut parameters, self.set_data.index, 28i32);
                put(
                    &mut parameters,
                    self.set_data.value,
                    [1.0f32, 2.0, 3.0, 4.0],
                );
                self.set_data.function.call(component, &mut parameters);
                let size =
                    usize::try_from((*reflection::class_of(component.cast())).properties_size)
                        .unwrap_or(0);
                let mut found = Vec::new();
                for offset in (0..size.saturating_sub(16)).step_by(8) {
                    let array = component
                        .cast::<u8>()
                        .add(offset)
                        .cast::<TArray<f32>>()
                        .read();
                    if array.data.is_null()
                        || !(32..=64).contains(&array.num)
                        || array.max < array.num
                        || !(array.data as usize).is_multiple_of(4)
                    {
                        continue;
                    }
                    let values = core::slice::from_raw_parts(array.data.add(28), 4);
                    if values == [1.0, 2.0, 3.0, 4.0] {
                        found.push(offset);
                    }
                }
                let reflected = self.custom_data.load(Ordering::Relaxed);
                match found[..] {
                    [offset] => {
                        self.custom_data.store(offset, Ordering::Relaxed);
                        log!("custom primitive data at {offset:#x} (reflected: {reflected:#x})");
                    }
                    _ => log!(
                        "custom primitive data: reflected offset {reflected:#x}, found {found:x?}"
                    ),
                }
                put(&mut parameters, self.set_data.value, [0.0f32; 4]);
                self.set_data.function.call(component, &mut parameters);
            }
        });
    }

    /// The custom primitive data of `component` (a primitive component), for the log.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn custom_values(&self, component: *mut UObjectBase) -> Vec<f32> {
        // SAFETY: per the contract; the offset comes from reflection, and the array holds `num`
        // floats.
        unsafe {
            let data = component
                .cast::<u8>()
                .add(self.custom_data.load(Ordering::Relaxed))
                .cast::<TArray<f32>>()
                .read();
            match usize::try_from(data.num) {
                Ok(count) if !data.data.is_null() => {
                    core::slice::from_raw_parts(data.data, count).to_vec()
                }
                _ => Vec::new(),
            }
        }
    }

    /// Gives `to` the custom primitive data of `from` (both primitive components), which their
    /// materials read (a brick's color, pattern, and its place while being moved); slots `to` has
    /// beyond `from`'s are set to 0.
    ///
    /// # Safety
    /// Both must be live primitive components. Game thread only.
    pub unsafe fn copy_custom_data(&self, from: *mut UObjectBase, to: *mut UObjectBase) {
        // SAFETY: per the contract; the offset comes from reflection (checked in game), and the
        // values are copied before anything is set, which may reallocate `to`'s array.
        unsafe {
            let mut values = self.custom_values(from);
            let extra = self.custom_values(to).len();
            if values.len() < extra {
                values.resize(extra, 0.0);
            }
            if values != self.custom_values(to) {
                self.set_custom_values(to, &values);
            }
        }
    }

    /// Sets the custom primitive data of `component` from slot 0 on, four slots at a time (the
    /// last ones padded with 0).
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    unsafe fn set_custom_values(&self, component: *mut UObjectBase, values: &[f32]) {
        let call = &self.set_data;
        for (chunk, values) in values.chunks(4).enumerate() {
            let mut value = [0.0f32; 4];
            value[..values.len()].copy_from_slice(values);
            let mut parameters = Parameters::new();
            // SAFETY: per the contract; the parameters are an `int32` and an `FVector4`.
            unsafe {
                put(&mut parameters, call.index, (chunk * 4) as i32);
                put(&mut parameters, call.value, value);
                call.function.call(component, &mut parameters);
            }
        }
    }

    /// Gives `to` the custom depth settings of `from` (both primitive components), if they differ:
    /// the editor outlines selected and hovered bricks through them.
    ///
    /// # Safety
    /// Both must be live primitive components. Game thread only.
    pub unsafe fn copy_custom_depth(&self, from: *mut UObjectBase, to: *mut UObjectBase) {
        // SAFETY: per the contract; the fields' offsets come from reflection.
        unsafe {
            let (from_base, to_base) = (from.cast::<u8>(), to.cast::<u8>());
            let stencil = from_base.add(self.stencil).cast::<i32>().read();
            if to_base.add(self.stencil).cast::<i32>().read() != stencil {
                self.set_stencil.call(to, stencil);
            }
            let custom_depth = self.custom_depth.get(from_base);
            if self.custom_depth.get(to_base) != custom_depth {
                self.set_custom_depth.call(to, custom_depth);
            }
        }
    }
}
