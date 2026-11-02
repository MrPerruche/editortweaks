"""editortweaks dev tool. Run `python tools/et.py --help`.

Common loop:  et.py install  ->  launch the game  ->  et.py log  (and et.py crashes if it died)
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from datetime import datetime, timedelta, timezone
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


def parse_expiry(text: str) -> datetime:
    """`--expires`: a local date and time (`2026-11-15 18:00`, `2026-11-15T18:00`), a date (its
    midnight, as it starts), or a time from now (`7d`, `36h`)."""
    relative = re.fullmatch(r"(\d+)([dh])", text.strip())
    if relative:
        amount, unit = int(relative.group(1)), relative.group(2)
        moment = datetime.now() + (timedelta(days=amount) if unit == "d" else timedelta(hours=amount))
        return moment.replace(second=0, microsecond=0)
    try:
        moment = datetime.fromisoformat(text.strip())
    except ValueError:
        raise argparse.ArgumentTypeError(f"{text!r}: use 2026-11-15, 2026-11-15 18:00, 7d or 36h")
    if moment.tzinfo is not None:
        raise argparse.ArgumentTypeError(f"{text!r}: give a local time, without a time zone")
    return moment


def expiry_of(args: argparse.Namespace) -> int | None:
    """`--expires` as seconds since 1970 (what the mod compares its clock with), checked."""
    moment = getattr(args, "expires", None)
    if moment is None:
        return None
    if moment <= datetime.now():
        raise SystemExit(f"--expires {moment:%Y-%m-%d %H:%M} is already past.")
    return int(moment.timestamp())


def describe_expiry(expires: int | None) -> str:
    if expires is None:
        return "never expires"
    local = datetime.fromtimestamp(expires)
    utc = datetime.fromtimestamp(expires, timezone.utc)
    return f"stops loading after {local:%Y-%m-%d %H:%M} (your time; {utc:%Y-%m-%d %H:%M} UTC)"


def workspace_version() -> str:
    text = (paths.PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    return match.group(1) if match else "unknown"


def git_revision() -> str:
    """Short commit, `+changes` if the working tree differs from it."""
    def git(*args: str) -> str:
        return subprocess.run(["git", *args], cwd=paths.PROJECT_ROOT, capture_output=True, text=True).stdout.strip()

    commit = git("rev-parse", "--short", "HEAD") or "unknown"
    return f"{commit}+changes" if git("status", "--porcelain") else commit


def check_runtime_imports(dll: Path) -> None:
    missing = [d for d in PeImage(dll).imported_dlls() if d.lower() in UNSHIPPED_RUNTIME_DLLS]
    if missing:
        print(f"WARNING: the mod imports {', '.join(missing)}, which the game folder may not have.")


# --- commands -------------------------------------------------------------------------------


def cmd_setup(args: argparse.Namespace) -> None:
    toolchain.setup()


def cargo_features(args: argparse.Namespace) -> list[str]:
    """The cargo feature arguments for `--coop-test` (coop building itself is in every build)."""
    return ["--features", "coop-test"] if args.coop_test else []


def cmd_build(args: argparse.Namespace) -> None:
    expires = expiry_of(args)
    toolchain.cargo("build", *([] if args.debug else ["--release"]), *cargo_features(args), expires=expires)
    print(f"Built {paths.build_output(profile_of(args))} ({describe_expiry(expires)})")


def cmd_install(args: argparse.Namespace) -> None:
    game = paths.find_game()
    if game_running():
        raise SystemExit("The game is running; close it first (the DLL is locked while loaded).")
    if args.no_build and args.expires:
        raise SystemExit("--expires needs a build: leave out --no-build.")
    if not args.no_build:
        cmd_build(args)
    dll = paths.build_output(profile_of(args))
    if not dll.exists():
        raise SystemExit(f"{dll} doesn't exist; build first.")

    check_runtime_imports(dll)
    loader = game.win64 / "brickworks.dll"
    if not loader.exists():
        print("WARNING: no loader in the game folder (xinput1_3.dll + brickworks.dll); the mod won't load.")
    elif not KNOWN_LOADERS.get(sha256(loader), "").endswith("(supported)"):
        print(f"WARNING: loader is {KNOWN_LOADERS.get(sha256(loader), 'an unknown build')}.")

    game.native_mods.mkdir(exist_ok=True)
    shutil.copy2(dll, game.installed_mod)
    print(f"Installed {game.installed_mod}")


PACKAGE_README = """\
editortweaks {version}{coop_note}
Built {built} from {revision}. This build {expiry}.

A mod for Brick Rigs' vehicle editor, loaded by the brickworks mod loader 0.1.0 (included).
Made for game version {game_version} (Steam): with another version it may not load or may crash.

INSTALL
1. Close Brick Rigs.
2. In Steam, right-click Brick Rigs > Manage > Browse local files. That opens the game folder
   (the one with BrickRigs.exe in it).
3. Extract everything in this zip into that folder, keeping the folders, so that you get:
     <game folder>\\brickworks\\editortweaks.dll
     <game folder>\\BrickRigs\\Binaries\\Win64\\brickworks.dll
     <game folder>\\BrickRigs\\Binaries\\Win64\\xinput1_3.dll
   (If Windows asks to replace an xinput1_3.dll, another mod loader is installed there: ask first.)
4. Start the game from Steam. The options have a new "Editor tweaks" menu.
{coop_howto}
IF SOMETHING GOES WRONG
Send these files from the game folder (close the game first, they're rewritten at each start):
  brickworks.txt{coop_logs}
If the game crashed, also the newest folder in
  %LOCALAPPDATA%\\BrickRigs\\SavedRemastered\\Crashes
{backups}
UNINSTALL
Delete <game folder>\\brickworks\\editortweaks.dll (the mod). To remove the loader too, delete
brickworks.dll and xinput1_3.dll from <game folder>\\BrickRigs\\Binaries\\Win64.
Steam's "Verify integrity of game files" doesn't remove them.

LICENSES
editortweaks: GNU GPL v3 or later (licenses\\editortweaks-LICENSE.txt); its source is available
from its author on request. brickworks loader: MIT (licenses\\brickworks-LICENSE.txt).

Files (SHA-256):
{hashes}
"""

PACKAGE_COOP_HOWTO = """
COOP BUILDING ({kind})
Both players need this same build, and to be Steam friends.
- Open the vehicle editor. In its top bar, "Editor tweaks" > "Coop..." opens the coop page.
- To host: "Host this vehicle". When a friend asks to join, the coop page opens: "Let in".
- To join: "Join a friend", wait for the host to show in the list, then "Join". Your own vehicle
  is backed up first, then replaced by the host's.
- Everything you place, move, paint or change shows for the other player. Undo (Ctrl+Z) only
  undoes your own changes.
{test_steps}"""

PACKAGE_COOP_TEST_STEPS = "- COOP-TEST.txt has the steps to test it together.\n"

PACKAGE_BACKUPS = """Coop backs up your vehicle before joining, and the session's vehicle every few minutes, in
  %LOCALAPPDATA%\\BrickRigs\\SavedRemastered\\EditorTweaks\\CoopBackups
To restore one, copy it over the Vehicle.brv in a saved vehicle's folder.
"""


def cmd_package(args: argparse.Namespace) -> None:
    """A zip to give a tester: the release DLL, the loader from this game install, and a README."""
    game = paths.find_game()
    loader = [game.win64 / "brickworks.dll", game.win64 / "xinput1_3.dll"]
    for file in loader:
        if not file.exists():
            raise SystemExit(f"{file} doesn't exist: install the brickworks loader 0.1.0 first (README.md).")
    if not KNOWN_LOADERS.get(sha256(loader[0]), "").endswith("(supported)"):
        raise SystemExit(f"The installed loader is {KNOWN_LOADERS.get(sha256(loader[0]), 'an unknown build')}, not 0.1.0.")

    expires = expiry_of(args)
    args.debug = False
    cmd_build(args)
    check_runtime_imports(paths.build_output("release"))
    # Without its debug info (code and exports unchanged, a quarter of the size): crash addresses
    # still match this PC's build, which keeps it for `crashes` and `hang`.
    stripped = Path(tempfile.mkdtemp()) / "editortweaks.dll"
    subprocess.run([str(paths.MINGW_BIN / "strip.exe"), "--strip-debug", "-o", str(stripped),
                    str(paths.build_output("release"))], check=True)
    dll = stripped

    version = workspace_version()
    built = datetime.now()
    files = {
        "brickworks/editortweaks.dll": dll,
        "BrickRigs/Binaries/Win64/brickworks.dll": loader[0],
        "BrickRigs/Binaries/Win64/xinput1_3.dll": loader[1],
        "licenses/editortweaks-LICENSE.txt": paths.PROJECT_ROOT / "LICENSE",
        "licenses/brickworks-LICENSE.txt": paths.PROJECT_ROOT / "third_party" / "brickrust" / "LICENSE",
    }
    readme = PACKAGE_README.format(
        version=version,
        coop_note=" (coop building test build)" if args.coop_test else "",
        built=f"{built:%Y-%m-%d %H:%M}",
        revision=git_revision(),
        expiry=describe_expiry(expires).replace("stops loading", "stops loading (the mod just doesn't start)"),
        game_version=game_version(),
        coop_howto=PACKAGE_COOP_HOWTO.format(
            kind="test build" if args.coop_test else "beta",
            test_steps=PACKAGE_COOP_TEST_STEPS if args.coop_test else "",
        ),
        coop_logs="\n  editortweaks-coop-host.txt or editortweaks-coop-member.txt (whichever you have)",
        backups=PACKAGE_BACKUPS,
        hashes="\n".join(f"  {sha256(source)}  {name}" for name, source in files.items() if name.endswith(".dll")),
    )

    out_dir = Path(args.out) if args.out else paths.PROJECT_ROOT / ".local" / "packages"
    out_dir.mkdir(parents=True, exist_ok=True)
    name = f"editortweaks-{version}{'-coop-test' if args.coop_test else ''}-{built:%Y%m%d-%H%M}.zip"
    archive = out_dir / name
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
        for inside, source in files.items():
            zf.write(source, inside)
        zf.writestr("README.txt", readme.replace("\n", "\r\n"))
        if args.coop_test:
            steps = (paths.PROJECT_ROOT / "docs" / "coop-friend-test.txt").read_text(encoding="utf-8")
            zf.writestr("COOP-TEST.txt", steps.replace("\n", "\r\n"))
    shutil.rmtree(stripped.parent, ignore_errors=True)
    print(f"Packaged {archive} ({archive.stat().st_size:,} bytes; {describe_expiry(expires)})")
    print("Your own game needs the same build: et.py install --no-build")


def game_version() -> str:
    """The game version the mod targets (`TARGET_GAME_VERSION` in lib.rs)."""
    text = (paths.PROJECT_ROOT / "crates" / "editortweaks" / "src" / "lib.rs").read_text(encoding="utf-8")
    match = re.search(r'TARGET_GAME_VERSION: &CStr = c"([^"]+)"', text)
    return match.group(1) if match else "unknown"


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
    game = paths.find_game()
    log = game.coop_log(args.coop) if args.coop else game.log
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


def cmd_timings(args: argparse.Namespace) -> None:
    """Switches the mod's timings (`framework/timings.rs`) with `LogTimings` in `EditorTweaks.ini`,
    or prints the timing lines of the last session's log."""
    ini = paths.SAVED_DIR / "Config" / "WindowsNoEditor" / "EditorTweaks.ini"
    if args.state is None:
        log = paths.find_game().log
        lines = log.read_text(encoding="utf-8", errors="replace").splitlines() if log.exists() else []
        timed = [line for line in lines if "timings" in line]
        print("\n".join(timed[-args.lines:] if args.lines else timed) or "No timings in the log.")
        return
    value = "True" if args.state == "on" else "False"
    text = ini.read_text(encoding="utf-8") if ini.exists() else "[EditorTweaks]\n"
    text, found = re.subn(r"(?im)^\s*LogTimings\s*=.*$", f"LogTimings={value}", text)
    if not found:
        text = text.rstrip("\n") + f"\nLogTimings={value}\n"
    ini.parent.mkdir(parents=True, exist_ok=True)
    ini.write_text(text, encoding="utf-8")
    print(f"LogTimings={value} in {ini}")
    if game_running():
        print("The game is running: this applies from its next start (and changing a setting in "
              "its menu before then writes the file over).")


def cmd_second(args: argparse.Namespace) -> None:
    """Starts another game instance beside the running one (coop's local test, `testing.md`).

    Steam refuses a second launch from the library, so this starts the game's launcher directly, with
    the app id Steam would pass (without it the game can't reach Steam). Both instances share the
    Steam account, `brickworks.txt` and `EditorTweaks.ini`.
    """
    game = paths.find_game()
    if not game_running():
        print("The game isn't running: start it from Steam first, then run this for the second one.")
    env = dict(os.environ, SteamAppId=paths.BRICK_RIGS_APP_ID, SteamGameId=paths.BRICK_RIGS_APP_ID)
    flags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
    subprocess.Popen([str(game.launcher)], cwd=game.root, env=env, creationflags=flags, close_fds=True)
    print(f"Started {game.launcher.name}. Coop logs: et.py log --coop host / --coop member")


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
        unique = list(dict.fromkeys(r.targets))
        targets = ", ".join(f"0x{t:X}" for t in unique[:4]) or "-"
        if len(unique) > 4:
            targets += f" (+{len(unique) - 4} more)"  # shapes common to many functions
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


def cmd_blueprint(args: argparse.Namespace) -> None:
    from etlib import blueprint

    decompiler = blueprint.Decompiler(blueprint.export_json(Path(args.asset)))
    functions = decompiler.functions()
    if args.function:
        functions = [f for f in functions if f["ObjectName"] in args.function]
    for function in functions:
        print("\n".join(decompiler.lines(function)))


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

    expires_help = ("a test build: the mod doesn't start after this local date/time "
                    "(2026-11-15, '2026-11-15 18:00', or from now: 7d, 36h)")

    p = sub.add_parser("build", help="build the mod DLL")
    p.add_argument("--debug", action="store_true", help="debug profile instead of release")
    p.add_argument("--coop-test", action="store_true", help="with coop's test-only tools (drift key, local test, probe)")
    p.add_argument("--expires", type=parse_expiry, metavar="WHEN", help=expires_help)
    p.set_defaults(func=cmd_build)

    p = sub.add_parser("install", help="build and copy the DLL into <game>/brickworks/")
    p.add_argument("--debug", action="store_true", help="install the debug build")
    p.add_argument("--no-build", action="store_true", help="install the existing build as-is")
    p.add_argument("--coop-test", action="store_true", help="with coop's test-only tools (drift key, local test, probe)")
    p.add_argument("--expires", type=parse_expiry, metavar="WHEN", help=expires_help)
    p.set_defaults(func=cmd_install)

    p = sub.add_parser("package", help="a zip for a tester: release DLL, loader, README")
    p.add_argument("--coop-test", action="store_true",
                   help="a tester's zip: coop's test-only tools and COOP-TEST.txt")
    p.add_argument("--expires", type=parse_expiry, metavar="WHEN", help=expires_help)
    p.add_argument("--out", help="folder for the zip (default: .local/packages)")
    p.set_defaults(func=cmd_package)

    sub.add_parser("uninstall", help="remove the DLL from the game").set_defaults(func=cmd_uninstall)
    sub.add_parser("status", help="game, loader and install state").set_defaults(func=cmd_status)

    p = sub.add_parser("log", help="print brickworks.txt (the loader + mod log)")
    p.add_argument("-n", "--lines", type=int, default=0, help="only the last N lines")
    p.add_argument("-f", "--follow", action="store_true", help="keep printing new lines")
    p.add_argument("--coop", choices=["host", "member"],
                   help="that instance's coop log instead (needs a --coop-test build, local test)")
    p.set_defaults(func=cmd_log)

    p = sub.add_parser("timings", help="switch the mod's timings in the log (next start), or print them")
    p.add_argument("state", nargs="?", choices=["on", "off"], help="omit to print the log's timing lines")
    p.add_argument("-n", "--lines", type=int, default=0, help="only the last N lines")
    p.set_defaults(func=cmd_timings)

    sub.add_parser("second", help="start a second game instance (coop's local test, needs a --coop-test build)").set_defaults(func=cmd_second)

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

    p = sub.add_parser("blueprint", help="a Blueprint asset's code (e.g. from BRMK), with offsets")
    p.add_argument("asset", help=".uasset file, e.g. BRMK's .../UI/Menu/Widgets/WBP_Menu.uasset")
    p.add_argument("function", nargs="*", help="only these functions or events")
    p.set_defaults(func=cmd_blueprint)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
