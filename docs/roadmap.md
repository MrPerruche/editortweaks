# Roadmap

An outline of what the mod could become, not a spec. Ideas from the user (2026-10-08); notes on
likely approach are first guesses, to be revised as the editor gets reverse engineered.

## Ideas

- **Slider limits**: remove or raise them. Started 2026-10-09 as `features/property_limits`:
  no limits by default, per-property rules in `rules.rs`, brick size capped by a budget of
  connector points (the game's largest brick has 4.86 million). Limits depending on other
  properties (brick size on connector spacing, light brightness on direction) are re-checked when
  those change (`dependencies.rs`).
  Out-of-range warnings added 2026-10-11 (`property_limits/highlight.rs`, untested in game yet):
  sliders outside of the game's range turn `Highlight` (the game's warning color), outside of the
  rules too `Negative`. Possible later: the game's own row warning ("⚠ Name" in `Highlight`, as its
  menus do), see "Color styles and warnings" in `docs/re/property-panel.md`.
- **Actuator arrows past zero**: a linear actuator's max extension above 0 or max contraction
  below 0 (possible with the lifted limits) lost its green/red arrow; it now shows turned around.
  Done 2026-10-11 as `features/actuator_arrows` (see `docs/re/actuator-arrows.md`), untested in
  game yet.
- **Suspension arrows**: a selected axle shows its suspension travel as a green arrow from each wheel
  connector, down, `SuspensionLength` long (where a wheel is built is the top of its travel). Done 2026-10-11 as `features/suspension_arrows` (see
  `docs/re/axle-arrows.md`), untested in game yet.
- **Precise sliders**: sliders stop rounding values away (like BrickEdit Interface 2 does).
  Started 2026-10-09 as `features/precise_sliders`: exact, ungrouped text while editing; labels keep
  the game's decimals but show trailing zeros when digits are hidden.
- **Delta sliders**: change a value relative to its current one, e.g. while holding Alt.
  Started 2026-10-09 as `features/slider_drag`: Alt-drag changes the value continuously, faster
  the farther the mouse and the larger the value; drags no longer stop at the slider's ends
  (`LIFT_SLIDER_LIMITS`).
- **`x` macro in number inputs**: `x` stands for the previous value (`x*2`, `x+0.5`, ...).
  Started 2026-10-09 as `features/slider_formulas`: `x` is each edited object's value, `X` the
  slider's; both in the shown unit.
- **Editor distance limit**: bricks and the camera may go 100 km from the vehicle's origin along
  each axis. Done 2026-10-11 as `features/editor_bounds` (`BOUNDS` in its constants): raises the
  reflected `EditorBounds` of the editor's static settings (see `docs/re/editor-bounds.md`),
  untested in game yet. Brick places are `f32`: 0.5 cm steps past 42 km, 1 cm past 84 km.
- **Camera speed**: exponential speed steps. Done 2026-10-09 as `features/camera_speed`
  (slowest, fastest and step count in its constants).
- **Orthographic view**: a key (Numpad 5) switches the editor's view to orthographic. Started
  2026-10-14 as `features/ortho_view` (see `docs/re/editor-camera.md`). First test: worked, but the
  sky broke (fixed by shrinking it while orthographic) and the switch jumped in size; now it keeps
  what's in the middle the same size (a 0.3 s dolly zoom, the camera moved back when leaving) and
  the zoom steps zoom it without limits. Second test: the sky renders; but switches often leapt
  hundreds of metres to kilometres (the trace went through the vehicle), and turning felt random.
  Now the switch keeps the size of the brick nearest the middle of the view, and turning the camera
  turns around the brick in the middle. Third test: both fine; but faces flickered at half a
  millimetre (10 km of depth) and the view couldn't look inside the vehicle (it showed from 100 m
  behind the camera, "camera super far + 1° FOV"). Now it shows from the camera on, cutting through
  what the camera is in, as deep as its width calls for, and the game's zoom notification says
  "Perspective 20" or "Orthographic 2.4 m" (asked for in the same test). Fourth test: fine; asked
  for Shift + right drag to pan, and leaving the editor while orthographic broke the view on coming
  back (old frames smeared: the sky stayed unshrunk). Now Shift + right drag pans either view
  (`keybinds::PAN_VIEW`) and leaving the editor switches back to perspective. Possible follow-ups:
  focus (`FocusCamera`) fitting the zoom, thumbnails taken from an orthographic view, a narrower
  smallest width.
- **Slower moves**: Ctrl slows down moving bricks and turning the camera with the mouse, Shift
  moving bricks by a gizmo handle. Done 2026-10-09 as `features/move_slowdown` (factors in its
  constants). Shift replaced Alt (2026-10-10, Alt is the resize tool's key); it leaves the camera
  and G/dragged moves alone (there it's the game's "move perpendicular").
- **Rebindable keys**: the mod's keys in the game's own input settings. Done 2026-10-10 as
  `features/keybinds`: an "Editor tweaks" category of their own (2026-10-14; before, a section of
  the editor's), listed after the editor's, by section: selection (drag select, add to drag
  selection; 2026-10-14), fast editing (fast editing, use models, snap, next orientation), fast
  editing: lines (line point, slide along line, clear line), slow movement (slow, super slow),
  view (orthographic view, pan view), sliders (value scrubbing). Default keys may need modifier keys (`Ctrl+RightMouseButton`). Saved
  with the game's keys (`Input.ini` keeps only what differs from the defaults, so a new default key
  reaches players who didn't rebind it). Not rebindable: Shift/Ctrl during an Alt-drag (the game's
  own slider keys) and the resize tool's mouse buttons (the game's `Select` / `MoveCamera`,
  rebindable there, and swapped by the click swap setting).
- **Swap left and right click** in the editor: a setting (2026-10-14, `features/click_swap`), off
  by default. The game's selection keys were already rebindable (Brick editor > Selection), but
  binding Select to the right button left the camera on it, and "Move camera" is shared with the
  free camera and vehicles (on the left button it would fire weapons too). So the swap renames the
  two buttons as they reach the player's input (`UPlayerInput::InputKey`) while the player looks
  through the editor: the game's actions and the mod's keys follow, menus keep their clicks. See
  `docs/re/selection.md`. Untested in game yet.
- **Settings menu**: the mod's settings in the game's options. Started 2026-10-14 as
  `features/settings_menu` and `framework/settings`: an "Editor tweaks" button after the options'
  own (Gameplay, User interface, ...) opens a sub-menu like theirs (Back returns to the options),
  whose page is a settings page like the game's (the gameplay settings page asked for our category
  instead), with check boxes and lists by section: selection (swap left and right click, what a
  drag select box picks), fast editing snapping (each kind of point), turning to match (faces, edges, square up), lines
  (how long a line lasts), sliders (drag past slider ends). Saved as they change to
  `%LOCALAPPDATA%\BrickRigs\SavedRemastered\Config\WindowsNoEditor\EditorTweaks.ini`, read at
  start-up. A feature's setting is a `Toggle` or `Choice` static next to its code, listed in
  `features::SETTINGS`. See `docs/re/settings-menu.md`. Reworked the same day to make the
  sub-menu the way the game makes its own; works in game (button kept after Back, settings saved).
  Later: numbers
  (sliders), switching whole features off, the menu from the editor's own settings.
- **Precise moves**: no more 89.99999° and places a hair off after moving or turning groups of
  bricks. Started 2026-10-10 as `features/precise_moves`: every move is computed again in `f64`
  from the game's own inputs (stored places, the move's drag and turn as the game snaps them), only
  the result rounded to `f32`; no rounding to steps (an earlier version snapped to 0.01°/0.001 cm,
  dropped). The fold when a gizmo handle is released (`ApplyAccumulatedInput`) is kept in `f64`
  too (the first test still gave 89.99999° without it). Not yet: mirror mode counterparts (they
  stay as the game computes them) and mirroring a selection (`MirrorBrickEditorObject`).
- **More view modes**: started 2026-10-10 as `features/view_modes` (Z or the panel's list; colors
  in each mode's constants):
  - "Mirror": bricks with a mirror-mode counterpart green, their own mirror image (on the plane)
    green and gray, a mirror image mirror mode doesn't link (a property differs, or both on the
    plane) orange, the others gray.
  - "Duplicates": bricks with copies at their place (same type, place, rotation, size) red, deeper
    the more copies relative to the vehicle's most, the others gray.
  `game/editor_view` adds view modes and paints bricks with any color, so other view modes (e.g.
  the reference model distance below) can reuse it.
- **Collision view mode**: bricks' collision shapes ("hitboxes"). Done 2026-10-12 as the
  "Collision" view mode (`features/view_modes/collision`, see `docs/re/collision-shapes.md`):
  each brick's simple collision (boxes, spheres, capsules, convex hulls) as a solid mesh with the
  brick's own material, color and pattern, following it while moved and outlined when selected;
  the brick itself see-through (the game's hidden material), so a brick that is only see-through
  has no collision. In game: static meshes built at run time (procedural ones lost the custom
  primitive data), colors, patterns and outlines right. Wheels, spinners and flaps have materials
  that deform the mesh, so their shapes show a plain version of the material (the brick's
  parameters on the plain scalable brick's base material); 2026-10-13, untested in game yet.
  Ideas: the complex collision too, a key to switch, only the selection.
- **Drag to select**: box selection. Started 2026-10-14 as `features/drag_select`: in the editor's
  default mode, Ctrl + right drag (the "Drag select" key, rebindable) draws a box from where it was
  pressed; letting go selects the bricks it picks through the editor mode's own `SelectObjects`
  (replacing the selection, or adding with Shift, "Add to drag selection"). A setting picks the
  bricks it touches (default), those inside it, or those whose center is; bricks behind others
  count, hidden ones don't. First version (boxes and lines) worked in game. Then, at the user's
  request: bricks picked by their shape (what a click hits: their complex collision), shown while
  dragging with the game's own selection outline as letting go would leave them, and the box drawn
  with the game's UI in the UI style's highlight color, see-through inside (untested in game yet).
  See `docs/re/selection.md`. Ideas: only visible bricks (a setting), removing from the selection,
  the UI style's marquee border.
- **Selection "mesh quality"**: a setting to improve the "mesh quality" of selection (the user's
  idea, 2026-10-14, for later: to be specified).
- **Advanced resizing**:
  - Scale gizmo.
  - Push/extend the edges of scalable bricks. Started 2026-10-10 as `features/brick_resize`: in
    the editor (not moving), Alt shows the box of the brick under the mouse and the face, edge or
    corner the mouse is on (also just outside the box; selected bricks first, then the one in
    front); Alt + drag resizes the brick by it (the opposite part stays; a corner keeps
    proportions; the camera stays still), one undo step, mirror counterpart along.
  - Snap scalable brick faces and edges to other bricks' faces and edges: Ctrl while dragging
    (onto a face: proportions kept, to its plane; onto an edge or corner: to the point under the
    mouse); right clicks look deeper along the mouse for bricks behind. Letting go of Ctrl keeps
    snapping until the mouse moves 8 pixels away (`SnapKey`, 2026-10-11): letting go of it a moment
    before the button no longer unsnaps.
  - Fast editing while moving (2026-10-11, prototype, `brick_resize/motion.rs`): in the move with
    the mouse free (G twice, the gizmo showing), Alt + drag a moved brick's corner, edge or face:
    the bricks follow that point (in the plane across the vehicle axis the camera looks along the
    most, by the game's grid steps); Ctrl snaps it like resizing (Ctrl at the press grabs the
    brick's own snap point), and a face onto a face or an edge onto an edge also turns the bricks
    to match, then squares them with the target brick (each a boolean in `motion::MATCHING`). It
    gives the game's move its place, so the game moves the selection, mirror counterparts and gizmo
    and commits the move (one undo step). First in-game test: the bricks only moved at the commit
    (the flag asking `TickMode` to move them was misread, see `docs/re/editor-operations.md`);
    fixed, with the game's brick snapping kept off for such moves. Unsnapped again, the bricks keep
    the last snap's turn and slide on from where it put them; snapping onto a mirror counterpart
    meets it halfway across the mirror plane (it moves too). R (`keybinds::NEXT_ORIENTATION`, its
    own rebindable key) cycles the matching turns from the smallest: a face facing the target or
    level with it, an edge either way along, each squared up every way (8 for boxes); the choice is
    kept while the snap stays on the same pieces. Ideas: match while moving freely too, show the
    next orientation faintly, a vertex onto a face lying flat.
  - Moving about a line (2026-10-13, `brick_resize/line_tool.rs` and `hinge.rs`; players' ideas
    merged into one by the user): in the move with the mouse free, Alt + a click of the line key
    (`keybinds::LINE_POINT`, the right mouse button by default) places a point where snapping finds
    one (any brick, moved ones too); two make a purple line, shown while moving with the mouse
    free, kept until cleared (a click over no brick, or the clear key `keybinds::CLEAR_LINE`, none
    by default) or replaced (the "Line lasts" setting: or until the move ends, or for one drag).
    With Alt held the right button is the line key's (the camera started under it and captured the
    mouse: first in-game test, 2026-10-13, only P worked); it moves the camera again once Alt is let
    go. With a line, Alt + drag turns the moved bricks about it, the grabbed point going round it
    as near the mouse as it can (by the game's angle step), and Alt + a drag of the line key (the
    right button) slides them along it (by the grid step; also a drag with
    `keybinds::SLIDE_ALONG_LINE` held, no key by default: it was Shift, but Alt + Shift switches
    Windows' keyboard layout, second test 2026-10-14). Ctrl turns them to where the grabbed face or
    edge matches the target best (facing it or level with it, the one nearest the turn so far
    first, R the next; a face across the line squares up instead; a corner goes round to the
    target's side), or slides the grabbed point level with it. The game's gizmo isn't moved onto
    the line: its turns always pivot on the reference brick (see `docs/re/editor-operations.md`).
    Tested in game 2026-10-14 (points and turning fine; lines lasting until cleared felt long).
    Ideas: typed angles, several lines, a line along a picked edge in one click.
  - Smarter snapping (2026-10-11, `brick_resize/snap.rs`): like a CAD program's object snaps, the
    visible point of another brick nearest the mouse: corners/vertices, halfway along edges
    (thirds and quarters optional), faces' centers, where two bricks' edges cross, where an edge
    goes through another brick's face, halfway between those crossings along an edge; seams where
    two bricks go into each other (lines where their faces cross, found once per pair of bricks
    and kept while the drag lasts): halfway along them, or anywhere along them; else along an
    edge, else onto a face. Each kind has its own mark, and a dashed line shows what the
    dragged part lines up with. Each kind is a setting (the settings menu, 2026-10-14). Ideas: match another brick's size, round sizes, show the points available nearby.
  - The tool computes in `f64` in the vehicle's space from stored values (a scalable brick's box
    from its size), rounding only the results to `f32`: its first version worked in the world in
    `f32` and gave sizes like 110.000545 cm for 110.
  - Fast editing on models (2026-10-11): with the model key held (thumb mouse button by default),
    bricks' models replace their boxes: the mouse points at, drags and snaps to their faces, edges
    and vertices (a ramp's slope, a cylinder's rim). Models come from the meshes' complex collision
    (see `docs/re/brick-models.md`). Untested in game yet. Later: the same for moving bricks.
- **Reference images and models** (asked 2026-10-16, planned in `docs/model-io.md`; started
  2026-10-16: the `model_io` crate and an in-game prototype answering the probes; images first,
  then models): load PNG/JPG images and 3D models
  (glTF/GLB, OBJ, STL) from files and show them in the editor as non-brick previews that behave
  like bricks: moved, rotated, scaled, mirrored per axis, with an opacity, and a "selectable"
  switch in the side panel (refinement). Not editor objects (those are counted in mass, price and
  bounds and saved with the vehicle): components of the editor's own actor (the editor hides every
  other actor's, `docs/re/map-in-editor.md`), our own picking, the game's own selection outline
  (custom depth stencil), numbers in a "References" panel section first and an own gizmo later.
  - Remembered per vehicle in `<vehicle folder>/editortweaks.toml` (TOML, versioned, every field
    optional), each reference either a path (link) or a copy in `<vehicle folder>/editortweaks/`
    (a setting and a per-reference switch). The game ignores foreign files; Steam Cloud and the
    Workshop (only `Vehicle.brv` is in subscribed items) don't carry them, so they stay on this PC.
    The vehicle's folder comes from the editor's `FUGCFileInfo` (`game/editor_file.rs`), polled,
    not hooked.
  - Images: a plane (front/side/top presets, size, flip, opacity, draw order), best with the
    orthographic view. Texture through the engine's decoder (`ImportFileAsTexture2D`, works) on
    the engine's widget materials (`docs/re/references.md`).
  - Models: parsed on a worker thread into a neutral `ModelData`, built by `mesh_builder` in
    slices over frames (no hitch), with import settings for formats without fixed axes or units
    (up, forward, units, pivot). Rust crates (`gltf`, `tobj`, `stl_io`) rather than Assimp: no
    bloat, and no C++ parser reading downloaded files inside the game; Assimp stays an optional
    cargo feature if more formats are ever wanted.
  - FBX is not planned (decision 2026-10-16). Decided 2026-10-16: new references are links by
    default, references and bricks are never selected together, undo comes after the first
    prototype, and the map, images and models share one "References" panel section (the map
    is a reference too).
  - View mode showing the distance between the model and the bricks: built 2026-10-18 as the
    "Reference fit" view mode (`features/view_modes/fit`, see `docs/re/editor-view-modes.md`):
    smooth colors on a log scale, teal (0.01 cm) to dark red (1 km), no settings. Second round
    (2026-10-18): faster (threads, procedural meshes), no seams, spinners. Ideas for later if it
    reads badly: topographic lines (dark lines drawn into the gradient texture at each power of
    ten), and crisp lines where a brick crosses the model (needs a signed distance to cut
    triangles along the zero line). Drawing over the screen per pixel (the user's first idea)
    needs a shader comparing two depths: not possible without a cooked pak.
  - Later: own undo (wanted), mirror across the editor's mirror plane, live reload on file change, a
    Windows file dialog (a drop-in folder listed in the panel first).
- **Vehicle export as a 3D model** (asked 2026-10-16, deferred by the user until they have gathered
  more information; notes at the end of `docs/model-io.md`): write the vehicle (or the selection)
  as GLB, also OBJ and STL, from the bricks' models `game/brick_model` already reads (complex
  collision triangles: no UVs or normals). Known gaps: bricks with no readable model (spinners,
  wheels, flaps), the collision LOD against the render mesh.
- **Reference plumbing** (first step of the above, built 2026-10-16): the `crates/model_io` crate
  (data, readers, Unreal axes/units/winding, the image plane, the TOML sidecar; `cargo test -p
  model_io`); probes answered in game (`docs/re/references.md`). The framework's first part
  (2026-10-16): the open vehicle's `editortweaks.toml` read and shown, live as it or the files
  change (edited by hand for now). Next: writing it, the panel section with the map's rows,
  picking and the outline, numeric rows.
- **Map in the editor** (city covers, asked 2026-10-14): in the panel when nothing is selected, show
  the map around the vehicle (a toggle, later an opacity slider) and pick one of the map's garages,
  its spawn point marked with a 3-axis cross, so the map shows as it will be around the vehicle
  spawned there. Investigated 2026-10-14, built 2026-10-15 as `features/map_in_editor` (a "Map"
  section in the panel with a garage list, the "Pinned spawn point" setting and Show map, plus a
  show map key; no opacity yet), tested in game alone; in multiplayer since 2026-10-15 (the editor
  kept apart from the world while the map shows; the panel rows not tested yet): report
  `docs/city-cover-helper.md`,
  findings `docs/re/map-in-editor.md`. In short: the editor is an actor in the map's world (100 m
  above its origin) that hides every other actor each frame; a garage places the vehicle's bounding
  box centre on its spawn point. Plan: move the editor to where the vehicle will spawn and take the
  map back out of the hidden list, after a test version checks the risks (editor moved and turned,
  collision when hosting, the placement prediction).

## First technical milestones

1. Reflection dump: list the editor's classes, properties and functions by name from the running
   game. This is the map for everything else, and it's the update-resilient way in.
2. Property system: find where slider ranges and rounding live (brick property reflection,
   `FNumericBrickPropertyBase`, edit infos). Covers limits, precision, delta sliders, `x` macro.
3. Editor input and selection: how clicks, drags and the selection set work. Covers drag to
   select and is the base for new tools.
4. Rendering helpers: drawing lines/boxes/meshes in the editor viewport. Needed by the gizmo,
   snapping visuals and reference previews.
