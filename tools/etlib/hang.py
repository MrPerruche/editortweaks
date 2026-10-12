"""Where a frozen game is stuck: samples its threads' instruction pointers and call stacks.

A hang leaves no crash report. This suspends the game thread (and the busiest other thread) a
few times, reads its registers and stack, and resumes it. The stack is scanned for return
addresses (values pointing into a module right after a call instruction): a heuristic, so stale
values from earlier calls can show up deeper down, but the top frames are reliable.

Windows only, stdlib only (ctypes). The game keeps running afterwards.
"""

from __future__ import annotations

import ctypes
import subprocess
import time
from ctypes import wintypes
from dataclasses import dataclass, field
from pathlib import Path

from . import paths
from .pe import PeImage

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

TH32CS_SNAPTHREAD = 0x4
TH32CS_SNAPMODULE = 0x8
TH32CS_SNAPMODULE32 = 0x10
TH32CS_SNAPPROCESS = 0x2
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
THREAD_SUSPEND_RESUME = 0x0002
THREAD_GET_CONTEXT = 0x0008
THREAD_QUERY_INFORMATION = 0x0040

# x64 CONTEXT: 0x4D0 bytes, 16-byte aligned; offsets of the fields read here.
CONTEXT_SIZE = 0x4D0
CONTEXT_FLAGS_OFFSET = 0x30
CONTEXT_CONTROL_INTEGER = 0x00100003
RSP_OFFSET = 0x98
RIP_OFFSET = 0xF8

MAX_STACK_BYTES = 512 * 1024


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD), ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
        ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", wintypes.LONG),
        ("dwFlags", wintypes.DWORD), ("szExeFile", wintypes.WCHAR * 260),
    ]


class THREADENTRY32(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
        ("th32ThreadID", wintypes.DWORD), ("th32OwnerProcessID", wintypes.DWORD),
        ("tpBasePri", wintypes.LONG), ("tpDeltaPri", wintypes.LONG), ("dwFlags", wintypes.DWORD),
    ]


class MODULEENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD), ("th32ModuleID", wintypes.DWORD),
        ("th32ProcessID", wintypes.DWORD), ("GlblcntUsage", wintypes.DWORD),
        ("ProccntUsage", wintypes.DWORD), ("modBaseAddr", ctypes.c_void_p),
        ("modBaseSize", wintypes.DWORD), ("hModule", wintypes.HMODULE),
        ("szModule", wintypes.WCHAR * 256), ("szExePath", wintypes.WCHAR * 260),
    ]


class MEMORY_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BaseAddress", ctypes.c_void_p), ("AllocationBase", ctypes.c_void_p),
        ("AllocationProtect", wintypes.DWORD), ("PartitionId", wintypes.WORD),
        ("RegionSize", ctypes.c_size_t), ("State", wintypes.DWORD),
        ("Protect", wintypes.DWORD), ("Type", wintypes.DWORD),
    ]


kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.OpenThread.restype = wintypes.HANDLE
kernel32.SuspendThread.restype = wintypes.DWORD
kernel32.ResumeThread.restype = wintypes.DWORD
kernel32.ReadProcessMemory.argtypes = [
    wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_size_t),
]
kernel32.VirtualQueryEx.argtypes = [
    wintypes.HANDLE, ctypes.c_void_p, ctypes.POINTER(MEMORY_BASIC_INFORMATION), ctypes.c_size_t,
]
kernel32.VirtualQueryEx.restype = ctypes.c_size_t
kernel32.GetThreadContext.argtypes = [wintypes.HANDLE, ctypes.c_void_p]
kernel32.GetThreadTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_ulonglong)] * 4


@dataclass
class Module:
    name: str
    path: Path
    base: int
    size: int

    def contains(self, address: int) -> bool:
        return self.base <= address < self.base + self.size


@dataclass
class Sample:
    rip: int
    returns: list[int]  # return addresses found on the stack, innermost first


@dataclass
class ThreadReport:
    tid: int
    main: bool
    cpu_share: float  # of one core, over the measuring interval
    samples: list[Sample] = field(default_factory=list)


def _snapshot(flags: int, pid: int = 0) -> int:
    handle = kernel32.CreateToolhelp32Snapshot(flags, pid)
    if handle == INVALID_HANDLE_VALUE or handle is None:
        raise SystemExit(f"CreateToolhelp32Snapshot failed ({ctypes.get_last_error()})")
    return handle


def find_pid(exe_name: str) -> int | None:
    snap = _snapshot(TH32CS_SNAPPROCESS)
    try:
        entry = PROCESSENTRY32W(dwSize=ctypes.sizeof(PROCESSENTRY32W))
        ok = kernel32.Process32FirstW(snap, ctypes.byref(entry))
        while ok:
            if entry.szExeFile.lower() == exe_name.lower():
                return entry.th32ProcessID
            ok = kernel32.Process32NextW(snap, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snap)
    return None


def thread_ids(pid: int) -> list[int]:
    """The process's threads, in the order Windows lists them (the main thread first)."""
    snap = _snapshot(TH32CS_SNAPTHREAD)
    found = []
    try:
        entry = THREADENTRY32(dwSize=ctypes.sizeof(THREADENTRY32))
        ok = kernel32.Thread32First(snap, ctypes.byref(entry))
        while ok:
            if entry.th32OwnerProcessID == pid:
                found.append(entry.th32ThreadID)
            ok = kernel32.Thread32Next(snap, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snap)
    return found


def modules(pid: int) -> list[Module]:
    snap = _snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    found = []
    try:
        entry = MODULEENTRY32W(dwSize=ctypes.sizeof(MODULEENTRY32W))
        ok = kernel32.Module32FirstW(snap, ctypes.byref(entry))
        while ok:
            found.append(Module(entry.szModule, Path(entry.szExePath), entry.modBaseAddr or 0,
                                entry.modBaseSize))
            ok = kernel32.Module32NextW(snap, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snap)
    return found


def _read(process: int, address: int, size: int) -> bytes | None:
    buffer = ctypes.create_string_buffer(size)
    done = ctypes.c_size_t()
    if not kernel32.ReadProcessMemory(process, address, buffer, size, ctypes.byref(done)):
        return None
    return buffer.raw[: done.value]


def _cpu_time(tid: int) -> int | None:
    handle = kernel32.OpenThread(THREAD_QUERY_INFORMATION, False, tid)
    if not handle:
        return None
    try:
        times = [ctypes.c_ulonglong() for _ in range(4)]
        if not kernel32.GetThreadTimes(handle, *(ctypes.byref(t) for t in times)):
            return None
        return times[2].value + times[3].value  # kernel + user, 100 ns units
    finally:
        kernel32.CloseHandle(handle)


def _stack_end(process: int, rsp: int) -> int:
    """The end of the committed stack region holding `rsp` (the stack grows down to `rsp`)."""
    info = MEMORY_BASIC_INFORMATION()
    if not kernel32.VirtualQueryEx(process, rsp, ctypes.byref(info), ctypes.sizeof(info)):
        return rsp + 64 * 1024
    return min((info.BaseAddress or 0) + info.RegionSize, rsp + MAX_STACK_BYTES)


def _call_length(code: bytes, start: int) -> int | None:
    """The length of an indirect call (FF /2) starting at `code[start]`, if it is one."""
    if start + 1 >= len(code) or code[start] != 0xFF:
        return None
    modrm = code[start + 1]
    mod, reg, rm = modrm >> 6, (modrm >> 3) & 7, modrm & 7
    if reg != 2:
        return None
    if mod == 3:
        return 2
    length = 2
    if rm == 4:
        length += 1
        if mod == 0 and start + 2 < len(code) and code[start + 2] & 7 == 5:
            length += 4
    elif mod == 0 and rm == 5:
        length += 4
    length += {0: 0, 1: 1, 2: 4}[mod]
    return length


def _after_call(process: int, address: int) -> bool:
    """Whether `address` comes right after a call instruction (so it's a return address)."""
    code = _read(process, address - 8, 8)
    if code is None or len(code) < 8:
        return False
    if code[3] == 0xE8:  # call rel32, 5 bytes
        return True
    # FF /2, possibly after a REX prefix: lengths 2 to 7.
    return any(_call_length(code, 8 - back) == back for back in range(2, 8))


def _sample_thread(process: int, tid: int, mods: list[Module], depth: int) -> Sample | None:
    handle = kernel32.OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, False, tid)
    if not handle:
        return None
    raw = ctypes.create_string_buffer(CONTEXT_SIZE + 16)
    context = (ctypes.addressof(raw) + 15) & ~15
    ctypes.c_uint32.from_address(context + CONTEXT_FLAGS_OFFSET).value = CONTEXT_CONTROL_INTEGER
    try:
        if kernel32.SuspendThread(handle) == 0xFFFFFFFF:
            return None
        try:
            if not kernel32.GetThreadContext(handle, context):
                return None
            rip = ctypes.c_uint64.from_address(context + RIP_OFFSET).value
            rsp = ctypes.c_uint64.from_address(context + RSP_OFFSET).value
            stack = _read(process, rsp, _stack_end(process, rsp) - rsp) or b""
        finally:
            kernel32.ResumeThread(handle)
    finally:
        kernel32.CloseHandle(handle)

    returns = []
    for offset in range(0, len(stack) - 7, 8):
        value = int.from_bytes(stack[offset : offset + 8], "little")
        if any(m.contains(value) for m in mods) and _after_call(process, value):
            returns.append(value)
            if len(returns) >= depth:
                break
    return Sample(rip, returns)


def sample(pid: int, count: int, interval: float, depth: int, busy_threads: int) -> tuple[list[ThreadReport], list[Module]]:
    """Samples the main thread and the `busy_threads` busiest others, `count` times each."""
    tids = thread_ids(pid)
    if not tids:
        raise SystemExit("The game has no threads (exiting?).")
    before = {tid: _cpu_time(tid) for tid in tids}
    measure = 0.5
    time.sleep(measure)
    share = {}
    for tid in tids:
        start, end = before.get(tid), _cpu_time(tid)
        share[tid] = (end - start) / (measure * 1e7) if start is not None and end is not None else 0.0
    main = tids[0]
    busiest = sorted((t for t in tids if t != main), key=lambda t: share[t], reverse=True)
    chosen = [main] + busiest[:busy_threads]

    process = kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)
    if not process:
        raise SystemExit(f"Can't open the game process ({ctypes.get_last_error()}).")
    try:
        mods = modules(pid)
        reports = [ThreadReport(tid, tid == main, share[tid]) for tid in chosen]
        for index in range(count):
            for report in reports:
                taken = _sample_thread(process, report.tid, mods, depth)
                if taken:
                    report.samples.append(taken)
            if index + 1 < count:
                time.sleep(interval)
    finally:
        kernel32.CloseHandle(process)
    return reports, mods


class Symbolizer:
    """Names addresses: game functions by their .pdata start, ours by addr2line (line tables)."""

    def __init__(self, mods: list[Module]):
        self.mods = mods
        self.images: dict[Path, PeImage | None] = {}
        self.ours: dict[int, str] = {}

    def module_of(self, address: int) -> Module | None:
        return next((m for m in self.mods if m.contains(address)), None)

    def resolve_ours(self, addresses: list[int]) -> None:
        """Runs addr2line once for every address in our DLL."""
        mine = sorted({a for a in addresses if (m := self.module_of(a)) and m.name.lower() == f"{paths.MOD_NAME}.dll"})
        tool = paths.MINGW_BIN / "addr2line.exe"
        if not mine or not tool.exists():
            return
        module = self.module_of(mine[0])
        image_base = self._image(module.path).image_base if self._image(module.path) else 0x180000000
        # A return address points after the call: look up the byte before it.
        queried = [image_base + (a - module.base) - 1 for a in mine]
        out = subprocess.run(
            [str(tool), "-f", "-C", "-e", str(module.path), *(f"0x{q:x}" for q in queried)],
            capture_output=True, text=True,
        ).stdout.splitlines()
        for i, address in enumerate(mine):
            if 2 * i + 1 < len(out):
                function, line = out[2 * i], out[2 * i + 1]
                where = line.replace("\\", "/").split("/src/")[-1] if "?" not in line else ""
                self.ours[address] = f"{function}  {where}".strip()

    def _image(self, path: Path) -> PeImage | None:
        if path not in self.images:
            try:
                self.images[path] = PeImage(path)
            except Exception:  # noqa: BLE001 - any unreadable module just stays unnamed
                self.images[path] = None
        return self.images[path]

    def name(self, address: int) -> str:
        module = self.module_of(address)
        if module is None:
            return f"0x{address:X} (no module: a hook trampoline or generated code)"
        rva = address - module.base
        text = f"{module.name.removesuffix('.exe').removesuffix('.dll')}+0x{rva:X}"
        if address in self.ours:
            return f"{text}  {self.ours[address]}"
        if module.path.name.lower() == paths.GAME_EXE_NAME.lower():
            image = self._image(module.path)
            start = image.function_containing(rva) if image else None
            if start is not None:
                text += f"  (function 0x{start:X}+0x{rva - start:X})"
        return text
