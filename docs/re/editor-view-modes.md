# Editor view modes and mirror pairing

How the vehicle editor's view modes color bricks, how mirror mode pairs bricks, how a view mode is
added, and how bricks' places and sizes are compared. Found in BRMK 1.11.6
(`BrickRigsModKitSteam-BrickRigs.dll` exports, reflection data), confirmed in game 1.11.7 (build
25831956). Used by `features/view_modes` (the Mirror and Duplicates view modes) through
`game/editor_view.rs`, `game/mirror_mode.rs` and `game/editor_object.rs`.

## The view modes

`EBrickEditorViewMode` (reflected `UEnum`): Default 0, Wireframe 1, Physics 2, InputChannels 3,
WeldGroups 4 ("Welds"), EditorGroups 5 ("Groups"), then the generated `_MAX` (6). The current one
is the user setting `UBrickUserSettings::EditorViewMode` (reflected; BRMK +0x2FA, game +0x2EA),
next to `LastNonDefaultEditorViewMode` (+1, for `ToggleDefaultViewMode`), `EditorMirrorAxis`
(BRMK +0x2F8, game +0x2E8) and `bEditorMirrorAxisEnabled` (+1).

Z (`Action_CycleViewMode`, input row `CycleViewMode`) calls `UBrickEditorDefaultMode::CycleViewMode`
(BRMK 0x937A0, game 0xCD48B0), which is data-driven:

```text
Enum = StaticEnum<EBrickEditorViewMode>();
Index = index of the current value in Enum->Names (or -1);
Index = (Index + 1) % (Enum->NumEnums() - 1);   // skips the last entry, _MAX
SetEditorViewMode(Enum->Names[Index].Value);
```

`UEnum::Names` is a `TArray<TPair<FName, int64>>` at +0x40 in the shipping game (0x10 per entry;
BRMK: +0x48, 0x18 per entry, editor `FName`s being bigger). So a new entry before `_MAX` joins the
cycle; `features/view_modes` adds `EBrickEditorViewMode::Mirror` = 6 and `Duplicates` = 7 (`_MAX`
becomes 8), growing the array with `FMemory::Realloc` and making the `FName` with the reflected
`UKismetStringLibrary::Conv_StringToName`.

Name case: the shipping game stores each `FName` once, in the case it was first made with, and
compares names ignoring case. The parameters `InString` of `Conv_StringToName`/`Conv_StringToText`
read "inString" in the game (the exe has both spellings), so a case-sensitive lookup missed them (the
first version of the Mirror view mode stayed inactive). `game::reflection::name_is` compares like the engine.

Other readers of the setting only compare it to 2, 3, 4 or 5 (BRMK: `GetGroupTypeToVisualize`,
`UpdateDesiredCustomDepth`, `UpdateInputChannelVisualization`, `UpdatePhysicsProperties`,
`SelectObjects`, `ShouldBeHiddenByViewMode`, the functions below), so an unknown value behaves like
Default. No array is indexed by it in C++.

Names and the view mode list: the view modes' names are a fixed list of six `FEnumPropertyItem`s
built by `UBrickUserSettings::GetEditorViewModeItems` (BRMK 0x320630, game 0xE66EF0; only called
through function pointers), each named from the `BrickEditor` string table (`Name_ViewMode_*`).
The editor's panel (nothing selected) shows it as the view mode list. The notification after Z gets
its text from a generic helper (BRMK 0x50D20, game 0xDBEF20, our name `GetEnumPropertyItemText`):
fill the items, return the name of the item with the setting's value, or the text "Invalid". In
the game, `GetEditorViewModeDisplayName` is inlined into `Action_CycleViewMode` (0xDCCA10).

`FEnumPropertyItem` (game 0xA8 bytes, BRMK 0xB0): value (`int32`, +0), display info (+8: name
`FText`, then an icon soft pointer up to +0x48), three more `FText`s (+0x48, +0x60, +0x78), a
pointer (+0x90) and flags (+0x98, +0x99, +0xA0). Its copy constructor (game 0xCC4380, 132 callers)
adds the texts' references. `features/view_modes` hooks `GetEditorViewModeItems` and appends, per
view mode, a copy of the last item with its value and name (`ViewModeItems::append`), which fixes both the list
and the name. The function is found by the only code loading `L"Name_ViewMode_Default"`
(`framework::sig::StringRefSig`, checked by `et.py sigcheck` like byte patterns).

Saving: with the mod, the setting can hold 6 or 7 when the game saves its settings. Without the mod
the value has no name; an import of an unknown name fails and keeps the default, and a raw 6 renders
as Default and cycles to Default. Not observed yet.

## How bricks are colored

`ABrickEditor::OnViewModeChanged` (BRMK 0xC5030) calls `UpdateObjectsVisualization` (0xFB3C0),
which calls every object's virtual `UpdateEditorVisualization`; for bricks this ends in
`UBrick::UpdateBrickMaterial` (BRMK 0xF61F0, game 0xD0F0A0), for bricks in the editor
(`(Flags & 0x18) == 8`, the flags byte at BRMK +0x50, game +0x48):

```text
Override = GetEditorMaterialOverride();
for each material slot of the mesh: SetMaterial(slot, Override ? Override : GetEditorBrickMaterial(slot));
UpdatePrimitiveData(MeshRef, Transform, Identity);   // our name, see below
```

`UBrick::GetEditorMaterialOverride` (BRMK 0xACBF0, game 0xCE27D0) returns, from the brick's
`UBrickStaticInfo` (default object of its static info class; `GetBrickStaticInfo`, game 0xCBD0F0):
`HiddenMaterial` for hidden objects (`EditorParams->Flags & 0x18`), `PhysicsVisualizationMaterial`
for Physics, `ColorVisualizationMaterial` for WeldGroups and EditorGroups (all reflected; game
+0xF0, +0xF8, +0x100), else null. `GetViewMode` (BRMK 0xB7B70, game 0xCE92B0) reads the user setting
when the object's owner is a `BrickEditor`, else 0.

The brick's primitive data (BRMK 0x42C40, game 0xCB8110; also called by `OnEditorMoveUpdate`) feed
`ColorVisualizationMaterial` in the group view modes:

```text
Group = WeldGroup (brick, game +0x64) or EditorParams->Group (game +8; BRMK +0x10)
Low = Group ? 0.2 : 0.05;  High = 1.0
SetCustomPrimitiveDataVector4(0, {bit 3 ? High : Low, bit 4 ..., bit 5 ..., 1})
SetCustomPrimitiveDataVector4(3, {bit 0 ? High : Low, bit 1 ..., bit 2 ..., 1})
```

So slots 0 to 2 and 3 to 5 are two colors (64 distinct groups); the same color in both is a plain
color. Default writes the brick's color to slot 0 and transform and pattern data to slots 4 to 17.
`features/view_modes` lets the game's function run, then calls the reflected
`UPrimitiveComponent::SetCustomPrimitiveDataVector4` on the reflected
`UBrickEditorObject::GetRootComponent` (the same mesh: both read the shared weak pointer at BRMK
+0x80).

## Mirror pairing

Mirror mode ("Mirror mode" toggle, `bEditorMirrorAxisEnabled`, with the "Mirror axis"
`EditorMirrorAxis`, `EAxis` X 1, Y 2, Z 3) edits paired bricks together. The pairs come from
`ABrickEditor::UpdateMirroredObjects` (BRMK 0xFA450, game 0xD11EA0), called after every change
(`PostModify`), when objects are created (`OnBrickEditorObjectsCreated`) and when mirror settings
change (`OnMirrorSettingsChanged`):

```text
if (!mirror mode on or no axis) { clear every object's mirrored object; return; }
clear every object's mirrored object, and hash those on the negative side (location on the axis < 0)
    by (location, class)
for each object on the positive side of the axis or the plane (location on the axis >= 0):
    MirrorTransform(Location, Rotation, out MirroredLocation, out MirroredRotation, Axis)
    MirroredClass = GetBrickEditorObjectMirroredClass(Class)
    for each offset in {0, -0.0001, +0.0001}^3 around MirroredLocation, a candidate must match:
        class == MirroredClass, location within tolerance, rotation q or -q within tolerance,
        every property IsBrickPropertyMirroredFrom(Object, Property, Axis)   // vtable, BRMK +0x3A8
    found: both objects' mirrored object = each other
```

The mirrored object is an `FWeakObjectPtr` at `EditorParams + 0` in the game (BRMK + 8, after an
editor-only field); `EditorParams` is a pointer at game +0x40 in `UBrickEditorObject` (BRMK +0x48).
`GetMirroredObject` (BRMK 0xB0C90) is inlined everywhere. The game probe writing it: 0xCB68F0
(`mov rcx, [object + 0x40]` straight into `FWeakObjectPtr::operator=`).

In detail (2026-10-10), all of the work is in lambdas: one clears every pair and hashes the
objects with `Location[Axis] < 0` by (`RoundToInt(location)`, static info class) (BRMK 0x4F440,
`comiss 0, location; jbe skip`; inlined in the game at 0xD123C0), one clears the pairs (mirror mode
off), and the matching loop (BRMK 0x4F910, game 0xCBC6D0; `game::mirror_mode::MATCHING`) takes each
object with `Location[Axis] >= 0`, so objects on the plane too:

| Step | BRMK | Game | Notes |
|---|---|---|---|
| `FRotator::Quaternion` | Core import | 0x10E4150 | |
| `UBrickEditorObject::MirrorTransform(Loc, Quat, OutLoc, OutQuat, Axis)` | 0xBFD60 | 0xCEDFE0 | per static info: `MirrorMode`, `MirrorRotationOrigin` |
| `UBrickEditorInterfaceComponent::GetBrickEditorObjectMirroredClass(StaticInfoClass)` | 0xA7D10 | 0xCDF500 | `MirrorClassOverride`, else from the class name |
| the candidate check, 27 times (offsets {0, ±0.0001}³) | 0x3FA50 | 0xCB68F0 | below |

The candidate check looks up the hash at the rounded mirrored location and, for each candidate,
in this order: the class is the mirrored class; each location coordinate within 0.0001; the
rotation `q` or `-q` within 0.0001 per component (all `KINDA_SMALL_NUMBER`); then, after gathering
the object's properties once (its property interface at game +0x28, BRMK +0x30, vtable +0x10),
`Candidate->IsBrickPropertyMirroredFrom(Object, Property, Axis)` for each (game vtable +0x2E8, BRMK
+0x3A8; implemented by `UBrickEditorObject`, `UActuatorBrick`, `UFlapBrick`, `UScalableBrick`,
`USpinnerBrick`, `USprocketWheelBrick`, `UWheelBrick`). All passing: both mirrored objects are set
and the candidate leaves the hash. Candidates are only objects on the negative side, so an object on
the plane (`Location[Axis]` exactly 0) is never paired, neither with itself nor with another one
on the plane. (A first version of the Mirror view mode assumed symmetric bricks on the plane were
paired with themselves, and showed them as mismatched: wrong, corrected 2026-10-10.) Not
duplicating a brick on the plane must be decided elsewhere, not by pairing.

Location and rotation are the spawn ones (`GetBrickEditorObjectSpawnLocation`/`Rotation`, reflected
`UFUNCTION`s; game +0x4C/+0x58, BRMK +0x54/+0x60); the type is the reflected `StaticInfoClass`
(game +0x30, BRMK +0x38). `UBrickEditorObjectStaticInfo` reflects `MirrorMode`,
`MirrorClassOverride` and `MirrorRotationOrigin`. `UBrickUserSettings::GetUserSettings` and
`UBrickEditorObject::GetBrickEditor` are reflected too.

So a brick failing only on properties is found without knowing them: redo the checks before the
property one with the game's own functions, among unpaired bricks (`MirrorMode::images`); a match
across the plane the editor didn't pair failed a property check (`features/view_modes/mirror`:
orange).
A match on the plane was never compared, so only type, place and rotation are known to mirror:
a brick that is its own mirror image (a mirrored copy would be a duplicate) shows a gray and green
pattern; two different bricks there (e.g. spinners facing away from each other) show orange like
across the plane, as the editor doesn't link them either: one click doesn't select both
(confirmed in game, 2026-10-10, two spinners at Y = 0 with different radii).

Checking their properties anyway would take `IsBrickPropertyMirroredFrom` (vtable) with the
editor's `FBrickPropertyInstance`s (0x30 bytes each; +0 points to a property descriptor with its
name at +0x10 and a value comparison at vtable +0x40). They come from
`IBrickPropertyInterface::ReflectBrickProperties(FBrickPropertyReflection&)` (interface vtable
+0x10; the interface at game +0x28 in an editor object), and `FBrickPropertyReflection` isn't
reflected and has no standalone constructor or destructor (BRMK 1.11.6: built and torn down inline
in `UpdateMirroredObjects`; arrays, texts, a gameplay tag container). Not done: it would copy that
layout. The spinner's override, for one, compares `SpinnerAngle` as `this + other ≈ 0` and passes
the rest to `UBrickEditorObject`'s value comparison.

Reflected along the way: `ABrickEditor::EditorInterfaceComponent` (game +0x438),
`UBrickEditorInterfaceComponent::BrickEditorObjects` (`TArray`, game +0x1F8).

## Duplicates: comparing bricks' places and sizes (2026-10-10)

`features/view_modes/duplicates` groups bricks by type, location and rotation, then by size, all
through reflection (`game/editor_object.rs`):

- Type: `UBrickEditorObject::StaticInfoClass`; place: the spawn location and rotation (above).
  Rotations are compared as the rotated X and Y axes (`FRotationMatrix`'s formula, in our code),
  so different `FRotator`s for the same rotation match.
- Size: `virtual bool UBrickEditorObject::GetBrickEditorObjectLocalBounds(FVector& OutBoundsMin,
  FVector& OutBoundsMax) const`, a reflected `UFUNCTION` (parameters `OutBoundsMin` +0,
  `OutBoundsMax` +0xC, `ReturnValue` +0x18; BRMK `FFunctionParams` 0xBDABF0, structure size 0x1C).
  Its thunk (BRMK 0x4DED80) calls the virtual (BRMK vtable +0x370), so `ProcessEvent` gets each
  type's version: `UBrickEditorObject` (0xA7C00), `UBrick` (0xA7B00: its static mesh's bounds
  times its mesh scale, virtuals +0x3E0 and +0x430; a scalable brick's scale follows `BrickSize`),
  `USpinnerBrick`, `UThrusterBrick`, `UWinchBrick`. False without a mesh. The box is in the
  brick's own space, so it stands for every size property without knowing any.

Values are rounded (0.01 cm, 0.001 per axis coordinate) instead of compared within a tolerance,
so that a stack of thousands of copies stays one hash lookup per brick; bounds are only asked for
bricks sharing a type, place and rotation with another.

## What a view mode can do (2026-10-10)

- Any number of view modes, each named, in the Z cycle and the panel's list.
- Per brick, any material the game has loaded, through `GetEditorMaterialOverride`. Useful ones:
  `ColorVisualizationMaterial` (opaque, lit; `Color1`/`Color2` from data slots 0-2 and 3-5, mixed
  by a screen-space grid texture, `T_EditorGrid`: one color, or two in a pattern), the brick's
  `HiddenMaterial` (`M_HiddenObject`: unlit, dithered see-through, a fixed color as far as seen),
  `PhysicsVisualizationMaterial` (center of mass and lift data), or the brick's own materials.
- Per brick, colors from anything readable: reflected properties, transforms, sizes (local
  bounds, see Duplicates), the game's mirror
  pairing (only while mirror mode is on, only across the plane, and strict: every property
  mirrored), the game's own mirroring functions (what is at a brick's mirror image, see above),
  or our own logic. Which property fails isn't known without calling the per-type virtual, which
  needs the game's property list (layout not mapped).
- Hiding bricks per view mode: the game's `ShouldBeHiddenByViewMode` (virtual, BRMK `UBrick` 0xEABF0;
  Input channels hides bricks without channels) could be hooked. Not done.
- Not possible from the DLL: new materials or shaders (the shipping game can't compile them; only a
  cooked pak made with BRMK could add some). Lines, text or arrows in the viewport need viewport
  drawing, not explored. Only bricks are colored; one view mode at a time.
- Repainting happens when the game updates a brick (edits, view mode changes) and after re-pairing
  (after every change, mirror mode on or off); anything else (selection, camera) needs its own
  trigger. Painting costs three reflected calls per brick; a view mode works out every brick's
  colors once per change (a few reflected calls per brick).
