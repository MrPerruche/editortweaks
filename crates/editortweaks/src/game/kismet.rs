//! Unreal's Blueprint string conversions (`UKismetStringLibrary`, `UKismetTextLibrary`), called
//! through reflection: the engine's own way to make names and texts, safe from any thread state.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UFunction;

use crate::game::reflection::{self, Parameters};
use crate::game::slider_text::{FString, OwnedFString};
use crate::game::text::Text;

/// The engine's `FName` for `text`, created if new (`UKismetStringLibrary::Conv_StringToName`).
/// Unlike writing to the name pool directly, this takes the pool's lock and indexes the name.
pub fn string_to_name(text: &str) -> Result<FName, String> {
    let call = Conversion::find(
        "KismetStringLibrary",
        "Default__KismetStringLibrary",
        "Conv_StringToName",
        ("NameProperty", size_of::<FName>()),
    )?;
    // SAFETY: the result is a NameProperty, an FName; on the game thread like every caller here.
    let name: FName = unsafe { call.run(text) };
    if name.is_none() {
        return Err(format!("no name made for '{text}'"));
    }
    Ok(name)
}

/// A new `FText` showing `text` as is, not localized (`UKismetTextLibrary::Conv_StringToText`).
pub fn string_to_text(text: &str) -> Result<Text, String> {
    let call = Conversion::find(
        "KismetTextLibrary",
        "Default__KismetTextLibrary",
        "Conv_StringToText",
        ("TextProperty", size_of::<Text>()),
    )?;
    // SAFETY: the result is a TextProperty, an FText holding one reference: now ours.
    Ok(unsafe { call.run(text) })
}

/// A static reflected function taking `const FString& InString` and returning `ReturnValue`.
struct Conversion {
    /// The library's default object, which static functions are called on.
    defaults: *mut UObjectBase,
    function: *const UFunction,
    input: usize,
    output: usize,
    parameters: Parameters,
}

impl Conversion {
    fn find(
        library: &'static str,
        defaults: &'static str,
        function: &'static str,
        (kind, size): (&'static str, usize),
    ) -> Result<Self, String> {
        let class = reflection::find_class(library).ok_or(format!("class {library} not found"))?;
        let defaults = reflection::find_object(defaults, library)
            .ok_or(format!("{library}'s default object not found"))?;
        let found = reflection::find_function(class, function)?;
        let field = |name, kind, size: usize| {
            reflection::struct_field_offset(found.cast(), name, kind, size as i32)
                .map_err(|reason| format!("{function}: {reason}"))
        };
        let input = field("InString", "StrProperty", size_of::<FString>())?;
        let output = field("ReturnValue", kind, size)?;
        // SAFETY: a live function record.
        if !unsafe { Parameters::fit(found) } {
            return Err(format!("{function}: unexpected parameters"));
        }
        Ok(Self {
            defaults,
            function: found,
            input,
            output,
            parameters: Parameters::new(),
        })
    }

    /// Calls the function with `text` and returns its result.
    ///
    /// # Safety
    /// `T` must be the result's type. Game thread only.
    unsafe fn run<T>(mut self, text: &str) -> T {
        let string = OwnedFString::new(text);
        let base = self.parameters.as_mut_ptr();
        // SAFETY: both parameters lie within the checked size; the function only reads the string
        // (`const FString&`), and ProcessEvent copies the parameters shallowly without destroying
        // them, so the result is moved out of the buffer exactly once.
        unsafe {
            core::ptr::copy_nonoverlapping(
                string.as_ptr().cast::<u8>(),
                base.add(self.input),
                size_of::<FString>(),
            );
            reflection::call(self.defaults, self.function, base);
            base.add(self.output).cast::<T>().read()
        }
    }
}
