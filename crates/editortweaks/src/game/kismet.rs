//! Unreal's Blueprint string conversions (`UKismetStringLibrary`, `UKismetTextLibrary`), called
//! through reflection: the engine's own way to make names and texts, safe from any thread state.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UFunction;

use crate::game::reflection::{self, Function, Parameters};
use crate::game::slider_text::{FString, OwnedFString};
use crate::game::text::Text;

/// The engine's `FName` for `text`, created if new (`UKismetStringLibrary::Conv_StringToName`).
/// Unlike writing to the name pool directly, this takes the pool's lock and indexes the name.
pub fn string_to_name(text: &str) -> Result<FName, String> {
    names()?.name(text)
}

/// `UKismetStringLibrary::Conv_StringToName`, found once to be used often (finding it walks every
/// object).
pub struct Names {
    to_name: Conversion,
}

// SAFETY: reflection records and a default object living as long as the game; only called on the
// game thread.
unsafe impl Send for Names {}
unsafe impl Sync for Names {}

/// Finds [`Names`]. After engine init.
pub fn names() -> Result<Names, String> {
    Ok(Names {
        to_name: Conversion::find(
            "KismetStringLibrary",
            "Default__KismetStringLibrary",
            "Conv_StringToName",
            ("NameProperty", size_of::<FName>()),
        )?,
    })
}

impl Names {
    /// The engine's `FName` for `text` (see [`string_to_name`]). Game thread only.
    pub fn name(&self, text: &str) -> Result<FName, String> {
        // SAFETY: the result is a NameProperty, an FName; on the game thread like every caller
        // here.
        let name: FName = unsafe { self.to_name.run(text) };
        if name.is_none() {
            return Err(format!("no name made for '{text}'"));
        }
        Ok(name)
    }
}

/// A new `FText` showing `text` as is, not localized (`UKismetTextLibrary::Conv_StringToText`).
pub fn string_to_text(text: &str) -> Result<Text, String> {
    // SAFETY: the result is a TextProperty, an FText holding one reference: now ours.
    Ok(unsafe { string_to_text_conversion()?.run(text) })
}

fn string_to_text_conversion() -> Result<Conversion, String> {
    Conversion::find(
        TEXT_LIBRARY,
        "Default__KismetTextLibrary",
        "Conv_StringToText",
        ("TextProperty", size_of::<Text>()),
    )
}

const TEXT_LIBRARY: &str = "KismetTextLibrary";

/// `UKismetTextLibrary`'s conversions between strings and texts, found once to be used often
/// (finding them walks every object).
pub struct Texts {
    from_string: Conversion,
    /// `Conv_TextToString(const FText& InText)`, and the offsets of `InText` and the result.
    to_string: Function,
    text: usize,
    string: usize,
}

// SAFETY: reflection records and a default object living as long as the game; only called on the
// game thread.
unsafe impl Send for Texts {}
unsafe impl Sync for Texts {}

/// Finds [`Texts`]. After engine init.
pub fn texts() -> Result<Texts, String> {
    let class =
        reflection::find_class(TEXT_LIBRARY).ok_or(format!("class {TEXT_LIBRARY} not found"))?;
    let to_string = Function::find(class, "Conv_TextToString")?;
    Ok(Texts {
        from_string: string_to_text_conversion()?,
        text: to_string.parameter("InText", "TextProperty", size_of::<Text>() as i32)?,
        string: to_string.parameter("ReturnValue", "StrProperty", size_of::<FString>() as i32)?,
        to_string,
    })
}

impl Texts {
    /// A new `FText` showing `text` as is (see [`string_to_text`]). Game thread only.
    pub fn text(&self, text: &str) -> Text {
        // SAFETY: the result is a TextProperty, an FText holding one reference: now ours.
        unsafe { self.from_string.run(text) }
    }

    /// What `text` shows, giving up its reference (`Conv_TextToString`).
    ///
    /// # Safety
    /// `text` must hold a live reference. Game thread only.
    pub unsafe fn string(&self, text: Text) -> String {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract. The text goes in as is, the function only reading it (`const
        // FText&`); the result is a new string of the game's, ours to free. ProcessEvent destroys
        // neither.
        unsafe {
            reflection::put(&mut parameters, self.text, text);
            self.to_string
                .call(self.from_string.defaults, &mut parameters);
            reflection::take::<Text>(&parameters, self.text).release();
            let string = reflection::take::<FString>(&parameters, self.string);
            let shown = string.to_string();
            string.free();
            shown
        }
    }
}

/// A static reflected function taking `const FString& InString` and returning `ReturnValue`.
struct Conversion {
    /// The library's default object, which static functions are called on.
    defaults: *mut UObjectBase,
    function: *const UFunction,
    input: usize,
    output: usize,
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
        })
    }

    /// Calls the function with `text` and returns its result.
    ///
    /// # Safety
    /// `T` must be the result's type. Game thread only.
    unsafe fn run<T>(&self, text: &str) -> T {
        let string = OwnedFString::new(text);
        let mut parameters = Parameters::new();
        let base = parameters.as_mut_ptr();
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

/// Opens `url` in the player's browser, as Blueprints do (`UKismetSystemLibrary::LaunchURL`; on
/// Windows the shell opens it). Game thread only, after engine init.
pub fn launch_url(url: &str) -> Result<(), String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    let class = reflection::find_class(LIBRARY).ok_or(format!("class {LIBRARY} not found"))?;
    let defaults = reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
        .ok_or(format!("{LIBRARY}'s default object not found"))?;
    let launch = Function::find(class, "LaunchURL")?;
    let at = launch.parameter("URL", "StrProperty", size_of::<FString>() as i32)?;
    let string = OwnedFString::new(url);
    let mut parameters = Parameters::new();
    // SAFETY: on the game thread; the URL goes in as our string's header at its reflected offset,
    // which the function only reads (`const FString&`) and ProcessEvent doesn't destroy.
    unsafe {
        core::ptr::copy_nonoverlapping(
            string.as_ptr().cast::<u8>(),
            parameters.as_mut_ptr().add(at),
            size_of::<FString>(),
        );
        launch.call(defaults, &mut parameters);
    }
    Ok(())
}
