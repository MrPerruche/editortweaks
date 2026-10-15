# City cover helper: investigation report

2026-10-14. Whether the editor can show the map and the map's garages, to help build "city covers"
(vehicles covering the map, spawned at a set garage so they line up), and how to build it. The game
findings behind it, with addresses and how they were found, are in `docs/re/map-in-editor.md`.

## The ask

In the editor's side panel, when nothing is selected:

- a toggle (or an opacity slider, 0 meaning off) to show the map around the vehicle;
- a slider stepping through the map's garages, marking the selected garage's spawn point with a
  3-axis cross, so builders can line their vehicle up with the garage it must be spawned at.

## Verdict

It's possible, through what the game already does:

- The garages and the exact place a vehicle lands at one can be worked out from the game's
  reflected data and its own functions.
- The controls fit the panel through the hook that already adds our settings menu.
- The cross is the line drawing `brick_resize` already uses.
- Showing the map solid, with the vehicle in front of or behind buildings as it will really be, is
  possible by moving the editor into the map and asking the game to stop hiding the map. That has
  risks to check first, in a short test version.
- A real opacity slider (a see-through map) is the costly part: it needs a second render of the
  view. The report recommends a toggle first and opacity later.

## What the game does today

**The editor is in the map's world.** It is an actor spawned 100 m above the map's origin,
unrotated. Bricks, camera, gizmo, grid and the build-area box are all placed relative to it.

**The map is only hidden.** Every frame, while the vehicle editor is open, the player controller
lists every component of every other actor (the map, characters, vehicles) as hidden for the
player's view. Nothing is unloaded, and the map's collision stays where it is.

**A garage places the vehicle's bounding box, not its origin.**

- The garage has a spawn point (from its reflected `VehicleSpawnOffset` and `VehicleSpawnRotation`,
  or `PinnedVehicleSpawnOrigin` when spawned pinned) and a spawn box (`VehicleSpawnExtent`).
- The game takes the box around all the vehicle's bricks and puts its centre on the spawn point,
  rotated like the garage.
- If the vehicle is smaller than the garage's box, it is pushed back against the box's back and
  floor (still centred sideways).
- With bricks placed far out, a city cover is bigger than the box on every axis, so its box centre
  lands exactly on the spawn point. That is why the trick lines up reliably.
- The game also checks for things in the way (other vehicles, characters) and may shift the
  vehicle sideways. Buildings and terrain are ignored at a garage.

So, in the editor:

- The selected garage's spawn point sits at the centre of the vehicle's bounding box (moved back
  and down when the vehicle is smaller than the garage's box), along the editor's own axes.
- The map around it is the garage's surroundings, turned so the garage's forward is the editor's
  +X.
- Adding a brick that grows the box moves that point, exactly as it would move the spawned vehicle.

## Proposed design

### Controls

A "Map" section in the editor panel when nothing is selected (the game lists its editor settings
there; `settings_menu`'s hook can add our rows to that list):

| Control | What | Default |
|---|---|---|
| Garage | none, or one of the map's garages | the garage the editor was entered from (the controller's reflected `EditorEntryPoint`) |
| Spawned pinned | which spawn point of the garage to use | follow the vehicle browser's spawn option if it is readable, else on |
| Show map | off / on (later: an opacity slider, 0 = off) | off |

Plus rebindable keys for next and previous garage (memory "tunables-and-keys").

- Our panel rows today are check boxes and lists (`game/property_reflection.rs`). A list of
  garages ("Garage 3 of 12", the entry garage marked) works with that now.
- A slider row like the user described, or an opacity slider, needs a numeric row type: a copy of
  the game's float property table, found by its type name like the bool and list ones.

### Finding the garages and the spawn point

- The garages: the game state's reflected `SpawnPointArray`, keeping `AGarage`s.
- The spawn point and box: the game's own `GetVehicleSpawnPoint(controller, pinned)` and
  `GetVehicleSpawnBounds`, through the garage's spawn-point interface. The game does the maths,
  including any garage type that overrides it.
- The vehicle's box: the same union of per-brick boxes the game uses (`CalcBrickEditorObjectsBounds`),
  from reflected per-brick functions we already call (`LocalBounds`, spawn transforms).
- The placement formula (about 15 lines): redone in Rust from `GetVehicleSpawnOffset`, since it
  needs a spawned vehicle in the game.

This gives the transform the vehicle will have in the world, and the garage's spawn point in the
editor. It is worked out again when the garage, the pinned option or the vehicle's box changes
(after an edit, not during a drag).

### The cross

With a garage selected, drawn with the line batcher (`game/line_batcher.rs`) whether or not the map
shows:

- three coloured axes at the spawn point (forward, right, up);
- the garage's spawn box outline;
- optionally the vehicle's bounding box, so it is clear what gets centred on the cross.

### Showing the map: move the editor into place (recommended)

1. **Move the editor actor** to where the vehicle will spawn: the reflected
   `K2_SetActorLocationAndRotation`, teleporting. Bricks, camera, gizmo and grid follow, since they
   are relative to it. The view doesn't jump, but the map appears around the vehicle exactly as it
   will be once spawned, buildings in front of bricks and bricks in front of buildings.
2. **Un-hide the map.** Hook the game's `UpdateHiddenComponents`. After the game has listed
   everything as hidden, take the map's components back out of the hidden set:
   - Which components: those of actors that were loaded with the map (not spawned since), plus the
     levels' BSP. They are listed once and refreshed when the toggle turns on or levels stream in.
   - Characters, vehicles and debris stay hidden, including the player's own character standing at
     the garage.
   - Taking ids out of the set is about 40 lines on the engine's `TSet` layout. The components' id
     offset can be learnt at run time, by finding which field of a map component holds a value the
     game put in the set (like `SpawnTransforms::learn`).
3. **Toggle off, or no garage:** stop un-hiding and put the editor back at its usual place.
   Otherwise the invisible map collision around the moved editor would block clicks.

Upsides:

- Real rendering at no extra cost (the game renders the map like in play).
- Correct depth.
- It works with the orthographic view: a top view lines roads up exactly.

Downsides are the risks below.

### Alternative: a picture of the map (no editor move)

A scene capture renders the map from the camera's place carried into the map (same garage
transform), and the editor shows it as a see-through image over the view. It is a UMG image on the
canvas `view_canvas` already uses, with the slider as its opacity.

- Upsides: it never touches the editor, collision or the hidden list, and opacity comes free.
- Downsides:
  - no depth: the map is drawn over the vehicle, like a tracing paper;
  - a second full render every frame;
  - a softer image (captures lack the view's anti-aliasing history);
  - the sky and lighting may not match.
- It is good enough in top and side orthographic views, poor in perspective.

This is the fallback if moving the editor turns out unsafe.

### Opacity

The game can't make the map's own materials see-through: a shipping game can't compile new
materials, and a cooked material would mean a content mod that breaks with game updates.

What can be done after the solid map works:

- Render the vehicle-only view (what the editor shows today) a second time with a scene capture.
- Draw it over the solid view at 1 minus the opacity.

At 0 the map is gone, at 1 it is solid, in between it fades, and it stays correct in depth. The
cost is a second, cheap render (only the vehicle). Its quality (shimmer, exposure) needs a test.
Until then, "Show map" is a toggle.

## Risks, and the test version to check them

A first version with no panel would show these quickly. A debug key moves the editor to the entry
garage and un-hides the map, and the log prints the predicted vehicle transform.

1. **The editor moved and turned.**
   - Does placing, moving, the gizmo, mirror mode, undo, focus and the camera still work?
   - Do our own features still work? The orthographic view, drag select, resize and precise moves
     may assume an unrotated editor somewhere: they need an audit.
   - The game never moves its editor, so this is untested ground.
2. **Collision with the world.** If editor bricks block characters or vehicles physically (not only
   clicks), moving them into the map could push the local character. Worse, when hosting, it could
   push other players' vehicles, which breaks the "client side only" rule.
   - Check an editor brick's collision setting (reflected).
   - If it isn't query-only: refuse to move the editor when hosting, or use the picture approach.
3. **Clicks hitting the map.** With the map shown, a click goes to what you see, so a brick under
   a road surface can't be clicked through it. That is probably fine; if not, the editor's trace
   could be made to ignore the map's components.
4. **The prediction.** Compare the predicted transform with a vehicle really spawned at a few
   garages:
   - a small vehicle (pushed to the box's back and floor);
   - a city cover (centred);
   - pinned and not pinned.

   The game computes the offset in two passes whose interplay isn't untangled yet.
5. **Thumbnails.** When saving, check the vehicle's thumbnail doesn't show the map; if it does,
   hide the map during the capture.
6. **Speed.**
   - The game already walks every actor every frame in the editor. Taking map ids back out adds a
     hash lookup each, well under a millisecond for tens of thousands.
   - Moving a 10 000-brick vehicle when its box changes may cost tens of milliseconds, once per
     change.

## Update resilience

| Relies on | Kind | If an update changes it |
|---|---|---|
| Garage list (`SpawnPointArray`), garage and controller fields (`EditorEntryPoint`, `BrickEditor`) | reflection by name | survives anything but renames |
| Moving the editor (`K2_SetActorLocationAndRotation`), per-brick bounds | reflection | same |
| `GetVehicleSpawnPoint` / `GetVehicleSpawnBounds` | game interface table slots (+0x48, +0x50) | only if the devs add functions to that interface; checked at load (the slots must be code, the bounds' alignment within -1..1) |
| The placement formula | redone from the game's code | silently off if the devs change it; the log's prediction check (risk 4) catches it, and it is worth redoing after big updates |
| `UpdateHiddenComponents` | an engine virtual, found by a unique signature (`UpdateHiddenActors`, right before it) or by its table slot | signature checked at load (`sigcheck`); on failure only "Show map" is disabled |
| The hidden set's layout, component ids | engine (UE 4.27.2) | stable unless the game changes engine version; the id offset is learnt, not hard-coded |
| Panel rows | our existing `settings_menu` hooks | already maintained for the settings menu |

Every part disables itself alone when what it needs is missing (the mod's usual rule): no map
without the hiding hook, no prediction without the garage interface, the cross and garage list
still working.

Purely client side: the vehicle file is unchanged, the game's spawning isn't touched, and moving
the editor only moves the local editor actor (risk 2 is the one to confirm).

## Phases

1. **Test version** (risks 1 to 5): debug key, editor move, un-hiding, logged prediction. About a
   session of work plus one in-game test.
2. **The feature**:
   - garage list and pinned option in the panel, plus keys;
   - the cross;
   - "Show map" as a toggle.
3. **Opacity**: the numeric row type, then the faded view (or the picture approach if the editor
   move was ruled out).
