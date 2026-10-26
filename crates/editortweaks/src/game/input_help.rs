//! The vehicle editor's top bar and the dropdowns it opens (the game's input help), where the mod
//! adds a menu of its own (`features::editor_menu`). Found 2026-10-26 in BRMK 1.11.6 (exports,
//! the widgets' Blueprints read with `et.py blueprint`) and game 1.11.7 (build 25831956); see
//! `docs/re/editor-menu.md`.
//!
//! - The top bar is `WBP_BrickEditor_TopDashboard` (a `WBP_Dashboard`, a `UDashboardWidget`; not a
//!   `UEditorDashboardWidget`).
//!   `UDashboardWidget::UpdateDashboard` (reflected) calls three Blueprint events:
//!   `PreUpdateDashboard` (resets the counts of its widgets), `PopulateDashboard` (the Blueprint adds
//!   the File, Modify, Selection and Groups categories with `AddInputHelpCategory`, then the exit
//!   actions) and `PostUpdateDashboard` (pools the widgets it didn't use, finishes the row).
//!   [`InputHelp::populate`] is the C++ stub of the second, which every dashboard's update calls
//!   (`UpdateDashboard` and 10 inlined copies). Its widgets fill a grid `NumColumns` wide, a new
//!   row when one doesn't fit.
//! - A category's button opens the input help on it (`UInputHelpWidget::OpenCategory`, then
//!   `UpdateActions`, [`InputHelp::update_actions`]): the actions bound in the player's input stack
//!   whose rows belong to the category at the top of `InputCategoryStack`, as `FInputActionInfo`s
//!   handed to the Blueprint event `AddCategoriesAndActions(Categories, Actions)`, which shows one
//!   `WBP_InputAction` per action.
//! - Such a widget shows and runs its action through its input component:
//!   `UBaseInputComponent::ExecuteAction(Name, Params)` ([`EXECUTE_ACTION`]) with the mode `Test`
//!   to know whether it's enabled (its result's `State`), `Real` when clicked. An action the
//!   component hasn't bound gets `State` 2 (not `Enabled`), so the widget is greyed out.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::framework::sig::{self, CodeShape, FunctionSig};
use crate::game::reflection::{self, BoolField, Function, Parameters, SingleCall};

/// `FBrActionResult UBaseInputComponent::ExecuteAction(const FName& Name, const FBrActionParams&
/// Params)` (BRMK 0x234620, game RVA 0xDDB5F0): finds the action's binding in the component and
/// runs it (or only asks it, in `Test` mode), else returns a result with `State` 2. Every widget
/// of an action calls it: the input help's, the top bar's.
pub const EXECUTE_ACTION: FunctionSig = FunctionSig {
    name: "UBaseInputComponent::ExecuteAction",
    // The binding's delegate: `IsBound()` (vtable +0x30), then picks its object to `Execute`.
    pattern: "ff 50 30 84 c0 74 ?? 39 6b 08 74 05 48 8b 0b eb 03 48 8b cd 48 8b 01",
};

/// The result is returned by value: MSVC passes its address after `this` and returns it.
pub type ExecuteActionFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    result: *mut u8,
    name: *const FName,
    params: *const u8,
) -> *mut u8;

/// `void UDashboardWidget::PopulateDashboard()`: the C++ stub of the Blueprint event.
pub type PopulateFn = unsafe extern "C" fn(dashboard: *mut UObjectBase);

/// `void UInputHelpWidget::UpdateActions(bool bForce)`.
pub type UpdateActionsFn = unsafe extern "C" fn(help: *mut UObjectBase, force: bool);

/// `UDashboardWidget::UpdateDashboard()` (game 0xEA88E0, the jump target of its reflected
/// thunk): `PreUpdateDashboard(); PopulateDashboard(); PostUpdateDashboard();`, the last a jump.
const UPDATE_DASHBOARD: CodeShape = CodeShape {
    name: "UDashboardWidget::UpdateDashboard",
    shape: "40 53 48 83 ec 20 48 8b d9 e8 ?? ?? ?? ?? 48 8b cb e8 ?? ?? ?? ?? 48 8b cb 48 83 c4 20 5b e9 ?? ?? ?? ??",
};
/// Where [`UPDATE_DASHBOARD`]'s call of `PopulateDashboard` is.
const POPULATE_CALL: usize = 0x11;

/// The editor's top bar's Blueprint class.
const TOP_BAR_CLASS: &str = "WBP_BrickEditor_TopDashboard_C";

/// The top bar, the input help and the editor's actions (after engine init).
pub struct InputHelp {
    /// `PopulateDashboard`'s stub (game 0xFAEB50), to hook.
    pub populate: *const u8,
    /// `UInputHelpWidget::UpdateActions` (game 0xF3ED30), to hook.
    pub update_actions: *const u8,
    /// Offset of the editor's (`ABrickEditor`'s) `InputComponent`, its `UEditorInputComponent`.
    editor_input: usize,
    /// Offset of `UInputHelpWidget::InputCategoryStack` (`TArray<UInputCategory*>`).
    category_stack: usize,
    /// `UInputHelpWidget::AddCategoriesAndActions(const TArray<UInputCategory*>& Categories, const
    /// TArray<FInputActionInfo>& Actions)`.
    add_actions: Function,
    add_categories_param: usize,
    add_actions_param: usize,
    info: InfoLayout,
    params: ParamsLayout,
    /// `EBrActionState::Enabled`.
    enabled: u8,
    /// Offset of `FBrActionResult::State` (a byte).
    state: usize,
    /// How widgets sit in a dashboard's grid, or why that wasn't found.
    grid: Result<Grid, String>,
}

/// A widget's place in a `UGridPanel`.
struct Grid {
    /// Offset of `UWidget::Slot` (`UPanelSlot*`).
    widget_slot: usize,
    /// `UGridSlot`.
    class: *const UStruct,
    /// Offsets of `UGridSlot::Row` and `Column` (`int32`).
    row: usize,
    column: usize,
    /// `UGridSlot::SetColumn(int32 InColumn)`, which moves the widget (setting `Column` doesn't).
    set_column: SingleCall,
}

// SAFETY: code pointers and reflection records living as long as the game; only used on the game
// thread.
unsafe impl Send for InputHelp {}
unsafe impl Sync for InputHelp {}

/// `FInputActionInfo`: an action as the input help lists it.
struct InfoLayout {
    size: usize,
    /// `InputComponent` (`UBaseInputComponent*`).
    component: usize,
    /// `ActionName` (`FName`).
    name: usize,
}

/// `FBrActionParams`: how an action is run.
struct ParamsLayout {
    /// `Mode` (`EBrActionMode`, a byte).
    mode: usize,
    /// `EBrActionMode::Real`: run it (`Test` only asks whether it would run).
    real: u8,
    /// `AxisValue` (`float`).
    axis: usize,
    secondary: BoolField,
}

/// How an action widget runs an action (see [`InputHelp::params`]).
pub struct Run {
    /// Run, not only asked about.
    pub real: bool,
    /// Its secondary action (e.g. the release of a held one).
    pub secondary: bool,
    /// Its axis value: 0 from a click, not while a widget of an axis is held (run each frame).
    pub axis: f32,
}

impl InputHelp {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let script_struct = |name| {
            reflection::find_object(name, "ScriptStruct")
                .map(|found| found.cast_const().cast::<UStruct>())
                .ok_or(format!("struct {name} not found"))
        };
        let help = class("InputHelpWidget")?;

        let update_dashboard =
            reflection::find_function(class("DashboardWidget")?, "UpdateDashboard")
                .and_then(reflection::parameterless_native)
                .map_err(|reason| format!("UpdateDashboard: {reason}"))?;
        if !UPDATE_DASHBOARD.matches(update_dashboard) {
            return Err(format!("{} changed", UPDATE_DASHBOARD.name));
        }
        // SAFETY: the shape checked covers the call (E8, then its distance from its end).
        let populate = unsafe {
            let distance = update_dashboard
                .add(POPULATE_CALL + 1)
                .cast::<i32>()
                .read_unaligned();
            update_dashboard
                .wrapping_add(POPULATE_CALL + 5)
                .wrapping_offset(distance as isize)
        };
        if !sig::is_function_start(populate) {
            return Err("PopulateDashboard's stub isn't a function".into());
        }

        let update_actions_thunk =
            reflection::native_thunk(reflection::find_function(help, "UpdateActions")?)
                .map_err(|reason| format!("UpdateActions: {reason}"))?;
        let update_actions = *sig::direct_calls(update_actions_thunk)
            .last()
            .filter(|&&target| sig::is_function_start(target))
            .ok_or("UpdateActions' thunk calls no function last")?;

        let add_actions = Function::find(help, "AddCategoriesAndActions")?;
        let info = script_struct("InputActionInfo")?;
        let params = script_struct("BrActionParams")?;
        let result = script_struct("BrActionResult")?;
        let byte = |name, entry| -> Result<u8, String> {
            u8::try_from(reflection::enum_value(name, entry)?)
                .map_err(|_| format!("{entry}: unexpected value"))
        };
        Ok(Self {
            populate,
            update_actions,
            editor_input: reflection::field_offset(
                class("BrickEditor")?,
                "InputComponent",
                "ObjectProperty",
                8,
            )?,
            category_stack: reflection::field_offset(
                help,
                "InputCategoryStack",
                "ArrayProperty",
                0x10,
            )?,
            add_categories_param: add_actions.parameter("Categories", "ArrayProperty", 0x10)?,
            add_actions_param: add_actions.parameter("Actions", "ArrayProperty", 0x10)?,
            add_actions,
            info: InfoLayout {
                // SAFETY: a live struct.
                size: unsafe { reflection::struct_size(info) },
                component: reflection::struct_field_offset(
                    info,
                    "InputComponent",
                    "ObjectProperty",
                    8,
                )?,
                name: reflection::struct_field_offset(info, "ActionName", "NameProperty", 8)?,
            },
            params: ParamsLayout {
                mode: reflection::struct_field_offset(params, "Mode", "EnumProperty", 1)?,
                real: byte("EBrActionMode", "EBrActionMode::Real")?,
                axis: reflection::struct_field_offset(params, "AxisValue", "FloatProperty", 4)?,
                secondary: reflection::bool_field(params, "bIsSecondaryAction")?,
            },
            enabled: byte("EBrActionState", "EBrActionState::Enabled")?,
            state: reflection::struct_field_offset(result, "State", "EnumProperty", 1)?,
            grid: Grid::find(),
        })
    }

    /// Whether `dashboard` is the editor's top bar.
    ///
    /// # Safety
    /// `dashboard` must be a live object. Game thread only.
    pub unsafe fn is_top_bar(&self, dashboard: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(dashboard.cast())) }
            // SAFETY: classes of a live object.
            .any(|class| unsafe {
                reflection::name_is(reflection::struct_name(class), TOP_BAR_CLASS)
            })
    }

    /// The editor's input component, if it has one.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    pub unsafe fn editor_input(&self, editor: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field is an object pointer of editors.
        let input = unsafe {
            editor
                .cast::<u8>()
                .add(self.editor_input)
                .cast::<*mut UObjectBase>()
                .read()
        };
        (!input.is_null()).then_some(input)
    }

    /// Adds a button opening `category` to `dashboard` (`AddInputHelpCategory`, a Blueprint
    /// function of its class), as its Blueprint adds the game's, on the same row.
    ///
    /// # Safety
    /// `dashboard` must be a live dashboard widget, `category` a live input category. Game thread
    /// only, while the dashboard populates (between its pre- and post-update).
    pub unsafe fn add_category(
        &self,
        dashboard: *mut UObjectBase,
        category: *mut UObjectBase,
    ) -> Result<(), String> {
        // SAFETY: per the contract; the class is the dashboard's (a Blueprint class), and the
        // parameter is written at its reflected offset.
        unsafe {
            self.make_room(dashboard)?;
            let class = reflection::class_of(dashboard.cast()).cast::<UClass>();
            let function: *const UFunction =
                reflection::find_function(class, "AddInputHelpCategory")?;
            if !Parameters::fit(function) {
                return Err("AddInputHelpCategory: too many parameters".into());
            }
            let offset = reflection::parameter_offset(
                function,
                "AddInputHelpCategory",
                "InCategory",
                "ObjectProperty",
                8,
            )?;
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, offset, category);
            reflection::call(dashboard, function, parameters.as_mut_ptr());
        }
        Ok(())
    }

    /// Makes room for one more column on `dashboard`'s row, which its Blueprint sized for its own
    /// widgets: a widget past its `NumColumns` would start a new row (`AddContentWidget`). Its width
    /// (`NumColumns` times 64) is set by its `PreConstruct`, run again for it. Only the first time:
    /// then the row has room.
    ///
    /// # Safety
    /// As for [`Self::add_category`].
    unsafe fn make_room(&self, dashboard: *mut UObjectBase) -> Result<(), String> {
        // SAFETY: per the contract; both are `int32` variables of the dashboard's Blueprint.
        unsafe {
            let class = reflection::class_of(dashboard.cast());
            let columns = int_variable(dashboard, class, "NumColumns")?;
            let used = int_variable(dashboard, class, "CurrentColumn")?.read();
            if used < columns.read() {
                return Ok(());
            }
            columns.write(used + 1);
            let construct = reflection::find_function(class.cast(), "PreConstruct")?;
            if !Parameters::fit(construct) {
                return Err("PreConstruct: too many parameters".into());
            }
            // `IsDesignTime` false.
            let mut parameters = Parameters::new();
            reflection::call(dashboard, construct, parameters.as_mut_ptr());
        }
        Ok(())
    }

    /// Moves the category button [`Self::add_category`] just added to `dashboard` before the
    /// actions on its row (the top bar's exit buttons), which move one column right. A dashboard's
    /// Blueprint adds its categories first, so only actions sit between.
    ///
    /// # Safety
    /// As for [`Self::add_category`], right after it.
    pub unsafe fn move_before_actions(&self, dashboard: *mut UObjectBase) -> Result<(), String> {
        let grid = self.grid.as_ref().map_err(Clone::clone)?;
        // SAFETY: per the contract; the widgets are the dashboard's, alive while it is.
        unsafe {
            let class = reflection::class_of(dashboard.cast());
            let categories = used_widgets(
                dashboard,
                class,
                "InputCategoryWidgets",
                "NumInputCategories",
            )?;
            let actions = used_widgets(dashboard, class, "ActionWidgets", "NumActions")?;
            let ours = categories
                .last()
                .and_then(|&widget| grid.slot(widget))
                .ok_or("its button isn't in the grid")?;
            let (row, column) = grid.place(ours);
            let before: Vec<_> = actions
                .iter()
                .filter_map(|&widget| grid.slot(widget))
                .filter(|&slot| {
                    let (other_row, other_column) = grid.place(slot);
                    other_row == row && other_column < column
                })
                .collect();
            let Some(first) = before.iter().map(|&slot| grid.place(slot).1).min() else {
                return Ok(());
            };
            for slot in before {
                grid.set_column.call(slot, grid.place(slot).1 + 1);
            }
            grid.set_column.call(ours, first);
        }
        Ok(())
    }

    /// How many action widgets `help` shows: its Blueprint's `ActionWidgets` (pooled down to the
    /// count of actions at each `AddCategoriesAndActions`), if found.
    ///
    /// # Safety
    /// `help` must be a live input help widget. Game thread only.
    pub unsafe fn shown_action_count(&self, help: *mut UObjectBase) -> Option<i32> {
        // SAFETY: per the contract; the property is the widget class's own, checked to be an
        // array.
        unsafe {
            let property =
                reflection::find_property(reflection::class_of(help.cast()), "ActionWidgets");
            if property.is_null() || !reflection::is_kind(property, "ArrayProperty") {
                return None;
            }
            let widgets = &*help
                .cast::<u8>()
                .add((*property).offset_internal as usize)
                .cast::<TArray<*mut UObjectBase>>();
            Some(widgets.num)
        }
    }

    /// The category `help` shows (the top of its stack), if any.
    ///
    /// # Safety
    /// `help` must be a live input help widget. Game thread only.
    pub unsafe fn shown_category(&self, help: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field is an array of category pointers.
        unsafe {
            let stack = &*help
                .cast::<u8>()
                .add(self.category_stack)
                .cast::<TArray<*mut UObjectBase>>();
            if stack.num <= 0 || stack.data.is_null() {
                return None;
            }
            let top = stack.data.add(stack.num as usize - 1).read();
            (!top.is_null()).then_some(top)
        }
    }

    /// Shows the actions `actions` in `help`, run through `component`, in place of what it shows.
    ///
    /// # Safety
    /// `help` must be a live input help widget, `component` a live input component, `actions`
    /// valid names. Game thread only.
    pub unsafe fn show_actions(
        &self,
        help: *mut UObjectBase,
        component: *mut UObjectBase,
        actions: &[FName],
    ) -> Result<(), String> {
        // The Blueprint only reads the arrays (const references): ours, borrowed for the call.
        let mut infos = vec![0u8; self.info.size * actions.len()];
        for (index, name) in actions.iter().enumerate() {
            let info = &mut infos[index * self.info.size..];
            // SAFETY: both fields lie within the info (reflected offsets and sizes).
            unsafe {
                info.as_mut_ptr()
                    .add(self.info.component)
                    .cast::<*mut UObjectBase>()
                    .write_unaligned(component);
                info.as_mut_ptr()
                    .add(self.info.name)
                    .cast::<FName>()
                    .write_unaligned(*name);
            }
        }
        let array = RawArray {
            data: infos.as_mut_ptr(),
            num: actions.len() as i32,
            max: actions.len() as i32,
        };
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the arrays are written at their reflected offsets, the
        // categories' empty (all zeros), and outlive the call.
        unsafe {
            let no_categories = RawArray {
                data: core::ptr::null_mut(),
                num: 0,
                max: 0,
            };
            reflection::put(&mut parameters, self.add_categories_param, no_categories);
            reflection::put(&mut parameters, self.add_actions_param, array);
            // The Blueprint's (`WBP_InputHelp`): the C++ class only declares the event.
            self.add_actions.call_event(help, &mut parameters)
        }
    }

    /// How `params` (an `FBrActionParams` given to [`EXECUTE_ACTION`]) runs its action.
    ///
    /// # Safety
    /// `params` must point to a valid `FBrActionParams`.
    pub unsafe fn params(&self, params: *const u8) -> Run {
        // SAFETY: per the contract; both fields lie within it (reflected offsets).
        unsafe {
            Run {
                real: params.add(self.params.mode).read() == self.params.real,
                secondary: self.params.secondary.get(params),
                axis: params.add(self.params.axis).cast::<f32>().read_unaligned(),
            }
        }
    }

    /// Makes `result` (an `FBrActionResult` [`EXECUTE_ACTION`] returned) say the action is
    /// enabled.
    ///
    /// # Safety
    /// `result` must point to a valid `FBrActionResult`.
    pub unsafe fn enable(&self, result: *mut u8) {
        // SAFETY: per the contract; the state is a byte enum at its reflected offset.
        unsafe { result.add(self.state).write(self.enabled) };
    }
}

/// The `int32` variable `name` of `object`, whose class is `class`.
///
/// # Safety
/// `object` must be a live object of `class`, and stay alive while the pointer is used.
unsafe fn int_variable(
    object: *mut UObjectBase,
    class: *const UStruct,
    name: &'static str,
) -> Result<*mut i32, String> {
    // SAFETY: per the contract; the property is the class's, checked to be an `int32`.
    unsafe {
        let property = reflection::find_property(class, name);
        if property.is_null() || !reflection::is_kind(property, "IntProperty") {
            return Err(format!("variable {name} not found"));
        }
        Ok(object
            .cast::<u8>()
            .add((*property).offset_internal as usize)
            .cast())
    }
}

/// The widgets `object` (a dashboard) uses of its pool `array`: the first `count` (both variables
/// of its Blueprint; widgets past the count are pooled at the end of its update).
///
/// # Safety
/// `object` must be a live object of `class`. The widgets live while it does.
unsafe fn used_widgets(
    object: *mut UObjectBase,
    class: *const UStruct,
    array: &'static str,
    count: &'static str,
) -> Result<Vec<*mut UObjectBase>, String> {
    // SAFETY: per the contract; the property is the class's, checked to be an array (of widget
    // pointers, as the Blueprint declares it).
    unsafe {
        let used = int_variable(object, class, count)?.read();
        let property = reflection::find_property(class, array);
        if property.is_null() || !reflection::is_kind(property, "ArrayProperty") {
            return Err(format!("variable {array} not found"));
        }
        let widgets = &*object
            .cast::<u8>()
            .add((*property).offset_internal as usize)
            .cast::<TArray<*mut UObjectBase>>();
        if widgets.data.is_null() {
            return Ok(Vec::new());
        }
        let shown = used.clamp(0, widgets.num.max(0)) as usize;
        Ok(core::slice::from_raw_parts(widgets.data, shown)
            .iter()
            .copied()
            .filter(|widget| !widget.is_null())
            .collect())
    }
}

impl Grid {
    fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let grid_slot = class("GridSlot")?;
        Ok(Self {
            widget_slot: reflection::field_offset(class("Widget")?, "Slot", "ObjectProperty", 8)?,
            class: grid_slot.cast(),
            row: reflection::field_offset(grid_slot, "Row", "IntProperty", 4)?,
            column: reflection::field_offset(grid_slot, "Column", "IntProperty", 4)?,
            set_column: SingleCall::find(grid_slot, "SetColumn", "InColumn", "IntProperty", 4)?,
        })
    }

    /// `widget`'s grid slot, if it's in a grid.
    ///
    /// # Safety
    /// `widget` must be a live widget.
    unsafe fn slot(&self, widget: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; `Slot` is an object pointer of widgets, checked to be a grid
        // slot before use.
        unsafe {
            let slot = widget
                .cast::<u8>()
                .add(self.widget_slot)
                .cast::<*mut UObjectBase>()
                .read();
            (!slot.is_null() && reflection::is_a(slot, self.class)).then_some(slot)
        }
    }

    /// `slot`'s row and column.
    ///
    /// # Safety
    /// `slot` must be a live grid slot.
    unsafe fn place(&self, slot: *mut UObjectBase) -> (i32, i32) {
        // SAFETY: per the contract; both are `int32` fields at their reflected offsets.
        unsafe {
            let field = |offset| slot.cast::<u8>().add(offset).cast::<i32>().read();
            (field(self.row), field(self.column))
        }
    }
}

/// A `TArray` of structs whose size is known at run time.
#[repr(C)]
struct RawArray {
    data: *mut u8,
    num: i32,
    max: i32,
}
