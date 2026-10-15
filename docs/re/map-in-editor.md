# The map in the editor, garages, and where a spawned vehicle lands

What the "city cover" idea (roadmap: **Map in the editor**, report `docs/city-cover-helper.md`)
relies on: where the vehicle editor is in the world, how it hides the map, and how a garage places
a vehicle. Found 2026-10-14 in BRMK 1.11.6 (`BrickRigsModKitSteam-BrickRigs.dll` exports), with
the hiding checked in game 1.11.7 (build 25831956) where a game address is given. Nothing of it is
used by the mod yet.

## Where the editor is

`ABrickPlayerController::SpawnBrickEditor(UClass*)` (BRMK 0x2E0130) spawns the editor actor with
`UWorld::SpawnActor(Class, (0, 0, 10000), FRotator::ZeroRotator)`: the editor is an actor of the
same world as the map, 100 m above the world origin, unrotated. `SetBrickEditor` (BRMK 0x2DD140)
then destroys the previous one, stores it (reflected `BrickEditor` on the controller, next to
`EditorEntryPoint`: the actor the editor was entered through, e.g. a garage), updates the HUD and
view target, and calls `AWorldSetupActor::SetIsLocalPlayerInEditor` (BRMK 0x4A7210: the sky's
editor material, `EditorSkyMaterial`, and `UpdateWorldSetupParams`).

The editor works in its own frame:

- `ABrickEditor::GetEditorTransform` (BRMK 0xACFA0) is its root component's `ComponentToWorld`.
- Bricks' root components are attached to the editor's root with their spawn transforms as
  relative transforms (`editor-operations.md`), so they follow it.
- The camera is relative (`GetCameraRelativeLocation`/`Rotation`; `GetCameraViewPoint`, BRMK
  0xA9FE0, composes it with the root's `ComponentToWorld`).
- `GetEditorBounds` (BRMK 0xAC9A0) and `ConstraintLocationToEditorBounds` (0x903D0) use the static
  info's local box.

## How the editor hides the map

The engine (UE 4.27 `APlayerController::BuildHiddenComponentList`, called by
`ULocalPlayer::CalcSceneView` for each view) calls the virtual `UpdateHiddenActors(ViewLocation)`,
adds the components of `HiddenActors` and of `HiddenPrimitiveComponents` (both reflected) to the
view's hidden set, then calls the virtual `UpdateHiddenComponents(ViewLocation,
TSet<FPrimitiveComponentId>& Out)`. Brick Rigs overrides both:

- `ABrickPlayerController::UpdateHiddenActors` (BRMK 0x2E3530, game 0xE48BC0) empties
  `HiddenPrimitiveComponents` (game +0x300).
- `ABrickPlayerController::UpdateHiddenComponents` (BRMK 0x2E3560, vtable +0xDC8; game 0xE48BF0,
  the function right after, its vtable entry at game 0x390D308) rebuilds it and adds it to `Out`:

```text
if (Thumbnails = World->GetSubsystem<UBrickThumbnailSubsystem>())
    Thumbnails->UpdateHiddenComponents(HiddenPrimitiveComponents);
if (BrickEditor && BrickEditor->EditorInterfaceComponent->GetUGCType() == 1)  // vtable +0x640 BRMK, +0x540 game; 1 = vehicle
{
    Keep = { AWorldSetupActor::Get(this), BrickEditor };                        // game 0xF58410
    for (AActor* Actor : TActorIterator<AActor>(World))                         // every actor of every level
        if (!Keep.Contains(Actor))
            for (UPrimitiveComponent* C : Actor->GetComponents<UPrimitiveComponent>())
                if (C->IsRegistered())
                    HiddenPrimitiveComponents.Add(C);
    for (ULevel* Level : World->GetLevels())
        for (UModelComponent* C : Level->ModelComponents)                      // BSP (BRMK level +0xD0)
            HiddenPrimitiveComponents.Add(C);
}
for (C : HiddenPrimitiveComponents) if (C valid && registered) Out.Add(C->ComponentId);   // as the engine does
```

So in the vehicle editor, every frame, every component of every other actor is hidden from the
player's view: the map, characters (the player's own, standing where the editor was entered),
vehicles. Nothing is unloaded and the map keeps its collision: the editor's clicks trace
`ECC_Visibility` (`selection.md`), so map geometry around the editor blocks them unseen.

`UpdateHiddenActors`' whole body is unique in game 1.11.7: `8b 91 ?? ?? 00 00 c7 81 ?? ?? 00 00 00
00 00 00 39 91 ?? ?? 00 00 74 0c 48 81 c1 ?? ?? 00 00 e9 ?? ?? ?? ?? c3` (found with `et.py find`
from BRMK's bytes). `UpdateHiddenComponents` starts at the next 16-byte boundary, and its gate is
`call [rax+0x540]; cmp al, r15b` (r15 = 1) at game 0xE48DA6.

The hidden set is a `TSet<FPrimitiveComponentId>` (engine template, default allocators): a sparse
array of 12-byte elements (`{Id, HashNextId, HashIndex}`), its allocation bits, free list, then
the hash buckets; `GetTypeHash(FPrimitiveComponentId)` is the id itself.
`UPrimitiveComponent::ComponentId` isn't reflected.

## Garages

`ABrickGameState::SpawnPointArray` (reflected; `GetSpawnPointArray`, BRMK 0x23DA90) holds the
spawn points: actors implementing `ISpawnPointInterface` (`AGarage`, `ASpawnArea`, `ATeamBase`,
`ACapturePoint`, `ABrickVehicle`). Only `AGarage` spawns vehicles.

`AGarage : ABrickBuilding, ISpawnPointInterface` (the interface at BRMK +0x3B0). Reflected fields
(BRMK offsets): `VehicleSpawnExtent` (+0x410), `VehicleSpawnOffset` (+0x41C),
`VehicleSpawnRotation` (+0x428), `PinnedVehicleSpawnOrigin` (+0x434), `PlayerSpawnPoint`,
`HUDIconProperties`; from `ABrickBuilding`: `BuildingBoundsOrigin` (+0x330), `BuildingBoundsExtent`
(+0x33C). A `VehicleSpawnBox` name is in the binary too (likely a component showing the box).

The interface's table (game-defined: `??_7AGarage@@6BISpawnPointInterface@@@`): +0x30
`CanPlayerSpawn`, +0x38 `GetSpawnPoint`, +0x40 `CanPlayerSpawnVehicle`, +0x48
`GetVehicleSpawnPoint(ABrickPlayerController*, bool bPinned, FVector&, FRotator&)`, +0x50
`GetVehicleSpawnBounds() -> FSpawnPointVehicleBounds`.

- `GetVehicleSpawnBounds` (BRMK 0x1CE240): `{Extent = VehicleSpawnExtent, Alignment = (-1, 0, -1)}`.
- `GetVehicleSpawnBoxRelativeTransform(bool bPinned)` (BRMK 0x1CE2B0):

```text
bUsePinned = bPinned && !PinnedVehicleSpawnOrigin.IsZero();
Base  = bUsePinned ? PinnedVehicleSpawnOrigin : VehicleSpawnOffset;
Along = bUsePinned ? 0 : |Dot(VehicleSpawnExtent, VehicleSpawnRotation.Vector())|;
Location = (BuildingBoundsOrigin.X + BuildingBoundsExtent.X + Along + Base.X,   // in front of the building
            Base.Y,
            Base.Z + VehicleSpawnExtent.Z);
return FTransform(VehicleSpawnRotation, Location, 1);
```

- `GetVehicleSpawnPoint` (BRMK 0x1CE410): that transform times the garage root's
  `ComponentToWorld` (scale handled), out as location and rotation.

`bPinned` comes from the spawn request through the restart lambda (BRMK 0x21A080, a captured
bool); the vehicle browser's spawn option `VehicleSpawnInvincibility` (reflected on
`UUGCBrowserWidget`) has a `Pinned` value (`EPlayerVehicleSpawnInvincibility::Pinned`). Which one
sets it is still to confirm.

## Where a vehicle lands

`ABrickVehicle::GetVehicleSpawnOffset(FVector& Location, const FQuat& Rotation, AActor*
SpawnPoint, bool bCheckCollision)` (BRMK 0x434340):

```text
Half = (Bounds.Max - Bounds.Min) / 2;  Center = (Bounds.Min + Bounds.Max) / 2;   // vehicle +0x68C, +0x698 (BRMK)
Offset = (0, 0, Half.Z);
if (SpawnPoint's ISpawnPointInterface: B = GetVehicleSpawnBounds(); B.Extent isn't ~zero)
    Offset = Max(B.Extent - Half, 0) * B.Alignment;      // garage: back (-X) and floor (-Z) of its box, centred in Y
Location += Rotation * Offset;
if (!Vehicle[+0x964] && bCheckCollision)
    overlap test at Location: a box of Half minus a tolerance, every channel blocking (WorldStatic
    ignored at a spawn point with bounds); if blocked, BRMK 0x41AC50 tries moving it right, left,
    forward, back, up;
Location -= Rotation * Center;
```

The vehicle's bounds box centre is what gets placed, not its origin: on each axis where the vehicle
is bigger than the garage's box, it is centred on the spawn point there. A vehicle bigger than the
box on every axis (a city cover, with bricks far out) has its bounds centre exactly on the spawn
point.

The bounds come from `UBrickEditorInterfaceComponent::CalcBrickEditorObjectsBounds(Min, Max)`
(BRMK 0x8C070): the union of each object's `GetBrickEditorObjectSpawnRelativeBounds` (BRMK 0xA8270:
its local bounds through its transform relative to the root, `GetBrickEditorObjectBounds`). The
vehicle and the editor both hold their bricks in that component class, so the editor can work out
the same box (reflected per object: `GetBrickEditorObjectLocalBounds`,
`GetBrickEditorObjectSpawnTransform`).

Callers: `ABrickGameMode::RestartPlayerAt` (BRMK 0x2588F0) calls `ABrickVehicle::RepairVehicle(
Location, Rotation, SpawnPoint, true, Type == 1)` (BRMK 0x448B00), which applies the offset and
stores the restart transform; the vehicle's post-construct (BRMK 0x4546C0) computes the bounds
(`CalcBrickEditorObjectsBounds` into +0x68C), applies the offset again from its root's place and
`SetWorldLocation`s. How the two passes combine for a new vehicle isn't untangled: compare a
prediction with a spawned vehicle in game.

## The panel when nothing is selected

`UBrickEditorWidget::UpdatePropertiesPanel` (BRMK 0x370BB0): with a selection,
`UPropertiesPanelWidget::AddPropertiesForSelection`; without, `AddProperties(UBrickUserSettings,
Filter)` (the editor's user settings) and `AddProperties` for one more object from a weak pointer
(likely the current editor mode). `features/settings_menu` already hooks
`UBrickUserSettings::ReflectBrickProperties` (`property_reflection::REFLECT_USER_SETTINGS`); the
editor panel's filter tag is native (not in `DefaultGameplayTags.ini`): log it from that hook.
