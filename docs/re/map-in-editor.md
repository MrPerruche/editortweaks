# The map in the editor, garages, and where a spawned vehicle lands

What the "city cover" idea (roadmap: **Map in the editor**, report `docs/city-cover-helper.md`)
relies on: where the vehicle editor is in the world, how it hides the map, and how a garage places
a vehicle. Found 2026-10-14 in BRMK 1.11.6 (`BrickRigsModKitSteam-BrickRigs.dll` exports), with
the hiding checked in game 1.11.7 (build 25831956) where a game address is given. Used by
`features/map_in_editor` since 2026-10-15 (see "Built" and after, at the end), tested in game since.

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
vehicles. Nothing is unloaded and the map keeps its collision. The editor's clicks trace
`ECC_Visibility` (`selection.md`) but don't hit the hidden map: `GetEditorTraceParams` (game
0xCE2860, see "Multiplayer") adds the controller's `HiddenPrimitiveComponents` (+0x300), the list
above, to the query's ignored components. (Corrected 2026-10-15: this said the map blocked them.)

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
bool), set by `ABrickGameMode::RestartPlayerAt` (BRMK 0x2588F0; found 2026-10-15):

- A new vehicle: `bPinned = Request.VehicleSpawnInvincibility == Pinned` (0x2589A3:
  `cmp byte [request+0x111], 2; sete`). `FPlayerSpawnRequest` (reflected) +0x111 is copied from the
  vehicle browser's `UUGCBrowserWidget::VehicleSpawnInvincibility` (BRMK +0x54C, `__PPO__` 0x101410)
  by `MakeVehicleSpawnRequest` (BRMK 0x3D1C00). `EPlayerVehicleSpawnInvincibility`: None 0,
  Invincible 1, Pinned 2. The browser Blueprint (`WBP_UGCBrowser`) doesn't set it and nothing saves
  it (no ini key; only `CycleVehicleSpawnInvincibility`, `UpdateButtons` and
  `MakeVehicleSpawnRequest` use the field), so it starts at None and the player cycles it in the
  browser. The map's garage icons (`UHUDIconComponent::MakeSpawnRequest`) leave it at None.
- Respawning the player's current vehicle (request types 1 and 2): that vehicle's pin mode
  (`ABrickVehicle` +0x964, `GetPinMode` BRMK 0xB32E0, non-zero when pinned).

After `SpawnActor` a new vehicle gets `SetCanBeDamaged(option != Invincible)` (BRMK 0x2DD360) and
`SetPinMode(2)` when the option is Pinned (call at 0x259102), both before `ConstructVehicle`
(0x25911F).

**City2's garages** (the mod kit's `City2.umap`, exported with UAssetGUI, 2026-10-15). The 14 in
game 1.11.7's list are 13 `BP_Editor_*` buildings plus `BP_TrainStation_2` (garage 1).
`AGarage`'s constructor (BRMK 0x1A6780) sets `VehicleSpawnExtent` to (5000, 5000, 2400) and
`VehicleSpawnOffset.X` to 50, and leaves `VehicleSpawnRotation` and `PinnedVehicleSpawnOrigin` at
zero. Only `BP_Editor_Medium_2`, the central garage (garage 12, at (-17690, -5493), yaw 90), sets
`PinnedVehicleSpawnOrigin`: (5053, 0, 0). No other kit asset has the name (a byte search of every
`.uasset` and City2 `.umap`). So everywhere else a pinned vehicle lands like any other.

At the central garage the normal spawn point is 570 + 2500 + 50 = 3120 cm in front of the garage
(the building's extent, the box's reach, the offset). That is also what the mod predicted in game
from the garage's fields at run time. The pinned one is 570 + 5053 = 5623 cm, so a pinned vehicle
lands 2503 cm further along the garage's forward. The `placement` test
`a_pinned_cover_lands_25_m_further_out_at_the_central_garage` checks this.

## Where a vehicle lands

`ABrickVehicle::GetVehicleSpawnOffset(FVector& Location, const FQuat& Rotation, AActor*
SpawnPoint, bool bCheckCollision)` (BRMK 0x434340):

```text
Half = (Bounds.Max - Bounds.Min) / 2;  Center = (Bounds.Min + Bounds.Max) / 2;   // vehicle +0x68C, +0x698 (BRMK)
Offset = (0, 0, Half.Z);
if (SpawnPoint's ISpawnPointInterface: B = GetVehicleSpawnBounds(); B.Extent isn't ~zero)
    Offset = Max(B.Extent - Half, 0) * B.Alignment;      // garage: back (-X) and floor (-Z) of its box, centred in Y
Location += Rotation * Offset;
if (!Vehicle[+0x964] && bCheckCollision)                 // +0x964: the pin mode (GetPinMode)
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

The overlap fallback (BRMK 0x4345F0 on, 2026-10-15): `OverlapAnyTestByChannel(Location, Rotation,
channel 26, box of Half minus a tolerance)`, every response Block except WorldStatic (Ignore when
the spawn point has bounds, as garages do), the vehicle itself ignored. If anything overlaps, the
lambda 0x41AC50 is tried with Right, -Right, Forward, -Forward, then Up (each turned by `Rotation`)
and a distance of some value plus the vehicle's half size on that axis: it sweeps the box from
`Location + Rotation · (Direction · Distance)` back to `Location`, and the first sweep that hits
something without starting inside it puts the vehicle where it stopped (pulled back a little);
later directions are then skipped. It only runs for an unpinned vehicle (`!Vehicle[+0x964]`), and
a pinned spawn sets the pin mode before construction (above), so a cover spawned pinned is never
moved by it. A 2 km cover spawned unpinned in a city would almost surely be moved, by however much
the world around it at that moment asks, and would fall: covers are spawned pinned.

Reported in game (2026-10-15): with the map shown, covers were 20 to 50 m off along X, the same for
every cover, while ordinary vehicles lined up. The last test was at the central garage. The editor
then placed vehicles as if spawned unpinned ("Pinned spawn point" defaulted to off), and covers are
spawned pinned, which at that garage is 25 m further along the vehicle's X, the only City2 garage
where pinning changes anything. That fits the report, so the setting now defaults to on, as
`docs/city-cover-helper.md` planned. Still to confirm in game, by turning the setting off and on
there. An earlier test logged, for each vehicle the feature hid while the map showed, its place
and its components' box (`AActor::GetActorBounds`), to compare with the prediction. It found no
cover: the spawned cover wasn't among the vehicles hidden (gone, or already hidden by the game),
only a vehicle at the origin with no components. Calls on that one returned zeros, as for an actor
in a world whose actors aren't initialized, such as a preview. So that log was removed.

## The panel when nothing is selected

`UBrickEditorWidget::UpdatePropertiesPanel` (BRMK 0x370BB0): with a selection,
`UPropertiesPanelWidget::AddPropertiesForSelection`; without, `AddProperties(UBrickUserSettings,
Filter)` (the editor's user settings) and `AddProperties` for one more object from a weak pointer
(likely the current editor mode). `features/settings_menu` already hooks
`UBrickUserSettings::ReflectBrickProperties` (`property_reflection::REFLECT_USER_SETTINGS`).

Found 2026-10-15 (BRMK 1.11.6):

- The weak pointer (widget +0x40C) is the current editor mode. Without a selection the panel asks
  it, through its virtual +0x380, `UBrickEditorMode::GetUserSettingsPropertyFilter(
  FBrickPropertyReflectionFilter&) -> bool` (BRMK 0xB6ED0: false, no user settings), and lists
  the user settings only if it says true. `UBrickEditorDefaultMode`'s (BRMK 0xB6EA0) adds the
  native tag at +0x174 of the game's tag table (getter BRMK 0x2AA230, built by 0x284240, its
  `Properties` part by 0x285B40); `UBrickEditorMoveMode` has its own (0xB6EE0). The table is built
  name part by name part (a part added under the current tag by 0x295A60, `RequestDirectParent` to
  go back up). 0x285B40 adds `Properties` (+0xFC), then 0x285F00 adds `Settings` and its children
  `Video`, `Gameplay`, `UserInterface`, `Input`, `InputMappings`, `Host`, `Match`, `TopBar` (nine
  entries of 12 bytes from +0x108, stepping back up after each child and once more at the end),
  then `Editor` (+0x174, under `Properties`) and `Move` (+0x180, under it). So the editor panel's
  tag is `Properties.Editor` and the move mode's `Properties.Editor.Move`. Then the mode's own
  properties (`AddProperties(mode, empty filter)`) and `UpdateProperties`. The settings list logs
  each new set of tags it's asked for once (`features/settings_list.rs`), to confirm in game.
- Refreshing the panel (used): `UPropertiesPanelWidget::UpdateProperties()` is reflected (BRMK
  `execUpdateProperties` 0x58F200; the function, 0x3FAC30, is `UpdateProperties(nullptr)`). The
  event version (0x3FA760) skips if already running (+0x430), then for each listed container
  (+0x400, 0x88 bytes each) empties its reflection and runs the object's `ReflectBrickProperties`
  again (interface slot +0x10), adds every property again (`AddProperty`, reusing the widgets) and
  pools the widgets left over. So one reflected call lists our rows again with fresh values and a
  list's fresh items. `UBrickEditorWidget::PropertiesPanel` and `GetBrickEditor()` are reflected
  too; the shipping exe has all three names (game 1.11.7: `UpdateProperties` at 0x39D2468,
  `GetBrickEditor` 0x395C280, `PropertiesPanel` 0x3960248, ASCII). `game/editor_panel.rs`.
- Refreshing rows after a value changed outside the panel, the game's way (not used): the user
  settings' setters (e.g.
  `SetEditorSnappingEnabled`, BRMK 0xE7560) build an `FBrickPropertyChangedEvent` with the
  property's name and the object (BRMK 0x62B60: `(out, const FName*, UObject*, flags)`) and pass it
  to BRMK 0x33B740, which runs each object's `PostModifyBrickProperty` (interface slot 4) and then
  broadcasts the native multicast delegate at BRMK .data 0xBD12D8. Every `UPropertiesPanelWidget`
  subscribes to it in `NativeConstruct` (`OnAnyPropertyModified`, BRMK 0x3DB040: skips interactive
  events, +0x48 == 2; with +0x49 set and +0x4A clear only panels editing one of the event's
  objects, at +0x30; then `UpdateProperties(event)`). Generic helpers doing the compare, store,
  event and broadcast exist per value type (BRMK 0x60530 float, 0x606F0 byte, ...). In game 1.11.7
  the "toggle snapping" code (0xDD0A10, found from the UTF-16 `bEditorSnappingEnabled` at
  0x38FE408) shows the same: event constructor 0xCC3C20, broadcaster 0xE7E260, then the event's
  allocations freed at +0x30, +0x18 and +0x8.

## Built (2026-10-15, `features/map_in_editor`)

What was checked offline, and the choices that differ from `docs/city-cover-helper.md`:

- **The hiding hook.** `UpdateHiddenComponents` is found by `UPDATE_HIDDEN_COMPONENTS`
  (`game/hidden_components.rs`), a pattern on the gate's code (unique in 1.11.7, `sigcheck` OK, the
  function start 0xE48BF0 from `.pdata`; the prologue alone is unique too). Three independent checks
  agree on the function: the pattern, the vtable entry (`et.py vtable 0x390D308` lists 0xE48BF0 as
  slot 0 from that address), and the gate `call [rax+0x540]; cmp al, r15b` at 0xE48DA6. Arguments:
  rcx controller, rdx `const FVector&` view, r8 `TSet<FPrimitiveComponentId>&` (sparse array: data
  +0, num +8, allocation bits inline from +0x10, free count +0x34; hash size +0x48).
- **No `TSet` surgery.** The report planned to take the map's ids out of the set afterwards. Instead
  the detour runs the game's function with the controller's `BrickEditor` null (the gate loads it
  at 0xE48D5F, `test rbx, rbx; je` past the whole vehicle-hiding block, so `Keep`, the actor
  walk and the BSP are all skipped), then puts it back. The pattern's displacement at +3 is that
  field's offset; `install` compares it with reflection's `BrickPlayerController::BrickEditor` and
  disables the hook if they differ. Cost: everything is then visible, characters and vehicles
  included, so those are hidden with reflected `SetActorHiddenInGame` (restored when the map hides
  or the editor closes). Hooked on the controller's views, so it also tells the feature that the
  editor closed (lines cleared, characters shown, even though the editor no longer ticks).
- **The garage's interface isn't reflected** (no `GetVehicleSpawnBounds`/`GetVehicleSpawnPoint`
  names in the exe; they are `ISpawnPointInterface` natives). So `game/garage.rs` reads the reflected
  fields (`VehicleSpawnExtent`, `VehicleSpawnOffset`, `VehicleSpawnRotation`,
  `PinnedVehicleSpawnOrigin` on `Garage`; `BuildingBoundsOrigin`/`Extent` on `BrickBuilding`) and
  `placement.rs` redoes the formulas from BRMK (above). BRMK `GetVehicleSpawnBounds` (0x1CE240) returns
  its 24-byte struct through rdx (hidden return pointer: zero vectors, extent in the first 12 bytes),
  `GetVehicleSpawnPoint` (0x1CE410) takes `this` = garage + 0x3B0 in rcx, so calling the table
  slots stays an option if the redone formula turns out to differ.
- **Class names** are TCHAR (UTF-16) literals in the exe: `et.py find` finds `BrickPlayerController`,
  `BrickGameState`, `BrickBuilding`, `Garage`, `BrickVehicle` there, not as ASCII.
- **The panel rows** (2026-10-15, `map_in_editor/panel.rs`): a "Map" section under
  `Properties.Editor` (see "The panel when nothing is selected"; the game's own section there,
  "Editor settings", has the UI scale, view mode, mirror axis and automatic mirroring, and the
  editor's default mode adds no rows), in this order: Show map (only if the map can be shown at
  all), Garage (a dropdown: None, then "Garage 3 of 12", the entry garage marked "entered here"
  and garages with a `PinnedVehicleSpawnOrigin` "has pinned spawn point") and Pinned spawn point
  (the setting, also in the options; on by default since 2026-10-15, as planned; it was off before,
  see "Where a vehicle lands"). The garage list was first shown the game's default way, its items
  side by side in one row: with 15 items, unreadable slivers. It's a dropdown since, as the siren
  brick's type is (`docs/re/settings-menu.md`, "How a list shows"). The show map key stays
  (`features/keybinds.rs`, "Map"); the next and previous garage keys, and the zoom notification
  naming the garage, are gone. The list's items are worked out each time the panel lists it, so it
  follows the map's garages; a list holds at most 256 items (the game sets the index as a byte),
  so None and the first 255 garages. Rows only note what was asked; the next editor tick acts on
  it as on the key (showing the map with no garage selects the entry garage, else the first;
  choosing None hides the map), then has the panel list the rows again if it changed what they
  show. Before that, keys did it all: show map, next and previous garage.
- **Safety gates** (risk 2): at first the map only showed in a single-player game
  (`UKismetSystemLibrary::IsStandalone`) and with a setting accepting the editor's bricks' physics
  collision (they have 3). Replaced on 2026-10-15 by keeping the editor and the world apart: see
  "Multiplayer" at the end.
- **Existing features** compute in the vehicle's frame from `GetWorldTransform` (brick_resize,
  drag_select) or from the camera view (ortho_view), so a moved and turned editor should work, but
  none was tested that way.

## The turned editor's camera (in-game tests 2026-10-15, `features/editor_frame`)

With the map shown the editor is turned (a garage's yaw, `R`). The game's camera model, from BRMK
1.11.6 (game addresses where checked): the camera is a location **relative to the editor, in the
editor's space** (editor +0x3F0 game / +0x4C8 BRMK; through the view target interface, +0x1D0) and
a rotation **in the world's**, `C` (the interface +0x10: editor +0x230 game / +0x308 BRMK; the
player's control rotation is copied there each frame by `UpdateViewRotation`). Four functions are
only right while `R` is the identity, which it always is in the game:

- `GetCameraViewPoint` (BRMK 0xA9FE0, game 0xCE0840, slot 11 of both editors' `IViewTargetInterface`
  tables, 0x38D8B08 and 0x38DC250): location = root translation + relative location, **not turned**.
- `UpdateViewRotation(FRotator&, float)` (BRMK 0xFD400, game 0xD13C50, slot 8): while pivoting, it
  turns the relative location around the middle of the selection's bounds (the editor's space) by
  the change of the camera rotation (the world's): a yaw about Up (the same in both spaces for a
  yaw-only editor), then a pitch about `old rotation.RotateVector(Right)`, a **world** axis applied to
  an editor-space vector.
- `AddCameraMovementInput(Axis, Value)` (BRMK 0x85A40, game 0xCCD690; the fly keys, through
  `UEditorInputComponent::HandleMovementAxis`): the camera's rotation in the editor's space is
  computed as **`C · R⁻¹`** (the right one is `R⁻¹ · C`), its `Axis` (X, Y; Z is the world's up as
  is) times `Value` added to the camera input (+0x3FC game / +0x4D4 BRMK; `ABrickEditor::Tick`,
  BRMK 0xEFA30, moves the relative location by it).
- `UBrickEditorMoveMode::TickMode` (BRMK 0xF19A0 +0x174, game 0xD0C8F0 +0x103): the same `C · R⁻¹`,
  whose axes turn the mouse's and keys' input (`UBrickEditorMoveMode::AddMoveInput`) into the
  bricks' move.

`ABrickEditor::GetCameraRelativeRotation` (BRMK 0xA9E70, not called: inlined in the last two) is the
same product. The order was read from the SIMD of `UE VectorQuaternionMultiply2(Quat1, Quat2)`: Quat1's
components are broadcast (`shufps x, x, 0/0x55/0xAA/0xFF`), Quat2 is swizzled (`0x1B/0x4E/0xB1`) and
the four sign masks are `GlobalVectorConstants` (`.data`, filled at start-up, so zero in the file:
QINV_SIGN_MASK at game 0x4975110, QMULTI_SIGN_MASK0-2 at 0x49750C0/B0/A0). The camera's quaternion is
the broadcast one, the root's (times the conjugate mask) the swizzled one: `C · R⁻¹`. Replaying the
game's instructions in Python with UE's constants, a camera 35° down at a 97° garage: forward
(0.24, -0.97, 0.07), level; right (0.81, 0.16, -0.57), down. That is the second report: "forward
can't go down, left and right go up and down". `C · R⁻¹ = R⁻¹ · C` while `R` is the identity or
both are yaws only (a level camera). The first version of these notes said these two were right:
wrong, the product's order was misread.

**How the list was checked complete** (BRMK `BrickRigs.dll`, every function in `.pdata`, capstone):
every read of the rotation (+0x308..+0x310) and relative location (+0x4C8..+0x4D0) in the editor,
and of the relative location through the interface (+0x1D0..+0x1D8): only the functions above,
`BecomeViewTarget` (sets the rotation from the controller: world, right), `SetCameraRelativeLocation`
and `ABrickEditor::Tick` (editor space, right), and other classes' fields at the same offsets.
Every editor function using a quaternion's inverse (the conjugate mask): the two above, `FocusCamera`
and `FocusCameraOnBrickEditorObjects` (the view from `GetCameraViewPoint`, then
`InverseTransformPosition` by the root: right), `GetGizmoAxisAtScreenPosition` and
`GetPlaceObjectTransform` (`InverseTransformPosition`: right), and `GetBrickEditorRootSpawnTransform`
(the brick's own transform only, never the editor's: what's saved can't depend on the turn).
The interface's other slots: slot 9 (BRMK 0xAFB10) returns the stored rotation as is (world: right).
`TickMode`'s virtual calls while it runs: the editor's `GetMoveSnappingOffset` (vtable +0x7E0 BRMK)
and `UpdateMoveValidityStates` (+0x7E8), and its objects' components: none reads the camera.

**What `features/editor_frame` does** while `editor_frame::set` says which editor is turned (nothing
otherwise; game patterns `GET_CAMERA_VIEW_POINT`, `UPDATE_VIEW_ROTATION`,
`ADD_CAMERA_MOVEMENT_INPUT`, `MOVE_MODE_CAMERA` in `game/editor_camera.rs`, the last two ending on the
product's operand order, so a game that fixed it would disable the feature rather than be corrected
twice):

- `GetCameraViewPoint`: location = editor location + `R` · relative location.
- `UpdateViewRotation`: runs with the stored rotation and the one it's given brought into the editor's
  space (`R⁻¹ · rotation`), then back.
- `AddCameraMovementInput` and `TickMode` (the latter only for a mode whose outers include the turned
  editor): run with the stored rotation replaced by `R⁻¹ · C · R` (its `· R⁻¹` is `R⁻¹ · C`), then the
  stored bytes are put back exactly. Checked offline for yaws 0..180° and pitches up to ±89.5°,
  through the `f32` rotator the game reads (`editor_frame` tests). The rotation's offset is read from
  both patterns and must equal the interface's offset (reflection) + 0x10.
- When the map moves the editor (or back), the player's control rotation (`SetControlRotation`) and
  the camera's stored rotation are turned by the editor's turn: the vehicle looks the same from the
  camera, from that frame on (turning only the control rotation showed the turned place with the old
  rotation for one frame, until `UpdateViewRotation` copied it).

`ortho_view`'s camera moves (`SetCameraRelativeLocation` by a world offset) go through
`editor_frame::to_editor_space`.

Lag while moving bricks (reported with the map shown): the box was worked out again every second,
even during moves, and the editor teleported when it changed. Now the box is worked out when an
operation ends (`PostModify`, hooked through the live editor's vtable slot) or the player undoes
or redoes (`Action_Undo`/`Action_Redo` thunks), never during a move mode, and only with a garage
selected; the log says how long it took when slow.

## Multiplayer (2026-10-15, `features/map_in_editor/isolation.rs`)

In multiplayer the log said "the map isn't shown: not a single-player game": the first gate. Lifting
it needs the map to change nothing for other players, whoever hosts. Three things did.

**Hiding characters and vehicles.** `SetActorHiddenInGame` sets `AActor::bHidden`, which the engine
replicates: when hosting, it would hide them for everyone. Now they go in the player controller's
reflected `HiddenActors` (`game/hidden_actors.rs`), which the engine adds to that player's hidden
set each frame and doesn't replicate. The game only empties the array next to it: its
`UpdateHiddenActors` (game 0xE48BC0) resets the `TArray` at controller +0x300, and
`GetEditorTraceParams` (below) reads `HiddenPrimitiveComponents` at that same +0x300, through the
editor's `PlayerController` (+0x270). The feature logs once if the array is emptied anyway.

**The editor's collision.** The editor's bricks have `ECollisionEnabled` 3 (queries and physics,
logged in game). The project defines no collision profile for the editor (BRMK 1.11.6
`Config/DefaultEngine.ini`). A brick's components get their profile from a settings struct applied
by BRMK 0xBA710 (`FBodyInstance::SetCollisionProfileName`, `SetCollisionEnabled`, `SetObjectType`,
per-channel responses), called when components are made or taken back from the editor's pool
(`UBrickEditorInterfaceComponent::GetObjectPool`: one pool per interface component, so editor
components never become a vehicle's). The "Brick" profile blocks every channel but Camera. Moved
into the map, the bricks would push and block whatever this machine simulates, and be seen by its
queries.

Who simulates what (`ABrickVehicle::UpdateHasPhysicsAuthority`, BRMK 0x461960; `HasPhysicsAuthority`
reads bit 2 of +0x688): a driven vehicle by its driver's machine (the controller's
`IsLocalController`, vtable +0x830), an unoccupied one by the server (`GetNetMode() != NM_Client`).
Clients send their vehicles' movement (`ServerUpdateMovement`). So a host simulates every parked
vehicle, other players' too.

Queries that matter (BRMK `BrickRigs.dll`, every call to a `UWorld` trace, sweep or overlap by
channel, with the channel when it's a constant): `ECC_Visibility` (3) by the camera brick
(`UCameraBrick::CameraTrace`) and the camera's focal distance only, but `USensorBrick::DoSensorTrace`
(0x13B900) traces either object types or channel 3, by its mode. Turning off only physics and
non-visibility responses would have left the bricks visible to sensors on vehicles the host
simulates.

**So the editor and the world are kept apart** while the map shows:

- The editor's own queries are `GetObjectAtScreenPosition` (BRMK 0xB2240, game 0xCE5E00: a line
  trace) and `GetPlaceObjectTransform` (BRMK 0xB33B0, game 0xCE66D0: a sweep). Both take their
  channel from `ABrickEditor::GetEditorTraceParams(ECollisionChannel&, FCollisionQueryParams&)`
  (BRMK 0xACDC0, game 0xCE2860: 3) and pass `FCollisionResponseParams::DefaultResponseParam` (game
  0x4B7A148, 32 bytes before `FCollisionObjectQueryParams::DefaultObjectQueryParam`): every object
  type blocks. The editor makes no overlap query and needs no physics collision.
- `game/editor_trace.rs` hooks `GetEditorTraceParams` and the two engine functions the editor calls,
  found from the editor's call sites: `UWorld::LineTraceSingleByChannel` (game 0x243F310, its
  `RaycastSingle`) and `UWorld::SweepSingleByChannel` (game 0x24481F0; arguments: rcx world, rdx hit,
  r8 start, r9 end, then rotation, channel, shape, query and response parameters on the stack). At
  load it checks that both call sites' functions call `GetEditorTraceParams` and pass the same
  response parameters. The call must be looked for in every part of the function
  (`sig::direct_calls_in_all_parts`): `GetPlaceObjectTransform` has four unwind table entries in
  1.11.7 (0xCE66D0..0xCE6794, then chained 0xCE6794..0xCE6BEB, 0xCE6BEB..0xCE6CE0,
  0xCE6CE0..0xCE6D1E), and calls it from the second (0xCE69DC, +0x30C); `GetObjectAtScreenPosition`
  has one (its call at 0xCE5F38). Looking in the main part only refused the map everywhere in the
  first build (2026-10-15). While isolated, the editor's channel becomes a channel the project doesn't
  define (the last `ECC_GameTraceChannel` not in `UCollisionProfile::DefaultChannelResponses`: 31 in
  1.11.7, logged at start), and queries on it get responses blocking only objects of that type.
  The engine finds a hit only when both sides block: the shape's response to the query's channel,
  and the query's response to the shape's object type.
- Every primitive component of the editor that collides (`AActor::K2_GetComponentsByClass`) gets:
  object type that channel, every response ignore, and block on that channel if it had queries and
  blocked visibility (what the editor's clicks could hit stays clickable). Its `CollisionEnabled`
  stays as it was (see "Clicks lost" below). Read from the reflected `BodyInstance`
  (`CollisionEnabled`, `ObjectType`, `CollisionResponses.ResponseToChannels`), changed through
  `UPrimitiveComponent`'s setters (`game/component_collision.rs`). The check runs every tick while the map shows (new components,
  and pooled ones the game set again, are caught the same tick), before the editor moves in; the
  collision each had is put back once it's out, or when the editor closes (not for components
  being destroyed with it).
- Coverage: before the editor first moves in, and whenever the vehicle's box is worked out again,
  every brick's root component (reflected `UBrickEditorObject::GetRootComponent`) must have the
  channel's type. If one doesn't (the component list missed it), everything is put back and the
  map refused, with a log line; the counts are logged at each show.
- Nothing else can find them: other channels' queries see "ignore", no object query asks for an
  undefined type, and the physics lets no contact through (below). In the other direction, the editor's clicks go through the map
  to its bricks (risk 3 of the report), and `ortho_view`'s visibility trace (`game/line_trace.rs`)
  is made one of the editor's (`editor_trace::as_editor`) to keep finding them.
- A networked game also needs the editor actor unreplicated (reflected `AActor::bReplicates`), or
  the map is refused. `game/net_mode.rs` says alone, hosting or joined, for the log.

The setting "Show map with colliding bricks" went with the gate: nothing collides anymore.

**Clicks lost (2026-10-15, first in-game test of the above).** Clicking a brick selected nothing
any more, shown or hidden, once the map had been shown (drag select, which works from the bricks'
geometry, still worked). The first version made colliding components query-only (`CollisionEnabled`
3 to 1, and back on hiding). Turning physics off or on rebuilds the component's physics state:
`FBodyInstance::SetCollisionEnabled` (BRMK Engine 0x1136370) updates the filter data, then calls
`UActorComponent::RecreatePhysicsState` on its component whenever the physics bit changes. A brick's
component refuses to make one outside its brick: `UBrickStaticMeshComponent::ShouldCreatePhysicsState`
(BRMK 0x1868D0) asks the brick's flag `+0xEA & 0x20` (`UBrick::IsCreatingPhysicsState`, 0xBC0D0),
which only `UBrick::CreatePhysicsState` (0x929E0) sets around its `UActorComponent::CreatePhysicsState`
call. So the first show destroyed every brick's physics state for good: no shapes, nothing for a
click's trace to hit. Reopening the editor (new components) brought clicks back.

Now `CollisionEnabled` never changes; only the object type and the responses do. Their setters
(`FBodyInstance::SetObjectType` 0x1137C10, `SetResponseToAllChannels` 0x1137DD0,
`SetResponseToChannel` 0x1137E60) only update the filter data, no rebuild. That isolates as well:
the game sets no `GSimulationFilterShader` (only the Engine module names it), so the engine's
default filter (BRMK Engine 0x12051D0, the scene's shader when that one is null, 0x11FFA2F) applies:
it reads each body's channel as `(word3 >> 21) & 0x1F` and suppresses the pair unless each one's
blocking bits (`word1`) include the other's channel. An editor brick blocks only the editor's
channel, which nothing in the world is, so it touches nothing; scene queries need the same both
ways. Pairs that already exist: `UpdatePhysicsFilterData` (0x1143540; its lambda 0x1109A10) only
rebuilds each shape's filter data and writes it (`ExecuteShapeWrite`, `SetSimulationFilter`), with
no `resetFiltering` of its own. PhysX re-filters a shape's pairs when its simulation filter data
changes (its documentation of `PxShape::setSimulationFilterData`: a change enabling a suppressed
pair wakes the actor), not checked in this build's PhysX binary. Bricks already there are isolated
before the editor moves into the map, so they have no pair with the world then. A brick added
while the map shows (placed, undone, taken from the pool) has the game's collision until the next
editor tick isolates it: if the game adds it after that tick in a frame, it may meet the world for
one physics step. That was already so before this change.

Still to confirm in game: the log's "first component collided" line (the editor bricks' real
object type and responses), the cost of the per-tick check, and, hosting or joined, that another
player sees nothing change.
