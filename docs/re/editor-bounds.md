# Editor bounds (how far bricks and the camera may go)

Found 2026-10-11 for `features/editor_bounds`. BRMK 1.11.6 addresses
(`BrickRigsModKitSteam-BrickRigs.dll`), then game 1.11.7 (build 25831956) where named. Binding:
`game/editor_static_info.rs`, reflection only (no signatures).

## The limit: `UBrickEditorStaticInfo::EditorBounds`

Found by the export `ABrickEditor::ConstraintLocationToEditorBounds(FVector&, float) const`
(BRMK 0x903D0; game 0xCD29E0, found by its body's pattern `f3 0f 10 78 ?? f3 0f 10 68 ?? 0f 28 d7
f3 0f 10 60 ?? f3 0f 58 fe`):

```text
bool ConstraintLocationToEditorBounds(FVector& Location, float Padding)
    B = GetStaticInfo()->EditorBounds                 // BRMK +0x30, game +0x28
    each axis: Location = clamp(Location, -B - Padding, B + Padding); true if any was clamped
```

`ABrickEditor::GetStaticInfo` (BRMK 0xB4F50) returns the default object of the editor's
`TSubclassOf<UBrickEditorStaticInfo>` (BRMK +0x5B0, a Blueprint class), read again on every call:
changing that default object changes the editor at once. `EditorBounds` is a reflected `FVector`
(`CPF_Edit`; its `FStructPropertyParams` at BRMK 0xBDBE40, found by a pointer to its name), half
the box's size, in cm, centred on the vehicle's origin. Its neighbours: `BoundsFadeRange` (+0x3C, a
min/max pair), `BoundsMesh` (+0x48), `CenterOfMassMesh`, `GridMesh`, `GizmoMesh`, ...,
`InitialCameraLocation`. The game's value (Blueprint default, not in the code) is logged by
`editor_bounds` at start-up.

## Who reads it

Readers of `GetStaticInfo()` that touch +0x30 (BRMK), all in `ABrickEditor`:

- `ConstraintLocationToEditorBounds`, called from:
  - `UBrickEditorMoveMode::GetMoveTransformWithAccumulatedInput` (BRMK 0xB1730, game 0xCE57E0),
    padding 0: every move.
  - `UBrickEditorMoveMode::TickMode` (BRMK 0xF19A0, game 0xD0C8F0), per moved object; its result
    goes to a byte of the object's `EditorParams` (+0x13 BRMK; likely an out-of-bounds state).
  - `ABrickEditor::SetCameraRelativeLocation` (BRMK 0xE6680, game 0xD06630), padding 100 cm: the
    camera.
- `SetCameraRelativeLocation` again: the camera's distance to the nearest face of the box (`B -
  |Location|`, smallest axis) against `BoundsFadeRange`; once within it, the bounds mesh component
  (editor +0x518 BRMK, created there the first time, `SetRelativeScale3D(EditorBounds * 0.01)`,
  mesh `BoundsMesh`) shows, its material (+0x500) getting `Opacity` from that distance. Its scale
  is set only at creation: changing the bounds after the camera first came near the edge leaves
  the old box (`features/editor_bounds` changes them at engine init and map loads, before that).
- `GetObjectAtScreenPosition` (BRMK 0xB2240): the click trace is `|2 * EditorBounds|` long (the
  box's diagonal).
- An unnamed function (BRMK 0x78B40, called from 0x9DE80) returning the box as an `FBox`.
- `AVehicleEditor::GetMoveSnappingOffset` (BRMK 0xB0DE0, game 0xCE5340; editor vtable +0x688,
  called from `TickMode` every frame of a move while snapping is on): the editor's floor is one
  virtual connector, built on the stack at BRMK 0xB1162 (type 1, gender 2, at the vehicle's origin,
  unrotated) whose grid is sized from `EditorBounds.X`/`.Y`: `RoundToInt(B * 0.26667 + 0.5) - 1`,
  made odd (BRMK constants 0x7764A8, 0x7764B4). Moved bricks snap onto it like onto any brick.
  Connector pairs go through game 0xF1C550 -> 0xF16B90 (type, gender and facing match, and the
  brick's connector within the snapping distance of the floor's plane) -> 0xF16D30, which walks
  every cell of one grid and finds the matching cells of the other. 0xF16B90 makes it walk the
  smaller one: it compares the grids' areas, `imul` of the counts at +0x1C and +0x20, in **signed
  32 bits** (0xF16CA7..0xF16CC4). An axle against the floor walks the axle's one cell: instant.

  Found 2026-10-11 from a freeze (`et.py hang`): moving an axle down to the floor (Z = 0) with
  `editor_bounds` at ±10 000 000 cm. The live floor grid was 1 333 331 x 1 333 331 cells (15 cm,
  shipping 1.11.7): its area overflows to -344 904 983, so the floor counted as the smaller grid
  and 0xF16D30 walked its 1.8e12 cells, at about 5e7 per second: hours for one frame. The area
  stays positive up to 46 340 cells per side: bounds up to about ±347 000 cm (shipping cells).
  So this function must see bounds below that (the game's are ±25 000 and ±100 000 cm).

  `features/editor_bounds` keeps ±10 000 000 cm everywhere else: `MoveMode::snap_around` hooks
  this function and runs it with the static info objects back at the game's bounds (at most
  ±300 000 cm), restoring ours after; bricks snap to the floor only within the game's box. The
  function is found before any editor spawns: the default objects of `ABrickEditor`'s subclasses
  (native ones exist from engine init and carry their class's vtable), the one whose slot +0x688
  differs from `ABrickEditor`'s own. If that fails, the bounds stay at ±300 000 cm.

`GetEditorBounds(FVector& Min, FVector& Max)` (BRMK 0xAC9A0) returns `-B`, `B`; no native callers
(maybe Blueprint).

## Precision

Brick places are `f32`: their steps are 0.25 cm from 21 km, 0.5 cm from 42 km, 1 cm from 84 km
from the vehicle's origin. Rendering far from the world origin jitters similarly (UE4 has no large
world coordinates).
