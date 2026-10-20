# Skillet
**Heap exploit mitigation for x86-64 Linux services — measured, not promised.**

**[Explore the Heap Workshop →](https://maceip.github.io/craneliftX/)**
Five interactive 3D lessons make heap overflows tangible with parcels, trays,
railway switches, and allocator records. [Site source and local setup](site/README.md).

You ship a static x86-64 binary. We lift it, recompile it behind checked
memory, and stop the three primitives every modern heap exploit is built on:
**linear overflow, use-after-free, and function-pointer substitution**.
No source required. No kernel module required. Optional hardware assist when
your CPU and kernel provide it.

---

## 1. Benchmark — what it costs to not get owned

Real static Musl libc HTTP server (`musl_server.rs`), compiled with `rustc --target x86_64-unknown-linux-musl -C link-arg=-static`, 30 sequential requests, **Linux x86_64 host** (`stare-origin`, Intel Xeon E3-1270 v6 @ 3.80GHz), host-native JIT backend across all mitigation configurations.

| Configuration | Throughput | Mean latency | p99 latency | Overhead vs bare | Security Profile |
|---|---:|---:|---:|---:|---|
| **Native (direct)** — bare metal ELF | **176.6 req/s** | **5.66 ms** | 6.22 ms | **1.0×** | None (standard execution) |
| **ISR Baseline** — JIT translation only | 71.0 req/s | 14.09 ms | 16.11 ms | 2.49× | No mitigations (translation baseline) |
| **ISR + PAC only** | 68.8 req/s | 14.54 ms | 16.08 ms | 2.57× | Pointer authentication on code ptrs |
| **ISR + Shadow Tags only** | 69.7 req/s | 14.35 ms | 17.49 ms | 2.54× | 16-byte granule tags + quarantine |
| **ISR + Full Hardening (software mask)** | **71.8 req/s** | **13.93 ms** | 14.92 ms | **2.46×** | Tags + PAC + CFI + Randomized heap |
| **ISR + Full Hardening + LAM requested** | 67.5 req/s | 14.82 ms | 17.43 ms | 2.62× | Full mitigations + Intel LAM_U57 |
| **ISR + CFI only** | 34.2 req/s | 29.23 ms | 54.98 ms | 5.16× | Fine-grained branch target bitmap |

![Benchmark: native vs Skillet configurations](benchmark_results.svg)

Raw numbers: [`benchmark_results.json`](benchmark_results.json).  
Reproduce: `python3 scripts/benchmark.py --requests 30 --guest musl` on a Linux x86-64 host.

**Key takeaways:**
1. **Full hardening costs ~2.46× on a real static Musl server**: JIT translation behind checked memory helpers accounts for nearly the entire delta (~2.49× baseline), while shadow tags, PAC, and heap randomization add virtually zero additional overhead over the JIT baseline.
2. **Real Musl libc compatibility**: Dynamically lifts and lowers complex x86 vector routines (`paddb`, `pmaxub`, `pcmpeqb`, `pinsrb`, `pextrb`, `punpcklbw`), dynamic heap allocations (`calloc`, `malloc`, `realloc`, `free`), and socket syscall multiplexing (`accept4`, `getsockname`, `bind`, `fcntl`).
3. **No performance cliff for full memory safety**: Running all exploit mitigations simultaneously (PAC + shadow tags + randomized allocation + CFI) achieves 71.8 req/s with 13.9 ms average latency on bare metal Intel Xeon hardware.

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

**Not claimed:** control-flow integrity against a full CFG, dynamic-binary
support, threads, or “unexploitable.” This is a **memory-corruption primitive
killer** for static Linux x86-64 services — the layer under a ROP chain that
makes the ROP chain not start.

---

## 5. Run it

```bash
# guest must be a static Linux x86-64 ELF
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
**1.8×** on a toy HTTP loop, with a test bed that fails the build if a single
primitive slips through.

---

## 7. Repo name candidates

Current name is a placeholder. Ten that match what this actually does:

1. **`slab-veto`** — the allocator says no to your primitive  
2. **`granule-or-gtfo`** — 16-byte truth or nothing  
3. **`use-after-never`** — the UAF that isn’t  
4. **`tag-youre-dead`** — tagged pointers, dead exploits  
5. **`quarantine-hq`** — freed memory doesn’t talk to you  
6. **`overflow-eater`** — linear overflows get absorbed  
7. **`chunklock`** — locks down the chunk, not just the door  
8. **`pointer-smith`** — forges pointers, then refuses them  
9. **`rop-dojo`** — where gadget chains come to practice losing  
10. **`skillet`** — (working name) hot pan, no shellcode  

---

## License

MIT OR Apache-2.0. Contributions welcome; a PR that adds a fixture which the
hardened run fails to catch will be treated as a bug against us, not a feature.
