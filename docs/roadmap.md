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
- **Slower moves**: Ctrl slows down moving bricks and turning the camera with the mouse, Shift
  moving bricks by a gizmo handle. Done 2026-10-09 as `features/move_slowdown` (factors in its
  constants). Shift replaced Alt (2026-10-10, Alt is the resize tool's key); it leaves the camera
  and G/dragged moves alone (there it's the game's "move perpendicular").
- **Rebindable keys**: the mod's keys in the game's own input settings. Done 2026-10-10 as
  `features/keybinds`: the editor's category gets an "Editor tweaks" section (fast editing, on
  models, fast editing snapping, slow moves, slow gizmo moves, keep dragging sliders), saved with the
  game's keys. Not
  rebindable: Shift/Ctrl during an Alt-drag (the game's own slider keys) and the resize tool's mouse
  buttons (the game's `Select` / `MoveCamera`, rebindable there).
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
- **Collision view mode**: a view mode showing bricks' collision shapes ("hitboxes") as lines,
  bricks see-through. Looked into 2026-10-12 (`docs/re/collision-shapes.md`): feasible by
  reflection (`AggGeom` of the brick's body setup, scaled like its mesh) with the existing line
  batcher and view mode code; not started.
- **Drag to select**: box selection.
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
  - Smarter snapping (2026-10-11, `brick_resize/snap.rs`): like a CAD program's object snaps, the
    visible point of another brick nearest the mouse: corners/vertices, halfway along edges
    (thirds and quarters optional), faces' centers, where two bricks' edges cross, where an edge
    goes through another brick's face, halfway between those crossings along an edge; seams where
    two bricks go into each other (lines where their faces cross, found once per pair of bricks
    and kept while the drag lasts): halfway along them, or anywhere along them; else along an
    edge, else onto a face. Each kind has its own mark, and a dashed line shows what the
    dragged part lines up with. Each kind is a boolean in `snap::SNAPPING`, for a settings menu
    later. Ideas: match another brick's size, round sizes, show the points available nearby.
  - The tool computes in `f64` in the vehicle's space from stored values (a scalable brick's box
    from its size), rounding only the results to `f32`: its first version worked in the world in
    `f32` and gave sizes like 110.000545 cm for 110.
  - Fast editing on models (2026-10-11): with the model key held (thumb mouse button by default),
    bricks' models replace their boxes: the mouse points at, drags and snaps to their faces, edges
    and vertices (a ramp's slope, a cylinder's rim). Models come from the meshes' complex collision
    (see `docs/re/brick-models.md`). Untested in game yet. Later: the same for moving bricks.
- **Reference previews**: import 3D models and images as non-brick previews.
  - View mode showing the distance between the model and the bricks; opacity setting.

## First technical milestones

1. Reflection dump: list the editor's classes, properties and functions by name from the running
   game. This is the map for everything else, and it's the update-resilient way in.
2. Property system: find where slider ranges and rounding live (brick property reflection,
   `FNumericBrickPropertyBase`, edit infos). Covers limits, precision, delta sliders, `x` macro.
3. Editor input and selection: how clicks, drags and the selection set work. Covers drag to
   select and is the base for new tools.
4. Rendering helpers: drawing lines/boxes/meshes in the editor viewport. Needed by the gizmo,
   snapping visuals and reference previews.
