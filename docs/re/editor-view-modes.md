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
  cooked pak made with BRMK could add some). Only bricks are colored; one view mode at a time.
- Beyond colors: lines through the world's line batcher (`game/line_batcher.rs`), and meshes of
  our own with any of the game's materials, as static meshes built at run time (the Collision view
  mode, see `collision-shapes.md`; procedural meshes don't get the custom primitive data brick
  materials read).
- Repainting happens when the game updates a brick (edits, view mode changes) and after re-pairing
  (after every change, mirror mode on or off); anything else (selection, camera) needs its own
  trigger. Painting costs three reflected calls per brick; a view mode works out every brick's
  colors once per change (a few reflected calls per brick).

## Reference fit: smooth colors without a shader (2026-10-18)

The "Reference fit" view mode (`features/view_modes/fit`) colors bricks by their distance to the
reference models, smoothly across each face. With no shaders of our own and no vertex colors in
the materials we can use, the color goes through the texture coordinate: each vertex of a mesh of
ours gets U = its distance on a log scale (`model_io::fit::scale_u`), and the mesh shows the
references' lit textured material (the Bricksville sign's, `references::looks::textured`) with a
4096 x 1 gradient texture generated at run time (`model_io::fit::gradient`, imported like any PNG).
The GPU interpolates U across each triangle. U is kept on texel centers inside the scale (the gray
texels at the end are for "no reference").

Stripes (2026-10-18): the first versions showed nested dark, shiny, wavy bands along the lines of
equal distance. Widening the gradient from 256 to 4096 texels (thinking it was point-sampled) didn't
change them. The cause is the material: `M_LegacyProp_Master` takes its roughness from a fixed,
non-parameter texture (`/Game/ThirdParty/Textures/Plastic_SmoothWorn_512_roughness`) read with the
same UVs, and the sign's instance leaves the `bOverrideRoughness` static switch off (see
`references.md`). With U following the distance, the worn-plastic smudges were stretched along the
distance's contour lines. Fix without another material: the gradient is 1 texel tall and has no
mips (`ImportBufferAsTexture2D` makes one), so V doesn't change its color; each vertex gets a V far
from its neighbors' (`model_io::fit::scattered_v`, whole numbers up to 65472), which makes the GPU
read the roughness texture from its smallest mip, one average value. Not seen in game yet.

The mesh is subdivided where linear interpolation of U would be off by more than about a sixth of a
power of ten (`model_io::fit::sample`, cells down to 0.5 cm, about 3000 triangles per brick at
most), then made conforming (a triangle with a neighbor's midpoints on its edges becomes a fan from
its center), so there are no color seams between patches split to different depths. Triangles whose
corners' distances already bound what's inside (a distance changes by at most the way travelled)
aren't measured inside at all, and each new vertex's search starts from its neighbors' distances.
Measuring runs on threads of ours (the cores but two), only when a brick's place, surface or scale
changes, or the references do.

Like the Collision view mode, each brick gets a component attached to its own mesh component with
the body setup scale (`Collision::scale_of`), and the brick shows its `HiddenMaterial` (returned by
our `GetEditorMaterialOverride` hook for this mode). The component is a procedural mesh
(`game/procedural_mesh.rs`, one reflected call per brick; see `collision-shapes.md`): the first
version built static meshes, about half of 6 s on a 1282-brick vehicle. The surface is the brick's
body setup (`Collision::body_setup_of`: its mesh's, or a spinner's own, made at run time with its
mesh) read as complex collision (`game/brick_model.rs`), wound counterclockwise from outside, so
reversed for the engine (told by its enclosed volume); without complex collision, its simple shapes;
else its local box. Each fallback is logged once per mesh name. Spinners' simple shapes are convex
pieces along the arc (`selection.md`); drawn as hulls they showed as overlapping blocks, not the
smooth arc the spinner draws. When a body setup has only convex pieces that chain (each piece the
cross-section at one step and at the next, point by point, the next piece starting where this one
ends), the surface is swept through those sections instead (`model_io::loft::chained`), ends
closed with a fan. Seen in game 2026-10-18: still looked like the collision, because the collision
uses a coarser level of the shape's profile than the visible mesh. So now (`game/spinner_profiles.rs`)
the mod asks the game's profile table for the spinner's shape (`SpinnerShape`, reflected), finds
the arc from the collision pieces and the collision level's profile (each section is linear in the
profile's radial coordinate: the slope gives its angle and the size, the offset the radii; the arc
found must reproduce every collision point), and sweeps the visible level's profile along it, at
64 steps a turn (`model_io::loft::reswept`). If the table or the fit fails, the joined collision
sections stay (logged once per mesh). Not seen in game yet. Bricks are placed by their spawn location and rotation
(`Places`), which are relative to the editor's root, the space references are placed in. Seen in
game: works, wheels fit. Possible misfits: mirrored bricks (negative sizes; the scale is clamped
positive).

Moves update live (2026-10-19): while bricks are dragged, the game moves their mesh components
(`collision-shapes.md`, "Moves are drawn by the material", correction), so our components, attached
to them, follow by themselves. A brick's place for measuring is its mesh component's reflected
`RelativeLocation`/`RelativeRotation` (relative to the editor's root, like the spawn place, which it
equals at rest). The hook runs every frame of a drag, and the brick is measured again there. Its
mesh keeps its colors until a newer measure is done, and every measure newer than the one shown is
used, not only the latest (by the time one is done the brick has moved on): colors catch up with the
drag, and nothing blinks. The worker measures only the latest place of each brick waiting.

The first try took the place from the primitive data function's `Place` argument instead. The log
of a drag (2026-10-19) showed the function called twice per frame for a dragged brick: with the
move's place, then with the spawn place (the component staying at the move's place). Each frame's
last job was then at the spawn place, so colors never changed, and placing our component at the
argument's place pulled it back to the spawn place.

Drawing over the screen instead (comparing, per pixel, where the bricks and the model are hit)
would need a shader comparing two depths; without one it means ray tracing the screen on the CPU at
a low resolution, redone whenever the camera moves, with a number that depends on the view angle.

Same z-fight in Reference fit (2026-10-19): our mesh *is* the brick's surface, so once built, the
brick's own mesh leaves the main and depth passes (`MeshComponents::set_drawn`, the reflected
`SetRenderInMainPass` + `bRenderInDepthPass` of `docs/re/references.md`), restored when the brick
leaves the view mode. Custom depth is untouched (the outline is copied to our component anyway),
and bricks measured as their box only keep the dithered mesh. Seen in game 2026-10-19: spinners
looked completely wrong, since with their own mesh gone only our swept stand-in showed. Now only
bricks measured as their complex collision (exact) lose their mesh; spinners, simple shapes and
boxes keep the dithered one. Why the spinners' swept surface looks wrong is still open.
