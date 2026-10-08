#!/usr/bin/env python3
"""Build and read Anvill specification protobufs.

Anvill (vendor/anvill) lifts a function from a specification, not from a raw
binary. This module writes that protobuf. `anvill-decompile-spec` decodes it
with Specification::DecodeFromPB, asks remill to lift each instruction, and
runs Anvill's cleanup passes. There is no second spec format.
"""
from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import specification_pb2 as pb  # noqa: E402


def parse_signature(signature: str) -> tuple[str, list[str]]:
    signature = signature.replace(" ", "").upper()
    paren = signature.find("(")
    if paren < 0 or not signature.endswith(")"):
        raise ValueError(f"bad remill signature: {signature}")
    ret = signature[:paren]
    inner = signature[paren + 1 : -1]
    args = [a for a in inner.split(",") if a]
    if ret in ("", "VOID"):
        ret = ""
    return ret, args


def build_spec(symbol: str, code: bytes, signature: str, address: int = 0) -> bytes:
    """Anvill Specification for one already-compiled function."""
    ret, args = parse_signature(signature)
    spec = pb.Specification()
    spec.arch = pb.ARCH_AMD64
    spec.operating_system = pb.OS_LINUX

    fn = spec.functions.add()
    fn.entry_address = address
    fn.func_linkage = pb.FUNCTION_LINKAGE_NORMAL_UNSPECIFIED
    call = fn.callable
    call.calling_convention = pb.CALLING_CONVENTION_AMD64_SYSV
    call.is_variadic = False
    call.is_noreturn = False

    # System V: the return address is the 8 bytes at [RSP] on entry.
    call.return_address.mem.base_reg = "RSP"
    call.return_address.mem.offset = 0
    call.return_stack_pointer.reg.register_name = "RSP"
    call.return_stack_pointer.offset = 8

    for reg in args:
        param = call.parameters.add()
        param.name = reg.lower()
        value = param.repr_var.values.add()
        value.reg.register_name = reg
        param.repr_var.type.base = pb.BT_U64

    if ret:
        returned = getattr(call, "return")
        ret_val = returned.values.add()
        ret_val.reg.register_name = ret
        returned.type.base = pb.BT_U64

    sym = spec.symbols.add()
    sym.name = symbol
    sym.address = address

    mem = spec.memory_ranges.add()
    mem.address = address
    mem.is_writeable = False
    mem.is_executable = True
    mem.values = code
    return spec.SerializeToString()


def load_spec(blob: bytes) -> pb.Specification:
    spec = pb.Specification()
    spec.ParseFromString(blob)
    return spec


def signature_of(spec: pb.Specification) -> str:
    fn = spec.functions[0]
    regs = []
    for param in fn.callable.parameters:
        regs.append(param.repr_var.values[0].reg.register_name)
    returned = getattr(fn.callable, "return")
    if fn.callable.HasField("return") and returned.values:
        ret = returned.values[0].reg.register_name
    else:
        ret = "VOID"
    return f"{ret}(" + ",".join(regs) + ")"


def code_of(spec: pb.Specification) -> bytes:
    for mem in spec.memory_ranges:
        if mem.is_executable and mem.values:
            return bytes(mem.values)
    raise ValueError("anvill spec has no executable bytes")


def symbol_of(spec: pb.Specification) -> str:
    if spec.symbols:
        return spec.symbols[0].name
    return ""
