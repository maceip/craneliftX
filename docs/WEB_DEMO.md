# Chunder: legacy curated ISR API

The current frontend uses the process runtime and all 13 ledger groups. See
[the live pipeline UI guide](PIPELINE_UI.md) for its setup and streaming contract.
This document describes the retained `/api/runs` three-tier API and its earlier
frontend; its browser controls and acceptance checks do not describe the current UI.

Public website: **https://chunder.public.computer/**.

The browser controls a curated Rust worker. Each worker owns its live x86 code,
GuestContext objects, buffers, TCP sockets, and embedded interpreter. Translation
and execution happen **in that worker process**. The Python coordinator never
passes an address from its own address space to the worker.

```text
browser ── POST /api/runs ──► Starlette run coordinator
                                  │
                                  ├─ start Rust worker once
                                  │      iced-x86 → audit → CLIF → RV64 bytes
                                  │                         → embedded interpreter
                                  │      stdout: flushed JSONL events
                                  │      stderr: bounded retained diagnostics
                                  │
                                  └─ SQLite journal + projected snapshot
                                         │
browser ◄── SSE with sequence IDs ◄───────┘
```

## Workloads

| Tier | Request payload | TCP connections | Dependent passes per record | Trace executions |
| --- | ---: | ---: | ---: | ---: |
| Baby | One 64-byte record | 0 | 1 | 1 |
| Medium | 8 MiB | 4 | 128 | 16,777,216 |
| Large | 512 MiB | 16 | 256 | 2,147,483,648 |

Medium and Large use real `std::net::TcpStream` connections to `127.0.0.1`.
They send a 16 KiB batch and receive the transformed batch. Each 64-byte record
contains seven input words and an output word. A pass incorporates the previous
output into its seed, sums the input words, and applies XOR, shift, ADD, OR, and
memory stores through a translated x86 kernel. Four kernels are prepared once
and reused; passes cycle across them. There are no animation delays or sleeps
in the workload. The reporting thread waits up to 150 ms for worker completion
and emits cumulative counters while computation runs.

The server side checks all 16 context registers and the exit RIP at every pass.
The client computes an independent result and compares every byte of every
returned batch. On an x86_64 worker, the client **also executes the same native
x86 assembly functions** and compares them against the Rust reference at every
pass. Other hosts use the Rust reference and the report explicitly says so.
Baby additionally reports changed registers and before/after memory. Every tier
runs a literal two-byte `0f 05` SYSCALL rejection test and asserts that lowering
and codegen were not invoked for that trace.

Network throughput means verified request payload divided by elapsed execution
time; the network also carries an equally sized response. It is not raw wire
throughput. RISC-V counts are actual interpreted instructions in completed
batches, not host cycles. Live counters can lag work in the current batch;
completion totals are checked. `compilation_ns` sums measured preparation stages;
`execution_ns` includes record generation, engine execution, network I/O, and
verification. Durations depend on hardware; tiers select work, never a timer.

Memory is independent of total payload: each connection owns a 64 KiB interpreter
stack and three 16 KiB data buffers, plus thread/runtime overhead. The UI keeps
120 chart samples and 80 diagnostic messages. The coordinator retains 100 run
journals, pages replay in groups of 128 events, and limits a worker to 12,000
events and 20 minutes. Logs continue draining after the retained diagnostic cap.

## Running locally

From the repository root, with stable Rust, Python 3.11+, and Node 22+:

```sh
cargo build --locked --release --bin isr-demo
python3 -m venv .venv
.venv/bin/python -m pip install -r webapp/requirements.txt
npm --prefix webapp ci
npm --prefix webapp run build
.venv/bin/python webapp/server.py --port 8877
```

Open **http://127.0.0.1:8877**. The server binds only to loopback. The default
worker is `target/release/isr-demo`, and SQLite state is in `.isr-state/`.
Set `ISR_WORKER` and `ISR_STATE_DIR` to override them. Build before starting the
server; requests never trigger a build.

On Apple Silicon, a native ARM worker translates readable x86 byte fixtures and
uses the Rust oracle. For the actual x86 function-pointer/native comparison path,
install Rosetta and the Rust x86 target, then use:

```sh
rustup target add --toolchain stable x86_64-apple-darwin
RUSTC="$(rustup which --toolchain stable rustc)" \
  rustup run stable cargo build --locked --release --target x86_64-apple-darwin --bin isr-demo
ISR_WORKER="$PWD/target/x86_64-apple-darwin/release/isr-demo" \
  .venv/bin/python webapp/server.py --port 8877
```

## Run and recovery contract

- `POST /api/runs` accepts only `{ "tier": "baby|medium|large", "seed": "42" }`.
  Seeds are unsigned 64-bit decimal **strings**. Arbitrary code, pointers,
  destinations, and unbounded work sizes are not accepted.
- `GET /api/runs/{id}` returns the current snapshot, artifacts, worker SHA-256,
  provenance, counters, and journal cursor. Registers and addresses use hex
  strings, preserving all 64 bits in JavaScript.
- `GET /api/runs/{id}/events?after={cursor}` replays persisted events, then tails
  the running worker. `Last-Event-ID` takes precedence on reconnect. Every event
  has schema version, run ID, increasing sequence number, timestamp, and type.
- `POST /api/runs/{id}/cancel` sends cooperative cancellation over stdin, then
  kills an unresponsive worker after two seconds. No partial run passes.
- Browser disconnect/reload does not start or stop computation. The run ID is
  stored in the URL. A snapshot plus its cursor closes the subscribe/replay race;
  duplicate sequences are ignored and gaps trigger snapshot recovery.
- A worker result is withheld until its process exits with code zero. Work size,
  seed, pass count, target, oracle, verification count, and rejection result must
  agree. EOF, missing results, malformed events, errors, and nonzero exits fail.
- A coordinator restart marks unfinished journal entries **interrupted**.
  It does not pretend to resume live pointers. Managed workers cancel when their
  stdin closes; systemd also owns their process group.
- Only one run is admitted at a time. Run state and browser connection state are
  separate. A reconnecting browser never declares completion on its own.

The audit is not a memory sandbox. This demo deliberately exposes only fixed
workloads. It does not install interception hooks, capture arbitrary process
threads, preserve RFLAGS, resume native execution, detect general gadget chains,
or implement secret opcode randomization. See the root README for the engine's
exact boundary.

## Verification and deployment

```sh
make check
npm --prefix webapp run typecheck
npm --prefix webapp run build
npm --prefix webapp run test:server
# Install nginx locally first; tests refuse to silently substitute a fake proxy.
cd webapp
npx playwright install chromium
npm run test:e2e
```

Browser tests start a disposable **real nginx** using `deploy/nginx.conf`, and a
real coordinator/worker. They exercise all three tiers, full Large completion,
mobile overflow, artifacts, saved replay, Last-Event-ID, future cursors, reload,
background/foreground, offline/online, proxy crash/restart, cancellation, and
coordinator crash/restart. Failure tests use separate fixture workers to verify
malformed/missing results, simultaneous stdout/stderr draining, and forced kill.
The latter are failure-injection tests, never demo evidence.

`webapp/test-results/` holds screenshots and the full Large JSON report.
`ISR_KEEP_E2E_STATE=1` retains the disposable SQLite journal for inspection.
CI runs Rust checks on Linux and macOS, then the full proxy/browser suite on Linux
x86_64 and uploads its artifacts. A local run is not proof of a CI or production
deployment; confirm those environments separately.

For Linux, `deploy/setup_linux.sh` builds the worker and frontend. Deploy the
checkout at `/var/www/craneliftx`, install the supplied systemd and nginx configs,
and start the services. systemd owns writable state at `/var/lib/craneliftx`.
Keep **one coordinator process**; multi-process admission and distributed workers
are outside this demo. nginx serves the actual `webapp` root and disables API
buffering. SSE heartbeats arrive every five seconds, below the proxy timeout.

On the shared `vectorheart` host, install `deploy/vectorheart.override.conf` as
`/etc/systemd/system/craneliftx.service.d/deployment.conf`. It uses the dedicated
`craneliftx` account, `/srv/craneliftx/current` release symlink, and loopback port
8879; port 8765 already belongs to mail authentication. Each release includes a
SHA-256 manifest of its exact source files. The existing Caddy proxy must route
the chosen public hostname to this service; do not replace the host's shared
proxy with the standalone nginx example.

After public routing is configured, run the browser suite against that exact URL:

```sh
cd webapp
ISR_BASE_URL=https://chunder.public.computer npm run test:e2e
```

External-site mode verifies Baby, Medium, full Large, live updates, artifacts,
replay, reload, offline recovery, cancellation, and input constraints. It skips
the coordinator crash test and proxy crash injection, which require the
disposable local stack. A tunnel-only check does not verify public DNS, TLS, or
proxy streaming.

## Recorded local acceptance run

On 2026-10-10, the x86_64 worker under Rosetta passed all six browser/proxy tests,
six supervisor tests, and 19 Rust tests (17 on the ARM host). The complete Large
run processed 512 MiB / 8,388,608 records / 2,147,483,648 trace executions in
86.47 seconds, with zero mismatches against native x86 plus the Rust reference.
Medium completed in approximately 2.6 seconds. These are measured on this
workstation, not promises for other machines.

The [recorded run summaries](evidence/2026-10-10-demo.json) include worker and
source hashes, results, and the deliberately cancelled/interrupted cases.
The active local demo at `http://127.0.0.1:8765` retains the full report at
`/?run=b5b16478c69f4f2ca21ccc8f477fb102`. Local evidence is also preserved in
`.isr-state/evidence/`; it is intentionally excluded from Git.

## Remote staging acceptance

Release `20261010T231447Z-887282e67516` is installed on `vectorheart` behind
`craneliftx.service`, listening only on `127.0.0.1:8879`. The native Linux worker
passed 19 Rust tests, six supervisor tests, and five external-site browser tests.
Medium completed in 2.70 seconds and Large in 312.65 seconds, with zero mismatches.
Live browser progress, replay, reload, offline recovery, cancellation, desktop and
mobile rendering, and a separate real systemd crash/restart check passed. Saved
reports survived restart and no worker remained orphaned.

The [remote staging receipt](evidence/2026-10-10-vectorheart.json) records exact
source, worker, asset, and service hashes plus run results. Those browser checks
used a private SSH tunnel before the public hostname was chosen. Public
deployment is recorded separately below.

## Public deployment

Chunder runs on `vectorheart` at `78.141.219.102`. The authoritative Knot zone
already contained `chunder.public.computer A 78.141.219.102`; deployment verified
that record on both nameservers and public resolvers without changing it.

The existing internal service and state paths retain their original names so
saved journals and service ownership remain intact:

| Component | Location |
| --- | --- |
| Release | `/srv/craneliftx/releases/20261011T040216Z-880b009502e2` |
| Active release | `/srv/craneliftx/current` |
| systemd unit | `craneliftx.service` (description: Chunder live ISR demo coordinator) |
| Coordinator | `127.0.0.1:8879` |
| Durable reports | `/var/lib/craneliftx/runs.sqlite` |
| Public proxy | `/etc/caddy/chunder.caddy`, imported by the shared Caddyfile |

`deploy/chunder.caddy` is the deployed site configuration. Caddy terminates HTTPS
with a Let's Encrypt certificate, redirects HTTP to HTTPS, and flushes streamed
events immediately. The browser loads the same compiled assets recorded in the
release manifest. The source, build output, and deployment configuration have
separate recorded hashes.

The original release remains available at
`/srv/craneliftx/releases/20261010T231447Z-887282e67516`. The Caddyfile backup before
the new site was added is
`/etc/caddy/Caddyfile.before-chunder-20261011T040216Z-880b009502e2`. A rollback must
preserve any later edits to the shared proxy configuration and the durable report
database. The deployment added one site route and verified preservation of the
46 existing routes before reloading Caddy.

## Public acceptance run

On 2026-10-11 UTC (October 10 in Los Angeles), five browser tests passed against
**https://chunder.public.computer/** with normal certificate validation. Baby
completed with exact register and memory results. Medium verified 8 MiB over
four real loopback TCP connections in 2.70 seconds. Large verified 512 MiB /
8,388,608 records / 2,147,483,648 trace executions over 16 connections in
309.27 seconds, with zero mismatches against native x86 and the Rust reference.
All three tiers rejected SYSCALL during audit before codegen.

The browser visibly advanced during Large execution. A separate public SSE
sample measured a median 151.68 ms between steady progress events; this measures
one sample's event cadence, not end-to-end latency or a guaranteed SLA. Replay,
reload, background/foreground, offline recovery, cancellation, input constraints,
desktop/mobile layout, branding, and share metadata were checked through HTTPS.

The disposable coordinator crash test was skipped in external-site mode. A
separate public-browser check killed only the demo coordinator's main process:
systemd restarted it, the unfinished run became **Interrupted**, saved Large
results survived, no worker was orphaned, and a new Baby run passed. The shared
Caddy service was not stopped for this check.

Inspect the [saved public Large run](https://chunder.public.computer/?run=cab4d532d06c4776a299d4ad1f1a385d)
and [deployment receipt](evidence/2026-10-11-chunder-public.json). Full reports,
browser screenshots, the HTML test report, and a consistent SQLite backup are
retained under `.isr-state/deploy/`. The release manifest verifies 49 selected
source files; deployed worker and public assets have separate recorded hashes.
This deployment uses the uncommitted workspace snapshot, not a GitHub CI release.
