//! Hiding actors from one player's view: `APlayerController::HiddenActors` (reflected, a
//! `TArray<AActor*>`). Each frame the engine adds their components to that player's hidden set
//! (`BuildHiddenComponentList`, before the game's `UpdateHiddenComponents`, see
//! `game::hidden_components`). Unlike `AActor::bHidden` (`SetActorHiddenInGame`), the list isn't
//! replicated, so it changes nothing for other players, even when hosting. The game only empties
//! the list next to it, `HiddenPrimitiveComponents` (`UpdateHiddenActors`, game 1.11.7 0xE48BC0,
//! the controller's +0x300; `GetEditorTraceParams` reads that one too, see `game::editor_trace`).
//! Destroyed actors' entries are nulled by the garbage collector and dropped by the engine.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;

use crate::game::reflection;

pub struct HiddenActors {
    /// Offset of `HiddenActors` in a player controller.
    offset: usize,
}

/// Finds the list. After engine init.
pub fn find() -> Result<HiddenActors, String> {
    let controller =
        reflection::find_class("PlayerController").ok_or("class PlayerController not found")?;
    Ok(HiddenActors {
        offset: reflection::field_offset(controller, "HiddenActors", "ArrayProperty", 16)?,
    })
}

impl HiddenActors {
    /// The controller's list.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    unsafe fn list<'a>(&self, controller: *mut UObjectBase) -> &'a mut TArray<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            &mut *controller
                .cast::<u8>()
                .add(self.offset)
                .cast::<TArray<*mut UObjectBase>>()
        }
    }

    /// Whether `actor` is in the controller's list.
    ///
    /// # Safety
    /// As for [`Self::list`].
    pub unsafe fn contains(&self, controller: *mut UObjectBase, actor: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.list(controller).iter().any(|listed| listed == actor) }
    }

    /// Hides `actor` from the controller's view, unless it's in the list already.
    ///
    /// # Safety
    /// As for [`Self::list`]; `actor` must be a live actor.
    pub unsafe fn add(&self, controller: *mut UObjectBase, actor: *mut UObjectBase) {
        // SAFETY: per the contract; `push` grows the array with the game's allocator, like
        // `TArray` does.
        unsafe {
            if !self.contains(controller, actor) {
                self.list(controller).push(actor);
            }
        }
    }

    /// Takes `actor` out of the controller's list (every time it's there), keeping the others in
    /// order.
    ///
    /// # Safety
    /// As for [`Self::list`].
    pub unsafe fn remove(&self, controller: *mut UObjectBase, actor: *mut UObjectBase) {
        // SAFETY: per the contract; the slice covers the array's elements, and only its count
        // shrinks, so its allocation stays the game's.
        unsafe {
            let list = self.list(controller);
            let Ok(count) = usize::try_from(list.num) else {
                return;
            };
            if count == 0 || list.data.is_null() {
                return;
            }
            let elements = core::slice::from_raw_parts_mut(list.data, count);
            let others: Vec<_> = elements
                .iter()
                .copied()
                .filter(|&listed| listed != actor)
                .collect();
            elements[..others.len()].copy_from_slice(&others);
            list.num = others.len() as i32;
        }
    }
}
