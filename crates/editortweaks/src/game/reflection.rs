//! Finding classes, their fields and their functions by name through Unreal's reflection data.
//! Names survive game updates and offsets are read from the running game, so layout changes don't
//! matter. Only usable after engine init (see `Feature::on_engine_init`).

use core::ffi::c_void;
use core::marker::PhantomData;

use brickrust::ue::coreuobject::{GObjects, UObjectBase, UObjectVTable};
use brickrust::ue::fframe::FFrame;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::framework::sig;

/// `FProperty::PropertyFlags` bit of a function's return value (`CPF_ReturnParm`).
pub const RETURN_PARAMETER: u64 = 0x400;

/// Whether `name` is `text`, ignoring ASCII case like Unreal's name comparisons. The engine stores
/// each name once, in the case it was first made with: the game's parameters `InString` are named
/// "inString", which some other code made first.
///
/// # Safety
/// `name` must be a valid name (of the live name pool).
pub unsafe fn name_is(name: &FName, text: &str) -> bool {
    // SAFETY: per the contract; an entry holds `len` characters, of one or two bytes.
    unsafe {
        let (chars, len, wide) = name.as_sptr2();
        let len = usize::from(len);
        if len != text.len() {
            return false;
        }
        if wide {
            core::slice::from_raw_parts(chars.cast::<u16>(), len)
                .iter()
                .zip(text.bytes())
                .all(|(&char, byte)| {
                    u8::try_from(char).is_ok_and(|c| c.eq_ignore_ascii_case(&byte))
                })
        } else {
            core::slice::from_raw_parts(chars, len).eq_ignore_ascii_case(text.as_bytes())
        }
    }
}

/// The native class called `name`, without its `U`/`A` prefix (e.g. `"BrickSliderWidget"`).
/// Walks every live object, so call it once and keep the result.
pub fn find_class(name: &'static str) -> Option<*const UClass> {
    find_object(name, "Class").map(|object| object as *const UClass)
}

/// The first live object called `name` whose class is called `class` (e.g. an enum: `"Enum"`; a
/// class's default object: `"Default__KismetStringLibrary"`, `"KismetStringLibrary"`). Walks every
/// live object, so call it once and keep the result.
pub fn find_object(name: &'static str, class: &'static str) -> Option<*mut UObjectBase> {
    // SAFETY: runs on the game thread after engine init; GObjects slots hold live objects or null,
    // and every object has a valid class pointer.
    unsafe {
        let wanted = FName::search_str(name);
        if wanted.is_none() {
            return None;
        }
        GObjects()
            .iter()
            .find(|&object| {
                !object.is_null()
                    && (*object).name_private.comparison_index == wanted.comparison_index
                    && (*object).name_private.number == wanted.number
                    && !(*object).class_private.is_null()
                    && name_is(
                        &(*(*object).class_private)
                            .ustruct
                            .ufield
                            .uobject
                            .name_private,
                        class,
                    )
            })
            .map(|object| object.cast())
    }
}

/// The reflected field `field` of `class` (including inherited ones), checking that its property
/// type is `kind` (e.g. `"FloatProperty"`) and that it's `size` bytes (e.g. 12 for an `FVector`).
pub fn field(
    class: *const UClass,
    field: &'static str,
    kind: &'static str,
    size: i32,
) -> Result<*const FProperty, String> {
    // SAFETY: `class` comes from `find_class`.
    struct_field(unsafe { &(*class).ustruct }, field, kind, size)
}

/// Byte offset of the reflected field `field` in instances of `class`; see [`field`].
pub fn field_offset(
    class: *const UClass,
    name: &'static str,
    kind: &'static str,
    size: i32,
) -> Result<usize, String> {
    // SAFETY: `field` only returns live property records.
    field(class, name, kind, size).map(|property| unsafe { (*property).offset_internal as usize })
}

/// [`field`] for a struct (or class) found through reflection, e.g. a `StructProperty`'s
/// [`struct_type`].
pub fn struct_field(
    ustruct: *const UStruct,
    field: &'static str,
    kind: &'static str,
    size: i32,
) -> Result<*const FProperty, String> {
    // SAFETY: `ustruct` comes from reflection; property records live as long as their struct.
    unsafe {
        let property = find_property(ustruct, field);
        if property.is_null() {
            return Err(format!("field '{field}' not found"));
        }
        if !is_kind(property, kind) {
            return Err(format!("field '{field}' isn't a {kind}"));
        }
        if (*property).element_size != size {
            return Err(format!(
                "field '{field}' is {} bytes, expected {size}",
                (*property).element_size
            ));
        }
        Ok(property)
    }
}

/// Byte offset of the reflected field `field` in `ustruct`; see [`struct_field`].
pub fn struct_field_offset(
    ustruct: *const UStruct,
    name: &'static str,
    kind: &'static str,
    size: i32,
) -> Result<usize, String> {
    // SAFETY: `struct_field` only returns live property records.
    struct_field(ustruct, name, kind, size)
        .map(|property| unsafe { (*property).offset_internal as usize })
}

/// The property called `name` in `ustruct` or its parents, or null.
///
/// # Safety
/// `ustruct` must be a live, linked class or struct.
pub unsafe fn find_property(ustruct: *const UStruct, name: &'static str) -> *const FProperty {
    // SAFETY: per the contract.
    unsafe { properties(ustruct) }
        // SAFETY: `properties` only yields live property records.
        .find(|&property| unsafe { name_is(&(*property).field.name_private, name) })
        .unwrap_or(core::ptr::null())
}

/// Every property of `ustruct` and its parents; for a function, its parameters in order.
///
/// # Safety
/// `ustruct` must be a live, linked class, struct or function, and stay alive while iterating.
pub unsafe fn properties(ustruct: *const UStruct) -> impl Iterator<Item = *const FProperty> {
    // SAFETY: per the contract; `property_link` chains every property of the struct and its
    // parents.
    let first = unsafe { (*ustruct).property_link };
    core::iter::successors((!first.is_null()).then_some(first), |&property| {
        // SAFETY: as above.
        let next = unsafe { (*property).property_link_next }.cast_const();
        (!next.is_null()).then_some(next)
    })
}

/// Whether the property's type is `kind` (e.g. `"FloatProperty"`).
///
/// # Safety
/// `property` must be a live property record.
pub unsafe fn is_kind(property: *const FProperty, kind: &'static str) -> bool {
    // SAFETY: per the contract; a property's class record lives as long as the program.
    unsafe {
        let class = (*property).field.class_private;
        !class.is_null() && name_is(&(*class).name, kind)
    }
}

/// The struct a `StructProperty` holds (e.g. `Vector`), or null for other property types.
///
/// # Safety
/// `property` must be a live property record.
pub unsafe fn struct_type(property: *const FProperty) -> *const UStruct {
    // UE 4.27: `FStructProperty` adds `UScriptStruct* Struct` right after the `FProperty` fields.
    const _: () = assert!(size_of::<FProperty>() == 0x78);
    // SAFETY: per the contract; a StructProperty record has the struct pointer at that place.
    unsafe {
        if !is_kind(property, "StructProperty") {
            return core::ptr::null();
        }
        *property.add(1).cast::<*const UStruct>()
    }
}

/// The class or struct that declares `property`, or null if the property is part of another field
/// (e.g. the elements of an array).
///
/// # Safety
/// `property` must be a live property record.
pub unsafe fn declaring_struct(property: *const FProperty) -> *const UStruct {
    // SAFETY: per the contract. A property declared by a class or struct is owned by that object.
    unsafe { (*property).field.owner.as_uobject().cast::<UStruct>() }
}

/// Whether `ustruct` is a class (its instances are objects) rather than a plain struct.
///
/// # Safety
/// `ustruct` must be a live class or struct.
pub unsafe fn is_class(ustruct: *const UStruct) -> bool {
    // SAFETY: per the contract; every object has a class, here `Class`, `ScriptStruct` or one of
    // their subclasses (e.g. `BlueprintGeneratedClass`).
    unsafe {
        let meta = (*ustruct).ufield.uobject.class_private;
        !meta.is_null() && (*meta).ustruct.InheritsFrom_str("Class")
    }
}

/// The class of a live object.
///
/// # Safety
/// `object` must be a live object.
pub unsafe fn class_of(object: *const c_void) -> *const UStruct {
    // SAFETY: per the contract.
    unsafe {
        (*object.cast::<UObjectBase>())
            .class_private
            .cast::<UStruct>()
    }
}

/// The name of a class or struct, e.g. `ScalableBrick`.
///
/// # Safety
/// `ustruct` must be a live class or struct.
pub unsafe fn struct_name<'a>(ustruct: *const UStruct) -> &'a FName {
    // SAFETY: per the contract.
    unsafe { &(*ustruct).ufield.uobject.name_private }
}

/// `ustruct` followed by its parents.
///
/// # Safety
/// `ustruct` must be null or a live class or struct, and stay alive while iterating.
pub unsafe fn with_parents(ustruct: *const UStruct) -> impl Iterator<Item = *const UStruct> {
    let first = (!ustruct.is_null()).then_some(ustruct);
    core::iter::successors(first, |&current| {
        // SAFETY: per the contract; a parent is live as long as its children.
        let parent = unsafe { (*current).super_struct };
        (!parent.is_null()).then_some(parent)
    })
}

/// The reflected function (`UFUNCTION`) `name` of `class` or its parents.
pub fn find_function(class: *const UClass, name: &'static str) -> Result<*const UFunction, String> {
    // SAFETY: `class` comes from `find_class`; the lookup only reads the class's function map.
    let function = unsafe { (*class).FindFunctionByName(name, true) };
    if function.is_null() {
        return Err(format!("function '{name}' not found"));
    }
    Ok(function)
}

/// What the engine calls for a reflected C++ function (its "exec" thunk): reads the parameters from
/// `stack` and calls the C++ function. `result` receives the return value, if any.
pub type NativeThunk =
    unsafe extern "C" fn(context: *mut c_void, stack: *mut c_void, result: *mut c_void);

/// `UFunction` fields brickrust doesn't declare: their offsets in UE 4.27 shipping builds, right
/// after `UStruct` (0xB0 bytes). Editor builds (BRMK) differ.
const FUNCTION_FLAGS: usize = 0xB0;
const FUNCTION_THUNK: usize = 0xD8;
/// `EFunctionFlags::FUNC_Native`: implemented in C++.
const FUNC_NATIVE: u32 = 0x400;

/// The exec thunk of a reflected C++ function, checked to be the start of a game function. Hooking
/// it intercepts every call made through reflection: Blueprints, dynamic delegates (e.g. bound
/// widget events) and `ProcessEvent`.
pub fn native_thunk(function: *const UFunction) -> Result<*const u8, String> {
    // SAFETY: `function` comes from `find_function`; both fields lie within a UFunction.
    let (flags, thunk) = unsafe {
        let base = function.cast::<u8>();
        (
            base.add(FUNCTION_FLAGS).cast::<u32>().read(),
            base.add(FUNCTION_THUNK).cast::<*const u8>().read(),
        )
    };
    if flags & FUNC_NATIVE == 0 {
        return Err("not a C++ function".into());
    }
    if !sig::is_function_start(thunk) {
        return Err(format!("its code pointer {thunk:p} isn't a game function"));
    }
    Ok(thunk)
}

/// A buffer for the parameters of a reflected call (see [`call`]), zeroed and aligned for vectors.
#[repr(C, align(16))]
pub struct Parameters([u8; Parameters::SIZE]);

impl Parameters {
    pub const SIZE: usize = 0x40;

    pub fn new() -> Self {
        Self([0; Self::SIZE])
    }

    /// Whether the parameters of `function` fit.
    ///
    /// # Safety
    /// `function` must be a live function record.
    pub unsafe fn fit(function: *const UFunction) -> bool {
        // SAFETY: per the contract.
        let size = unsafe { (*function.cast::<UStruct>()).properties_size };
        usize::try_from(size).is_ok_and(|size| size <= Self::SIZE)
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
}

/// Calls the reflected function `function` on `object` through `ProcessEvent`, as Blueprints do.
/// `params` holds the function's parameters at their reflected offsets (see [`properties`]); the
/// return value is written there too.
///
/// # Safety
/// `object` must be a live object whose class has `function`, and `params` must point to at least
/// `properties_size` writable bytes of the function, laid out as its parameters. Game thread only.
pub unsafe fn call(object: *mut UObjectBase, function: *const UFunction, params: *mut u8) {
    // SAFETY: per the contract; every object's vtable starts with UObject's.
    unsafe {
        let vtable = &*(*object).vtable.cast::<UObjectVTable>();
        (vtable.ProcessEvent)(object, function.cast_mut(), params.cast());
    }
}

/// Where the parameters of an exec thunk's call are, if it was called through `ProcessEvent` (e.g.
/// by a dynamic delegate): at their reflected offsets in the frame's locals. `None` for calls from
/// Blueprint bytecode, whose parameters the thunk reads from the script instead.
///
/// # Safety
/// `stack` must be the frame an exec thunk was called with.
pub unsafe fn native_call_parameters(stack: *const c_void) -> Option<*const u8> {
    // SAFETY: per the contract; brickrust declares `FFrame`'s layout up to `Locals`.
    let frame = unsafe { &*stack.cast::<FFrame>() };
    (frame.code.is_null() && !frame.locals.is_null()).then_some(frame.locals.cast_const())
}

/// `EPropertyFlags::CPF_OutParm`: an out parameter, or a `const&` one (UHT flags both alike).
const OUT_PARAMETER: u64 = 0x100;

/// Where an exec thunk called through `ProcessEvent` reads its parameter `parameter` from, like
/// `FFrame::StepExplicitProperty`: an out or `const&` parameter from the caller's own variable
/// (listed in the frame's `OutParms`), any other from its copy in the frame's locals (see
/// [`native_call_parameters`]). `None` for calls from Blueprint bytecode.
///
/// # Safety
/// `stack` must be the frame an exec thunk was called with, and `parameter` one of its function's
/// parameters.
pub unsafe fn native_call_parameter(
    stack: *const c_void,
    parameter: *const FProperty,
) -> Option<*mut u8> {
    // UE 4.27's `FFrame::OutParms` (vendored brickrust patch 8).
    const _: () = assert!(core::mem::offset_of!(FFrame, out_params) == 0x78);
    // SAFETY: per the contract; the out parameter records live as long as the call.
    unsafe {
        let locals = native_call_parameters(stack)?;
        if (*parameter).property_flags & OUT_PARAMETER == 0 {
            let offset = (*parameter).offset_internal as usize;
            return Some(locals.add(offset).cast_mut());
        }
        let mut record = (*stack.cast::<FFrame>()).out_params;
        while !record.is_null() {
            if (*record).prop.cast_const() == parameter {
                return Some((*record).addr.cast());
            }
            record = (*record).next;
        }
        None
    }
}

/// A reflected function without parameters returning a `T`: an object pointer (`"ObjectProperty"`),
/// or a small struct read as plain data, e.g. an `FVector` as `[f32; 3]` (`"StructProperty"`).
pub struct Getter<T> {
    function: *const UFunction,
    /// Offset of the return value in the parameters.
    result: usize,
    returns: PhantomData<T>,
}

impl<T: Copy> Getter<T> {
    /// The function `name` of `class`, checked to return a `kind` property of `T`'s size.
    pub fn find(
        class: *const UClass,
        name: &'static str,
        kind: &'static str,
    ) -> Result<Self, String> {
        let function = find_function(class, name)?;
        // SAFETY: a live function record; its only parameter must be the return value.
        unsafe {
            let mut parameters = properties(function.cast());
            match (parameters.next(), parameters.next()) {
                (Some(result), None)
                    if (*result).property_flags & RETURN_PARAMETER != 0
                        && is_kind(result, kind)
                        && (*result).element_size as usize == size_of::<T>()
                        && Parameters::fit(function) =>
                {
                    Ok(Self {
                        function,
                        result: (*result).offset_internal as usize,
                        returns: PhantomData,
                    })
                }
                _ => Err(format!("{name}: unexpected parameters")),
            }
        }
    }

    /// # Safety
    /// `object` must be a live object whose class has this function (any object of the class for a
    /// static function, e.g. its default object), and every value the function returns must be a
    /// valid `T`. Game thread only.
    pub unsafe fn call(&self, object: *mut UObjectBase) -> T {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the result lies within the checked parameter size.
        unsafe {
            let base = parameters.as_mut_ptr();
            call(object, self.function, base);
            base.add(self.result).cast::<T>().read_unaligned()
        }
    }
}

/// A reflected function without parameters that returns a `bool` (e.g. `IsTyping`).
pub struct BoolFunction {
    function: *const UFunction,
    /// Offset of the return value in the parameters.
    result: usize,
}

// SAFETY: reflection records live as long as the game; calls only happen on the game thread.
unsafe impl Send for BoolFunction {}
unsafe impl Sync for BoolFunction {}

impl BoolFunction {
    pub fn find(class: *const UClass, name: &'static str) -> Result<Self, String> {
        let function = find_function(class, name)?;
        // SAFETY: a live function record; its only parameter must be the returned bool.
        unsafe {
            let parameters = function.cast::<UStruct>();
            match properties(parameters).next() {
                Some(result)
                    if (*parameters).properties_size == 1 && is_kind(result, "BoolProperty") =>
                {
                    Ok(Self {
                        function,
                        result: (*result).offset_internal as usize,
                    })
                }
                _ => Err(format!("{name}: unexpected parameters")),
            }
        }
    }

    /// # Safety
    /// `object` must be a live object whose class has this function. Game thread only.
    pub unsafe fn call(&self, object: *mut UObjectBase) -> bool {
        let mut params = [0u64; 1];
        // SAFETY: per the contract; the one-byte return value fits in `params`.
        unsafe {
            let base = params.as_mut_ptr().cast::<u8>();
            call(object, self.function, base);
            base.add(self.result).read() != 0
        }
    }
}
