# Settings menu: a sub-menu of the options, and rows of our own

How the game's menu opens its contexts and shows its settings pages, and how
`features/settings_menu` adds an "Editor tweaks" sub-menu to the options, made like the game's own
(Gameplay, User interface, ...), whose page lists the mod's settings (`game::menu`,
`game::property_reflection`). Found in BRMK 1.11.6 (exports, and `WBP_Menu`'s Blueprint code read
with `et.py blueprint`), then in game 1.11.7 (build 25831956) on 2026-10-14.

## Contexts

The main menu and the in-game menu are one widget, `WBP_Menu` (`UMenuWidget`). It shows a
context, a path like `MainMenu/Options/Gameplay` (parts separated by `/`: `SplitMenuContext`
compares each character with 0x2F): buttons, and for some a page beside them.
`UMenuWidget::OpenMenu(FName InMenu)` (reflected, by value; BRMK 0x3E08F0, game 0xEEBF30, called
last by its reflected thunk, game 0xFE6590: our way to find it) opens one:

1. A name not starting with the root (`MainMenu` in the main menu, else `InGameMenu`) is relative:
   `<current>/<name>`. The game's buttons open their sub-context that way, e.g.
   `OnClickedGameplaySettings` is `OpenMenu('Gameplay')`.
2. The same context as the current one: nothing happens.
3. Otherwise it stores the context (game +0x28C, not reflected), resets its button panel's count of
   used buttons, cancels a page still loading and removes the current page.
4. `SplitMenuContext` (game 0xEFA5A0) splits it into parent and last part; a context with a parent
   gets a Back button (`CreateBackButton`, whose Blueprint makes it with
   `BP_MenuFunctionLibrary.CreateBackButton`, bound to `OnClickedBack`).
5. `UModHookSubsystem::ForEachModHook` calls each Blueprint mod's `UModHook::OverrideMenu(menu, last
   part) -> bool`; if the last one returns false (the result is assigned, not combined, so the
   last hook decides), the Blueprint event `OnMenuContextChanged(last part)` runs (the C++ stub,
   game 0xFE4800).
6. `UMenuButtonPanelWidget::RemoveUnusedButtons` (game 0xEF3F00), `UpdateInputMode`,
   `PlayMenuTransition`, `UpdateMenuSequenceButtons`.

`StepBack` (`OnClickedBack`; game 0xEFB4B0) first asks the current page (`UMenuPageWidget::StepBack`,
true if it handled it), then reopens the parent with `OpenMenu`; at the root it asks to quit (main
menu) or closes the menu.

`WBP_Menu`'s `OnMenuContextChanged` switches on the last part: `MainMenu` and `InGameMenu` make the
root buttons; `Options` sets the title (`ShowMenuPage` without a class) and makes Gameplay, User
interface, Video, Input and Key bindings (`OnClicked*Settings`, `OnClickedInputMappings`); `Host`,
`Join`, `Gameplay`, `UI`, `Video`, `Input`, `InputMappings`, `Credits`, `Match`, `Admin`, `Players`
each show their page (`ShowMenuPage(WBP_..., LOCTABLE title)`); any other name does nothing (only
the Back button). `OnClickedEditorSettings` opens `EditorSettings`, which `WBP_Menu` doesn't handle.

## Buttons

`UMenuButtonPanelWidget` (the menu's `ButtonPanel`, reflected on `UMainWidgetBase`) keeps its
buttons in `Buttons` (reflected array, game +0x260) and how many the current context uses (game
+0x270). `CreateButton(FText DisplayText, TSoftObjectPtr<UTexture2D> IconTexture, const
FOnClickedMenuButton& Delegate) -> UMenuButtonWidget*` (reflected, parameters 0x58 bytes) reuses
`Buttons[used]` or gets one from `UBrickObjectPool`, initializes it and places it in the grid by
its index (row = index / `NumButtonsPerRow`): buttons show in the order made, Back first.
`RemoveUnusedButtons` pools (`UBrickObjectPool::PoolWidget`) the buttons past the used count and
shrinks the array: their widgets get reused, by this panel or another (popups have their own).
Icons are under `/Game/BrickRigs/UI/Textures/Icons/` (ours: `T_VehicleEditor`).

A click runs `UMenuButtonWidget::OnClicked` (reflected, bound by `UMenuButtonWidget::Initialize`;
its thunk, game 0xFE6430, jumps to game 0xEE7120, the exe's only way there), which executes the
button's delegate (game +0x288, not reflected) if bound.

## Pages

`UMenuWidget::ShowMenuPage(TSoftClassPtr<UMenuPageWidget> WidgetClass, FText TitleText)`
(reflected, parameters 0x40 bytes) sets the title and loads the class asynchronously; once loaded
(BRMK callback 0x377760), the page widget comes from `UBrickObjectPool::GetWidget` (pages are
pooled too), gets its owning menu, then replaces the current page: the old one is removed
(`RemoveMenuPage`), `CurrentMenuPage` (reflected) set, and `AddMenuPage` (Blueprint: adds it to
`MenuPageBorder`, which constructs it).

Settings pages are `UMenuSettingsPageWidget`s: `/Game/BrickRigs/UI/Menu/Settings/WBP_GameplaySettings`,
`WBP_UserInterfaceSettings`, `WBP_InputSettings`, `WBP_VideoSettings` (`WBP_InputMappings` is a
`UInputSettingsPageWidget`). `WBP_GameplaySettings` has no Blueprint code: it is the C++ page with
its `SettingsCategories` (reflected `FGameplayTagContainer`, `GameplayTags` first, a tag being an
`FName`) set to `Properties.Settings.Gameplay`. Its `NativeConstruct` (and `UpdateProperties`)
clears its `PropertiesPanel` and calls `AddProperties` (a `BlueprintNativeEvent`; its thunk, game
0xFCD790, jumps through the virtual table, +0x488, to `AddProperties_Implementation`), which copies
the categories into a filter and has the panel list `UBrickUserSettings`' properties of those
categories (`UPropertiesPanelWidget::AddPropertiesForSelection`).

## Our sub-menu

The game does the work; we add our button and page where `WBP_Menu`'s `OnMenuContextChanged` adds
its own, through a hook of `RemoveUnusedButtons`, which `OpenMenu` calls right after it (and a hook
of `OpenMenu` noting which menu opens which context meanwhile; other panels' calls are ignored):

- When a menu's context `.../Options` is made, we add our button (`CreateButton` with an empty
  delegate) after the game's, before the panel pools the unused ones, so it is laid out, pooled
  and animated like theirs. A hook of the C++ `OnClicked` tells its clicks: the button we made,
  still in that menu's `Buttons` (its widget may be reused elsewhere once pooled). A click opens
  `OpenMenu('EditorTweaks')` through reflection, as the game's buttons do.
- When a menu's context `.../Options/EditorTweaks` is made (the menu made its Back button and its
  Blueprint nothing), we show our page: `ShowMenuPage(WBP_GameplaySettings, "Editor tweaks")`.
- A hook of `AddProperties_Implementation`: the page that is the `CurrentMenuPage` of a menu in our
  context lists our tag `Properties.Settings.EditorTweaks` (unknown to the game's tag manager; the
  filter only compares names) instead of its own categories, only for the time of the call: the
  pooled page keeps its own for when it is the gameplay page again.

Not used: a `UModHook` of our own (registered in `UModHookSubsystem::ModHooks`) gets
`OverrideMenu` at the right time, but its result would decide for every context whether the game
makes the menu (the last hook's result wins), and a button made there would come before the
game's.

## Reflection: what a panel lists

A panel lists what `IBrickPropertyInterface::ReflectBrickProperties(FBrickPropertyReflection&)`
adds. brickrust declares `FBrickPropertyReflection` (`br/properties/reflection.rs`): +0
`is_serializing` (listing properties to save or load them, not to show them), +0x20 the filter's
tags, +0x40 the properties (instances), +0x50 the edit infos, +0x60 the categories' names, +0x70
the current category, +0x78 the parent path. `UBrickUserSettings::ReflectBrickProperties` (BRMK
0x34D8F0, game 0xE8E380, the one function loading `L"Name_Menu_Gameplay"`) goes through its
categories:

```text
if (Reflection.ShouldReflect(Tags.Gameplay))            // game 0xE721C0: is the tag's FName in +0x20
    if (!Reflection.is_serializing)
        Reflection.AddCategory(LOCTABLE("UserInterface", "Name_Menu_Gameplay"));
    static TSharedPtr<FBoolBrickProperty> P = MakeShared(...), P->Field = FindPropertyByName("bAutoCounterSteering");
    if (Reflection.is_serializing)
        Reflection.AddPropertyName(P, "bAutoCounterSteering");            // game 0xCCCF00
    else
        Info = Reflection.AddProperty(P, "bAutoCounterSteering", LOCTABLE("Game", "Name_Setting_AutoCounterSteering"));
        Info->Description = LOCTABLE("Game", "Description_Setting_AutoCounterSteering");   // +0x80
```

- `AddCategory(const FText&)` (our name; BRMK 0x85C60, game 0xCCD8B0): makes the category with
  that name current, adding it if new.
- `AddProperty(const TSharedRef<FBrickProperty>&, const FString& Name, const FText& DisplayName)`
  (our name; BRMK 0x85540, game 0xCCCFB0): makes the property's instance (with the reflection's
  path), then a shared `FBrickPropertyEditInfo` (game 0xCC1C60, 0xE8 bytes with the reference
  counts; neither reads the property's field), appends it to +0x50 and returns it. Its display
  name is at +0x40, the description at +0x80, a flag +0xB1 (see "How a list shows"), color style
  +0xB2 (unconfirmed), and how a list shows at +0xB4 and +0xB8.
- `UBrickUserSettings::PostModifyBrickProperty` (after a value is set from the panel) saves the
  game's config and compares the changed property's name with its own (volumes, HUD, ...): other
  names do nothing there.

## Brick properties

A brick property is `{vtable, FProperty* Field, FName Name}` (bool, 0x18 bytes; an enum adds two
`TFunction`s up to 0x38), made by `MakeShared` (the reference controller first: vtable, shared
and weak counts; not thread-safe counts for brick properties). The table (brickrust's
`FBrickPropertyVTable`, then the typed functions):

| Slot | Bool (game) | Enum (game) | What |
|---|---|---|---|
| 0, 1 | 0xCE8900 (both) | 0xCE8870, 0xCE90E0 | type names, `"FBoolBrickProperty"` / `"FEnumBrickProperty"`, `"FEnumBrickPropertyBase"` |
| 2, 3 | 0xCEC260, 0xCE8590 | 0xCEC140, 0xCE83E0 | is of type, type hierarchy (also load the name) |
| 4 | 0xCCC550 | 0xD28420 | deleting destructor |
| 6 | 0xCD28A0 | 0xCD28A0 | compare two values (bytes) |
| 7 | 0x82C0C0 | same | has a field |
| 8 | 0xE5CD80 | same | same value in two containers (reads through the field) |
| 9 | 0xEA0F40 | 0xEA1330 | serialize (through the field's `SerializeItem`; `{2, false}` without one) |
| 11, 12, 14, 16 | | | value as text, from text, export, import: through 18 and 19 |
| 18 | 0xCE8D20 | 0xCE8BA0 | get: `Container.Path.Last() (or Root) + Field->Offset_Internal (+0x4C)`, a byte (an enum's as `int32`) |
| 19 | 0xD083D0 | 0xD08170 | set: a byte (an enum asks 21 first) |
| 20 | | 0x7FC0F0 | the list's items: calls the `TFunction` at +0x18 |
| 21 | | 0xCD54F0 | whether a value may be set: the `TFunction` at +0x28, if any |

Tables are found by type name (`framework::sig::VtableSig`): the tables of exactly 20 (bool) or 22
(enum) functions whose slots 0, 2 and 3 load the ASCII name. The bool one is unique (0x38D4AF8);
the enum one has two identical copies (0x38E4928, 0x3917568: they differ in slots 4, 6 and 21) and
a longer subclass table (0x38DC3E8, 24 slots). Ours copy the table and replace 4, 6, 7, 8, 9, 18,
19 (and 20, 21) with functions reading and writing our settings; the field stays null. The game's
own functions in the other slots only go through those.

Since 2026-10-15 the hook of `ReflectBrickProperties` is shared (`features/settings_list.rs`): a
feature adds a listing (a tag and its sections), so the editor's side panel gets the map's rows
under `Properties.Editor` too (`docs/re/map-in-editor.md`). A row's value is a check box or a list
read and written through our functions (`property_reflection::Check`, `List`): one of the mod's
settings (saved when changed), or a feature's own value (the map's garage, Show map). A list's
items are made each time the game asks for them (slot 20), their names as new texts released once
copied into the items; at most 256, since slots 19 and 21 take the index as a byte.

Sections of the same title merge only if the title is the very same text: `AddCategory` (game
0xCCD8B0) looks for an existing category with game 0x10A18B0, `FText::IdenticalTo` (same text data,
or same display string pointer), not an equal string. Two texts made from "References" (the map's
and the references') gave two sections; `settings_list` now makes one text per title (2026-10-17).
A listing can also *replace* the panel's rows (`settings_list::replace`): while its condition holds,
a panel asking for its tag gets only its sections, without the game's user settings or the other
listings (the references' rows while references are selected). The game's function isn't called
then, which is safe only because a reflection that saves or loads (`is_serializing`) shows no tag.

Enum items: `FEnumPropertyItem` (game 0xA8 bytes): value `int32` +0, `FDisplayInfo` +8 (name
`FText`, then the icon's `TSoftObjectPtr` with its weak object index -1 at +0x20), three texts
+0x48, +0x60, +0x78 (empty in the game's lists), a pointer +0x90 (null), two flags +0x98, +0x99
(false) and one +0xA0 (true), as `TransmissionMode`'s list makes them (game 0xE6C4B0, with
`FDisplayInfo`'s constructor 0xCC42F0 and the item's copy constructor 0xCC4380).

## How a list shows

Found 2026-10-15. An enum row is a `UEnumPropertyWidget` (game 1.11.7: its edit info at +0x268,
its `UBrickComboBoxWidget` at +0x288). Its `UpdateValue` (BRMK 0x3FCD70, game 0xF03A30) lists the
items (slot 20, keeping those slot 21 allows), then passes two fields of the row's edit info to
the combo box (the reads at game 0xF03D83):

| Edit info (game) | BRMK | Default | Combo box |
|---|---|---|---|
| +0xB1 flag | +0xB9 | false | if set, `SetMaxListItems(0)` instead of the next field |
| +0xB4 `int32` | +0xBC | -1 | `SetMaxListItems` (BRMK 0x450D50, combo +0x478 there) |
| +0xB8 `int32` | +0xC0 | 0 | `SetMaxItemsPerRow` (BRMK 0x450CE0) |

`UBrickComboBoxWidget::InitItemsInternal` (BRMK 0x437C20) then shows the items side by side, as
buttons in rows of `MaxItemsPerRow` (0: one row), if `MaxListItems` is -1, or isn't 0 and the
count is at most `MaxListItems`; else a dropdown (the selected item, opening a list:
`UWindowManagerWidget::OpenComboBoxList`). Its `UpdateUseItemList` event tells the Blueprint.
The defaults (-1, 0) put every item in one row, so a long list's buttons get very thin. The
game's long lists set the field after `AddProperty`: the siren brick's `SirenType` to 0, always a
dropdown (BRMK 0x178E08); the match's `Weather` to 10 with 6 per row (BRMK 0x34D867); the
editor's view mode only 3 per row (BRMK 0x355BAA).

Ours (`property_reflection::List::dropdown`, the map's garage list) set +0xB4 to 0 like the
siren's, only if the edit info still holds -1 there and false at +0xB1 (else the log says so once
and the list keeps its buttons). +0xB1 isn't written: the widget treats it as "always a dropdown",
but other code may read it as something else (a read-only row, perhaps).

## Settings file

`framework::settings` keeps our settings in
`%LOCALAPPDATA%\BrickRigs\SavedRemastered\Config\WindowsNoEditor\EditorTweaks.ini`
(`[EditorTweaks]`, `Key=True`/`False` or an option's key), read at start-up and written whenever
one changes in the menu. Unknown keys and values are ignored (and logged).

## Checked in game

- 2026-10-14, first version (the page shown in the options' own context, the button made after
  `OpenMenu` returned): the page opened and listed our rows (our properties' null field didn't
  crash it). The button went missing after visiting a sub-menu and coming back, likely because it
  was made past the panel's pooling and transition; hence the rework above.
- 2026-10-14, the rework: our button last among the options', our sub-menu with its Back button
  and page fine. But the button was missing once back in the options: Back reopens the parent by
  its full path (`StepBack`: `OpenMenu(MainMenu/Options)`), and we took the last part after a `.`
  (the separator is `/`), so it wasn't seen as the options. Found with a trace of the menus'
  buttons (each context opened, the buttons kept, ours with its visibility and grid cell); fixed.
- 2026-10-14, with the fix: the button stays after Back. `EditorTweaks.ini` was written (only a
  change in the menu writes it), so toggling saves.
