# Editor input: camera speed and move mode

How the vehicle editor's camera speed steps, mouse movement and mouse-driven brick moves work.
Found in BRMK 1.11.6 (`BrickRigsModKitSteam-BrickRigs.dll` exports, content assets), confirmed in
game 1.11.7 (build 25831956). Used by `features/camera_speed`, `features/move_slowdown`,
`features/mouse_moves` and `features/brick_resize`.

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
| `MoveCamera` | RightMouseButton (held) |
| `PivotCamera` | MiddleMouseButton, SpaceBar (held) |
| `Select` | LeftMouseButton |
| `MouseMoveRight` / `MouseMoveUp` | MouseX / MouseY (scale 1) |
| `ToggleSnapping` | SpaceBar |
| `MoveSelection` | G |
| `AddToSelection` | Shift + LeftMouseButton |
| `SpeedUpStep` / `SpeedDownStep` | Ctrl + MouseScrollUp / Down |
| `DuplicateSelection`, `Undo`, `SaveItem`, ... | Ctrl + C, Z, S, ... (Shift + Ctrl variants too) |
| `UnhideAll` | Alt + H |
| `RemoveFromEditorGroup` / `RemoveFromWeldGroup` | Ctrl + Alt + G / W |

No editor row uses Ctrl or Alt alone, so holding them while dragging is free (Shift isn't).
Action names and descriptions: `Content/Localization/StringTables/Input.csv`. How rows become
bindings, how players rebind them and how the mod adds its own: `input-bindings.md`.

## Mouse movement and the camera

The engine reports the mouse's movement (MouseX/MouseY) only while the viewport has the mouse
captured, i.e. while a mouse button is held over the 3D view (UE 4.27 `FSceneViewport::OnMouseMove`
accumulates `MouseDelta` only then), with the default axis sensitivity 0.07 per pixel
(`BaseInput.ini`; `move_slowdown` logged 0.07 for a one-pixel move). Whether the cursor stays visible
meanwhile depends on the input mode (`UEditorInputComponent::GetInputMode`, BRMK 0x23A2C0: no cursor
while the camera is moved, or while the mode's virtual +0x338 says so; otherwise shown); the resize
tool logs which it sees.

Both rows reach `UEditorInputComponent::Axis_MouseMoveRight` / `Up` (BRMK 0x2282C0 / 0x2285A0):

```text
Mode = this->Mode (weak pointer, +0x208), if a UBrickEditorMoveMode;
if (!(this->CameraFlags & 4) && Mode) {          // +0x210 (game +0x200): MoveCamera or PivotCamera
    if (Params.Mode == Real) Mode->AddMouseMoveInput(Axis, AxisValue * EditorMouseMoveSensitivity * k);
    return Handled;
}
return Axis_ViewYaw(Params, MouseSensitivity);   // Pitch for Up
```

`UBaseInputComponent::Axis_ViewYaw` (BRMK 0x228DB0) adds the value to the player controller's yaw
input (`AddYawInput`, vtable +0xD18) unless it ignores look input (+0x8C0); the input component's
own check (vtable +0x540) returns true for every input component. So without a move mode, holding
any mouse button and moving the mouse turns the editor's camera (seen in game 2026-10-10: the
first resize drags turned it): the resize tool takes that
movement while it drags (`features/mouse_moves`, shared with `move_slowdown`, since the loader hooks
a function only once). `Action_MoveCamera` / `Action_PivotCamera` (BRMK 0x222710 / 0x223110) set bits
0 / 1 of +0x210 and call `UpdateCameraMode` (0x261F80), which sets bit 2 (4) when either is set.
In the game the overrides call `HandleStealMouseMove` (0xDE3CC0), whose flag test gives the offset
(signature `editor_input::STEAL_MOUSE_MOVE`, `CameraFlags`). The resize tool takes the right button
(`Action_MoveCamera`, hooked) while it drags, so the camera mode doesn't start then, and so does drag
select while its key is held; both go through `features/editor_buttons`, which hooks the `Select`
and `MoveCamera` thunks once for every feature (see `selection.md`).
`ortho_view` takes the mouse's movement while the camera mode is on and its pan key is held
(Shift + right button), and pans the camera by it (`editor-camera.md`).

The editor creates its input component in `ABrickEditor::EnableInput` (BRMK 0x9C9A0) with itself as
the outer (`StaticConstructObject_Internal`, stored in `AActor::InputComponent`), so a handler's
editor is its component's outer (`game::editor_mode::EditorModes::editor_of`).

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
- The gizmo axis (`FBrickEditorGizmoAxis`, not reflected) is the move mode's first own field: BRMK
  +0x40/+0x44, game +0x38/+0x3C (after `UBrickEditorMode::BrickEditor`, reflected, BRMK +0x38, game
  +0x30): the axis (4 bytes, `EAxis`: 1 X, 2 Y, 3 Z; `TickMode` picks the unit vector by it) and a
  type (1 byte): 1 follows the mouse in the plane across the axis (the only branch of `TickMode`,
  BRMK 0xF19A0, reading the perpendicular flag), 3 rotates around the axis (adds to the rotation
  angle of that axis), other values for the gizmo's arrows. G (`Action_MoveSelection`, 0x222BB0)
  starts a move by {Z, 1} through `HandleStartMoveOrSetGizmoAxis` (0x23F010), the default mode's
  `MoveSelection` and the move mode's `BeginMode`. `GetGizmoAxis` (0xAEF90) returns its address;
  `SetGizmoAxis` (0xE7E40) sets it. During a move, `HandleStartMoveOrSetGizmoAxis` sets the axis it's
  given ({Z, 1} for G) unless the move already has it; then it sets the second one it's given
  instead ({Z, 0} for G): G during a G move frees the mouse and shows the gizmo (the precision move
  mode below), G again goes back. Two leaf functions test it: `IsInPrecisionMoveMode` (0xBC650,
  not virtual, inlined in the game): `Axis == 0 || Type == 0`; `ShouldCaptureMouse` (0xEAD40,
  virtual; game 0xD08E20, its only pointer at `.rdata` 0x38D8398 in the move mode's vtable): the
  opposite. Leaf functions have no `.pdata` entry, so the mod finds it by its whole code
  (`editor_mode::SHOULD_CAPTURE_MOUSE`, a `CodeShape`) among the virtual functions of the move
  mode's default object, and reads the two offsets from it. The game has no RTTI: vtables follow
  one another directly in `.rdata`. (The first version took the two fields the other way around,
  so it took G's move for a gizmo one.)
- `Action_MovePerpendicular` (BRMK 0x222A40) sets bit 0x10 of the move mode's flags byte (BRMK
  +0x48, right after the gizmo axis; read by `GetMouseMovePerpendicular`, 0xB0D30) while Shift is
  held, in any move mode; only type 1 moves use it. `move_slowdown` slows down by Shift only moves
  of the other types (`EditorModes::moving` is `ByGizmo`), and never while the camera is moved.

- The precision move mode (no axis, or type 0: the mouse is free, the gizmo shows; `Moving::Precisely`
  in `game::editor_mode`): the move mode's `PressObjectAtScreenPosition` (BRMK 0xC8180) only acts on
  a gizmo handle, the one `ABrickEditor::GetGizmoAxisAtScreenPosition` (0xAEFA0) finds there if it's
  the hovered one (editor +0x4E0/+0x4E4): `SetGizmoAxis` with it. `ReleaseObjectAtScreenPosition`
  (0xD58B0) ends a handle's drag (flag 0x20 and an axis with a type: `SetGizmoAxis(none)`, the move
  goes on) or else ends the move (`EnterDefaultEditorMode`, the commit). So a click off the gizmo
  ends the move. The editor's default mode instead marks the hovered object pressed on a press and
  selects on the release (`UBrickEditorMode`, 0xC8150 / 0xD57E0).

## Snapping while moving (explored 2026-10-10)

Looked into for face/edge snapping while moving (roadmap "Advanced resizing"). The resize tool's
moving (2026-10-11) doesn't go through the game's brick snapping: it gives the move its own place
(see `editor-operations.md`, "Moving the bricks of a move from code"), as do its other pieces (undo
operations, moving bricks from code, line drawing, the left click).

- `virtual FBrickEditorSnappingOffset ABrickEditor::GetMoveSnappingOffset(const
  FGetMoveSnappingOffsetParams&) const` (BRMK 0xB0DA0) returns no offset: zero vectors at +0 and
  +0xC, 0 at +0x18, an identity `FQuat` at +0x20 (0x30 bytes). `AVehicleEditor` overrides it (BRMK
  0xB0DE0) with connector snapping: it reads two floats from the parameters, +0x60 (squared and
  compared: a distance, likely `EditorBrickSnappingDistance`) and +0x64 (`cosf`: an angle, likely
  `EditorBrickSnappingAngle`), takes an array through the pointer at +0x8 (count read at +8), and
  runs a query over 12 entries of 0x18 bytes with callbacks (helpers 0x437610 and 0x42D410,
  unexported). Neither function is reflected, and the shipping exe has no string naming it (the
  stat name is compiled out). In the game: the editor's vtable +0x688, read from `TickMode`'s call
  (see `editor-operations.md`, "Moving the bricks of a move from code"); `brick_resize` hooks it.
- The user's snapping settings are reflected `UBrickUserSettings` properties (names present in
  the shipping exe): `bEditorSnappingEnabled` (Space toggles it), `EditorBrickSnappingDistance` /
  `Angle`, `EditorGridSnappingDistance` / `Angle`.
- `UBrickEditorMoveMode::EffectiveMoveLocation` / `EffectiveMoveRotation` are reflected properties
  (BRMK `__PPO__` exports). The functions that apply a move (`GetEffectiveObjectTransform`,
  `SanitizeObjectTransform`, `UBrickEditorObject::SetMoveTransform`) are not.
- `ABrickEditor::GetHoveredObject` and `GetObjectAtScreenPosition` (the editor's own picking) are
  not reflected; the resize tool picks among the bricks' boxes itself.
