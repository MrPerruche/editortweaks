# Reference images and models (and vehicle export, later)

Plan and research for the user's goal (2026-10-16), none of it built yet:

1. **Reference images**: PNG/JPG files shown in the editor (blueprints to trace over).
2. **Reference models**: 3D models (OBJ, glTF/GLB, STL, ...) shown in the editor as non-brick
   previews.
3. **Vehicle export** as a 3D model: deferred (the user is gathering information first). Its notes
   are kept at the end.

Order: images first, then models. Both behave like bricks in the editor without being bricks (move,
rotate, scale, optionally selectable) and are remembered per vehicle in a small file next to it.

Status: researched from the code, the game files on this machine and UE 4.27 knowledge. **Nothing
here was tried in game.** Marks: *verified* (seen on this machine or in the disassembly),
*inferred* (reasoned from code), *unverified* (from memory of the engine; check first, see
"Probes"). Confirmed engine/game findings move to `docs/re/` once found.

Decisions so far (user, 2026-10-16):
- **FBX is dropped** (closed format, parser effort for little gain; glTF through Blender instead).
- Open source libraries are fine if their licenses fit this project's **GPL-3.0** (MIT, BSD,
  Apache-2.0, zlib, public domain, MPL, LGPL; never the proprietary Autodesk FBX SDK).
- **No bloat, no lag**: a longer compile is fine; slower game loading or in-game stutter is not.
- Everything stays client side (`CLAUDE.md`): a reference changes what *this* player sees in the
  editor; nothing a vanilla player loads is touched, and the vehicle file stays as the game wrote it.
- Foreign files in a vehicle's folder are fine: the game ignores them (the user, 2026-10-16).
- Answers to the open questions (user, 2026-10-16; section 8): undo wanted, after the first
  prototype; a new reference is a **link** by default; references and bricks are never selected
  together; references stay out of thumbnails unless they appear there by themselves.
- The sidecar is `editortweaks.toml`: `.etm` was only an invented name, not an existing format.
- The map is a reference too: **Map, images and models share one "References" panel section**
  (one header, not three).

## 1. How the game stores a vehicle (for the sidecar file)

Seen in `%LOCALAPPDATA%\BrickRigs\SavedRemastered\Vehicles` (*verified*, 542 folders, 117 MB):

- One folder per vehicle, named by a number (`1`, `10`, `489`, ...): `Vehicle.brv` (the bricks),
  `MetaData.brm` (name, description, dates, tags), usually `Preview.png`, often `Backup.brv` and
  `Autosave.brv`, rarely `VehicleManualBackup*.brv`. Because folders are numbered, renaming a
  vehicle doesn't move its folder.
- Foreign files stay: other tools keep folders and files in there and the game leaves them alone
  (user, and seen on disk).
- **Steam Cloud** (`steam_autocloud.vdf` sits in `Vehicles/` and in `Config/WindowsNoEditor/`)
  syncs only some kinds of files. The user has seen a nested `Vehicle.brv` travel and `.toml` files
  not. So a `.toml` sidecar and its copies stay on this PC (not synced to others), and don't count
  against the cloud quota. Which extensions *do* sync isn't known: keep copies' extensions as they
  are and look at the first test (an image copy could be one that syncs).
- **Workshop**: the 114 subscribed items in `steamapps/workshop/content/552100/<id>/` each hold
  only `Vehicle.brv` (*verified*). So a published vehicle carries nothing else: references never
  travel with it (good for the client-side rule and for copyrighted models; also means nobody
  else sees them). Publishing can't be tested (public action, `CLAUDE.md`); this is from what
  came down.
- **Duplicate in the game's browser** (`UBrickGameInstance::DuplicateUGCItem`, BRMK 0x230700):
  calls `IFileManager::Copy` (vtable +0x40, the 8-argument one) twice, so it copies files one by
  one, not the folder (*inferred*: the disassembly shows two copies, their paths unread). A
  duplicate won't have the sidecar. **Delete** (`DeleteUGCItem`, 0x230160) calls a three-argument
  `IFileManager` function (vtable +0x68, `RequireExists` and `Tree` both true: probably
  `DeleteDirectory`), so deleting a vehicle takes the sidecar and copies with it (*inferred*).
  Upload (`StartUGCUpload`, 0x25F3A0) goes through a virtual of a UGC subsystem; not traced.
- The editor knows the vehicle: `ABrickEditor::GetFileInfo()`/`SetFileInfo`, `OnFileInfoChanged`
  (BRMK exports), a `FUGCFileInfo` struct (reflected, `StaticStruct<FUGCFileInfo>`) with an
  `EUGCFileType` enum; also `LoadItem`, `SaveItem`, `SaveItemInternal`, `OnSaveItemAs`.
  `GetFileInfo` isn't reflected; the struct is read from the editor between two reflected fields
  (`game/editor_file.rs`, `docs/re/references.md`). Saving is a C++ function (signature needed);
  polling is the update-resilient way, see below.

## 2. References behave like bricks, without being bricks

### Why not real editor objects

Selection, the gizmo, mirroring, undo and saving are all built on `UBrickEditorObject` and its
owner `UBrickEditorInterfaceComponent` (exports: `DuplicateBrickEditorObjects`,
`DestroyBrickEditorObjects`, `CalcBrickEditorObjectsMass/Price/Bounds`, objects built from classes
by `UBrickAssetManager::BuildBrickEditorObjects`; the undo snapshot serializes the vehicle in
`PostModify`, `docs/re/editor-operations.md`). An editor object of ours would be counted in the
vehicle's mass, price and bounds and saved into `Vehicle.brv`, which breaks the client-side rule.
So references are a layer of our own, using the game's pieces where they are free.

### Pieces

| Need | Plan |
|---|---|
| See-through look | Models dithered (lit, textured, masked: no smearing) or smooth (translucent), a setting; images smooth. See `docs/re/references.md`. |
| OBJ quirks | Blender 5 writes `usemtl` without a name for objects without a material; tobj rejects the whole file over it, so those lines are renamed to an unknown material first (`readers/obj.rs`). |
| Showing | Components added to the **editor's own actor** (`AddComponentByClass`, as `game/mesh_component.rs` does): the editor hides every other actor's components each frame (`docs/re/map-in-editor.md`). No collision. |
| Selecting | Our own pick: when a click reaches the editor and hits no brick before the reference (or the reference is in front), select it. Ray against the reference's triangles/box in `f64` (we already deproject the mouse and trace: `game/editor_trace.rs`). A selected reference unselects bricks (`SelectObjects` with none) so the panel shows our section (below). |
| Selected look | The game's own outline: it's custom depth + a stencil value (`SetRenderCustomDepth`, `SetCustomDepthStencilValue`; encoding in `docs/re/selection.md`, "The selection outline"), so our components get exactly the bricks' selected/hovered look by setting the same stencil. `mesh_component.rs` already copies a brick's outline. |
| Moving | **Prototype: numbers in the side panel.** Then our own gizmo: arrows/rings drawn with the line batcher and hit by ray-vs-axis math, dragged with the `f64` math `brick_resize` uses, Shift/Ctrl slow-down from `move_slowdown`. Reusing the game's gizmo mesh is deeper (its move mode is built for editor objects: `docs/re/editor-operations.md`, "The gizmo's place"): not planned. |
| Selectable switch | A toggle in the panel (the user's idea): off = never picked (no ray test, no outline), the component stays visible. Refinement, not for the first prototype. |
| Undo | The game's undo restores vehicle snapshots; reference edits aren't in them. Wanted by the user, but after the first prototype: our own small stack bound to the same keys, running only when a reference was the last thing changed (to design). |
| Mirror mode | A toggle "mirror across the mirror plane": a second component with negative scale on the editor's mirror axis. Cheap, since UE flips winding for negative scale itself. Optional. |

### The panel

`features/map_in_editor/panel.rs` already adds a "Map" section under `Properties.Editor` when
nothing is selected (toggle and dropdown rows, listed again with one reflected call:
`docs/re/map-in-editor.md`, "The panel when nothing is selected"). A **"References"** section
works the same and also takes over the Map rows (the map is a reference too, one header for all
three kinds; the existing "Map" section moves into it): a dropdown of this vehicle's references (choosing one selects it), "Add image" and
"Add model" as dropdowns listing the files in the drop-in folder (below), then the selected
reference's rows. Number rows (position, rotation, scale, opacity) need the panel's numeric
property reflection for rows of ours, which the options menu doesn't have yet ("Later: numbers
(sliders)" in the roadmap's settings menu entry; the property-panel notes cover the game's numeric properties,
`docs/re/property-panel.md`): the main new piece of work in the UI. Probe 6.

### What a reference stores and offers

Both kinds: name, source file, visible, **selectable**, locked, position (X/Y/Z cm), rotation
(pitch/yaw/roll or a quaternion typed as angles), scale (uniform, optionally per axis), mirror per
axis (X/Y/Z flips), opacity, "show in": all views / only orthographic / only perspective (a
blueprint wants the ortho views).

Images: plane preset (front/back/left/right/top/bottom or free), size (width in cm; height from
the aspect ratio, ratio lock), double sided, flip horizontal/vertical, tint, brightness (unlit),
filtering (smooth / pixel-exact for pixel art), draw order against bricks (behind / in front),
pivot (centre / corner), crop (later).

Models: **up axis and forward axis** (Y-up or Z-up; which axis points forward, +/-, the user's "X /
Y / Z forward"; glTF is fixed Y-up/+Z forward so it needs none), handedness flip, **units** (m, cm,
mm, in; or "fit to a given size"), pivot (file origin / bounds centre / bottom centre), use file
materials / one flat colour, backface culling, lit or flat, wireframe overlay (later), hide a
sub-mesh (later). The import settings re-run the import when changed.

Both: a **missing file** keeps its entry, greyed in the panel with its path (never dropped
silently); a **changed file** (modification time polled every second or so) reloads, handy while
editing a model in Blender next to the game.

## 3. Storage: the sidecar

### Where and what

`<vehicle folder>/editortweaks.toml` (the user's `.etm` was an invented name; `.toml` is the same
idea with syntax highlighting and, going by the user's observation, a kind of file Steam Cloud
doesn't sync; an unknown extension would be a guess on that). Copies of files go in
`<vehicle folder>/editortweaks/` (the folder only exists if a copy was asked for).

TOML over JSON or binary: hand-editable and diffable, comments allowed, tolerant to change. With
`serde` + the `toml` crate (MIT/Apache-2.0), every field gets `#[serde(default)]`: a newer mod
reads an older file, an older one ignores unknown keys, so updating the format costs a field
and a default. A `version = 1` is there for the day that isn't enough.

```toml
version = 1

[[reference]]
id = "r1a2b3"            # stable, for the panel and undo
kind = "model"           # "image" | "model"
name = "Hilux body"
visible = true
selectable = true
source = { mode = "copy", path = "D:/Models/hilux.glb", copy = "editortweaks/r1a2b3.glb", bytes = 1843200, modified = "2026-10-12T09:14:00Z" }
position = [0.0, 0.0, 0.0]   # cm, vehicle space
rotation = [0.0, 0.0, 0.0]   # pitch, yaw, roll in degrees
scale = [1.0, 1.0, 1.0]
mirror = [false, false, false]
opacity = 0.5
import = { up = "Z", forward = "X", units = "cm", pivot = "origin" }
```

### Link or copy (the user's idea)

Per reference, `mode = "link"` (only the path) or `"copy"` (a copy in the vehicle's folder), with a
global default in the settings menu (a `Choice`). Rules:
- Loading tries the copy first, then the link; the other way round for a link-mode reference
  (the original may have changed). If neither exists: the missing state above.
- A copy is `editortweaks/<id>.<ext>`. A model that points to other files (`.gltf` with its `.bin`
  and images, `.obj` with its `.mtl` and textures; `model_io::companions`) is copied with them into
  `editortweaks/<id>/`, keeping its file name and the files' relative paths; files outside the
  model's own folder are left out (reported as `Copied::skipped`). The size cap counts them all.
- Switching mode is a panel action: link -> copy copies the file now, copy -> link deletes the
  copy (asking first through the panel's wording; it's the mod's own file).
- Copies don't travel with the vehicle (Workshop carries `Vehicle.brv` only, Duplicate copies two
  files, Steam Cloud skips them as far as seen). The panel says so on copy mode: "stays on this PC".
- A size cap on copies (a few hundred MB per vehicle folder would be odd next to 117 MB for 542
  vehicles): refuse above a setting, default 64 MB per file, say so in the panel.
- Writes are atomic (write a temp file, rename). Path comparison is case-insensitive; paths are
  stored with forward slashes.

### When it's read and written

Polled, no hook on the game's save (update-resilient, *inferred* to work): each editor tick (or
every few ticks) read the editor's file info (`FUGCFileInfo`, reflected: probe 5) and map it to the
vehicle's folder. When it changes (loading another vehicle, Save As to a new number, a new vehicle
getting its first save), read that folder's sidecar; if references were already open (a Save As),
**write them to the new folder** instead, so they follow. Edits write through (debounced to about
a second). A brand-new, unsaved vehicle keeps references in memory and writes them when its folder
exists. Opening a vehicle from the Workshop folder (read-only content) shows no references.

## 4. Reference images

### Making the texture

- **Option A (reflection, preferred, *unverified*):** `UKismetRenderingLibrary::
  ImportFileAsTexture2D(WorldContextObject, Filename)` / `ImportBufferAsTexture2D(WorldContextObject,
  TArray<uint8>)`: static `UFUNCTION`s in Engine 4.27 that decode with the engine's `ImageWrapper`
  and return a `UTexture2D` (`PF_B8G8R8A8`, transient). Called like the other static library
  functions the mod uses (`UKismetMaterialLibrary`, `docs/re/collision-shapes.md`). Reading the
  file in Rust and passing the bytes keeps path rules out of it. Check filtering, mips and sRGB
  in game (a big image drawn small can alias).
- **Option B (fallback):** decode in Rust (`png`, `zune-jpeg`; or the `image` crate with only those
  features) and make the texture ourselves with `UTexture2D::CreateTransient` (static C++, not
  reflected: a signature) plus a copy into mip 0 (`FByteBulkData::Lock`, not reflected). Any format
  a crate decodes, mips built by us; more engine internals to find.
- Large images: cap the longest side (default 4096), downscale on the worker thread. Decoding runs
  on a worker thread; only the texture creation happens on the game thread.

### Showing it

- A two-triangle plane from `game/mesh_builder.rs` with UVs (`SetVertexInstanceUV` is reflected and
  used there already; check that `Triangles` supports UVs and a material slot) and a
  `UMaterialInstanceDynamic` with the texture as a parameter.
- **Which material?** *Answered (probe 2): the engine's one-sided `Widget3DPassThrough` instances,
  masked when opaque, translucent for opacity (`docs/re/references.md`).* A `UMaterial` with texture
  sampling can't be made at run time (the expression
  graph is editor-only), so an existing one with a texture parameter is needed, ideally unlit and
  translucent for opacity. Probe 2 lists them: for
  instances the reflected `UMaterialInstance::TextureParameterValues`; a base `UMaterial` keeps no
  parameter list in a cooked build (only the virtual C++ `GetAllTextureParameterInfo`, not
  reflected), plus blend mode and shading model. Candidates: the game's UI/thumbnail materials,
  `/Engine/EngineMaterials/*`, brick materials (a texture slot, but lit and reading custom
  primitive data).
- **Fallback with no usable material:** a `UWidgetComponent` (UMG 3D widget) showing a `UImage`
  (`SetBrushFromTexture`, reflected): engine widget materials, no hunt; costs a render target and
  harder depth sorting.
- Draw order against bricks follows the blend mode and `SetTranslucentSortPriority` (reflected).

## 5. Reference models

### Formats

The reader sits behind a small trait (`fn read(&Path) -> Result<ModelData, Error>`), so a format
is one module and a bigger library could be added later behind a cargo feature.

| Format | What it is | Strengths | Gotchas | Rust crate (license, verify) | Plan |
|---|---|---|---|---|---|
| **glTF / GLB** | Khronos's "JPEG of 3D", 2015+; JSON + `.bin` + images, or one `.glb` | Units/axes fixed (m, Y-up, +Z forward): no guessing; PBR materials; textures embedded; node tree with TRS; what Sketchfab and Blender export by default | Extensions: Draco and meshopt compression, KTX2/WebP textures are not read (Draco/meshopt would need decoders: `meshopt` is MIT); skins/animation ignored | own reader on `serde_json` (the `gltf` crate cost 1.3 MB of code) | **First.** The recommended format. |
| **OBJ (+MTL)** | Wavefront, 1990s text; the lowest common denominator | Everything reads and writes it; positions, UVs, normals, groups, MTL colours and texture paths | No units, no up axis (the import settings above), no hierarchy; n-gons to triangulate; smoothing groups; paths in the MTL are relative; huge files from scans | `tobj` (MIT) | **First.** |
| **STL** | 3D printing; triangle soup, binary or ASCII | Trivial | No indices (weld), no UVs or materials; mm and Z-up by habit | `stl_io` (MIT), or by hand (~60 lines) | First (cheap). |
| **PLY** | Scans/research; ASCII or binary | Vertex colours | Few tools write textures | `ply-rs` (MIT) | Later, if asked. |
| **3MF** | Modern 3D printing; zip + XML | Units, colours, several objects | Rare outside printing | `threemf` crates (young) | Probably never. |
| **DAE (Collada)** | XML, SketchUp/older assets | Scene graph | Legacy; no good pure-Rust reader | none solid | Only via Assimp. |
| **FBX** | Autodesk's, in nearly everything (Unity, Mixamo, marketplaces) | Common | Closed, many versions, units/axes in the header, skeletons | `ufbx` (C, public domain/MIT) | **Dropped** (decision). |
| 3DS, X3D, OFF, USD(Z), BLEND | legacy / specialist | | BLEND is only read well by Blender: tell users to export | | No (Assimp covers some). |

Images: PNG and JPG first; BMP, TGA, WebP, DDS, HDR/EXR would each need a decoder crate, only if
asked (each adds code). 

### Assimp, honestly

What it is: the Open Asset Import Library, C++, BSD-3-Clause, about 40 import formats (OBJ, glTF,
FBX, DAE, 3DS, X3D, ...) and about 10 exporters, with post-processing (triangulate, join vertices,
generate normals). Rust bindings exist (`russimp`, MIT) and it has a plain C API small enough to
bind by hand.

- **Game loading and lag**: a statically linked library costs nothing at startup (the loader maps
  the DLL; Assimp registers its importers when an `Importer` object is made, not at load) and
  nothing per frame. Parsing happens on a worker thread in any case, for any library. The real
  stutter risk is not parsing but building the engine mesh on the game thread
  (`BuildFromStaticMeshDescriptions`), which is the same whichever parser fed it. So Assimp
  would not add lag. It would add **size**: with every importer except the wanted ones switched
  off (`ASSIMP_BUILD_ALL_IMPORTERS_BY_DEFAULT=OFF`, exporters off) roughly 1-3 MB of code
  (an estimate, not measured), against the mod's 2.0 MB of code now (`.text`; the rest of the
  16 MB file is debug info). With all importers about 10 MB or more.
- **Build**: the project's MinGW already has `g++`, `cmake` and `ninja`. A cold Assimp build takes
  minutes, once (cached by the build script until `cargo clean`); later builds are as fast as now.
  It also needs libstdc++ and libgcc linked statically into the DLL (`-static-libstdc++
  -static-libgcc`) so no extra DLLs are needed beside the game.
- **Safety**: it parses files people download from the internet inside the game process, in C++
  with a history of memory-safety bugs in its parsers; a bad file could crash the game or worse. A
  Rust parser fails with an error instead. This is the main reason to prefer crates for the common
  formats.
- **Exporters**: its glTF/OBJ/STL writers are fine for plain geometry, but vehicle export needs
  our own mesh assembly anyway and a GLB writer is ~200 lines; no need for it.

Recommendation: **Rust crates for glTF, OBJ, STL now** (a few hundred KB of code in total, memory
safe, no C++ build). Keep Assimp as an **optional cargo feature, off by default** ("more formats":
DAE, 3DS, X3D, ...), only if wanted later; the trait above makes that one extra reader. If the
user wants it in anyway, the cost is size and a slower cold build, not lag.

### Pipeline and cost

- Parse/decode on a **worker thread** (`std::thread`; plain data in, plain data out, no engine
  calls), producing a neutral `ModelData`: per material group positions, normals, UVs, indices;
  materials (base colour factor, texture index); decoded images as RGBA8.
- Build the engine objects on the game thread: `mesh_builder` (UVs, a polygon group and material
  slot per material), material instances, components on the editor actor. Large models are
  built **in slices over several frames** (a triangle budget per frame) so there's no hitch; the
  reference appears when done and the panel says "loading".
- Limits, settable: triangle count (default 1,000,000), texture side (4096), file size (256 MB).
  Over the limit: refuse with a message, never freeze.
- Space conversion (`model_io/space.rs`): UE is left-handed, Z-up, X-forward, cm; glTF is
  right-handed, Y-up, +Z-forward, m. glTF (x, y, z) from UE (X, Y, Z) is `(-Y, Z, X) / 100`, a
  mirror (determinant -1). *Checked in game:* the mirror alone gives Unreal's winding, so the
  corners keep their order (`docs/re/references.md`, "Triangle winding"; the first prototype
  swapped them too and showed models inside out). The OBJ/STL settings (up, forward, units) feed
  the same conversion.
- Normals: no reflected function sets them; the engine computes them from the mesh description
  (`mesh_builder::COMPUTE_NORMALS`), smooth where vertices are shared. To keep an import's hard
  and soft edges, share a vertex only where the imported normals agree (else split it); with no
  normals in the file, share by position and an angle threshold.
- Materials: a plain game material with a base-colour parameter (`M_Simple_Master`), textured
  parts the Bricksville sign's lit material with the part's texture (loaded by path when first
  needed; the unlit image material showed no shading or shadows: `docs/re/references.md`).
  Opacity is one parameter for the whole reference.
- **Distance view mode** (roadmap): colour each brick by its distance to the model's surface via
  `game/editor_view` (`docs/re/editor-view-modes.md`); CPU side with a grid/BVH over the model.

### Layout

- `src/model_io/` (no game dependency, unit tested with `cargo test`): `data.rs`, `obj.rs`,
  `gltf.rs`, `stl.rs`, `image.rs`, `space.rs`, `sidecar.rs` (the TOML, read/write/migrate).
- `src/game/`: `texture.rs` (pixels to a `UTexture2D`), `vehicle_file.rs` (the editor's file info
  to a folder).
- `src/features/`: `references` (the list, panel section, selection, picking), `reference_images`,
  `reference_models`; settings in `framework::settings`, keys as `Keybind`s.
- `src/platform/`: `file_dialog.rs` later.

### Choosing a file

The game has no file picker. Until the panel exists, references are added by editing the
vehicle's `editortweaks.toml` by hand (links to any file). Then: a drop-in folder the mod scans
(`...\SavedRemastered\EditorTweaks\references\`, created on first use), its files listed in the
panel's "Add" dropdowns (like the garage list). Later: the Windows common file dialog
(`IFileDialog`) in `platform/file_dialog.rs`, on a thread of its own with the game window as
owner (modal), the chosen path handed to the game thread.

## 6. Probes (run first, throwaway log lines)

Run 2026-10-16, answers in `docs/re/references.md`: 1 yes (both, ~1 ms); 2 the engine's widget
materials for images, `M_Simple_Master` for model colors; 3 yes (UVs and a material per mesh);
4 yes for the masked material, not translucent ones; 5 `GetFileInfo` isn't reflected, the struct is
read beside two reflected fields, `LocalItemId` is the folder name (a new one after Save As); 6
from the code (section 7); 7 a save leaves foreign files alone, Save As doesn't copy them (the
mod carries the sidecar over itself).

1. `UKismetRenderingLibrary::ImportFileAsTexture2D`/`ImportBufferAsTexture2D` present? Does a
   PNG and a JPG decode (log the size)?
2. Which materials have a texture parameter, which are unlit/translucent, which take a colour
   parameter? Log name, blend mode, shading model, parameters.
3. Do `UStaticMeshDescription` UVs and several material slots work through `mesh_builder`?
4. Does a component with a custom-depth stencil on the editor actor get the bricks' selection
   outline (`docs/re/selection.md`)?
5. `FUGCFileInfo`'s fields (reflect the struct): which one gives the vehicle's folder number? Does
   it change on Save As? What is it for a new, unsaved vehicle and for a Workshop vehicle?
6. Can rows of ours in the panel be numeric (sliders/typed numbers)? What does it take to reflect
   them (`docs/re/property-panel.md`, "Numeric brick properties")?
7. Does the game delete or move foreign files in a vehicle folder on Save/Save As/autosave
   (it hasn't on this machine)? Put a test file in a folder and save from the editor.

## 7. Order of work

1. **`model_io` core with tests** (no game): `ModelData`, `space.rs`, `sidecar.rs` (round trip,
   old/new versions), glTF/OBJ/STL readers, PNG/JPG decoding. `cargo test`.
   *Built 2026-10-16* as the workspace crate `crates/model_io` (`cargo test -p model_io`):
   `model` (parts, materials, bounds, checks), `space` (axes, units, pivot, winding), `plane`
   (the image rectangle), `image` (PNG/JPEG to RGBA8, PNG out), `readers` (glTF/GLB, OBJ+MTL,
   STL), `sidecar`. Size matters: with the `image` and `gltf` crates it added 5 MB of code to
   the DLL (`cargo bloat`: `image` pulls colour management, `moxcms` + `pxfm`, 1.7 MB; `gltf_json`
   0.9 MB), so it uses `png` + `zune-jpeg` and its own small glTF reader on `serde_json`: the
   DLL's code went from 2.0 to 3.0 MiB (`model_io` with its share of serde/JPEG code 0.5 MiB,
   `toml` 0.2 MiB).
2. **Probes 1-7.** *Run 2026-10-16* with a throwaway prototype (the drop-in folder's first image
   and model at the vehicle's origin, log lines for materials and file info); answers in section
   6. Probe 6, answered from the code (2026-10-16): feasible the way our check-box and list rows
   are made (`game/property_reflection.rs`: copy a game brick property's vtable, replace the
   slots touching a value). For numbers, copy a numeric property's vtable (three variants, e.g.
   RVA 0x38DC498; `game/numeric_property.rs`) and replace [6] compare, [7] valid, [8] same
   values, [9] serialize, [18] get, [19] set, [4] destructor; the range is a stored `TOptional`
   at +0x30 (a delegate at +0x58), the axis lock at +0x68, "don't clamp" at +0x98. To find: the
   variants' RTTI type names (for a `VtableSig`), and where the value type that picks the units
   (cm vs bricks, degrees) is kept.
3. **References framework** (`features/references`): vehicle-folder tracking, sidecar
   read/write, the panel section, picking and the selected look, the numeric rows.
   *Started 2026-10-16*: the open vehicle's folder from the editor's file info (`vehicle.rs`), its
   sidecar read and shown (`scene.rs`: images at once, models read on a worker thread and built
   2000 triangles a piece within 4 ms a frame), every reference's position, rotation, scale,
   mirror, visibility and (images) width, flips, pivot, two sides and opacity applied. The open
   vehicle, its sidecar and the referenced files are looked at every second, so editing them by
   hand shows at once. Checked in game the same day: image orientation, model winding and
   handedness, live edits, the sidecar surviving a save. A Save As carries the references to the
   new folder (copies copied, relative links made absolute); models without normals get sharp
   edges split at 30°.
   The side panel's References section (2026-10-16, `references/panel.rs`; the map's rows first,
   the old "Map" title gone): Add image / Add model dropdowns (the drop-in folder's files, two
   levels deep, and "Browse..." for the Windows file dialog, `platform/file_dialog.rs`, on a thread
   of its own), New references Link/Copy (a setting), a Reference dropdown, then for the chosen one
   visible, link/copy (copying or deleting the copy), opacity steps, mirror, remove; images: "Place
   as" a blueprint view (front/back/left/right/top/bottom: turned to face that side, placed behind
   the vehicle's box as seen from there, as wide as the vehicle; new images start as a front view),
   both sides, flip; models: pivot, and for OBJ/STL up axis, front axis, units. Rows of the chosen
   reference come and go with it (`Row::when`). Every change is written to the sidecar at once; an
   unsaved vehicle keeps its references in memory and its first save writes them (copies made
   then). Textured model parts are lit (they were unlit, without shadows).
   Reworked after the user's first try (2026-10-17): references are selected by clicking them
   (never together with bricks; the bricks' outline on opaque ones), Delete removes and Ctrl + C
   duplicates the selected one, and a "Select references" switch turns clicking them off. The
   panel: Add image / Add model as Link... / Copy... buttons (straight to the file dialog; the
   drop-in list and the "New references" setting are gone), Hidden references: Show all, then for
   the selected one its name, opacity (a slider; 0 hides it), position, rotation, width (images) or
   scale (models) as number rows of our own (`game/property_reflection.rs`, see
   `docs/re/property-panel.md`), link/copy, and for models pivot and the OBJ/STL axes and units.
   Opacity and width no longer rebuild anything (materials and scale change in place); models
   below 100 % use the glass bricks' material. Gone from the panel (still in the TOML): the
   reference list, Remove, "Place as", both sides, flip, mirror. Changes are written 0.4 s after
   the last one (slider drags).
   Not yet: the game's move gizmo (references aren't editor objects; an own gizmo would be needed),
   undo, `show_in`, `pixelated`.
4. **Reference images** (plane in the 3D view).
5. **Reference models** (OBJ, glTF, STL), sliced building, import settings.
6. Refinements: selectable switch, mirror mode, live reload, the distance view mode, own gizmo,
   own undo, file dialog.

## 8. Decisions on the open questions (user, 2026-10-16)

- **Undo**: wanted, but a polish step after the first prototype (own stack, see the table above).
- **Link or copy**: a new reference is a **link** by default (a copy is easy bloat); copy stays a
  per-reference switch and a setting.
- **Selection**: references and bricks are never selected together. Selecting a reference
  unselects bricks, and selecting a brick unselects the reference.
- **Thumbnails**: don't care. If references show up in the game's captures, leave them; don't try
  to make them appear or hide them.

Still to look at right before building: probes 1-7 above (they decide the texture route, the
material, the numeric rows and the folder tracking), the crate versions and licenses at the time
(`cargo tree` / `cargo deny`-style check), and that the DLL size and load time stay as they are.

## Later: vehicle export (notes)

Reads the editor's bricks and writes a file; no engine objects created. Geometry from
`game/brick_model.rs` (each brick mesh's complex-collision triangles: positions and indices only,
no UVs or normals, `docs/re/brick-models.md`), the brick's transform and size from the editor,
`BrickModel::fit` for scalable bricks. Output GLB first (a hand-written writer, ~200 lines, with
`serde_json`), then OBJ (+MTL), STL; one glTF mesh per brick type and one TRS node per brick
(`fit` is a per-axis scale and offset in the brick's own space), a material per brick colour; the
space conversion of section 5 reversed (winding flips). Known gaps to check: bricks without a
readable model (spinners, wheels, flaps, `docs/re/collision-shapes.md`), the collision LOD against
the render mesh (`brick-models.md`, "Not checked yet"), no UVs. Triggered by a key and/or a panel
button; settings for format, scope (vehicle or selection), instancing or baked, and collision
shapes instead of visuals. The exported geometry is the game's own brick meshes: fine for personal
use, but worth reading the game's terms before sharing exported models.
