# Vendored brickrust

- Upstream: https://github.com/KoT3isGood/brickrust (MIT, see `LICENSE`)
- Commit: `994add65880e2057c141968c6700942f8f9020f1` (2026-09-23, after tag `v0.1.1`)
- Runtime loader this is used with: the official **0.1.0** release binaries
  (`xinput1_3.dll` + `brickworks.dll`). Every loader function the mod imports exists in 0.1.0;
  the newer `brickworks_get_base_*` exports are declared but never called.

## What was copied

The library crates only: the root crate (`src/`), `brickworks/` (the mod-side half of the loader
API) and `macros/`. `examples/` is kept as reference but not built (`autoexamples = false`).
Not copied: `brickworks_impl/` (loader implementation), `xinput_proxy/`, `brmk_plugin/`, build
scripts, and upstream's agent instruction files.

## Local patches

Every patch is marked with an `editortweaks patch` comment.

1. `Cargo.toml`: removed `[workspace]`, profiles, `[[example]]` entries and the unused
   dependencies `brickworks_impl`, `min_hook_rs`, `wchar`.
2. `brickworks/Cargo.toml`: removed `libloading` and `min_hook_rs` (loader-only).
3. `brickworks/build.rs`: deleted. It linked against `libbrickworks.dll.a`, which upstream only
   gets as a by-product of building the loader in the same target dir (fails on a clean build).
4. `brickworks/src/iface.rs`, `brickworks/src/patterns.rs`: the extern blocks importing loader
   functions use `#[link(name = "brickworks.dll", kind = "raw-dylib", modifiers = "+verbatim")]`
   instead, so no import library is needed.
5. `src/lib.rs`, `brickworks/src/lib.rs`, `macros/src/lib.rs`: `#![allow(warnings)]`.
6. `src/br/statics.rs`: `GetProjectVersion_ptr` offset `0x2B` -> `-0x2B`. The pattern is inside
   the function, 0x2B bytes after its start; the upstream value jumped into the next function.
7. `src/ue/ffield.rs`: `FFieldVariant::as_uobject()`, read access to a field's owner (upstream
   keeps both fields private).
8. `src/ue/fframe.rs`: `FFrame::out_params` moved from 0x60 to 0x78 (UE 4.27's layout, checked
   against the game's `FFrame::StepExplicitProperty`); upstream's `set_output` wrote nowhere useful.

## Updating

Copy the same set of files from the new upstream commit over this directory, re-apply the patches
above (search for `editortweaks patch` in the old copy), update the commit hash here, rebuild, and
run `python tools/et.py sigcheck` against the current game build.
