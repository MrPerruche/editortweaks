# Editor operations: changing bricks from code, undo, drawing

Found 2026-10-10 for the resize tool (`features/brick_resize`). BRMK 1.11.6 addresses and offsets
(`BrickRigsModKitSteam-BrickRigs.dll` unless noted), then game 1.11.7 (build 25831956) where
named. Bindings: `game/brick_editor.rs`, `game/editor_object.rs` (`SpawnTransforms`,
`WorldTransforms`), `game/scalable_brick.rs` (`BrickSizes`), `game/brick_property.rs`,
`game/editor_input.rs` (`SelectAction`), `game/line_batcher.rs`, `game/viewport.rs`.

## The editor's tick

`ABrickEditor::Tick(float)` (BRMK 0xEFA30, game 0xD0B0C0): `AActor::Tick`, a member's tick
(+0x220, the view target), then the camera input vector (three floats, compared with zero) moving
the camera, `GetCameraSpeed` (game 0xE6AD60, our `view_target` binding; its only caller among the
editor's), `SetCameraRelativeLocation`, the gizmo. `AVehicleEditor::Tick` (BRMK 0xEFC60) calls it
first; in the game it's reached through a tail jump (0xD0B350). Found as the caller of
`GetCameraSpeed` whose calls match BRMK's `Tick`. Signature: `brick_editor::TICK`.

## Scoped operations (undo steps)

`ABrickEditor` keeps the open operation at +0x468 (BRMK; game +0x390). Operations nest: the
outermost wins.

```text
FBrickEditorScopedOperation* StartScopedOperation(Op)    // 0xECF30, inlined where used
    if (!Editor->Open) Editor->Open = Op;  return Editor->Open;
bool FinishScopedOperation(Op)                            // 0xA10B0, inlined where used
    if (!Editor->Open) return false;  Editor->Open = null;  Editor->PostModify(*Op);
    // vtable +0x7F0 (game +0x698)
```

The operation is a 0x18-byte stack struct (from `RemoveObjectFromGroup`, 0xD7610, and
`UBrickEditorMoveMode::EndMode`, 0x9CE60): `ABrickEditor* Editor` (+0), the outermost open
operation (+8), change flags (+0x10, or-ed into the outermost one), a byte whose two low bits are
cleared at the start (+0x14). Flags seen: group changes 0x80; a move commit 0x1 and 0x10, plus 0x4
unless the move's event type is 2. `PostModify` reads them bit by bit (0, 2, 3, 5, 6, 7 at least);
meanings not mapped.

In the game: a `UBrickEditorMode` action at 0xD0EAF0 (takes the editor from the mode at +0x30,
opens an operation, changes the selection through 0xCE96A0, finishes it) gives both the start
(field offset, and the stack layout checked against ours) and the finish (field offset again,
`PostModify`'s vtable offset): signatures `brick_editor::OPERATION_START` and `OPERATION_FINISH`,
checked to be in the same function. The finish is inlined at 7 places (search `48 c7 81 ?? ?? 00 00
00 00 00 00 ff 90`).

`virtual ABrickEditor::PostModify(const FBrickEditorScopedOperation&)` (0xC6A90, `AVehicleEditor`
overrides it, 0xC76F0) serializes into an `FArchive`: the undo snapshot. Every editor action that
changes the vehicle opens one (writes of +0x468 found with `find "48 89 ?? 68 04 00 00"`):
grouping, hiding, duplicating, deleting, loading, selection, undo/redo, moves.

Property edits: `AVehicleEditor::OnAnyPropertyModified(const FBrickPropertyChangedEvent&)` (BRMK
0xC0A10) records them itself (reflects the brick's properties, serializes), and returns early for
events of type 2 (interactive), so only the final change of a slider drag is recorded.

## Moving a brick (the move mode's commit)

`UBrickEditorMoveMode::EndMode` opens an operation (not `BeginMode`: while dragging, bricks only
show a temporary move transform, `UBrickEditorObject::SetMoveTransform`, and `OnEditorMoveUpdate`
previews it), then per moved object: `GetEffectiveObjectTransform` -> `SanitizeObjectTransform`
(returns location and rotator) -> writes them to the spawn location (+0x54) and rotation (+0x60)
(inlined `SetSpawnTransform`, export 0xE99B0, no direct callers) -> `ApplySpawnTransform()`
(0x89050) -> clears +0x48 (the move transform) -> `OnEditorMoveCommitted` (UBrick vtable +0x3B0;
it calls `UpdateBrickMaterial`, +0x4E0) or `OnEditorMoveCancelled` (+0x3B8). Mirrored objects go
through 0x7B2E0 (also calls `ApplySpawnTransform`). Then the operation is finished.

`ApplySpawnTransform` is only `if (RootComponent) RootComponent->SetRelativeLocationAndRotation(
SpawnLocation, SpawnRotation, false, nullptr, ETeleportType::None)`, which the reflected
`USceneComponent::K2_SetRelativeLocationAndRotation` does too. The spawn location and rotation
(game +0x4C, +0x58) are found at run time: the only place in the object holding what the reflected
getters `GetBrickEditorObjectSpawnLocation`/`Rotation` return, one after the other
(`SpawnTransforms::learn`).

`UBrickEditorObject::GetMirroredSpawnTransform(FVector&, FQuat&, EAxis)` (0xB0CB0) and
`MirrorTransform` (0xBFD60; our `mirror_mode` binding) compute mirrored places.

### How a move is computed (found 2026-10-10 for `features/precise_moves`)

BRMK 1.11.6 addresses; game 1.11.7 (build 25831956) offsets where they differ (move mode fields 8
lower, user settings 0x10 lower). Bindings: `game/move_mode.rs`, every offset read from the game's
code (signatures `MOVE_TRANSFORM`, `MOVE_GRID`, `MOVE_TURN`, `GIZMO_FRAME`, `REFERENCE`,
`SANITIZE`, `SANITIZE_ROTATION`, `APPLY_INPUT`; consistency checked across them).

The move mode (`UBrickEditorMoveMode`, game offsets): `+0x30` the editor, `+0x38`/`+0x3C` the
gizmo axis and type (see `editor-input.md`), `+0x40` flags (bit 0x01: the gizmo is the world's;
0x02 `BeginMode`'s bool argument; 0x08 input folded into `MoveLocation`/`MoveRotation` since the
move began; 0x10 move perpendicular; 0x20 releasing the mouse folds the input instead of ending the
move; 0x40/0x80 results close to `EffectiveMoveLocation`/`Rotation` become them, set by
`BeginMode`), `+0x41` more flags (`Flags2` below; `BeginMode` sets 0x01 and 0x02), `+0x48` the
reference brick (`FWeakObjectPtr`, the brick the move is about), `+0x50` its mirror mode counterpart,
`+0x6C` the drag so far (cm), `+0x78` the turn so far (degrees about each gizmo axis), `+0x84`
`MoveLocation`, `+0x90` `MoveRotation` (an `FRotator`), `+0xA0` the brick snapping adjustment
(offset `+0`, pivot `+0xC`, quaternion `+0x20`; 0x30 bytes), `+0xD0`/`+0xDC`
`EffectiveMoveLocation`/`Rotation` (reflected; `__PPO__` offsets BRMK 0xE0/0xEC). User settings
(`UBrickUserSettings::GetUserSettings()`, game 0xE6E250): `+0x338` snapping on, `+0x328` grid
step, `+0x32C` angle step.

```text
BeginMode(Axis, bArgument) (0x8A400)
    SetGizmoAxis(Axis, true); Flags: 0x08, 0x10, 0x20 cleared, 0x04, 0x40, 0x80 set, 0x02 = bArgument
    Drag, Turn (and +0x58) zeroed, adjustment none; Reference = the brick grabbed
    MoveLocation, MoveRotation = EffectiveMoveLocation, Rotation = the reference's spawn location and
        rotation (copied, exact)

ApplyAccumulatedInput() (0x88870; game 0xCCE610), from SetGizmoAxis when the axis or type changes
        (BeginMode, PressObjectAtScreenPosition on a gizmo handle, ReleaseObjectAtScreenPosition if
        Flags & 0x20, AddMoveInput, HandleStartMoveOrSetGizmoAxis) and OnGizmoWorldSpaceChanged
    if Drag or Turn != 0:
        T = GetMoveTransformWithAccumulatedInput()
        MoveLocation = T.Location; MoveRotation = T.Rotation.Rotator() (FQuat::Rotator: f32, FastAsin)
        Flags |= 0x08; Drag = Turn = 0

ReleaseObjectAtScreenPosition (0xD58B0): Flags & 0x20 and a gizmo axis: SetGizmoAxis(none) (the
    fold above, the move goes on); else EnterDefaultEditorMode (EndMode, the commit)

GetGizmoTransformInternal(bWithInput) (0xAF410; game 0xCE46C0)
    G = bWithInput ? Adjust(GetMoveTransformWithAccumulatedInput()) : (MoveLocation, Quat(MoveRotation))
    if (Flags & 1) G.Rotation = Identity

GetMoveTransformWithAccumulatedInput() (0xB1730; game 0xCE57E0)
    T = (MoveLocation, Quat(MoveRotation)); G = GetGizmoTransformInternal(false)
    if Drag != 0:
        if Snapping && Grid > 0:   F = GizmoType == 1 ? Identity : G.Rotation
                                   T.Location += F.Rotate(floor((F.Unrotate(Drag) + Grid/2) / Grid) * Grid)
        else                       T.Location += Drag
    if Turn != 0: Step = Snapping ? Angle : 0
        for i in X, Y, Z (lambda 0x78FC0; game 0xCC8E70):
            a = Step > 0 ? floor((Turn[i] + Step/2) / Step) * Step : Turn[i]
            if a: T.Rotation = FQuat(G.Rotation.Rotate(unit i), a * 0.0174532924) * T.Rotation
                  (FMath::SinCos: UE's polynomial)
    ConstraintLocationToEditorBounds(T.Location)

Adjust(T) (0x88E00; game 0xCCEB00): T.Rotation = Q * T.Rotation; T.Location = Pivot + Q.Rotate(T.Location + Offset - Pivot)

EndMode (0x9CE60; game 0xCD8820):
    A = Adjust(GetMoveTransformWithAccumulatedInput()); B = GetEffectiveMirroredTransform(A)
    commit (lambda 0x7B2E0; game 0xCCAD20) mirror mode counterparts (EditorParams flags 0x200), the
    mirrored reference, the others, the reference last: Sanitize(GetEffectiveObjectTransform(o, A, B)),
    written to the spawn fields, ApplySpawnTransform

GetEffectiveObjectTransform(o, A, B) (0xADA10; game 0xCE3140)
    o == mirrored reference: B;  o == reference: A
    plain: A * Reference⁻¹ * o (GetRelativeTransform, then FTransform multiply; scale 1)
    a counterpart whose source is selected: computed from the source, then MirrorTransform
```

`TickMode` (0xF19A0; game 0xD0C8F0) previews the same way every frame (`Sanitize` of each object's
`GetEffectiveObjectTransform`, 0xF254C), then sanitizes the reference with
`GetMoveTransformWithAccumulatedInput()` *without* `Adjust` (0xF26D2) and writes that into
`EffectiveMoveLocation` unless Flags & 0x40, `EffectiveMoveRotation` unless Flags & 0x80 (shown in
the panel).

`SanitizeObjectTransform` (0xDFC50; game 0xD019C0):

```text
(FVector, FRotator) SanitizeObjectTransform(const UBrickEditorObject* Object, const FMoveTransform& T)
    Location = T.Location (+0), but EffectiveMoveLocation if Flags & 0x40 and every coordinate is
        within 1e-4 of it, or the object's spawn location if Flags2 & 1 and within 1e-4
    Rotation = QuatToNiceRotator(T.Rotation (+0x10)), but EffectiveMoveRotation if Flags & 0x80
        and the quaternions (q or -q) are within 1e-4 per component, or the spawn rotation if
        Flags2 & 2 and within 1e-4
```

Everything above is `f32`: a group turned 4 times by 90° drifts (a simulation of the pipeline
gives 44.999992 for 45, 90.00001° for 90°). `features/precise_moves` hooks
`SanitizeObjectTransform` and computes the plain case again in `f64` from the same inputs, rounding
only the result; the simulation then returns to exactly 45 and 90°. For the reference brick it
also tries the move without `Adjust`, for `TickMode`'s second call.

First in-game test (2026-10-10): a 90° turn with the gizmo still gave 89.99999°. Releasing the
gizmo handle folds the turn into `MoveRotation` through `FQuat::Rotator` in `f32` before the
commit, and the commit started from that. A fold without a turn also turns `MoveRotation` through
`f32` angles and back (180° became 179.99998°), so other bricks came out a hair off (19.999985 for
20). `precise_moves` now also hooks `ApplyAccumulatedInput`: the fold is computed in `f64` too, the
game gets the nearest `f32` values, and the `f64` one is used while `MoveLocation`/`MoveRotation`
still hold those and Flags & 0x08 (a simulation of two released 90° turns: 179.99998° from the
`f32` fold, exactly 180° now).

`UFluMathStatics::QuatToNiceRotator(const FQuat&)` (reflected `UFunction`; native BRMK 0x2CA470,
game 0xE387A0, the call in `execQuatToNiceRotator`, found through the `FNameNativePtrPair` naming
it): forward = the rotated X axis; if `forward.x² + forward.y² > 1e-8`: yaw = `atan2(forward.y,
forward.x)`, pitch = `asinf(clamp(forward.z, -1, 1))`; else pitch = ±90 by the sign of forward.z
and yaw = `atan2(up.y, up.x)` of the rotated Z axis (0 if that's vertical too); then roll from
`conj(Quaternion(pitch, yaw, 0)) * q`: `-atan2(2(x w + y z), 1 - 2(x² + z²))`; every angle
normalized to (-180, 180]. All `f32` (an `rsqrtss` with two Newton steps for the lengths).
Ported to `f64` as `math::Quat::to_rotator`. Its other callers: `MirrorBrickEditorObject` (0xBFCB0)
and 0x2665F0.

## Resizing a scalable brick

Like the panel: write `BrickSize`, then `PostModifyBrickProperty` with an event naming it.
`UScalableBrick::PostModifyBrickProperty` (BRMK 0x1672A0): `ConstraintBrickSize`, then unless the
event is a mirrored copy (`IsPostModifyBrickPropertyMirrored`) the mesh's mirror axes,
`UpdateRenderScale3D`, `UpdateBodySetupScale3D`, `CreatePhysicsState`, then (not for type 2)
`UpdateAerodynamicLiftSurface`, `UpdateBrickMaterial`, the connectors (not for type 2), then
`UBrick::PostModifyBrickProperty`. It doesn't touch the mirrored counterpart: the panel edits both.

`FBrickPropertyChangedEvent` (`brick_property::ChangedEvent`): 0x18 unknown bytes (no handler of
bricks reads them), the path `TArray<FName>` (+0x18), level (+0x28), the edited objects
`TArray<FWeakObjectPtr>` (+0x30, read by `OnAnyPropertyModified`), the source object
`FWeakObjectPtr` (+0x40, `IsPostModifyBrickPropertyMirrored`: a copy made for a counterpart), the
type (+0x48: 2 while dragging). Our events (`ChangedEvent::with_property`) leave the unknown part,
the objects and the source empty.

The event goes to the brick's `IBrickPropertyInterface`, at the offset `UClass::Interfaces`
(`TArray<FImplementedInterface>`, UE 4.27 shipping +0x1D8: after `FuncMap` and `SuperFuncMap`,
0x50 bytes each, and their lock) lists for `UBrickEditorObject` (game +0x28), checked by calling
the interface's `_getUObject`. Its vtable (BRMK `??_7UScalableBrick@@6BIBrickPropertyInterface@@@`,
0x75B468): [0] destructor thunk, [1] `_getUObject` (`lea rax, [rcx - 0x30]; ret` in BRMK), [2]
`ReflectBrickProperties`, [3] `CanModifyBrickProperty`, [4] `PostModifyBrickProperty` (called with
the interface pointer as `this`, MSVC's convention for overrides of a secondary base's virtuals).
`_getUObject` is a leaf function, absent from `.pdata`: the first in-game test (2026-10-10) failed
with "property interface not found" because the check required a `.pdata` function start; it now
only requires game code (`sig::is_code`).

A brick's box (`GetBrickEditorObjectLocalBounds`) is its static mesh's box times its scale, and a
scalable brick's scale follows `BrickSize`: the box scales with the size along each axis, around
the brick's origin.

## The selection

Not reflected. `ABrickEditor::SelectObjects` (BRMK 0xE41B0) calls a selection update (BRMK
0x4F700; game 0xD04A90, signature `editor_object::SELECTION_UPDATE`) that gives each editor object
its `EBrickSelectionState` (0 none, 1 selected, 2 the primary one) with an inlined
`UBrickEditorObject::SetSelectionState` (BRMK 0xE9990): a byte in the object's `EditorParams` (the
pointer at BRMK +0x48, game +0x40), BRMK +0x12, game +0xA, four bytes before the flags byte the
hidden check reads (BRMK +0x16, game +0xE). `GetSelectionState` (0xB48C0) reads it.

## Clicks: `Action_Select`

The left mouse button is the `Select` input row (Shift: `AddToSelection`; Alt doesn't change the
row), handled by
`UBaseEditorInputComponent::Action_Select` (reflected, BRMK 0x223B50) -> `HandleSelect(Params,
false)` (0x23EF50): nothing unless `Params.Mode == 1` (`EBrActionMode::Real`, not a test); then
`Mode->PressObjectAtScreenPosition` (vtable +0x3A0) if the byte at +8 of `FBrActionParams` is 0,
else `ReleaseObjectAtScreenPosition` (+0x3A8). `FBrActionParams` is 0xC bytes (`Mode` +0,
`bIsSecondaryAction` +1, `AxisValue` +4, the unreflected press/release byte +8). The result,
`FBrActionResult` (reflected), starts with its outcome byte: 0 handled, 2 not. The generated thunk
assigns to the return value, so the caller constructs it.

## Drawing lines: `ULineBatchComponent` (engine)

`UWorld::LineBatcher` / `PersistentLineBatcher` / `ForegroundLineBatcher` are reflected
properties (names in the shipping exe). `DrawLine(const FVector& Start, const FVector& End, const
FLinearColor&, uint8 DepthPriority, float Thickness, float LifeTime)` and `DrawPoint` are the last
two slots of `ULineBatchComponent`'s own vtable, after `UPrimitiveComponent`'s many (BRMK engine
module: pointers at .rdata 0x2083400/0x2083408, after `DoCustomNavigableGeometryExport`), so their
slot numbers differ between builds. `DrawLine` (BRMK engine 0x8A31F0, game 0x247FBA0) appends a
0x34-byte `FBatchedLine` (start, end, color +0x18, thickness +0x28, remaining lifetime +0x2C, depth
priority +0x30) to `BatchedLines` (game +0x4B0) and marks the render state dirty. Signature:
`line_batcher::DRAW_LINE`, from its line-filling code (the array's offset read from it).

`TickComponent` (BRMK engine 0x8D57C0) only drops lines with a positive lifetime once it runs out:
lines with none stay until removed. `DrawBox`, `DrawSolidBox`, `DrawMesh` (filled triangles),
`DrawCircle` and `Flush` are not virtual. Thickness is in world units.

`AActor::AddComponentByClass` is reflected (engine 4.26+): another way to show things is adding
mesh components to the editor actor and giving them a material the game has loaded.
