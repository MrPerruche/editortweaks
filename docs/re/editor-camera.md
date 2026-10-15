# Editor camera: its view, zoom, gizmo size and the sky

How the vehicle editor makes its view each frame, so that `features/ortho_view` can make it
orthographic. Found through BRMK 1.11.6's exports (`BrickRigsModKitSteam-BrickRigs.dll`), then in
game 1.11.7 (build 25831956). Bindings: `game/camera_manager.rs`, `game/editor_camera.rs`,
`game/world_sky.rs`, `game/line_trace.rs`, `game/scene_component.rs`,
`game/zoom_notification.rs`, zoom steps in `game/editor_input.rs`, the zoom ratio in
`game/view_target.rs`.

## `ABrickEditor::CalcCamera`

`void ABrickEditor::CalcCamera(float DeltaTime, FMinimalViewInfo& OutResult)` (the `AActor`
virtual; BRMK 0x8C1E0, game 0xCD0AB0). The camera manager (`APlayerCameraManager::UpdateViewTarget`,
engine) resets the view target's `FMinimalViewInfo` first (`FOV` = the manager's, the projection
perspective unless its `bIsOrthographic`), then calls it:

```text
if (virtual +0x728 BRMK / +0x5D0 game) { AActor::CalcCamera(DeltaTime, OutResult); return; }
StaticInfo = GetStaticInfo();
ViewTarget->GetCameraViewPoint(OutResult.Location, OutResult.Rotation);   // interface vtable +0x58
ViewTarget->UpdateZoom(DeltaTime);          // BRMK 0x371A30, game 0xEAB5C0
OutResult.FOV *= ViewTarget->GetZoomFOVFactor();                          // BRMK 0x32D3E0, game 0xE717C0
... a post-process blendable (BRMK +0x4F8) added to OutResult.PostProcessSettings ...
if (Gizmo /* GizmoMeshComponent */ && Gizmo->virtual +0x550 BRMK /* +0x468 game; visibility? */ && CameraManager) {
    FVector2D FOV = CameraManager->GetEffectiveFOV(OutResult.FOV);       // BRMK 0x2AD270, game 0xE22B10
    Gizmo->SetRelativeScale3D(Distance(Gizmo, OutResult.Location) * tanf(FOV.Y / 2) * StaticInfo->GizmoFactor);
}
```

It never touches `ProjectionMode`, `OrthoWidth` or the ortho clip planes, and nothing after it does
(`ABrickCameraManager::UpdateViewTargetInternal` adds the focal distance; `DoUpdateCamera` only
fades). So the view can be made orthographic right after it, in `FMinimalViewInfo`'s reflected
fields (`ProjectionMode`, `ECameraProjectionMode::Orthographic`; `OrthoWidth`; `OrthoNearClipPlane`,
`OrthoFarClipPlane`; `Location`, `Rotation`, `FOV`). `ortho_view` does it in
`ABrickCameraManager::ApplyCameraModifiers` (BRMK 0x2977F0, game 0xE142C0; signature
`camera_manager::APPLY_CAMERA_MODIFIERS`, two post-process override bits set around a lerp): the
engine's `UpdateViewTarget` calls it right after the view target's `CalcCamera`, every frame and
for every view target (vehicles, characters and the free camera override `CalcCamera`, none is a
camera actor), so it also sees when the editor isn't viewed. Its `FMinimalViewInfo&` is the view
target's `POV` in `APlayerCameraManager::ViewTarget` (`FTViewTarget`, reflected: `Target`, `POV`),
which tells the main view from a blended one. (The first version hooked `CalcCamera` itself.)

Signature `editor_camera::CALC_CAMERA`: the gizmo's scale computation (`tanf`, the three `movss`,
`SetRelativeScale3D`), which also holds `GizmoFactor`'s offset in the static info as a disp8
(`mulss xmm6, [rbp+0x60]` in the game, `[r12+0x68]` in BRMK). `GizmoFactor` is not reflected: the
static info's reflected fields go `GizmoMesh` (BRMK +0x60), then `OutlineMaterial` (+0x78).

`GizmoMeshComponent` (BRMK +0x528), `PlayerController` (+0x348) and `StaticInfoClass` (+0x5B0, a
`ClassProperty`: `GetStaticInfo` returns its default object) are reflected on `ABrickEditor`.

## Effective FOV

`ABrickCameraManager::GetEffectiveFOV(float)` (BRMK 0x2AD270, game 0xE22B10) asks the local player
for its projection data (`ULocalPlayer::GetProjectionData`, vtable +0x388 BRMK / +0x2C8 game) and
returns `2 * atan(1 / P[0][0])` and `2 * atan(1 / P[1][1])` in degrees: from the projection
matrix, which is built from the camera cache, so the last frame's view. For an orthographic view
`1 / P[1][1]` is half the view's height in cm, so the game's gizmo scale becomes
`Distance * HalfHeight * GizmoFactor` there. So on every frame whose view it changes (and the game
sized the gizmo for the last frame's), `ortho_view` sizes the gizmo itself: `GizmoFactor *
HalfHeight` for an orthographic view (the same share of the screen as in perspective),
`GizmoFactor * Distance * tan(FOV / 2) * aspect` for a perspective one (the gizmo's place:
`K2_GetComponentLocation`; the aspect: `APlayerController::GetViewportSize`, an orthographic
view's height following its width).

Other readers of the effective FOV: `ABrickEditor::FocusCamera` (BRMK 0xA10F0, through
`GetCachedEffectiveFOV`: how far to put the camera from the focused bricks),
`UBrickGameInstance::CaptureUGCThumbnail` (BRMK 0x22B1B0: a thumbnail from the player's view uses
its FOV), `UCrosshairWidget::NativeTick`, and the free camera's `CalcCamera`. Turning the camera
scales with the FOV only for the free camera (`ABrickPlayerController::ShouldScaleRotationInputToFOV`
checks for an `ABrickSpectatorPawn`).

## Zoom

`IViewTargetInterface` (the editor's at game +0x220, BRMK +0x2F8) has `GetZoomParams` (vtable
+0xA0) and `GetZoomCache` (+0xA8):

- `FViewTargetZoomParams` (reflected, 0x18; the editor's are `UBrickEditorStaticInfo::ZoomParams`,
  BRMK +0xAC) extends `FViewTargetSharedParams`: +0x0 (the axis speed), `Step` (+0x4, by how much a
  zoom step changes the ratio), `Exponent` (+0x8), `InterpSpeed` (+0xC); then `ZoomRange` (+0x10,
  an `FFloatInterval`: Min, Max).
- `FViewTargetZoomCache` (not reflected): +0x0 the held zoom axis input, +0x4 the target ratio,
  +0x8 the current ratio (0 to 1), which `UpdateZoom` moves towards the target with `FInterpTo`
  at `InterpSpeed`.

`GetZoomFOVFactor` (BRMK 0x32D3E0 -> 0x32D350, game 0xE717C0 -> 0xE71740) returns
`Lerp(ZoomRange.Max, ZoomRange.Min, powf(CurrentRatio, Exponent))`: ratio 0 is unzoomed. The zoom
step actions (`UBaseInputComponent::Action_ZoomInStep`/`OutStep`, BRMK 0x225520/0x2255D0) call the
view target's zoom step (BRMK 0x372190) with +1/-1. The settings' `EditorZoomRatio`
(`UBrickUserSettings`) is only written when the editor ends (`ABrickEditor::EndPlay`), to start the
next one at the same zoom.

The zoom ratio stops at 0 and 1, but an orthographic view can only zoom out by zooming, so
`ortho_view` takes the zoom steps while orthographic: the editor input's reflected
`Action_ZoomInStep`/`Action_ZoomOutStep` (on `UBaseInputComponent`, taking `FBrActionParams`,
returning `FBrActionResult`), their exec thunks hooked like `Action_Select`'s (see
`editor-input.md`), real inputs only (`Params.Mode`). Its own width glides at the zoom params'
`InterpSpeed`, like the game's ratio.

The current ratio is read through `GetZoomCache` (slot 21, +0xA8; the game's `UpdateZoom`, 0xEAB5C0,
calls +0xA0 and +0xA8 like BRMK's), for the notification shown when switching back (below).

## The zoom notification

`UpdateZoom` stores the new current ratio when it changed, calls the interface's virtual +0xB0,
then broadcasts the native multicast delegate at interface +0x38 with it (BRMK 0x371B2F). The HUD
overlay binds to it (BRMK 0x369EB0, when the view target changes: `OnCameraSpeedRatioChanged` to
the delegate at +0x20, `OnCameraZoomRatioChanged` to +0x38):

- `UGameOverlayWidget::OnCameraZoomRatioChanged(float)` (BRMK 0x338F00, game 0xE7BE70; signature
  `zoom_notification::ZOOM_CHANGED`, the whole function, checked at load to load the characters
  "zoom"): `CreateHUDNotification(FName("zoom"), true)` (BRMK 0x313D50, game 0xE5E1E0: the
  notification with that context if it shows, else one from the widget pool, unless
  `ShouldShowHUDNotifications` says no; then `StartNotification(bAutoEnd)`, which with true ends it
  after a while), then `UpdateCameraZoomNotification(Notification, Ratio)` (BRMK 0x53F840, game
  0xFC5920: the Blueprint event, through `ProcessEvent`).
- `WBP_GameOverlay`'s event: `Name = UIFunctionLibrary.NumberToText(NewZoomRatio, 2 (Percent), 0,
  false, false, false)` (0 to 100, no unit), the icon `T_Zoom`, then
  `Widget.InitializeNotification(DisplayInfo, 0)`. The camera speed's is the same with `T_Speed`.
- `WBP_HUDNotification`'s `InitializeNotification` calls the native one
  (`InitializeNotification_Implementation`, BRMK 0x3CDCB0: `IconImage` (+0x420) gets the icon,
  `TextBlock` (+0x418, reflected) `SetText(DisplayInfo.Name)`), then colours its border.
- The overlay, as `UGameOverlayWidget::Get` finds it (BRMK 0x31B660): the game instance's
  `WindowManagerWidget` (`UBrickGameInstance`, reflected, BRMK +0x3B0), its `ActiveWidget`
  (reflected, +0x420) if it's a `UHUDContainerWidget`, and that one's `GameOverlayWidget`
  (reflected, +0x420).

`ortho_view` hooks `OnCameraZoomRatioChanged` and does what it does, then sets the text block to
its label ("Perspective 20" from the same `NumberToText`) while the editor is viewed; the
orthographic zoom steps and the switches show it the same way (`CreateHUDNotification` and the
event's stub taken from the hooked function's calls), labelled "Orthographic" and the view's width
(`NumberToText(Width, DistanceAuto, ..., bIncludeUnits)`: m or ft as the player chose). Texts:
`Conv_StringToText` and `Conv_TextToString` (`UKismetTextLibrary`), `UTextBlock::SetText`, all
reflected.

## Picking

`ABrickEditor::GetObjectAtScreenPosition` (BRMK 0xB2240) traces from `DeprojectScreenPositionToWorld`'s
origin out to `|2 * EditorBounds|` (the editor box's diagonal). For an orthographic view the origins
lie on its near plane, `OrthoNearClipPlane` from the camera (behind it when negative): `ortho_view`'s
is the camera itself, so clicks pick what the view shows. Drag select's box (`features/drag_select/frustum.rs`)
takes each corner's ray origin and direction, so a box (parallel rays) works like a pyramid.

## Moving the camera

`ABrickEditor::SetCameraRelativeLocation(const FVector&)` (BRMK 0xE6680, game 0xD06630; signature
`editor_camera::SET_CAMERA_LOCATION`) clamps the location to the editor's bounds
(`ConstraintLocationToEditorBounds`, 100 cm padding), stores it (game +0x3F0, BRMK +0x4C8: the
pattern holds the stores' offsets), then fades the bounds' box by the camera's distance to it.
`GetCameraViewPoint` (BRMK 0xA9FE0) gives the editor's root component location plus it (no
rotation), and the camera's rotation. `ortho_view` moves the camera along its view when switching
back to perspective, so that what's in the middle keeps its size, and around what's in the middle
while the orthographic view turns (below).

## What's in the view

`ortho_view` finds how far the vehicle is with `UKismetSystemLibrary::LineTraceSingle` (reflected;
visibility channel, complex, like the editor's clicks), reading `FHitResult::Distance` and
`FHitResult::Component` (a `TWeakObjectPtr`, `WeakObjectProperty`).

- Nothing may be left out: the bricks the editor shows are components of the editor actor itself
  (`GetViewMode` checks that a brick's owner is a `BrickEditor`, `editor-view-modes.md`). The first
  version traced with `bIgnoreSelf` and the editor as the world context, so it went through the
  whole vehicle and hit the ground or the map hundreds of metres away, or nothing (game log
  2026-10-14: switches "as 23153 cm away", "as 140442 cm away", or the 1000 cm fallback; in game
  the view leapt kilometres back, and switching back threw the camera forward by as much).
- A hit is a brick when its component implements `IBrickEditorComponentInterface` (the interface
  the editor's clicks find the object through; `reflection::interface_offset`). BRMK's implementers:
  `UBrickStaticMeshComponent`, `UBrickEditorStaticMeshComponent`, `UBrickEditorISMComponent`,
  `UBrickEditorSkeletalMeshComponent`, `UBrickEditorArrowComponent`,
  `UBrickEditorParticleComponent`, `UBrickEditorNiagaraComponent`.
- Into the orthographic view: rays from the camera through a 9 x 5 grid of the screen, nearest the
  middle first (`features/ortho_view/probe.rs`); the first brick sets the depth. With no brick in
  view, what's in the middle if nearer than 10 m, else 10 m.
- While orthographic, and back out of it: one ray along the view from the camera (what the middle
  of the view shows), counted only if it hits a brick.

## Turning the camera

The editor turns its camera in place (`MoveCamera`, right button) or around a pivot (`PivotCamera`,
middle button or Space; the input component's camera flags bit 1, see `editor-input.md`).
`UEditorInputComponent::UpdateCameraMode` passes both to `ABrickEditor::UpdateCameraMode(bool,
bool)` (BRMK 0xF6840), which keeps them in the editor's flags (BRMK +0x4F0: bit 2 either, bit 3
pivoting). `ABrickEditor::UpdateViewRotation(FRotator&, float)` (BRMK 0xFD400) then, while
pivoting, turns the camera's place around the middle of the selected objects' bounds
(`UBrickEditorObject::GetBrickEditorObjectSpawnRelativeBounds`, BRMK 0xA8270, over the array of the
editor's `UBrickEditorInterfaceComponent` at +0x228), or the editor's origin with none, by the
rotation's change.

Turning in place suits a perspective view. An orthographic one shows no depth, so turning in place
swings what's in front of the camera across the screen by its distance, and what's behind it the
other way: it looks like turning around some random point (in game 2026-10-14: "super
counterintuitive, feels extremely unpredictable"). So while orthographic and not pivoting,
`ortho_view` moves the camera each frame it turns (`SetCameraRelativeLocation`) by
`(LastForward - Forward) * Pivot`: the point `Pivot` cm along the view stays in the middle of the
screen. `Pivot` is the brick in the middle when a turn starts after 250 ms without turning, else the
last one, kept in place as the camera flies along the view (once flown past, the camera turns in
place: the view doesn't show what's behind it).

## Panning

While the camera is moved or pivoted (the input component's camera flags bit 2, see
`editor-input.md`), the mouse's movement reaches `Axis_MouseMoveRight`/`Up`, which turn it. With
`keybinds::PAN_VIEW` held too (Shift + right button: the game's `MoveCamera` ignores modifier keys,
so the camera mode starts as usual), `ortho_view` takes that movement (`features/mouse_moves`) and
at the next frame moves the camera across the view (`SetCameraRelativeLocation`) by as many cm as
the pixels moved show: the orthographic view's width over the screen's (`GetViewportSize`), or in
perspective the view's width at the vehicle's depth (found as in a switch, once per pan). The
movement is the engine's raw mouse counts (`AXIS_PER_PIXEL`), as many as pixels at Windows' default
pointer speed: the cursor is hidden and held still while the camera is moved.

## Leaving the editor

The camera manager blends from one view target to the next. While it does, the target being blended
to isn't `ViewTarget` (the engine's `PendingViewTarget`), so `ortho_view` sees "not the editor" for
it, and for the game's view once the editor is left; it puts the sky back then. The first versions
stayed orthographic, and coming back the sky wasn't looked for again (only after a switch back to
perspective), so the view showed no sky and kept old frames (in game 2026-10-14: "half stuck between
normal map and editor thing, smearing"). Now any frame not of the editor switches back to
perspective at once (the camera stays where it is).

## The view's depth

The first versions showed from 100 m behind the camera to 10 km in front, so the camera's place
along the view didn't matter: it looked like "camera super far + 1° FOV", nothing could be seen
from inside the vehicle, and the depth steps (32-bit float depth, linear for an orthographic view,
reversed: about the depth range times 2^-24 near the camera, so 0.6 mm) made faces flicker at half a
millimetre (in game 2026-10-14). Now the near plane is the camera, as in perspective, so flying into
the vehicle cuts through it, and the far plane follows the width: 4000 times it (depth steps under
half a pixel at 1920 pixels across), at least twice as far as what the view turns around, at most
10 km. At the narrowest (10 cm) that's 400 m deep, steps of 0.02 mm.

## The sky

`AWorldSetupActor` (reflected: `SkyMeshComponent` BRMK +0x550, `SkyMesh` +0x5A0, `SkyMeshScale`
+0x5A8, `SkyMaterial`, `EditorSkyMaterial` +0x5C0) draws the sky as a sphere mesh:

- `PreRegisterAllComponents` (BRMK 0x49EE50) sets the sky component's `BoundsScale`
  (`UPrimitiveComponent` BRMK +0x2C4, reflected) to 10 000 000 (never culled), clears two
  rendering flags, sets its mesh (`SM_SkySphere`) and its scale to `SkyMeshScale` (500.0, the C++
  constructor's: the Blueprint `BP_WorldSetupActor` leaves it), and its material (the editor's when
  the local player is in the editor, `SetIsLocalPlayerInEditor`).
- `PostRegisterAllComponents` binds `OnBeginPostProcessSettings(FVector ViewLocation, FSceneView*)`
  (BRMK 0x4975E0) to the world's `OnBeginPostProcessSettings`: before each view is drawn, it moves
  the sky (`SetWorldLocation`) and the clouds to the view's place, among other weather updates.
- The materials (`M_Sky_Master`, `M_Sky_Editor`) are opaque, unlit, `bIsSky`: in 4.27 that only
  skips aerial perspective and clamps the color (`BasePassPixelShader.usf`); the sphere's depth is
  its real one.

A perspective view's far plane is infinite, so the sphere always shows. An orthographic view's
isn't: with a far plane short of the sphere nothing draws behind the vehicle, and the screen keeps
old frames there (seen in game 2026-10-14: "stuff rendered is kinda permanent"). The mesh reaches
500 m (its bounds' sphere radius, game log 2026-10-14), so 250 km scaled; a depth range that long
would make nearby faces flicker (32-bit depth, linear for an orthographic view: steps of the range
times 2^-24 near the camera). So `ortho_view` scales the sky component down to 90 % of the view's
depth while the view is orthographic (`UStaticMesh::GetBounds`, reflected, for the mesh's size;
`SetRelativeScale3D`, again whenever the depth changes), and back as soon as it isn't, or the
editor isn't viewed. Confirmed in game 2026-10-14 (with a 10 km far plane): everything renders.
The sky follows the view's place (`OnBeginPostProcessSettings` gets the view's location), so it
stays around the camera however shallow the view.
