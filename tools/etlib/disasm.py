"""Annotated x64 disassembly (needs `capstone`): call/jump targets are named from the file's
exports and imports, and RIP-relative operands show the string or symbol they point at."""

from __future__ import annotations

import capstone
from capstone import x86

from .pe import PeImage


class Disassembler:
    def __init__(self, image: PeImage):
        self.image = image
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True
        self.exports = {rva: name for name, rva in image.exports().items()}
        self.imports = image.import_slots()

    def lines(self, rva: int, count: int) -> list[str]:
        out = []
        code = self.image.image[rva : rva + count * 15]
        for i, insn in enumerate(self.md.disasm(code, rva)):
            if i >= count:
                break
            note = self.annotate(insn)
            text = f"0x{insn.address:08X}  {insn.bytes.hex(' '):<32} {insn.mnemonic} {insn.op_str}"
            out.append(f"{text:<90} ; {note}" if note else text)
        return out

    def annotate(self, insn: capstone.CsInsn) -> str:
        notes = []
        for op in insn.operands:
            if op.type == x86.X86_OP_IMM and (insn.group(capstone.CS_GRP_CALL) or insn.group(capstone.CS_GRP_JUMP)):
                name = self.symbol(op.imm)
                if name:
                    notes.append(name)
            elif op.type == x86.X86_OP_MEM and op.mem.base == x86.X86_REG_RIP:
                target = insn.address + insn.size + op.mem.disp
                notes.append(self.describe_data(target))
        return "  ".join(n for n in notes if n)

    def symbol(self, rva: int) -> str | None:
        """Export name at `rva`, following a jump thunk (incremental linking) or import stub."""
        if rva in self.exports:
            return self.exports[rva]
        insn = next(self.md.disasm(self.image.image[rva : rva + 15], rva), None)
        if insn is None or insn.mnemonic != "jmp" or not insn.operands:
            return None
        op = insn.operands[0]
        if op.type == x86.X86_OP_IMM and op.imm in self.exports:
            return self.exports[op.imm]
        if op.type == x86.X86_OP_MEM and op.mem.base == x86.X86_REG_RIP:
            return self.imports.get(insn.address + insn.size + op.mem.disp)
        return None

    def describe_data(self, rva: int) -> str:
        if rva in self.imports:
            return self.imports[rva]
        if rva in self.exports:
            return self.exports[rva]
        text = self.string_at(rva)
        if text:
            return repr(text)
        return f"[0x{rva:X} {self.image.section_of(rva)}]"

    def string_at(self, rva: int) -> str | None:
        data = self.image.image[rva : rva + 160]
        if len(data) < 4:
            return None
        # UTF-16 (TEXT("...")) first, then ASCII.
        if data[1] == 0 and 32 <= data[0] < 127:
            chars = []
            for i in range(0, len(data) - 1, 2):
                c = data[i] | data[i + 1] << 8
                if c == 0:
                    break
                if not 32 <= c < 127:
                    return None
                chars.append(chr(c))
            return "".join(chars) if len(chars) >= 3 else None
        end = data.find(b"\0")
        if end >= 4 and all(32 <= b < 127 for b in data[:end]):
            return data[:end].decode()
        return None
