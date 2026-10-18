# Reference images and models: engine findings

Game 1.11.7 (build 25831956) unless noted; BRMK 1.11.6 for names and editor-build offsets. The
plan and decisions are in `docs/model-io.md`; this file holds what was checked in game. Code:
`crates/editortweaks/src/features/references/`, `game/textures.rs`, `game/editor_file.rs`.

## Textures from files (probe 1, 2026-10-16)

`UKismetRenderingLibrary::ImportFileAsTexture2D(WorldContextObject, Filename)` and
`ImportBufferAsTexture2D(WorldContextObject, Buffer)` are reflected and work in the shipping game
(`game/textures.rs`, called with the editor as world context). A 256x128 PNG with alpha: 0.5-1.9 ms
from a file, 0.2 ms from bytes in memory, alpha kept. Both decode on the game thread (the engine's
image wrappers): a very large image will hitch for its decode; not measured yet.

## Materials for references (probes 2 and 4, 2026-10-16)

Materials can't be made at run time, only dynamic instances of loaded ones
(`UKismetMaterialLibrary::CreateDynamicMaterialInstance`). Of the 544 loaded in the editor (City2),
the useful ones:

| Material | Kind | Parameters | Use |
|---|---|---|---|
| `/Engine/EngineMaterials/Widget3DPassThrough` | Material, blend Translucent, one-sided | texture `SlateUI`, vector `TintColorAndOpacity`, scalar `OpacityFromTexture` (names from UE 4.27's source; set without error) | base of the ones below |
| `Widget3DPassThrough_Masked`, `_Translucent` | MIC, **two-sided** | same | not used: their back side draws black |
| `Widget3DPassThrough_Masked_OneSided` | MIC (parent `_Opaque`) | same | opaque images, textured model parts |
| `Widget3DPassThrough_Translucent_OneSided` | MIC | same | see-through images (opacity < 1) |
| `Widget3DPassThrough_Opaque`, `_Opaque_OneSided` | MIC | same | (alpha ignored) |
| `/Game/BrickRigs/Materials/M_Simple_Master` | Material, opaque, lit | vector `Color`, scalars `Metallic`, `Roughness` (from its instances `M_Brick_Chrome`, `M_LauncherCap`) | untextured model parts |
| `/Game/BrickRigs/Blueprints/Props/BricksvilleSign/Materials/M_Bricksville_Sign` | MIC of `M_LegacyProp_Master`, opaque, lit | textures `BaseColor`, `Normal`, `Mask`, `AO`; colors `Color`, `RColor`, `GColor`, `BColor`; scalars `Metallic`, `Roughness` | textured model parts |
| `WorldGridMaterial` | engine default | | what a null material shows |

**Textured model parts** (2026-10-16): the widget materials are unlit, so a textured model (a
croissant glTF) received no light and no shadows. `M_LegacyProp_Master` (BRMK
`Content/BrickRigs/Materials`, read with UAssetGUI: node classes, parameter names and editor
positions; the connections don't export) feeds its `BaseColor` texture and its `Color` into a
static switch on `HasBaseColorMap`, then tints by `Mask` channels behind the `HasRMask`/`HasGMask`/
`HasBMask` switches (off by default). Static switches can't change in dynamic instances, so the
instance used is the Bricksville sign's, which has `HasBaseColorMap` on: `BaseColor` set to the
part's texture, `Normal` to `/Engine/EngineMaterials/DefaultNormal` (the sign's own normal map
otherwise), `Metallic` 0; roughness comes from the master's default roughness texture. Only City2
loads the sign, so both are loaded by path (`game/assets.rs`, brickrust's `StaticLoadObject`) when
a textured part first needs them. Texture alpha (cut-outs) is lost on these parts.

Seen in game with the prototype (a PNG on all three of `_Translucent`, `_Masked`, the base, tint
alpha 0.7): the translucent ones ~50 % see-through, the masked one opaque with the transparent
corner cut out; the two-sided instances draw their back side black; the base material only one
side.

**Selection outline** (probe 4): the bricks' outline (`game/outline.rs`, custom depth + stencil)
works on a component of ours with the **masked** material, following the cut-out (the outline
goes around the hole). Translucent materials don't draw into custom depth, so a translucent
reference needs another way: a second, masked copy of its mesh that only renders to custom
depth. Built 2026-10-17 (not seen in game yet): while a see-through reference is selected,
`scene.rs` adds one copy per piece (same mesh; the masked image material with the picture, or the
plain material) with reflected `SetRenderInMainPass(false)` and `SetCastShadow(false)`, outlined
like the pieces, removed when unselected or opaque again. (`M_Brick_Master` has
`AllowTranslucentCustomDepthWrites` set, so see-through models may outline by themselves too.)
Seen in game 2026-10-18: a selected see-through model smeared inside ("accumulating"). UE 4.26+
has a separate `UPrimitiveComponent::bRenderInDepthPass` (default true, no setter function in
4.27; the shipping exe has the property name): out of the main pass, the copies still drew into
the depth prepass, so the background failed the depth test where the dithered or translucent
pieces let it through, and those pixels kept stale colors that TAA smeared. The copies now get
that reflected bit cleared before `SetRenderInMainPass(false)`, whose render state rebuild then
reads both. (`M_EditorOutline_Master` itself only draws edges, a custom node comparing
`PPI_CustomStencil` around each pixel, and has the default blendable location, after tone
mapping: not the smear.)

**See-through models** (2026-10-17, from BRMK's assets read with UAssetGUI; not seen in game yet):
the glass bricks' base, `/Game/BrickRigs/Bricks/Materials/M_Brick_Translucent` (MIC of `M_Brick` ->
`M_Brick_Master_Inst` -> `M_Brick_Master`, a layered material with one layer `ML_Brick_Master`;
the MIC only overrides the blend mode to Translucent, lighting mode `TLM_Surface`: lit). Parameters
an instance can set: scalars `Opacity` (0.1), `EdgeOpacity` (0.8; a Fresnel blend between the two
is likely), `Metallic` (1), `MetallicAlpha` (1), `Roughness` (1), `Brightness` (0), tilings;
textures `Diffuse`, `Normal`, `MRAO`, `Emissive`, `Pattern`, projected from the world (triplanar).
Its color comes from custom primitive data, as bricks' do: `Color` slots 0-2, `RefLocation` 4,
`RefPriAxis` 7 (default (0,1,0)), `RefSecAxis` 10 ((1,0,0)), pattern offset/scale/rotation 13-17.
The `Glass` brick material asset sets `Opacity` 0.5 and its textures. References with an opacity
below 1 use an instance of it: `Opacity` = `EdgeOpacity` = the opacity, plastic (metallic 0,
roughness 0.5), the part's color in custom primitive data; the model's own textures don't show.

Seen in game 2026-10-17: every opacity below 1 looked the same, like glass. **The scalars are layer
parameters** (re-read from BRMK 1.11.6 by raw export bytes, the expression links UAssetGUI shows as
0): every parameter of `ML_Brick_Master` resolves to `{Association LayerParameter, Index 0}`, so
`SetScalarParameterValue` (global association) never reaches them and the defaults stayed
(`Opacity` 0.1 inside, `EdgeOpacity` 0.8 at the rim). The layer's opacity output is
`Lerp(Opacity, EdgeOpacity, Fresnel(exponent 4))`, plain scalars (no custom primitive data; slot 3
is unused). The MIC chain overrides only `RefractionDepthBias`; `M_Brick_Master` is one
`MaterialAttributeLayers` node, `TLM_Surface`, one-sided, `AllowTranslucentCustomDepthWrites`.
Static switches `bIsLight`/`bIsWheel`/`bIsFlap`/`bIsSpinner` are false. Fixed by setting them with
the reflected `UMaterialInstanceDynamic::SetScalarParameterValueByInfo(const FMaterialParameterInfo&,
float)` (`game/textures.rs`; the info's `Name`/`Association`/`Index` offsets and
`EMaterialParameterAssociation::LayerParameter`'s value read by reflection, index 0). The exe has
no `EdgeOpacity` string, so glass bricks keep 0.8 at their rim. No better lit translucent material
was found among BRMK's 74 translucent assets (editor ones are unlit, `M_Glass_Shards_Master` has a
constant opacity).

## Selecting references (2026-10-17)

Tried in game 2026-10-17 (first version): picks hit (the log's per-click lines showed the images
hit tens of times), but see-through images showed no outline and their rows came last in the
editor settings, so selecting them looked broken. Since then: several references can be selected
(Shift + click, `Action_AddToSelection` hooked like `Action_Select`, toggles one; Delete and
Ctrl + C act on all of them), see-through ones get the outline copies above, and the side panel
shows only the selected references' rows, as it shows a selected brick's
(`settings_list::replace`, `docs/re/settings-menu.md`).

`features/references/select.rs`. A `Select` press (through `features/editor_buttons`) in the
editor's default mode deprojects the mouse (`viewport`), takes the ray into the editor root's space
(`USceneComponent::K2_GetComponentToWorld`, reflected) and tests each shown reference's triangles
(`model_io::pick`, the same placement the components get). If one is hit before the first brick
along the ray (`line_trace`, `Hit::is_brick`), the press is taken and the release selects the
reference after unselecting the bricks (the default mode's `SelectObjects` with no objects).
References have no collision, so the game's own clicks go through them. The selected reference's
components get the selected bricks' outline (custom depth, stencil state `Selected`): it shows on
opaque ones only, translucent materials don't write custom depth. The editor's keys
`Action_DeleteSelection` (Delete) and `Action_DuplicateSelection` (Ctrl + C), reflected handlers of
`UEditorInputComponent` taking `FBrActionParams` like `Action_Select` (names in the exe's function
table, strings at 0x39904F0 / 0x3990508), are hooked next to the mouse buttons; with a reference
selected they remove or duplicate it instead.

**Hiding (2026-10-18, not seen in game yet).** The editor's `Action_HideSelection` (H, input row
`HideSelection`, BRMK `UBaseEditorInputComponent` 0x222070, it calls `UBrickEditorDefaultMode::
HideSelection` 0xB9200) and `Action_UnhideAll` (Alt + H, row `UnhideAll`, 0x225340, calling
`UnhideAll` 0xF51F0) are reflected handlers of the same kind, hooked the same way (optional: logged
"Hide not taken over" / "Unhide all not taken over" if missing). With references selected, H hides
them for the session (not in the sidecar) and the press is taken; Alt + H shows them again and goes
on to the game for its bricks. The mod's keybind Show references (default Two; One is the
orthographic view, Zero the map) and the panel's "Show references" check box hide them all.
Since the user asked for it (2026-10-18), references hidden with H keep showing with the hidden
bricks' material, `/Game/BrickRigs/BrickEditor/Materials/M_HiddenObject` (as it is, no instance),
and can't be clicked; only Show references makes them invisible.

**See-through textured models (2026-10-18).** Seen in game: the croissant (one base color texture,
base color factor white) went white below 100% opacity, since the see-through material takes only
a color. Each material's see-through color is now its color times its texture's average (linear,
alpha-weighted, `model_io::Image::average_color`). The setting "See-through textures" can show the
textures unlit instead (the translucent widget material, as see-through images use).

## Dithered see-through models (2026-10-18)

Seen in game: see-through references smeared while the camera moved, worst with only the sky
behind them. That's the engine's temporal anti-aliasing on translucent materials: they draw no
depth or motion, so their pixels are reprojected with what's behind them (the sky: as if
infinitely far). Masked materials draw both, like opaque ones. The game's dithered look (hidden
bricks) is masked, its opacity a screen-space pattern:

- `M_HiddenObject` (MIC, base overrides Masked, Unlit, clip 0.3333) of `M_HiddenObject_Master`, one
  layer `ML_HiddenObject`: layer parameters `Color` (0.108 grey, alpha 0.1), `MinOpacity`,
  `MinFadeDistance` 200, `MaxFadeDistance` 500 (a fade with the camera's distance), no custom
  primitive data. Unlit, one color: fine for hidden things, not for references.
- The dither is the function `/Game/BrickRigs/Materials/MaterialFunctions/MF_OpacityMask`:
  `ScreenAlignedPixelToPixelUVs` into `T_OpacityMask`, then a custom node `TestValue = Opacity +
  (Opacity - 0.5) * Threshold; return TestValue > Mask ? 1 : 0`. Used by `M_HiddenObject_Master`,
  `M_LightFrustum_Master`, `M_Character_Master`, `M_Shockwave_Master`, `M_Projectile_Master`
  (grep of BRMK 1.11.6's assets for the function's name).
- `/Game/BrickRigs/Character/Materials/M_Character_Master`: lit, opaque by itself (no blend mode
  set); `OpacityMask` = `MF_OpacityMask(scalar Opacity)`. Its instance `M_Character_Editor` (the
  editor's see-through character, parent `M_Character_Inst`) overrides the blend mode to Masked,
  `Opacity` 0.25. Graph (links read from the export bytes, see below): `BaseColor = Mask.R *
  Lerp6`, where the face (`Face_Color`, `Tongue_Color` from `Faces` on UV 1, alpha `Mask2.G`),
  the badge (`BadgeTexture` on UV 1, alpha `Mask2.B`) and the shirt (`Shirt_Color *
  Shirt_Texture(UV 0 * Shirt_Tiling)`, alpha `Shirt_Color.A * Mask2.R`) are blended over the
  skin/underpants/pants (`Mask` G and B). `EmissiveColor` = `Eyes_Color` times the face masks
  (`Mask2.G`). `Normal` = texture `Normal` (UV 0), `Metallic`, `Roughness` (0.4) scalars. All global
  parameters (not layered).

So references use an instance of `M_Character_Editor` with `Mask` and `Mask2` a 1 x 1 red texture
(imported at run time: the shirt everywhere, no face, no eyes), `Shirt_Texture` the part's texture
(left at its default `WhiteSquareTexture` without one), `Shirt_Color` the part's color with alpha 1,
`Normal` the flat `DefaultNormal`, metallic 0, roughness 0.6, `Opacity` the reference's (setting
"See-through models", default Dithered; Smooth keeps the glass material). Not seen in game yet.
Pieces are then outlined only while opaque: a dithered piece would draw a dotted custom depth,
so the outline-only copies do it while see-through. The masks' sampler type is `Masks` (linear)
and the imported texture is sRGB; 0 and 1 are the same either way, and shipping builds don't check
sampler types as far as known. Images stay translucent (no unlit dithered material with a texture).

Reading links: UAssetGUI 1.1.0 shows every `ExpressionInput`'s `Expression` as 0. In an editor
asset (one `.uasset`, no `.uexp`) the export's tagged properties are at `SerialOffset`; a
`StructProperty` tag (name, type, size, array index, struct name, GUID, has-GUID byte) of struct
`ExpressionInput` (or `ColorMaterialInput`, `ScalarMaterialInput`, ...) is followed by the
`FPackageIndex` of the linked expression, then its output index. Function call inputs are tagged
structs inside an array (`FunctionInputs[i].Input`).

## Blender 5 OBJ files (2026-10-18)

Blender 5.0.1 writes `usemtl ` with no name for objects without a material (and an MTL file with
only comments). tobj 4.0.5 (`parse_obj_line`) returns `MaterialParseError` for that and drops the
whole file ("material parse error" in the log for the user's car exports). `readers/obj.rs` renames
those lines to a material no MTL defines before parsing, which tobj takes as no material.

## Triangle winding (2026-10-16)

`game/mesh_builder.rs` meshes face the side **opposite** to `(b - a) × (c - a)` computed on the
raw coordinates (clockwise when X, Y, Z are read right-handed; the editor draws left-handed, so
they look counter-clockwise from the front). The collision view already did this; the prototype's
first image plane and models were built the other way round and showed inside out (backs drawn,
lighting dark). `model_io::model::unreal_facing` encodes the rule; converting a right-handed
model (glTF/OBJ/STL, counter-clockwise fronts) with a mirroring axis change keeps its triangles'
corner order.

## Which vehicle is open (probe 5, 2026-10-16)

`ABrickEditor::GetFileInfo()` is a C++ accessor, not a `UFUNCTION` (calling it by reflection
failed: "function 'GetFileInfo' not found"). BRMK 1.11.6 (`BrickRigsModKitSteam-BrickRigs.dll`):

```text
?GetFileInfo@ABrickEditor@@QEBAAEBUFUGCFileInfo@@XZ (0xAE400):  lea rax, [rcx + 0x378]; ret
__PPO__PlayerController    -> 0x348   (reflected, ObjectProperty)
FileInfo                   =  0x378   (not reflected; FUGCFileInfo is 0xE8 bytes, to 0x460)
OpenOperation              =  0x468   (game 0x390: game/brick_editor.rs)
__PPO__CurrentEditorMode   -> 0x488   (reflected, ObjectProperty)
```

The game's offsets are lower (editor-only `AActor` fields; the open operation is 0xD8 lower), the
class's own layout the same. `game/editor_file.rs` reads the struct at `PlayerController + 0x30`
only while `CurrentEditorMode - PlayerController == 0x140` (both reflected), its fields by
reflection. `FUGCFileInfo` in game (reflected, 0xE8 bytes):

```text
+0x08 FileType EnumProperty      +0x50 Title StrProperty        +0x88 TimeCreated
+0x09 UGCType EnumProperty       +0x60 Description StrProperty  +0x90 TimeUpdated
+0x0A Version ByteProperty       +0x70 NumObjects UInt16        +0x98 Tags
+0x0B ContentVersion Enum        +0x74 Dimensions (12 bytes)    +0xA0 AuthorId (40 bytes)
+0x10 LocalItemId StrProperty    +0x80 Mass Float               +0xC8 VotesUp, +0xCC VotesDown
+0x20 OnlineItemId (16 bytes)    +0x84 Price Float              +0xD0/+0xD8 TimePlayed..., +0xE0 Visibility
```

Seen in game (2026-10-16, the mod logs every change as "editor file: ..."):

| Editor showing | FileType/UGCType | LocalItemId | Title |
|---|---|---|---|
| nothing loaded yet | 0/1 | "" | "" |
| the map's cover vehicle | 5/1 | "" | "Reno, Nevada Cover \| Project Reno" |
| saved vehicle | 1/1 | "491" | "EDITOR TWEAKS 5" |
| after Save As | 1/1 | "492" | "EDITOR TWEAKS 6" |

So `LocalItemId` is the folder name under `SavedRemastered\Vehicles`. Save As keeps the same
editor actor and switches the file info to a new id and folder. That new folder held only the
game's three files, so the mod carries the references over itself
(`references::carry_over`): when the open vehicle's folder changes to one created since the last
poll, the previous references are written there. An existing vehicle opened from the menu has an
older folder and inherits nothing. Still unseen: a Workshop vehicle's id.

## Foreign files in a vehicle folder (probe 7)

Saving an existing vehicle from the editor's menu rewrites `MetaData.brm`, `Preview.png` and
`Vehicle.brv` (2026-10-16, vehicle 491). With `editortweaks.toml` in the folder, the save left it
untouched and added a `Backup.brv` (the previous `Vehicle.brv`): the game leaves foreign files
alone. Save As doesn't copy them (see above).

## Image orientation (checked in game, 2026-10-16)

The arrow test model (glTF/OBJ convention, block on its right) shows its block on the vehicle's
right, +Y. An unrotated image faces +X with its left edge on +Y, so it reads correctly from in
front of the vehicle looking back, and mirrored from behind the vehicle (its back side). That's
intended: a front view shows the vehicle's right on its left.

## Shading of models without normals

The mesh builder gives one vertex instance per vertex and lets the engine average normals over
the triangles sharing it. An OBJ without `vn` (the arrow test file) or a glTF primitive without
`NORMAL` shares corner vertices across hard edges and looked smooth-shaded and blotchy. Those
parts get their vertices split at edges sharper than 30° (`Part::split_sharp_edges`). Files with
normals already come with their hard edges split.
