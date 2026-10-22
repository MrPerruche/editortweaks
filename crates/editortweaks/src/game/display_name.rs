//! What the editor calls an editor object (a brick: "Gun 2x2x2"): the reflected
//! `FText UBrickEditorObject::GetBrickEditorObjectDisplayName()` (BRMK 0xA7A20, a `UFUNCTION`),
//! called through `ProcessEvent`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::kismet::Texts;
use crate::game::reflection::{self, Function, Parameters};
use crate::game::text::Text;

/// Names editor objects (after engine init).
pub struct DisplayNames {
    /// `BrickEditorObject`.
    class: *const UStruct,
    function: Function,
    result: usize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for DisplayNames {}
unsafe impl Sync for DisplayNames {}

pub fn find() -> Result<DisplayNames, String> {
    let class =
        reflection::find_class("BrickEditorObject").ok_or("class BrickEditorObject not found")?;
    let function = Function::find(class, "GetBrickEditorObjectDisplayName")?;
    Ok(DisplayNames {
        class: class.cast(),
        result: function.parameter("ReturnValue", "TextProperty", size_of::<Text>() as i32)?,
        function,
    })
}

impl DisplayNames {
    /// What the editor calls `object`, if it's an editor object.
    ///
    /// # Safety
    /// `object` must be a live object. Game thread only.
    pub unsafe fn of(&self, texts: &Texts, object: *mut UObjectBase) -> Option<String> {
        // SAFETY: per the contract; the class is checked, and the result is a new text, ours to
        // give up (`Texts::string`).
        unsafe {
            if !reflection::is_a(object, self.class) {
                return None;
            }
            let mut parameters = Parameters::new();
            self.function.call(object, &mut parameters);
            let name = texts.string(reflection::take::<Text>(&parameters, self.result));
            (!name.is_empty()).then_some(name)
        }
    }
}
