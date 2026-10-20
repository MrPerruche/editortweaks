# Input bindings: the game's input actions and rebinding

How the game's input actions, their default keys, the player's rebound keys and the input settings
fit together, and how the mod adds rebindable actions of its own (`features/keybinds`,
`game::input_table`, `game::player_input`). Found in BRMK 1.11.6 (exports, `InputTable.uasset`),
then in game 1.11.7 (build 25831956) on 2026-10-10. BRMK is an editor build: its `FName` is 12
bytes (case-preserving names), so its struct offsets differ more than usual from the shipping game.

## Input tables

Every action is a row of an input table: a `UDataTable` whose `RowStruct` is `FInputTableRow`
(reflected). The game has one, `/Game/BrickRigs/Input/InputTable` (row names = action names, e.g.
`MovePerpendicular`). `FInputTableRow`'s reflected fields (names as in the shipping exe):

- `PrimaryDisplayInfo`, `SecondaryDisplayInfo` (`FDisplayInfo`: `Name` text, `IconTexture`),
  `PrimaryActionDescription`, `SecondaryActionDescription` (texts), `ActionIndex` (byte),
  `bDisplayActionIndex`;
- `Type` (`EInputRowType`, a byte): `PressAction`, `ReleaseAction`, `PressReleaseAction`,
  `HoldAction`, `ToggleAction`, `Axis`, `HalfAxis`. Values below 5 are actions (`Action_<Row>`),
  the others axes (`Axis_<Row>`). Keys held for an effect are `PressReleaseAction`
  (`MovePerpendicular`, `PivotCamera`, `Select`); `HoldAction` means "tap for the primary action,
  hold for the secondary one" and gets two rows in the settings (as do axes);
- `SortPriority`, `ContextTags` (`FGameplayTagContainer`), `DefaultMappings`
  (`TArray<FDefaultInputMapping>`: `bShift`, `bCtrl`, `bAlt`, `bCmd`, `Key` (`FKey`), `Scale`),
  `InputCategory` (`UInputCategory*`), `bConsumeInput`, `bCanBeRebound`, `bHideFromInputHelp`,
  `bCloseInputHelp`, `bIsClickable`, `bCallAxisWhenZero`.

Which default keys are taken: `InputTable.uasset`'s name map lists every key name a default
mapping uses, and each `Key` value references one by index (2026-10-11, choosing R for
`EditorTweaks_NextOrientation`). R is used twice, by rows outside the editor's categories (near
`Reload` with context `Input.Game.Pawn.Player`, and a `PressReleaseAction` with
`Input.Game.PlayerController`, row name not decoded): no editor action has it by default.

`UInputCategory` (a `UPrimaryDataAsset`, assets in `/Game/BrickRigs/Input/Categories`):
`DisplayInfo`, `ParentCategory`, `bShowInInputHelp`. Top categories (e.g. `IC_BrickEditor`, the
vehicle editor) have no parent; subcategories (`IC_BrickEditor_Move`, ...) point to one.

`UBrickAssetManager` keeps the tables and categories in reflected arrays, `InputDataTables` and
`InputCategories` (game +0x490 / +0x4A0). `UBrickAssetManager::LoadAssetLibraries
(ELoadAssetLibrariesMode)` (BRMK 0x2B9C70, game 0xE2CF70; called by `PostInitialAssetScan`,
`UBrickGameInstance::Init` and `::Shutdown`) refills them: every `UDataTable` under `Input` whose
`RowStruct` is `FInputTableRow`, then every `UInputCategory` under `Input/Categories` without a
parent (+0x50 in BRMK) that some row's category leads to, sorted. It replaces the arrays, so
anything added must be added again after each call. `ForEachInputTableRow`, `GetInputTableRow(s)`
read all the tables. The mod finds it by the code loading `"Input/Categories"` (an ASCII string in
the shipping exe: `StringRefSig` only handles UTF-16), signature `input_table::LOAD_ASSET_LIBRARIES`.

## Binding rows to handlers

`UBaseInputComponent::SetupInputComponent` (BRMK 0x25CF30) goes through every row and skips those
whose `ContextTags` don't match the component's (`FGameplayTagContainer::FilterExact` with the
tags from its virtual +0x4F8), then `FindFunction("Action_<Row>")` (`"Axis_<Row>"` for axes): if
missing, an `Error condition failed: FindFunction(ActionFunctionName)` check that only logs (and
breaks into a debugger) and skips the row, then `BindAction`. Rows with no context tags are never
bound, which is what the mod's rows are: the mod reads their keys itself.

## The player's bindings

`UBrickPlayerInput` (the `UPlayerInput` of `ABrickPlayerController`, reflected
`APlayerController::PlayerInput`) overrides `ForceRebuildingKeyMaps` (BRMK 0x319BD0 ->
`ForceRebuildingKeyMapsInternal` 0x319BE0):

```text
UPlayerInput::ForceRebuildingKeyMaps(bRestoreDefaults);   // copies UInputSettings' mappings in
Saved = copy of ActionMappings / AxisMappings;            // the player's saved ones
Empty them;
for (Row : GetInputTableRows())                           // every table
    for (Mapping : Row.DefaultMappings)
        if (Row.Type < Axis) ActionMappings.Add({RowName, Mapping.bShift.., Mapping.Key});
        else AxisMappings.Add({RowName, Mapping.Scale, Mapping.Key});
if (!bReset) replace each action's defaults by its saved mappings, if any;
```

So the bindings are `FInputActionKeyMapping`s (reflected struct, 0x28 bytes: `ActionName` +0,
`bShift`/`bCtrl`/`bAlt`/`bCmd` bits 0-3 of +8, `Key` +0x10) named after the rows, in a non-reflected
`TArray` at game +0x140 (BRMK +0x148). `IsInputActionKeyPressed(FName, const FKey&)` (a `UFUNCTION`;
BRMK 0x331B20, game 0xE75440, a leaf function called by its thunk 0xF9C6F0) only tells whether the
key is bound to the action (no modifiers, not whether it's held); the mod reads the array's offset
from it (`player_input::ACTION_KEY`, a `CodeShape`; the same code is at 0x2691A10 too).

Rebinding (`UpdateInputMapping`, then `PostEditInputMappings`, BRMK 0x33DBD0) collects every row's
current mappings (`ForEachInputTableRow`) into `UInputSettings` (`ClearActionMappings`,
`AddActionMappings`, the same for axes) and saves them (`SaveKeyMappings`, `Input.ini` in the
user's config). Rows missing at that moment (e.g. the mod's, when playing without it) lose their
saved keys. `ResetDefaultInputMappings` (the settings' "revert") rebuilds from the defaults.

Conflicts (`GenerateInputActionConflicts`, BRMK 0x31A6D0) are only looked for between rows with a
common context tag (BRMK 0x3AB530 both ways) and the same byte at BRMK row +0x108; `InteractPri` /
`InteractAxis` are special-cased, modifier-only keys too (`IsModifierKey`). Rows without context
tags never conflict.

## The input settings

The input settings page is a brick property panel over `UBrickUserSettings`
(`ReflectBrickProperties`, BRMK 0x34D8F0): `InputCategory` (a dropdown of the asset manager's
`InputCategories`), `InputMappingSearchText`, then one property `InputMappings.<Row>` per row of
the chosen category (`UInputMappingPropertyWidget`), named by `PrimaryDisplayInfo.Name` ("{0} {1}"
with the action index if `bDisplayActionIndex`), described by `PrimaryActionDescription`; two for
`HoldAction` and axis rows. Labels in `Content/Localization/StringTables/Input.csv` use short
sentence case: "Move perpendicular", "Add to selection".

Which rows, and their sections (BRMK, read 2026-10-14):

- A row is listed if it's rebindable (`bCanBeRebound`, BRMK row +0x109) and, with no search text,
  its category leads to the chosen one (BRMK 0x3BB260 lists the row's category and its parents),
  else if its labels contain the search text (the predicate inside the filter BRMK 0x2F3200, called
  by `ReflectBrickProperties` at 0x35279E on `GetInputTableRows`'s map). Every row of the game's
  table is rebindable, `bHideFromInputHelp` ones too: `Select` and `AddToSelection` are listed
  under Brick editor > Selection, `MoveCamera` under Camera.
- The dropdown's options are `GetInputCategories`, in the array's order.
- A row belongs to the chosen category if its `InputCategory` leads to it:
  `UInputCategory::ResolveInputCategory(Object, bTopMost)` (BRMK 0x2D9B20) follows
  `ParentCategory` up to the top, however deep.
- The rows are sorted (predicate BRMK 0x3F1420): rows of different categories by
  `UInputCategory::SortInputCategory` (BRMK 0x2DF820), else by the row's `SortPriority`, then
  more. `SortInputCategory(A, B)` puts a parent before its direct children, then compares the
  categories' `SortPriority` (reflected `int32`, BRMK +0xA4; the assets set it on a few
  subcategories, e.g. 10 on `IC_BrickEditor_File`), then their object names (case-insensitive).
- Each row's own category's `DisplayInfo.Name` is a property category of the panel, made at its
  first row: a section with that title. So subcategories are sections, in that order.

## Adding rows at run time

`UDataTable` in the shipping game (vtable 0x3ECFF60, found through the class's
`GetPrivateStaticClass`, whose class constructor calls the C++ constructor storing it): `RowStruct`
+0x28, `RowMap` (`TMap<FName, uint8*>`) +0x30, size 0xB0. Its own virtual functions follow
`UObject`'s 78: +0x270 `GetNonConstRowMap`, +0x278 `AddRowInternal` (`RowMap.Add(Name, Row)`),
+0x280/+0x288 `GetRowMap`, +0x290 (returns false), +0x298 `EmptyTable`, +0x2A0 `RemoveRow`, +0x2A8
`AddRow` (0x251C520):

```text
Row = FMemory::Malloc(RowStruct->GetStructureSize());   // PropertiesSize +0x58, MinAlignment +0x5C
RowStruct->InitializeStruct(Row, 1);                    // UScriptStruct vtable +0x2B0
RowStruct->CopyScriptStruct(Row, &RowData, 1);
this->AddRowInternal(RowName, Row);                     // vtable +0x278
```

The mod builds its rows the same way without the copy: game allocator, `InitializeStruct` (the
struct's C++ constructor, e.g. `FTableRowBase`'s vtable and empty texts), fields written through
reflection (texts assigned with a reference each, `DefaultMappings` elements constructed the same
way with only `Key.KeyName` set; `FKey` looks its details up when needed), then `AddRowInternal`.
Both slots are read from `AddRow` (`input_table::ADD_ROW`). The table and its categories are
made with `StaticConstructObject_Internal` (outer: the asset manager; `RF_Transient |
RF_MarkAsRootSet`).

The mod's rows: `PressReleaseAction`, no context tags, `bCanBeRebound`, `bHideFromInputHelp`,
default mappings with their modifier keys (`FDefaultInputMapping::bShift`, ..., e.g. Ctrl + right
mouse button for drag select). Their
categories: a top one, `IC_EditorTweaks` ("Editor tweaks"), which the mod adds to
`InputCategories` after each load, right after `IC_BrickEditor` (last if that isn't there); and
inside it one subcategory per section (`IC_EditorTweaks_FastEditing`, ...), its `SortPriority`
giving the sections' order (since 2026-10-19 the sections follow the changelog's headers: Fast
editing, User interface, Classic building & movement, Selection, References). Until 2026-10-14 they were a single subcategory of `IC_BrickEditor`.
The selected category (`UBrickUserSettings::InputCategory`) isn't saved in the config files, so
categories made at run time leave nothing behind.

## Reading held keys

The mod reads whether a bound key is held with `GetAsyncKeyState`, like it did before keys were
rebindable: Unreal's own key state only changes with input that reaches the game viewport, which a
dragged slider or a focused text box may keep. Polling alone ignores what the game does with the
keys, so two gates apply (`platform::keyboard`, 2026-10-16; numpad keys as defaults typed into
sliders and fired while the game was in the background):

1. The game's window must be the foreground one (`GetForegroundWindow`'s process is ours).
2. Keys that type text (everything but modifier keys and mouse buttons) must also have been
   delivered to the viewport: the hooked `UPlayerInput::InputKey` (`click_swap` owns the hook, the
   loader hooks a function once) records press/repeat/release per virtual key, and a key counts
   while it's both physically down and last delivered as pressed. A release a text box took can't
   leave it stuck (the physical state ends it), and a press typed into a text box never counts.
   Modifiers and mouse buttons are exempt (dragged sliders, e.g. Alt scrubbing, keep them from the
   viewport as well). If the hook isn't in place the gate is off.

Assumption to verify in game: a key typed into a focused text box doesn't reach
`APlayerController::InputKey`. (`UBrickTextBoxWidget::NativeOnKeyDown`, BRMK 0x43ED40, handles
CapsLock/Accept/Back itself and passes the rest to `HandleKeyDown`; not read further.) If it does
reach it, gate 2 needs `UBrickTextBoxWidget::IsTyping` instead.

`FKey` names map back to virtual keys as UE 4.27's `FWindowsPlatformInput::GetKeyMap` maps them
forward: named keys by virtual key code (`LeftAlt` = `VK_LMENU`, ...), printable ones by the
character they type without modifiers in the current layout (`Zero`.. `Nine`, `Semicolon`, ...,
and the AZERTY names `Ampersand`, `E_AccentAigu`, ...; other characters are their own name), found
with `VkKeyScanW`. The top row's digits are the same virtual keys (`VK_0`..`VK_9`) on every layout,
so `One` is the key typing `&` on AZERTY; letters are where the layout puts them, like the game's own
bindings. Matching goes through the virtual key, so the delivered name (`One` or `Ampersand`) and
the bound one agree either way. Gamepad keys and the mouse wheel can't be held this way.
