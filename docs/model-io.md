# Images, 3D models and vehicle export

Plan and research for three features (the user's goal, 2026-10-16), none built yet:

1. **Reference images**: load PNG/JPG files and show them in the editor (blueprints to trace over).
2. **Reference models**: load 3D models (OBJ, glTF/GLB, STL, ...) and show them in the editor as
   non-brick previews.
3. **Vehicle export**: write the vehicle being edited as a 3D model file (GLB first).

Status: researched from the code and from UE 4.27 knowledge. **Nothing here was tried in game.**
Claims marked *unverified* are from memory of the engine source, to be checked (first by
reflection, see "Probes to run first") before building on them. Format and engine findings that
are confirmed belong in `docs/re/` once found; this file is the plan.

Decisions (user, 2026-10-16):
- **FBX is dropped.** It's a closed format that needs a heavy parser for little gain; people can
  convert to glTF (Blender). If ever reconsidered: `ufbx` (C, public domain/MIT) reads it, and
  builds with MinGW through the `cc` crate.
- Open source libraries are fine as long as their licenses are compatible with this project's
  **GPL-3.0**: MIT, BSD, Apache-2.0 (fine for GPLv3), zlib, public domain, MPL, LGPL. Never the
  proprietary Autodesk FBX SDK. Pure Rust crates preferred (the build is the GNU toolchain with
  project-local MinGW; C libraries also build through `cc`, at the cost of build weight).
- Everything stays client side (`CLAUDE.md`): imported files only change what *this* player sees in
  the editor; the vehicle file and anything a vanilla player loads are never touched. Export only
  reads.

## Formats and libraries

Licenses are from memory: check each crate's `license` in `cargo metadata` when adding it, and turn
off default features that pull in more than needed.

| Purpose | Format | Library | License (verify) | Notes |
|---|---|---|---|---|
| Image in | PNG | `png` (or `image` with only `png`) | MIT/Apache-2.0 | pure Rust |
| Image in | JPG | `zune-jpeg` or `jpeg-decoder` (or `image` with `jpeg`) | MIT/Apache-2.0 (zune: MIT/Apache/Zlib) | pure Rust |
| Model in | OBJ (+ MTL) | `tobj` | MIT | triangulates; materials from the MTL |
| Model in | glTF / GLB | `gltf` | MIT/Apache-2.0 | embedded textures; the recommended format |
| Model in | STL | `stl_io` (or by hand, ~60 lines) | MIT | no colors, no UVs |
| Model in | PLY | `ply-rs` | MIT | optional, vertex colors |
| Model out | GLB | `serde_json` + a hand-written writer | MIT/Apache-2.0 | ~200 lines; nodes with TRS |
| Model out | OBJ (+ MTL) | by hand | n/a | ~100 lines, the fallback everything opens |
| Model out | STL | by hand | n/a | one mesh, for 3D printing; optional |
| Later | simplify big meshes | `meshopt` | MIT | only if big imports need it |

Not planned: FBX, skeletons/animation, DAE, 3DS. `assimp` (BSD-3, C++, ~40 formats) is the heavy
hammer if the long tail is ever wanted; it adds a lot of build time.

## Layout

A pure-Rust module with no game dependency (parsers, writers, the neutral `ModelData`, the
UE/glTF space conversion), so it can be unit tested with `cargo test` without the game, and
a user who is new to Rust can read it in isolation:

- `src/model_io/`: `data.rs` (`ModelData`: per material group positions, normals, UVs, indices; a
  material list with a base color and optional texture; decoded images as RGBA8), `obj.rs`,
  `gltf.rs` (read), `glb.rs` (write), `stl.rs`, `image.rs`, `space.rs` (axes, units, winding).
- `src/game/`: `texture.rs` (turning pixels into a `UTexture2D`), reference plane/model bindings;
  `mesh_builder.rs` (exists) and `mesh_component.rs` (exists) are reused.
- `src/features/`: `reference_images`, `reference_models`, `vehicle_export`, each a `Feature`
  with settings in `framework::settings`, keys as `Keybind`s ("Editor tweaks" category).
- `src/platform/`: a file picker wrapper (see "Choosing a file").

## Part 1: reference images

Pixels to a texture, then the texture onto something in the 3D view.

### Making the texture

- **Option A (reflection, preferred, *unverified*):** `UKismetRenderingLibrary::ImportFileAsTexture2D(
  WorldContextObject, Filename)` and `ImportBufferAsTexture2D(WorldContextObject, TArray<uint8>)`:
  static `UFUNCTION`s (BlueprintCallable) in Engine 4.27 that decode with the engine's
  `ImageWrapper` and return a `UTexture2D` (`PF_B8G8R8A8`, transient). Reached the same way the mod
  already calls other static library functions (`UKismetMaterialLibrary` in
  `docs/re/collision-shapes.md`). Reading the file in Rust and passing the bytes avoids the game's
  path rules. First check they exist by reflection in the running game.
  To check in game: the texture's filtering and mips (a big image drawn small may alias; a
  `UTexture::Filter`/`MipGenSettings` change after import needs `UpdateResource`), and sRGB.
- **Option B (fallback):** decode in Rust (`png`, JPEG crate) and make the texture ourselves with
  `UTexture2D::CreateTransient(w, h, PF_B8G8R8A8)` (static C++, **not reflected**, needs a
  signature, see "Update resilience" in `CLAUDE.md`), then lock mip 0 (`FByteBulkData::Lock`, not
  reflected either) and copy, `UpdateResource`. More engine internals to find; fully ours
  otherwise (any format a crate supports, mips built by us).

### Showing it

The editor hides every component of every actor except the editor's own actor and the world setup
actor, each frame (`UpdateHiddenComponents`, `docs/re/map-in-editor.md`). So reference objects must
be components **added to the editor actor** (`AActor::AddComponentByClass`, as the collision view
meshes are in `game/mesh_component.rs`), never new actors. They also need `NoCollision`: the
editor's clicks trace `ECC_Visibility` and must not hit references (`docs/re/selection.md`).

- **World-space plane (a "blueprint"):** a two-triangle static mesh with UVs from `mesh_builder`
  (`SetVertexInstanceUV` is reflected; check `Triangles` there supports UVs and a material slot),
  a `UMaterialInstanceDynamic` with the texture as a parameter, shown by a static mesh component.
  Settings per image: plane (front/side/top, or free), position, size (a width in cm; height from
  the aspect ratio), opacity, flip, lock. Pairs well with `features/ortho_view` (trace in
  orthographic front/side/top views).
- **Which material:** creating a `UMaterial` with texture sampling at run time is editor-only
  (`WITH_EDITOR` expression graph), so an existing material with a texture parameter is needed,
  ideally unlit and translucent so opacity works. *Unknown which one the shipping game has*:
  probe by walking the loaded `UMaterialInterface`s (for instances, the reflected
  `UMaterialInstance::TextureParameterValues`; a base `UMaterial` keeps no parameter list in a
  cooked build, so there only the virtual C++ `GetAllTextureParameterInfo`, not reflected, or
  `GetUsedTextures`, would tell), and each one's `BlendMode` and shading model; candidates: the
  game's UI/thumbnail materials, engine ones under
  `/Engine/EngineMaterials/`, brick materials (they have a texture slot, but are lit and read custom
  primitive data). The first milestone should settle this.
- **Fallback if no usable material exists:** a `UWidgetComponent` (UMG's 3D widget) showing a
  `UImage` with the texture (`SetBrushFromTexture`, reflected): it uses the engine's own widget
  materials, so no material hunt; costs a render target, pixelated at low `DrawSize`, harder
  depth-sorting.
- **Screen overlay alternative:** an image in the HUD (UMG `UImage` over the viewport, like the
  drag select box). Simple and always visible, but not anchored in space, so less useful than a
  plane.
- Draw order against bricks (reference behind or in front, depth test on/off) follows from the
  material's blend mode and sort priority (`SetTranslucentSortPriority` is reflected on primitive
  components); settle it with the chosen material.

## Part 2: reference models

File to `ModelData` (worker thread, `std::thread`: parsing and decoding need no engine), then to
engine objects on the game thread (engine objects are only safe there): static mesh + material
instances + components on the editor actor, as above. Big models: build in slices over several
frames (the engine's mesh build is the editor pipeline and may hitch beyond ~100k triangles; *not
measured*).

- **Space conversion** (`model_io/space.rs`): UE is left-handed, Z up, X forward, in cm; glTF is
  right-handed, Y up, +Z forward, in metres. With glTF (x, y, z) from UE (X, Y, Z):
  `(−Y, Z, X) / 100`, a mirror (determinant −1), so **triangle winding flips** both ways.
  Proposed from the axes' definitions; test with an asymmetric model (an "F" or a car) both ways.
  OBJ/STL have no declared units or up axis: settings for "units" (m, cm, mm, in) and "up axis" (Y
  or Z), per import.
- **Normals:** no reflected function sets them; the mod lets the engine compute them from the
  mesh description (`mesh_builder::COMPUTE_NORMALS`), which averages the polygons sharing a
  vertex. To keep an import's hard and soft edges, share a vertex only where the imported
  normals agree (else split it): the same trick `docs/re/collision-shapes.md` describes for flat
  faces.
- **Materials:** one polygon group and material slot per imported material; a plain game material
  with a base color parameter (the first milestone's probe also looks for one), textured ones with
  the Part 1 material. Opacity setting for the whole model.
- **Placement:** per model a transform (position, rotation, scale; typed values or the existing
  gizmo/keys, to decide), plus "match the vehicle's origin" and "snap to a brick" later.
- **Distance view mode** (roadmap): a view mode painting each brick by its distance to the
  reference model's surface, via `game/editor_view` (any color per brick, `docs/re/editor-view-modes.md`).
  CPU side: distance from a brick's center (or each corner) to the nearest triangle, with a grid or
  BVH over the model so large vehicles stay interactive.
- **Persistence:** the mod can't store anything in the vehicle file. References are per session,
  with their paths remembered (a sidecar file keyed by the vehicle's name, or the settings file):
  open question below.

### Choosing a file

The game has no file picker. Options, easiest first:
1. A folder the mod scans (`...\Config\...\EditorTweaks\references\`, or next to `EditorTweaks.ini`),
   its files listed in the panel as a `Choice` list, like the map's garage list
   (`features/map_in_editor`). No OS code; the user drops files into the folder.
2. The Windows common file dialog (`IFileDialog`, or `GetOpenFileNameW`) wrapped in
   `platform/file_dialog.rs` (safe wrapper, `// SAFETY:` on the unsafe, as `CLAUDE.md` asks). It's
   modal: run it on a thread of its own with the game window as owner, deliver the path to the game
   thread.

## Part 3: vehicle export

Reads the editor's bricks and writes a file. No engine objects are created, so it's the lowest
risk part and verifiable by opening the file in Blender.

### Where the geometry comes from

`game/brick_model.rs` already reads a brick's triangles: render buffers are GPU-only in the
shipping game, but each brick mesh's complex collision (a PhysX triangle mesh cooked from the
collision LOD's render triangles, vertices welded) is readable (`docs/re/brick-models.md`). It has
positions and indices only: **no UVs, no vertex normals** (flat or computed shading in the file), and
welded vertices (so smooth round surfaces can be re-smoothed with an angle threshold in the writer).
Each brick's transform and size come from the editor (the same data `features/brick_resize` uses).
`BrickModel::fit` already scales a model's box onto the brick's box for scalable bricks.

Pipeline: choose bricks (the whole vehicle, or the selection: a setting) -> per brick the model
(cached per mesh) -> brick transform -> `ModelData` -> writer.

### glTF layout

- One glTF mesh per brick model (type), one node per brick with translation, rotation and scale:
  `fit` is a per-axis scale and offset in the brick's own space, so each brick is a single TRS node
  (translation = brick place + rotation x offset). Files stay small (instancing), and Blender
  imports them as linked duplicates. A setting can bake everything into one mesh instead.
- Materials: one per brick color (reflected brick properties; `game/brick_materials.rs`
  knows the materials), base color factor, metallic/roughness from the brick material type if
  readable. Patterns and textures aren't exported first.
- Node names: the brick type name plus an index, so the model is usable in an outliner.
- Space: the Part 2 conversion, reversed (winding flips). The vehicle's own origin is the file's.

### Known gaps (to check, none verified)

- **Bricks with no readable model:** spinners (their own meshes made at run time, own body setup;
  `docs/re/brick-models.md` says `USpinnerBrick::GetBodySetup` reads +0x110, probably readable the
  same way), wheels, flaps and other bricks whose material or code deforms the mesh
  (`docs/re/collision-shapes.md`, "Some brick materials shape their mesh"). The first version
  exports what it can and reports ("N bricks without a model: skipped") in the log and the game's
  notification.
- **The collision LOD may differ from the render mesh** (listed as not checked in
  `docs/re/brick-models.md`). Compare an export of a test vehicle with the game's picture.
- Brick types without complex collision keep their box (what `brick_model` already does).
- Per-brick extra geometry that isn't a brick mesh (ropes, cables, liquid) probably can't be read.
- The exported geometry is the game's own brick meshes: fine for personal reference and 3D use,
  but worth a look at the game's terms before sharing exported models.

### Triggering it

A rebindable key (an "Editor tweaks" `Keybind`) and/or a panel button, an output folder setting
(default: next to the mod's settings), the file named after the vehicle plus a timestamp, a
notification with the path through the game's notification (the ortho zoom message uses one), and
settings for format (GLB, OBJ, STL), scope (vehicle, selection), instancing vs baked, and whether to
export collision shapes instead of the visuals (`features/view_modes/collision` already knows the
simple shapes).

## Probes to run first

Cheap checks that decide the unknowns above (each as a throwaway log line in the next `install`,
nothing user visible):

1. Do `UKismetRenderingLibrary::ImportFileAsTexture2D` / `ImportBufferAsTexture2D` exist in the
   reflection dump? Does a PNG and a JPG decode (log the size)?
2. Which materials with a texture parameter exist (`UMaterialInterface` instances; log their
   names, blend mode, shading model, parameters)? One unlit and translucent is the target.
3. Do `UStaticMeshDescription` UV and several-material-slot calls work through `mesh_builder`
   (it already does `SetVertexInstanceUV` for one group)?
4. Compare a brick model's triangles with the rendered brick (the collision LOD question).

## Order of work

1. **`model_io` core with tests**: `ModelData`, space conversion, GLB and OBJ writers, OBJ/glTF/
   STL readers, PNG/JPG decoding. Pure Rust, `cargo test` (write a file, read it back with the
   `gltf` crate, compare). No game needed.
2. **Vehicle export** (GLB, then OBJ): on top of `brick_model`. First visible result; also the test
   bed for the space conversion and winding.
3. **Probes 1-3**, then **reference images** (plane in the 3D view, opacity, plane presets).
4. **Reference models** (OBJ, glTF, STL): mesh building with UVs, groups and materials on top of
   the same component code as images; placement controls.
5. **Distance view mode, opacity polish, remembered references.**

## Open questions (for the user)

- Where should references live between sessions: remembered per vehicle (a sidecar file keyed by the
  vehicle's name; renames lose them), or only for the session?
- Are references meant to be seen only in the editor, or also from the test drive/map view? (The
  editor-only plan above is the simple one.)
- Export scope: just the visuals, or collision shapes too? Selection only as well as the whole
  vehicle?
- File choice: is a drop-in folder enough at first, or is a real file dialog wanted from the start?
