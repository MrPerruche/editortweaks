"""Byte-signature checks: do the patterns the mod depends on still match the current game exe?

Signatures are read straight from the Rust sources, so there's a single source of truth:
- brickrust's `LookupInfo::Binary(offset, mode, sig!(...))` entries in `lookup!` blocks. These
  resolve like brickworks does: the FIRST match in the image wins, then offset/mode is applied.
- our `FunctionSig { name: "...", pattern: "..." }` declarations (mode "Function"): the single
  match in a code section, mapped to its function's start through .pdata, like `framework::sig`.
- our `StringRefSig { name: "...", text: "..." }` declarations (mode "StringRef"): the function(s)
  loading the address of that UTF-16 string with `lea r64, [rip + disp32]`; there must be one.
- our `CodeShape { name: "...", shape: "..." }` declarations (mode "Shape"): a leaf function's
  whole code, which the mod finds through a vtable at run time; at least one function (at a
  function boundary) must have it. Several identical ones are fine.
- our `VtableSig { name: "...", type_name: "...", length: N }` declarations (mode "Vtable"): the
  virtual function tables of exactly N functions whose entries 0, 2 and 3 load that ASCII type
  name, like `framework::sig::VtableSig`. Several (identical copies) are fine.
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
_STRING_REF_SIG = re.compile(
    r'StringRefSig\s*\{\s*name:\s*"(?P<name>[^"]+)",.*?text:\s*"(?P<text>[^"]+)"',
    re.S,
)
_CODE_SHAPE = re.compile(
    r'CodeShape\s*\{\s*name:\s*"(?P<name>[^"]+)",.*?shape:\s*"(?P<pattern>[^"]+)"',
    re.S,
)
_VTABLE_SIG = re.compile(
    r'VtableSig\s*\{\s*name:\s*"(?P<name>[^"]+)",\s*type_name:\s*"(?P<type>[^"]+)",'
    r"\s*length:\s*(?P<length>\d+)",
    re.S,
)
# `lea r64, [rip + disp32]`: REX.W (48 or 4C), 8D, ModRM with mod 00 and r/m 101.
_RIP_LEA = re.compile(rb"[\x48\x4c]\x8d[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]", re.S)


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
        if len(self.targets) == 1 or self.signature.mode in ("Shape", "Vtable"):
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
            for m in _STRING_REF_SIG.finditer(text):
                # The pattern holds the string itself.
                signatures.append(Signature(m["name"], 0, "StringRef", m["text"], source(m)))
            for m in _CODE_SHAPE.finditer(text):
                signatures.append(Signature(m["name"], 0, "Shape", m["pattern"], source(m)))
            for m in _VTABLE_SIG.finditer(text):
                # The pattern holds the type name, the offset the table's length.
                signatures.append(
                    Signature(m["name"], int(m["length"]), "Vtable", m["type"], source(m))
                )
    return signatures


def string_references(image: PeImage, text: str) -> list[int]:
    """Start of the function containing each `lea` of the UTF-16 string `text` (-1 if none)."""
    needle = text.encode("utf-16-le") + b"\x00\x00"
    strings = {
        m.start()
        for m in re.finditer(re.escape(needle), image.image)
        if m.start() % 2 == 0 and not image.section_of(m.start()).startswith(".text")
    }
    return _functions_loading(image, strings)


def vtables(image: PeImage, type_name: str, length: int) -> list[int]:
    """Tables of `length` code pointers whose entries 0, 2 and 3 load the ASCII `type_name`."""
    needle = type_name.encode() + b"\x00"
    strings = {
        m.start()
        for m in re.finditer(re.escape(needle), image.image)
        if image.image[m.start() - 1] == 0
        and not image.section_of(m.start()).startswith(".text")
    }
    loading = {image.image_base + f for f in _functions_loading(image, strings) if f >= 0}

    def is_code(value: int) -> bool:
        rva = value - image.image_base
        return 0 < rva < len(image.image) and image.section_of(rva).startswith(".text")

    tables = []
    for first in loading:
        for m in re.finditer(re.escape(struct.pack("<Q", first)), image.image):
            at = m.start()
            if at % 8 or image.section_of(at).startswith(".text"):
                continue
            table = struct.unpack_from(f"<{length + 1}Q", image.image, at)
            if (
                table[2] in loading
                and table[3] in loading
                and all(is_code(entry) for entry in table[:length])
                and not is_code(table[length])
            ):
                tables.append(at)
    return sorted(tables)


def _functions_loading(image: PeImage, strings: set[int]) -> list[int]:
    """Start of the function containing each `lea` of one of `strings` (RVAs; -1 if none)."""
    functions = []
    for section in (s for s in image.sections if s.executable):
        code = image.image[section.rva : section.rva + section.size]
        for m in _RIP_LEA.finditer(code):
            at = section.rva + m.start()
            if at + 7 + struct.unpack_from("<i", image.image, at + 3)[0] in strings:
                start = image.function_containing(at)
                functions.append(start if start is not None else -1)
    return functions


def resolve(image: PeImage, signature: Signature) -> list[int]:
    if signature.mode == "StringRef":
        return string_references(image, signature.pattern)
    if signature.mode == "Vtable":
        return vtables(image, signature.pattern, signature.offset)
    if signature.mode == "Shape":
        targets = []
        for section in (s for s in image.sections if s.executable):
            code = image.image[section.rva : section.rva + section.size]
            for match in signature.regex().finditer(code):
                if is_function_start(image, section.rva + match.start()):
                    targets.append(section.rva + match.start())
        return targets
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
        elif sig.mode in ("Function", "StringRef"):
            function_ok = -1 not in targets  # every match must be inside a known function
        else:
            function_ok = True
        results.append(Result(sig, targets, function_ok))
    return results
