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
- **Slower moves**: Ctrl (1/10) and Alt (1/100) slow down moving bricks with the mouse. Done
  2026-10-09 as `features/move_slowdown` (factors in its constants).
- **Drag to select**: box selection.
- **Advanced resizing**:
  - Scale gizmo.
  - Push/extend the edges of scalable bricks.
  - Snap scalable brick faces and edges to other bricks' faces and edges.
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
