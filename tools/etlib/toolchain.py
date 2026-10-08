"""Build toolchain: the Rust GNU toolchain plus a project-local MinGW-w64, and running cargo."""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

from .paths import MINGW_BIN, PROJECT_ROOT

RUST_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"

# WinLibs GCC 16.2.0 + MinGW-w64 14.0.0 (msvcrt, matching Rust's windows-gnu target).
WINLIBS_URL = (
    "https://github.com/brechtsanders/winlibs_mingw/releases/download/16.2.0posix-14.0.0-msvcrt-r2/"
    "winlibs-x86_64-posix-seh-gcc-16.2.0-mingw-w64msvcrt-14.0.0-r2.zip"
)
WINLIBS_SHA256 = "1b90ec73c96e6905a913746b56fb6b20122422787029810d3afc6a3b440f2322"
LOCAL_DIR = PROJECT_ROOT / ".local"
VENV_DIR = PROJECT_ROOT / ".venv"


def build_env() -> dict[str, str]:
    """Environment for cargo: MinGW first on PATH (provides `dlltool` + `as` for raw-dylib)."""
    if not (MINGW_BIN / "as.exe").exists():
        raise SystemExit(f"MinGW not found at {MINGW_BIN}. Run: python tools/et.py setup")
    env = dict(os.environ)
    env["PATH"] = f"{MINGW_BIN}{os.pathsep}{env.get('PATH', '')}"
    return env


def cargo(*args: str) -> None:
    result = subprocess.run(["cargo", *args], cwd=PROJECT_ROOT, env=build_env())
    if result.returncode != 0:
        raise SystemExit(result.returncode)


def setup() -> None:
    """Idempotently installs everything needed to build and run the tools."""
    print(f"* Rust toolchain {RUST_TOOLCHAIN}")
    subprocess.run(
        ["rustup", "toolchain", "install", RUST_TOOLCHAIN, "--profile", "minimal",
         "--component", "rustfmt", "--component", "clippy"],
        check=True,
    )

    print(f"* MinGW-w64 in {MINGW_BIN.parent}")
    if (MINGW_BIN / "as.exe").exists():
        print("  already installed")
    else:
        _install_winlibs()

    print(f"* Python venv in {VENV_DIR}")
    python = VENV_DIR / "Scripts" / "python.exe"
    if not python.exists():
        subprocess.run([sys.executable, "-m", "venv", str(VENV_DIR)], check=True)
    subprocess.run(
        [str(python), "-m", "pip", "install", "--quiet", "-r", str(PROJECT_ROOT / "requirements.txt")],
        check=True,
    )
    print("Done.")


def _install_winlibs() -> None:
    downloads = LOCAL_DIR / "downloads"
    downloads.mkdir(parents=True, exist_ok=True)
    archive = downloads / WINLIBS_URL.rsplit("/", 1)[1]
    if not archive.exists() or _sha256(archive) != WINLIBS_SHA256:
        print(f"  downloading {WINLIBS_URL}")
        with urllib.request.urlopen(WINLIBS_URL) as response, open(archive, "wb") as out:
            shutil.copyfileobj(response, out)
    if _sha256(archive) != WINLIBS_SHA256:
        raise SystemExit(f"SHA-256 mismatch for {archive}; delete it and retry")
    print("  extracting")
    with zipfile.ZipFile(archive) as zf:
        zf.extractall(MINGW_BIN.parent.parent)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()
