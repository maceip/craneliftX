# Rust TCP server through RV64

`isr-process` runs a static Linux x86-64 ELF from its entry point. It maps the
ELF's load segments and a guest stack, decodes basic blocks with **iced-x86**,
lifts their semantics into CLIF, compiles them with Cranelift's RV64 backend,
and interprets the emitted RV64 instructions. There is no Icicle dependency
and no native x86 execution fallback.

The acceptance server's entry, accept loop, persistent request loop, framing,
payload transformation, checksum, calls, branches, and connection state execute
through this path. The host supplies kernel services through a small Linux
syscall bridge. Guest descriptors stay in one table across lazy compilation and
cache invalidation, so recompiling code does not close or replace sockets.

## Reproduce

Requires stable Rust, the `x86_64-unknown-linux-gnu` Rust target, Python 3, and
an ELF linker (`ld.lld` or Rust's bundled `rust-lld`). No guest Linux VM is needed.

```sh
rustup target add --toolchain stable x86_64-unknown-linux-gnu
make verify-network
# Equivalent:
cargo build --locked --release --bin isr-process
python3 scripts/verify_rv64_network_server.py
```

The script compiles [the Rust server](../tests/fixtures/rv64_network_server.rs)
as a static x86-64 ELF, starts **only `isr-process`**, and checks every response
against an independent Python implementation. Its exit status is nonzero unless
all network, translation, ledger, and artifact checks pass. Each invocation gets
a unique `.isr-state/rv64-network/<id>/report.json` plus the actual ELF,
compiler command/output, runner hash, JSONL ledgers, and x86/CLIF/RV64 artifacts.
CI runs this probe on Linux x86-64 and macOS ARM64 and retains the evidence.

Run another executable within the supported profile:

```sh
target/release/isr-process ./server.elf --ledger-dir .isr-state/process
# Exercise retranslation while preserving all guest state and sockets:
target/release/isr-process ./server.elf --recompile-every-syscalls 25
```

Guest stdout is stdout; the runner prints the ledger location to stderr.
Budgets default to 500 million RV64 instructions and 120 seconds. Use
`--max-rv64-steps` and `--max-seconds` (1–3600) to adjust them. Socket waits are
bounded by the process deadline. Unsupported instructions/syscalls, unmapped
accesses, and exhausted budgets terminate with failure and a durable receipt.

The [original acceptance receipt](evidence/rv64-network-acceptance.json) records
15 verified responses, 74,835 payload bytes, 299 unique lifted x86 instructions
(1,117 source bytes), and 19,181,161 executed RV64 instructions. The first
connection stayed open across two cache invalidations and 108 block compilations,
with twelve verified responses and zero reconnects. Format, Clippy, 36 ARM64
tests, and 39 x86-64 tests under Rosetta passed. Remote CI is configured; these
results are local execution evidence.

The newer [grouped/live-stream acceptance](evidence/live-pipeline-acceptance.json)
repeats the Rust server proof with live subscribers, reconnect replay and a
stalled reader, and separately exercises a 16 MiB input. Its final source also
passes forty ARM64 tests and forty-three x86-64 tests under Rosetta.

## What the acceptance checks

- Fifteen exact responses across two connections, with payloads from 0 to 65,536
  bytes; the first connection serves twelve requests without reconnecting.
- Split TCP headers and payloads, an idle interval, and clean half-close/EOF.
- Per-connection request sequence included in the checksum. Keeping a socket
  alive while losing guest handler state cannot pass this test.
- Cold block compilation and forced cache invalidations while the first socket
  remains open; verified responses both before and after invalidation.
- A third connection sends an oversized frame, which the translated handler
  rejects. The guest then closes its listener and exits with status zero.
- Actual interpreted RV64 step counts reconcile with block execution totals;
  network byte counts reconcile with all sent and received frames.
- Artifact lengths and SHA-256 hashes match the bytes on disk; event sequence
  numbers and monotonic timestamps are checked.
- Replacing the same ELF's entry with UD2 causes a recorded lowering failure,
  no readiness record, and a nonzero runner exit. It cannot fall back to a
  native server.

This is a **freestanding Rust server**, using `core` and direct Linux syscalls.
Application logic is compiler-generated Rust. Assembly is confined to the
Linux entry and syscall ABI shims; there are no hand-written assembly request
handlers, lifting callbacks, or engine imports. Auto-vectorization is disabled
because the current process lowerer supports scalar instructions.

The earlier `std::net` multithreaded Mach-O server probe remains in
`scripts/verify_rust_network_server.py`. It records the old single-trace
runner's limitations and is not the RV64 process acceptance test. An arbitrary
`std`/Tokio executable is **not yet supported** by this runtime.

## Runtime boundary

The loader accepts linked, static, little-endian x86-64 ELF64 executables.
PT_INTERP/PT_DYNAMIC and runtime relocations are rejected, including when
section names are absent. It maps at most 64 MiB, below virtual address 128 MiB,
with a separate 1 MiB guest stack. It rejects overlapping, unreadable, and
writable/executable segments. The initial stack has an empty argv/environment
and an AT_NULL auxiliary-vector terminator.

Guest memory, the 18-word GPR/RIP/RFLAGS state, RV64 code, and the private RV64
stack occupy distinct interpreter address spaces. Generated guest memory
operations check the guest address limit before applying their mapping bias;
the interpreter then checks region bounds and write permissions. Guest values
never become host pointers. Calls and returns use the mapped guest stack.
The existing unsafe live-pointer trace API remains a separate contract.

The scalar lowerer currently supports integer MOV/MOVZX/MOVSX/MOVSXD, LEA,
ADD/SUB/CMP/TEST/AND/OR/XOR, INC/DEC/NEG/NOT, two/three-operand IMUL, immediate
ROL/ROR, 64-bit PUSH/register POP, near CALL/RET/JMP, integer Jcc/CMOVcc/SETcc,
NOP, and the Linux SYSCALL boundary. Arithmetic status flags and partial
register semantics are preserved. Undefined flags have deterministic values;
they are not a compatibility promise. SIMD, floating point, TLS, atomics,
string operations, division and other unimplemented forms fail explicitly.
The RV64 interpreter now supports integer multiply/high-multiply forms needed
by this path, in addition to its existing RV64I support.

The initial syscall profile is IPv4 **loopback TCP**: socket, bind, listen,
getsockname, accept with NULL address output, read, write, close, exit and
exit_group. The bridge owns private guest descriptors and checks every buffer;
stdout/stderr are the only inherited outputs. No files, subprocesses, external
network connections, allocators, thread scheduler, dynamic linker, or general
Linux compatibility layer are provided. `bind`/`listen` use a host TcpListener;
backlog tuning and exact Linux socket-option behavior are not implemented.
The guest server handles connections sequentially; multithreaded servers need
additional runtime services.

## Ledger

See [operation groups and live subscriptions](PIPELINE_GROUPS.md) for the code
map and consumer API. Add `--events-bind 127.0.0.1:0` to receive events while
the binary is translated and executed. All events carry a stable `group`;
compilation progress is emitted per block and hot execution progress every
250 ms. Consumers can reconnect from their last sequence without blocking the
guest runtime. The network acceptance now verifies this live stream alongside
connection continuity, including a disconnected and a stalled subscriber.

Every process run records its full input hash, load mappings, entry address,
options, and host architecture. Each compilation records decoded instruction
addresses/bytes/assembly, lowering and codegen start/end/failure timestamps,
block ID, source length, emitted length/hash, and three artifacts. Blocks end
at the first control-flow/syscall boundary or 32 instructions. Unlike the
single-trace API, the process runtime **executes the boundary** and resumes at
the RIP computed by RV64.

Syscalls have paired started/completed or failed records, guest arguments,
source RIP, elapsed time, result, and cumulative execution counters. Cache
invalidations record the live descriptor count and preserved RIP/RSP.
Per-block counts are flushed at invalidation and exit instead of syncing a
ledger event for every hot-loop iteration. Unique source instructions/bytes,
recompilations, dynamic x86 instruction counts, emitted code bytes, executed
RV64 instructions, syscalls, and network bytes are separate quantities.
Execution counters count completed blocks; a failed block may have partially
modified guest memory. No rollback is promised. Absence of a terminal result
means an incomplete run, never success.

ISA/syscall references: [Intel instruction manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html),
[Linux socket interface](https://man7.org/linux/man-pages/man7/socket.7.html).
