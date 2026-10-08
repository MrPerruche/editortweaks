"""Summaries of the game's crash reports (Unreal's CrashReportClient folders)."""

from __future__ import annotations

import datetime as dt
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path

from .paths import SAVED_DIR

CRASH_DIR = SAVED_DIR / "Crashes"


@dataclass(frozen=True)
class Crash:
    folder: Path
    time: dt.datetime
    error: str
    seconds_since_start: str
    frames: list[str]  # "module+0xoffset", innermost first
    modules: list[str]  # loaded non-Windows modules

    @staticmethod
    def load(folder: Path) -> Crash | None:
        context = folder / "CrashContext.runtime-xml"
        if not context.exists():
            return None
        props = ET.parse(context).getroot().find("RuntimeProperties")
        if props is None:
            return None

        def text(tag: str) -> str:
            node = props.find(tag)
            return (node.text or "").strip() if node is not None else ""

        # PCallStack: "Module 0xBASE + offset Module 0xBASE + offset ..."
        tokens = text("PCallStack").split()
        frames = [f"{tokens[i]}+0x{tokens[i + 3]}" for i in range(0, len(tokens) - 3, 4)]
        modules = [m.strip() for m in text("Modules").splitlines() if m.strip()]
        modules = [m for m in modules if "\\windows\\" not in m.lower()]
        return Crash(
            folder=folder,
            time=dt.datetime.fromtimestamp(folder.stat().st_mtime),
            error=text("ErrorMessage"),
            seconds_since_start=text("SecondsSinceStart"),
            frames=frames,
            modules=modules,
        )


def recent(count: int) -> list[Crash]:
    if not CRASH_DIR.exists():
        return []
    folders = sorted((p for p in CRASH_DIR.iterdir() if p.is_dir()), key=lambda p: p.stat().st_mtime)
    crashes = [Crash.load(folder) for folder in folders[-count:]]
    return [c for c in crashes if c is not None]
