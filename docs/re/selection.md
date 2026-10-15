# Selection: clicks, selecting from code, and the mouse buttons

How the vehicle editor selects bricks on a click, how the mod selects a box of them the same way
(`features/drag_select`), and how mouse buttons reach the editor, which the mod swaps
(`features/click_swap`). Found in BRMK 1.11.6, then in game 1.11.7 (build 25831956) on 2026-10-14.
The first version (boxes, lines) worked in game; the shapes, outlines and UI box below were
untested in game when written.

## The selection keys

All in the input table (`InputTable.uasset`, read with UAssetGUI), category
`IC_BrickEditor_Selection`, every one rebindable (see `input-bindings.md` for what the settings
list):

| Row | Type | Keys | Context tags |
|---|---|---|---|
| `Select` | PressReleaseAction | LeftMouseButton | Editor.Default, Editor.PickObjects, Editor.Move |
| `AddToSelection` | PressReleaseAction | Shift + LeftMouseButton | Editor.Default, Editor.PickObjects |
| `ToggleSelection` | PressAction | A | Editor.Default |
| `InvertSelection` | PressAction | I | Editor.Default |
| `SelectAttached` / `SelectAttachedRecursive` | PressAction | L / Ctrl + L | Editor.Default, ContextMenu.Editor |
| `SelectByType` / `SelectByBrick` | PressAction | V / B | same |
| `SelectByColor` / `SelectByMaterial` / `SelectByPattern` | PressAction | C / X / Y | same |

The right button is `MoveCamera` (category `IC_Camera`, a `ToggleAction` held: the mouse turns the
camera, WASD moves it), `bConsumeInput`, with the context tags `Input.Game.Pawn.Spectator`,
`Input.Game.Editor` and `Input.Game.Pawn.Player.Vehicle`. Rebinding `Select` to the right button
leaves `MoveCamera` on it too, which takes it (seen in game by the user). Moving `MoveCamera` to the
left button would move the free camera and vehicles' cameras with it as well, where the left
button is `Fire` (`Input.Game.Pawn.Player`). Hence the mod's swap below instead of rebinding.

## Clicks

`Action_Select` / `Action_AddToSelection` (`UBaseEditorInputComponent`, BRMK 0x223B50 / 0x2200B0;
game thunk 0xFBBEB0 -> 0xDCF2B0) call `HandleSelect(Params, bAdd)` (BRMK 0x23EF50, game 0xDE3900):
on a press, the current mode's `PressObjectAtScreenPosition(Mouse, bAdd)` (vtable game +0x2E0, BRMK
+0x3A0), on a release `ReleaseObjectAtScreenPosition` (+0x2E8, BRMK +0x3A8). The default mode keeps
`UBrickEditorMode`'s: the press marks the hovered object pressed (editor flags BRMK +0x4F0, flag 0x2,
`UpdateHoveredObjectPressed`); the release (BRMK 0xD57E0, game 0xCFF240, signature
`editor_mode::SELECT_OBJECTS_CALL`):

```text
if (!(Editor->Flags & 2)) return;            // no press in the view
clear the flag; UpdateHoveredObjectPressed();
Object = Editor->GetObjectAtScreenPosition(Mouse);
if (Object != Editor->HoveredObject) return;  // the mouse left the pressed object
TArray<UBrickEditorObject*> Objects; if (Object) Objects.Add(Object);
this->SelectObjects(Objects, bAdd, true);     // vtable game +0x2F0, BRMK +0x3B0
```

A click on no brick selects nothing (an empty array): it unselects everything. Mode vtables in the
game, found through the pointers to that release at +0x2E8: 0x38D7DE0 (`UBrickEditorMode`, its
`SelectObjects` an empty stub), 0x38D95C0 (the default mode, `SelectObjects` 0xD05220), 0x3962AA0,
0x3962DF8, 0x3963120 (object pickers, 0xD05500).

`UBrickEditorDefaultMode::SelectObjects(const TArray<UBrickEditorObject*>&, bool bAdd, bool
bClicked)` (BRMK 0xE4DE0, game 0xD05220) calls `ABrickEditor::SelectObjects(Objects,
ESelectObjectsMode, bool, bool, bool)` (BRMK 0xE41B0, game 0xD04A90, where `SELECTION_UPDATE`
matches):

- no objects: unselect all (mode 4 with an empty array), unless adding (then nothing);
- adding: mode 2 if the first object is the primary selected one (BRMK editor +0x4A4, a weak
  pointer), else 1: so 1 adds and 2 removes (shift-clicking the primary brick unselects it), for
  every object of the array;
- else, with `bClicked` and a user setting (+0x2EA in the game), a click on the only selected,
  primary brick selects its group (editor or weld group) instead; otherwise mode 4 replaces the
  selection.

`ESelectObjectsMode` isn't reflected. Drag select calls the mode's virtual with `bClicked` false,
replacing, or adding with the already selected bricks left out (so the first is never the primary).

## What a click hits

`ABrickEditor::GetObjectAtScreenPosition(FVector2D)` (BRMK 0xB2240): `DeprojectScreenPositionToWorld`,
then `UWorld::LineTraceSingleByChannel` out to the static info's reach, with
`GetEditorTraceParams` (BRMK 0xACDC0: channel 3, `ECC_Visibility`; the hidden objects' components
ignored) and `bTraceComplex` set (BRMK's `FCollisionQueryParams` +0x18, after two 12-byte editor
`FName`s); the hit component's `IBrickEditorComponentInterface` gives its editor object (a weak
pointer at +8 in the interface). So a click hits bricks' complex collision: the triangle meshes
`game/brick_model.rs` reads (see `brick-models.md`), the render mesh's collision LOD.

## The selection outline

Selected and hovered bricks are outlined by a post-process material (`M_EditorOutline`, its
instance `ABrickEditor::OutlineMID`, reflected) from custom depth.
`UBrickEditorObject::UpdateEditorVisualization` (BRMK 0xF88C0, virtual) calls
`UpdateDesiredCustomDepth` (BRMK 0xF71B0, game 0xD0F780), which works out in `EditorParams`
(object +0x48 in BRMK, +0x40 in the game) a stencil value (+0x14, game +0xC) and whether to render
custom depth (flags +0x16 bit 0, game +0xE), then calls `SetRenderCustomDepth` and
`SetCustomDepthStencilValue` on the object's root component (a weak pointer at BRMK +0x80):

```text
State = GetDesiredOutlineState()        // inlined in the game
Color = (Object->EditorGroup - 1) % 15  // BRMK object +0x40, game +0x38
Stencil = Hovered (flags bit 2) | (State + 1) << 1 | Color << 4
Render = !(flags & 0x18 hidden) && (pairs with the hovered or primary brick's group || hovered
         || State != Unselected || view mode 1)
```

`EBrickOutlineState` (BRMK `GetDesiredOutlineState` 0xABFC0, not reflected; names after the
material's parameters): from the validity byte (`EditorParams` +0x13, game +0xB: 1 -> 2 `Valid`,
2 -> 1 `Neutral`, 3 -> 0 `Invalid`, for property pickers), else the selection byte (+0x12, game
+0xA: 1 -> 4 `Selected`, 2 -> 3 `Active`, the primary one), else 5 `Unselected`. Same mapping and
encoding in the game (checked 2026-10-14).

The material's parameters: colors `ActiveColor`, `SelectedColor`, `UnselectedColor`,
`HoveredColor`, `ValidColor`, `InvalidColor`, `NeutralColor`, thicknesses `SelectedThickness` 1,
`UnselectedThickness` 1, `ActiveThickness` 2, `HoveredThickness` 3, and `bIsHoveredObjectPressed`,
`PressedBrightness` 0.75. `ABrickEditor::UpdateOutlineColorParam(FString, EBrickUIColorStyle)`
(BRMK 0xFB8F0, inlined around 0xFCDE0) sets the colors from the current UI style (BRMK 0x31B500:
`UBrickUIStyle::CurrentStyle` or its class default), converted to sRGB (`ToFColor(true)` / 255:
the post-process draws after tone mapping): `ValidColor` = style +0x2E78, `InvalidColor` +0x2E88,
`NeutralColor` +0x2E98, `SelectedColor` +0x2E68, `UnselectedColor` +0x2E38: `ColorStyles[Positive,
Negative, Neutral, Highlight, Default]` (the array at +0x2E38 in BRMK). So selected bricks are
outlined in the style's `Highlight` color (orange in the default style).

## Drag select

`features/drag_select`: the press of its key, when it's the left or right button, is taken from
`Action_Select` / `Action_MoveCamera` (`features/editor_buttons`, shared with the resize tool), so
the camera mode doesn't start; the mouse's movement is taken (`features/mouse_moves`), since
without the camera mode it still turns the camera (`Axis_ViewYaw`, see `editor-input.md`). The
bricks' boxes come from `GetBrickEditorObjectLocalBounds` (scaled, around the object's location) and
`GetWorldTransform` (not scaled), the box on the screen from `DeprojectScreenPositionToWorld` at its
four corners (the origins lie on the near plane). The view through the box is a pyramid; a brick is
picked when it touches it, lies inside, or has its center inside (a setting):

- By its shape, what a click hits: its mesh's complex collision triangles (`game/brick_model.rs`),
  scaled to the brick's box like the resize tool's models (`brick_model::scale_to_fit`: the box
  must be the mesh's box scaled around the origin, else the brick keeps its box). Its box comes
  first: missed, or taken in whole, and the triangles aren't needed. A triangle touches the view
  if anything is left of it once cut by the pyramid's sides and near plane; a brick is inside if
  all its mesh's points are.
- Spinners by their own collision (2026-10-14): their mesh is a generic one bent by the material,
  without a body setup (the first version's log: "picked by its box: no body setup"). A click
  hits `SpinnerBodySetup` instead: `FindOrCreateSpinnerBodySetup`'s builder (BRMK 0x478B90) fills
  `ConvexElems` only, one piece per arc step (the cross-section at two angles), at the spinner's
  size, and creates it (BRMK 0x478210, game 0xF4EC80) with `CollisionTraceFlag` (BRMK +0x3D, game
  +0x31) = 2, `CTF_UseSimpleAsComplex`, and `bGenerateMirroredCollision` cleared. The mod reads
  the pieces (`game::collision`), makes their faces (`math::hull`) and places them unscaled at the
  brick; its box (`CalcSpinnerMeshBounds`, worked out from the arc) only checks them loosely.
- Bricks without a readable mesh by their box (separating axis test: the pyramid's sides and near
  plane, the box's axes, and the crossings of their edges).

While dragging, bricks show as letting go would leave them, through the outline above: picked
bricks that aren't selected get the `Selected` state (`Stencil & !0b1110 | 5 << 1`, rendered), and
unless adding, selected bricks the box misses get `Unselected` (rendered only if hovered). Through
the reflected `bRenderCustomDepth`, `CustomDepthStencilValue` and their setters on the root
component (`GetRootComponent`, reflected), remembering the game's look; when the game changes a
brick's look meanwhile, that's its look. Before letting go selects, every brick gets the game's
look back (if unchanged since), and the selection update then outlines what changed.

### The box, with the game's UI

`WBP_BrickEditor` (BRMK content, `UBrickEditorWidget`'s Blueprint) has a canvas panel over the 3D
view, `ViewportCanvasPanel` (a Blueprint variable, so a reflected property of `WBP_BrickEditor_C`;
in a vertical box between the top dashboard and `ViewportBottomBox`, so its corner isn't the
viewport's). The editor widget keeps its editor in an unreflected weak pointer (BRMK +0x404,
`GetBrickEditor` 0x31C900); the mod finds the live one among all objects: a `UBrickEditorWidget`,
not a template, whose `GetOwningPlayer()` is the editor's controller and which `IsVisible()`.

`game/view_canvas.rs` adds plain `UBorder`s to it (made inside the panel's outer, its widget tree,
as `ConstructWidget` does; their default brush draws a solid color): `UCanvasPanel::
AddChildToCanvas`, `UCanvasPanelSlot::SetPosition` / `SetSize` / `SetZOrder`,
`UBorder::SetBrushColor(InBrushColor)`, `UWidget::SetVisibility` (`HitTestInvisible`, so they never
take the mouse, or `Collapsed`). Viewport pixels to the panel's units: `UWidget::GetCachedGeometry`,
then `USlateBlueprintLibrary::LocalToViewport` of two points. The box: one border, see-through
(alpha 0.15), and four edges, in the UI style's `Highlight` color (the selected bricks' outline,
read from `UBrickUserSettings::UIStyle`'s default object, `ColorStyles`, see `game/ui_style.rs`).
Without the editor's widget, the box is drawn as lines in the 3D view.

The UI style's own brushes (`UBrickUIStyle::BrushStyles` by `EBrickUIBrushStyle` Border,
FilledBorder, Button, MapButton, Background, Title, Slider, SliderReadOnly, Spacer, each with a
brush per `EBrickUIBrushState` Default, Focused, Pressed, Selected, FocusedSelected,
PressedSelected, Marquee) are drawn by `UBrickBorder` (reflected `SetBrushStyle`, `SetBrushState`,
`SetColorStyle`, `SetPaddingStyle`). `Border` is `M_Border` (`M_Brush_Master`, a border texture
with its inside at `BackgroundBrightness` 0.25), its `Marquee` state `M_Border_Marquee` (the same
with the static switch `bIsMarquee`: a dashed border). Not used: whether the inside is see-through
wasn't checked.

## Mouse buttons: `UPlayerInput::InputKey`

Every key the player's controller gets from the game's viewport (not what UMG/Slate widgets take)
goes through `APlayerController::InputKey`, then `virtual bool UPlayerInput::InputKey(FKey Key,
EInputEvent Event, float AmountDepressed, bool bGamepad)` (engine; BRMK engine module 0x1758A30,
game 0x29C9DB0, signature `player_input::INPUT_KEY` on its switch over the event), which records it
in `KeyStateMap` for the bindings to read each frame (`ProcessInputStack`). `UBrickPlayerInput`
doesn't override it. Its vtable slot, for the record:

- BRMK `??_7UBrickPlayerInput@@6B@` (0x7DF5D0, entries resolved through the import thunks): after
  `UObject`'s 102 virtual functions, `UPlayerInput`'s own: [102] `ForceRebuildingKeyMaps`
  (`UBrickPlayerInput`'s), [103] `InputKey`, [104] `ProcessInputStack` (`UBrickPlayerInput`'s),
  `SmoothMouse`, `DisplayDebug`, `MassageVectorAxisInput`, `MassageAxisInput`,
  `ConditionalBuildKeyMappings_Internal`, [110] `IsKeyHandledByAction` (`UBrickPlayerInput`'s).
- Game: the class's registration (0xF995E0, loading `"BrickPlayerInput"`) passes its vtable helper
  constructor (0xF9A520), which stores the vtable 0x3973F10: `UObject`'s 78, then +0x270 0xE60750,
  +0x278 `InputKey` 0x29C9DB0, +0x280 0xE817B0, ..., +0x2B0 0xE75530 (next to
  `IsInputActionKeyPressed`, 0xE75440).

`FKey` (0x18 bytes): `KeyName` (`FName`), then `TSharedPtr<FKeyDetails>` (the details, their
reference controller: vtable, shared count +8, weak count +0xC; not thread safe). Passed by value:
the caller passes a pointer to its copy and the callee destroys it (Microsoft's convention).
`EInputEvent`: 0 pressed, 1 released, 2 repeat, 3 double click (as the game's switch reads it).

`features/click_swap` hooks it: with the setting on, a left or right button pressed while the
player's controller looks through the editor comes in as the other button (the key's name
changed, its details let go: one reference released, the pointer nulled; the engine looks them up
again from the name), and its release the same way as its press. While editing, the view target is
the editor: `ABrickEditor::BecomeViewTarget` (BRMK 0x8A130) overrides `AActor`'s, read through the
reflected `APlayerController::PlayerCameraManager` -> `APlayerCameraManager::ViewTarget`
(`FTViewTarget`) -> `Target`. The mod's own keys read held buttons from Windows
(`GetAsyncKeyState`), so `keybinds` swaps the names it reads too.
