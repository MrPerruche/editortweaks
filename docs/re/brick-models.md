# Brick models: reading a brick's triangles

How the mod gets the faces, edges and vertices of a brick's model for fast editing on models
(`features/brick_resize/model.rs`, `game::brick_model`). Found 2026-10-11 in BRMK 1.11.6 and the
game's PhysX 3.4 DLLs (game 1.11.7, build 25831956).

## Render triangles are GPU-only

Bricks are drawn by a `UBrickStaticMeshComponent` (a `UStaticMeshComponent`) showing a plain
`UStaticMesh` (`UBrick::GetStaticMesh`, virtual; `UScalableBrick` and `USpinnerBrick` override it,
spinners making theirs at run time in `UVehicleResourceSubsystem::FindOrCreateSpinnerMesh`). A
cooked UE 4.27 game keeps a static mesh's vertex and index buffers on the CPU only if the mesh sets
`bAllowCPUAccess` (`FStaticMeshLODResources::SerializeBuffers`: `bNeedsCPUAccess =
!RequiresCookedData() || bAllowCPUAccess`; the RHI discards the arrays after upload). None of the
406 brick meshes in BRMK's `Content/BrickRigs/Bricks/Meshes` sets it (the name doesn't occur in
their name tables), so the render triangles can't be read.

## Complex collision has them

The brick meshes use `CollisionTraceFlag = CTF_UseSimpleAndComplex` (in their name tables), so the
cook also makes a PhysX triangle mesh from the render triangles of the collision LOD, for precise
traces. PhysX welds vertices while cooking (UE's cooking parameters), so the triangles share their
corners: faces, edges and vertices can be found from them.

Where it is:
- `UBrickStaticMeshComponent::GetBodySetup` (BRMK 0x142F20) asks its brick (weak pointer at BRMK
  +0x620) through a virtual (BRMK +0x3E8) = `UBrick::GetBodySetup` (BRMK 0xA6970):
  `GetStaticMesh()->BodySetup` (BRMK +0x250, the reflected `UStaticMesh::BodySetup`). So a brick's
  body setup is its mesh's, shared by every brick of the type (except spinners: their own,
  `USpinnerBrick::GetBodySetup` reads +0x110). The mod reads the mesh from the brick's root
  component (`UBrickEditorObject::GetRootComponent`, reflected) and its reflected
  `UStaticMeshComponent::StaticMesh`.
- `TriMeshes` (`TArray<physx::PxTriangleMesh*>`) is not reflected. It's the last field of
  `UBodySetupCore`, after its reflected ones, and `UBodySetup` starts with the reflected `AggGeom`:
  - BRMK: `UBodySetupCore` is 0x50 bytes (`GetPrivateStaticClass`, PhysicsCore 0x2DD630), its
    constructor (0x25C700) zeroes a 16-byte field at +0x40 after `CollisionReponse` (+0x3E), which
    `UBodySetup::UpdateTriMeshVertices` (BRMK engine 0x1175BD0) reads as the triangle meshes, and
    `AggGeom` is at +0x50.
  - Game: `UBodySetupCore` is 0x48 bytes (its `GetPrivateStaticClass` at 0x1BEF040, found from the
    UTF-16 name `UBodySetupCore`), its constructor (0x1BE93F0) zeroes +0x28 (`BoneName`), +0x31 and
    +0x38/+0x40, and `AggGeom` is at +0x48: `TriMeshes` at +0x38.

  The mod takes `AggGeom`'s offset minus 0x10 and checks that `AggGeom` starts where
  `UBodySetupCore` ends (its reflected size) and that `UBodySetupCore`'s reflected fields end
  before. (The first version expected the array after `UBodySetupCore` and turned itself off in
  game: "AggGeom at 0x48, BodySetupCore ends at 0x48".)
- Scalable bricks scale the body setup like their mesh: `UScalableBrick::GetBodySetupScale3D`
  (BRMK 0x142F70) = `BrickSize` / a base size in its static info (BRMK +0x11C, a C++ default:
  `BP_ScalableRamp` doesn't set it). The mod doesn't read it: it scales the model's box onto the
  brick's box (`BrickModel::fit`), and leaves bricks whose box isn't the model's box scaled around
  the origin (bricks with cosmetic meshes) on their box.

## PhysX's triangle mesh

`PhysX3Common_x64.dll` (shipped in `Engine/Binaries/ThirdParty/PhysX3/Win64/VS2015/`), no RTTI, so
vtables sit back to back in `.rdata`. `Gu::BV33TriangleMesh`'s vtable (RVA 0x160508 in the shipped
DLL) was found from its `getConcreteTypeName` (0x10DD20, `lea rax, "PxBVH33TriangleMesh"`); its
entries match PhysX 3.4's `PxBase` then `PxTriangleMesh` declarations, and BRMK's
`UpdateTriMeshVertices` calls +0x28 and +0x38:

| Offset | Function | Implementation in the DLL |
|---|---|---|
| +0x00 | `release` | refcount decrement |
| +0x08 | `getConcreteTypeName` | "PxBVH33TriangleMesh" (or "PxBVH34TriangleMesh") |
| +0x10 | `isReleasable` | |
| +0x18 | destructor | |
| +0x20 | `isKindOf` | |
| +0x28 | `getNbVertices` | `mov eax, [rcx+0x20]` |
| +0x30 | `getVertices` | `mov rax, [rcx+0x28]` (`PxVec3`, 3 floats each) |
| +0x38 | `getVerticesForModification` | |
| +0x40 | `refitBVH` | returns `PxBounds3` through a hidden pointer |
| +0x48 | `getNbTriangles` | `mov eax, [rcx+0x24]` |
| +0x50 | `getTriangles` | `mov rax, [rcx+0x30]` |
| +0x58 | `getTriangleMeshFlags` | writes the flags byte to `[rdx]`, returns `rdx` |

`getTriangleMeshFlags` returns a class (`PxFlags`), so MSVC passes a hidden return pointer after
`this`. `PxTriangleMeshFlag::e16_BIT_INDICES` (bit 1) means 16-bit indices, else 32-bit. Vertices
are in the mesh's own space (UE and PhysX share coordinates). The mod checks each function pointer
is in the DLL's code and the type name before calling the others, and every index against the
vertex count.

## Not checked yet (first in-game test)

- That editor bricks' body setups have their triangle meshes made (`UBodySetup::CreatePhysicsMeshes`
  runs when a component makes its physics state, or for async creation). If not, the log says
  "a brick keeps its box on models: no complex collision"; `CreatePhysicsMeshes` is virtual in this
  engine (BRMK export `?CreatePhysicsMeshes@UBodySetup@@UEAAXXZ`) and could be called.
- That the collision LOD's triangles match the render bounds (else `fit` refuses the model).
