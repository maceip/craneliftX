# LOCKSTEP
**Checked execution for Linux x86-64 services.**

**[Explore Lockstep →](https://maceip.github.io/lockstep/)**
Follow an attacker or operator terminal alongside a changing 3D memory layout.
Fourteen scroll-linked chapters explain five corruption patterns and the runtime
defenses, with explicit coverage limits. [Site source and local setup](site/README.md).

You ship an unmodified x86-64 binary. **LOCKSTEP** lifts it through Cranelift,
executes every load, store, and branch in strict lockstep with shadow-memory
verification, and kills the three primitives every modern heap exploit is built on:
**linear overflow, use-after-free, and function-pointer substitution**.
No source required. No kernel module required. Works with multi-process (`fork`)
and multi-threaded (`clone` + `futex`) `glibc` and `musl` binaries.

Native synchronization now has `adaptive`, `block`, and `access` policies.
[Thread synchronization, invariants and verification](docs/THREAD_SYNCHRONIZATION.md).

---

## 1. Recorded Nginx benchmark

Real full-feature **Nginx 1.26.2** compiled against both **glibc 2.39** and **musl 1.2.5** (`worker_processes 2; thread_pool default threads=4 max_queue=1024; aio threads=default;`), serving 256 KiB SHA-256 verified HTTP payloads across 8 concurrent clients on a **Linux x86-64 host** (`stare-origin`, Intel Xeon E3-1270 v6 @ 3.80GHz). Every 50 ms, `/proc` telemetry samples Resident Memory (`VmRSS` MB) and CPU utilization (`%`) across the master process, worker processes, and AIO thread pools.

| Nginx Configuration (`2 workers × 4 threads`) | Throughput | vs. Native | Median Latency | p95 Latency | Steady-State Heap | Peak RSS | Mean CPU |
|---|---:|---:|---:|---:|---:|---:|---:|
| **LOCKSTEP Full Defense · Nginx (`glibc` 2.39)** | **93.3 req/s** | `28.0× slower` | **72.8 ms** | 152.6 ms | **+5.8 MB** | 362.2 MB | 258.3% |
| **LOCKSTEP Full Defense · Nginx (`musl` 1.2.5)** | **86.2 req/s** | `29.8× slower` | **77.2 ms** | 166.3 ms | **+10.7 MB** | 253.5 MB | 252.3% |
| **LOCKSTEP Shadow Tags + Quarantine · Nginx (`glibc`)** | **88.6 req/s** | `29.5× slower` | **75.2 ms** | 178.4 ms | **+10.8 MB** | 353.5 MB | 255.2% |
| **LOCKSTEP PAC + CFI Only · Nginx (`glibc`)** | **90.8 req/s** | `28.8× slower` | **75.9 ms** | 171.7 ms | **+8.1 MB** | 351.6 MB | 257.6% |
| **Unprotected Native · Nginx (`glibc` 2.39)** | 2612.0 req/s | `1.0× (ref)` | 2.8 ms | 4.8 ms | +0.5 MB | 6.0 MB | 73.0% |
| **Unprotected Native · Nginx (`musl` 1.2.5)** | 2571.5 req/s | `1.0× (ref)` | 2.9 ms | 4.9 ms | +0.2 MB | 3.5 MB | 81.8% |

Raw 50 ms time-series telemetry & receipts: [`benchmark_results.json`](benchmark_results.json) · [`.isr-state/last-mile-recovery/recovery.json`](.isr-state/last-mile-recovery/recovery.json)  
Reproduce: `python3 scripts/benchmark.py --duration 4.0 --concurrency 8` and `python3 scripts/verify_nginx.py` on a Linux x86-64 host.

**What the time-series telemetry shows:**
1. **Real multi-process, multi-threaded Nginx (`glibc` and `musl`)**: Powered by a Blink-derived process/thread substrate (`CLONE_VM | CLONE_THREAD | CLONE_SETTLS`, `fork`, shared `futex` wait queues, `epoll`, `eventfd`, `pread64`, and scalar/XMM pointer provenance tracking), full-feature Nginx runs unmodified under complete heap and pointer protection.
2. **Steady-state quarantine plateau (no unbounded memory growth)**: During the initial ~1.2s warmup, dynamic memory rises as the bounded quarantine ring fills with freed connection and request pools (`malloc` / `posix_memalign` / `free`). Once the quarantine ring reaches capacity, evicted chunks are recycled with fresh CSPRNG tags and memory flattens out at **+5.8 MB (`glibc`)** and **+10.7 MB (`musl`)** across the rest of the run.
3. **Explicit translation tradeoff (`~28×` vs. Unprotected Native)**: Checked software translation of every guest load, store, and indirect branch trades raw single-node throughput (`~2,612 → 93.3 req/s`, or **~23.9 MB/s** of SHA-256 verified HTTP payload throughput across ~2.5 cores) for deterministic, hardware-independent elimination of heap overflows, use-after-free, and pointer hijacking.

---

## 2. Test bed — real exploit shapes, real catches

This is the product. Eight freestanding x86-64 guests. Each one performs a
genuine exploitation primitive through the runtime allocator. Every guest is
run **twice**:

- **Hardened** — mitigations armed. Must be rejected with the *specific* fault.
- **Control** — matching mitigation disarmed. The primitive must *succeed*
  (neighbor corrupted, UAF observed, substituted handler runs).

A crash alone is not evidence. Both directions must hold.

| Guest (exploit primitive) | Hardened fault | Control result | Efficacy |
|---|---|---|---|
| `heap_overflow` — linear overflow into neighbor | `TagMismatch` | exit 3 — neighbor corrupted | **caught** |
| `use_after_free` — write after free | `UseAfterFree` | exit 0 — write landed | **caught** |
| `pointer_substitution` — swap function pointers | `PointerAuth` | exit 20 — substituted handler ran | **caught** |
| `cve_2026_42533` — fixed-buffer capture overflow (Nginx map-shaped) | `TagMismatch` | exit 3 | **caught** |
| `cve_2026_42530` — lifecycle UAF (HTTP/3 QPACK-shaped) | `UseAfterFree` | exit 4 — dangling read | **caught** |
| `cve_2026_14432` — GC cache UAF (V8-shaped) | `UseAfterFree` | exit 4 — dangling read | **caught** |
| `cve_2026_53362` — slab overflow into adjacent object (IPv6 frag-shaped) | `TagMismatch` | exit 3 — neighbor corrupted | **caught** |
| `heap_grooming` — adjacency under attack layout | *(measured)* | ordered 16/16 · randomized **0/16** | **defeated** |

CVE-named guests replicate the *vulnerable pattern* of public heap bugs
(unchecked fixed-buffer copy, dangling cached pointer, same-class slab
neighbor overflow). They are generic primitives in the same shape as those
bugs — not reduced copies of the original products.

Reproduce: `python3 scripts/verify_heap_mitigations.py --backend x86_64`  
Network regression (real TCP guest, 2.7M native instructions) included.

---

## 3. Optional capabilities (on when they help, honest when they don’t)

### Intel Linear Address Masking (LAM) — `hardening.lam = true` (default)

LAM lets the CPU ignore pointer tag bits 56/57–62 during data address
translation. We **request LAM_U57** at startup via `arch_prctl` / `prctl`.
No custom kernel module.

| | |
|---|---|
| What it does | Hardware-masked tag bits 57:62 (6-bit tags); PAC stays in 48:55 |
| What it does *not* do | Skip the shadow-tag **check**, tag RIP/branches, or replace PAC/CFI |
| Default | Try it; fall back to software 8-bit tags (56:63) if the kernel says no |
| Observed on Ubuntu 6.8 + Intel | CPUID LAM **present**, kernel returns `EINVAL` → software tags (ledger says so) |
| Opt out | `--no-lam` or `hardening.lam = false` |

When a kernel accepts LAM_U57, the heap automatically switches schemes. The
run receipt records `tags.lam` / `tags.scheme` so you can see which path you got.

**Lab note.** On `secure.build` (Intel, Ubuntu 6.8): CPUID reports LAM; every
`arch_prctl`/`prctl` enable path returns `EINVAL`, so the engine falls back to
software tags and logs why. Debian 12 was booted in QEMU/KVM on the same host
to validate the “Linux kernel on Debian” deployment path — that guest kernel
(6.1) is likewise too old for LAM. A newer guest kernel (with tagged-address
prctl) would flip `tags.lam` to `lam-u57` with no code change.

### RV64 interpreted backend — `--backend rv64` (optional)

The original cross-ISA path: x86-64 guest → CLIF → RISC-V, interpreted.

| | |
|---|---|
| Use it when | You want the same ProcessBus mitigations with a slower, fully interpreted substrate |
| Cost | Several× slower than host-native (earlier HTTP run: ~5× vs bare) |
| Security | Same shadow tags, PAC, quarantine, CFI as native |
| Default | **No** — host-native `x86_64` is the production default |

Native is not “unsafe RV64.” Native loads/stores call the same checked
`ProcessBus` helpers the interpreter uses. Syscalls never hit the host kernel
as guest syscalls; the runtime serves them at block boundaries.

---

## 4. What you actually get

```
static x86-64 ELF
        │
        ▼
   decode + policy          (privileged ops, stack, targets)
        │
        ▼
   lift → Cranelift → host-native JIT (W^X pages)
        │
        ├─ every load/store → ProcessBus → shadow tags + PAC
        ├─ malloc/free      → randomized tagged heap + quarantine
        ├─ indirect target  → mid-instruction CFI
        └─ syscall          → runtime bridge (never host kernel)
```

| Mitigation | Overflow | UAF | Ptr substitution | Grooming |
|---|:---:|:---:|:---:|:---:|
| Shadow granule tags | ✔ | ✔ (quarantine) | | |
| Pointer authentication | | | ✔ | |
| Randomized slab placement | | | | ✔ |
| Mid-instruction CFI | | | ✔ (ROP into insn body) | |
| LAM_U57 (optional) | tag path only | tag path only | | |

**Supported out of the box:** multi-process (`fork`) and multi-threaded (`clone` + `CLONE_THREAD` + `CLONE_SETTLS` + shared `futex` wait queues) static `glibc` and `musl` binaries, backed by our Blink-derived process/thread substrate.  
**Not claimed:** control-flow integrity against a full source-level CFG, dynamic-linker (`ld.so`) `dlopen` plugins, or “unexploitable.” This is a **memory-corruption primitive killer** for static Linux x86-64 services — the layer under a ROP chain that makes the ROP chain not start.

---

## 5. Run it

```bash
# guest must be a static Linux x86-64 ELF (glibc or musl)
cargo build --release --bin isr-server --bin isr-process

# one-shot
./target/release/isr-process ./guest.elf --backend x86_64

# supervisor (systemd/Docker ready)
./target/release/isr-server --config deploy/isr-server.toml
```

| Knob | Default | Meaning |
|---|---|---|
| `backend` | `x86_64` | Host-native JIT (use `rv64` for the interpreter) |
| `hardening.tags` | on | 16-byte shadow granules, quarantine on free |
| `hardening.pac` | on | SipHash PAC on code pointers in memory |
| `hardening.random_placement` | on | CSPRNG slot draw (kills reliable grooming) |
| `hardening.cfi` | on | Reject transfer into a known instruction body |
| `hardening.lam` | on | Request Intel LAM_U57; software tags if refused |

Evidence and ledgers land in `.isr-state/`. A/B any knob with `--no-tags`,
`--no-pac`, `--no-random-placement`, `--no-cfi`, `--no-lam`.

---

## 6. Why this exists

ROP/JOP is not a memory-corruption bug. It is what you do *after* you have
overwritten a pointer. Heap tags, PAC, and placement randomization remove the
overwrite and the fake pointer. Compile-time diversification (fresh register
map + stack pad per block) means the gadget set you scraped from the original
binary is not the gadget set in lifted code.

If you run untrusted or historically vulnerable C/C++ on x86-64 Linux and you
cannot patch on day zero, this is the layer that buys you time with the overhead shown in the recorded workload above. The test bed checks specific hardened faults against successful control cases.

---

## License

MIT OR Apache-2.0. Contributions welcome; a PR that adds a fixture which the
hardened run fails to catch will be treated as a bug against us, not a feature.
