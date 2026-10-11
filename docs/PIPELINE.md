# Binary pipeline and execution ledger

The active pipeline is **iced-x86 → handwritten x86 semantics in CLIF →
Cranelift's RISC-V backend → embedded RV64 interpreter**. Icicle is not a
current dependency. The old remill/anvill/Pulley directories do not participate
in this root crate's build or execution.

`isr-process` now adds a static ELF process runtime with mapped memory, flags,
branches, calls/returns, and a loopback TCP syscall bridge. The real Rust server
acceptance runs its entire freestanding server through RV64 while recompiling
on an open connection. See [PROCESS_RUNTIME.md](PROCESS_RUNTIME.md) for the
current executable path and its limits. The single-trace API and `isr-run`
contracts below remain intentionally narrower.

## Review and changes

The original entry points consumed a live RIP or embedded byte fixtures. The
web demo selected fixed kernels; it did not upload or load arbitrary binaries.
Preparation exposed stage durations, but not timestamps or decoded instructions.
Execution was unobserved in the library, and the worker could return an error
without closing its execution stage. Its instruction count included the retained
but unexecuted RET.

The demo commands now reject supplied binary paths, unknown flags, missing
values, and duplicate worker options instead of silently running their fixtures.
File ingestion has an explicit `isr-run` command.

The file runner now reads at most 64 MiB, parses a linked little-endian x86-64
ELF/Mach-O/PE image, and selects its entry, an exact text symbol, or a virtual
address in a text section. Symbols bound the selected span when available;
otherwise the next text symbol or section end supplies an explicitly reported
bound. It rejects malformed/wrong-architecture images, relocatable objects,
missing/non-text selections, and relocations in the selected span. Raw x86
bytes require the explicit `--raw` option. Mach-O LC_MAIN entries are resolved
from their file offset into the text virtual address; treating that offset as a
virtual address was found and fixed during real-server verification.

`PreparedTrace::prepare_bytes` decodes only its bounded slice. File addresses
remain virtual addresses in the audit, CLIF, ledger, and resulting RIP. They
are never turned into live code pointers. The existing unsafe live-pointer API
remains available for trusted in-process callers.

A trace ends at the first control-flow boundary or 32 decoded instructions.
The boundary is decoded and audited but not executed. The ledger separates
`decoded_instructions`/`decoded_bytes` from `lifted_instructions`/`lifted_bytes`,
reports the retained boundary and exit RIP, and records unharvested bytes.
Generated code bytes include literal pools; byte count divided by four is not
reported as an instruction count. `riscv_steps` counts instructions actually
fetched during successful interpretation.

The file runner executes **register-only traces**. It rejects guest memory
operands before execution, including low addresses that could otherwise be
confused with the interpreter's literal pool. Independently, the checked
interpreter validates all internal loads/stores against its code, owned stack,
and initialized context fields. Context alignment padding is excluded. The
unsafe in-process path still supports caller-owned host data under its existing
contract; the curated network demo continues using that path.

## Run a binary

```sh
cargo build --locked --release --bins
target/release/isr-run ./program --symbol my_function --set rdi=35 --set rsi=7
# Or an address from the binary's symbol/disassembly output:
target/release/isr-run ./program --address 0x401020 --set rax=35
# Raw instruction bytes (base virtual address 0x100000):
target/release/isr-run ./trace.bin --raw --set rax=35
```

Exact symbol spelling matters: Mach-O normally prefixes C symbols with `_`.
With no selection flag the runner uses the file entry, which often encounters
unsupported startup code. Integers accept decimal or `0x` hexadecimal. Initial
registers default to zero except RSP (`0x2000`); stack-adjustment policy is
`0x1000..=0x2000`. RIP always comes from the input selection. Initial registers
are explicit values, not an automatically constructed process/ABI environment.

The runner prints its ledger path before work and returns nonzero on failure.
`--ledger-dir PATH` overrides `ISR_LEDGER_DIR`, which otherwise defaults to
`.isr-state/pipeline`. `cargo run --locked` also records its built-in acceptance
case and expected SYSCALL rejection in a run ledger.

## Ledger contract

[Pipeline operation groups and the live API](PIPELINE_GROUPS.md) define stable
`group`/`group_order` fields shared by these events and the process runtime.
The process CLI can serve events during translation; `isr-events` can follow or
replay any retained JSONL run. Library `StageEvent` observers also receive a
typed `PipelineGroup`.

Each run gets a new directory; existing artifacts are never overwritten.
`events.jsonl` is appended, flushed, and synced per event. A log creation/write
failure is an execution failure, not a successful result. OS/process termination
can leave an incomplete ledger: **absence of a terminal result is not success**.
Local run directories are retained until explicitly removed by the operator.

Every event includes schema `v`, `run_id`, increasing `seq`, Unix nanoseconds as
an exact decimal string (`timestamp_unix_ns`), and monotonic `run_elapsed_ns`.
Wall time may move if the system clock is adjusted; ordering uses sequence and
durations use monotonic clocks. Stages have started/completed/failed states,
`elapsed_ns`, error text, and stage-specific details:

| Event | Evidence |
| --- | --- |
| `started` | CLI arguments, decoder/backend, host architecture, single-trace scope |
| `input` | Canonical input path, full-file length and SHA-256, even if parsing fails |
| `ingest` stage | Selection, format, section, symbol, file offset, virtual entry, span bound |
| `harvest` stage | Every decoded instruction's address, relative offset, exact bytes, assembly, boundary flag |
| `audit` stage | Stack/image policy or the actual rejection |
| `lower` stage | Lifted counts, CLIF text and byte count |
| `codegen` stage | Target, emitted byte count and SHA-256 |
| `artifact` | Absolute path, size, SHA-256 of the bytes actually written |
| `trace` | Summary, selected and unharvested bytes; `whole_program: false` |
| `execute` stage | Before/after registers as exact hex strings; steps on success; error and potentially partial stores on failure |
| `result` | Completed/failed run and trace execution totals |

The file runner saves `input-x86.bin` (selected span, not the full original
executable), `trace-x86.bin` (decoded span including the boundary), `trace.clif`,
and `trace-rv64.bin`. Failures retain the input and stages/artifacts reached so
far. A lowering/codegen failure cannot be mistaken for an executed trace.
Generated RV64 artifacts are code buffers for this interpreter, not standalone
RISC-V ELF executables.

For the existing demo worker, standalone invocations persist their JSONL events
locally and continue emitting the stdout protocol. `--managed` workers stream
into the coordinator's existing SQLite journal (`.isr-state/runs.sqlite`, or
`ISR_STATE_DIR`); they do not open a second log in the deployment working directory.
Events carry `worker_run_id`, `worker_seq`, worker wall timestamps and monotonic
time. Coordinator receipt `time`/`seq` remain separate. Artifacts are embedded
in the worker's `artifact` events and saved report. Expected SYSCALL rejection
has its own nested stage evidence in the `security` event.

Network runs compile one or four unique traces and reuse them. The result now
reports `compiled_traces`, `unique_lifted_bytes`, `unique_lifted_instructions`,
`trace_executions`, and actual RV64 steps separately from **payload** bytes.
Per-record executions remain counted in bounded progress batches; the worker
does not serialize billions of hot-loop events. Execute failure/cancellation
closes the stage before the terminal error/cancel event.

Library users can attach a `StageEvent` observer to preparation and to
`execute_observed`/`execute_bounded`, or use
`try_randomize_and_execute_observed`. The ordinary library APIs do not create
hidden files. Trusted callers using `execute` in hot loops own their counters
and run-level sink, as the demo worker does.

## Reproduce the Rust executable check

On x86-64 Linux:

```sh
mkdir -p .isr-state/pipeline-evidence
rustc --edition=2021 -C opt-level=2 tests/fixtures/pipeline_input.rs \
  -o .isr-state/pipeline-evidence/rust-input
.isr-state/pipeline-evidence/rust-input
target/release/isr-run .isr-state/pipeline-evidence/rust-input \
  --symbol pipeline_entry --set rdi=35 --set rsi=7
```

On Apple Silicon with the installed Rust x86 target and Rosetta, compile with
`rustup run stable rustc --target x86_64-apple-darwin` and select
`--symbol _pipeline_entry`. The ARM-native runner can ingest and interpret that
x86 file; the original executable runs under Rosetta for the native comparison.

The fixture is a real Rust executable with a deliberately supported naked leaf
function. Its four arithmetic instructions produce `(35 + 7 ^ 85) << 2 = 508`.
The macOS acceptance run decoded five instructions/15 bytes, lifted four/14,
emitted 256 RV64 bytes, executed 62 RV64 steps, and matched native output.
This fixture establishes the complete file-to-state path; it does not establish
support for compiler-generated functions generally.

A separately linked Rust-produced ELF leaf image also passed file-entry ingestion
on the ARM host and returned 508 (64 interpreted RV64 steps). Native comparison
was exercised on the Mach-O executable; the ELF image was interpreted locally.

Tests cover ledger sequencing, exact artifact hashes, boundary/limit accounting,
truncated input, audit rejection before lowering, unsupported lowering, invalid
CLI values, unmapped memory, context padding, and failures before execution.
On x86-64 Unix, an integration test also compiles the Rust fixture, runs its
native result, selects its file symbol, and checks the lifted result. The prior
native differential tests still cover 1,024 vectors across two live functions.

## Remaining runtime boundary

`isr-run` remains a single-trace integer translator. `isr-process` supplies a
mapped-memory runtime, stack semantics, RFLAGS, control-flow continuation and a
limited Linux TCP syscall bridge for static scalar ELF programs. Dynamic
linking, TLS, allocator/thread services and broader instruction/syscall coverage
are still required for arbitrary Rust std/Tokio servers. Unsupported code fails
with its stage and decoded evidence; there is no native execution fallback.
