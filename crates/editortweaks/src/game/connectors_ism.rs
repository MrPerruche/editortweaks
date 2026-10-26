//! `UBrickConnectorsISMComponent`: the markers a selected scalable brick shows where its connectors
//! are (dots at 10 cm spacing, crosses at 15 cm, circles at 30 cm), gray, or yellow where hovering a
//! size slider changes them. Found through BRMK 1.11.6's exports, then in game 1.11.7 (build
//! 25831956) through its only caller; see `docs/re/connector-markers.md`.
//!
//! An instanced static mesh component, one per brick, made and filled by
//! `UBrick::UpdateEditorVisualization` (`ClearConnectorFields`, then [`ADD_CONNECTOR_FIELDS`]). Each
//! instance is a rectangle of a face's connector points: a 10 cm quad ([`MESH_CM`]) at the
//! rectangle's middle, scaled to `cells * spacing / 10` along its X and Y, with three floats of
//! custom data:
//!
//! ```text
//! [0] 30 / spacing        // 1, 2, 3: which marker (circles, crosses, dots)
//! [1] focused             // 0 or 1: the material's Color or FocusedColor
//! [2] marker size         // up to 1, smaller on bricks smaller than a cell
//! ```
//!
//! The material (`M_ScalableConnector`) tiles its markers over an instance by `UV * ObjectScale.xy *
//! Tiling (1/3) * [0]`: one per cell, from the instance's scale. Its color is `Lerp(Color,
//! FocusedColor, [1])`, two vector parameters the component sets on its dynamic material instance
//! (`MID`) from its UI styles.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::{Transform, Vector};
use crate::game::reflection::{self, Function, Getter, Parameters, put, take};

/// `void UBrickConnectorsISMComponent::AddConnectorFields(const TArray<FConnectorField>& Fields,
/// uint8 FocusedAxisFlags, const FVector& BrickSize)` (BRMK export 0x1354D0, game RVA 0xD2AB10):
/// adds the instances of `Fields`' connector points, merged into rectangles (see the module
/// documentation), with `AddInstances` and then `SetCustomData` for each.
pub const ADD_CONNECTOR_FIELDS: FunctionSig = FunctionSig {
    name: "UBrickConnectorsISMComponent::AddConnectorFields",
    // A field's "merge into rectangles" flag (`FConnectorField` +0x25) read, the marker kind's
    // `* 30` (constant wildcarded), then the custom data being stored (stack offsets wildcarded).
    pattern: "41 0f b6 45 25 49 8b ff f3 0f 59 3d ?? ?? ?? ?? 41 8b f7 4c 89 7c 24 ??",
};

pub type AddConnectorFieldsFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    fields: *const c_void,
    focused_axes: u8,
    brick_size: *const Vector,
);

/// The marker mesh's side, in cm: an instance `cells * spacing` wide has a scale of that over this.
pub const MESH_CM: f64 = 10.0;
/// The spacing (cm) whose marker kind (custom data [0]) is 1: the kind is this over the spacing.
pub const KIND_SPACING_CM: f64 = 30.0;
/// Custom data floats per instance.
pub const CUSTOM_FLOATS: usize = 3;

/// One marker instance: where it is in its component, and its custom data.
#[derive(Clone, Copy)]
pub struct Instance {
    pub transform: Transform,
    pub data: [f32; CUSTOM_FLOATS],
}

/// `bool GetInstanceTransform(int32 InstanceIndex, FTransform& OutInstanceTransform, bool
/// bWorldSpace) const`.
struct GetTransform {
    function: Function,
    index: usize,
    transform: usize,
    result: usize,
}

/// `int32 AddInstance(const FTransform& InstanceTransform)`.
struct AddInstance {
    function: Function,
    transform: usize,
    result: usize,
}

/// `bool SetCustomDataValue(int32 InstanceIndex, int32 CustomDataIndex, float CustomDataValue,
/// bool bMarkRenderStateDirty)`.
struct SetCustomData {
    function: Function,
    instance: usize,
    index: usize,
    value: usize,
    dirty: usize,
}

/// Reads and refills connector marker components, through the reflection of
/// `UInstancedStaticMeshComponent` (after engine init).
pub struct ConnectorsIsm {
    /// `BrickConnectorsISMComponent`.
    class: *const UClass,
    /// `int32 GetInstanceCount() const`.
    count: Getter<i32>,
    get_transform: GetTransform,
    /// `void ClearInstances()`.
    clear: Function,
    add: AddInstance,
    set_data: SetCustomData,
    /// `USceneComponent* USceneComponent::GetAttachParent() const`.
    parent: Getter<*mut UObjectBase>,
    /// Offsets of `PerInstanceSMCustomData` (`TArray<float>`, `NumCustomDataFloats` per
    /// instance), `NumCustomDataFloats` (`int32`) and the component's own `MID`.
    custom_data: usize,
    floats: usize,
    mid: usize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ConnectorsIsm {}
unsafe impl Sync for ConnectorsIsm {}

impl ConnectorsIsm {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let connectors = class("BrickConnectorsISMComponent")?;
        let ism = class("InstancedStaticMeshComponent")?;
        let scene = class("SceneComponent")?;
        let transform_size = size_of::<Transform>() as i32;

        let function = Function::find(ism, "GetInstanceTransform")?;
        // `bWorldSpace` stays false: transforms are the component's own.
        function.parameter("bWorldSpace", "BoolProperty", 1)?;
        let get_transform = GetTransform {
            index: function.parameter("InstanceIndex", "IntProperty", 4)?,
            transform: function.parameter(
                "OutInstanceTransform",
                "StructProperty",
                transform_size,
            )?,
            result: function.parameter("ReturnValue", "BoolProperty", 1)?,
            function,
        };
        let function = Function::find(ism, "AddInstance")?;
        let add = AddInstance {
            transform: function.parameter("InstanceTransform", "StructProperty", transform_size)?,
            result: function.parameter("ReturnValue", "IntProperty", 4)?,
            function,
        };
        let function = Function::find(ism, "SetCustomDataValue")?;
        let set_data = SetCustomData {
            instance: function.parameter("InstanceIndex", "IntProperty", 4)?,
            index: function.parameter("CustomDataIndex", "IntProperty", 4)?,
            value: function.parameter("CustomDataValue", "FloatProperty", 4)?,
            dirty: function.parameter("bMarkRenderStateDirty", "BoolProperty", 1)?,
            function,
        };
        Ok(Self {
            class: connectors,
            count: Getter::find(ism, "GetInstanceCount", "IntProperty")?,
            get_transform,
            clear: Function::find(ism, "ClearInstances")?,
            add,
            set_data,
            parent: Getter::find(scene, "GetAttachParent", "ObjectProperty")?,
            custom_data: reflection::field_offset(
                ism,
                "PerInstanceSMCustomData",
                "ArrayProperty",
                size_of::<TArray<f32>>() as i32,
            )?,
            floats: reflection::field_offset(ism, "NumCustomDataFloats", "IntProperty", 4)?,
            mid: reflection::field_offset(connectors, "MID", "ObjectProperty", 8)?,
        })
    }

    /// Whether `object` is a connector marker component.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_connectors(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.class.cast()) }
    }

    /// Whether `component` is attached to something. The game's marker components are, to their
    /// brick's root, while they show: when a brick stops showing its markers, the editor detaches
    /// its component, unregisters it and keeps it for another brick, rather than destroying it
    /// (`UBrickEditorObject::RemoveBrickEditorComponentInternal`).
    ///
    /// # Safety
    /// `component` must be a live connector marker component. Game thread only.
    pub unsafe fn is_attached(&self, component: *mut UObjectBase) -> bool {
        // SAFETY: per the contract; the function has no parameters.
        !unsafe { self.parent.call(component) }.is_null()
    }

    /// Every instance of `component`, or `None` if they can't be read as expected (another number
    /// of custom data floats, a transform the engine didn't give).
    ///
    /// # Safety
    /// `component` must be a live connector marker component. Game thread only.
    pub unsafe fn instances(&self, component: *mut UObjectBase) -> Option<Vec<Instance>> {
        // SAFETY: per the contract; the offsets come from reflection, and the custom data array is
        // checked to hold `CUSTOM_FLOATS` floats per instance before it's read.
        unsafe {
            let base = component.cast::<u8>();
            if base.add(self.floats).cast::<i32>().read() != CUSTOM_FLOATS as i32 {
                return None;
            }
            let count = usize::try_from(self.count.call(component)).ok()?;
            let data = base.add(self.custom_data).cast::<TArray<f32>>().read();
            if usize::try_from(data.num).ok()? != count * CUSTOM_FLOATS
                || (count > 0 && data.data.is_null())
            {
                return None;
            }
            let data = if count > 0 {
                core::slice::from_raw_parts(data.data, count * CUSTOM_FLOATS).to_vec()
            } else {
                Vec::new()
            };
            let call = &self.get_transform;
            let mut instances = Vec::with_capacity(count);
            for (index, &data) in data.as_chunks::<CUSTOM_FLOATS>().0.iter().enumerate() {
                let mut parameters = Parameters::new();
                put(&mut parameters, call.index, index as i32);
                call.function.call(component, &mut parameters);
                if !take::<bool>(&parameters, call.result) {
                    return None;
                }
                instances.push(Instance {
                    transform: take(&parameters, call.transform),
                    data,
                });
            }
            Some(instances)
        }
    }

    /// Removes every instance of `component`.
    ///
    /// # Safety
    /// `component` must be a live connector marker component. Game thread only.
    pub unsafe fn clear(&self, component: *mut UObjectBase) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the function has no parameters.
        unsafe { self.clear.call(component, &mut parameters) };
    }

    /// Adds `instances` to `component`, with their custom data. Whether it could (the component
    /// keeps `CUSTOM_FLOATS` floats per instance).
    ///
    /// # Safety
    /// `component` must be a live connector marker component. Game thread only.
    pub unsafe fn add(&self, component: *mut UObjectBase, instances: &[Instance]) -> bool {
        // SAFETY: per the contract; each call gets its parameters at their reflected offsets.
        unsafe {
            if component.cast::<u8>().add(self.floats).cast::<i32>().read() != CUSTOM_FLOATS as i32
            {
                return false;
            }
            let last = instances.len().saturating_sub(1);
            for (number, instance) in instances.iter().enumerate() {
                let mut parameters = Parameters::new();
                put(&mut parameters, self.add.transform, instance.transform);
                self.add.function.call(component, &mut parameters);
                let index: i32 = take(&parameters, self.add.result);
                let call = &self.set_data;
                for (slot, &value) in instance.data.iter().enumerate() {
                    let mut parameters = Parameters::new();
                    put(&mut parameters, call.instance, index);
                    put(&mut parameters, call.index, slot as i32);
                    put(&mut parameters, call.value, value);
                    // Once, at the end: the render state is rebuilt from all of it.
                    put(
                        &mut parameters,
                        call.dirty,
                        number == last && slot == CUSTOM_FLOATS - 1,
                    );
                    call.function.call(component, &mut parameters);
                }
            }
            true
        }
    }

    /// The dynamic material instance `component` sets its colors on (its `MID`), or null if it made
    /// none.
    ///
    /// # Safety
    /// `component` must be a live connector marker component.
    pub unsafe fn material(&self, component: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            component
                .cast::<u8>()
                .add(self.mid)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }
}
