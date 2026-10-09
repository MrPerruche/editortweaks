"""Byte-signature checks: do the patterns the mod depends on still match the current game exe?

Signatures are read straight from the Rust sources, so there's a single source of truth:
- brickrust's `LookupInfo::Binary(offset, mode, sig!(...))` entries in `lookup!` blocks. These
  resolve like brickworks does: the FIRST match in the image wins, then offset/mode is applied.
- our `FunctionSig { name: "...", pattern: "..." }` declarations (mode "Function"): the single
  match in a code section, mapped to its function's start through .pdata, like `framework::sig`.
"""

from __future__ import annotations

import re
import struct
from dataclasses import dataclass
from pathlib import Path

from .pe import PeImage

_LOOKUP = re.compile(
    r"pub\s+const\s+(?P<name>\w+)\s*:[^=]*?=\s*LookupInfo::Binary\(\s*"
    r"(?P<offset>-?(?:0x[0-9A-Fa-f]+|\d+))\s*,\s*LookupMode::(?P<mode>\w+)\s*,\s*"
    r'sig!\("(?P<pattern>[^"]*)"\)\s*\)',
    re.S,
)
_FUNCTION_SIG = re.compile(
    r'FunctionSig\s*\{\s*name:\s*"(?P<name>[^"]+)",.*?pattern:\s*"(?P<pattern>[^"]+)"',
    re.S,
)


@dataclass(frozen=True)
class Signature:
    name: str
    offset: int
    mode: str  # SignatureStart | Offset32 | Direct64
    pattern: str
    source: str  # file:line

    def regex(self) -> re.Pattern[bytes]:
        parts = []
        for token in self.pattern.split():
            parts.append(b"." if token in ("?", "??") else re.escape(bytes([int(token, 16)])))
        return re.compile(b"".join(parts), re.S)


@dataclass(frozen=True)
class Result:
    signature: Signature
    targets: list[int]  # resolved RVA per match, in match order
    looks_like_function: bool  # first target, for SignatureStart (function) lookups

    @property
    def status(self) -> str:
        if not self.targets:
            return "MISSING"
        if not self.looks_like_function:
            return "NOT-FUNC"
        if len(self.targets) == 1:
            return "OK"
        return "SAME" if len(set(self.targets)) == 1 else "AMBIGUOUS"


def is_function_start(image: PeImage, rva: int) -> bool:
    """MSVC places functions on 16-byte boundaries, preceded by int3/nop padding or a `ret`."""
    return rva % 16 == 0 and image.image[rva - 1] in (0xCC, 0x90, 0xC3)


def find_signatures(roots: list[Path]) -> list[Signature]:
    signatures = []
    for root in roots:
        for path in sorted(root.rglob("*.rs")):
            text = path.read_text(encoding="utf-8", errors="replace")
            source = lambda m: f"{path.relative_to(root.parent)}:{text.count(chr(10), 0, m.start()) + 1}"
            for m in _LOOKUP.finditer(text):
                signatures.append(
                    Signature(m["name"], int(m["offset"], 0), m["mode"], m["pattern"], source(m))
                )
            for m in _FUNCTION_SIG.finditer(text):
                signatures.append(Signature(m["name"], 0, "Function", m["pattern"], source(m)))
    return signatures


def resolve(image: PeImage, signature: Signature) -> list[int]:
    if signature.mode == "Function":
        targets = []
        for section in (s for s in image.sections if s.executable):
            code = image.image[section.rva : section.rva + section.size]
            for match in signature.regex().finditer(code):
                start = image.function_containing(section.rva + match.start())
                targets.append(start if start is not None else -1)
        return targets
    targets = []
    for match in signature.regex().finditer(image.image):
        at = match.start() + signature.offset
        if signature.mode == "Offset32":
            targets.append(at + 4 + struct.unpack_from("<i", image.image, at)[0])
        elif signature.mode == "Direct64":
            targets.append(struct.unpack_from("<Q", image.image, at)[0] - image.image_base)
        else:
            targets.append(at)
    return targets


def check(image: PeImage, signatures: list[Signature]) -> list[Result]:
    results = []
    for sig in signatures:
        targets = resolve(image, sig)
        if sig.mode == "SignatureStart":
            function_ok = not targets or is_function_start(image, targets[0])
        elif sig.mode == "Function":
            function_ok = -1 not in targets  # every match must be inside a known function
        else:
            function_ok = True
        results.append(Result(sig, targets, function_ok))
    return results
