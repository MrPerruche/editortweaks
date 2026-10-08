# Reverse-engineering notes

Findings about the game that the mod relies on. Record how something was found, not just the
result, so it can be found again after an update without BRMK.

Conventions:
- Addresses are RVAs in `BrickRigsSteam-Win64-Shipping.exe` for the build named next to them.
  They change with every build; names, signatures and the method used to find them don't.
- Prefer reflection names over addresses (see "Update resilience" in `CLAUDE.md`).

## Game builds

| Steam build | Date | Exe size | `sigcheck` | Notes |
|---|---|---|---|---|
| 25633865 (public) | 2026-10-08 | 81,198,080 | 16 OK, 1 SAME, 2 AMBIGUOUS, 0 MISSING | First build used. UE 4.27.2. Upstream `GetProjectVersion` offset fixed (vendored patch 6). |

Lesson from `GetProjectVersion` (crashed the game on first launch): its pattern is inside the
function and upstream applied the offset in the wrong direction, landing mid-instruction in the next
function. `sigcheck` now reports `NOT-FUNC` when a function lookup doesn't land on a 16-byte-aligned
address preceded by padding/`ret`.

Ambiguous upstream signatures on 25633865 (the loader takes the first match):
- `IsModdedAsset_ptr`: matches 0xD97E00 and 0x2EEC140. Unverified which is right.
- `NUMERIC_BRICK_PROPERTY_FLOAT_VTABLE`: three different targets. Unverified.
Neither is used by editortweaks yet; anything building on them needs a better signature first.

## Engine (UE 4.27.2)

Resolved by brickrust signatures on build 25633865: `ProcessInternal` (Blueprint VM) 0x136C0D0,
`StaticConstructObject_Internal` 0x1372FE0, `UEngine::Init` 0x2999DF0, `UEngine::LoadMap`
0x299F3A0, `GUObjectArray` 0x4A5D5F0, `FNamePool` 0x4A21180, `GWorld` 0x4BA0DF8.
