"""editortweaks dev tool. Run `python tools/et.py --help`.

Common loop:  et.py install  ->  launch the game  ->  et.py log  (and et.py crashes if it died)
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from etlib import crashes, paths, sigs, toolchain  # noqa: E402
from etlib.pe import PeImage  # noqa: E402

# SHA-256 of known loader builds (brickworks.dll) -> release name.
KNOWN_LOADERS = {
    "f27975ca33bd95bccb3972220b461fe1a4b313dd6de1d91e183f0d3e8f5c6198": "0.1.0 (supported)",
    "5d09c3a60a1e8dfa623ae15ab67b21ef20ab7b0c85996ed845eb90bb2f1a6997": "0.0.3 (too old: no hook API)",
}
# DLLs a mod may import that aren't guaranteed to exist next to the game.
UNSHIPPED_RUNTIME_DLLS = {"libgcc_s_seh-1.dll", "libwinpthread-1.dll", "libstdc++-6.dll"}


def game_running() -> bool:
    out = subprocess.run(
        ["tasklist", "/FO", "CSV", "/NH", "/FI", f"IMAGENAME eq {paths.GAME_EXE_NAME}"],
        capture_output=True, text=True,
    ).stdout
    return paths.GAME_EXE_NAME.lower() in out.lower()


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def profile_of(args: argparse.Namespace) -> str:
    return "debug" if getattr(args, "debug", False) else "release"


# --- commands -------------------------------------------------------------------------------


def cmd_setup(args: argparse.Namespace) -> None:
    toolchain.setup()


def cmd_build(args: argparse.Namespace) -> None:
    toolchain.cargo("build", *([] if args.debug else ["--release"]))
    print(f"Built {paths.build_output(profile_of(args))}")


def cmd_install(args: argparse.Namespace) -> None:
    game = paths.find_game()
    if game_running():
        raise SystemExit("The game is running; close it first (the DLL is locked while loaded).")
    if not args.no_build:
        cmd_build(args)
    dll = paths.build_output(profile_of(args))
    if not dll.exists():
        raise SystemExit(f"{dll} doesn't exist; build first.")

    missing = [d for d in PeImage(dll).imported_dlls() if d.lower() in UNSHIPPED_RUNTIME_DLLS]
    if missing:
        print(f"WARNING: the mod imports {', '.join(missing)}, which the game folder may not have.")
    loader = game.win64 / "brickworks.dll"
    if not loader.exists():
        print("WARNING: no loader in the game folder (xinput1_3.dll + brickworks.dll); the mod won't load.")
    elif not KNOWN_LOADERS.get(sha256(loader), "").endswith("(supported)"):
        print(f"WARNING: loader is {KNOWN_LOADERS.get(sha256(loader), 'an unknown build')}.")

    game.native_mods.mkdir(exist_ok=True)
    shutil.copy2(dll, game.installed_mod)
    print(f"Installed {game.installed_mod}")


def cmd_uninstall(args: argparse.Namespace) -> None:
    game = paths.find_game()
    if game_running():
        raise SystemExit("The game is running; close it first.")
    if game.installed_mod.exists():
        game.installed_mod.unlink()
        print(f"Removed {game.installed_mod}")
    else:
        print("Not installed.")


def cmd_status(args: argparse.Namespace) -> None:
    game = paths.find_game()
    manifest = paths.app_manifest()
    print(f"game folder   {game.root}")
    if manifest:
        branch = paths.manifest_field(manifest, "BetaKey") or "public"
        print(f"steam build   {paths.manifest_field(manifest, 'buildid')} (branch: {branch})")
    if game.exe.exists():
        mtime = time.strftime("%Y-%m-%d %H:%M", time.localtime(game.exe.stat().st_mtime))
        print(f"game exe      {game.exe.stat().st_size:,} bytes, modified {mtime}")
    loader = game.win64 / "brickworks.dll"
    if loader.exists():
        print(f"loader        {KNOWN_LOADERS.get(sha256(loader), 'unknown build ' + sha256(loader)[:16])}")
    else:
        print("loader        NOT INSTALLED")
    mods = sorted(p.name for p in game.native_mods.glob("*.dll")) if game.native_mods.exists() else []
    print(f"native mods   {', '.join(mods) or '(none)'}")
    built = paths.build_output()
    if game.installed_mod.exists() and built.exists():
        same = sha256(game.installed_mod) == sha256(built)
        print(f"editortweaks  installed, {'matches' if same else 'DIFFERS FROM'} the current release build")
    else:
        print(f"editortweaks  {'installed' if game.installed_mod.exists() else 'not installed'}")
    print(f"game running  {'yes' if game_running() else 'no'}")


def cmd_log(args: argparse.Namespace) -> None:
    log = paths.find_game().log
    if not log.exists():
        raise SystemExit(f"{log} doesn't exist yet (it's created when the game starts).")
    with open(log, encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
        for line in lines[-args.lines:] if args.lines else lines:
            print(line, end="")
        while args.follow:
            line = f.readline()
            if line:
                print(line, end="", flush=True)
            else:
                time.sleep(0.25)


def cmd_crashes(args: argparse.Namespace) -> None:
    found = crashes.recent(args.count)
    if not found:
        print(f"No crash reports in {crashes.CRASH_DIR}")
    for crash in found:
        print(f"{crash.time:%Y-%m-%d %H:%M:%S}  {crash.error}  (t+{crash.seconds_since_start}s)")
        print(f"  folder:  {crash.folder.name}")
        print(f"  frames:  {' <- '.join(crash.frames[: args.frames])}")
        extra = [m for m in crash.modules if "brickworks" in m.lower() or "\\brick rigs\\" not in m.lower()]
        if extra:
            print(f"  non-game modules: {', '.join(Path(m).name for m in extra)}")


def cmd_hang(args: argparse.Namespace) -> None:
    from etlib import hang

    pid = hang.find_pid(paths.GAME_EXE_NAME)
    if pid is None:
        raise SystemExit("The game isn't running: run this while it's frozen.")
    reports, mods = hang.sample(pid, args.samples, args.interval, args.frames, args.threads)
    names = hang.Symbolizer(mods)
    names.resolve_ours([a for r in reports for s in r.samples for a in [s.rip, *s.returns]])
    for report in reports:
        kind = "main (game) thread" if report.main else "busy thread"
        print(f"{kind} {report.tid}: {report.cpu_share:.0%} of a core")
        if not report.samples:
            print("  (couldn't sample it)")
            continue
        for index, taken in enumerate(report.samples):
            print(f"  sample {index + 1} at  {names.name(taken.rip)}")
        print("  stack (first sample, innermost first):")
        for address in report.samples[0].returns:
            print(f"    {names.name(address)}")
        print()
    print(
        "Busy (~100%) with samples in one function: an endless or very long loop there.\n"
        "Idle (~0%) in ntdll waits: a deadlock; look for the first editortweaks frame."
    )


def cmd_sigcheck(args: argparse.Namespace) -> None:
    exe = Path(args.exe) if args.exe else paths.find_game().exe
    roots = [paths.PROJECT_ROOT / "crates", paths.PROJECT_ROOT / "third_party"]
    signatures = sigs.find_signatures(roots)
    image = PeImage(exe)
    print(f"{len(signatures)} signatures vs {exe}\n")
    results = sigs.check(image, signatures)
    for r in results:
        targets = ", ".join(f"0x{t:X}" for t in dict.fromkeys(r.targets)) or "-"
        print(f"[{r.status:9}] {r.signature.name:34} {targets:24} {r.signature.source}")
    print(
        "\nMISSING: not found (a missing Offset32 signature crashes the game at startup)."
        "\nNOT-FUNC: a function lookup that doesn't land on a function start; calling it crashes."
        "\nAMBIGUOUS: several matches with different targets; the loader takes the first one."
        "\nSAME: several matches, all resolving to the same address (harmless)."
    )
    if any(r.status in ("MISSING", "NOT-FUNC") for r in results):
        raise SystemExit(1)


def cmd_find(args: argparse.Namespace) -> None:
    image = PeImage(Path(args.file) if args.file else paths.find_game().exe)
    signature = sigs.Signature("pattern", 0, "SignatureStart", args.pattern, "cli")
    exports = {rva: name for name, rva in image.exports().items()}
    matches = [m.start() for m in signature.regex().finditer(image.image)]
    for rva in matches[: args.limit]:
        start = image.function_containing(rva)
        where = f"in function 0x{start:X} +0x{rva - start:X}" if start is not None else "(no .pdata entry)"
        name = f"  {exports[start]}" if start in exports else ""
        print(f"0x{rva:08X} {image.section_of(rva):6} {where}{name}")
    print(f"{len(matches)} match(es)")


def cmd_xrefs(args: argparse.Namespace) -> None:
    """Direct calls/jumps (E8/E9 rel32) to an RVA, with the calling function."""
    import re
    import struct

    image = PeImage(Path(args.file) if args.file else paths.find_game().exe)
    exports = {rva: name for name, rva in image.exports().items()}
    target = int(args.rva, 16)
    found = 0
    for section in image.sections:
        if section.name != ".text":
            continue
        data = image.image[section.rva : section.rva + section.size]
        for m in re.finditer(rb"[\xE8\xE9]", data):
            at = section.rva + m.start()
            if at + 5 > section.rva + section.size:
                break
            if at + 5 + struct.unpack_from("<i", image.image, at + 1)[0] != target:
                continue
            start = image.function_containing(at)
            name = exports.get(start, "") if start is not None else ""
            kind = "call" if image.image[at] == 0xE8 else "jmp "
            where = f"0x{start:X}+0x{at - start:X}" if start is not None else "?"
            print(f"{kind} at 0x{at:08X}  in {where}  {name}")
            found += 1
    print(f"{found} reference(s)")


def cmd_vtable(args: argparse.Namespace) -> None:
    """Entries of a vtable (array of absolute code pointers) at an RVA."""
    import struct

    image = PeImage(Path(args.file) if args.file else paths.find_game().exe)
    exports = {rva: name for name, rva in image.exports().items()}
    rva = int(args.rva, 16)
    for i in range(args.count):
        pointer = struct.unpack_from("<Q", image.image, rva + 8 * i)[0]
        target = pointer - image.image_base
        if not 0 <= target < len(image.image) or not image.section_of(target).startswith(".text"):
            print(f"  [{i:2}] +0x{8 * i:03X}  0x{pointer:X} (not code: end of vtable?)")
            break
        print(f"  [{i:2}] +0x{8 * i:03X}  0x{target:08X}  {exports.get(target, '')}")


def cmd_exports(args: argparse.Namespace) -> None:
    image = PeImage(Path(args.dll))
    exports = image.exports()
    shown = [(name, rva) for name, rva in sorted(exports.items()) if args.filter.lower() in name.lower()]
    for name, rva in shown:
        print(f"0x{rva:08X}  {name}")
    print(f"\n{len(shown)} of {len(exports)} exports; imports: {', '.join(image.imported_dlls())}")


def cmd_disasm(args: argparse.Namespace) -> None:
    try:
        from etlib.disasm import Disassembler
    except ImportError:
        raise SystemExit("capstone missing: run this with .venv\\Scripts\\python.exe (see et.py setup)")
    image = PeImage(Path(args.file) if args.file else paths.find_game().exe)
    disassembler = Disassembler(image)
    target = args.target
    if not all(c in "0123456789abcdefABCDEF" for c in target.removeprefix("0x")):
        # An export name (or unique substring of one), e.g. "SetValueInternal@UBrickSliderWidget".
        matches = {name: rva for name, rva in image.exports().items() if target in name}
        if len(matches) != 1:
            raise SystemExit(f"{len(matches)} exports match {target!r}: {', '.join(list(matches)[:10])}")
        (name, rva), = matches.items()
        print(name)
    else:
        rva = int(target, 16)
    for line in disassembler.lines(rva, args.count):
        print(line)


# --- CLI ------------------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(prog="et.py", description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("setup", help="install the Rust toolchain, MinGW and the Python venv").set_defaults(func=cmd_setup)

    p = sub.add_parser("build", help="build the mod DLL")
    p.add_argument("--debug", action="store_true", help="debug profile instead of release")
    p.set_defaults(func=cmd_build)

    p = sub.add_parser("install", help="build and copy the DLL into <game>/brickworks/")
    p.add_argument("--debug", action="store_true", help="install the debug build")
    p.add_argument("--no-build", action="store_true", help="install the existing build as-is")
    p.set_defaults(func=cmd_install)

    sub.add_parser("uninstall", help="remove the DLL from the game").set_defaults(func=cmd_uninstall)
    sub.add_parser("status", help="game, loader and install state").set_defaults(func=cmd_status)

    p = sub.add_parser("log", help="print brickworks.txt (the loader + mod log)")
    p.add_argument("-n", "--lines", type=int, default=0, help="only the last N lines")
    p.add_argument("-f", "--follow", action="store_true", help="keep printing new lines")
    p.set_defaults(func=cmd_log)

    p = sub.add_parser("crashes", help="summarize recent game crash reports")
    p.add_argument("-n", "--count", type=int, default=3)
    p.add_argument("--frames", type=int, default=8, help="call stack frames to show")
    p.set_defaults(func=cmd_crashes)

    p = sub.add_parser("hang", help="while the game is frozen: where its threads are stuck")
    p.add_argument("-s", "--samples", type=int, default=5, help="samples per thread")
    p.add_argument("--interval", type=float, default=0.2, help="seconds between samples")
    p.add_argument("--frames", type=int, default=24, help="stack frames to show")
    p.add_argument("--threads", type=int, default=1, help="busiest other threads to sample too")
    p.set_defaults(func=cmd_hang)

    p = sub.add_parser("sigcheck", help="check all byte signatures against the game exe")
    p.add_argument("--exe", help="exe to check (default: the installed game)")
    p.set_defaults(func=cmd_sigcheck)

    p = sub.add_parser("find", help="search a byte pattern; shows each match's enclosing function")
    p.add_argument("pattern", help='hex bytes, ?? for wildcards, e.g. "48 8b ?? 0f 2f"')
    p.add_argument("--file", help="PE file (default: the game exe)")
    p.add_argument("--limit", type=int, default=20)
    p.set_defaults(func=cmd_find)

    p = sub.add_parser("xrefs", help="direct calls/jumps to an RVA, with their functions")
    p.add_argument("rva", help="hex RVA of the called function")
    p.add_argument("--file", help="PE file (default: the game exe)")
    p.set_defaults(func=cmd_xrefs)

    p = sub.add_parser("vtable", help="list the function pointers of a vtable")
    p.add_argument("rva", help="hex RVA of the vtable")
    p.add_argument("-n", "--count", type=int, default=40)
    p.add_argument("--file", help="PE file (default: the game exe)")
    p.set_defaults(func=cmd_vtable)

    p = sub.add_parser("exports", help="list a DLL's exports (e.g. BRMK modules)")
    p.add_argument("dll")
    p.add_argument("filter", nargs="?", default="")
    p.set_defaults(func=cmd_exports)

    p = sub.add_parser("disasm", help="annotated disassembly at an RVA or export (needs the venv)")
    p.add_argument("target", help="hex RVA (e.g. 136C0D0) or a unique substring of an export name")
    p.add_argument("-n", "--count", type=int, default=20, help="instructions")
    p.add_argument("--file", help="PE file (default: the game exe)")
    p.set_defaults(func=cmd_disasm)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
