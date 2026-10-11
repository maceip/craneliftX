# Pipeline operations and live events

Every new ledger event has a stable `group` and `group_order`. The `started`
event includes the complete catalog of IDs, order and display labels. Consumers
can group the log directly; they do not need to parse messages or filenames.
`PipelineGroup` in [groups.rs](../src/groups.rs) owns this contract across the
process runner, trace runner, library stage observer and demo worker.

| Order | Group | Operation and code | Log evidence |
| --- | --- | --- | --- |
| 1 | `run` | CLI setup, options, ledger and live endpoint | `started`, run catalog and configuration |
| 2 | `ingest` | [input::ingest](../src/process/input.rs): read and hash the binary | Input path, exact size/hash, ingest span |
| 3 | `load` | [input::load / memory](../src/process/memory.rs): validate static ELF and map segments | Load span, entry, region permissions and addresses |
| 4 | `initialize` | [Runtime::initialize](../src/process/runtime.rs): guest registers, stacks, syscall state | Initial RIP/RSP, state and stack sizes, budgets |
| 5 | `decode` | [compile::decode](../src/process/compile.rs): iced-x86 basic-block decoding | Decode/harvest events; address, exact bytes and assembly per instruction |
| 6 | `audit` | [audit](../src/audit.rs): single-trace security policy | Audit spans and security results |
| 7 | `lower` | [compile::lower / lower_block](../src/process/lower.rs): x86 semantics to CLIF | Lower span, CLIF size or unsupported-instruction failure |
| 8 | `codegen` | [compile::generate / codegen](../src/codegen.rs): Cranelift RV64 backend | Target, emitted bytes/hash and codegen span |
| 9 | `artifacts` | [compile::persist](../src/process/compile.rs): save x86, CLIF and RV64 | Artifact span and each file's path, size and SHA-256 |
| 10 | `execute` | [Runtime::execute](../src/process/runtime.rs): interpreted RV64, resume at guest RIP | Execute span, periodic progress, block totals and execution summary |
| 11 | `syscall` | [Runtime::syscall / Kernel](../src/process/syscall.rs): guest OS services | Started/completed/failed syscall, arguments, result, duration, network counters |
| 12 | `cache` | [BlockCache](../src/process/cache.rs): compile on demand, reuse and invalidate | Outer `lift` span, `compilation_progress`, invalidation and preserved state |
| 13 | `result` | CLI finalization | Terminal completed/failed result; errors/cancellation in the demo |

These are logical groups, not thirteen mandatory sequential phases. Decode,
lower, codegen and artifacts recur on each reached cache miss; execution and
syscalls alternate. The existing `lift` stage is the outer cache-miss span and
contains those compilation operations. Its elapsed time overlaps its children;
do not add parent and child durations. `group_order` is a display order, while
`seq` is execution order. Process spans use `status`; legacy trace/demo spans
retain `state`, so a shared consumer reads `event.status ?? event.state`.
The isolated process enforces its memory policy in
loading, lowering and checked execution; the single-trace `audit` group does
not imply a separate process audit that was never performed.

The orchestrator is [process/mod.rs](../src/process/mod.rs). Input, compilation,
cache and guest runtime have separate modules. Guest memory, registers and
descriptors belong to the runtime, so cache invalidation preserves a server's
connections and state.

## Receive updates during a run

```sh
cargo build --locked --release --bin isr-process --bin isr-events
target/release/isr-process ./server.elf --events-bind 127.0.0.1:0
```

Stderr prints `ledger: /absolute/run/events.jsonl` and
`events: http://127.0.0.1:PORT/events` before ingestion. Port zero selects a free
port; a fixed loopback port is also accepted. Guest stdout remains available
for the server's own output. The endpoint lasts for that runner invocation.

```sh
curl -N http://127.0.0.1:PORT/events
# Resume after the last event successfully handled by your consumer:
curl -N -H 'Last-Event-ID: 120' http://127.0.0.1:PORT/events
# Equivalent initial cursor:
curl -N 'http://127.0.0.1:PORT/events?after=120'
# Catalog, including groups absent from this particular run:
curl http://127.0.0.1:PORT/groups
```

The endpoint uses [Server-Sent Events](https://html.spec.whatwg.org/multipage/server-sent-events.html).
Each message has `event: ledger`, `id: <seq>`, and one `data:` line containing
the exact JSON event committed to the ledger. `Last-Event-ID` takes precedence
over `after`. Events after the cursor are replayed from disk, then the same
connection follows new records. Keep the cursor together with `run_id`: a new
run has its own sequence. Standard EventSource consumers should call `close()`
on `type: result` so they do not reconnect after completion. Browser use needs
the same origin or an application proxy; this local endpoint does not grant
cross-origin access.

For a retained ledger after the runner exits, or to follow another runner's
JSONL file:

```sh
target/release/isr-events /absolute/run/events.jsonl --bind 127.0.0.1:8878
curl -N 'http://127.0.0.1:8878/events?after=120'
```

`isr-events` remains available until stopped. It serves existing records
unchanged, so old ledgers without groups do not acquire invented metadata.
Embedded callers can hold a `RunLedger::serve_events(address)` guard for the
duration of their run, or use `LiveFeed::from_file(path, address)`.

## Progress and correlation

For each newly compiled block, consumers receive decode, lower, codegen and
artifact start/end or failure events plus a `compilation_progress` record.
Stages and progress carry `block_id` and source `rip`; recompiling the same
address gets a new block ID. `harvest` contains the precise source instructions,
and artifact receipts identify the saved source/CLIF/RV64 bytes. Hot cached
execution emits cumulative `progress` at most once per 250 ms, checked after
each completed block. Syscall start/end records reveal waits and resumptions.

Counters distinguish input size, unique source instructions/bytes lifted,
compilation count, emitted RV64 bytes, executed RV64 instructions, guest block
executions and network bytes. No whole-binary completion percentage is invented:
dynamic reachability is unknown. A large input can contain data or unreachable
code that is never translated. Unix nanosecond timestamps are decimal strings;
use `seq` for ordering and monotonic elapsed fields for durations.

Transport comments (`: keepalive`) arrive roughly every 250 ms while caught up.
They indicate stream liveness, not guest progress. A blocking guest read can
leave only its started syscall and keepalives until data or the deadline arrives.
No terminal `result` means incomplete evidence, not success.

## Delivery and resource limits

The producer appends, flushes and syncs each event, then advances a committed
byte watermark. Live readers cannot publish integrated-run events before that
commit. Each subscriber tails the journal on its own thread. There is no
subscriber queue or socket write on the translation thread. A slow/disconnected
client may lose its stream but can recover from its last handled sequence;
it cannot hold up translation or guest TCP service. Disk persistence remains
part of execution and logging failures still fail the run.

The service binds only to loopback, allows eight concurrent clients, limits
request headers to 8 KiB and individual streamed records to 4 MiB, and uses
bounded read/write waits. On shutdown existing subscribers get a bounded drain
window; a lagging client must replay the retained ledger using `isr-events`.
The write deadline applies to the entire event, including partial writes,
so slowly reading a few bytes at a time cannot keep shutdown waiting.
Records are not dropped from the journal to accommodate clients. Operator-owned
run directories persist until explicitly removed. File followers wait for a
complete JSONL line before emitting it; they detect truncation and close on
malformed records instead of silently skipping data.

## Verify

```sh
make check
make verify-network
make verify-live
```

The Rust integration tests prove pre-completion delivery, exact replay, cursor
precedence, complete-record handling and producer independence from a stalled
reader with a 16 MiB event backlog. The network acceptance consumes live events,
disconnects/reconnects using the last sequence, keeps a second subscriber stalled,
and compares the complete received stream with the durable ledger while a real
freestanding Rust server serves persistent connections through RV64.

The separate large-input acceptance uses a 16 MiB synthetic static ELF with
1,024 arithmetic blocks plus entry/exit. It checks increasing translation
counters and events received before the terminal result. Its receipt explicitly
distinguishes reached source bytes from padding. Both scripts retain consumer
receipt timestamps, runner state, raw events and a JSON report under
`.isr-state/`; CI is configured to run both and retain those artifacts.

The [local acceptance receipt](evidence/live-pipeline-acceptance.json) records
the final runner/source hashes, both probes and standalone replay. The Rust
server returned fifteen correct responses; its first connection survived 107
compilations and two cache invalidations without reconnecting. The large input
translated 132,110 reached source bytes into 4,604,612 RV64 bytes and executed
1,151,148 RV64 instructions. Both live consumers resumed with no gaps or
duplicates and received compilation updates before the terminal result.
Format, Clippy, forty ARM64 tests and forty-three x86-64 tests under Rosetta
passed. These are local results; remote CI has not been exercised here.
