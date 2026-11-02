//! Floating text in the 3D view, for what the line batcher can't draw: one `UTextRenderComponent`
//! per text, added to the vehicle editor's own actor and driven through reflected functions only.
//! Used by coop to name the owner of each camera marker.
//!
//! The component belongs to the editor's actor because the vehicle editor hides every primitive
//! component of every other actor each frame (`docs/re/map-in-editor.md`): a separate
//! `ATextRenderActor` is never seen. Components of the editor's own actor stay visible, as
//! `game::mesh_component` relies on. Found by reading UE 4.27's engine source (nothing of it is
//! Brick Rigs' own), so every name below is looked up by reflection when [`find`] runs and its
//! absence disables the labels only; see `docs/re/coop/camera-labels.md`:
//! - `AActor::AddComponentByClass(Class, bManualAttachment, RelativeTransform, bDeferredFinish)`:
//!   makes and registers the component. It exists on this machine only: nothing replicates it.
//! - `UTextRenderComponent::K2_SetText`, `SetTextRenderColor`, `SetWorldSize`,
//!   `SetHorizontalAlignment` and `SetVerticalAlignment`, plus `UPrimitiveComponent::
//!   SetCollisionEnabled` and `SetCastShadow` (best effort: a label stays out of the editor's
//!   traces and casts no shadow).
//! - `USceneComponent::K2_SetWorldLocationAndRotation` to move it, `UActorComponent::
//!   K2_DestroyComponent` (with `GetOwner`) to remove it.
//!
//! A text render component's text faces its X axis and reads along +Y, with +Z up, so a label
//! faces a viewer when its X axis points at them ([`facing`]).
//!
//! Labels are held by weak object references, so one that the engine destroyed with its owner
//! just stops being there.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::game::editor_object::{Transform, Vector};
use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Getter, Parameters, SingleCall, put, take};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;

/// A made label: its text component.
#[derive(Clone, Copy)]
pub struct TextLabel {
    component: WeakObject,
}

impl TextLabel {
    /// Whether the label still exists (its owner may have gone). Game thread only.
    pub fn is_alive(&self) -> bool {
        self.component.get_live().is_some()
    }
}

/// What makes and drives text labels (after engine init).
pub struct TextActors {
    texts: Texts,
    /// `TextRenderComponent`, the class made.
    component_class: *const UClass,
    /// `AActor::AddComponentByClass`.
    add: Function,
    add_class: usize,
    add_manual: usize,
    add_transform: usize,
    add_result: usize,
    /// `AActor* UActorComponent::GetOwner() const`.
    owner: Getter<*mut UObjectBase>,
    /// `UActorComponent::K2_DestroyComponent(UObject* Object)`: only destroys if `Object` is the
    /// component's owner.
    destroy: SingleCall,
    /// `USceneComponent::K2_SetWorldLocationAndRotation(FVector NewLocation, FRotator
    /// NewRotation, bool bSweep, FHitResult& SweepHitResult, bool bTeleport)`.
    place: Function,
    place_location: usize,
    place_rotation: usize,
    place_teleport: usize,
    set_text: Function,
    text_value: usize,
    set_color: SingleCall,
    set_size: SingleCall,
    set_horizontal: SingleCall,
    set_vertical: SingleCall,
    no_collision: Option<SingleCall>,
    no_shadow: Option<SingleCall>,
    /// `EHTA_Center` and `EVRTA_TextBottom`.
    centered: u8,
    bottom: u8,
}

// SAFETY: reflection records and a class living as long as the game; only used on the game
// thread.
unsafe impl Send for TextActors {}
unsafe impl Sync for TextActors {}

const BYTE_KINDS: &[&str] = &["ByteProperty", "EnumProperty"];

fn class(name: &'static str) -> Result<*const UClass, String> {
    reflection::find_class(name).ok_or(format!("class {name} not found"))
}

/// The value of the enum entry `entry` of `name`, `fallback` (its value in UE 4.27) if the enum
/// isn't reflected under that name.
fn enum_byte(name: &'static str, entry: &str, fallback: u8) -> u8 {
    reflection::enum_value(name, entry)
        .ok()
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(fallback)
}

/// Finds what text labels need. After engine init.
pub fn find() -> Result<TextActors, String> {
    let actor = class("Actor")?;
    let actor_component = class("ActorComponent")?;
    let scene = class("SceneComponent")?;
    let component = class("TextRenderComponent")?;
    let primitive = class("PrimitiveComponent")?;

    let add = Function::find(actor, "AddComponentByClass")?;
    // `bDeferredFinish` stays false: the component is registered right away.
    add.parameter("bDeferredFinish", "BoolProperty", 1)?;
    let place = Function::find(scene, "K2_SetWorldLocationAndRotation")?;
    let set_text = Function::find(component, "K2_SetText")?;
    let transform = size_of::<Transform>() as i32;
    Ok(TextActors {
        texts: kismet::texts()?,
        component_class: component,
        add_class: add.parameter("Class", "ClassProperty", 8)?,
        add_manual: add.parameter("bManualAttachment", "BoolProperty", 1)?,
        add_transform: add.parameter("RelativeTransform", "StructProperty", transform)?,
        add_result: add.parameter("ReturnValue", "ObjectProperty", 8)?,
        add,
        owner: Getter::find(actor_component, "GetOwner", "ObjectProperty")?,
        destroy: SingleCall::find(
            actor_component,
            "K2_DestroyComponent",
            "Object",
            "ObjectProperty",
            8,
        )?,
        place_location: place.parameter("NewLocation", "StructProperty", 12)?,
        place_rotation: place.parameter("NewRotation", "StructProperty", 12)?,
        place_teleport: place.parameter("bTeleport", "BoolProperty", 1)?,
        place,
        text_value: set_text.only_parameter(&["TextProperty"], size_of::<Text>() as i32)?,
        set_text,
        set_color: SingleCall::find_only(component, "SetTextRenderColor", &["StructProperty"], 4)?,
        set_size: SingleCall::find_only(component, "SetWorldSize", &["FloatProperty"], 4)?,
        set_horizontal: SingleCall::find_only(component, "SetHorizontalAlignment", BYTE_KINDS, 1)?,
        set_vertical: SingleCall::find_only(component, "SetVerticalAlignment", BYTE_KINDS, 1)?,
        no_collision: SingleCall::find_only(primitive, "SetCollisionEnabled", BYTE_KINDS, 1).ok(),
        no_shadow: SingleCall::find_only(primitive, "SetCastShadow", &["BoolProperty"], 1).ok(),
        centered: enum_byte("EHorizTextAligment", "EHTA_Center", 1),
        bottom: enum_byte("EVerticalTextAligment", "EVRTA_TextBottom", 2),
    })
}

/// The rotation (pitch, yaw, roll in degrees) that makes a label at `from` face a viewer at `to`
/// (its X axis pointing at them, Z up). `None` if they're at the same place.
pub fn facing(from: Vector, to: Vector) -> Option<Vector> {
    let [dx, dy, dz] = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let across = dx.hypot(dy);
    if across < 1e-3 && dz.abs() < 1e-3 {
        return None;
    }
    Some([
        dz.atan2(across).to_degrees(),
        dy.atan2(dx).to_degrees(),
        0.0,
    ])
}

/// An `FColor` (blue, green, red, alpha) of a linear colour. The text's vertex colour is read
/// by the text material as is, as lines' colours are, so no gamma is applied.
fn color_bytes(linear: [f32; 3]) -> [u8; 4] {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    [byte(linear[2]), byte(linear[1]), byte(linear[0]), 255]
}

impl TextActors {
    /// A new label owned by `owner` (the vehicle editor's actor, whose components the editor
    /// leaves visible), reading `text` in `color` (linear), its letters `size` centimeters high,
    /// its text centered above its place. It sits at the owner's origin facing +X until
    /// [`Self::place`] moves it.
    ///
    /// # Safety
    /// `owner` must be a live actor. Game thread only.
    pub unsafe fn spawn(
        &self,
        owner: *mut UObjectBase,
        text: &str,
        color: [f32; 3],
        size: f32,
    ) -> Result<TextLabel, String> {
        // SAFETY: per the contract; every offset comes from reflection on the function's own
        // parameters, and the object read is a live one the engine just returned.
        let component = unsafe {
            let mut parameters = Parameters::new();
            put(&mut parameters, self.add_class, self.component_class);
            // Not attached to the actor's root: the label is placed in the world each tick.
            put(&mut parameters, self.add_manual, true);
            put(
                &mut parameters,
                self.add_transform,
                Transform {
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    translation: [0.0; 4],
                    scale: [1.0, 1.0, 1.0, 0.0],
                },
            );
            self.add.call(owner, &mut parameters);
            take::<*mut UObjectBase>(&parameters, self.add_result)
        };
        if component.is_null() {
            return Err("the editor's actor made no text component".into());
        }
        // SAFETY: the component is live, just made.
        let label = TextLabel {
            component: unsafe { WeakObject::new(component) },
        };
        // SAFETY: as above.
        unsafe {
            self.set_horizontal.call(component, self.centered);
            self.set_vertical.call(component, self.bottom);
            self.set_size.call(component, size);
            if let Some(call) = &self.no_collision {
                call.call(component, 0u8);
            }
            if let Some(call) = &self.no_shadow {
                call.call(component, false);
            }
            self.set_text(&label, text);
            self.set_color(&label, color);
        }
        Ok(label)
    }

    /// Makes the label read `text`. Nothing if it's gone.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_text(&self, label: &TextLabel, text: &str) {
        let Some(component) = label.component.get_live() else {
            return;
        };
        let shown = self.texts.text(text);
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; `K2_SetText` takes a `const FText&` it copies, and
        // ProcessEvent doesn't destroy the parameters, so the reference made above is still ours
        // to release (moved into the buffer and back out once).
        unsafe {
            put(&mut parameters, self.text_value, shown);
            self.set_text.call(component, &mut parameters);
            take::<Text>(&parameters, self.text_value).release();
        }
    }

    /// Colors the label's text `color` (linear red, green, blue). Nothing if it's gone.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_color(&self, label: &TextLabel, color: [f32; 3]) {
        if let Some(component) = label.component.get_live() {
            // SAFETY: per the contract; the parameter is an `FColor`, four bytes.
            unsafe { self.set_color.call(component, color_bytes(color)) };
        }
    }

    /// Moves the label to `location` in the world, turned to `rotation` (pitch, yaw, roll in
    /// degrees). Nothing if it's gone.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn place(&self, label: &TextLabel, location: Vector, rotation: Vector) {
        let Some(component) = label.component.get_live() else {
            return;
        };
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the offsets come from the function's own parameters, and the
        // others (sweep, the hit result) stay zero.
        unsafe {
            put(&mut parameters, self.place_location, location);
            put(&mut parameters, self.place_rotation, rotation);
            put(&mut parameters, self.place_teleport, true);
            self.place.call(component, &mut parameters);
        }
    }

    /// Removes the label. Nothing if it's gone.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn destroy(&self, label: &TextLabel) {
        if let Some(component) = label.component.get_live() {
            // SAFETY: per the contract; a live component, destroyed through its owner, as
            // `K2_DestroyComponent` requires.
            unsafe { self.destroy.call(component, self.owner.call(component)) };
        }
    }
}
