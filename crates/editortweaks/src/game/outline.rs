//! The editor's selection outline: a post-process material (`ABrickEditor::OutlineMID`, made from
//! `M_EditorOutline`) outlines what renders to custom depth, as its stencil value says. Each editor
//! object's root component renders there when the object is selected or hovered, with a stencil
//! value the game works out from the object's state (`UBrickEditorObject::UpdateDesiredCustomDepth`,
//! BRMK 0xF71B0, game 0xD0F780, called by `UpdateEditorVisualization` whenever the state changes).
//! Found in BRMK 1.11.6, then in game 1.11.7 (build 25831956); see `docs/re/selection.md`.
//!
//! Everything here is reflected but the stencil value's layout (the game's code):
//!
//! ```text
//! bit 0      hovered
//! bits 1-3   the EBrickOutlineState, plus one
//! bits 4-7   a color index from the object's group (one in 15), so neighbours outline apart
//! ```
//!
//! `EBrickOutlineState` (not reflected; named after the material's parameters): `Invalid` 0,
//! `Neutral` 1, `Valid` 2 (bricks a property picker offers), `Active` 3 (the primary selected
//! brick), `Selected` 4, `Unselected` 5. Each has its color in the material (`SelectedColor`, ...),
//! from the UI style. The material's code (BRMK's `M_EditorOutline_Master`, a custom node) switches
//! on the bits 1-3 themselves, its "case": 1 `ValidColor`, 2 `InvalidColor`, 3 `NeutralColor`, 4
//! `ActiveColor`, 5 `SelectedColor`, else `UnselectedColor` (so the first three names above may be
//! off by their order; only cases 1-3 show validity, while moving bricks or picking a property).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection::{self, BoolField, Getter, SingleCall};

/// The stencil value's bits holding the outline state.
const STATE_BITS: i32 = 0b1110;
/// The stencil value's bit set while the object is hovered.
const HOVERED_BIT: i32 = 0b1;

/// `EBrickOutlineState`, as far as selecting goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutlineState {
    Selected = 4,
    Unselected = 5,
}

/// How a component renders to custom depth: what the outline shows of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Look {
    /// `bRenderCustomDepth`: outlined at all.
    pub outlined: bool,
    /// `CustomDepthStencilValue`.
    pub stencil: i32,
}

impl Look {
    /// The material's case for this look (see the module's doc).
    pub fn case(self) -> i32 {
        (self.stencil & STATE_BITS) >> 1
    }

    /// This look outlined in the material's `case`, its hover and group kept.
    pub fn in_case(self, case: i32) -> Self {
        Self {
            outlined: true,
            stencil: (self.stencil & !STATE_BITS) | ((case << 1) & STATE_BITS),
        }
    }

    /// This look for an object in `state`, its hover and group kept. An unselected object is
    /// outlined only while hovered (as the game does in the usual view modes).
    pub fn in_state(self, state: OutlineState) -> Self {
        let stencil = (self.stencil & !STATE_BITS) | ((state as i32 + 1) << 1);
        let outlined = match state {
            OutlineState::Selected => true,
            OutlineState::Unselected => self.stencil & HOVERED_BIT != 0,
        };
        Self { outlined, stencil }
    }
}

/// Reads and changes how editor objects are outlined (after engine init).
pub struct Outlines {
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`: the component outlined.
    root: Getter<*mut UObjectBase>,
    /// `UPrimitiveComponent::bRenderCustomDepth` and `CustomDepthStencilValue` (`int32`).
    outlined: BoolField,
    stencil: usize,
    /// `SetRenderCustomDepth(bool bValue)`, `SetCustomDepthStencilValue(int32 Value)`: change them
    /// and update the component's rendering.
    set_outlined: SingleCall,
    set_stencil: SingleCall,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Outlines {}
unsafe impl Sync for Outlines {}

pub fn find() -> Result<Outlines, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let primitive = class("PrimitiveComponent")?;
    Ok(Outlines {
        root: Getter::find(
            class("BrickEditorObject")?,
            "GetRootComponent",
            "ObjectProperty",
        )?,
        outlined: reflection::bool_field(primitive.cast(), "bRenderCustomDepth")?,
        stencil: reflection::field_offset(primitive, "CustomDepthStencilValue", "IntProperty", 4)?,
        set_outlined: SingleCall::find(
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

impl Outlines {
    /// The component the outline shows of `object` (an editor object), if any.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn component_of(&self, object: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract.
        let component = unsafe { self.root.call(object) };
        (!component.is_null()).then_some(component)
    }

    /// How `component` renders to custom depth now.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn look(&self, component: *mut UObjectBase) -> Look {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let base = component.cast::<u8>();
            Look {
                outlined: self.outlined.get(base),
                stencil: base.add(self.stencil).cast::<i32>().read(),
            }
        }
    }

    /// Gives `component` the look `look`, changing only what differs.
    ///
    /// # Safety
    /// As for [`Self::look`].
    pub unsafe fn set_look(&self, component: *mut UObjectBase, look: Look) {
        // SAFETY: per the contract.
        unsafe {
            let now = self.look(component);
            if now.stencil != look.stencil {
                self.set_stencil.call(component, look.stencil);
            }
            if now.outlined != look.outlined {
                self.set_outlined.call(component, look.outlined);
            }
        }
    }
}
