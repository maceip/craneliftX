# Chunder

An in-process x86-64 → RISC-V ISR engine, with a live demonstration at
[chunder.public.computer](https://chunder.public.computer/).

The active project is the Rust crate at this repository's root:

```text
live x86 RIP → iced-x86 (≤32 instructions) → security audit → CLIF
            → riscv64gc-unknown-none-elf bytes → embedded RV64I → GuestContext
```

For a complete server within the supported static Linux profile, `isr-process`
loads an x86-64 ELF, translates reached basic blocks to RV64, and resumes calls,
branches and syscalls while retaining guest memory and TCP connections.
`make verify-network` builds a real freestanding Rust server and verifies its
responses through RV64, including recompilation on the same open connection.
See [the process runtime and network acceptance guide](docs/PROCESS_RUNTIME.md)
for the supported profile, evidence, and remaining limits.

The [operation groups and live event API](docs/PIPELINE_GROUPS.md) map the code
to thirteen stable log groups. `isr-process --events-bind 127.0.0.1:0` streams
committed events during translation with sequence-based replay. `make verify-live`
exercises this with a 16 MiB input and a stalled subscriber.

`isr-run` remains the single-trace inspection runner: linked x86-64
ELF/Mach-O/PE → selected symbol/address/entry → bounded iced-x86 decode → audit
→ CLIF → checked RV64 execution. It accepts register-only traces and leaves
control-flow boundaries unexecuted.

```sh
cargo run --locked --bin isr-run -- ./program --symbol my_function --set rdi=35
```

Every CLI run writes a timestamped, sequenced ledger under
`.isr-state/pipeline/<run-id>/events.jsonl` (override with `ISR_LEDGER_DIR`).
It records input hashes and addresses, decoded assembly, separate decoded/lifted
counts, stage durations, register results, failures, and artifact hashes/paths.
See [the pipeline and ledger guide](docs/PIPELINE.md) for exact commands,
format, review findings, and the Rust executable acceptance fixture.

```sh
cargo run --locked          # arithmetic + pointer writes, then SYSCALL rejection
make check                 # format, Clippy, tests; warnings are errors
```

Both paths require a 64-bit host and stable Rust. The following live-pointer
API details describe the original trace engine; the process runtime uses
isolated guest mappings and an extended flags context instead.

The trace engine requires a 64-bit host and stable Rust. It does not execute the emitted
RISC-V bytes natively. Its interpreter fetches from a flat `Vec<u8>` at PC zero,
sets `a0` to the host context pointer, and provides a separate 64 KiB aligned
stack for Cranelift's ABI frame/spills. Host x86 code, guest data, and interpreter
stack addresses are separate from offsets into the generated code array.

## Modules

| Module | Responsibility |
| --- | --- |
| `src/context.rs` | Exact `#[repr(C, align(16))]` context, 16 GPRs followed by RIP |
| `src/harvest.rs` | Bounded byte decoder and unsafe live-pointer harvester |
| `src/ingest.rs` | Bounded file reader and x86-64 text selection at file virtual addresses |
| `src/ledger.rs` | Durable JSONL events, SHA-256 artifact receipts, and register serialization |
| `src/audit.rs` | Privilege, stack-pivot, absolute-address, and live RSP adjustment checks |
| `src/lower.rs` | One I64 context argument, SSA register state, direct loads/stores |
| `src/codegen.rs` | Fixed RISC-V target and rejection of unresolved external relocations |
| `src/riscv_exec.rs` | RV64I interpreter, including word operations and ABI prologue/epilogue |
| `src/lib.rs` | Linear pipeline; fallible and panic-on-error entry points |
| `src/main.rs` | Executable end-to-end acceptance demo |
| `src/pipeline.rs` | Typed preparation events and immutable reusable traces; per-context stack rechecks |
| `src/bin/isr-demo.rs` | JSONL demo worker with native/reference checks and real TCP workloads |
| `src/bin/isr-run.rs` | File ingestion and checked single-trace execution with a durable ledger |
| `src/process/` | Static ELF mappings, scalar block lowering, control-flow continuation and TCP syscall bridge |
| `src/bin/isr-process.rs` | Isolated process execution through RV64 with durable per-run evidence |

The original trace API supports MOV register/register, MOV register/immediate,
ADD, SUB, XOR, AND, OR with register/immediate sources, SHL/SHR with immediate
counts, and integer MOV memory loads/stores. Integer operands may be 8, 16, 32,
or 64 bits, including AH/BH/CH/DH. Partial writes, 32-bit zero extension, signed
immediates, wrapping arithmetic, and x86 shift masks are preserved.
Memory operands support a 64-bit base plus displacement. Audited absolute and
RIP-relative MOV operands are also supported. Scaled indexes, 32-bit address
bases, segment overrides, LOCK/REP, SIMD, flag consumers, and other instruction
families return an error rather than being silently skipped.

## Calling the engine

```rust,no_run
use cranelift_isr::{try_randomize_and_execute, GuestContext};

let code: [u8; 5] = [0x48, 0x83, 0xc0, 7, 0xc3]; // add rax,7; ret
let mut ctx = GuestContext {
    rax: 35,
    rip: code.as_ptr() as u64,
    ..GuestContext::default()
};
// No stack adjustments or data accesses in this fixture.
unsafe { try_randomize_and_execute(&mut ctx, 0x1000, 0x2000)?; }
assert_eq!(ctx.rax, 42);
assert_eq!(ctx.rip, code.as_ptr() as u64 + 4); // the unexecuted RET
# Ok::<(), cranelift_isr::EngineError>(())
```

`randomize_and_execute` has the requested panic-on-error behavior;
`try_randomize_and_execute` reports the failed stage as `EngineError`.
Use `try_randomize_and_execute_with_image(..., Some(image_start..image_end))`
when absolute/RIP-relative data accesses should be allowed. The ordinary driver
rejects them. Image bounds are independent of stack bounds and use an exclusive
end; the entire memory operand must fit. Immediate RSP adjustments check the
initial and every resulting RSP against the supplied stack range (its upper end
is a valid empty-stack pointer).

## Original trace API contract

- **Dependencies:** iced-x86, Cranelift codegen/frontend, and target-lexicon remain
  the engine. File ingestion uses `object`; structured ledgers use `serde`,
  `serde_json`, and `sha2`. `Cargo.lock` fixes the resolved versions. iced-x86 needs
  `std` enabled, plus `intel` for the ledger's assembly text. Its
  [feature documentation](https://docs.rs/crate/iced-x86/1.21.0/source/README.md)
  requires either `std` or `no_std`. Transitive dependencies are those required
  by these crates. Cranelift 0.115 uses target-lexicon 0.12 internally, so the
  target string is reparsed across the 0.13/0.12 type boundary.
- **Control-flow exits:** JMP/Jcc/CALL/RET end harvesting immediately and remain
  in the trace for audit. They are not executed. `next_rip`/`ctx.rip` points AT
  that boundary; otherwise it points after the last executed instruction.
  This avoids incorrectly skipping a call, return, or taken branch. SYSCALL,
  SYSENTER, and interrupts remain visible to the audit and are rejected.
- **RFLAGS:** the specified context has no flags. This engine returns exact GPR
  and memory results for the supported trace, but cannot supply flags for native
  resumption at a Jcc or any later flag consumer. It does not install hooks or
  resume native execution.
- **Fixed-width RISC-V:** the triple remains exactly `riscv64gc-unknown-none-elf`.
  Compressed and optional bit-manipulation emission is disabled; uncompressed
  RV64I is a valid subset of GC. Generated ABI stack accesses and literal-pool
  loads are handled. Unknown instructions, malformed fetches, and step-budget
  exhaustion return errors.
- **Rust safety:** APIs accepting live code/data addresses are `unsafe`. The
  caller must keep readable instruction memory alive through the boundary or
  32 complete instructions, and provide valid/exclusive host memory for every
  guest load/store. Harvesting reads incrementally, never a speculative 480-byte
  slice. The byte-slice harvester is safe and reports truncated instructions.
  The audit is a static policy filter, not a sandbox for arbitrary host pointers.
  Runtime errors can leave earlier data stores applied.
- **Randomization scope:** this implements the specified cross-ISA execution
  pipeline. There is no key, entropy source, randomized opcode encoding, or
  cryptographic security claim in this implementation.

## Evidence and checks

`tests/pipeline.rs` covers the complete supported matrix, memory widths and
unaligned offsets, partial registers, retained exit RIP, the 32-instruction cap,
stack aliases/adjustments, image bounds, unsupported forms, and a SYSCALL after
a pending store (neither memory nor context changes). A SYSCALL after an
unsupported IMUL also proves the audit wins before lowering.

`tests/native_x86.rs` lifts pointers to two actual assembly functions and compares
RAX and the entire data buffer against direct x86 execution over 512 vectors each.
It also checks preservation of the other 15 GPRs and the exit RIP. Cases include
overflow, signed immediates, partial-register writes, memory operations, and
masked shifts. These tests run on x86-64 Unix; other hosts
run the portable suite. On Apple Silicon with Rosetta and the Rust x86 target:

```sh
rustup target add --toolchain stable x86_64-apple-darwin
RUSTC="$(rustup which --toolchain stable rustc)" \
  rustup run stable cargo test --locked --target x86_64-apple-darwin --all-targets
```

CI exercises the root crate on Linux x86-64 and macOS ARM64. These are executable
tests, not formal verification or proof for arbitrary x86 binaries.

## Live pipeline UI

Chunder displays all 13 operation groups from the process runtime and consumes
its original ledger events over SSE while execution continues. Open a supported
static x86-64 Linux ELF or run the freestanding Rust TCP server through Cranelift
RV64 and the interpreter. Inspect decoded instructions, generated artifacts,
timestamps, counters, failures, and groups that were not exercised.

See [the pipeline UI guide](docs/PIPELINE_UI.md) for setup, streaming/replay
semantics, and the real browser verification. The older curated three-tier API
is documented separately in [the legacy demo guide](docs/WEB_DEMO.md).

## Previous work

The old LLVM/remill/anvill/Pulley source directories are preserved but are not
part of this crate, its dependency graph, Makefile, or CI. Former root plans,
Makefile, release workflow, and obsolete web analysis components are in [`archived/pre-isr`](archived/pre-isr).

The RV64 process path includes a [lightweight instruction filter](docs/INSTRUCTION_FILTER.md)
for shellcode-oriented screening, with live address-linked findings in the
`audit` group. The dashboard distinguishes static passes, runtime checks, and
rejections; this is not a general shellcode or taint classifier.
