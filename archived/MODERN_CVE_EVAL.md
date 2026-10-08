# Modern evaluation target: replacing CVE-2013-2028 with a 2025 CVE

## The pick: CVE-2025-62507

A stack buffer overflow in Redis, with a publicly demonstrated full RCE chain.

| Attribute | Value |
|---|---|
| CVE | **CVE-2025-62507** |
| CVSS | 8.8 (v3.1) / 7.7 (v4.0) |
| Type | **Stack buffer overflow** |
| Location | `XACKDEL` command implementation, Redis 8.2+ |
| Root cause | Fixed-size stack array `static_ids`; no check that the supplied ID count fits `STREAMID_STATIC_VECTOR_LEN`; no reallocation when it doesn't |
| Primitive | Overwrites saved registers and the return address |
| Affected | Redis 8.2.0–8.2.2, 8.3.0–8.3.1. Fixed in 8.2.3 / 8.3.2 |
| Trigger | A single `XACKDEL` command with enough message IDs |
| Auth | Required in theory; **Redis defaults to no authentication**, so effectively unauthenticated |
| RCE | **Demonstrated by JFrog Security Research**, January 2026 |
| Found by | **Google Big Sleep** (DeepMind + Project Zero) |

The JFrog finding is the part that matters for this experiment:

> "the overflow does not merely corrupt adjacent data; it allows an attacker to
> overwrite sensitive stack contents, including saved registers and the function's
> return address. The structure of stream IDs, which are parsed as two independent
> numeric values, makes it possible to precisely control the overwritten memory."

## Why it is the right analog

| | CVE-2013-2028 (HeterSec) | CVE-2025-62507 (proposed) |
|---|---|---|
| Class | Stack buffer overflow → control-flow hijack → ROP | **Same** |
| Application | Nginx | **Redis — which HeterSec also benchmarked** |
| Vintage | 2013 | **2025 CVE ID, disclosed Jan 2026** |
| Public RCE | Yes | **Yes (JFrog), with precise memory control** |
| Trigger | HTTP chunked request | Single `XACKDEL` command |
| Source | C, open | C, open — builds for x86_64 and aarch64 |

Nearly a drop-in replacement, in the same application family, thirteen years newer.

## Second candidate: CVE-2025-49844 "RediShell"

Redis Lua engine **use-after-free**, CVSS **10.0**, disclosed 3 Oct 2025, affects
Redis ≤ 8.2.1, authenticated attacker, memory corruption → RCE.

Weaker fit for this purpose: a UAF gives a less direct return-address overwrite
than a stack overflow, and it requires authentication — which breaks the clean
"remote attacker with a known interface" model HeterSec assumed. Keep it as a
follow-on case study, not the primary target.

## Experiment design

**Goal:** reproduce HeterSec §4.1 — a pre-built, single-architecture ROP payload
succeeds on the architecture it was built for and fails on the other — using
modern software and modern mitigations.

### Setup

1. Build Redis **8.2.2** from source twice: once for `x86_64`, once for `aarch64`.
   Pin the compiler, version, and flags across both builds.
2. Deploy one instance of each on two hosts, or one host with both ISAs available.
3. Baseline: confirm the JFrog-style exploit achieves RCE on x86_64.

### Procedure

1. Fire the exploit, with the payload authored for **x86_64**, at the x86_64
   instance → expect success.
2. Fire the *same* payload at the **aarch64** instance → expect crash (HeterSec
   observed "crash and restart").
3. Then run the MTD loop: randomize which architecture serves each connection,
   fire the x86_64 payload repeatedly, and measure the **empirical success rate**.

### What to measure

- Success rate vs. `1/N` — this is the number that settles the argument. If N=2
  and you observe ~50%, you have confirmed the log₂(N) ceiling empirically.
- Whether failures crash (detection signal) or silently misbehave.
- Whether an adaptive attacker — one who re-authors for aarch64 after the first
  failure — succeeds. **They will.** Record how many attempts it takes.

## Confounds you must control

This is where a naive replication goes wrong. Modern mitigations did not exist in
HeterSec's 2013 setup:

1. **Stack canary.** Modern Redis builds typically use `-fstack-protector`. The
   2013 exploit predates it. Either build with `-fno-stack-protector` or use a
   payload with a canary bypass. Otherwise your aarch64 "failure" may be the
   canary, not ISA diversity.
2. **Pointer authentication (PAC).** On Apple Silicon and ARMv8.3+, PAC will
   block return-address overwrite. An aarch64 failure could be PAC, not
   architecture. Disable PAC for the control condition, or use a non-PAC target,
   and say which you did.
3. **BTI / branch target identification** on ARMv8.5+ — same problem.
4. **ASLR.** HeterSec took ASLR as the baseline and kept it on. Keep it on both
   sides; it is not a confound *between* architectures, but note it.
5. **Toolchain and libc drift.** Different compilers, glibc versions, or build
   flags between the two arches will change stack layout independently of ISA.
   Pin everything, and record the actual stack-frame deltas you observe.
6. **Redis version skew.** Use the identical source tarball for both builds.

## Practical hardware

| Option | Notes |
|---|---|
| **Cloud: one x86_64 instance + one ARM instance** (Graviton / Ampere) | Easiest, closest to HeterSec's two-server setup. Recommended. |
| **Two boards** — x86_64 box + Raspberry Pi 5 / Radxa | Cheapest real hardware; HeterSec-style. |
| **Apple Silicon Mac** — aarch64 native + x86_64 under Rosetta 2 | Two ISAs on one machine, no second host. Rosetta is AOT translation at launch, not runtime switching, so this is fine for A/B testing but is *not* a migration substrate. |
| **QEMU** aarch64 on x86_64 | Cheapest, but the emulator is itself a confound for any security claim — use for correctness only. |

## What a positive result would and would not prove

**Would prove:** a pre-built, single-architecture ROP payload fails on the other
architecture, on modern software with modern mitigations. That is a real,
publishable replication with a thirteen-year-newer target.

**Would not prove:** that an adaptive attacker is defeated. For that you need the
retry experiment with crash detection and throttling. Expect the adaptive attacker
to succeed in ~2 attempts at N=2 — and report that honestly, because it is the
result that actually matters.
