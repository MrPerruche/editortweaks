# Collision shapes and run-time meshes

How bricks collide, how the mod reads their collision shapes, and how it draws extra meshes in
the editor. Found 2026-10-12 in BRMK 1.11.6 and game 1.11.7 (build 25831956). Used by the
Collision view mode (`features/view_modes/collision`) through `game/collision.rs`,
`game/mesh_builder.rs`, `game/mesh_component.rs` and `game/brick_materials.rs`.

## Simple and complex collision

The brick meshes use `CTF_UseSimpleAndComplex` (see `brick-models.md`): PhysX simulates contacts
with the simple shapes (`UBodySetup::AggGeom`: boxes, spheres, capsules, convex hulls), and
precise traces (`bTraceComplex`) hit the complex triangle mesh, made from the render mesh's
collision LOD. The view mode shows the simple shapes. Which traces in the game are complex
(editor picking, weapons) isn't checked.

## Bricks use the engine's body instance, scaled by the game

- `UBrick::CreatePhysicsState` (BRMK 0x929E0) sets a flag on the brick (`+0xEA |= 0x20`,
  `IsCreatingPhysicsState`), calls the engine's `UActorComponent::CreatePhysicsState`, and clears
  it. `UBrickStaticMeshComponent::ShouldCreatePhysicsState` (BRMK 0x1868D0) only agrees while that
  flag is set (so the engine never makes a brick's physics on its own). Bodies are ordinary
  `FBodyInstance`s from the brick's body setup: its mesh's, or a spinner's own
  (`USpinnerBrick::SpinnerBodySetup`, reflected, made by
  `UVehicleResourceSubsystem::FindOrCreateSpinnerBodySetup`). There's no per-type "no collision"
  flag there: a brick without collision has no simple shapes.
- `UBrick::UpdateBodySetupScale3D` (BRMK 0xF6020) takes the brick's virtual
  `GetBodySetupScale3D` (BRMK vtable +0x430), clamps each axis to at least 0.01 (BRMK .rdata
  0x776478), and passes it to `UBrickStaticMeshComponent::SetBodySetupScale3D` (0x182EF0) =
  `FBodyInstance::SetBodySetupScale3D` on the component's `BodyInstance` (+0x320).
- `FBodyInstance::SetBodySetupScale3D` is the game's engine change (BRMK engine 0x3DC7E0, not in
  stock 4.27): it stores the scale in a new field at body instance +0x6C (next to the stock
  `Scale3D` at +0x60) and calls `UpdateBodyScale`. Not reflected (the name isn't in the game exe).
- Overrides of `GetBodySetupScale3D`: `UBrick` (1, 0xA69A0), `UScalableBrick` (`BrickSize` / the
  static info's base size, 0x142F70) and `UWheelBrick` (0x1C6660: X from the radius over the
  static info's +0x1A8, Y and Z `GetBodySetupTargetRadius()` (virtual, +0x5A0; car and track wheels
  override it) over +0x1AC).

How the body uses it: `FBodyInstance::UpdateBodyScale` (BRMK engine 0x1140C60) runs a lambda per
shape (0x110DA20) that multiplies the body setup scale (+0x6C) by the component's world scale, then
scales each element with the stock `GetFinalScaled` (a sphere by the smallest axis). Wheels collide
as their sphere: `UWheelBrick::OnContactModify` (BRMK 0x1D5DF0) only scales a contact's friction
when `IsDriveSurfaceHit` (0x1D1790, the contact normal against the wheel's axis) holds.

The scale isn't reflected, but `UBrick::GetBrickEditorObjectLocalBounds` (BRMK 0xA7B00,
reflected) is `GetStaticMesh()` (+0x3E0) bounds times that same virtual (+0x430), so the mod takes
the ratio of the brick's local bounds to its mesh's bounds (`UStaticMesh::GetBounds`, reflected),
for `ScalableBrick` and `WheelBrick` and their subclasses only (the others are 1). Overrides of the
local bounds: `UThrusterBrick` (a `UScalableBrick`, BRMK 0x1436A0) calls `UBrick`'s and only moves
the smallest Z down for its nozzle, so the ratio is taken from the largest corner; `USpinnerBrick`
(a `UScalableBrickBase`, not scaled) and `UWinchBrick` (a `UBrick`) aren't scaled anyway. Vtable
check: in every vtable holding `UWheelBrick::GetBodySetupScale3D`, the slot before it (+0x428)
holds its `GetRenderScale3D`.

## Render scale

`UBrickStaticMeshComponent` has its own `RenderScale3D` (BRMK +0x644, not reflected, set by
`SetRenderScale3D` 0x184580 from `UBrick::UpdateRenderScale3D` 0xFCB80 = virtual +0x428
`GetRenderScale3D`). `GetRenderMatrix` (0x153A30) is the component transform with its scale
multiplied by it: the mesh is drawn scaled, the component isn't. `GetRenderScale3D` is the same as
`GetBodySetupScale3D` for `UBrick` (1) and `UScalableBrick` (identical code, 0x153BB0), but not for
wheels (`UWheelBrick` 0x1CD380: the radius from the static info's +0x1A0 instead of the target
radius over +0x1AC). So for everything but wheels, a component attached to a brick's with the body
scale renders in the brick's exact space.

## Moves are drawn by the material

`UBrick::OnEditorMoveUpdate` (BRMK 0xC25D0) only calls the brick's primitive data function (0x42C40,
see `editor-view-modes.md`) with the temporary move transform: while dragging, bricks don't move,
their material moves them from the custom primitive data. A mesh with the brick's material and a
copy of its custom primitive data moves along.

## Reading the shapes by reflection

`UBodySetup::AggGeom` (`FKAggregateGeom`) and its element structs are reflected in the shipping
game (names in the exe: `AggGeom`, `BoxElems`, `SphereElems`, `SphylElems`, `ConvexElems`,
`TaperedCapsuleElems`, `VertexData`, `KConvexElem`, ...):

| Element | Reflected fields read | Scaled like `GetFinalScaled` |
|---|---|---|
| `FKBoxElem` | `Center`, `Rotation`, `X`, `Y`, `Z` | lengths along its own axes, center |
| `FKSphereElem` | `Center`, `Radius` | radius by the smallest axis, center |
| `FKSphylElem` | `Center`, `Rotation`, `Radius`, `Length` | radius by max(X, Y), whole length by Z (min 0.1) |
| `FKConvexElem` | `VertexData` (`TArray<FVector>`), `Transform` | points (PhysX mesh scale) |
| `FKTaperedCapsuleElem` | (counted) | not used by PhysX, ignored |

All are in the mesh's space. A convex element only keeps its points (`IndexData` is for Chaos);
its faces are its convex hull, worked out by the mod (`collision/hull.rs`). Cooked meshes keep
`VertexData` (first test: e.g. `SM_ScalableRamp` 6 points, `SM_ScalableHemisphere` 121); the log
lists each brick type's shapes the first time it's shown.

Fallback if the reflected shapes don't match what PhysX has: read the live shapes from the body
instance's actor (`PxRigidActor::getShapes`, `PxShape::getGeometry`/`getLocalPose`, convex
polygons from `PxConvexMesh`), PhysX vtables found like `PxTriangleMesh`'s in `brick-models.md`.

## Drawing meshes

What a DLL can draw in the 3D view, from simplest:

- Lines: the world's line batcher (`game/line_batcher.rs`), unlit, any color.
- Bricks recolored by a view mode (`editor-view-modes.md`): the game's materials only.
- Meshes of our own, with any of the game's materials: static meshes built at run time, shown by
  `UStaticMeshComponent`s added to the bricks' actor.

### Procedural meshes get no custom primitive data

The first version (2026-10-12) drew the shapes as `UProceduralMeshComponent`s (the engine's
ProceduralMeshComponent plugin is compiled into the game). In game, the brick materials on them
showed neither the brick's color nor its pattern, and their textures looked like one averaged
color; spinners' shapes didn't show at all. Cause: a procedural mesh is drawn the dynamic way
(`GetDynamicMeshElements`), with a primitive uniform buffer it fills each frame through
`FDynamicPrimitiveUniformBuffer::Set`, and in UE 4.27 no overload of `Set` takes custom primitive
data (BRMK's ProceduralMeshComponent module imports
`?Set@FDynamicPrimitiveUniformBuffer@@QEAAXAEBUFMatrix@@0AEBUFBoxSphereBounds@@1_N222@Z`): the
material reads zeros, where brick materials expect the color, pattern and transform data.
Static meshes are drawn from their scene proxy's own uniform buffer, which has it.

### Static meshes built at run time

Everything but the normals is reflected:

1. `reflection::new_object(StaticMesh, outer = the brick component's package, NAME_None,
   RF_Transient)`, then `UStaticMesh::CreateStaticMeshDescription(Outer = the mesh)` (static).
2. `UMeshDescriptionBase`: `CreatePolygonGroup`, then per vertex `CreateVertex`,
   `SetVertexPosition`, `CreateVertexInstance` and `UStaticMeshDescription::SetVertexInstanceUV`,
   per triangle `CreatePolygon(PolygonGroupID, VertexInstanceIDs, NewEdgeIDs)` (the engine
   allocates `NewEdgeIDs`; the mod frees it). Triangles in UE's winding (clockwise from the front).
3. A material slot: `UStaticMesh::AddMaterial` returns its name, given to the polygon group with
   `SetPolygonGroupMaterialSlotName`.
4. Normals: no reflected function sets them, and the run-time build doesn't compute them, but the
   reflected `UStaticMeshDescription::CreateCube` (game 0x225DFF0) ends with
   `FStaticMeshOperations::ComputePolygonTangentsAndNormals(MeshDescription, 0.0f)` (0x225CFC0)
   and `ComputeTangentsAndNormals(MeshDescription, Normals | Tangents = 3)` (0x225D330), the mesh
   description being the `FMeshDescription` at +0x28 in the object (BRMK: +0x30). The mod finds
   both calls and the offset with a signature on that code (`mesh_builder::COMPUTE_NORMALS`) and
   calls them on its own description. Both register the attributes they need (BRMK:
   `ComputePolygonTangentsAndNormals` calls `RegisterPolygonNormalAndTangentAttributes`). A vertex
   instance's normal averages the polygons around its vertex, so flat faces get vertices of their
   own and round shapes share theirs.
5. `UStaticMesh::BuildFromStaticMeshDescriptions` (exec at game 0x2B7B090, function 0x291C110):
   `BuildFromMeshDescriptions` (0x291B750) with `bBuildSimpleCollision` false. The game itself
   doesn't call it (its spinner meshes take another path); the engine code is stock 4.27.

The mod keeps the meshes it built (by what they're made of) through weak pointers: the engine
frees one when no component shows it any more.

### The components

`game/mesh_component.rs`, all reflected:

1. `AActor::AddComponentByClass(StaticMeshComponent, bManualAttachment = true, RelativeTransform,
   bDeferredFinish = false)` on the brick component's owner (`UActorComponent::GetOwner`): made,
   given the transform, registered, not attached.
2. `SetCollisionEnabled(NoCollision)`, then `USceneComponent::K2_AttachToComponent(Parent = the
   brick's component)`, attachment rules 0 (keep relative): it follows the brick's component.
3. `SetStaticMesh`, `SetMaterial`, `SetCustomPrimitiveDataVector4` (from the brick component's
   reflected `CustomPrimitiveData.Data`)
   (the reflected offset of `Data` was 0x10 too low in game 1.11.7 (0x228, really 0x238), found
   in game by writing a probe value and scanning the component for the array holding it:
   `MeshComponents::check_custom_data`), `SetRelativeScale3D`, `K2_DestroyComponent(Object = its
   owner)`.

### The selection outline

Selected and hovered bricks are outlined through custom depth: `UBrickEditorObject::
UpdateEditorVisualization` (BRMK 0xF88C0; overridden by sprocket wheels, thrusters and winches)
updates the brick's look (where the view modes' primitive data hook runs), then
`UpdateDesiredCustomDepth` (0xF71B0) works out the outline in `EditorParams` (+0x14: the stencil
value, from the selection state, mirror pairing and groups; +0x16 bit 0: render custom depth), then
calls `SetRenderCustomDepth` and `SetCustomDepthStencilValue` on the root component. Since that
comes after the hook, the Collision view mode copies the reflected `bRenderCustomDepth` and
`CustomDepthStencilValue` from each brick's component to its shapes' every frame (the editor's
tick, shared through `brick_editor::after_tick`), calling the setters only on changes. (In the first
test, bricks showing the hidden material had no visible outline.)

### Some brick materials shape their mesh

Wheels' and spinners' materials deform their mesh from extra custom primitive data (BRMK, found by
listing every `SetCustomPrimitiveData*` call site of the game module): `UBrick::UpdatePrimitiveData`
(0x42C40, slots 0 to 17: the color, then the transform data the plain materials read) then, for
wheels, `UWheelBrick::PostModifyBrickProperty` (0x1DBA90) slots 18 to 20 (the width and the
radius minus the static info's, then a `GetBodySetupTargetRadius`-like virtual, +0x5A8),
`UCarWheelBrick::UpdateTireMaterial` (0x194290) more; for spinners
`USpinnerBrick::PostModifyBrickProperty` (0x167890) slots 18 to 22: `SizeX`, `SizeY` (m), the
radius twice (m) and the angle (rad), e.g. 1.0, 1.1, 0.8, 0.9, 2.094 for a 1 x 1.1 m spinner of
radius 0.8 and 0.9 m and 120 degrees. A spinner's static mesh is a generic +-150 x +-150 x +-50 cm
one that its material bends into the arc; a wheel has 29 values, a spinner 23, a plain brick 18.

What the shapes looked like with the brick's own material (2026-10-12, all in game): wheels' as a
dome with a spike or an egg depending on the diameter, spinners' thin and the wrong way around;
with slots 18 and up set to 0, wheels' were clean spheres 2 to 3 times too big and spinners' ~1 cm.
With a plain scalable brick's material they were right. So the material, not the data, is the
problem: such a material can't draw any other mesh.

Where the materials come from: `UBrick::GetBrickMaterial` (BRMK 0xA96C0) =
`UVehicleResourceSubsystem::FindOrCreateVehicleMID(BrickMaterial (brick +0xB0), base, pattern
class)` (0x48ED50), the base being the static info's `DefaultMaterial` (BRMK +0xF0), or
`TranslucentMaterial` (+0xF8) when a float of the brick material (+0x80) is below a constant. The
cached instance is a `UMaterialInstanceDynamic::Create(base, transient package)` with parameters
set by name from the brick material (`BaseTiling`, textures `Diffuse`, `Normal`, `Emissive`, ...)
and the pattern (texture `Texture`). `UBrickStaticInfo` reflects `StaticMesh`,
`DefaultBrickMaterial`, `DefaultMaterial`, `TranslucentMaterial`, `HiddenMaterial`,
`PhysicsVisualizationMaterial` and `ColorVisualizationMaterial`; the C++ constructors set the
defaults (the code referencing each asset path string of the game module):

| Static info constructor | `DefaultMaterial` (+ `_Translucent`) | `HiddenMaterial` |
|---|---|---|
| `UBrickStaticInfo` | `M_Brick` | `M_HiddenObject` |
| `UScalableBrickStaticInfo` | `M_ScalableBrick` | (inherited) |
| `UWheelBrickStaticInfo` | `M_WheelBrick` | `M_HiddenWheel` |
| `USpinnerBrickStaticInfo` | `M_SpinnerBrick` | `M_HiddenSpinner` |
| `UFlapBrickStaticInfo` | `M_FlapBrick` | `M_HiddenFlap` |
| `ULightBrickStaticInfo`, math, RC, sensor, switch, thruster | `M_LightBrick`, `M_MathBrick`, ... | (inherited) |

The tell for a material that deforms the mesh is a hidden material of the type's own: a hidden
brick must keep its shape, so its hidden material has to deform the same way (seen in game for
wheels and spinners; flaps by the same reasoning, not checked). That's the test
`game/brick_materials.rs` uses (no type named): when a brick's static info has another
`HiddenMaterial` than `ScalableBrickStaticInfo`'s default object, its shapes show the *plain
version* of its material: a new `UMaterialInstanceDynamic` of the plain type's `DefaultMaterial`
(or `TranslucentMaterial` if the brick's material is an instance of its own type's translucent one)
made with the reflected static `UKismetMaterialLibrary::CreateDynamicMaterialInstance(context,
Parent, NAME_None, EMIDCreationFlags::Transient)` (Engine 0xCBAC20: transient package with that
flag), then `K2_CopyMaterialInstanceParameters(Source = the brick's material, bQuickParametersOnly =
false)` (Engine 0xEC4770: `UMaterialInstance::CopyMaterialInstanceParameters`, each parameter of
the new instance takes the source's value if it has it). One per source material, kept weakly.
Hidden bricks' shapes show the plain hidden material (`M_HiddenObject`). Other types keep their
own material on their shapes. Custom primitive data is copied whole.

Pitfall (2026-10-12): `UPrimitiveComponent::CreateDynamicMaterialInstance(Index, Source)` (Engine
0x89DE30) sets `Source`, then returns the slot's material *as is* if it's already a dynamic
instance. Given a brick's instance, it made nothing and handed back the game's cached instance,
which every brick of that material and pattern shares: the first plain-look version copied the
wheel's and spinner's parameters into it.
