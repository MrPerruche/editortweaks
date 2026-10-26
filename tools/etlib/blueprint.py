"""Blueprint bytecode, readable: the compiled code of a Blueprint asset's functions and events.

The mod kit (BRMK) ships the game's Blueprints as editor assets, which keep their compiled code.
UAssetGUI (github.com/atenfyr/UAssetGUI, fetched on first use) exports an asset to JSON; this prints
each function's statements with their offsets in memory (UE 4.27 layout), so jumps (`goto 6025`,
an event's `ExecuteUbergraph_X(6025)`) can be followed. E.g. `WBP_Menu`: which context each button
opens, what each context shows.
"""

from __future__ import annotations

import json
import subprocess
import tempfile
import urllib.request
from pathlib import Path

from . import paths

UASSETGUI_VERSION = "1.1.0"
UASSETGUI_URL = (
    f"https://github.com/atenfyr/UAssetGUI/releases/download/v{UASSETGUI_VERSION}/UAssetGUI.exe"
)
UASSETGUI = paths.PROJECT_ROOT / ".local" / "downloads" / f"uassetgui-{UASSETGUI_VERSION}" / "UAssetGUI.exe"

# In-memory sizes of bytecode operands (UE 4.27, 64-bit): object and property pointers, script
# names (`FScriptName`: comparison index, display index, number) and jump offsets.
POINTER, NAME, OFFSET = 8, 12, 4


def uassetgui() -> Path:
    if not UASSETGUI.exists():
        print(f"downloading UAssetGUI {UASSETGUI_VERSION} to {UASSETGUI.parent}")
        UASSETGUI.parent.mkdir(parents=True, exist_ok=True)
        urllib.request.urlretrieve(UASSETGUI_URL, UASSETGUI)
    return UASSETGUI


def export_json(asset: Path) -> dict:
    """The asset as UAssetGUI's JSON (it reads a copy, in a scratch folder). A cooked asset's
    `.uexp` (and `.ubulk`) next to it are copied too: they hold its exports' data."""
    with tempfile.TemporaryDirectory() as scratch:
        copy = Path(scratch) / asset.name
        copy.write_bytes(asset.read_bytes())
        for suffix in (".uexp", ".ubulk"):
            sibling = asset.with_suffix(suffix)
            if sibling.exists():
                copy.with_suffix(suffix).write_bytes(sibling.read_bytes())
        out = Path(scratch) / "asset.json"
        subprocess.run([str(uassetgui()), "tojson", str(copy), str(out), "VER_UE4_27"], check=True)
        if not out.exists():
            raise SystemExit(f"UAssetGUI exported nothing for {asset}")
        return json.loads(out.read_text(encoding="utf-8"))


class Decompiler:
    def __init__(self, asset: dict):
        self.imports = asset["Imports"]
        self.exports = asset["Exports"]

    def functions(self) -> list[dict]:
        return [e for e in self.exports if "FunctionExport" in e.get("$type", "")]

    def lines(self, function: dict) -> list[str]:
        out = [f"== {function['ObjectName']} ({function.get('ScriptBytecodeSize')} bytes)"]
        at = 0
        for statement in function.get("ScriptBytecode") or []:
            if kind(statement) not in ("EX_Tracepoint", "EX_WireTracepoint", "EX_Nothing"):
                out.append(f"  {at:5}: {self.show(statement)}")
            at += size(statement)
        if at != function.get("ScriptBytecodeSize"):
            out.append(f"  !! offsets wrong: {at} bytes counted")
        return out

    def object_name(self, index) -> str:
        if isinstance(index, dict):
            index = index.get("Index", 0)
        if not isinstance(index, int) or index == 0:
            return "null"
        if index > 0:
            return self.exports[index - 1]["ObjectName"]
        imported = self.imports[-index - 1]
        outer = imported.get("OuterIndex", 0)
        owner = self.object_name(outer) if outer else ""
        return f"{owner.split('.')[-1]}.{imported['ObjectName']}" if owner not in ("", "null") else imported["ObjectName"]

    def show(self, e: dict) -> str:
        k = kind(e)
        show = self.show
        if k in ("EX_LocalVariable", "EX_InstanceVariable", "EX_LocalOutVariable"):
            return ("self." if k == "EX_InstanceVariable" else "") + property_name(e["Variable"])
        if k == "EX_Let":
            return f"{show(e['Variable'])} = {show(e['Expression'])}"
        if k in ("EX_LetObj", "EX_LetBool", "EX_LetWeakObjPtr", "EX_LetDelegate", "EX_LetMulticastDelegate"):
            return f"{show(e['VariableExpression'])} = {show(e['AssignmentExpression'])}"
        if k == "EX_LetValueOnPersistentFrame":
            return f"frame.{property_name(e['DestinationProperty'])} = {show(e['AssignmentExpression'])}"
        if k == "EX_Jump":
            return f"goto {e['CodeOffset']}"
        if k == "EX_JumpIfNot":
            return f"if not ({show(e['BooleanExpression'])}) goto {e['CodeOffset']}"
        if k == "EX_ComputedJump":
            return f"goto ({show(e['CodeOffsetExpression'])})"
        if k == "EX_PushExecutionFlow":
            return f"push {e['PushingAddress']}"
        if k == "EX_PopExecutionFlow":
            return "pop"
        if k == "EX_PopExecutionFlowIfNot":
            return f"pop if not ({show(e['BooleanExpression'])})"
        if k == "EX_Return":
            return f"return {show(e['ReturnExpression'])}".rstrip()
        if k in ("EX_IntConst", "EX_FloatConst", "EX_ByteConst"):
            return str(e["Value"])
        if k == "EX_NameConst":
            return f"'{e['Value']}'"
        if k in ("EX_StringConst", "EX_UnicodeStringConst"):
            return json.dumps(e["Value"])
        if k == "EX_ObjectConst":
            return self.object_name(e["Value"])
        if k == "EX_SoftObjectConst":
            return f"soft({show(e['Value'])})"
        if k == "EX_TextConst":
            return self.text(e["Value"])
        if k in ("EX_FinalFunction", "EX_LocalFinalFunction", "EX_CallMath"):
            return f"{self.object_name(e['StackNode'])}({', '.join(map(show, e['Parameters']))})"
        if k in ("EX_VirtualFunction", "EX_LocalVirtualFunction"):
            return f"{e['VirtualFunctionName']}({', '.join(map(show, e['Parameters']))})"
        if k in ("EX_Context", "EX_Context_FailSilent"):
            return f"{show(e['ObjectExpression'])}->{show(e['ContextExpression'])}"
        if k == "EX_StructMemberContext":
            return f"{show(e['StructExpression'])}.{property_name(e['StructMemberExpression'])}"
        if k == "EX_DynamicCast":
            return f"Cast<{self.object_name(e['ClassPtr'])}>({show(e['Target'])})"
        if k == "EX_PrimitiveCast":
            return f"({e['ConversionType']})({show(e['Target'])})"
        if k == "EX_BindDelegate":
            return f"bind({show(e['Delegate'])}, {show(e['ObjectTerm'])}, '{e['FunctionName']}')"
        if k == "EX_SwitchValue":
            cases = "; ".join(f"{show(c['CaseIndexValueTerm'])}: {show(c['CaseTerm'])}" for c in e["Cases"])
            return f"switch ({show(e['IndexTerm'])}) {{{cases}; default: {show(e['DefaultTerm'])}}}"
        if k == "EX_StructConst":
            return f"{self.object_name(e['Struct'])}{{{', '.join(map(show, e['Value']))}}}"
        if k == "EX_SetArray":
            return f"{show(e['AssigningProperty'])} = [{', '.join(map(show, e['Elements']))}]"
        if k == "EX_ArrayConst":
            return f"[{', '.join(map(show, e['Elements']))}]"
        if k in ("EX_Int64Const", "EX_UInt64Const", "EX_IntConstByte", "EX_SkipOffsetConst"):
            return str(e["Value"])
        if k in ("EX_VectorConst", "EX_RotationConst", "EX_TransformConst"):
            return f"{k[3:-5]}{json.dumps(e['Value'], separators=(',', ':'))}"
        if k in ("EX_ObjToInterfaceCast", "EX_CrossInterfaceCast", "EX_InterfaceToObjCast"):
            return f"Cast<{self.object_name(e['ClassPtr'])}>({show(e['Target'])})"
        if k == "EX_MetaCast":
            return f"ClassCast<{self.object_name(e['ClassPtr'])}>({show(e['TargetExpression'])})"
        if k == "EX_InterfaceContext":
            return show(e["InterfaceValue"])
        if k == "EX_InstanceDelegate":
            return f"delegate('{e['FunctionName']}')"
        if k == "EX_AddMulticastDelegate":
            return f"{show(e['Delegate'])} += {show(e['DelegateToAdd'])}"
        if k == "EX_RemoveMulticastDelegate":
            return f"{show(e['Delegate'])} -= {show(e['DelegateToAdd'])}"
        if k == "EX_ClearMulticastDelegate":
            return f"{show(e['DelegateToClear'])}.Clear()"
        if k == "EX_CallMulticastDelegate":
            return f"{show(e['Delegate'])}.Broadcast({', '.join(map(show, e['Parameters']))})"
        if k == "EX_ArrayGetByRef":
            return f"{show(e['ArrayVariable'])}[{show(e['ArrayIndex'])}]"
        if k == "EX_DefaultVariable":
            return "default." + property_name(e["Variable"])
        simple = {"EX_Self": "self", "EX_True": "true", "EX_False": "false", "EX_NoObject": "None",
                  "EX_Nothing": "", "EX_EndOfScript": "end", "EX_IntZero": "0", "EX_IntOne": "1",
                  "EX_NoInterface": "None"}
        return simple.get(k, k)

    def text(self, v: dict) -> str:
        literal = v["TextLiteralType"]
        if literal == "LocalizedText":
            return f"LOCTEXT({self.show(v['LocalizedSource'])})"
        if literal == "StringTableEntry":
            return f"LOCTABLE({self.show(v['StringTableKey'])})"
        if literal == "InvariantText":
            return f"INVTEXT({self.show(v['InvariantLiteralString'])})"
        if literal == "LiteralString":
            return f"TEXT({self.show(v['LiteralString'])})"
        return "EmptyText"


def kind(e: dict) -> str:
    return e["$type"].split(",")[0].split(".")[-1]


def property_name(pointer: dict | None) -> str:
    new = (pointer or {}).get("New") or {}
    return ".".join(new.get("Path") or ["?"])


def size(e: dict) -> int:
    """An expression's size in memory, as the engine lays out loaded bytecode (jump targets count
    in it)."""
    k = kind(e)
    if k in ("EX_EndOfScript", "EX_Nothing", "EX_Self", "EX_True", "EX_False", "EX_NoObject",
             "EX_Tracepoint", "EX_WireTracepoint", "EX_PopExecutionFlow"):
        return 1
    if k in ("EX_LocalVariable", "EX_InstanceVariable", "EX_LocalOutVariable", "EX_DefaultVariable",
             "EX_ObjectConst"):
        return 1 + POINTER
    if k == "EX_Let":
        return 1 + POINTER + size(e["Variable"]) + size(e["Expression"])
    if k in ("EX_LetObj", "EX_LetBool", "EX_LetWeakObjPtr", "EX_LetDelegate", "EX_LetMulticastDelegate"):
        return 1 + size(e["VariableExpression"]) + size(e["AssignmentExpression"])
    if k == "EX_LetValueOnPersistentFrame":
        return 1 + POINTER + size(e["AssignmentExpression"])
    if k in ("EX_Jump", "EX_PushExecutionFlow"):
        return 1 + OFFSET
    if k == "EX_JumpIfNot":
        return 1 + OFFSET + size(e["BooleanExpression"])
    if k == "EX_ComputedJump":
        return 1 + size(e["CodeOffsetExpression"])
    if k == "EX_PopExecutionFlowIfNot":
        return 1 + size(e["BooleanExpression"])
    if k == "EX_Return":
        return 1 + size(e["ReturnExpression"])
    if k in ("EX_IntConst", "EX_FloatConst"):
        return 1 + 4
    if k == "EX_ByteConst":
        return 1 + 1
    if k == "EX_NameConst":
        return 1 + NAME
    if k == "EX_StringConst":
        return 1 + len(e["Value"].encode("latin-1", "replace")) + 1
    if k == "EX_UnicodeStringConst":
        return 1 + 2 * (len(e["Value"]) + 1)
    if k == "EX_SoftObjectConst":
        return 1 + size(e["Value"])
    if k == "EX_TextConst":
        v = e["Value"]
        literal = v["TextLiteralType"]
        parts = {
            "LocalizedText": ["LocalizedSource", "LocalizedKey", "LocalizedNamespace"],
            "InvariantText": ["InvariantLiteralString"],
            "LiteralString": ["LiteralString"],
            "StringTableEntry": ["StringTableId", "StringTableKey"],
        }.get(literal, [])
        table = POINTER if literal == "StringTableEntry" else 0
        return 2 + table + sum(size(v[part]) for part in parts)
    if k in ("EX_FinalFunction", "EX_LocalFinalFunction", "EX_CallMath"):
        return 1 + POINTER + sum(map(size, e["Parameters"])) + 1
    if k in ("EX_VirtualFunction", "EX_LocalVirtualFunction"):
        return 1 + NAME + sum(map(size, e["Parameters"])) + 1
    if k in ("EX_Context", "EX_Context_FailSilent"):
        return 1 + size(e["ObjectExpression"]) + OFFSET + POINTER + size(e["ContextExpression"])
    if k == "EX_StructMemberContext":
        return 1 + POINTER + size(e["StructExpression"])
    if k == "EX_DynamicCast":
        return 1 + POINTER + size(e["Target"])
    if k == "EX_PrimitiveCast":
        return 1 + 1 + size(e["Target"])
    if k == "EX_BindDelegate":
        return 1 + NAME + size(e["Delegate"]) + size(e["ObjectTerm"])
    if k == "EX_SwitchValue":
        cases = sum(size(c["CaseIndexValueTerm"]) + OFFSET + size(c["CaseTerm"]) for c in e["Cases"])
        return 1 + 2 + OFFSET + size(e["IndexTerm"]) + cases + size(e["DefaultTerm"])
    if k == "EX_StructConst":
        return 1 + POINTER + 4 + sum(map(size, e["Value"])) + 1
    if k == "EX_SetArray":
        return 1 + size(e["AssigningProperty"]) + sum(map(size, e["Elements"])) + 1
    if k == "EX_ArrayConst":
        return 1 + POINTER + 4 + sum(map(size, e["Elements"])) + 1
    if k in ("EX_IntZero", "EX_IntOne", "EX_NoInterface"):
        return 1
    if k == "EX_IntConstByte":
        return 1 + 1
    if k in ("EX_Int64Const", "EX_UInt64Const"):
        return 1 + 8
    if k == "EX_SkipOffsetConst":
        return 1 + OFFSET
    if k in ("EX_VectorConst", "EX_RotationConst"):
        return 1 + 12
    if k == "EX_TransformConst":
        return 1 + 40
    if k in ("EX_ObjToInterfaceCast", "EX_CrossInterfaceCast", "EX_InterfaceToObjCast"):
        return 1 + POINTER + size(e["Target"])
    if k == "EX_MetaCast":
        return 1 + POINTER + size(e["TargetExpression"])
    if k == "EX_InterfaceContext":
        return 1 + size(e["InterfaceValue"])
    if k == "EX_InstanceDelegate":
        return 1 + NAME
    if k in ("EX_AddMulticastDelegate", "EX_RemoveMulticastDelegate"):
        return 1 + size(e["Delegate"]) + size(e["DelegateToAdd"])
    if k == "EX_ClearMulticastDelegate":
        return 1 + size(e["DelegateToClear"])
    if k == "EX_CallMulticastDelegate":
        return 1 + POINTER + size(e["Delegate"]) + sum(map(size, e["Parameters"])) + 1
    if k == "EX_ArrayGetByRef":
        return 1 + size(e["ArrayVariable"]) + size(e["ArrayIndex"])
    raise SystemExit(f"bytecode expression {k} not supported yet: add it to etlib/blueprint.py")
