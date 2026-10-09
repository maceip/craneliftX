# CraneliftX Rearchitecture & Fix Plan

**Audience:** implementation agents (Muse, Codex, Claude Code, or a human).
**Status:** authoritative handoff. Execute against this document.
**Authority:** full permission to cut, replace, merge, and restructure first-party code under `/Users/mac/craneliftX` (see §8 for the do-not-touch list).
**Companion docs (do not rewrite):** `MULTI_BACKEND_MIGRATION.md` (design SSOT), `ENVIRONMENT.md` (toolchain provisioning).

---

## 0. How to use this document

1. Read §1–§3 once. They are evidence, not opinions.
2. Work the packages in §6 in order. P0 is mandatory before any UI polish.
3. Every package has **Acceptance** bullets. Do not mark a package done until they pass.
4. When you disagree with a decision, implement it anyway and note the objection in the PR/commit body. This document overrides ad-hoc taste.
5. After each package, the following must still work:
   - `make demo` exits 0 and prints the MULTI LIFT-AND-DROP SUMMARY (see `ENVIRONMENT.md` §5)
   - `npm --prefix webapp run typecheck`
   - `ci/check_llvm_version.sh`

---

## 1. What this project is (one paragraph)

CraneliftX demonstrates **ceremony-mode ISA hiding**: take an already-compiled native function, lift it through remill/anvill to LLVM IR, drop it onto **Cranelift → Pulley** (optionally with a keyed, randomized opcode map), and interpret it. The security claim is that an attacker who does not know the secret representation cannot drop shellcode/ROP into that routine; wrong bytes trap or crash, never execute silently. A second track (`liftmap/`) decides *which* functions to lift for **performance** (not security), and a local web UI visualizes the analysis. Performance is explicitly not a goal; this is a one-shot ceremony.

---

## 2. Current architecture (as-built)

```
                    ┌─────────────────────────────────────────┐
  make demo         │  liftmap/ingest_tracer.py   (Capstone)   │  DECIDE
                    │  liftmap/placement.py       (keyed)      │
                    │  liftmap/lift_drop_demo.py  (orchestrate)│
                    └───────────────┬─────────────────────────┘
                                    │ lift_map.json / lift_plan.json
                                    v
                    ┌─────────────────────────────────────────┐
                    │  ceremony/o2pulley.sh   (bash CLI glue)  │  LIFT
                    │  pipeline/lift_drop.py  (590 LOC)        │
                    │  pipeline/anvill_spec.py + specification_pb2
                    │  vendor/anvill + vendor/remill (LLVM 20) │
                    └───────────────┬─────────────────────────┘
                                    │ wasm / linked bc
                                    v
                    ┌─────────────────────────────────────────┐
                    │  ceremony-wasm/  (wasmtime → Pulley)     │  DROP
                    │  optional: qemu-riscv64 dual target      │
                    └─────────────────────────────────────────┘

  make web  →  webapp/server.py  (HTTP+SSE)  →  ingest_tracer + placement
            →  React UI (webapp/src/)        →  FABRICATED dual-run (see P0)
```

**Side crates (not on `make demo`):** `ceremony/` (CLIF→Pulley concept demo + trap trials), `probe/` (multi-backend capability smoke).

**Out of live path:** `hetersec2/` (RAID’20 prior art, gitignored), `archived/` (old design notes), `upstream/` (cranelift/wasmtime checkout), `vendor/` (remill/anvill trees), `build/` (lifter build tree).

---

## 3. Findings (severity-ranked)

### F1 — P0 INTEGRITY: the web UI fabricates execution results

`webapp/server.py:246–470` hardcodes a `VECTORS` dict (fake `native_cycles`, `pulley_cycles`, invented `native_asm` / `pulley_bc` traces), then emits:

- `"verified": true`, `"bit_for_bit_identical": true`, `"equivalence": true`
- `"summary": "Dual execution completed: 100% bit-for-bit equivalence..."`

**No dual run happens.** The browser shows a side-by-side “Native vs Pulley” panel that is theater. `lift_results.json` is only used to overwrite a couple of expected values if present.

This is the highest-priority fix. Research code that claims verification it did not perform is worse than no UI.

**Evidence:** `webapp/server.py:246` (“Synthetic & real test vectors for demonstration”), `webapp/server.py:395–397` (`"status": "VERIFIED_EQUIVALENT"`, `"equivalence": True`), `webapp/server.py:442–448` (`"bit_for_bit_identical": True`).

### F2 — Policy constants duplicated in 6+ places

| Constant | Locations |
|---|---|
| loop density reject `0.45` | `liftmap/ingest_tracer.py:54`, `pipeline/lift_drop.py:52`, `webapp/server.py:322,456`, `webapp/src/components/StaticAnalysisView.tsx:88,216`, `webapp/src/components/VoronoiBinaryGraph.tsx:127–381` |
| call fraction reject `0.40` | `liftmap/ingest_tracer.py:55`, `pipeline/lift_drop.py:53`, `webapp/server.py:457` |
| keyed lift fraction `0.6` | `liftmap/placement.py:41` only (good) |

`pipeline/lift_drop.py` re-checks thresholds as defense-in-depth (good), but the *values* are copy-pasted. A threshold change will silently diverge between tracer, lifter, and UI.

### F3 — Sample compilation logic triplicated

`ensure_sample()` exists in `liftmap/ingest_tracer.py:182`, `liftmap/lift_drop_demo.py:58`, and `webapp/server.py:45` with the same `cc -O1 -fno-inline` (+ `-arch x86_64` on Darwin) recipe.

### F4 — Control plane is a subprocess chain with hand-rolled CLIs

Live `make demo` path (verified):

```
lift_drop_demo.py
  ├─ [subprocess] ingest_tracer.py → objdump -d/-r + Capstone
  ├─ import placement.py
  ├─ from lift_drop import lift_and_drop   # pipeline/ on sys.path
  └─ lift_and_drop() → extract_bytes.py → anvill-decompile-spec
       → llvm-link/llc/wasm-ld → ceremony-wasm → (optional) qemu-riscv64
```

- `o2pulley.sh` is **not on the live path**: `lift_drop_demo.py:24` assigns `O2PULLEY` and never uses it. Docs still call it “the single entry point” — that is stale.
- `lift_drop.py` is 590 LOC of PATH shims, tempfile workdirs, IR rewriting, harness rendering, and two backends (Pulley + qemu).
- Symbol-name normalization (`_foo` vs `foo`) is ad-hoc in `ingest_tracer.py:305,358,365,441,488`, `lift_drop_demo.py:82`, `lift_drop.py:511`, `visualize.py:47`, `server.py:238,361-362,438`, `extract_bytes.py:34`. No shared helper.
- Signature strings are built in `ingest_tracer.py:372-375`, parsed in `anvill_spec.py:18-28`, and re-parsed in bash (`o2pulley.sh:46-52`).

### F5 — Naming collision: two “ceremony” things

| Path | What it actually is | On `make demo`? |
|---|---|---|
| `ceremony/` | CLIF→Pulley concept binary + trap trials + shell glue (`o2pulley.sh`) + wasm fixtures | glue yes, Rust binary **no** |
| `ceremony-wasm/` | Real drop runner (wasmtime 36 → Pulley) | **yes** |

A new reader will run the wrong one. `ceremony/src/main.rs` is a synthetic IR demo; the production-shaped path is `ceremony-wasm`.

### F6 — JSON artifacts have no schema

`lift_map.json`, `lift_plan.json`, `lift_results.json` are produced by three modules and consumed by the demo, the webapp, and `visualize.py`. Fields are implicit. `webapp/src/types.ts` re-declares the shapes independently.

### F7 — Dead code and secondary UI dead-ends

Never invoked by `make demo`, `make web`, or CI (safe to archive/delete):

| Path | Evidence |
|---|---|
| `ceremony/o2pulley.sh` | `lift_drop_demo.py:24` defines `O2PULLEY`, never calls it |
| `ceremony/lift_check.py` | zero make/CI/py references |
| `ceremony/sign_x86_64.o`, `sign_llvm.wasm`, `sign.c` | only inputs to dead `lift_check.py` (keep `sign.wasm` — CI smokes it) |
| `liftmap/visualize.py` + `pipeline_view.html` | no invoker; webapp supersedes |
| `webapp/src/components/Header.tsx` | never imported (`App.tsx` uses Sidebar/Topbar) |
| `ingest_tracer.LIFT_THRESHOLD` | defined `:56`, never read (rule cascade uses other thresholds) |
| `ingest_tracer.tarjan_dummy` | empty placeholder `:398` |
| `lift_drop_demo.O2PULLEY` | dead variable |

CI-only (keep): `ceremony` binary, `probe`/`xisa-probe`, `ceremony/sign.wasm`, `ci/*`, `docker/ci.Dockerfile`.

### F8 — Repo surface area vs live code

Rough first-party source (excluding vendor/upstream/build/target/node_modules): **~12k lines**, of which `MULTI_BACKEND_MIGRATION.md` is 754, `ci/static_libs.py` is 812, `webapp/src/styles.css` is 1135, `webapp/server.py` is 518. The *research core* (tracer + placement + lift + drop) is roughly 2k LOC. Documentation and demo chrome outweigh the engine.

### F8 — Artifact contracts (implicit today)

| Artifact | Produced by | Consumed by |
|---|---|---|
| `liftmap/lift_map.json` | `ingest_tracer.py` | demo, `placement.py` CLI, `visualize.py` |
| `liftmap/lift_plan.json` | `placement.plan` via demo | `visualize.py` (webapp recomputes in-process) |
| `liftmap/lift_results.json` | `lift_drop_demo.py` | webapp overlay of expecteds only |
| Anvill `spec.pb` (temp) | `anvill_spec.build_spec` | `anvill-decompile-spec` |
| SSE / `/api/report` | `webapp/server.py` | React `types.ts` (redeclared by hand) |

Also duplicated: objdump byte-window regex in `ingest_tracer.py:97-99` and `ceremony/extract_bytes.py:55-58`; test vectors as code (`lift_drop_demo.CASES` + `_ip_id_hash`) vs magic numbers (`server.VECTORS`).

### F9 — Good things to preserve

- `.llvm-version` as single source of truth + `ci/check_llvm_version.sh` guard.
- `ENVIRONMENT.md` failure-mode table (RTTI, abseil, XED serial, rpath) — operational gold.
- Performance gates re-asserted at the lift site (`pipeline/lift_drop.py:52–53,483–491`).
- Keyed placement (`liftmap/placement.py`) with deterministic CI mode vs secret-keyed deploy mode.
- Ceremony security trials in child processes (`ceremony/src/main.rs`) — crash containment is correct.
- `MULTI_BACKEND_MIGRATION.md` is honest about Cranelift having no decoder and about entropy limits.

---

## 4. Decisions (execute these)

| ID | Decision | Rationale |
|---|---|---|
| D1 | **Fix or remove fabricated dual-run.** Prefer wire the UI to real `lift_results.json` / real `ceremony-wasm` invocations; if that is too slow for the demo, label the panel `SIMULATED` and strip all `verified` / `bit_for_bit_identical` / cycle-count claims. | F1 |
| D2 | **Create `craneliftx/policy.py`** as the only place thresholds and labels live. Import from Python; expose a generated `policy.json` the UI reads at startup. | F2 |
| D3 | **One `ensure_sample()`** in `craneliftx/sample.py`. Delete the three copies (plus keep `liftmap/Makefile` recipe as the only other build site). | F3 |
| D4 | **Archive `o2pulley.sh`** (already off the live path). Make `pipeline/lift_drop.py` the documented CLI. Extract `normalize_symbol()` + `parse_signature()` into `craneliftx/abi.py`. | F4, F7 |
| D5 | **Rename for clarity:** `ceremony-wasm/` → `drop/`. `ceremony/` → `ceremony-demo/` (CI smoke + trap trials). Keep `sign.wasm`. | F5 |
| D6 | **Define JSON schemas** (`craneliftx/schema.py` TypedDicts) for `lift_map`, `lift_plan`, `lift_results`; keep field list in §3 contracts as the checklist. | F6 |
| D7 | **Archive** `visualize.py`, `lift_check.py`, `Header.tsx`, dead constants (`LIFT_THRESHOLD`, `tarjan_dummy`). | F7 |
| D8 | **Keep** `probe/`, `ci/`, `ENVIRONMENT.md`, `MULTI_BACKEND_MIGRATION.md`, vendored lifters, `make demo` as the hard gate. | F9 |
| D9 | **Do not** rewrite remill/anvill, wasmtime, or Cranelift. Do not change the security model (abort-on-trap). Do not “optimize” Pulley. | §8 |

---

## 5. Target layout

```
craneliftX/
  .llvm-version              # unchanged
  Makefile                   # thin; targets: lifters, demo, web, test, check
  ENVIRONMENT.md
  MULTI_BACKEND_MIGRATION.md
  REARCHITECTURE.md          # this file (delete after P0–P3 land, or keep as changelog)

  craneliftx/                # NEW — shared Python package (flat, no plugin system)
    __init__.py
    policy.py                # thresholds, fractions, labels, policy.json writer
    sample.py                # ensure_sample() only
    schema.py                # typed dicts / validation for the three JSON artifacts
    tracer.py                # moved from liftmap/ingest_tracer.py
    placement.py             # moved from liftmap/placement.py
    lift.py                  # moved from pipeline/lift_drop.py (split later if needed)
    anvill_spec.py           # moved from pipeline/
    runtime/                 # remill_runtime.ll + specification_pb2.py

  drop/                      # renamed from ceremony-wasm/
    Cargo.toml
    src/main.rs

  ceremony-demo/             # renamed from ceremony/ (concept + trap trials)
    src/main.rs
    fixtures/                # sign.wasm, sign_x86_64.o, …
    lift_check.py

  demo/
    lift_drop_demo.py        # make demo entry; imports craneliftx.*

  web/
    server.py                # thin HTTP; imports craneliftx.*; NO fabricated metrics
    src/                     # React app (existing)
    …

  ci/                        # unchanged role
  vendor/  upstream/  build/ # unchanged
  archived/                  # + visualize.py, o2pulley.sh if cut
```

Migration rule: **move, don’t rewrite.** Preserve git history with `git mv` where possible. Behavioral changes are confined to D1–D7.

---

## 6. Work packages

### P0 — Stop lying in the UI (mandatory)

**Goal:** every number and status the web UI shows is either real or explicitly simulated.

1. Read `webapp/server.py` `_stream` (from the `VECTORS` dict through `heatmap_data`).
2. Choose **one** of:
   - **P0-A (preferred):** for each selected function, invoke the real pipeline (`pipeline.lift_drop.lift_and_drop` or `ceremony-wasm` with the already-lifted wasm from `lift_results.json`). Populate `native`/`pulley` from actual outputs. Cycles/latency must come from measurement or be omitted.
   - **P0-B (acceptable):** keep static traces only if sourced from `lift_results.json` / Capstone disassembly of the real object. Delete invented `native_asm` and `pulley_bc`. Set UI banner: `SIMULATED VIEW — no live dual execution`.
3. Delete these lies unconditionally:
   - `"bit_for_bit_identical": True`
   - `"verified": True` / `"status": "VERIFIED_EQUIVALENT"`
   - `"summary": "… 100% bit-for-bit equivalence …"`
   - hardcoded `native_cycles` / `pulley_cycles` / `latency_ns` formulas (`* 0.28`, `* 1.15`)
4. Update `webapp/src/types.ts` + `SideBySideRunView.tsx` so unknown fields render as `—`, not as measurements.
5. If `lift_results.json` exists (from `make demo`), use its real expected/out values and statuses.

**Acceptance:**
- [ ] Grep of `webapp/` shows zero occurrences of `bit_for_bit_identical`, `VERIFIED_EQUIVALENT`, fabricated cycle formulas.
- [ ] UI either shows real `ceremony-wasm` output or a visible `SIMULATED` / `NOT EXECUTED` badge on every dual-run card.
- [ ] `npm --prefix webapp run typecheck` passes.

### P1 — Single policy source

1. Add `craneliftx/policy.py` (or `liftmap/policy.py` if you keep the package name) with:
   ```python
   LOOP_DENSITY_REJECT = 0.45
   CALL_FRACTION_REJECT = 0.40
   LIFT_THRESHOLD = 0.40
   DEFAULT_KEYED_FRACTION = 0.6
   ```
2. Change `ingest_tracer.py`, `pipeline/lift_drop.py`, `placement.py` to import them (keep local aliases if needed).
3. Emit `policy.json` on tracer completion (or at process start) and have the webapp read it; remove literal `0.45` / `0.40` from `StaticAnalysisView.tsx` and `VoronoiBinaryGraph.tsx`.
4. One `ensure_sample()`; delete duplicates.

**Acceptance:**
- [ ] `rg '0\.45|0\.40' --glob '!**/node_modules/**' --glob '!**/dist/**'` returns only `policy.py` (and docs).
- [ ] `make demo` still rejects `_crc32_tight` and lifts `_tcp_window_scaled`, `_ip_id_hash`, `_parse_packet` on `sample_network`.
- [ ] UI hot-loop highlighting still works and matches the tracer.

### P2 — Unify the lift CLI

1. Document `python3 pipeline/lift_drop.py` as the only lift entry (flags already exist: `--object/--bytes --symbol --signature --arg --expect --expect-out`). Optionally add `--target pulley|riscv64|both`.
2. `git mv ceremony/o2pulley.sh archived/` — it is already unused by `make demo`.
3. Extract `normalize_symbol()` and `parse_signature()` into `craneliftx/abi.py`; use from tracer, lift, demo, webapp. Share the objdump byte-window regex with `extract_bytes.py`.
4. Unify test vectors: one `CASES` table (from `lift_drop_demo.py`) that the webapp imports; delete `server.VECTORS` cycles/asm fabrications.
5. Optional (only if it reduces LOC): split `lift_drop.py` into orchestration vs IR-rewrite helpers. Do not invent a plugin framework.

**Acceptance:**
- [ ] `make demo` passes with no `o2pulley.sh` on the path.
- [ ] `rg o2pulley` in first-party code is empty (docs may mention it historically).
- [ ] Manual: `python3 pipeline/lift_drop.py --object liftmap/sample_network.o --symbol _tcp_window_scaled --signature 'RAX(RDI,RSI)' --arg 100 --arg 50 --expect 150` exits 0.

### P3 — Naming + dead code

1. `git mv ceremony-wasm drop` (update `Makefile`, `pipeline/lift_drop.py:runner_path`, `ci/*`).
2. `git mv ceremony ceremony-demo`; move shell fixtures used by the live path under `demo/` or `fixtures/`.
3. `git mv liftmap/visualize.py archived/` (and generated `pipeline_view.html` stays gitignored).
4. Collapse `liftmap/` Python into `craneliftx/` (or leave `liftmap/` as a thin namespace that re-exports — pick one, don’t do both).
5. Update `MULTI_BACKEND_MIGRATION.md` §4.8 path references in a single pass so the design doc matches the tree.

**Acceptance:**
- [ ] `make demo`, `make build-ui`, `ci/check_llvm_version.sh` pass.
- [ ] Top-level tree is understandable in <2 minutes: no two directories named `ceremony*`.
- [ ] `ci/build.sh` still builds and smokes `drop` + `probe`.

### P4 — Contracts (only after P0–P3)

1. Add `craneliftx/schema.py` with TypedDicts for `FunctionMetrics`, `LiftMap`, `LiftPlan`, `LiftResults`.
2. Validate at write time in tracer/demo; validate at read time in webapp.
3. Optionally generate `webapp/src/types.ts` from the same source (a 20-line script is enough; no codegen platform).

**Acceptance:**
- [ ] Invalid `lift_map.json` fails loudly in demo and webapp.
- [ ] `types.ts` cannot drift without a test failing (or is generated).

---

## 7. Verification checklist (run after every package)

```sh
# toolchain pin
ci/check_llvm_version.sh

# end-to-end research gate (hard)
make demo

# UI
npm --prefix webapp run typecheck
npm --prefix webapp run build

# optional if lifter + qemu present
python3 pipeline/lift_drop.py --help
```

`make demo` success looks like (`ENVIRONMENT.md` §5):

```
_tcp_window_scaled(100, 50) = 150      (expected 150)
_ip_id_hash(100, 50)        = 51156    (expected 51156)
_parse_packet(...)          = 89       OUT u32 4, OUT u32 20

MULTI LIFT-AND-DROP SUMMARY:
  lifted and validated : ['_ip_id_hash', '_tcp_window_scaled', '_parse_packet']
  kept NATIVE          : ['_crc32_tight', '_handle_connection', '_main']
```

---

## 8. Do not touch

| Path / topic | Why |
|---|---|
| `vendor/remill`, `vendor/anvill`, `upstream/` | external code; rebuild via `pipeline/build_lifters.sh` |
| `build/` | lifter build tree; regenerated |
| `.llvm-version` semantics | CI depends on the pin; change only with `ci/check_llvm_version.sh` |
| Security model: abort-on-trap, never restart; trap ≠ success | core claim of the research |
| Pulley / Cranelift / Wasmtime versions | pinned for reasons documented in `MULTI_BACKEND_MIGRATION.md` |
| `hetersec2/` | prior art; already gitignored |
| `MULTI_BACKEND_MIGRATION.md` body (except path renames in P3) | design SSOT |
| Adding a kernel module, hypervisor, or new ISA backend | out of scope for this pass |

---

## 9. Key file map (as-built → role)

| Path | LOC | Role | Verdict |
|---|---:|---|---|
| `liftmap/ingest_tracer.py` | 609 | Capstone CFG + lift scoring | Keep → `craneliftx/tracer.py` |
| `liftmap/placement.py` | 137 | Keyed subset selection | Keep |
| `liftmap/lift_drop_demo.py` | 266 | `make demo` orchestrator | Keep → `demo/` |
| `liftmap/visualize.py` | 295 | Second UI | **Archive** (D7) |
| `pipeline/lift_drop.py` | 590 | Lift/drop engine glue | Keep, thin (D4) |
| `pipeline/anvill_spec.py` | 110 | Protobuf spec writer | Keep |
| `pipeline/build_lifters.sh` | 216 | remill/anvill build | Keep |
| `pipeline/bootstrap_env.sh` | 159 | Machine provisioning | Keep |
| `ceremony/src/main.rs` | 201 | Concept + trap trials | Keep → `ceremony-demo/` |
| `ceremony/o2pulley.sh` | ~80 | Bash CLI (already off-path) | **Archive** (D4) |
| `ceremony/lift_check.py` | 119 | Wasm section prober | **Archive** (D7) |
| `ceremony-wasm/src/main.rs` | 266 | Real drop runner | Keep → `drop/` |
| `probe/src/main.rs` | 194 | Backend inventory smoke | Keep (CI) |
| `webapp/server.py` | 518 | Demo HTTP + **fabricated dual-run** | **Fix P0** |
| `webapp/src/components/Header.tsx` | 82 | Unused component | **Delete** (D7) |
| `webapp/src/App.tsx` + rest of components | ~2000 | React UI | Keep; un-hardcode metrics |
| `ci/*.py` | ~1100 | release inventory / static libs | Keep |
| `MULTI_BACKEND_MIGRATION.md` | 754 | Design SSOT | Keep |
| `ENVIRONMENT.md` | 129 | Toolchain map | Keep |

---

## 10. Suggested commit sequence

```
1. fix(web): remove fabricated dual-run metrics; mark simulated data
2. refactor(policy): single source for lift thresholds
3. refactor(lift): make pipeline/lift_drop.py the sole lift CLI
4. refactor: rename ceremony-wasm → drop, ceremony → ceremony-demo
5. chore: archive visualize.py and drop o2pulley.sh
6. feat(schema): typed lift_map / lift_plan / lift_results contracts
```

Each commit must leave `make demo` green.

---

## 11. Open questions for the human (non-blocking)

These do **not** block P0–P3; default answers are in brackets.

1. Should the web UI ever call the real lifter (slow, needs LLVM) or only replay `lift_results.json`? **[Default: replay results, clearly labeled; optional “Run live lift” button later.]**
2. Keep `probe/` as a separate crate or merge into `drop` as `drop inventory`? **[Default: keep separate; CI already smokes it.]**
3. Is `MULTI_BACKEND_MIGRATION.md` still the only design SSOT after this pass, or do we split “threat model” vs “pipeline mechanics”? **[Default: keep one SSOT; add path renames only.]**

---

*End of handoff. P0 is the only package that changes research honesty; everything else is structure.*
