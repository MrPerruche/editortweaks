# Editor Tweaks

A mod for Brick Rigs that extends the vehicle editor.

## Highlights

- Build with up to 31 friends using coop building
- Extrude, move, rotate and edit bricks in an instant using fast editing
- Import images and 3D models as references, and export your vehicles as 3D models
- Better sliders with close to no limits and higher precision
- New and improved previews of brick behaviors (guns etc.)
- Drag select, colored studs and more
- 6 new view modes
- Editor limits increased to 200km x 200km
- Highly configurable, fully remappable and removable keybinds
- [And so much more](https://cryptpad.fr/code/#/2/code/view/dc0tXSDCH7+wuSdAmFhmBYtYYZhiCyR8UC7MbWmUlk8/)

## Install

Editor Tweaks is a BrickRust mod, so it needs the brickworks loader.

1. Install [brickworks](https://github.com/KoT3isGood/brickrust/releases/tag/v0.1.0) **0.1.0**
   (ignore the repo being named brickrust instead of brickworks).
2. Download the [latest Editor Tweaks release](https://github.com/MrPerruche/editortweaks/releases/latest).
3. Follow the [installation tutorial](https://TODO).

## License

GNU GPL v3 or (at your option) any later version, see `LICENSE`. `third_party/brickrust/` is a
modified copy of [brickrust](https://github.com/KoT3isGood/brickrust) and stays under its own MIT
license (`third_party/brickrust/LICENSE`).

---

## Development

Written in Rust as a Windows DLL on a patched copy of brickrust. 
Generative AI was used (responsibly!) to make this project.
Notes for contributors:

### Setup

```
python tools/et.py setup    # Rust toolchain, project-local MinGW, Python venv
```

Install brickworks 0.1.0 into the game first (see above), copying `xinput1_3.dll` and
`brickworks.dll` to `steamapps/common/Brick Rigs/BrickRigs/Binaries/Win64`.

### Everyday use

```
.venv\Scripts\python.exe tools/et.py install    # build + copy into <game>/brickworks/
                                                # then launch the game
.venv\Scripts\python.exe tools/et.py log        # what the loader and the mod printed
.venv\Scripts\python.exe tools/et.py crashes    # if the game crashed
.venv\Scripts\python.exe tools/et.py status     # what's installed where
.venv\Scripts\python.exe tools/et.py package --expires 14d   # a zip for a tester
```

- `--expires` (on `build`, `install` and `package`) makes a test build: past that local date and
  time the mod doesn't start (it logs why in `brickworks.txt`). It goes by the PC's clock.
- `--coop-test` (on the same commands) adds coop's test-only tools (a local test with two game
  instances on one PC, a Steam self-send probe) and makes `package` a test zip with the friend
  test steps. Notes: `docs/re/coop/`, UI: `docs/coop-ui.md`.
- `python tools/et.py --help` lists everything. To disable the mod without uninstalling, rename
  `<game>/brickworks/editortweaks.dll` to `_editortweaks.dll`.
- After a game update, run `et.py sigcheck` before launching: it checks every byte pattern the mod
  relies on against the new game exe.

### Project layout

| Path | Contents |
|---|---|
| `crates/editortweaks/` | The mod. Each feature is one file in `src/features/`. |
| `third_party/brickrust/` | Patched copy of the brickrust SDK (see `VENDORED.md`). |
| `tools/` | `et.py`, the build/install/debug helper. |
| `docs/` | Roadmap and reverse-engineering notes. |
