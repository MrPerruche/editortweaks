//! Menu buttons' labels: which of the game's buttons a button is, whatever the player's language: the string table
//! key of its text. The game's Blueprints make their buttons with texts from string tables
//! (`CreateButton(LOCTABLE("Action_Duplicate"), ...)` in `WBP_UGCBrowser`), and the button shows
//! that text in its `TextBlock` (a reflected `UMenuButtonWidget` property): `UTextBlock::GetText()`,
//! then `UKismetTextLibrary::StringTableIdAndKeyFromText(FText, FName& OutTableId, FString&
//! OutKey)`, which says whether the text came from a table. All reflected. Found 2026-11-11 (game
//! 1.11.8, BRMK 1.11.6's `WBP_UGCBrowser`).

use std::cell::{Cell, RefCell};

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection::{self, Function, Parameters, SingleCall, take};
use crate::game::slider_text::FString;
use crate::game::text::Text;

/// What reading a button's key takes (after engine init).
pub struct ButtonKeys {
    /// `UMenuButtonWidget::TextBlock`.
    text_block: usize,
    /// `UTextBlock::GetText()` and its result.
    get_text: Function,
    text: usize,
    /// `StringTableIdAndKeyFromText`, on the library's default object, and its parameters.
    library: *mut UObjectBase,
    key_of: Function,
    key_text: usize,
    key: usize,
    found: usize,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for ButtonKeys {}
unsafe impl Sync for ButtonKeys {}

impl ButtonKeys {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let text_size = size_of::<Text>() as i32;
        let get_text = Function::find(class("TextBlock")?, "GetText")?;
        let key_of = Function::find(class("KismetTextLibrary")?, "StringTableIdAndKeyFromText")?;
        Ok(Self {
            text_block: reflection::field_offset(
                class("MenuButtonWidget")?,
                "TextBlock",
                "ObjectProperty",
                8,
            )?,
            text: get_text.parameter("ReturnValue", "TextProperty", text_size)?,
            get_text,
            library: reflection::find_object("Default__KismetTextLibrary", "KismetTextLibrary")
                .ok_or("KismetTextLibrary's default object not found")?,
            key_text: key_of.parameter("Text", "TextProperty", text_size)?,
            key: key_of.parameter("OutKey", "StrProperty", size_of::<FString>() as i32)?,
            found: key_of.parameter("ReturnValue", "BoolProperty", 1)?,
            key_of,
        })
    }

    /// The string table key of the text `button` shows (e.g. `"Action_Duplicate"`), if it's one.
    ///
    /// # Safety
    /// `button` must be a live `UMenuButtonWidget`. Game thread only.
    pub unsafe fn key(&self, button: *mut UObjectBase) -> Option<String> {
        // SAFETY: per the contract; the text block comes from a reflected object property, every
        // call gets its parameters at their reflected offsets. ProcessEvent destroys no
        // parameters: the text (new, from `GetText`) is released and the key (a new string of the
        // game's) freed here.
        unsafe {
            let text_block = button
                .cast::<u8>()
                .add(self.text_block)
                .cast::<*mut UObjectBase>()
                .read();
            if text_block.is_null() {
                return None;
            }
            let mut got = Parameters::new();
            self.get_text.call(text_block, &mut got);
            let text = take::<Text>(&got, self.text);
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.key_text, text);
            self.key_of.call(self.library, &mut parameters);
            take::<Text>(&parameters, self.key_text).release();
            let key = take::<FString>(&parameters, self.key);
            let shown = key.to_string();
            key.free();
            take::<bool>(&parameters, self.found).then_some(shown)
        }
    }
}

/// Showing or collapsing a menu button's label (its `TextBlock`), for buttons showing only their
/// icon: an empty label still takes its place (a Fill slot of `WBP_MenuButton`'s horizontal box,
/// after the icon's Auto one), which widens the button on the right.
///
/// Button widgets are pooled for the whole game (`UBrickObjectPool`): a panel gives back the
/// buttons it doesn't use in `RemoveUnusedButtons` (also run by `ClearButtons(false)` and by its
/// `NativeDestruct`, BRMK 0x3D6160), and any menu or popup may get them next, as they are. So a
/// collapsed label must be shown again before its button leaves: [`ButtonLabels::before_pooling`]
/// and [`ButtonLabels::after_pooling`] around every pooling (`features::menu_buttons`). Found
/// 2026-11-11: the main menu's buttons lost their labels after the browser's bar had used them.
pub struct ButtonLabels {
    /// `UMenuButtonWidget::TextBlock`.
    text_block: usize,
    /// `UWidget::Visibility`, `SetVisibility(ESlateVisibility)`, and `Collapsed`.
    visibility: usize,
    set_visibility: SingleCall,
    collapsed: u8,
    /// The visibility a label had before it was first collapsed (all labels have the Blueprint's).
    shown: Cell<Option<u8>>,
    /// The buttons whose label is collapsed by us.
    hidden_on: RefCell<Vec<*mut UObjectBase>>,
    /// Those of them the panel being pooled held before (see [`Self::before_pooling`]).
    leaving: RefCell<Vec<*mut UObjectBase>>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread (the cells
// too).
unsafe impl Send for ButtonLabels {}
unsafe impl Sync for ButtonLabels {}

impl ButtonLabels {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let widget = class("Widget")?;
        let collapsed = reflection::enum_value("ESlateVisibility", "ESlateVisibility::Collapsed")?;
        Ok(Self {
            text_block: reflection::field_offset(
                class("MenuButtonWidget")?,
                "TextBlock",
                "ObjectProperty",
                8,
            )?,
            visibility: reflection::field_offset(widget, "Visibility", "EnumProperty", 1)?,
            set_visibility: SingleCall::find(
                widget,
                "SetVisibility",
                "InVisibility",
                "EnumProperty",
                1,
            )?,
            collapsed: u8::try_from(collapsed).map_err(|_| format!("Collapsed is {collapsed}"))?,
            shown: Cell::new(None),
            hidden_on: RefCell::new(Vec::new()),
            leaving: RefCell::new(Vec::new()),
        })
    }

    /// Shows `button`'s label (as it was) or collapses it.
    ///
    /// # Safety
    /// `button` must be a live `UMenuButtonWidget`. Game thread only.
    pub unsafe fn show(&self, button: *mut UObjectBase, shown: bool) {
        // SAFETY: per the contract; the text block comes from a reflected object property, its
        // visibility is a byte at its reflected offset.
        unsafe {
            let label = button
                .cast::<u8>()
                .add(self.text_block)
                .cast::<*mut UObjectBase>()
                .read();
            if label.is_null() {
                return;
            }
            {
                let mut hidden_on = self.hidden_on.borrow_mut();
                hidden_on.retain(|&known| known != button);
                if !shown {
                    hidden_on.push(button);
                }
            }
            let now = label.cast::<u8>().add(self.visibility).read();
            if now != self.collapsed && self.shown.get().is_none() {
                self.shown.set(Some(now));
            }
            let wanted = match (shown, self.shown.get()) {
                (false, _) => self.collapsed,
                (true, Some(visible)) => visible,
                // Never collapsed by us: as it is.
                (true, None) => return,
            };
            if now != wanted {
                self.set_visibility.call(label, wanted);
            }
        }
    }

    /// A button panel is about to pool the buttons it doesn't use: `held`, all it holds now.
    pub fn before_pooling(&self, held: &[*mut UObjectBase]) {
        let hidden_on = self.hidden_on.borrow();
        *self.leaving.borrow_mut() = held
            .iter()
            .copied()
            .filter(|button| hidden_on.contains(button))
            .collect();
    }

    /// The panel pooled them, keeping `kept`: the buttons with a collapsed label that left get it
    /// back.
    ///
    /// # Safety
    /// Right after [`Self::before_pooling`], for the same panel (its pooled buttons are still
    /// live). Game thread only.
    pub unsafe fn after_pooling(&self, kept: &[*mut UObjectBase]) {
        let leaving = core::mem::take(&mut *self.leaving.borrow_mut());
        for button in leaving {
            if !kept.contains(&button) {
                // SAFETY: per the contract, a live pooled button.
                unsafe { self.show(button, true) };
            }
        }
    }
}
