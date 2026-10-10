# Property panel (vehicle editor, settings, ...)

How a numeric property gets from the UI into a brick. Found in BRMK 1.11.6
(`BrickRigsModKitSteam-BrickRigs.dll` exports), confirmed in game 1.11.7 (build 25633865).

## Widget chain

`UPropertiesPanelWidget` -> `UPropertyContainerWidget` -> `UPropertyWidget` (one per property)
-> `UNumericPropertyWidget` -> `Sliders`: one `UBrickSliderWidget` per component (1-3).
Brick properties themselves are plain C++ (`FNumericBrickPropertyBase` etc., not exported).

## UBrickSliderWidget

Reflected fields (names from the game's reflection data, in declaration order): `FillBorder`,
`MinValue`, `MaxValue`, `ValueStep`, `GridSnapMode`, `ValueType`, `MaxFractionalDigits`,
`bZeroAsUnlimited`, `CustomTextFormat`, `bIsReadOnly`, `bIsMarquee`, `OnValueChangedDelegate`.
Offsets differ between BRMK and the game (MinValue: 0x454 vs 0x2BC), so read them by name.

`SetValueInternal(float, bool bClampAndSnap, bool bBroadcast, EValueChangedEventType)`:
game RVA 0xF393E0 (build 25831956; 0xF39340 in 25633865), pattern in `crates/editortweaks/src/game/slider.rs`. Pseudocode there.
`GetDefaultValueRange(ValueType, {MinValue, MaxValue})` returns the slider's range unchanged
except for `ENumericValueType` 0x13 (distance in auto/brick units), rounded to the brick grid.
`GetDefaultGridSnapMode` is 0 (none) for every type except 0x13 (3 = grid from zero).

Callers and their event type (0 = committed, 1 = released, 2 = interactive):

| Caller | Value | Type | Clamps itself first |
|---|---|---|---|
| `OnTextChanged` (typed text, after `/ GetUnitConversionFactor`) | parsed | 0 | no |
| `OnReleased` (twice) | current value | 1, 0 | no |
| `NativeOnMouseMove` (drag; absolute in "fast slider mode", else relative; see below) | dragged | 2 | yes |
| `HandleKeyEvent` (arrow keys, value +- step) | stepped | 2 | yes |
| `CommitValue` (BRMK only, inlined in the game) | given | 0 | no |

### Dragging (`NativeOnMouseMove`)

BRMK export 0x440A50; game 0xF2D610 (build 25831956), 0xF2D570 (25633865). Pseudocode in
`crates/editortweaks/src/game/slider_drag.rs`. In short:
- Only while dragging: `OnDragged` (once the mouse moved past Slate's drag distance after a press)
  sets bit 0 of a flags byte and copies `Value` into a float `DragValue`; `OnReleased` clears it.
  Neither is reflected: game +0x298 and +0x29C (BRMK +0x430, +0x434), then the `uint64` frame
  number of the last change (+0x2A0 / +0x438).
- Mode: Shift always drags relatively; otherwise `UUIFunctionLibrary::ShouldUseFastSliderMode
  (ValueType)` (false only for types 15-19, the distances) XOR Ctrl picks absolute.
- Absolute: the value under the cursor, `Min + (ScreenX - Left) / Width * (Max - Min)`, from the
  geometry's render transform. Relative: `DragValue += Step * CursorDelta.X * (Shift ? 0.002 :
  0.05)`, Step being `ValueStep` or `GetDefaultValueStep(ValueType)` (game 0xECB500, not reflected).
- Then `DragValue` is clamped to `GetDefaultValueRange(ValueType, {Min, Max})`, and at most once
  per frame (`GFrameCounter`) passed to `SetValueInternal(.., true, true, Interactive)`. Relative
  drags then keep the cursor over the slider (`FReply::SetMousePos`).
- The slider captures the mouse in high precision mode (`ShouldCaptureHighPrecisionMouseMovement`
  returns true): the cursor is hidden, `CursorDelta` holds raw mouse counts, and events only come
  when the mouse moves. Alt isn't used.

`FPointerEvent` (engine layout): `ModifierKeys` +0x8 (bits: Shift 0-1, Ctrl 2-3, Alt 4-5),
`ScreenSpacePosition` +0x18, `CursorDelta` +0x28. `FGeometry`: `Size` +0, render transform matrix
+0x1C, translation +0x2C.

`slider_drag` lifts the clamp by patching it in the code (game 0xF2D7A1: `jb +7; movaps xmm1,
xmm4; minss xmm1, xmm2` becomes a 6-byte `nop; movaps xmm1, xmm4`; no jump lands inside it, checked
in both builds), and drives Alt-drags from a Win32 thread timer, since no mouse events come while
the mouse is held still.

## UNumericPropertyWidget::OnSliderValueChanged(float, EValueChangedEventType, int32 SliderIndex)

Called from the widget's Blueprint: `AddSliderWidget` is a Blueprint event that creates the sliders
(reflected `Sliders` array) and binds each one's `OnValueChangedDelegate` to this, passing its
index. Only reachable through `execOnSliderValueChanged`.

Reads the property's current value (up to 3 floats) from the first edited object (BRMK 0x3826C0:
builds that object's container, calls the property's getter, vtable [18]), replaces component
`SliderIndex` (all of them when `AreAxesLocked()`, also updating the other sliders), then writes
that one value to every edited object through a helper (BRMK 0x38ACD0, not exported) with the owning
`ABasePlayerController`: in an undo transaction, last object first, the property's typed setter
(vtable [19]) per object. So editing one axis of several bricks also gives them all the first
brick's other axes. The other sliders get the value as sent, before the property clamps it, and
aren't refreshed from the property during a drag (only once it's committed on release); seen with
a locked brick size dragged below the rules' minimum (2026-10-09). `slider_drag` shows the stored
values in them itself.

The slider's range comes from `UpdateValue` -> (BRMK 0x3883F0) -> `SetValueRange(Range.Min[i],
Range.Max[i])`, where the range is the property's virtual `GetValueRange` (vtable +0xA0).

## Numeric brick properties (game 1.11.7)

Found by following `UpdateValue`'s virtual calls; details and pseudocode in
`crates/editortweaks/src/game/numeric_property.rs`. Vtable [18] is the typed getter
(`bool Get(const Container*, Value* Out)`, e.g. 0xCE8BE0 for floats in build 25831956; a rotator
comes out as roll, pitch, yaw). The property clamps again when storing:
every typed setter (vtable [19], e.g. 0xD081B0 for floats) runs `ClampValue` (0xD01BD0), which
clamps each component to `GetValueRange` (0xCE8FC0) and applies the property's axis lock. Seen
in testing: with only the slider unclamped, typed values reached the property and were clamped
there (editor int/float/vector properties and settings menus alike).

The edited field: every numeric property holds the reflected `FProperty*` it edits at +0x8. The
typed setters (e.g. 0xD081B0 float, 0xD08220 rotator: stores components 1, 2, 0) write to
`Target + FProperty::Offset_Internal`, where `container` (their second argument, also
`ClampValue`'s) is `{ void* Root; TArray<void*> Path }` and `Target` is the last path entry, or
`Root` without a path. From the `FProperty` come the property's name, its type (`IntProperty`,
`FloatProperty`, `StructProperty`, ...) and its declaring class or struct (`FField::Owner`), which is
what `property_limits` matches its rules on.

Upstream brickrust's `NUMERIC_BRICK_PROPERTY_FLOAT_VTABLE` pattern matches three numeric vtables
(0x38DC498, 0x38DB968, 0x38DD5C8); they share everything but entries [6], [18] and [19].

## Typed text and display (BRMK 1.11.6 `UBrickSliderWidget`, game build 25831956)

`OnTextChanged(const FText&, EValueChangedEventType)` (game 0xF30F70) is a reflected function
bound to the text box; its only caller is its exec thunk (game 0xF9CCF0), so `slider_formulas`
hooks the thunk, found through reflection (`UFunction::Func`, see below). When committed (type 0;
interactive typing, type 2, is ignored) and not read-only:
1. Parses the text with `TDefaultNumericTypeInterface<float>::FromString(Text, ExistingValue)`
   (game 0xF0A250, BRMK 0x40A7A0; called once, or three times for `TimeOfDay`, type 9, typed as
   `h:m`): a number in the user's culture without digit grouping, else culture-invariant
   (`FastDecimalFormat::StringToNumber`, game 0x1077810, which hands numbers with more than 20
   integral digits to `wcstod`), else `FBasicMathExpressionEvaluator::Evaluate` (`+ - * / % ^`,
   `sqrt`, parentheses), whose double result is clamped to +-FLT_MAX. So an overflowing formula
   like `1000000000^100` gives FLT_MAX, not infinity. The result is converted to float.
   `ExistingValue` is `&this->Value`, the stored value, used by the operators `+=`, `-=`, `*=`,
   `/=`: they mix units on converted sliders (`+=10` on 50 % gives 10.5 %).
2. Rejects NaN (`_isnan`); otherwise divides by `UUIFunctionLibrary::GetUnitConversionFactor
   (ValueType)` in single precision (`divss`) and calls `SetValueInternal(Value, true, true,
   Committed)`.

When interactive and the text box is typing (`UBrickTextBoxWidget::IsTyping()`, reflected), it
filters the text instead (game 0xF31319): every character is removed unless it's `:`, a digit or
separator of the user's number format (`ExpressionParser::GetLocalizedNumberFormattingRules`), or
in a static `FString` built on first use (game 0x4979090, guard 0x49790A0) from
`1234567890()-+=\/.,*^%%`; if anything was removed, the text box gets the result. So letters can't
be typed, not even the evaluator's `sqrt`. Not typing: just `UpdateSliderValue(false, false)`.
`slider_formulas` skips the call for these keystrokes (the event type is read from the thunk's
frame: the text box's dynamic delegate calls through `ProcessEvent`, so `FFrame::Locals` holds the
parameters at their reflected offsets).

`UpdateSliderValue(bool bUseGivenFocus, bool bFocused)` (game 0xF45970) writes the value into the
text box (`TextBox`, reflected): when typing (`bFocused`, or without `bUseGivenFocus` the text box's
flag, bit 4 of byte +0x260 in the game, +0x3F8 in BRMK) and not `TimeOfDay`, the edit text, else
the label. `NativeOnFocusChanging` calls it with `(true, true)` when typing starts.
- Label: `UUIFunctionLibrary::NumberToText(Value, ValueType, MaxFractionalDigits, true, false,
  bZeroAsUnlimited)`, put into `CustomTextFormat` if set. See below.
- Edit text: `FText::AsNumber(Value * GetUnitConversionFactor(ValueType))` with default options:
  at most 3 decimals and digit grouping, which `FromString` can't read back (vanilla: committing an
  untouched "1,234.5" changes nothing, and edits of it are ignored). `precise_sliders` shows the
  shortest text that reads back as the stored value instead, ungrouped (with a unit factor: or as
  the float next to it, since the single-precision conversion is that far off itself; e.g. typing
  1 brick gives 29.999998 cm before the brick grid snaps it).
  Unreal's number formatting loops forever on infinity: opening the input froze the game
  (2026-10-09, log in `.local/logs/2026-10-09-freeze/`). There a typed `1e9^100` became FLT_MAX,
  / 100 (percent slider) = 3.4028236e36 stored, x 100 on display = infinity. So stored values must
  stay below FLT_MAX / (largest factor); `property_limits` caps them at FLT_MAX / 128.

`NumberToText(float, ENumericValueType, int32 MaxFractionalDigits, bool bShowUnit, bool
bAlwaysSign, bool bZeroAsUnlimited)` (game 0xEE5D90; also tooltips, map scale, inventory, ...):
resolves the type (user's units), then: `TimeOfDay` as `{0}:{1}`; `Gear` as `R{0}`/`N`/...;
otherwise `FText::AsNumber(Value * factor)` with `MinimumFractionalDigits` 0,
`MaximumFractionalDigits` from the argument (a per-type default if negative), `AlwaysSign` from the
argument, default rounding (half to even); with 0 decimals it rounds the value itself first
(`+0.5`, truncate). Zero with `bZeroAsUnlimited` (and huge `Money`) gives "Unlimited",
`DistanceBrickUnits` goes to `BrickUnitsToText`, non-finite values give "Invalid"; then the unit
(`GetUnitFormat`).

`FText::AsNumber(float, ...)` (game 0x108E9F0) only forwards to `AsNumberTemplate<float, float>`
(0x107B610; hooked by `precise_sliders`), which formats through
`FastDecimalFormat::NumberToString(double)` (0x1062130): at most 18 decimals (`min(requested,
18)`, 0x1062690). `FNumberFormattingOptions` is 0x18 bytes: `AlwaysSign` +0, `UseGrouping` +1,
`RoundingMode` +4, integral digits +8/+0xC, fractional digits min +0x10, max +0x14.

`ConvertToRealUnits(float, ENumericValueType)` (reflected, BRMK 0x905F0) is
`GetUnitConversionFactor(Type) * Value`; `GetUnitConversionFactor` resolves the `*Auto` types
itself. The mod gets the factor by calling it with 1 through `ProcessEvent`, no signature needed.

`GetUnitConversionFactor` (after `ResolveNumericValueType` picks the user's unit for `*Auto` types):
Percent 100, MassImperial 2.2046, SpeedMetric 0.036 (cm/s -> km/h), SpeedImperial 0.02237,
DistanceMetric 0.01 (cm -> m), DistanceImperial 0.03281 (cm -> ft), DistanceBrickUnits 1/30,
VolumeImperial 0.2642, ForceMetric 0.01, ForceImperial 0.002248, everything else 1.

`ENumericValueType` (from the reflected enum names): 0 Float, 1 Integer, 2 Percent, 3 Angle, 4 RPM,
5 Seconds, 6 Minutes, 7 Hours, 8 Days, 9 TimeOfDay, 10 Money, 11 Gear, 12-14 Speed{Auto,Metric,
Imperial}, 15-17 Distance{Auto,Metric,Imperial}, 18 DistanceAutoOrBrickUnits, 19
DistanceBrickUnits, 20-22 Volume*, 23 Bricks, 24-26 Mass*, 27-29 Force*, ...

## Scalable bricks: size and connectors

`UScalableBrick::ConstraintBrickSize` (game 0xD2D190, vtable +0x4F0; BRMK +0x5B0) is a pure clamp:
`BrickSize = Min(Max(BrickSize, GetMinBrickSize()), GetMaxBrickSize())` (virtuals +0x4D8/+0x4E0;
BRMK +0x598/+0x5A0). Its only caller is `UScalableBrick::PostModifyBrickProperty` (game
0xD4F560, see below), and only for `BrickSize`: not for `ConnectorSpacing`, and not on load
(`PostLoadBrickEditorObject` doesn't call it). Found by listing every `call [reg+0x5B0]` in BRMK and
`call [reg+0x4F0]` in the game. So oversized bricks survive saving and loading without the mod.

Connectors: each face of a scalable brick is one connector field (`UScalableBrickBase::
AddGeneratedConnector(Face, FVector2D Size, ..., EConnectorSpacing, EConnectorShape)`), with
`max(1, round(side / spacing))` points along each side. `ConnectorSpacing`
(`FScalableBrickConnectorSpacing`, declared on `UScalableBrickBase`, 16 bits) holds 2 bits per face
at bit `4 * axis + 2 * positive` (`UBrickStatics::GetScalableBrickConnectorSpacingAxis`, BRMK
0x1CD670, shift helper 0x142E80). `EConnectorSpacing`: None, Default 30 cm, Half 15 cm, Third
10 cm, Double 60 cm, Triple 90 cm, Quadruple 120 cm (distance switch BRMK 0x1587C0); faces only
use the first four. The game's largest brick (9000 cm cube, Third everywhere) has 4.86 million
points. Negative sizes (typed with the mod, or edited into the file; vanilla loads them as they
are) mirror the brick along that axis, and it still connects (seen in game): the mod's connector
budget counts such sides by their magnitude.

## Before and after a brick property is written (game build 25831956)

Found 2026-10-09 to re-check limits that depend on other properties (`property_limits::
dependencies`). There's no "pre-modify" notification (no BRMK export with `PreModify`), but:

- Before: each typed write helper (11 of them, game 0xEB3390 ... 0xEB6B50, one per value type;
  BRMK 0x389150 ... incl. the numeric one 0x38ACD0) calls `FBrickPropertyEditInfo::CanModify(const
  FBrickPropertyContainer&)` (our name; BRMK 0x3102A0, game 0xE5C810) for each edited object right
  before calling the property's setter. It casts `Container.Root` to `UBrickEditorObject`, returns
  false if the edit info's read-only byte (+0xB1, BRMK +0xB9) is set, and otherwise asks
  `ABrickEditor::CanModifyBrickProperty(Object, EditInfo)` (BRMK export 0x8EB40, game 0xCD1BA0:
  `EditorMode && EditorMode->vfunc(+0x2C8)`, BRMK +0x388). Found through that export's only caller
  in BRMK, then in the game by searching `48 8b 01 ff 90 c8 02 00 00 84 c0` (vtable offset minus
  0xC0, as for bricks), whose `xrefs` gave the wrapper and its 13 callers (the 11 helpers, plus
  0xCC2670 and 0xF01500, which show rows read-only).
- After: the object's `IBrickPropertyInterface::PostModifyBrickProperty(const
  FBrickPropertyChangedEvent&)`, interface vtable slot 4. The interface vtable is [0] destructor,
  [1] `_getUObject` (game 0x820F60: `lea rax, [rcx-0x28]`; BRMK -0x30), [2]
  `ReflectBrickProperties`, [3] `CanModifyBrickProperty` (returns true), [4] `PostModifyBrickProperty`,
  [5] `UpdateFocusedBrickProperty`. Found by searching .rdata for pointers to the functions below.
- `FBrickPropertyChangedEvent`: `TArray<FName> Path` at +0x18, `int32 Level` at +0x28; the
  property changed is `Path[Level]`, and an empty path means every property (FName is 8 bytes in
  the game, 12 in BRMK). BRMK checks a byte at +0x48 against 2 (interactive) before rebuilding
  connectors.
- `UScalableBrick::PostModifyBrickProperty` (BRMK export 0x1672A0, game 0xD4F560): checks
  `BrickSize` and `ConnectorSpacing` by name; `BrickSize` -> `ConstraintBrickSize`; either ->
  mirroring, render scale, body setup, physics state, aerodynamics, connectors; then
  `UBrick::PostModifyBrickProperty`. `ULightBrick::PostModifyBrickProperty` (BRMK 0x1671C0, game
  0xD4F4E0) calls it first, then checks `FlashSequence` (`UpdateBrickTickEnabled`) and always
  ends with `UpdateBrightness(true)` (game 0xD715E0).

## Spinner bricks: mesh and collision resolution (BRMK 1.11.6)

Checked 2026-10-09 while looking for what huge spinner values could break (the game had crashed
after Alt-dragging a spinner property far out, cause unknown: no crash report, so likely a freeze).
Neither resolution grows with the values:
- `USpinnerBrick::UpdateMesh` (BRMK 0x1912C0) builds the visible mesh at unit size and full angle
  (constant params), its resolution from `|SpinnerAngle|`, clamped to 1-32.
- `USpinnerBrick::UpdateBodySetup` (BRMK 0x18CAB0) gets the collision resolution from
  `CalcSpinnerMeshResolution` (ellipse arc length of the radii, `powf`), clamped to 1-32.
- `GetSpinnerMeshParams` (BRMK 0x158890) passes the radii unclamped, the other sizes clamped to a
  constant.

## Light bricks: intensity overflow (BRMK 1.11.6)

Crash found 2026-10-09: Alt-drag `LightBrick.Brightness` (game range 0..1) to the old cap of
±`SAFE_MAX` (2.66e36), deselect the brick, select it again: the game died (no crash report).
`ULightBrick::GetLightParams` (BRMK 0x1488C0; `Brightness` +0x138, `LightConeAngle` +0x16C) computes
`intensity = (omni ? 1 : pow(90° / cone, 0.75)) * StaticInfo[+0x1E0] * Brightness` and stores it
and `intensity * 650`, which overflows to infinity by a brightness of 5e35 at the latest (sooner
with a base intensity above 1). So values must stay far below
the float maximum for the game's own math, not just for display: `property_limits` now stops values
without a limit of their own at ±`rules::UNLIMITED` (1e9 at first, 1e15 after the user's tests).

`LightDirection` (reflected, `ELightBrickDirection`, 1 byte): None ("Off") 0, Omnidirectional 1,
X 2, XNeg 3, Y 4, YNeg 5, Z 6, ZNeg 7 (BRMK `FEnumParams` at 0xBFDB40, from
`Z_Construct_UEnum_BrickRigs_ELightBrickDirection`). Set to Off the brick only glows; the rules
limit a shining light's brightness to ±1000%.

## Reflected functions (UE 4.27 shipping)

`UFunction` after `UStruct` (0xB0 bytes): `FunctionFlags` +0xB0 (`FUNC_Native` 0x400), `NumParms`
+0xB4, `ParmsSize` +0xB6, `ReturnValueOffset` +0xB8, ..., `FirstPropertyToInit` +0xC0,
`EventGraphFunction` +0xC8, `EventGraphCallOffset` +0xD0, `Func` +0xD8 (the exec thunk). brickrust
doesn't declare these; `game::reflection::native_thunk` reads `Func` and checks that it's a
function start in the exe's unwind table. Parameters are the function's properties in order, at
their `Offset_Internal`, the return value flagged `CPF_ReturnParm` (0x400). `ProcessEvent` is
vtable slot 68 (+0x220), as brickrust declares it.
