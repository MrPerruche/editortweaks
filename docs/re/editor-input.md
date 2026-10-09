# Editor input: camera speed and move mode

How the vehicle editor's camera speed steps and mouse-driven brick moves work. Found in BRMK 1.11.6
(`BrickRigsModKitSteam-BrickRigs.dll` exports, content assets), confirmed in game 1.11.7 (build
25831956). Used by `features/camera_speed` and `features/move_slowdown`.

## Input bindings

Each row of the input table (`/Game/BrickRigs/Input/InputTable`, a DataTable) is bound by
`UBaseInputComponent::SetupInputComponent` (BRMK 0x25CF30) to the component's function
`Action_<Row>` / `Axis_<Row>`: `FindFunctionChecked(Name)`, then `BindAction(Name, delegate)` with a
delegate calling that `UFunction` through `ProcessEvent` (BRMK delegate vtable 0x7C6AA0, `Execute`
0x2338B0: `UserObject->ProcessEvent(CachedFunction, &Payload)`). So every handler runs through its
exec thunk, which can be hooked by reflection (`game::reflection::native_thunk`).

Where the thunk reads a parameter from (`game::reflection::native_call_parameter`): by-value ones
from the frame's copy (`Locals`), but `const&` ones (the handlers' `const FBrActionParams&`) like
out parameters: UHT flags them `CPF_OutParm`, `ProcessEvent` lists the caller's own variable in
the frame's `OutParms` (FFrame +0x78; vendored brickrust had +0x60, patch 8), and
`FFrame::StepExplicitProperty` (game 0x1375D80) returns that address. Changing the copy in
`Locals` does nothing (the first version of `move_slowdown` did that).

`FBrActionParams` (reflected, 0xC bytes): `Mode` (enum, +0), `bIsSecondaryAction` (+1),
`AxisValue` (float, +4).

Default editor bindings with modifier keys (read from BRMK's `InputTable.uasset`, tagged
properties `DefaultMappings: [{bShift, bCtrl, bAlt, bCmd, Key, Scale}]`):

| Row | Keys |
|---|---|
| `MovePerpendicular` | LeftShift (held) |
| `ToggleSnapping` | SpaceBar |
| `MoveSelection` | G |
| `AddToSelection` | Shift + LeftMouseButton |
| `SpeedUpStep` / `SpeedDownStep` | Ctrl + MouseScrollUp / Down |
| `DuplicateSelection`, `Undo`, `SaveItem`, ... | Ctrl + C, Z, S, ... (Shift + Ctrl variants too) |
| `UnhideAll` | Alt + H |
| `RemoveFromEditorGroup` / `RemoveFromWeldGroup` | Ctrl + Alt + G / W |

No editor row uses Ctrl or Alt alone, so holding them while dragging is free (Shift isn't).
Action names and descriptions: `Content/Localization/StringTables/Input.csv`.

## Camera speed

The editor camera's speed is `IViewTargetInterface::GetCameraSpeed()` (our name; BRMK 0x323F40,
game 0xE6AD60 in 25831956 and 0xE6ACC0 in 25633865), pattern and pseudocode in
`crates/editortweaks/src/game/view_target.rs`:
`Lerp(SpeedRange.Min, SpeedRange.Max, powf(GetCameraSpeedRatio(), Exponent))`.
Called by `ABrickEditor::Tick` (camera movement), the free camera (`ABrickSpectatorPawn`: `Tick`,
`UpdateMovementSpeed`, ...) and `UGameOverlayWidget::OnCameraSpeedRatioChanged` (the "CameraSpeed"
notification).

The ratio (0 to 1) changes with `Action_SpeedUpStep` / `Action_SpeedDownStep` (BRMK 0x224BC0 /
0x224B10), which call an unexported step function (BRMK 0x36B2E0):

```text
Params = GetCameraSpeedParams(); Cache = GetCameraSpeedCache();
Ratio = Cache->Ratio (+4) + Direction * Params->Step;
if (Params->Step != 0) Ratio = floor((Ratio + 0.5 * Step) / Step) * Step;   // nearest step
Cache->Ratio = Clamp(Ratio, 0, 1);
```

Reflected structs:
- `FViewTargetSharedParams` (0x10): `InputRate` (+0), `Step` (+4), `Exponent` (+8),
  `InterpSpeed` (+0xC).
- `FViewTargetCameraSpeedParams` (0x18) adds `SpeedRange` (`FFloatInterval` Min, Max; +0x10).

The editor's are `UBrickEditorStaticInfo::CameraSpeedParams` (`GetCameraSpeedParams` returns
`GetStaticInfo() + offset`). C++ defaults (BRMK constructor 0x65B80): InputRate 1, Step 0.05,
Exponent 1.5, InterpSpeed 8, SpeedRange 300..3000; `BP_BrickEditor_StaticInfo` overrides
SpeedRange to 50..24000 cm/s. So 21 speeds: 50, 318, 807, ..., 22,226, 24,000: 6.4 times faster after the first step, 8% at the top.

`IViewTargetInterface` vtable slots (BRMK and game; the editor's vtables in the game are 0x38D8B08
for `ABrickEditor` and 0x38DC250 for `AVehicleEditor`, found through pointers to the editor's
`GetCameraSpeedParams`): [2] `_getUObject` (`lea rax, [rcx-0x220]; ret` in the game, -0x2F8 in
BRMK), [16] `GetCameraSpeedParams`, [17] `GetCameraSpeedCache`, [18] `GetCameraSpeedRatio`.
`GetCameraSpeed` reads its two slot offsets from its own code (the pattern), so only slot 2 is
assumed.

## Move mode (moving bricks with the mouse)

`UBrickEditorMoveMode` (BRMK exports):
- `AddMouseMoveInput(EAxis, float)` (0x86800): adds to the mouse input X (+0x6C) or Y (+0x70)
  (BRMK offsets, not reflected).
- `TickMode` (0xF19A0) uses it: translating, the movement is mouse input times
  `MoveSpeed + Distance * MoveSpeedDistanceScale` (static info: 90 and 0.03, `Distance` from the
  camera to the moved bricks), added to an unsnapped move offset; snapping applies to the
  effective transform. Rotating uses the mouse input too. Keyboard nudges (`AddMoveInput`) are
  separate.
- The only callers of `AddMouseMoveInput` are `UEditorInputComponent::Axis_MouseMoveRight` /
  `Axis_MouseMoveUp` (0x2282C0 / 0x2285A0, overriding `UBaseInputComponent`'s UFUNCTIONs, thunks
  `execAxis_MouseMoveRight` / `Up`): `Value = AxisValue * EditorMouseMoveSensitivity * constant`,
  only while a move mode is active. In BRMK, `HandleStealMouseMove(Params, Axis)` (0x23F580)
  does the same but has no references; in the game it's what both overrides call (game 0xDE3CC0,
  from 0xDD2950 (X) and 0xDD2B50 (Y), `UEditorInputComponent` vtable slots at 0x3904BE8 / BE0).
  The game's thunks: `execAxis_MouseMoveRight` 0xF71540 (virtual call +0x440), `Up` 0xF71650,
  found through the class's native function table (name string, then thunk).
- `IsInPrecisionMoveMode` (0xBC650) exists but is unrelated to speed (no gizmo axis chosen).
