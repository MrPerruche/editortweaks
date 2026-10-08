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
            copied = min(raw_size, vsize) if vsize else raw_size
            image[rva : rva + copied] = data[raw_offset : raw_offset + copied]
            self.sections.append(Section(name, rva, max(vsize, raw_size), raw_offset, raw_size))
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
