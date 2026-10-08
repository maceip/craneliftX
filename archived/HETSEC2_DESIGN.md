# HeterSec-2: same security property, no Popcorn, faster

Target: reproduce HeterSec's multi-ISA MTD/MVX security property while  
satisfying two hard constraints.

1. **No Popcorn Linux.** No modified kernel, no LLVM fork.
2. **Performance must improve.** Baseline to beat: **504.85 µs ± 4.70** per ISA  
   switch (HeterSec Table 1), and ~15% overhead on Nginx.

---

## 1. Dependency inventory — exactly what Popcorn provides

Extracted from `test/mtd/basic/Makefile` and the README.

### A. Compiler — `/usr/local/popcorn`, ~9 GB, >1 h to build

LLVM/clang fork, `secure-popcorn` branch.

| Component                                                  | Role                                                                    |
| ---------------------------------------------------------- | ----------------------------------------------------------------------- |
| `clang -popcorn-migratable`                                | Emits heterogeneous binaries with migration support                     |
| `check_migrate` calls                                      | Inserted by an LLVM pass into function prologues/epilogues              |
| stackmaps                                                  | Live-state metadata at migration points; validated by `check-stackmaps` |
| `pyalign` + `aligned_linker_script_*.x`                    | Forces **identical code/data addresses across both ISAs**               |
| `gen-stackinfo`                                            | Post-processing that builds stack transformation metadata               |
| `ld.gold`                                                  | Custom link with the aligned linker scripts                             |
| `libmigrate.a`, `libstack-transform.a`, `libstack-depth.a` | Runtime migration + stack rewriting                                     |

### B. Kernel — `popcorn-kernel`, `HeterSec-kernel` branch

| Component                       | Role                                       |
| ------------------------------- | ------------------------------------------ |
| Replicated/distributed kernel   | Presents two machines as one system        |
| **Per-process page table sync** | Synchronized on *every* ISA switch         |
| `sys_hscall` (e.g. syscall 335) | HeterSec's own syscall for MTD/MVX control |
| Migration engine                | `MIGRATE [pid] to N` / `BACKMIG`           |
| `msg_layer/msg_socket.ko`       | Inter-kernel messaging                     |
| `/etc/popcorn/nodes`            | Node list                                  |

### C. musl — built from identical source on both nodes (MVX determinism)

Not really Popcorn-specific. Keep.

### D. Migration-point configuration — the fragile part

`migrate_x86.conf` / `migrate_arm64.conf` contain **raw hexadecimal addresses**  
that the user obtains by disassembling the binary with `chkaddr.sh` and reading  
off "the instruction right after `check_migrate`". Any recompile invalidates  
them. This must go.

---

## 2. Where the 505 µs actually goes

HeterSec's own measurements:

| Operation                | Latency       |
| ------------------------ | ------------- |
| local `getpid()`         | 0.47 µs       |
| remote `getpid()`        | 18.09 µs      |
| raw cross-node ping-pong | 17.62 µs      |
| **ISA switch**           | **504.85 µs** |

The network is only ~18 µs of the 505 µs. The paper attributes the rest to  
"the cross-ISA program **state transformation** and the **page synchronization**."

**~487 µs is software, not wire.** That is the entire optimization target.

---

## 3. Rule 1 — removing Popcorn

### 3.1 Kernel → stock Linux

| Popcorn kernel provides                        | Replacement                                                                                           |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Migration engine                               | Userspace runtime library in the application                                                          |
| `sys_hscall`                                   | Plain library call; no new syscall                                                                    |
| `msg_socket.ko` inter-kernel messaging         | Userspace transport over stock sockets or RDMA verbs                                                  |
| Distributed page tables + sync on every switch | **Deleted, not replaced** — see 3.4                                                                   |
| Syscall forwarding to master node              | Each node services its own syscalls locally (RPC model), or forward over the same userspace transport |

Nothing here requires kernel privileges. That is the point.

### 3.2 Compiler → stock clang

Two routes, in order of preference.

**Route A — out-of-tree LLVM pass plugin (recommended).** Stock clang supports  
`-fpass-plugin=`. Implement the migration-point insertion and stackmap emission  
as a plugin against a distro LLVM. No fork, no 9 GB build, and it tracks upstream  
LLVM as it moves.

**Route B — no compiler change at all.** Use `-finstrument-functions`, the stock  
GCC/Clang hook that emits `__cyg_profile_func_enter` / `__cyg_profile_func_exit`.  
Note the existing Makefile *already* uses `-finstrument-functions` for its  
stack-depth build, so there is in-tree precedent.

Route B works because HeterSec migrates at **function boundaries** anyway, where  
live state is ABI-defined: arguments and return address have known locations per  
calling convention. Full register-level stackmaps are only needed to migrate at  
*arbitrary* points, which HeterSec does not do.

Trade-off: Route A preserves stackmaps and arbitrary-point capability. Route B is  
dramatically simpler and sufficient for function-granularity migration. Start  
with B; add A if arbitrary points are needed.

### 3.3 Kill `pyalign` and the aligned linker scripts

Popcorn forces code and data to identical addresses on both ISAs so that a  
migrated program counter means the same thing on the far side. That constrains  
the linker, fragments the address space, and inflates binaries.

**Replace with an explicit address translation table.** At build time, read both  
binaries' symbol tables and link maps and emit `x86_addr ↔ arm_addr` pairs. At  
migration time, translate the PC through the table. This removes `pyalign`, the  
custom linker scripts, and the alignment constraint entirely — and it composes  
with ASLR if you make the table relative to load base.

### 3.4 The load-bearing change: migrate calls, not processes

This is what makes both rules work at once.

HeterSec migrates the **whole process**, so the entire address space must be  
coherent across nodes — which is *why* it needs distributed page tables and why  
it pays to sync them on every switch.

If instead we migrate at function boundaries as **remote calls**, the state that  
crosses is only:

- the arguments (ABI-defined),
- the return address (translated via 3.3),
- any globals explicitly declared as shared.

No address-space coherence is required, so **page table synchronization  
disappears** — and with it the largest single component of the 505 µs.

This is a real change of programming model: shared global state must be declared.  
For the HeterSec benchmarks (Nginx, Redis, NPB) that is a bounded amount of  
annotation work, and it buys the performance win.

### 3.5 Migration points: symbols, not hex addresses

Replace `chkaddr.sh` + hardcoded hex with symbol-based registration, resolved at  
load time from the ELF symbol table. Optionally a source-level  
`__attribute__((migratable))` or a registration macro. Survives recompilation.

---

## 4. Rule 2 — performance

Ordered by expected impact.

1. **Eliminate page-table sync** (§3.4). Removes the dominant cost. Expected:  
   hundreds of µs → gone.
2. **Precompute the stack transformation offline.** The x86↔arm frame transform  
   for a given migration point is *static* — it depends only on the two binaries,  
   never on runtime values. Emit a transform descriptor at build time; at runtime  
   just apply it (a shaped copy plus fixups) instead of computing it.
3. **Warm everything at startup.** HeterSec observed the first switch is extra  
   expensive because it loads code and builds kernel structures. Pre-load both  
   binaries and pre-register all migration points on both nodes before serving.
4. **One-sided RDMA instead of request/response.** Turns a round trip into a  
   single write plus a poll, and takes the remote CPU off the critical path.  
   Sub-2 µs one-way is routine on the ConnectX-4 class hardware HeterSec used.
5. **Cost-model the switch frequency.** HeterSec showed nbench losing 50% at a  
   mere 20% switch probability. Gate switching on a cost model, not a fixed coin.

**Target: tens of microseconds, i.e. roughly 10–25× better than 504.85 µs.**  
Stated as a target, not a promise — the achievable figure depends on the  
transport and on how much global state the application must share.

---

## 5. What must be re-implemented from scratch

| Popcorn component          | Re-implement?     | Notes                                                                     |
| -------------------------- | ----------------- | ------------------------------------------------------------------------- |
| `check_migrate` insertion  | Yes               | Route B: `-finstrument-functions` + a shim library                        |
| Stackmap generation        | Only for Route A  | Skip for function-granularity                                             |
| `libstack-transform`       | Yes               | The core cross-ISA frame rewriter; build-time descriptors + runtime apply |
| `pyalign` + linker scripts | No                | Deleted; replaced by translation table                                    |
| Kernel migration engine    | Yes, in userspace | Substantially simpler in the RPC model                                    |
| Page table sync            | No                | Deleted by construction                                                   |
| `msg_socket.ko`            | Yes, in userspace | Sockets or RDMA verbs                                                     |
| `sys_hscall`               | No                | Plain library call                                                        |
| musl                       | No                | Keep                                                                      |

---

## 6. Build order

1. **Harness first.** Build a measurement rig that reports per-switch cost  
   broken into *transform* / *serialize* / *transport*. Without this we cannot  
   claim rule 2 is satisfied. Do this before anything else.
2. **Transport layer** — TCP first for correctness, RDMA second for speed.
3. **Userspace migration runtime** with the RPC model, on stock kernels.
4. **Build-time tooling** — symbol-based migration points, address translation  
   table, precomputed transform descriptors.
5. **Port `basic`**, then Redis, then Nginx. Compare against HeterSec's 15%  
   Nginx figure.
6. **Security replication** — re-run the CVE evaluation (see `MODERN_CVE_EVAL.md`)  
   on the new stack.

---

## 7. Honest risks

- **The RPC model constrains the application.** Shared globals must be declared.  
  This is the real cost of removing page-table sync, and it is why this is a  
  different system rather than a drop-in replacement.
- **Stack transformation is still the hard engineering.** Even precomputed, the  
  cross-ISA frame rewriter is the piece most likely to consume schedule.
- **RDMA is not free** — it needs RNICs and pinning. TCP fallback will be slower  
  than the RDMA target; report both.
- **MVX determinism** may still want musl and identical syscall behaviour;  
  removing syscall forwarding could break MVX even while MTD works.

---

## 8. Is the kernel level even a good idea? And where does Cranelift fit?

### 8.1 The kernel is required *by their design*, not by the problem

HeterSec migrates the **whole process**. That forces the kernel to own it:

- the whole address space must be coherent across nodes → distributed page  
  tables (`page_server.c`, `pgtable.h`)
- threads, fds, signal handlers must move → `process_server.c`
- syscalls must be serviced somewhere → `syscall_server.c`
- nodes must talk → `msg_layer`

Once you migrate **calls instead of processes** (§3.4), none of that is needed in  
the kernel. The kernel work isn't a design preference — it's a consequence of the  
migration unit. Change the unit and the kernel has nothing left to do.


So: userland is achievable, but only together with the RPC model. You can't keep  
whole-process migration and drop the kernel.

### 8.2 The HeterSec-2 stack

```
  application C source
        |  compile twice, stock clang
        v
  binary_x86_64   binary_aarch64
        |  build-time tooling emits:
        |    - symbol -> migration point
        |    - x86_addr <-> arm_addr translation table
        |    - precomputed transform descriptors
        v
  libmig (userspace runtime)
        |-- migration points (registered by symbol)
        |-- transform apply (precomputed)
        |-- transport (TCP, then RDMA verbs)
        |-- switch policy
        v
  stock Linux x86_64  <---->  stock Linux aarch64
```

No kernel modules. No custom syscalls. No LLVM fork.

### 8.3 Getting back to Cranelift — two options

**Option A: the Wasm bridge — C → Wasm → Cranelift.**

clang targets wasm32/wasm64; Wasmtime compiles Wasm with Cranelift today on  
stock Linux. This gives runtime per-target compilation in userland.

- *For:* no kernel, no LLVM fork, and you get **many variants** rather than N=2 —  
  recompiling per switch is far stronger than 1.58 bits.
- *Against:* **the Wasm sandbox changes the threat model.** Bounds-checked linear  
  memory already contains most memory-corruption exploitation, so you have solved  
  the problem a different way and ISA diversity adds little on top. And an  
  attacker supplying Wasm gets it compiled correctly (report §5.7). Cranelift also  
  cannot compile C directly — Wasm is the only realistic input path.

**Option B: Cranelift as the transform/trampoline JIT (recommended).**

Keep the application as native C compiled by stock clang for both ISAs. Use  
Cranelift for the piece that genuinely is per-arch-pair and per-migration-point:  
**JIT-compile the state-transform routines and migration trampolines** from the  
precomputed descriptors at startup.

- No C→CLIF frontend needed — you emit CLIF directly from your own descriptors  
  using the cranelift-frontend builder.
- Bounded, well-scoped, and on the performance critical path.
- Requires neither the Wasm detour nor a change to the application's threat model.

### 8.4 Honest framing

**Neither of the two rules requires Cranelift.** The wins come from deleting page  
tables (§3.4) and precomputing transforms (§4.2). Cranelift is the right tool if  
you want runtime variant generation (Option A) or specialized transform code  
(Option B) — but it is optional for "improve the paper."

If you want it in scope, **Option B** is the one that fits HeterSec-2 without  
changing what the system is.

---

## 9. Where we are

Done: full source + kernel submodule cloned; dependency inventory confirmed  
against `kernel/popcorn/` and `msg_layer/`; cost model built and run  
(`hetersec2/bench`), showing the software path is ~84 ns and that essentially all  
of HeterSec's 505 µs is page-table sync plus their runtime transform.

Next: transport layer — TCP for correctness, then RDMA verbs reusing the approach  
already present in `msg_layer/rdma.c`.

Open decision: is Cranelift in scope (§8.3 Option B), or do we keep the port  
minimal and Cranelift-free?
