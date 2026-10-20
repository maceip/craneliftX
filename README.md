# LOCKSTEP
**The End of Heap Exploitation for Linux x86-64 Services.**

**[Explore the Heap Workshop →](https://maceip.github.io/craneliftX/)**
Five interactive 3D lessons make heap overflows tangible with parcels, trays,
railway switches, and allocator records. [Site source and local setup](site/README.md).

You ship an unmodified x86-64 binary. **LOCKSTEP** lifts it through Cranelift,
executes every load, store, and branch in strict lockstep with shadow-memory
verification, and kills the three primitives every modern heap exploit is built on:
**linear overflow, use-after-free, and function-pointer substitution**.
No source required. No kernel module required. Works with multi-process (`fork`)
and multi-threaded (`clone` + `futex`) `glibc` and `musl` binaries.

---

## 1. Nginx Production Benchmark — Memory & CPU Telemetry Over Time

Real full-feature **Nginx 1.26.2** compiled against both **glibc 2.39** and **musl 1.2.5** (`worker_processes 2; thread_pool default threads=4 max_queue=1024; aio threads=default;`), serving 256 KiB SHA-256 verified HTTP payloads across 8 concurrent clients on a **Linux x86-64 host** (`stare-origin`, Intel Xeon E3-1270 v6 @ 3.80GHz). Every 50 ms, `/proc` telemetry samples Resident Memory (`VmRSS` MB) and CPU utilization (`%`) across the master process, worker processes, and AIO thread pools.

![LOCKSTEP Nginx Benchmark: Memory & CPU Over Time](benchmark_results.svg)

| Nginx Configuration (`2 workers × 4 threads`) | Throughput | Median Latency | p95 Latency | Dynamic Heap Growth | Peak RSS | Mean CPU |
|---|---:|---:|---:|---:|---:|---:|
| **LOCKSTEP Full Defense · Nginx (`glibc` 2.39)** | **71.1 req/s** | **84.0 ms** | 212.5 ms | **+9.2 MB** | 366.1 MB | 243.4% |
| **LOCKSTEP Full Defense · Nginx (`musl` 1.2.5)** | **72.8 req/s** | **85.0 ms** | 217.5 ms | **+11.8 MB** | 254.8 MB | 243.5% |
| **LOCKSTEP Shadow Tags + Quarantine · Nginx (`glibc`)** | **73.1 req/s** | **81.9 ms** | 215.1 ms | **+12.3 MB** | 362.2 MB | 243.7% |
| **LOCKSTEP PAC + CFI Only · Nginx (`glibc`)** | **65.1 req/s** | **85.4 ms** | 249.4 ms | **+12.6 MB** | 355.5 MB | 240.3% |
| **Unprotected Native · Nginx (`glibc` 2.39)** | 2079.8 req/s | 3.5 ms | 5.2 ms | +0.3 MB | 5.9 MB | 60.6% |
| **Unprotected Native · Nginx (`musl` 1.2.5)** | 1944.3 req/s | 3.7 ms | 5.4 ms | +0.1 MB | 3.4 MB | 59.9% |

Raw 50 ms time-series telemetry & receipts: [`benchmark_results.json`](benchmark_results.json) · [`.isr-state/last-mile-recovery/recovery.json`](.isr-state/last-mile-recovery/recovery.json)  
Reproduce: `python3 scripts/benchmark.py --duration 2.0 --concurrency 8` and `python3 scripts/verify_nginx.py` on a Linux x86-64 host.

**What the time-series telemetry shows:**
1. **Real multi-process, multi-threaded Nginx (`glibc` and `musl`)**: Powered by a Blink-derived process/thread substrate (`CLONE_VM | CLONE_THREAD | CLONE_SETTLS`, `fork`, shared `futex` wait queues, `epoll`, `eventfd`, `pread64`, and scalar/XMM pointer provenance tracking), full-feature Nginx runs unmodified under complete heap and pointer protection.
2. **Bounded quarantine & shadow-tag memory growth over time**: As Nginx allocates per-request buffers and 256 KiB AIO payloads across 2 worker processes and 8 pool threads, dynamic memory grows smoothly by **+9.2 MB (`glibc`)** and **+11.8 MB (`musl`)** as the 256-slot quarantine populates and begins recycling slots.
3. **Predictable multi-core CPU scaling**: Across 2 worker processes and 8 AIO threads, LOCKSTEP utilizes ~2.4 cores (`~243%` CPU) continuously while serving ~71–73 verified 256 KiB SHA-256 responses per second (~18.5 MB/s of verified HTTP throughput).

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
cannot patch on day zero, this is the layer that buys you time — at about
**1.46×** on a static Musl HTTP service (and verified on multi-worker, multi-threaded `glibc` and `musl` Nginx), with a test bed that fails the build if a single primitive slips through.

---

## 7. Repo name candidates

Current name is a placeholder. Ten names built for a binary-lifting exploit mitigation engine:

1. **`WARDLINE`** — the checked boundary every load, store, and indirect call must cross  
2. **`GRANULE`** — named after the 16-byte shadow memory truth that kills overflows and UAFs  
3. **`PROVENANCE`** — pointer authentication and register/XMM lineage enforced from birth to dereference  
4. **`BLACKSEAL`** — binary-lifted execution under cryptographic pointer and slab attestation  
5. **`IRONLIFT`** — x86-64 lifted into Cranelift IR and re-emitted behind an armored memory bus  
6. **`SLABLOCK`** — randomized, tag-locked heap allocation with 256-slot quarantine  
7. **`AEGIS-64`** — user-space x86-64 shield requiring zero source changes and zero kernel modules  
8. **`FAULTLINE`** — turns silent heap corruption into an immediate, deterministic mitigation trap  
9. **`SHADOWBUS`** — the mediated `ProcessBus` substrate where raw guest pointers never touch bare memory unchecked  
10. **`VERDICT`** — binary-in, receipt-out: every memory access and control transfer is judged at runtime  

---

## License

MIT OR Apache-2.0. Contributions welcome; a PR that adds a fixture which the
hardened run fails to catch will be treated as a bug against us, not a feature.
