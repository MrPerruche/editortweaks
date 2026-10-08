# editortweaks

A native mod for Brick Rigs that improves the vehicle editor. It's a Windows DLL written in Rust,
loaded by the [brickworks](https://github.com/KoT3isGood/brickrust) mod loader.

(TODO if this mod gets released: add ai usage section)

## One-time setup

1. Install the brickworks loader **0.1.0** into the game: copy `xinput1_3.dll` and
   `brickworks.dll` from the [release zip](https://github.com/KoT3isGood/brickrust/releases/tag/0.1.0)
   to `steamapps/common/Brick Rigs/BrickRigs/Binaries/Win64`.
2. Install the build tools (Rust toolchain, a project-local MinGW, the Python venv):

   ```
   python tools/et.py setup
   ```

## Everyday use

```
.venv\Scripts\python.exe tools/et.py install    # build + copy into <game>/brickworks/
                                                # then launch the game
.venv\Scripts\python.exe tools/et.py log        # what the loader and the mod printed
.venv\Scripts\python.exe tools/et.py crashes    # if the game crashed
.venv\Scripts\python.exe tools/et.py status     # what's installed where
```

`python tools/et.py --help` lists everything. To disable the mod without uninstalling, rename
`<game>/brickworks/editortweaks.dll` to `_editortweaks.dll`.

After a game update, run `et.py sigcheck` before launching: it checks every byte pattern the mod
relies on against the new game exe.

## Project layout

| Path | Contents |
|---|---|
| `crates/editortweaks/` | The mod. Each feature is one file in `src/features/`. |
| `third_party/brickrust/` | Patched copy of the brickrust SDK (see `VENDORED.md`). |
| `tools/` | `et.py`, the build/install/debug helper. |
| `docs/` | Roadmap and reverse-engineering notes. |

## License

GNU GPL v3 or (at your option) any later version, see `LICENSE`. `third_party/brickrust/` is a modified copy of
[brickrust](https://github.com/KoT3isGood/brickrust) and stays under its own MIT license
(`third_party/brickrust/LICENSE`).
