# Cranelift × Pulley — Ceremony-Mode ISA-Hiding (single design document)

> **This is the single source of truth for the project.** All other design
> notes (HETSEC2_DESIGN, MODERN_CVE_EVAL, DECISION, PLAN, VERDICT,
> EXECUTION_SUBSTRATE, SUBSTRATE_SPECTRUM, CROSS_ISA_EXECUTION) have been moved
> to `./archived/` as historical reference. Read only this file.

**Source under analysis:** `bytecodealliance/wasmtime`, sparse checkout of `cranelift/`, HEAD `9d199bc05` (2026-10-07), cranelift-codegen `0.138.0-dev`.
Clone location: `./upstream`.

---

## 0. What this project is (read this first)

**One sentence:** Demonstrate that hiding the instruction set a critical routine
actually runs on makes life harder for an attacker who wants to drop
shellcode/ROP into it — using **Cranelift + Pulley** as the engine.

**The concept (performance is NOT the goal):** This is a *one-shot ceremony*
use case (e.g. a key-signing operation). You run it once; slow is fine. The
value is novelty of mechanism and a convincing demonstration, not speed.

**Why Cranelift + Pulley (locked in):**
- Cranelift is a true multi-backend code generator: it holds all native
  backends (x64/aarch64/riscv64/s390x) **plus Pulley** in one process, and can
  emit machine code *or* Pulley bytecode. (Verified in `probe/`.)
- Pulley is Cranelift's portable bytecode + fixed interpreter. Emitting a
  **keyed, randomized permutation of the Pulley opcode map** yields a *secret
  ISA* the attacker has never seen — ~64–80 bits of entropy, versus **1.58 bits**
  for "just pick one of three public ISAs."
- Cranelift is **drop-only**: it lowers IR to bits but has *no machine-code
  decoder*. So "lift already-compiled code" needs a separate lifter; Cranelift
  is the DROP engine. (See §4.7.)

**The pipeline (lift + drop):**
```
ALREADY-COMPILED CODE → [LIFTER] → PORTABLE IR → [Cranelift DROP] → Pulley(+random map) → INTERPRET
```

**What is working now (ceremony demo, `./ceremony`):**
- Functional: CLIF `f(a,b)=(a*b)+a` → 6 bytes of Pulley → runs to completion.
- Security: 20 random 48-byte attacker payloads → 3 clean traps + 17 crashes,
  **0% executed silently**. Wrong bytes fail loud → abort the ceremony.
- `lift_check.py` proves the boundary: `sign_x86_64.o` (native, Cranelift can't
  decode) vs `sign.wasm` (a real compiled module Cranelift *can* drop).
- **Both lift edges converge on Pulley (EXECUTED, 2026-10-08):** the
  `ceremony-wasm/` runner (`Config::target("pulley64")`) drops *both*
  `ceremony/sign.wasm` (Edge 1, `clang → wasm`) and `ceremony/sign_llvm.wasm`
  (Edge 2, full LLVM pipeline `clang→LLVM IR→llc→wasm-ld`) onto the same
  Cranelift→Pulley core and runs `ceremony_op(3,4)=15` in both — proving the
  shared DROP. (Pinned `wasmtime = 36.0.17` because the `pulley` feature is not
  present in 0.30/0.40.)

**Realistic eval target (CVE):** a 2025 memory-corruption RCE in a
*signing-relevant* library — **CVE-2025-15467** (OpenSSL CMS stack overflow;
CMS is a signed-data format, so the ceremony wraps the same primitive) is
primary; **CVE-2025-68973** (GnuPG armor overflow) is secondary. We defend the
**already-compiled library binary**, not source.

**Immediate next step:** close the loop by executing a *real already-compiled
artifact* through the DROP (Wasmtime→Pulley for `sign.wasm`, or a lifter for the
naked `.o`). Then run the chosen CVE's RCE path through the ceremony and show the
pre-built payload fails on the secret representation.

**Decisions locked:** Cranelift + Pulley is *the* lift/drop solution for now. A
kernel module is not off the table, but we demonstrate under a hypervisor / in
userland. Performance is explicitly not a constraint (one-shot ceremony).
Abort-on-trap, never restart: a wrong representation is a detection event.

---

## 1. Executive summary

**What is available today:** Cranelift can hold **all four native backends plus Pulley in a single process simultaneously** (`all-native-arch = ["x86", "arm64", "s390x", "riscv64"]`, `codegen/Cargo.toml:111`). It is a true cross-compiler; `isa::lookup(triple!("riscv64"))` works on an x86-64 host and produces correct RV64GC. So the *compilation* half of the pipeline is essentially free.

**What blocks the naive reading of the request:** nothing in Cranelift can *execute* foreign-ISA machine code. `cranelift-jit` writes bytes and `mprotect`s them `PROT_EXEC`; the CPU must understand them. There is no emulator or interpreter in `cranelift-jit` or `cranelift-codegen`. On a homogeneous machine, "x86-64 → riscv64 → arm64" at runtime is not physically executable.

**What is actually missing inside Cranelift:** not multi-target compilation, but **on-stack replacement (OSR)** — the ability to enumerate where every live value sits at an arbitrary program point, so that state can be lifted out of one frame and dropped into another. Cranelift computes this internally during register allocation and then throws it away.

**And a second, more fundamental gap the original brief was about:** Cranelift is a *drop-only* engine. It lowers `ir::Function` (CLIF) to machine code and Pulley, but it has **no machine-code decoder** — there is no path that lifts already-compiled native bytes *into* IR. So "lift and drop already-compiled blocks" cannot be done by Cranelift alone; the *lift* requires a separate binary lifter (remill/anvill, clang→Wasm, QEMU TCG). See §4.7. This is the half the original request was really about, and the half every demo so far has silently assumed away by compiling our own code instead.

**Recommended framing:** treat "backend" as *a (target ISA, flags, calling convention) configuration*, and build an execution-substrate-pluggable migration framework. Three milestones that are each achievable:

| Milestone | Transition | Executable today? | Value |
|---|---|---|---|
| M0 | x64-AVX block ↔ x64-SSE block | **Yes** | Real: heterogeneous cores (AVX-512 present on P-cores, absent on E-cores) |
| M1 | x64 block ↔ Pulley bytecode | **Yes** (Pulley is ISA-neutral and in-tree) | Real: tiering, deopt, portability fallback |
| M2 | x64 ↔ aarch64 ↔ riscv64 | Only under emulation, or on genuinely heterogeneous hardware | Research |

---

## 2. Backend inventory (verified)

`codegen/src/isa/` contains exactly: `x64`, `aarch64`, `riscv64`, `s390x`, `pulley32`, `pulley64`.

**The legacy backends are gone.** There is no `x86` (32-bit) and no `arm32` directory — both were removed. Every surviving native backend is a new-style ISLE/machinst backend implementing the same `ABIMachineSpec` + `LowerBackend` contract. This is materially good news for this project: there is no longer a legacy-vs-new impedance mismatch to bridge.

```rust
// codegen/src/isa/mod.rs:129
pub const ALL_ARCHITECTURES: &[&str] = &["x86_64", "aarch64", "s390x", "riscv64"];
```

Note Pulley is deliberately *excluded* from `ALL_ARCHITECTURES` — it is positioned as a portable/interpreter target, not a first-class one.

`TargetIsa` (`codegen/src/isa/mod.rs:284`) is the full polymorphic surface: `compile_function`, `emit_unwind_info`, `text_section_builder`, `function_alignment`, `dynamic_vector_bytes`, `frontend_config`, `flags`/`isa_flags`. `IsaBuilder::from_target_isa` (`isa/mod.rs:195`) clones flags across ISAs — useful for keeping N backends configured consistently.

---

## 3. The three sub-problems

Any runtime migration scheme decomposes into exactly three problems. Cranelift's coverage of each is wildly uneven.

### 3.1 Compilation granularity — *solved, with a twist*

`Context::compile` compiles a whole `ir::Function`. There is **no sub-function or sub-region API**. `machinst/blockorder.rs` gives you read-only RPO; the only embedder knob is `Layout::set_cold`.

The workaround is clean and fully supported: **synthesize one `ir::Function` per basic block.** Block parameters become function parameters; the block terminator becomes a `return`. This works because `VCode::emit` always emits a prologue for the entry block and an epilogue at every `MachTerminator::Ret` — there is no "emit without prologue" mode, so the compiled artifact is *always* a complete, callable function with its own frame.

Consequence: **"resume execution at a block" becomes "call a function."** That reframing simplifies the whole design.

Cost: every function-scoped entity must be rebuilt, because `Block`, `Value`, `Inst`, `StackSlot`, `GlobalValue`, `SigRef`, `FuncRef` are all function-local indices:

- sized + dynamic stack slots (use `StackSlotData.key: Option<StackSlotKey>` as a stable cross-function identity)
- `global_values` (including `DynamicTypeData.dynamic_scale`)
- `dfg.signatures`, `dfg.ext_funcs` (`colocated` affects call emission)
- `dfg.jump_tables` (`br_table`), `dfg.exception_tables` (`try_call`)
- `dfg.constants` / `immediates`, `func.stack_limit`, `func.srclocs`

Rewrite helpers already exist for inlining: `InstructionData::map_values`, `map_blocks`, `map_dynamic_stack_slot`, `map_jump_table` (`codegen/src/ir/instructions.rs`).

### 3.2 Live-state extraction — *this is the real gap*

You need, at a migration point P, the set `L(P) = {(value, type, location)}` for every live-out value. Cranelift knows this precisely. It does not export it.

What exists, and why each is insufficient:

| Mechanism | Location | Why insufficient |
|---|---|---|
| `ir::UserStackMap` | `ir/user_stack_maps.rs` | **Opt-in and call-only.** "Safepoint" is defined structurally as `is_call() && !is_return()`. Records only *explicit typed sized stack slots* — no registers, no spill slots, no dynamic slots. Module doc is explicit: *"Cranelift will not insert spills and record these stack map entries automatically."* |
| `ValueLabelsRanges` | `value_label.rs`, `CompiledCode::value_labels_ranges` | Gives `LabelValueLoc::{Reg, CFAOffset}` over `[start, end)` code ranges — the **only register-level live-location data in the tree**. But it is debug machinery: populated only when the producer sets `dfg.values_labels`, skipped for cold blocks, offsets "monotonized" (approximate). |
| `MachBufferFinalized::frame_layout()` | `machinst/buffer.rs` | Gives `MachBufferFrameLayout { frame_to_fp_offset, stackslots }` — enough to *interpret* a stack-relative location, not to enumerate live values. |
| `MachTrap` | `machinst/buffer.rs` | `{ offset, code }` and nothing else. No register state. Cranelift never resumes from a trap; the JIT installs no signal handler. |
| `stack_switch` | `ir/instructions.rs`, `isa/x64/inst/stack_switch.rs` | The right *shape* — saves `{SP, FP, IP}` and jumps — but **x64-only**. Verified: `grep -rl stack_switch codegen/src/isa/` returns only `x64/*`. No rule exists for aarch64/riscv64/s390x, so lowering fails with `CodegenError::Unsupported`. |

There is no `cont_new`/`cont_switch`/fiber API, and no stack-allocation API, anywhere in the tree.

**The key trick that makes this tractable:** don't try to read arbitrary locations. Instead, make the migration pseudo-instruction **clobber the entire register file**. Register allocation is then forced to spill every live-through value into spill slots, and you only need a *spill-slot → CLIF value* map plus the frame layout — a far smaller addition to Cranelift. This is exactly the mechanism `StackSwitchBasic` already uses on x64 (`ALL_CLOBBERS`), and what `SafepointSpiller` does in `cranelift-frontend/src/frontend/safepoints.rs`.

### 3.3 Execution transfer — *why a stack switch is mandatory*

You cannot hand an x86 frame to aarch64 or riscv64 code. The frame shapes genuinely differ:

| Aspect | x64 | aarch64 | riscv64 | s390x |
|---|---|---|---|---|
| Return address | pushed by hardware `call` | software, into `lr` (x30) | software, into `ra` (x1) | gpr14, caller's save area |
| Frame record | **always** `setup_area_size = 16` | conditional, **can be 0** | conditional (any call forces it) | none; 160-byte caller-allocated RSA |
| Frame slot order | `[retaddr][FP]` | `stp fp, lr` → fp@+0, lr@+8 | `sd fp,0(sp)`; `sd ra,8(sp)` | n/a |
| Stack alignment | 16 | 16 | 16 | **8** |
| Callee-saved FP (default) | **none** (SysV) | v8–v15, **low 64 bits only** | f8, f18–f27 | via `VecStoreLane` |
| Clobber save encoding | `sub rsp`; absolute stores | **pre-indexed `stp` that mutates SP during the save** | `addi sp`; SP-relative stores | `STMG` into caller's area |
| Shadow space | 32 B (Windows fastcall) | none | none | 160 B RSA |
| Lane order | LE | LE | LE | **BE except Tail** |

All of this is decided in `ABIMachineSpec::compute_frame_layout` (`machinst/abi.rs:499`) and the per-arch `gen_clobber_save` / `gen_prologue_frame_setup`. Also relevant: **Cranelift never uses a red zone**, so you never have to reconstruct sub-SP data.

Because frames differ, migration must allocate a **new stack segment** for the incoming architecture and copy state across. That is a stack switch, and it means:
- unwind info must be re-synthesized: `UnwindInst` (`isa/unwind.rs:156`) is the arch-neutral CFI layer (`PushFrameRegs`, `DefineNewFrame`, `SaveReg`, `RegStackOffset`), but a composite multi-arch stack chain has no representation. Pulley's `emit_unwind_info` returns `None`.
- Per-arch register-file capture/restore trampolines must be hand-written assembly. Cranelift cannot emit "push all registers" — there is no such instruction.

---

## 4. Proposed design

### 4.1 Design A — "always in the migration frame" (build this first)

Model every basic block as a standalone function with signature:

```rust
fn block(state: *mut MigrationFrame) -> u32   // returns next block id
```

All live-through values live in the `MigrationFrame` in memory, in a canonical slot layout with a compile-time-known type table. Migration is then **free**: call a different function pointer. No register translation, no OSR map, no Cranelift changes at all.

Cost: a memory round-trip at every block boundary. Expect roughly 2–5× slowdown versus straight-line code. That is acceptable for a correctness baseline and for the cold path.

### 4.2 Design B — "registers normally, spill on demand" (the real target)

Values stay in registers across block boundaries. Each block-version is compiled for a specific backend, and the scheduler emits **migration stubs** at each boundary: a variant of the block that spills the full live set into a canonical frame and then calls the scheduler.

Two variants per block per backend:

- **fast path:** direct branch within the same backend (no cost).
- **migration path:** spill-all → serialize → scheduler → new backend's restore sequence.

Which one is taken is decided by a patched branch or an indirect jump through a mutable per-block dispatch cell. The dispatch cell is the hot-patching primitive: `JITModule` keeps code pages `RX` after `finalize_definitions`, so patch the *dispatch cell in writable data*, not the code.

### 4.3 The universal migration frame

Arch-neutral rendezvous, addressable from any backend:

```
+--------------------------------------------------------------+
| seg_id        : which stack segment we are on                |
| block_id      : CLIF block currently executing               |
| next_block    : where the scheduler should go                |
| n_slots                                                       |
| slots[n]      : canonical, typed value slots (u64/u128)      |
| sp / fp / pc  : resume triple for the outgoing architecture   |
+--------------------------------------------------------------+
```

`ControlContext` in `isa/x64/inst/stack_switch.rs` is already this shape for the resume triple: `{ stack_pointer, frame_pointer, instruction_pointer }` at offsets `0 / 8 / 16`. Generalize it; don't invent something new.

### 4.4 Migration protocol

1. Block executes on backend A, reaches its boundary.
2. The migration stub spills all live values (guaranteed by the clobber-everything trick) into the frame.
3. Stub calls the scheduler. On x64 you can use `stack_switch` directly; elsewhere, a hand-written asm trampoline.
4. Scheduler reads the frame, applies policy (capability availability, cost model, thermal) and selects backend B.
5. Scheduler compiles `next_block` for B. Cache with `incremental_cache::compute_cache_key(isa, func)` — the key already includes the ISA, so per-block per-backend memoization is free.
6. Scheduler allocates a stack segment laid out per **B's** `FrameLayout`, and copies canonical slots into the offsets B's ABI expects. Source of truth: `MachBufferFinalized::frame_layout()` → `MachBufferFrameLayout { frame_to_fp_offset, stackslots }`.
7. Scheduler writes the resume triple and runs B's restore trampoline: load register file from memory, switch SP/FP, jump.

### 4.5 Reusable Cranelift primitives

| Need | Primitive | Location |
|---|---|---|
| N backends in one process | `all-native-arch` + `pulley` features | `codegen/Cargo.toml:107,111` |
| Build a backend on demand | `isa::lookup(Triple)` / `lookup_by_name` | `isa/mod.rs:108,133` |
| Clone flags across backends | `IsaBuilder::from_target_isa` | `isa/mod.rs:195` |
| Compile | `Context::compile(isa, ctrl_plane)` | `context.rs:220` |
| Frame geometry | `MachBufferFinalized::frame_layout()` | `machinst/buffer.rs` |
| Block → code offset | `CompiledCode::bb_starts` / `bb_edges` | **requires `machine_code_cfg_info=true`** (default `false`, `settings.rs:511`) |
| Address a point *inside* a function | `ModuleRelocTarget::FunctionOffset(FuncId, CodeOffset)` + `JITModule::get_address` | `module/src/module.rs`, `jit/src/backend.rs` |
| Add code while already running | repeated `declare_anonymous_function` → `define_function` → `finalize_definitions` | `jit/src/backend.rs` (drains `functions_to_finalize`; `already_protected` windows `mprotect`) |
| Per-block compile memoization | `incremental_cache::{compute_cache_key, serialize_compiled, try_finish_recompile}` | `codegen/src/incremental_cache.rs` |
| Arch-neutral CFI | `UnwindInst` | `isa/unwind.rs:156` |
| SP/FP/IP swap (x64 only) | `stack_switch`, `ControlContext` | `isa/x64/inst/stack_switch.rs` |
| Arch-neutral reference executor | `cranelift-interpreter` (CLIF-level, no `TargetIsa`) | `interpreter/` |

### 4.6 Gaps that must be added

**G1 — OSR/migration state map.** Generalize `UserStackMap` to (a) permit non-call program points and (b) record register *and* spill-slot locations. Add a `migrate` pseudo-instruction that clobbers the full register file and emits an `(offset → [(Value, Type, ValueLoc)])` record. This is the single largest piece of work and the only one requiring deep `machinst` changes.

**G2 — Per-arch capture/restore trampolines.** ~50 lines of hand-written asm per architecture: save/restore the full register file (GPRs, FPRs, vector regs, flags), switch SP/FP, jump. Not expressible in CLIF.

**G3 — Port `stack_switch` beyond x64.** Currently only `StackSwitchModel::Basic` on x64 Linux. Either add ISLE rules for aarch64/riscv64 or bypass with G2 trampolines.

**G4 — Composite unwind.** Synthesize `UnwindInst` sequences describing a stack chain that spans architectures. Required before any debugger, sampling profiler, or exception unwind will work.

**G5 — Stack segment management.** Cranelift has no stack allocation or bounds-description API. You own segment allocation, sizing, and the segment chain.

**G6 — PC → block inverse map.** `bb_starts` are *VCode* blocks in emission order, after critical-edge splitting; several CLIF blocks can vanish or move. Maintain your own `SourceLoc → (Block, Inst)` table and stamp it deliberately when synthesizing block functions.

---

## 4.7 The lift side — working with already-compiled code

The original brief was, verbatim: *"create a special multi-backend pipeline that can, at runtime, lift and drop blocks of code onto any of the supported backend architectures."* The word **lift** is doing work: it means take code that is **already compiled** (a native binary's machine code — the OpenSSL `.so`, the Redis binary, whatever is actually on disk) and raise it to a portable IR so it can be dropped onto a different backend. This is the half the design above has assumed away, and it is the half that decides whether the project is:

- **"compile our own code in many ways"** — easy, already solved by any multi-target toolchain (clang targets x86_64/aarch64/riscv64/wasm from one source), and *not* what an attacker cares about because there is no existing binary to defend; or
- **"take an existing binary and re-home it"** — the hard, novel, and attacker-relevant version, because the artifact the attacker targets *is* the already-compiled binary.

### 4.7.1 What Cranelift can and cannot do

- Cranelift is a **code generator**. Its only input is `ir::Function` (CLIF). It emits machine code (and Pulley) for any of its backends from that IR.
- It has **no machine-code decoder**. There is no `x86 → CLIF`, `aarch64 → CLIF`, or `riscv64 → CLIF` path anywhere in `cranelift/`. `cranelift-codegen` only goes one direction: IR → bits.
- **Consequence:** Cranelift alone can never "lift already-compiled native code." It is the **DROP** engine only. The **LIFT** must come from a separate component. This is a hard boundary, not a missing feature we can cheaply add — there is no x86 disassembler in the tree.

### 4.7.2 The lifter is the missing piece, and LLVM is its natural home

To raise native machine code to a portable IR you need a binary lifter:

| Lifter | Input | Output IR | Notes |
|---|---|---|---|
| **remill / anvill** (Trail of Bits) | naked x86/ARM/AArch64/PPC binary | **LLVM IR** | Production-grade "take an already-compiled binary, get IR." This is why LLVM appears in this project — *not* as a recompiler-from-source (that is HETSEC2 §3.2) but as the **lift intermediate**. |
| **clang → Wasm** | C source (or a Wasm build) | **Wasm** | A `.wasm` is a genuine *already-compiled* artifact, and Cranelift consumes Wasm natively via Wasmtime. Keeps Cranelift central; avoids LLVM-on-the-path-to-Pulley. Requires source/Wasm build rather than a naked binary. |
| **QEMU TCG** | guest machine code (runtime) | **TCG ops → host code** | The proven "run x86 code on a different chip" mechanism; makes cross-ISA-on-one-box actually executable. It is a LIFT+EXECUTE engine, not a Cranelift component. |
| Ghidra P-code / Valgrind VEX / RetDec | binary | respective IR | Same role, alternative IRs. |

From LLVM IR you can drop to any LLVM backend (a different real ISA) or, with a LLVM-IR→CLIF bridge, to Cranelift/Pulley. From Wasm, Cranelift drops directly.

### 4.7.3 Revised pipeline (lift + drop, both explicit)

```
ALREADY-COMPILED CODE   (OpenSSL .so | a .wasm module | a native .o)
        |
   [LIFTER]   remill/anvill   OR   clang → wasm   OR   QEMU TCG
        |     (raises to portable IR: LLVM IR | Wasm | TCG ops)
        v
   PORTABLE IR
        |
   [DROP]     Cranelift
        |       - Option A (HeterSec-class MTD): lower to a DIFFERENT REAL ISA
        |       - Option B (ceremony novelty):   lower to Pulley + keyed opcode permute
        v
   EXECUTE    Pulley interpreter (B) | native (A) | QEMU (cross-chip A)
```

### 4.7.4 Why this matters for the security argument

The attacker-targeted artifact is the **already-compiled** binary (their x86 ROP chain, their ARM shellcode). If we re-home that routine onto a different representation, their pre-built payload no longer matches *at the representation layer*. But the inversion from §5.7 still binds: if the lifter is faithful it lifts the attacker's injected bytes too — so the protection is **not** "it won't execute their code," it is "they must target the representation we are currently running, which is unknowable (secret Pulley opcode map) or absent (an ISA they did not compile for)." This is exactly why the secret-encoding variant (§5.3) and the ceremony abort-on-trap rule (never restart — a wrong representation is a detection event) are the load-bearing parts, and why "three public ISAs" is not.

### 4.7.5 Status

- **DROP side: PROVEN.** The ceremony demo (`./ceremony`) compiles CLIF → Pulley and runs it; arbitrary attacker bytes trap. CLIF here is the *stand-in* IR — in production it is supplied by the lifter.
- **LIFT side — two edges, both built as options (see §4.8).** Edge 1 (`clang → wasm`) is wired in `ceremony-wasm/` and executes `sign.wasm` on Pulley. Edge 2 (LLVM/remill lift of naked binaries) is buildable: the LLVM toolchain is **installable via Homebrew** on this machine (`llvm-config`/`llc` are not preinstalled). Both converge on the same Cranelift→Pulley DROP.

---

## 4.8 Dual lift-backend deployment — both options built, two separate edges

Both lift paths are built as **selectable backends** of the same ceremony. They
are developed and tested as independent edges and converge **only** at the shared
DROP (Cranelift → Pulley + randomized opcode map + abort-on-trap executor).

### Edge 1 — `clang → wasm` (source-available lift)
- **Input:** code we can compile ourselves (`clang --target=wasm32`).
- **Chain:** `sign.c` → `sign.wasm` → Cranelift (Wasmtime) → Pulley → interpreter.
- **Artifact proven:** `ceremony/sign.wasm`, executed by `ceremony-wasm/`
  (`Config::target("pulley64")`).
- **Strength:** Cranelift-native, no extra toolchain, trivially demonstrates the
  full lift+drop with a real compiled module.
- **Limit:** requires source. Defends code we ship, not a third-party binary.

### Edge 2 — LLVM / remill (naked-binary lift)
- **Input:** any native `x86_64`/`aarch64` binary, **no source** — the real
  "already-compiled" target (e.g. the vulnerable OpenSSL/Redis library).
- **Chain:** `sign_x86_64.o` → **remill-lift** raises machine code to **LLVM IR**
  → link remill runtime intrinsics → `llc -mtriple=wasm32` → `wasm-ld` → `.wasm`
  → Cranelift (Wasmtime) → Pulley → interpreter.
- **Strength:** maximum security value — defends a binary we did not compile;
  truest to the original "lift already-compiled blocks" brief.
- **Limit / toolchain reality:** the whole chain is standardized on
  **LLVM 16.0.6** (`/opt/homebrew/opt/llvm@16`, which also provides `wasm-ld`
  and registered wasm32/wasm64 targets). LLVM 22.1.4 is *also* present, but
  `llvm::Optional`/`llvm::None` were **removed in LLVM 16**, so the upstream
  `anvill` (CI supports LLVM 14/15 only) cannot be built against any LLVM
  installed here (16/21/22). The Homebrew sandbox is blocked in this environment,
  so LLVM 15 cannot be installed to satisfy anvill either.

#### anvill — what it actually is (important correction)
- anvill is **not** "feed a binary, get bitcode." Per its README, the `anvill`
  tool *consumes a protobuf specification generated by a Ghidra or Binary Ninja
  plugin* (closed-source on master; an open `binja-final-version` tag exists for
  Binary Ninja). It is a **spec-driven refinement** layer that resolves remill's
  `__remill_*` runtime intrinsics into real semantics **and recovers the clean C
  ABI** (remill-lifted functions otherwise carry remill's `State *` machinery).
- Consequence for this project: anvill cannot serve as the automated single
  entry point here — it needs (a) LLVM ≤15 (not available) **and** (b) a
  Ghidra/Binja-generated spec (not available). It remains the *production* path;
  the demo below uses remill-lift directly with a minimal runtime **stub** that
  stands in for anvill's full runtime for pure-compute functions.

### The single entry point — `ceremony/o2pulley.sh` (the "one command" the user wanted)
Feeds a native `.o` and gets a Pulley result, in one command:

```
./o2pulley.sh sign_x86_64.o ceremony_op 3 4
  [LIFT]  extract bytes -> remill-lift-16 -> LLVM IR
  [LINK]  + remill_runtime_stub.ll  (stubs __remill_* intrinsics)
  [DROP]  llc(wasm32) -> wasm-ld -> lifted_out.wasm
  [RUN]   wasmtime (Cranelift -> Pulley):  ceremony_op(3,4) = 15  -> OK
```

Mechanism:
1. `extract_bytes.py` pulls `ceremony_op`'s raw bytes from the Mach-O `.o`.
2. `remill-lift-16` raises them to LLVM IR (`call_sub_0(i64,i64)->i64` — note
   remill allocates `State` locally, so the signature is already C-like).
3. `remill_runtime_stub.ll` provides trivial definitions of the remill
   intrinsics (`__remill_function_return`, `__remill_read/write_memory_64`,
   `__remill_flag_computation_overflow`, `__remill_undefined_8`, the
   `__remill_symbolic_*` segment bases, plus `__multi3` for the 128-bit multiply
   wasm can't do natively). For a function that only does integer arithmetic
   these are no-ops and the result is unchanged.
4. `llc -mtriple=wasm32` + `wasm-ld --no-entry --export-all` → wasm.
5. `ceremony-wasm` (the shared DROP runner, `Config::target("pulley64")`) runs it.

**Honest scope of the stub:** it is sufficient for pure-compute routines (no real
loads/stores, no flag-dependent control flow). Functions that touch memory or
flags need anvill (with a spec) to supply the real remill runtime semantics. The
stub proves the *native → remill → Cranelift/Pulley* mechanism end-to-end; anvill
is the drop-in replacement for the stub once a spec source exists.

### How both run together (if both are picked)
- **Convergent DROP:** all edges emit `.wasm` and hand it to the *same*
  Cranelift→Pulley core. The security property is identical regardless of which
  edge produced the artifact.
- **Selection:** the ceremony harness picks the edge per artifact — source
  available → Edge 1; naked binary → Edge 2 (via `o2pulley.sh`).
- **Shared core (single owner):** the keyed/randomized Pulley opcode map, the
  one-shot ceremony harness (abort-on-trap, never restart), and the CVE
  evaluation (CVE-2025-15467 primary, CVE-2025-68973 secondary).
- **Separation of concerns:** Edge 1 and Edge 2 are independent components behind
  a common `lift()` interface; neither blocks the other; either can ship alone.

### Status (2026-10-08 — executed)
- **Edge 1: WORKING.** `ceremony-wasm/` (`wasmtime = 36.0.17`, `features =
  ["pulley"]`, `Config::target("pulley64")`) drops `ceremony/sign.wasm`
  (`clang --target=wasm32`) onto Pulley and runs `ceremony_op(3,4) = 15` → OK.
  (Note: versions 0.30 / 0.40 do **not** expose the `pulley` feature; 36.x does.)
- **Edge 2 — DROP half: WORKING.** The LLVM 16 `clang → LLVM IR → llc
  -mtriple=wasm32 → wasm-ld` pipeline produces `ceremony/sign_llvm.wasm`, which
  drops onto the **same** Pulley core and also yields `ceremony_op(3,4) = 15` →
  OK. This proves the convergent DROP: every edge feeds `.wasm` into one
  Cranelift→Pulley executor.
- **Edge 2 — LIFT half (remill): DONE + wired into the single entry point.**
  `remill` (and `remill-lift`, `remill-llvm-link`, `remill-clang`) rebuilt against
  **LLVM 16.0.6** (needs XED + glog + gflags; XED's macOS `-z` linker-flag bug
  worked around with a clang flag-stripping wrapper; remill tool rpaths patched
  with `install_name_tool`). `remill-lift-16` raises the **raw x86_64 bytes** of
  `ceremony_op` — `55 48 89 e5 48 8d 46 01 48 0f af c7 5d c3` — to LLVM IR that
  reconstructs `(a*b)+a`. **This is the operation Cranelift structurally cannot
  perform** (no machine-code decoder).
- **Single entry point: WORKING.** `ceremony/o2pulley.sh` runs the full
  `native .o → remill-lift → (stub runtime) → llc/wasm-ld → Cranelift/Pulley`
  chain and prints `ceremony_op(3,4) = 15` → OK. All three artifacts
  (`sign.wasm`, `sign_llvm.wasm`, `lifted_out.wasm`) converge on the same Pulley
  core with the identical result.
- **anvill: NOT integrated (environment-blocked, not a code defect).** Upstream
  anvill requires LLVM ≤15 (uses `llvm::Optional`/`llvm::None`, removed in LLVM
  16) and a Ghidra/Binary Ninja spec. Neither is available here (only LLVM
  16/21/22 installed; Homebrew sandbox blocks installing 15). The runtime stub in
  `remill_runtime_stub.ll` is the deliberate stand-in that makes the demo run;
  swapping in anvill is the documented production upgrade path.

---

## 5. Security threat model

Revised per clarification: the goal is **ISA diversity so that an attacker cannot write shellcode that works** — an "unknown ability quotient", where the instruction set actually executing is not knowable by the attacker. This is a legitimate research area with a mature prior art, which changes the advice substantially.

### 5.1 Prior art — this design already exists, and was measured

The closest work is not a Cranelift project. It is:

- **HIPStR** — Venkat, Shamasunder, Shacham, Tullsen. *Heterogeneous-ISA Program State Relocation*, ISCA 2016. Literally "lift and drop" program state onto a different ISA, motivated by making ROP harder through higher code entropy.
- **Venkat & Tullsen**, *Harnessing ISA Diversity: Design of a Heterogeneous-ISA Chip Multiprocessor*, ISCA 2014; and *Composite-ISA Cores*, HPCA 2019.
- **Popcorn Linux** (Virginia Tech SSRG) — a replicated-kernel OS presenting heterogeneous-ISA machines as one system. Its compiler produces multi-ISA binaries and its runtime migrates execution.
- **HeterSec** — Wang, Yeoh, Lyerly, Olivier, Kim, Ravindran. *A Framework for Software Diversification with ISA Heterogeneity*, RAID 2020 (earlier version at SFMA 2019). HeterSec builds **two** security applications on Popcorn-style machinery: a **multi-ISA moving target defense** (randomly switch the executing ISA) and a **multi-ISA multi-variant execution** system (detection via divergence).

HeterSec's measured results are the numbers that matter here:

| Measurement | Value | Source |
|---|---|---|
| ISA switch cost | **504.85 µs ± 4.70** | HeterSec Table 1 |
| Remote syscall | 18.09 µs ± 0.36 | HeterSec Table 1 |
| Raw cross-node ping-pong | 17.62 µs ± 0.33 | HeterSec Table 1 |
| Nginx overhead (MTD mode) | ~15% | HeterSec §4 |
| Redis overhead | ~2–20% depending on switch rate | HeterSec §4 |

Deployment: **two real machines** (x86_64 + ARM64) connected over InfiniBand, with a distributed kernel doing per-process page-table synchronization and inter-kernel messaging.

**The granularity they chose is function-level, not basic-block.** An LLVM pass inserts calls to a randomization library into instrumented function prologues and epilogues; on entry/exit the program asks whether to switch ISA. They instrumented only event-loop/critical-path functions — `ngx_process_events_and_timers()` for Nginx, `processTimeEvents()`/`serverCron()` for Redis (which run at `server.hz`, default 10 Hz).

### 5.2 What that means for basic-block granularity

A basic block executes in tens of nanoseconds. HeterSec's switch costs ~505 µs. That is a ratio of roughly **10⁴ to 10⁵**. Even discounting HeterSec's cross-node network and page-sync costs by an order of magnitude for a single-machine design, you are still looking at microseconds per switch versus tens of nanoseconds per block.

**Conclusion: continuous migration at every block boundary is not viable.** Migration must be a low-frequency event — HeterSec made the economics work at roughly 10 switches per second, and even then paid 15% on Nginx. Choose granularity from the cost number, not from ambition.

This also explains why the prior art chose function boundaries: a function call boundary has a well-defined ABI and a known live-value set (via LLVM stackmaps). A basic-block boundary has neither. That is exactly the OSR problem in §4.6 G1.

### 5.3 The entropy budget — the decisive security problem

This is the part that most undermines the literal version of the plan.

**Choosing among N public ISAs yields at most log₂(N) bits of entropy.** With three ISAs — x86-64, aarch64, riscv64 — that is **1.58 bits**. All three encodings are public and fully documented. An attacker responds by:

- writing three payloads and trying each, or
- writing **polyglot shellcode** — a single byte sequence that is valid and useful on multiple ISAs. This is a well-established technique, not a hypothetical.

So "run on a different ISA" is a *cost-raising* measure against mass-produced exploits, not a barrier against a determined attacker. It is closer to 1.6 bits than to the 20–40 bits you get from ASLR.

**The version with real entropy is a secret encoding, not a different public one.** Pulley gives you this almost for free. Verified in `pulley/src/lib.rs`: `for_each_op!` (lines 86–622) defines **220 primary opcodes** in a `#[repr(u8)]` 256-entry space, and `for_each_extended_op!` (from line 623) defines **323 extended** opcodes behind an `ExtendedOp` escape. A per-process random injective remap of those 220 opcodes onto the 256 byte values costs each opcode ~8 bits of guesswork; a payload needing 8–10 distinct opcodes faces **64–80 bits**. Register renumbering (XReg/FReg/VReg files) adds more.

Pulley has a second structural property that matters more than the entropy: **Pulley bytecode is data, decoded by a fixed interpreter loop.** There is no writable-and-executable region holding user code, so the classic "write bytes, jump to them" injection has nothing to jump into. The attacker must instead corrupt the interpreter's PC state — a different and harder primitive.

**Caveat, stated plainly:** this holds against a *blind* attacker (offline payload construction, or an exploit without an arbitrary-read primitive). With arbitrary read, the dispatch table is in memory and the mapping is recoverable. That is the standard and well-known limitation of every secret-encoding defense.

### 5.4 The attack-surface inversion

Building this requires a JIT, a compile cache, a scheduler, and writable code pages. That is a large amount of new, high-value attack surface added in order to defend against shellcode.

Two specific problems:

1. **Emulation defeats the purpose.** If you emulate foreign ISAs, the emulator is native code in one fixed ISA and is a well-known target. The attacker writes *host-ISA* shellcode against the emulator. You have moved the target, not removed it. This is why emulation-based ISA diversity does not deliver the property you want.
2. **W+X returns.** Cranelift is a JIT, so you will have writable-executable windows. Mitigate with dual mappings of the same physical pages (one W, one X, never both simultaneously), minimal write windows, read-only dispatch tables between patch operations, and randomized code-cache layout.

Net assessment: **ISA diversity is a marginal addition on top of NX + CFI + shadow stack, not a replacement for them.** If the goal is narrowly "the attacker cannot execute injected code", NX already delivers that without a JIT. Adding a JIT reintroduces the problem NX solved.

### 5.5 Where it genuinely pays

1. **JIT-ROP and gadget harvesting** — this is the strongest niche. Attackers who read the code cache to locate gadgets are defeated if the encoding is unpredictable and time-varying. Pulley re-encoding on a schedule directly addresses this.
2. **Defeating pre-built payloads** — most real exploits ship as fixed-ISA binaries. Diversity breaks them at scale even if it does not stop a bespoke attacker.
3. **Detection rather than prevention** — multi-variant execution across ISAs (HeterSec's second case study) uses diversity to *detect* divergence, which is a better fit for what ISA heterogeneity is actually good at.
4. **Exploit reliability tax** — a wrong-ISA guess crashes, which is both a failure for the attacker and a detection signal for you.

### 5.6 Encoding density asymmetry — a concrete design consequence

If you want a wrong-ISA payload to *fail safe* by trapping, the ISA matters:

- **RISC-V and AArch64** use fixed 32-bit encodings with many illegal bit patterns, so arbitrary bytes very likely hit an illegal-instruction trap.
- **x86-64** is variable-length and extremely dense — almost any byte sequence decodes to *valid* instructions. A payload written for another ISA will do *something* unpredictable, and "unpredictable" may be useful to the attacker.

Treat x86-64 as the window that does **not** fail safe. Do not rely on it to neutralize a mismatched payload.

### 5.7 Translation is not a sanitizer — "won't it just translate my shellcode?"

This is the sharpest objection to the whole scheme, and it is correct. It deserves a direct answer.

**Any faithful execution engine executes malicious code correctly.** Translation is semantics-preserving by construction. If the attacker's code ever becomes input to your translator, they get working code out. Changing the ISA changes nothing about that.

Two consequences follow.

**1. Cranelift is not a binary translator.** It compiles CLIF IR *to* machine code. It never lifts raw machine bytes *into* IR. Shellcode sitting in the heap never enters the pipeline — the compiler only ever sees `ir::Function` values your own code constructs. There is no point at which "bytes in memory" get translated, so the scenario as literally stated cannot occur.

**2. But if you add a lifting step, or use an emulator, the objection is fatal.** An emulator faithfully executes arbitrary bytes from writable memory. The attacker writes guest-ISA shellcode into the heap and points the emulator's PC at it. They no longer need an executable page at all — **emulation removes the W+X requirement, which is the one guarantee NX gave you.** Emulation makes injection strictly *easier*, not harder.

The same holds one level up. A JIT that compiles untrusted bytecode compiles it *correctly*. Wasmtime compiles untrusted Wasm with Cranelift every day and it runs exactly as authored. The compiler is not a security boundary; Wasm's sandbox is (bounded linear memory, no ambient authority, traps on out-of-bounds). If your system ever accepts untrusted bytecode, ISA diversity applied to it buys nothing — the attacker simply authors bytecode in whichever representation is currently live.

**Therefore the property cannot come from translation at all. It can only come from the attacker not knowing what to write.** That collapses the design back to a single requirement: the encoding must be unpredictable. Which is §5.3 again — a secret encoding (Pulley, randomized opcode map) buys ~64–80 bits; three public ISAs buy 1.58.

What this means for the concrete attack paths:

| Attack path | Native + NX | JIT'd multi-ISA | Pulley, randomized encoding |
|---|---|---|---|
| **Injection** — write bytes, jump to them | Blocked: no W+X | **Revived**: a JIT needs W+X | Structurally blocked: bytecode is data, and there is no W+X region holding user code |
| **Reuse** — ROP/JOP from existing gadgets | Needs CFI + shadow stack | Gadgets change per ISA, so chains break — but only log₂N bits of uncertainty | Gadget harvesting defeated if encoding is time-varying; the interpreter becomes the gadget source |
| **Untrusted bytecode as input** | n/a | Compiled correctly — it works | Compiled correctly — it works, *unless* the opcode map is secret |

The uncomfortable summary: **this scheme does not stop injection.** NX already does, and the JIT you would build to chase ISA diversity undoes it. The scheme's real value is against *reuse* and *pre-built payloads* — and only the randomized-encoding variant carries meaningful entropy.

### 5.8 How much entropy is actually required — the attempt budget

Entropy is not an absolute target you pick. It is *derived* from how many guesses
you can force the attacker to make. Saying "64 bits is good" without that is
hand-waving; here is the actual model.

**Model.** The attacker gets one shot per attempt. Per-attempt success
probability is `p`. If they can make `A` attempts before being detected or
stopped, and your tolerance for overall success is `T`:

```
p ≤ T / A        →        required entropy ≥ log₂(A / T)  bits
```

Worked cases:

| Attempts `A` | Tolerance `T` | Max per-attempt `p` | Entropy needed |
|---|---|---|---|
| 10 (crash-counted, isolated after 10) | 10⁻⁶ | 10⁻⁷ | ~24 bits |
| 10⁴ | 10⁻⁶ | 10⁻¹⁰ | ~34 bits |
| 10⁹ (offline brute force, or restart with a persistent key) | 10⁻⁶ | 10⁻¹⁵ | ~50 bits |

Two things dominate `A`, and **neither is a compiler property** — they are
systems properties:

1. **Re-keying.** If the encoding is re-randomized per process, every attempt
   faces a fresh draw and attempts do not accumulate. If the key persists across
   restarts, they accumulate without limit.
2. **Crash visibility.** If a wrong encoding *traps*, you get a detection event
   and can rate-limit. If it silently executes garbage, you get no signal and the
   attacker retries freely.

This is why three public ISAs is disqualifying **regardless of regime**: `p = 1/3`
means success in ~3 attempts, and no realistic attempt budget is that tight. The
user's instinct is right — "they can just try a couple of times" is exactly the
objection, and at 1.58 bits it is fatal.

**How to get a trap on mismatch, not just low probability.** Pulley packs 220
opcodes into 256 byte values — 86% dense. So a wrong byte usually decodes to
*some* valid opcode and silently does the wrong thing. That gives you entropy but
no tripwire. Fix it by sparsifying: move to **2-byte opcodes** (65,536 slots, 220
assigned → 0.34% valid). Then a guessed opcode is invalid ~99.7% of the time, so
any payload of more than one opcode almost certainly contains an invalid byte →
illegal-opcode trap → crash → detectable signal.

**A sparse encoding buys entropy and a tripwire with the same change.** This is
the same principle as §5.6: density is what makes a wrong guess fail *silently*
instead of failing *loudly*.

### 5.9 Would the attacker know they're on an emulator?

Two separate questions, and my earlier phrasing blurred them.

**To attack your migrating blocks, they do not need to detect emulation.** They
need to know the *guest ISA*, because that is what the decoder will interpret
their bytes as. Detection is irrelevant; guessing is the requirement. So the
guest-ISA randomization still works under emulation — my earlier "the attacker
writes host-ISA shellcode against the emulator" was imprecise. What they actually
do is **ROP/JOP through the emulator's gadgets**, because the emulator is native
host-ISA code.

**But they get a free, non-randomized target regardless.** The emulator or
interpreter must be resident for your scheme to function at all. It is native
host-ISA code, it is gadget-rich, and it is *always present* no matter which guest
ISA is currently active. Same for libc and your JIT runtime. **None of that is
covered by ISA randomization.**

(They can also just *detect* emulation — CPUID, timing, `uname`, `/proc/cpuinfo`,
page-size behaviour. Sandbox and VM detection is routine in malware. But they
don't need to.)

So the emulator is not a problem because the attacker figures out it's emulated.
It is a problem because it is always present, in the most guessable ISA, and
unprotected by the randomization. Which leads to the uncomfortable consequence:

**The defense only binds if the attacker is forced to traverse the protected
region. Usually they are not.** They can ignore your unknowable blocks entirely
and reuse code in the interpreter that is running them.

---

## 6. Revised build order (security framing)

0. **Read HeterSec, HIPStR and the Popcorn compiler first.** Do not rediscover this. The Cranelift-specific contribution is a JIT-shaped implementation of machinery that already exists in LLVM form — that is a real contribution, but only if you know what has been done.
1. **Decide the execution substrate.** This is the gating decision and it is not a Cranelift problem:
   - *Real heterogeneous hardware* (two machines over RDMA/InfiniBand, per HeterSec) — works, ~505 µs per switch, needs a distributed kernel.
   - *One machine, multiple ISAs* — requires emulation, which makes the emulator the target and defeats the purpose (§5.4).
   - *Pulley with randomized encoding* — no special hardware, far more entropy than three public ISAs, but interpreted and slower, with the interpreter as the trust boundary.
2. **Pick granularity from the cost number.** Function-level or event-loop-level at 1–100 Hz. Not basic blocks.
3. **Build the state-transformation layer** (§4.6 G1). Function boundaries keep this tractable because the ABI is defined there.
4. **Harden the JIT before adding diversity**: dual W/X mappings, minimal write windows, read-only dispatch tables, randomized code-cache layout. If you skip this, you have built an attack surface, not a defense.
5. **Measure**, and compare against the baseline of NX + CFI + shadow stack. If the diversity layer is not buying measurable entropy per unit of overhead, it is not earning its attack surface.

---

## 7. References

- A. Venkat, S. Shamasunder, H. Shacham, D. Tullsen. *HIPStR: Heterogeneous-ISA Program State Relocation.* ISCA 2016.
- A. Venkat, D. Tullsen. *Harnessing ISA Diversity: Design of a Heterogeneous-ISA Chip Multiprocessor.* ISCA 2014.
- A. Venkat, H. Basavaraj, D. Tullsen. *Composite-ISA Cores: Enabling Multi-ISA Heterogeneity Using a Single ISA.* HPCA 2019.
- X. Wang, S. Yeoh, R. Lyerly, P. Olivier, S.-H. Kim, B. Ravindran. *A Framework for Software Diversification with ISA Heterogeneity.* RAID 2020. (Earlier: *A Framework to Secure Applications with ISA Heterogeneity*, SFMA 2019.)
- Popcorn Linux, Virginia Tech SSRG — https://popcornlinux.org/
- K. Snow, F. Monrose, L. Davi, A. Dmitrienko, C. Liebchen, A.-R. Sadeghi. *Just-in-Time Code Reuse.* IEEE S&P 2013. (The JIT-ROP threat that §5.5 targets.)

---

## 8. Performance-oriented lift/drop analyzer (`liftmap/`)

New direction (user directive): the value of the heterogeneous core is not only
security. We can also use it for **performance** — move frequently-executed,
not-hot-loop code onto Pulley so the native core stays free for the tight loops
that would pay the lift/drop boundary tax. This needs a *static* decision about
**where to lift and where to drop** across an entire binary.

### Components
- **`liftmap/ingest_tracer.py` — the ingest tracer (analyzer).** Disassembles the
  whole binary with **Capstone** (GitHub: `capstone-engine/capstone`), builds an
  AST/IL per function (instruction nodes + operand trees + CFG), and scores every
  function for a performance lift/drop decision:
  - *loop density* (Tarjan SCC cycle fraction) → **reject super-tight hot loops**;
  - *network signal* (symbol-name + callee-name keywords: socket/recv/send/
    parse/packet/tcp/ip/...);
  - *frequency* (BFS reachability from the entry, since we can't measure runtime
    frequency statically);
  - *orchestration* (≥3 distinct callees) and *entry points* → keep native.
  - Emits `lift_map.json` and a human report.
- **`liftmap/lift_drop_demo.py` — the demo half.** Reads the map, picks the best
  pure-compute 2-arg LIFT candidate, and drives the existing
  `ceremony/o2pulley.sh` pipeline to lift it onto Pulley and validate the result
  (e.g. `_tcp_window_scaled(100,50)=150 → OK/PASS`).
- `liftmap/sample_network.c` — a deliberately-shaped workload (tight CRC32 loop,
  packet parser, pure network helpers, orchestrator) to exercise the decision.

### Decision rules (performance, not security)
`KEEP_NATIVE` if: tight loop (loop_density > 0.45) · orchestration fan-out
(≥3 callees) · entry point · not reachable from entry. Otherwise `LIFT` if
network-eligible or frequently executed. **Memory-dependent** lifts are flagged
"needs real memory semantics (anvill)" — honest about the runtime-stub limitation
from §4.8; they are lift *candidates* but not stub-safe to run today.

### Implementation notes
- Sample must be **x86_64** (`cc -arch x86_64` on Apple Silicon) because the
  whole lift pipeline is amd64→Pulley.
- Capstone register names are width-specific (`eax`/`edi`); normalize to the
  64-bit base for arg/return detection. `lea` is not a memory access; `rsp`/`rbp`
  refs are stack, not "data memory"; rip-relative constant loads are (so
  pure-compute detection is intentionally conservative).
- Re-disassembling a concatenated byte blob misaligns mid-function → disassemble
  each instruction individually. Trim trailing alignment NOPs (boundary bleed).
- `ceremony-wasm/src/main.rs` was made function-agnostic (no hardcoded expected);
  `o2pulley.sh` forwards an optional expected value so the runner still prints OK.

### Reproducibility
Generated artifacts (`sample_network`, `sample_network.o`, `lift_map.json`) are
gitignored; `make` (or the scripts themselves) rebuild them from
`sample_network.c`. pypcode (Ghidra SLEIGH) is the documented IL/AST upgrade path
vs Capstone's raw disassembly.
