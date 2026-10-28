"""Where things live: the project, its build outputs and toolchain, and the game install."""

from __future__ import annotations

import os
import re
import winreg
from dataclasses import dataclass
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
TARGET_TRIPLE = "x86_64-pc-windows-gnu"
MOD_NAME = "editortweaks"

# Project-local MinGW-w64 (WinLibs). Rust's bundled MinGW lacks the assembler that `dlltool`
# needs for raw-dylib imports, so builds put this on PATH. Installed by `et.py setup`.
MINGW_BIN = PROJECT_ROOT / ".local" / "toolchain" / "mingw64" / "bin"

BRICK_RIGS_APP_ID = "552100"
GAME_EXE_NAME = "BrickRigsSteam-Win64-Shipping.exe"

# The game writes saves, configs and crash reports here (not to `Saved`).
SAVED_DIR = Path(os.environ.get("LOCALAPPDATA", "")) / "BrickRigs" / "SavedRemastered"


def build_output(profile: str = "release") -> Path:
    return PROJECT_ROOT / "target" / TARGET_TRIPLE / profile / f"{MOD_NAME}.dll"


@dataclass(frozen=True)
class Game:
    """Paths inside one Brick Rigs install."""

    root: Path

    @property
    def exe(self) -> Path:
        return self.win64 / GAME_EXE_NAME

    @property
    def win64(self) -> Path:
        """Holds the game exe and the loader (xinput1_3.dll + brickworks.dll)."""
        return self.root / "BrickRigs" / "Binaries" / "Win64"

    @property
    def native_mods(self) -> Path:
        """Folder the loader scans for mod DLLs (`_name.dll` = disabled)."""
        return self.root / "brickworks"

    @property
    def installed_mod(self) -> Path:
        return self.native_mods / f"{MOD_NAME}.dll"

    @property
    def log(self) -> Path:
        """Loader log, rewritten on every launch."""
        return self.root / "brickworks.txt"

    def coop_log(self, role: str) -> Path:
        """Coop's own log of the instance in `role` ("host" or "member"), coop builds only."""
        return self.root / f"editortweaks-coop-{role}.txt"

    @property
    def launcher(self) -> Path:
        """What Steam starts: it starts the exe in `win64`."""
        return self.root / "BrickRigs.exe"

    @property
    def blueprint_mods(self) -> Path:
        return self.root / "BrickRigs" / "Mods"


def steam_libraries() -> list[Path]:
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam") as key:
            steam = Path(winreg.QueryValueEx(key, "SteamPath")[0])
    except OSError:
        return []
    libraries = [steam]
    vdf = steam / "steamapps" / "libraryfolders.vdf"
    if vdf.exists():
        text = vdf.read_text(encoding="utf-8", errors="replace")
        for match in re.finditer(r'"path"\s+"([^"]+)"', text):
            libraries.append(Path(match.group(1).replace("\\\\", "\\")))
    return libraries


def app_manifest() -> Path | None:
    for library in steam_libraries():
        manifest = library / "steamapps" / f"appmanifest_{BRICK_RIGS_APP_ID}.acf"
        if manifest.exists():
            return manifest
    return None


def manifest_field(manifest: Path, field: str) -> str | None:
    match = re.search(rf'"{field}"\s+"([^"]*)"', manifest.read_text(encoding="utf-8", errors="replace"))
    return match.group(1) if match else None


def find_game() -> Game:
    """The game install: `ET_GAME_DIR` if set, otherwise found through Steam."""
    override = os.environ.get("ET_GAME_DIR")
    if override:
        return Game(Path(override))
    manifest = app_manifest()
    if manifest:
        install_dir = manifest_field(manifest, "installdir")
        if install_dir:
            return Game(manifest.parent / "common" / install_dir)
    raise SystemExit(
        "Brick Rigs not found through Steam. Set ET_GAME_DIR to the folder containing BrickRigs.exe."
    )
