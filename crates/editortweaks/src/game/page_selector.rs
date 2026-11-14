//! A paged list's page selector (`UPageSelectorWidget`, Blueprint `WBP_PageSelector`): its previous
//! and next buttons, and turning the page as they do.
//!
//! `UPagedListWidget::PageSelector` (reflected) holds it. `WBP_PageSelector`'s buttons are its
//! Blueprint variables `PrevButton` and `NextButton`; their clicks call
//! `UPageSelectorWidget::GotoNextPage(bForward, bToEnd)` (reflected; BRMK 1.11.6 0x435120) with
//! (false, false) and (true, false). It moves the page by one, clamped to the pages there are, and
//! broadcasts `OnPageChanged`, which the list answers by loading that page: a click, without the
//! mouse. `UpdateButtons` disables the button that can't go further. See `docs/vehicle-menu.md`,
//! "Drag and drop".

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{self, BoolFunction, Function, Parameters};
use crate::game::ui_style::{self, ButtonColorStyle, ColorStyleValues};

/// Which way a page button goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Turn {
    Back,
    Forward,
}

/// Where the Blueprint keeps its buttons (found on the first live selector: its class is a
/// Blueprint's, loaded with the menu).
#[derive(Clone, Copy)]
struct ButtonFields {
    previous: usize,
    next: usize,
}

/// The page buttons' look while something dragged is over them (optional).
struct Highlight {
    style: ButtonColorStyle,
    styles: ColorStyleValues,
    /// `UButtonWidgetBase`: only its buttons get a color style.
    base: *const UStruct,
}

pub struct PageSelectors {
    /// `UPagedListWidget::PageSelector`.
    selector: usize,
    goto_next: Function,
    /// `GotoNextPage`'s parameters: forward, to the end.
    forward_at: usize,
    to_end_at: usize,
    is_enabled: BoolFunction,
    buttons: OnceLock<Result<ButtonFields, String>>,
    highlight: Option<Highlight>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for PageSelectors {}
unsafe impl Sync for PageSelectors {}

impl PageSelectors {
    /// After engine init.
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let list = class("PagedListWidget")?;
        let goto_next = Function::find(class("PageSelectorWidget")?, "GotoNextPage")?;
        let offsets = goto_next.parameter_offsets(&["BoolProperty", "BoolProperty"])?;
        let highlight = (|| -> Result<Highlight, String> {
            Ok(Highlight {
                style: ui_style::button_color_style()?,
                styles: ui_style::color_style_values()?,
                base: class("ButtonWidgetBase")?.cast(),
            })
        })()
        .ok();
        Ok(Self {
            selector: reflection::field_offset(list, "PageSelector", "ObjectProperty", 8)?,
            forward_at: offsets[0],
            to_end_at: offsets[1],
            goto_next,
            is_enabled: BoolFunction::find(class("Widget")?, "GetIsEnabled")?,
            buttons: OnceLock::new(),
            highlight,
        })
    }

    /// The page selector of the paged list `list`.
    ///
    /// # Safety
    /// `list` must be a live paged list. Game thread only.
    unsafe fn selector(&self, list: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset is reflected.
        let selector = unsafe {
            list.cast::<u8>()
                .add(self.selector)
                .cast::<*mut UObjectBase>()
                .read()
        };
        (!selector.is_null()).then_some(selector)
    }

    /// Whether `button` can be clicked (the selector disables the one that can't go further).
    ///
    /// # Safety
    /// `button` must be a live widget. Game thread only.
    pub unsafe fn is_enabled(&self, button: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.is_enabled.call(button) }
    }

    /// The previous and next buttons of `list`'s page selector.
    ///
    /// # Safety
    /// `list` must be a live paged list. Game thread only.
    pub unsafe fn buttons(&self, list: *mut UObjectBase) -> Vec<(*mut UObjectBase, Turn)> {
        // SAFETY: per the contract.
        let Some(selector) = (unsafe { self.selector(list) }) else {
            return Vec::new();
        };
        let fields = self.buttons.get_or_init(|| {
            // SAFETY: a live object; its class (the Blueprint's) lives as long as it.
            let class = unsafe { reflection::class_of(selector.cast()) }.cast::<UClass>();
            let field = |name| reflection::field_offset(class, name, "ObjectProperty", 8);
            Ok(ButtonFields {
                previous: field("PrevButton")?,
                next: field("NextButton")?,
            })
        });
        let Ok(fields) = fields else {
            return Vec::new();
        };
        [(fields.previous, Turn::Back), (fields.next, Turn::Forward)]
            .into_iter()
            .filter_map(|(offset, turn)| {
                // SAFETY: the selector is live; the offset is its class's reflected field, an
                // object pointer (a widget of its tree, alive with it).
                let button = unsafe {
                    selector
                        .cast::<u8>()
                        .add(offset)
                        .cast::<*mut UObjectBase>()
                        .read()
                };
                (!button.is_null()).then_some((button, turn))
            })
            .collect()
    }

    /// Turns `list`'s page as its button would (nothing past the first or last page).
    ///
    /// # Safety
    /// `list` must be a live paged list. Game thread only.
    pub unsafe fn turn(&self, list: *mut UObjectBase, turn: Turn) {
        // SAFETY: per the contract; both bools at their reflected offsets (a native `bool` is a
        // whole byte).
        unsafe {
            let Some(selector) = self.selector(list) else {
                return;
            };
            let mut parameters = Parameters::new();
            reflection::put(
                &mut parameters,
                self.forward_at,
                u8::from(turn == Turn::Forward),
            );
            reflection::put(&mut parameters, self.to_end_at, 0u8);
            self.goto_next.call(selector, &mut parameters);
        }
    }

    /// The highlight, if `button` can show it (a `UButtonWidgetBase`).
    ///
    /// # Safety
    /// `button` must be a live widget.
    unsafe fn highlight_of(&self, button: *mut UObjectBase) -> Option<&Highlight> {
        let highlight = self.highlight.as_ref()?;
        // SAFETY: per the contract.
        unsafe { reflection::is_a(button, highlight.base) }.then_some(highlight)
    }

    /// `button`'s color style now (one of [`PageSelectors::buttons`]), to give back after
    /// highlighting it; `None` if it can't be restyled.
    ///
    /// # Safety
    /// `button` must be a live widget. Game thread only.
    pub unsafe fn style(&self, button: *mut UObjectBase) -> Option<u8> {
        // SAFETY: per the contract; read only from a `UButtonWidgetBase`.
        unsafe {
            let highlight = self.highlight_of(button)?;
            Some(highlight.style.get(button.cast()))
        }
    }

    /// Shows `button` as a drop target (`None`), or with its own `style` again.
    ///
    /// # Safety
    /// `button` must be a live widget. Game thread only.
    pub unsafe fn show(&self, button: *mut UObjectBase, style: Option<u8>) {
        // SAFETY: per the contract; restyled only if it's a `UButtonWidgetBase`.
        unsafe {
            if let Some(highlight) = self.highlight_of(button) {
                let style = style.unwrap_or(highlight.styles.highlight);
                highlight.style.set(button.cast(), style);
            }
        }
    }
}
