# Roadmap

An outline of what the mod could become, not a spec. Ideas from the user (2026-10-08); notes on
likely approach are first guesses, to be revised as the editor gets reverse engineered.

## Ideas

- **Slider limits**: remove or raise them.
  - For brick size, replace the per-axis cap with a cap on connections. A 90 x 90 x 90 m cube is
    900 x 900 x 6 = 4.86 million connections.
- **Precise sliders**: sliders stop rounding values away (like BrickEdit Interface 2 does).
- **Delta sliders**: change a value relative to its current one, e.g. while holding Alt.
- **`x` macro in number inputs**: `x` stands for the previous value (`x*2`, `x+0.5`, ...).
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
