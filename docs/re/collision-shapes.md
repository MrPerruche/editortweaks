# Collision shapes: what a "hitbox" view mode would draw

Feasibility notes for a view mode showing bricks' collision (2026-10-12, BRMK 1.11.6, game 1.11.7
build 25831956). Nothing is implemented yet.

## Simple and complex collision

The brick meshes use `CTF_UseSimpleAndComplex` (see `brick-models.md`): PhysX simulates contacts
with the simple shapes (`UBodySetup::AggGeom`: boxes, spheres, capsules, convex hulls), and
precise traces (`bTraceComplex`) hit the complex triangle mesh, which is the render mesh's
collision LOD. "Hitbox" for vehicle-against-world contacts means the simple shapes. Which traces
in the game are complex (editor picking, weapons) isn't checked.

## Bricks use the engine's body instance, scaled by the game

- `UBrickStaticMeshComponent::ShouldCreatePhysicsState` (BRMK 0x1868D0): only if a flag of the
  brick's static info is set (BRMK `UBrickStaticInfo` +0xEA bit 0x20, through 0x43E50), then the
  engine's check. Bricks without it have no collision at all. Which reflected property the flag
  is isn't known yet.
- `UBrick::CreatePhysicsState` (BRMK 0x929E0) ends in the engine's
  `UActorComponent::CreatePhysicsState`: ordinary `FBodyInstance` bodies from the brick's body
  setup (the mesh's, or a spinner's own `SpinnerBodySetup`, reflected).
- `UBrick::UpdateBodySetupScale3D` (BRMK 0xF6020) takes the brick's virtual
  `GetBodySetupScale3D` (BRMK vtable +0x430; `UScalableBrick`: `BrickSize` / base size,
  `UWheelBrick`: from `GetBodySetupTargetRadius`), clamps each axis to a minimum (`maxss` with a
  constant at BRMK 0x776478), and passes it to `UBrickStaticMeshComponent::SetBodySetupScale3D`
  (0x182EF0) = `FBodyInstance::SetBodySetupScale3D` on the component's `BodyInstance` (+0x320).
- `FBodyInstance::SetBodySetupScale3D` is the game's engine change (BRMK engine 0x3DC7E0, not in
  stock 4.27): it stores the scale in a new field at body instance +0x6C (next to the stock
  `Scale3D` at +0x60) and calls `UpdateBodyScale`. Not reflected (the name isn't in the game exe).

That virtual is also what scales the mesh's bounds in `GetBrickEditorObjectLocalBounds` (see
"Duplicates" in `editor-view-modes.md`), so the shapes' scale is local bounds / mesh bounds, the
fit `features/brick_resize/model.rs` already does for models. Wheels need checking: their scale
comes from a target radius.

## Reading the shapes by reflection

`UBodySetup::AggGeom` (`FKAggregateGeom`) and its element structs are reflected in the shipping
game (their names are in the exe: `AggGeom`, `BoxElems`, `SphereElems`, `SphylElems`,
`ConvexElems`, `VertexData`, `IndexData`, `ElemBox`, `KConvexElem`, `KBoxElem`):

| Element | Reflected fields (UE 4.27) | Lines |
|---|---|---|
| `FKBoxElem` | `Center`, `Rotation`, `X`, `Y`, `Z` | 12 edges |
| `FKSphereElem` | `Center`, `Radius` | 3 circles |
| `FKSphylElem` | `Center`, `Rotation`, `Radius`, `Length` | rings and side lines |
| `FKConvexElem` | `VertexData` (`TArray<FVector>`), `IndexData`, `ElemBox`, `Transform` | hull edges |

All are in the mesh's space. A convex element's faces would come from its vertices (a hull,
merged into faces like `Model::new` does for triangles); `IndexData` may be empty with PhysX.
Not checked: that cooked meshes keep `VertexData` (it's a plain `UPROPERTY`, so it should be
serialized; log the counts in the first test).

Fallback if the reflected shapes don't match what PhysX has: read the live shapes from the body
instance's actor (`PxRigidActor::getShapes`, `PxShape::getGeometry`/`getLocalPose`, convex
polygons from `PxConvexMesh`), PhysX vtables found like `PxTriangleMesh`'s in `brick-models.md`.

## Drawing

`game::line_batcher` (world's foreground line batcher) draws lines that stay until replaced; a
view mode from `game::editor_view::add_view_mode` could paint the bricks see-through
(`HiddenMaterial`) or gray and draw the shapes once per change. Line count is the open cost: a
brick's hull is about 12 to 40 edges, so a few thousand bricks make about 100,000 lines, all
drawn every frame by the batcher's scene proxy. Not measured.
