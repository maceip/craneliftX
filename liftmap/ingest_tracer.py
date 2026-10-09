#!/usr/bin/env python3
"""
ingest_tracer.py -- static PERFORMANCE lift/drop analyzer for a native binary.

Why this exists (per the project direction):
  We have a heterogeneous core: hot native code + a Cranelift/Pulley side where
  we can DROP lifted functions. The question is WHERE to lift and where to keep
  native for PERFORMANCE (not security):
    * LIFT frequently-executed NETWORK code (per-packet work, parsers, header
      math) -- it runs often, but it is not a super-tight inner loop, so the
      lift/drop boundary overhead is amortized.
    * KEEP NATIVE the super-tight hot loops (the boundary overhead dominates) and
      the orchestration glue (mostly calls; nothing to gain by lifting).

How it works:
  1. Disassembles the ENTIRE binary with Capstone (https://github.com/capstone-engine/capstone,
     a real GitHub-sourced disassembler) at the function level.
  2. Builds an AST/IL per function: instruction nodes with operand trees +
     a control-flow graph (CFG) with intra-function edges.
  3. Scores every function and emits a lift/drop decision -> lift_map.json.

This is the "ingest tracer" half of the multi lift-and-drop story: it DECIDES
where to lift/drop. The companion lift_drop_demo.py performs the actual lift via
the existing ceremony/o2pulley.sh pipeline.
"""
import sys
import os
import re
import json
import subprocess
from capstone import (
    Cs, CS_ARCH_X86, CS_MODE_64,
    CS_GRP_JUMP, CS_GRP_CALL, CS_GRP_RET,
    CS_AC_READ, CS_AC_WRITE,
)
from capstone.x86 import (
    X86_OP_MEM, X86_OP_IMM, X86_OP_REG, X86_REG_INVALID,
)

# ------------------------------------------------------------------ config ---
NET_KEYWORDS = [
    "socket", "recv", "send", "read", "write", "connect", "bind", "listen",
    "accept", "http", "packet", "parse", "serial", "net", "sock", "tcp",
    "udp", "ip_", "ipc", "stream", "htons", "ntoh", "inet", "getaddrinfo",
    "dns", "tls", "ssl", "header", "frame", "proto",
]
# x86-64 System V integer argument registers, in order.
ARG_REGS = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"]
STACK_REGS = {"rsp", "rbp", "esp", "ebp", "sp", "bp"}
ENTRY_NAMES = ["main", "_main", "start", "_start",
               "event_loop", "run", "serve", "loop", "worker"]

LOOP_DENSITY_REJECT = 0.45   # above this -> tight loop -> keep native
CALL_FRACTION_REJECT = 0.40  # above this -> orchestration glue -> keep native
LIFT_THRESHOLD = 0.40        # composite score needed to lift

# x86-64 registers appear at various widths (rax/eax/ax/al). Normalize operand
# register names to their 64-bit base so argument/return detection is width-safe.
REG_NORM = {
    "al": "rax", "ah": "rax", "ax": "rax", "eax": "rax", "rax": "rax",
    "rdi": "rdi", "edi": "rdi", "di": "rdi", "dil": "rdi",
    "rsi": "rsi", "esi": "rsi", "si": "rsi", "sil": "rsi",
    "rdx": "rdx", "edx": "rdx", "dx": "rdx", "dl": "rdx",
    "rcx": "rcx", "ecx": "rcx", "cx": "rcx", "cl": "rcx",
    "r8": "r8", "r8d": "r8", "r8w": "r8", "r8b": "r8",
    "r9": "r9", "r9d": "r9", "r9w": "r9", "r9b": "r9",
    "rsp": "rsp", "esp": "rsp", "sp": "rsp",
    "rbp": "rbp", "ebp": "rbp", "bp": "rbp",
}


def normalize(reg):
    if not reg:
        return ""
    return REG_NORM.get(reg.lower(), reg.lower())


# --------------------------------------------------------------- helpers ---
def run(cmd):
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode != 0:
        sys.stderr.write(p.stderr)
        raise RuntimeError(f"{cmd[0]} failed ({p.returncode})")
    return p.stdout


def parse_objdump_disasm(obj):
    """Return (symbols, insns). symbols: list of (addr, name). insns: list of
    dicts {addr, size, bytes(b''), mnem, op} parsed from `objdump -d`."""
    out = run(["objdump", "-d", obj])
    symbols = []
    insns = []
    sym_re = re.compile(r"^([0-9a-fA-F]+)\s+<(.+)>:\s*$")
    # Same boundary as extract_bytes.py: stop before the mnemonic, or the
    # "da" in "data16" is parsed as another opcode byte.
    ins_re = re.compile(
        r"^\s*([0-9a-fA-F]+):\s+((?:[0-9a-fA-F]{2}[ \t])*[0-9a-fA-F]{2})\b(.*)$"
    )
    for line in out.splitlines():
        m = sym_re.match(line.strip())
        if m:
            symbols.append((int(m.group(1), 16), m.group(2)))
            continue
        m = ins_re.match(line)
        if m:
            addr = int(m.group(1), 16)
            raw = m.group(2).split()
            rest = m.group(3).strip()
            # objdump wraps a long instruction onto a second line that has
            # bytes and an address but no mnemonic. Those bytes belong to the
            # previous instruction; decoding them alone invents `add [rax], al`.
            if not rest and insns:
                extra = bytes(int(x, 16) for x in raw)
                insns[-1]["bytes"] += extra
                insns[-1]["size"] += len(extra)
                continue
            if "\t" in rest:
                mnem, op = rest.split("\t", 1)
            else:
                parts = rest.split(None, 1)
                mnem = parts[0] if parts else ""
                op = parts[1] if len(parts) > 1 else ""
            insns.append({"addr": addr, "size": len(raw), "bytes": bytes(int(x, 16) for x in raw),
                          "mnem": mnem, "op": op.strip()})
    return symbols, insns


def parse_text_relocs(obj):
    """Map a .text offset to the symbol a relocation names.

    An unlinked object encodes `call parse_packet` as `e8 00 00 00 00` plus
    an R_X86_64_PLT32. Without this map every callee looks unresolved and the
    call graph has no edges.
    """
    try:
        out = run(["objdump", "-r", obj])
    except Exception:
        return {}
    relocs = {}
    in_text = False
    for line in out.splitlines():
        if line.startswith("RELOCATION RECORDS FOR"):
            # ELF emits "[.text]:"; Mach-O emits "[__text]:". Both name the
            # code section, so match on the section containing "text".
            in_text = "text" in line.lower()
            continue
        if not in_text:
            continue
        m = re.match(r"^([0-9a-fA-F]+)\s+\S+\s+(\S+)", line.strip())
        if not m:
            continue
        sym = re.split(r"[+-]", m.group(2), maxsplit=1)[0]
        if sym:
            relocs[int(m.group(1), 16)] = sym
    return relocs


def is_padding(mnem):
    words = set(re.split(r"[\s,]+", mnem.lower()))
    return bool(words & {"nop", "nopl", "nopw", "endbr64", "pause"})


def disasm_function(code, start_addr):
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    return list(md.disasm(code, start_addr))


def symbol_at(symbols_sorted, addr):
    """Nearest symbol at or before addr (for resolving call targets)."""
    name = None
    for a, n in symbols_sorted:
        if a <= addr:
            name = n
        else:
            break
    return name


# ------------------------------------------------------------------ analysis ---
def ensure_sample(path):
    """Build sample_network[.o] from the committed sample_network.c so the
    analyzer/demo are reproducible from source. Built for x86_64 so it is
    liftable by remill-lift -arch amd64."""
    if os.path.exists(path):
        return True
    here = os.path.dirname(os.path.abspath(__file__))
    c = os.path.join(here, "sample_network.c")
    if not os.path.exists(c):
        return False
    import subprocess
    cc = os.environ.get("CC", "cc")
    flags = ["-O1", "-fno-inline"]
    if sys.platform == "darwin":
        flags += ["-arch", "x86_64"]
    cmd = [cc] + flags + (["-c", c, "-o", path] if path.endswith(".o")
                          else [c, "-o", path])
    try:
        subprocess.run(cmd, check=True, capture_output=True)
        return os.path.exists(path)
    except Exception as e:  # pragma: no cover
        sys.stderr.write(f"  warn: failed to build sample ({e})\n")
        return False


def analyze_function(name, insns, symbols_sorted, relocs=None):
    if not insns:
        return None
    # Sort by address; trim trailing alignment NOPs that objdump attributes to
    # the preceding function (boundary bleed between functions).
    raw = sorted(insns, key=lambda i: i["addr"])
    while raw and is_padding(raw[-1]["mnem"]):
        raw.pop()
    if not raw:
        return None
    start = raw[0]["addr"]

    # Disassemble each instruction individually with Capstone. Re-disassembling a
    # concatenated byte blob misaligns mid-function (capstone chokes on an
    # internal byte); one-instruction-at-a-time cannot misalign.
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    cs = []
    for r in raw:
        try:
            ins = next(md.disasm(r["bytes"], r["addr"]))
        except Exception:
            continue
        cs.append(ins)
    if not cs:
        return None

    nodes = [i.address for i in cs]
    addr_set = set(nodes)
    adj = {a: set() for a in nodes}
    calls = []            # resolved callee names
    data_mem = False      # touches real (non-stack) data memory
    arg_regs_used = set()
    written_regs = set()
    writes_rax = False
    n_call = 0
    n_jump = 0
    n_ret = 0

    def note_read(rname):
        if rname in ARG_REGS and rname not in written_regs:
            arg_regs_used.add(rname)

    for ins in cs:
        groups = ins.groups
        is_call = CS_GRP_CALL in groups
        is_jump = CS_GRP_JUMP in groups
        is_ret = CS_GRP_RET in groups or ins.mnemonic in ("ret", "retq")

        # CFG: fall-through to next instruction in this function
        nxt = ins.address + ins.size
        if nxt in addr_set:
            adj[ins.address].add(nxt)
        target = None
        for op in ins.operands:
            if op.type == X86_OP_IMM and (is_call or is_jump):
                target = op.imm
            elif op.type == X86_OP_MEM:
                base = normalize(ins.reg_name(op.mem.base)) if op.mem.base != X86_REG_INVALID else ""
                idx = normalize(ins.reg_name(op.mem.index)) if op.mem.index != X86_REG_INVALID else ""
                # a pointer argument used as a base/index still counts as an
                # argument for signature inference
                note_read(base)
                note_read(idx)
                if ins.mnemonic in ("lea", "nop", "nopl", "nopw") or is_padding(ins.mnemonic):
                    continue  # address math or padding, not a data access
                if base in STACK_REGS or idx in STACK_REGS:
                    continue  # stack slot, not data memory
                data_mem = True
            elif op.type == X86_OP_REG:
                rname = normalize(ins.reg_name(op.reg))
                # xor/sub of a register with itself writes zero. Capstone still
                # marks the register read, which would invent a fake argument.
                self_zero = (
                    ins.mnemonic in ("xor", "sub")
                    and len(ins.operands) == 2
                    and ins.operands[0].type == X86_OP_REG
                    and ins.operands[1].type == X86_OP_REG
                    and ins.operands[0].reg == ins.operands[1].reg
                )
                if op.access & CS_AC_READ and not self_zero:
                    note_read(rname)
                if op.access & CS_AC_WRITE or self_zero:
                    written_regs.add(rname)
                    if rname == "rax":
                        writes_rax = True
        if target is not None and target in addr_set:
            adj[ins.address].add(target)
        if is_call:
            n_call += 1
            callee = None
            if relocs:
                for off in range(ins.address, ins.address + ins.size):
                    if off in relocs:
                        callee = relocs[off]
                        break
            if callee is None and target is not None:
                callee = symbol_at(symbols_sorted, target)
            if callee and callee.lstrip("_") != name.lstrip("_"):
                calls.append(callee)
        elif is_jump and target is not None:
            # tail call: a direct jump to another function's address
            callee = symbol_at(symbols_sorted, target)
            if callee and callee != name:
                calls.append(callee)
        if is_jump:
            n_jump += 1
        if is_ret:
            n_ret += 1

    # ---- loop detection via Tarjan SCC (cycles == loops) ----
    loop_nodes = set()
    index = {}
    low = {}
    onstack = {}
    stack = []
    idx = [0]

    def strongconnect(v):
        index[v] = idx[0]
        low[v] = idx[0]
        idx[0] += 1
        stack.append(v)
        onstack[v] = True
        for w in adj.get(v, ()):
            if w not in index:
                strongconnect(w)
                low[v] = min(low[v], low[w])
            elif onstack.get(w):
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                onstack[w] = False
                comp.append(w)
                if w == v:
                    break
            if len(comp) > 1 or v in adj.get(v, ()):
                loop_nodes.update(comp)

    sys.setrecursionlimit(100000)
    for v in nodes:
        if v not in index:
            strongconnect(v)

    n_insn = len(nodes)
    loop_density = (len(loop_nodes) / n_insn) if n_insn else 0.0
    call_fraction = (n_call / n_insn) if n_insn else 0.0

    # ---- network signal ----
    low_name = name.lower().lstrip("_")
    net_raw = 0.0
    for kw in NET_KEYWORDS:
        if kw in low_name:
            net_raw += 2.0
            break
    for c in calls:
        cl = c.lower().lstrip("_")
        for kw in NET_KEYWORDS:
            if kw in cl:
                net_raw += 1.0
                break
    net_score = min(1.0, net_raw / 4.0)

    # ---- build suggested remill signature ----
    used = [r for r in ARG_REGS if r in arg_regs_used]
    ret_reg = "RAX" if writes_rax else "VOID"
    signature = f"{ret_reg}(" + ",".join(r.upper() for r in used) + ")"
    n_args = len(used)
    pure_compute = not data_mem

    return {
        "name": name,
        "addr": start,
        "size_bytes": sum(i.size for i in cs),
        "n_insn": n_insn,
        "n_call": n_call,
        "n_jump": n_jump,
        "n_ret": n_ret,
        "loop_density": round(loop_density, 3),
        "call_fraction": round(call_fraction, 3),
        "network_score": round(net_score, 3),
        "data_mem_access": data_mem,
        "pure_compute": pure_compute,
        "n_args": n_args,
        "signature": signature,
        "calls": calls,
    }


def tarjan_dummy():  # placeholder to keep linter calm about recursion
    pass


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "sample_network"
    out_json = sys.argv[2] if len(sys.argv) > 2 else "lift_map.json"
    if not os.path.exists(binary):
        # try alongside the script
        alt = os.path.join(os.path.dirname(os.path.abspath(__file__)), binary)
        if os.path.exists(alt):
            binary = alt
    if not os.path.exists(binary):
        # reproducible: build from the committed sample_network.c
        if not ensure_sample(binary):
            print(f"error: binary not found and could not build: {binary}",
                  file=sys.stderr)
            return 2

    print(f"[ingest_tracer] binary : {binary}")
    symbols, insns = parse_objdump_disasm(binary)
    relocs = parse_text_relocs(binary)
    print(f"[ingest_tracer] symbols: {len(symbols)}   instructions: {len(insns)}")

    # function boundaries from sorted text symbols
    text_syms = [(a, n) for a, n in symbols if re.match(r"^_?[a-zA-Z]", n)]
    text_syms.sort(key=lambda x: x[0])
    if not text_syms:
        print("error: no symbols found", file=sys.stderr)
        return 2

    funcs = []
    for i, (addr, name) in enumerate(text_syms):
        end = text_syms[i + 1][0] if i + 1 < len(text_syms) else None
        body = [ins for ins in insns if addr <= ins["addr"] and
                (end is None or ins["addr"] < end)]
        # skip the Mach-O header pseudo-symbol
        if name == "__mh_execute_header":
            continue
        finfo = analyze_function(name, body, text_syms, relocs)
        if finfo:
            funcs.append(finfo)

    # ---- build call graph + frequency (BFS from entry roots) ----
    cg = {f["name"]: set(f["calls"]) for f in funcs}
    roots = [n for n in cg if n.lower().lstrip("_") in ENTRY_NAMES]
    if not roots:
        # roots = functions nothing calls (top-level)
        called = set().union(*cg.values()) if cg else set()
        roots = [n for n in cg if n not in called]
    depth = {}
    from collections import deque
    q = deque()
    for r in roots:
        depth[r] = 0
        q.append(r)
    while q:
        u = q.popleft()
        for v in cg.get(u, ()):
            if v not in depth:
                depth[v] = depth[u] + 1
                q.append(v)
    for f in funcs:
        d = depth.get(f["name"], None)
        if d is None:
            f["frequency_score"] = 0.1
            f["reachable"] = False
        else:
            f["frequency_score"] = [1.0, 0.7, 0.5, 0.3, 0.2][min(d, 4)]
            f["reachable"] = True

    # ---- final lift/drop decision (performance-oriented, rule-based) ----
    # We LIFT frequently-executed NETWORK code that is NOT a super-tight loop,
    # and KEEP NATIVE the tight loops / orchestration glue / entry points.
    lift_names, keep_names = [], []
    for f in funcs:
        score = (0.5 * f["network_score"]
                 + 0.3 * f["frequency_score"]
                 - 0.6 * f["loop_density"]
                 + (0.2 if f["pure_compute"] else 0.0))
        f["lift_score"] = round(score, 3)
        # A single remill trace can execute a function that touches memory.
        # It cannot follow calls out of the lifted bytes, so those stay native.
        # Count call instructions, not only resolved names: an unlinked .o
        # has call relocations whose targets are still zero.
        f["liftable_now"] = f["n_call"] == 0 and len(set(f["calls"])) == 0
        reasons = []
        decision = "KEEP_NATIVE"
        if not f["reachable"]:
            reasons.append("not reachable from entry (rarely executed)")
        elif f["name"].lower().lstrip("_") in ENTRY_NAMES:
            reasons.append("program entry point")
        elif f["loop_density"] > LOOP_DENSITY_REJECT:
            reasons.append(f"tight loop (loop_density={f['loop_density']})")
        elif f["n_call"] >= 3 or len(set(f["calls"])) >= 3:
            ncal = max(f["n_call"], len(set(f["calls"])))
            reasons.append(f"orchestration fan-out ({ncal} callees)")
        elif f["network_score"] >= 0.25 or f["frequency_score"] >= 0.5:
            decision = "LIFT"
            reasons.append(f"network_score={f['network_score']}")
            reasons.append(f"loop_density={f['loop_density']} (not tight)")
            if not f["liftable_now"]:
                reasons.append("has callees -> not a single remill trace")
            elif f["pure_compute"]:
                reasons.append("pure-compute -> anvill spec + remill lift")
            else:
                reasons.append("memory-dependent -> anvill spec + remill memory semantics")
        else:
            reasons.append(f"low network/freq signal (net={f['network_score']}, freq={f['frequency_score']})")
        f["decision"] = decision
        f["reason"] = "; ".join(reasons)
        (lift_names if decision == "LIFT" else keep_names).append(f["name"])

    funcs.sort(key=lambda f: (-f["lift_score"], f["name"]))
    report = {
        "binary": os.path.abspath(binary),
        "total_functions": len(funcs),
        "lift": lift_names,
        "keep_native": keep_names,
        "functions": funcs,
    }
    with open(out_json, "w") as fh:
        json.dump(report, fh, indent=2)
    print(f"[ingest_tracer] wrote {out_json}")
    print(f"[ingest_tracer] LIFT        : {lift_names}")
    print(f"[ingest_tracer] KEEP_NATIVE : {keep_names}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
