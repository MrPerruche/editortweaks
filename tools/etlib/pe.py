"""Minimal PE (Windows exe/dll) reader: memory-mapped image, sections, exports, imports."""

from __future__ import annotations

import struct
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Section:
    name: str
    rva: int
    size: int
    raw_offset: int
    raw_size: int
    characteristics: int

    @property
    def executable(self) -> bool:
        return bool(self.characteristics & 0x2000_0000)  # IMAGE_SCN_MEM_EXECUTE


class PeImage:
    """A PE file laid out as the Windows loader maps it, so offsets are RVAs."""

    def __init__(self, path: Path):
        self.path = path
        data = path.read_bytes()
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        section_count = struct.unpack_from("<H", data, pe + 6)[0]
        optional_size = struct.unpack_from("<H", data, pe + 20)[0]
        optional = pe + 24
        self.image_base = struct.unpack_from("<Q", data, optional + 24)[0]
        image_size = struct.unpack_from("<I", data, optional + 56)[0]
        headers_size = struct.unpack_from("<I", data, optional + 60)[0]
        self._directories = optional + 112  # PE32+ data directories

        self.sections: list[Section] = []
        image = bytearray(image_size)
        image[:headers_size] = data[:headers_size]
        for i in range(section_count):
            off = optional + optional_size + i * 40
            name = data[off : off + 8].rstrip(b"\0").decode(errors="replace")
            vsize, rva, raw_size, raw_offset = struct.unpack_from("<IIII", data, off + 8)
            characteristics = struct.unpack_from("<I", data, off + 36)[0]
            copied = min(raw_size, vsize) if vsize else raw_size
            image[rva : rva + copied] = data[raw_offset : raw_offset + copied]
            self.sections.append(
                Section(name, rva, max(vsize, raw_size), raw_offset, raw_size, characteristics)
            )
        self.image = bytes(image)

    def section_of(self, rva: int) -> str:
        for section in self.sections:
            if section.rva <= rva < section.rva + section.size:
                return section.name
        return "?"

    def u32(self, rva: int) -> int:
        return struct.unpack_from("<I", self.image, rva)[0]

    def cstr(self, rva: int) -> str:
        return self.image[rva : self.image.index(b"\0", rva)].decode(errors="replace")

    def exports(self) -> dict[str, int]:
        """Export name -> RVA."""
        directory = self.u32(self._directories)
        if not directory:
            return {}
        name_count = self.u32(directory + 24)
        functions = self.u32(directory + 28)
        names = self.u32(directory + 32)
        ordinals = self.u32(directory + 36)
        result = {}
        for i in range(name_count):
            name = self.cstr(self.u32(names + 4 * i))
            ordinal = struct.unpack_from("<H", self.image, ordinals + 2 * i)[0]
            result[name] = self.u32(functions + 4 * ordinal)
        return result

    def function_starts(self) -> list[tuple[int, int]]:
        """(begin, end) RVAs of every function in the exception directory (.pdata), sorted.
        Chained entries (split-off cold code) are mapped back to their parent function's start."""
        if hasattr(self, "_functions"):
            return self._functions
        rva, size = struct.unpack_from("<II", self.image, self._directories + 3 * 8)
        functions = []
        for off in range(rva, rva + size, 12):
            begin, end, unwind = struct.unpack_from("<III", self.image, off)
            functions.append((self._primary_start(begin, unwind), begin, end))
        self._functions = sorted((b, e, p) for p, b, e in functions)
        return self._functions

    def _primary_start(self, begin: int, unwind: int) -> int:
        for _ in range(8):  # chains are short; bound the walk anyway
            flags = self.image[unwind] >> 3
            if not flags & 0x4:  # UNW_FLAG_CHAININFO
                return begin
            code_count = self.image[unwind + 2]
            parent = unwind + 4 + 2 * (code_count + (code_count & 1))
            begin, _, unwind = struct.unpack_from("<III", self.image, parent)
        return begin

    def function_containing(self, rva: int) -> int | None:
        """Start RVA of the function containing `rva`, per .pdata (None for leaf functions)."""
        import bisect

        functions = self.function_starts()
        i = bisect.bisect_right(functions, (rva, 1 << 32, 0)) - 1
        if i >= 0:
            begin, end, primary = functions[i]
            if begin <= rva < end:
                return primary
        return None

    def import_slots(self) -> dict[int, str]:
        """IAT slot RVA -> "dll!function" (or "dll!#ordinal")."""
        slots = {}
        directory = self.u32(self._directories + 8)
        while directory:
            lookup, _, _, name_rva, iat = struct.unpack_from("<IIIII", self.image, directory)
            if not name_rva:
                break
            dll = self.cstr(name_rva).removesuffix(".dll").removesuffix(".DLL")
            table = lookup or iat
            i = 0
            while True:
                entry = struct.unpack_from("<Q", self.image, table + 8 * i)[0]
                if not entry:
                    break
                if entry >> 63:
                    name = f"#{entry & 0xFFFF}"
                else:
                    name = self.cstr((entry & 0x7FFFFFFF) + 2)
                slots[iat + 8 * i] = f"{dll}!{name}"
                i += 1
            directory += 20
        return slots

    def imported_dlls(self) -> list[str]:
        directory = self.u32(self._directories + 8)
        dlls = []
        while directory:
            name_rva = self.u32(directory + 12)
            if not name_rva:
                break
            dlls.append(self.cstr(name_rva))
            directory += 20
        return dlls
