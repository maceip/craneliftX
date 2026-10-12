# Chunder live execution pipeline

The frontend displays the current state of the actual `isr-process` runtime.
It accepts a static Linux x86-64 ELF or starts the repository's freestanding
Rust TCP server. Reached x86 instructions are decoded by **iced-x86**, lowered
to CLIF, compiled by Cranelift to RV64, and executed by the embedded RV64
interpreter. Loopback networking goes through the explicit host syscall bridge.

The supported profile is scalar instructions, static linking, bounded guest
memory, and the implemented Linux syscalls. It does not promise arbitrary Rust
applications, dynamically linked binaries, Tokio, or physical RISC-V execution.

## Run locally

Use stable Rust, Python 3.11+, Node 22+, and an LLVM linker (`ld.lld`, provided
by `lld` on Ubuntu). From the repository root:

```sh
rustup target add --toolchain stable x86_64-unknown-linux-gnu
cargo +stable build --locked --release --bin isr-demo --bin isr-process
python3 scripts/build_web_fixture.py
python3 -m venv .venv
.venv/bin/python -m pip install -r webapp/requirements.txt
npm --prefix webapp ci
npm --prefix webapp run typecheck
npm --prefix webapp run build
.venv/bin/python webapp/server.py --port 8877
```

Open **http://127.0.0.1:8877/**. Select **Run demo** or **Open binary**.
The binary upload limit is 64 MiB. One guest process runs at a time, with a
120-second engine deadline. A file is translated as guest control flow reaches
its instructions; input file length is not a measure of lifted code.

`ISR_PROCESS` overrides `target/release/isr-process`; `ISR_SERVER_FIXTURE`
overrides `target/fixtures/rust-network-server.elf`. `ISR_STATE_DIR` selects the
state root (default `.isr-state`), with these runs under `pipeline-ui/`.
Missing runtime/fixture builds are visible in the UI. Requests do not build code.
`deploy/setup_linux.sh` builds both runtimes, the fixture, and the frontend.
The nginx and Caddy configurations permit the 64 MiB upload size and disable
buffering for the event stream. Local tests exercise nginx; this work does not
establish the state of any public deployment.

## Original events, two replayable streams

```text
isr-process → committed events.jsonl → engine SSE /events?after=N
                                            ↓
                            Python coordinator → SQLite event journal
                                            ↓
                          browser SSE → ordered reducer → 13-group UI
```

The coordinator obtains the group catalog from `isr-process --groups`.
It retains original engine payloads, `run_id`, `seq`, wall-clock nanosecond
timestamp strings, and monotonic elapsed nanoseconds. Engine subscriptions
resume from the last stored sequence after a transport disconnect. Conflicting
replays, sequence gaps, and changed run identities fail instead of advancing
the display. The retained engine journal is reconciled after process exit.

A newly launched run subscribes from the cursor in its POST response, preserving
early receipts even when execution is faster than another snapshot request.
Existing runs first load a bounded snapshot, then consume `event: ledger` over SSE. It applies each event in sequence and publishes batches every 50 ms (or
128 queued events). This batches rendering, not translation. Reconnects use
`Last-Event-ID`; reloads resume from the saved snapshot. A sequence gap triggers
snapshot recovery. Execution states and values never advance from a timer or
estimated percentage; presentation focus has a separate reading pace.

`event: coordinator` carries process lifecycle and host test-client results
separately, without impersonating an engine event or consuming an engine
sequence number. If the coordinator restarts, an unconfirmed run is marked
interrupted; it does not pretend to resume the terminated guest. The UI
distinguishes stream reconnection, saved journals, engine failures, and stopped
runs. An open span remains interrupted after cancellation.

## Operation groups

These are stable grouping identifiers, not a promise of 13 sequential steps.
Nested operations overlap and compilation groups recur for newly reached blocks.

| Order | ID | Operation |
| ---: | --- | --- |
| 1 | `run` | Run setup |
| 2 | `ingest` | Read and identify input |
| 3 | `load` | Validate and map image |
| 4 | `initialize` | Initialize guest state |
| 5 | `decode` | Decode x86 instructions |
| 6 | `audit` | Inspect instruction policy |
| 7 | `lower` | Lower x86 semantics to CLIF |
| 8 | `codegen` | Generate RV64 code |
| 9 | `artifacts` | Persist code artifacts |
| 10 | `execute` | Execute and resume guest |
| 11 | `syscall` | Bridge operating-system services |
| 12 | `cache` | Manage compiled blocks |
| 13 | `result` | Finalize run result |

`audit` consumes actual per-block `security` events from the process runner.
The inspector shows green **Passed**, amber **Runtime checked**, and red
**Rejected** checks with instruction addresses, assembly and reasons. Runtime
stack violations replace the latest audit output with the failure finding.
Older saved runs without inspection events remain **Not exercised**.
See [policy scope](INSTRUCTION_FILTER.md); accepted blocks are not classified
as shellcode-free. Event-only groups are **Observed**; paired
spans can be **Active**, **Completed**, **Failed**, or **Interrupted**. A completed
span can become active again when the next block reaches that operation.

## Connected timeline and motion

The dark launch screen offers **Run demo** and **Open binary**. During a run,
all thirteen groups occupy a connected vertical timeline, centered on one large
operation card. The preceding and following cards remain partly visible and
dimmed. There is no navigation bar or persistent side inspector. **Evidence**
opens an explicit modal containing the original event inspector, counters,
network verification, activity graph and downloadable ledger.
Cards share the same header, illustration and receipt-footer dimensions,
padding, flat elevation and typography. Operation glyphs share one size per
breakpoint; controls share a single height, radius and type size. Motion uses
one accent, cycle duration and receipt-pulse duration across the timeline.

Each group has an operation-specific illustration: input scanning, ELF segment
mapping, register initialization, decoded instruction bytes, policy findings,
x86-to-CLIF and CLIF-to-RV64 flows, artifact receipts, execution, syscall bridging,
compilation history, and the terminal result. The recovered scanner, sphere and
beam motifs use actual input hashes, segment addresses and lengths, instruction
bytes, block IDs, emitted code sizes, artifact hashes, and runtime counters.
Sphere dots correspond one-for-one to observed groups or decoded instructions.
Related decode/CLIF/RV64 values are joined only when their block IDs match.

A new run presents every observed group in catalog order, with at least 1.4
seconds of reading time per card. This is a **stage overview**, not a replay of
execution timing: the ledger and card values continue updating every 50 ms.
If the engine finishes before the overview, the caption immediately reads
**Engine complete · Reviewing recorded stages** and the gauge retains the actual
engine duration. Failed or cancelled runs skip groups with no events. No unseen
group becomes successful to fill the overview.

A repeating illustration indicates a visible, open engine span. A short pulse
marks a newly received receipt or an observed receipt entering stage review.
Terminal runs never show active-span loops. Saved runs opened from history do
not automatically replay animations. Offscreen cards, hidden tabs, disconnected
streams and reduced-motion preferences suppress motion. **Pause motion** also
pauses automatic screen progression, while the ledger keeps updating.

After the initial overview, **Follow live** samples the latest group every
600 ms for executions still running. User scrolling pauses following; selecting
a card or opening Evidence leaves the overview. Playback controls disappear
when execution and stage review have both ended. Pointer hover does not change
card opacity. Readability follows intersection with the middle of the viewport,
so scrolling back makes earlier operations bright without changing engine state.
Completed spans and observed receipts retain a small side notch; missing or
failed operations never receive a completion mark. Keyboard focus stays visible.
The filename and event cursor appear inside Evidence instead of the floating
controls.

Above the seconds dial, a thermometer contains one dot per catalog group. A
light ring marks the most recently updated open engine span, falling back to
the latest emitted group only when no span is open. It is independent of
scroll position and stage review. The ring disappears on disconnect, engine
result, cancellation or terminal coordinator state; each dot retains its actual
recorded status.

Clicking anywhere on a card expands the floating controls with a centered,
warm red/orange **Restart Analysis from {operation}?** confirmation. Cancel or
Escape closes it without stopping the guest. Confirming stops that run if it is
active, preserves its ledger, and creates a new execution of its retained input
bytes. The new timeline begins at the chosen group; required earlier stages
really execute again. The confirmation states this explicitly. This is a fresh
execution, not a CPU/memory/socket checkpoint restore. Another active run is
never stopped to restart a historical run; missing or changed retained inputs
are rejected before stopping anything.

The floating seconds dial uses `run.elapsed_ns`, the latest emitted engine
monotonic timestamp. It does not interpolate missing engine time or imply a
completion percentage. Its one-minute dial wraps every sixty measured seconds.
On completion, the result card displays a multicolored LED-matrix readout of
elapsed seconds, RV64 instruction count, compiled blocks, unique source
instructions/bytes and cache invalidations. TCP verification comes from the
coordinator's real test-client receipts. Missing counters display a dash; no CPU,
RAM, throughput or other uncollected performance metric is invented. LED pixel
patterns are display-font geometry, distinct from instruction/group sphere dots.

The inspector shows emitted source addresses, decoded instructions, mappings,
artifacts, counters, or raw JSON. Selecting a ledger record pins that exact
record; selecting a group restores its latest output. Artifact downloads retain
their engine byte lengths and SHA-256 hashes. Full JSONL is downloadable.

The browser keeps 120 recent events, displays at most 60 ledger rows, and keeps
180 compilation graph samples. Full events remain in SQLite and the engine
journal. Saved runs list the latest 30; on-disk journals are not automatically
pruned. Span timings overlap and must not be summed into wall-clock time. The
elapsed counter is the latest emitted engine time. The legacy report field
`native_x86_instructions_executed` is a default value, not an independent native
execution detector; it is not used as a UI proof metric.

## API

| Request | Result |
| --- | --- |
| `GET /api/pipeline/config` | Runner readiness and authoritative group catalog |
| `POST /api/pipeline/runs` with `{"mode":"rust-server"}` | Start the actual Rust fixture |
| `POST /api/pipeline/runs` with `application/octet-stream` | Upload and execute an ELF; optional encoded `X-Binary-Name` |
| `GET /api/pipeline/runs` | Recent run metadata |
| `GET /api/pipeline/runs/{id}` | Projected snapshot with event cursor |
| `GET /api/pipeline/runs/{id}/events?after=N` | Replay/follow engine and coordinator events |
| `POST /api/pipeline/runs/{id}/cancel` | Stop and reap the guest process |
| `POST /api/pipeline/runs/{id}/restart` with `{"focus_group":"lower"}` | Reexecute retained input; select the requested group in the new view |
| `GET /api/pipeline/runs/{id}/journal` | Original engine JSONL |
| `GET /api/pipeline/runs/{id}/artifacts/{name}` | Original x86/CLIF/RV64 artifact |

`Last-Event-ID` overrides `after`. Invalid cursors are rejected. Replay reads
64 rows at a time; slow browser sends have a 10-second timeout. The API accepts
same-origin writes; guest network destinations remain constrained by the
runtime's loopback syscall profile.

## Exercised verification

```sh
.venv/bin/python -m unittest discover -s webapp/tests -p 'test_*.py'
# Install nginx, and Playwright's browser once:
cd webapp
npx playwright install chromium
npm run test:e2e
```

The browser suite starts the real ASGI server behind the actual nginx config.
It exercises:

- All 13 group controls, changing event cursors before execution completes,
  exact engine-to-browser projection, artifact hashes, and saved-run reload.
- The real Rust server: 15 independently checked responses, 12 on one persistent
  connection, partial headers, an idle interval, EOF, oversized input rejection,
  and guest cache invalidations during the run.
- A 16 MiB uploaded ELF containing 256 arithmetic blocks plus entry/exit blocks:
  258 actual compilations and 33,038 reached source bytes, with updates continuing
  after an nginx restart. Padding is never counted as lifted source.
- Browser offline recovery, unsupported instructions, invalid ELF input,
  cancellation, bounded live rows, desktop/mobile layout, and raw event replay.
- Card confirmation, Escape/Cancel, new Rust-server execution with the identical
  input hash, repeated prerequisite stages, and a new preserved TCP connection.
  Active uploaded runs continue until confirmation, then are reaped and rerun.
- A fast real Rust server run showing all 13 observed groups before the result,
  independently measured engine time during review, paused progression and
  stable neighboring-card opacity on hover.
- Live timeline animations matched to journal sequence numbers, real instruction
  and mapping values, visible previous/current/next cards on desktop and mobile,
  and motion pause/reduced-motion/offline behavior while a real syscall loop runs.
  Scrolling to earlier cards restores readability; the thermometer ring is
  reconciled against the exact ledger at each sampled cursor.

The Python suite additionally cuts a real engine SSE TCP connection and verifies
live resubscription from the committed cursor, exact journal reconciliation,
process reaping, coordinator interruption, and the legacy worker supervisor.
Positive runtime tests use real binaries and emitted events. Synthetic ELF test
inputs are executable test programs, not simulated telemetry.
