//! Console variables' values by name: `UKismetSystemLibrary::GetConsoleVariableFloatValue(const
//! FString& VariableName)` (engine, reflected), which looks the variable up in the console manager
//! and gives 0 if there's none. Survives anything but renames, so the game's tunables (e.g.
//! `BR.FluidDynamics.AirDragCoefficient`) are read rather than copied.

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection::{self, Function, Parameters, take};
use crate::game::slider_text::{FString, OwnedFString};

const LIBRARY: &str = "KismetSystemLibrary";

/// Reads console variables (after engine init).
pub struct ConsoleVariables {
    /// `UKismetSystemLibrary`'s default object, which static functions are called on.
    library: *mut UObjectBase,
    function: Function,
    name: usize,
    result: usize,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for ConsoleVariables {}
unsafe impl Sync for ConsoleVariables {}

pub fn find() -> Result<ConsoleVariables, String> {
    let class = reflection::find_class(LIBRARY).ok_or(format!("class {LIBRARY} not found"))?;
    let library = reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
        .ok_or(format!("{LIBRARY}'s default object not found"))?;
    let function = Function::find(class, "GetConsoleVariableFloatValue")?;
    Ok(ConsoleVariables {
        library,
        name: function.parameter("VariableName", "StrProperty", size_of::<FString>() as i32)?,
        result: function.parameter("ReturnValue", "FloatProperty", 4)?,
        function,
    })
}

impl ConsoleVariables {
    /// The value of the console variable `name` (an integer one's too), or `None` if it's 0:
    /// what the engine gives for a variable it doesn't know.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn float(&self, name: &str) -> Option<f32> {
        let string = OwnedFString::new(name);
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the name goes in at its reflected offset as a copy of our
        // string's header, only read by the function (`const FString&`) while `string` lives;
        // ProcessEvent doesn't destroy parameters.
        let value = unsafe {
            core::ptr::copy_nonoverlapping(
                string.as_ptr().cast::<u8>(),
                parameters.as_mut_ptr().add(self.name),
                size_of::<FString>(),
            );
            self.function.call(self.library, &mut parameters);
            take::<f32>(&parameters, self.result)
        };
        (value != 0.0 && value.is_finite()).then_some(value)
    }
}
