# FULL FEATURE LIST OF EDITOR TWEAKS 0.1.1

## Coop Building
Host your editor session at any time, and let up to 31 other friends build with you
- Leaving the editor does not automatically disconnect you (both for members and the host) ;
- Players must get invited OR request access in order to join a session ;
- Can kick & ban players from the current editor session ;
- Has its own auto save behavior:
  - On top of vanilla behavior, the vehicle is auto saved when leaving the editor / game ;
  - Has its own auto save, which saves every time a session starts (in order to fix griefing).
- Shows each player's camera & selection live ;

## Fast Editing
Fast Editing is a powerful feature which is meant to make working with scalables and spinners way easier and faster.
- Extrude & resize bricks by using `ALT` and dragging faces/edges/corners in edit mode (Keybind: `Fast editing`) ;
  - Dragged faces show their size; press `Enter` to type it exactly (Keybind: `Type length`) ;
  - `Enter` no longer presses the last hovered button after using fast editing ;
- Snap fast editing operations of all kinds to other bricks using `CTRL` (Keybind: `Snap`) (customizable) ;
  - Snap points are configurable in Editor Tweaks' config.
- Move and rotate bricks easily and fast using Fast Editing in gizmo move mode ;
- Create lines by `Right clicking` twice in fast editing gizmo move mode (Keybind: `Line point`);
  - Rotate bricks around these lines using other parts ;
  - Rotation angle shows beside the grabbed point; press `Enter` to type it in degrees (Keybind: `Type length`) ;
  - Drag bricks along lines by holding right click ;
  - Shows lines' length, and the distance to the mouse while placing one ;
  - Press `Enter` to type a line's exact length, when one of its points is on the moved bricks (Keybind: `Type length`) ;
  - Full snapping support & switch rotations with `R` (Keybind: `Next orientation`).
- Snap against detailed brick models using `Thumb button` (Keybind: `Use models`) ;
- Modify spinners' shape using the curve tool ;
- Copy a brick's looks with `Middle Mouse Button` (Keybind: `Copy brick looks`) and a brick's full properties with `CTRL + Middle Mouse Button` (Keybind: `Copy brick properties`) ;
- Customizable behavior in Editor Tweaks' config.

## User Interface
- Slider property limits are increased to the highest reasonable value. *See APPENDIX 1* ;
- Out of vanilla bounds sliders / illegal values are highlighted / painted red respectively ;
- Sliders no longer hide precision (full value comes back if pressed). `Slider decimals when typing` caps the decimals shown ;
- Added a button to instantly switch values around for 2 numbers and 3 numbers properties. Pressing it repeatedly will cycle through all possible permutations ;
- Can now use `x` and `X` in sliders to use this brick's value and this slider's value respectively ;
- Use `ALT` to increase / decrease slider values over time (Keybind: `Value scrubbing`) ;
- All 3 scale axis are available for thruster bricks ;
- Many bricks whose names were identical to others were renamed ;
- Tells you at start when a newer version is out (`Check for updates` in the settings).

## Behavior Previews
- Preview guns' stats and trajectory when selected (configurable) ;
- Axles show suspension length and piston actuators show extension / contraction past their vanilla limits.

## References
- Export your vehicles into 3D models (glTF / OBJ / STL)
  - Can export at max quality, lower LODs and collision meshes.
- Can show any image and 3D model as reference in the editor ;
  - Can hide/unhide like bricks, and toggle visibility using `2` (Keybind: `Show references`) ;
  - Supports PNG, JPEG, glTF, GLB, OBJ, STL.
- Can show the map in editor (as reference, eg. for cover builders) ;
  - Select the garage, and if your creation is supposed to be placed pinned in the side panel.

## General changes to building
- Partially fixed "micro rotations", and they grow slower when unavoidable (won't fix existing micro rotations!) ;
- Increased editor size to 100km per axis *See APPENDIX 2* ;
- Can move bricks slower using `CTRL` (Keybind: `Slow`) and `SHIFT` (gizmo only) (Keybind: `Super slow`) ;
- Toggle between Perspective and Orthographic view using `1` (Keybind: `Orthographic view`) ;
  - Can pan around using `SHIFT Right click` (Keybind: `Pan view`) in orthographic view.
- Teleport the camera to a camera brick's view (unbound, Keybind: `Go to camera view`);
- Increased camera speed range ;
- Can disable lightning.

## Selection
- Drag select by selecting an area while holding `CTRL` (Keybind: `Drag select`) ;
- Added a setting to invert left and right click ;
- Studs are now colored red/green/blue for X/Y/Z ;
- Fixed innacurate stud locations on giant bricks.

## View modes
Many view modes were added & modified:
- `Physics`: Added Center of Lift, Thrust, Drag, and Lift & Drag force arrows *See APPENDIX 3* ;
- `Connections`: Color each group of connected bricks differently ;
- `Mirror`: Shows what Brick Rigs consider "mirrored". Green is mirrored, orange shows different properties, and gray dots means centered ;
- `Duplicates`: Highlights bricks with duplicates using shades of pink to red ;
- `Strength`: Stronger bricks appear dark blue ;
- `Collision`: Shows bricks' collisions ;
- `Reference fit`: See how accurate / inaccurate bricks are placed relative to all 3d models.

## Appendices
1. This unlocks slider limits. You can disable sliders going past their limits *while dragging* in the settings. Sliders are still limited to an extremely high value in order to avoid a crash with the User Interface. Some properties have custom rules: Light bricks are limited to 1,000% if they may emit light, and brick size is limited by how many connections it produces (→ reduce connector spacing for higher limits, remove all connections for unlimited). Some properties' slider limits (like color) are not unlocked because they are irrelevant.
2. A bug with similar mods exists, where snapping bricks at Z=0 crashes the game. Editor Tweaks fixes this bug.
3. Center of Lift has 2 points: a blue one for pitch and a green one for yaw. Additionally, when hovering over any center point, its coordinates are displayed. An "airflow" arrow was added, which you can move, in order to preview lift / drag forces depending on your speed and direction.
