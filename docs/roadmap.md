# Roadmap

An outline of what the mod could become, not a spec. Ideas from the user (2026-10-08); notes on
likely approach are first guesses, to be revised as the editor gets reverse engineered.

## Ideas

- **Slider limits**: remove or raise them. Started 2026-10-09 as `features/property_limits`:
  no limits by default, per-property rules in `rules.rs`, brick size capped by a budget of
  connector points (the game's largest brick has 4.86 million). Limits depending on other
  properties (brick size on connector spacing, light brightness on direction) are re-checked when
  those change (`dependencies.rs`).
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
- **Camera speed**: exponential speed steps. Done 2026-10-09 as `features/camera_speed`
  (slowest, fastest and step count in its constants).
- **Slower moves**: Ctrl slows down moving bricks and turning the camera with the mouse, Shift
  moving bricks by a gizmo handle. Done 2026-10-09 as `features/move_slowdown` (factors in its
  constants). Shift replaced Alt (2026-10-10, Alt is the resize tool's key); it leaves the camera
  and G/dragged moves alone (there it's the game's "move perpendicular").
- **Rebindable keys**: the mod's keys in the game's own input settings. Done 2026-10-10 as
  `features/keybinds`: the editor's category gets an "Editor tweaks" section (resize bricks, snap
  resizing, slow moves, slow gizmo moves, keep dragging sliders), saved with the game's keys. Not
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
    mouse); right clicks look deeper along the mouse for bricks behind. Next: the same while moving
    bricks (see `docs/re/editor-input.md`).
  - The tool computes in `f64` in the vehicle's space from stored values (a scalable brick's box
    from its size), rounding only the results to `f32`: its first version worked in the world in
    `f32` and gave sizes like 110.000545 cm for 110.
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
