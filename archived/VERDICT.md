# Verdict

Two different questions. They have two different answers.

## The details: no (as originally scoped)

"Stop at every basic-block boundary, recompile, resume on another ISA, executed
natively."

- **Block granularity is off by 10⁴–10⁵.** Measured switch cost ~505 µs; a block
  runs in tens of nanoseconds. Function-level works (~10/sec); block-level does not.
- **Foreign-ISA code cannot execute natively.** No Cranelift component can run
  x86-64 or RISC-V bytes on this machine.

Both are fixable by relaxing the scope — which is exactly what you proposed.

## The concept: yes, and it has empirical support

Not "yes it stops attackers." **Yes it makes exploitation measurably harder, and
someone demonstrated it against a real CVE.**

HeterSec ran a ROP exploit against Nginx (CVE-2013-2028, `ngx_http_read_discarded_request_body`
integer + buffer overflow). From the paper:

> "the script **failed on the ARM64 machine and caused the Nginx process to crash
> and restart**. The stack layouts between architectures differ therefore the
> address at which the overflow gains control over the program control flow are
> not the same."

Why it failed, in their words — note that ISA-ness is only one of several factors:

- different **stack layouts** (ARMv8 puts FP/LR at the lowest frame address; x86-64
  pushes RIP/RBP at the highest)
- different **register usage** (8 GP arg registers on ARM64 vs 6 on x86-64)
- different **syscall numbers and conventions** (`execve` = 59 on x86-64, 221 on
  ARM64; `x8` vs `rax` for the syscall number)
- different **microarchitectural primitives** (`rdtsc` vs a kernel-only perf
  counter; `clflush` vs a constructed eviction pattern)

**That is the real mechanism: diversity, not ISA secrecy.** Changing the ISA is
one expensive way to buy diversity. It is not the only way, and not the strongest.

## So: does it stop shellcode if they can't know the ISA?

Depends entirely on the attacker. Three models:

| Attacker | Outcome | Why |
|---|---|---|
| **Remote, pre-built payload** for one ISA (most real exploitation) | **Defeated** | Demonstrated by HeterSec's CVE-2013-2028 run. Wrong ISA → crash → detection. |
| **Adaptive, can retry** | **Not stopped** | `P(success) = 1/N` per attempt. N=2 → 50%; N=3 → 33%. Two or three tries and they are in. |
| **Adaptive, can read memory** | **Not stopped** | Recovers the mapping / just reuses code in the always-present runtime. |

Two ISAs is **one bit**. Three is **1.58 bits**. Your instinct earlier — "they can
just try a couple times" — is exactly right, and it is the correct objection to
"unknown ISA" as a *standalone* defense. HeterSec's exploit failed because the
system happened to be on the other architecture that time, not because the
attacker faced a hard problem.

**The saving grace is that a wrong guess crashes.** HeterSec observed the crash.
If you detect and throttle crashes, you bound the attempt budget and the defense
becomes meaningful. If the service silently auto-restarts, retries are free and
it does not.

## What I'd actually build

Keep the concept. Buy the diversity more cheaply.

1. **Diversification within one ISA.** Recompile with varying register allocation,
   stack layout, and gadget sets. You get most of HeterSec's diversity — the
   stack-layout and register-usage differences that actually broke their exploit —
   without any second machine or emulator.
2. **Randomized encoding (Pulley, sparsified 2-byte opcodes).** This is the only
   version that survives the retry objection: ~8 bits per opcode, and 99.7% of
   guessed opcodes trap, so failures are loud.
3. **MVX — detection, not prevention.** Run variants on different ISAs and alarm
   on divergence. This is HeterSec's second case study and it is the one with a
   sound argument: **detection does not depend on the attacker guessing wrong.**
   Arguably the best real use of ISA heterogeneity.

If you want two machines anyway — Option C in the substrate doc — that is proven
and viable at ~505 µs/switch and ~15% on Nginx. Don't delete it. But understand
you are buying one bit of prevention, plus detection.

## Files

| File | Verdict |
|---|---|
| `MULTI_BACKEND_MIGRATION.md` §1–4 | **Keep.** Verified Cranelift scaffolding. Reusable. |
| `MULTI_BACKEND_MIGRATION.md` §5–7 | **Keep**, but read as analysis of limits, not a roadmap. |
| `EXECUTION_SUBSTRATE.md` | **Keep, don't delete** — revised. Option C (two machines) is proven; Option B (Pulley) is the strongest single-machine path; Option A (emulation) is still the wrong substrate. |
| `hetersec-raid20.pdf` | **Keep.** §4.1 has the CVE-2013-2028 evaluation. |
| `probe/` | **Keep.** Runnable; all five backends compile one block in one process. |

## Honest bottom line

**The concept works as a cost-raising measure and is demonstrably effective
against the attacker you actually face most often — the one shipping a
pre-built payload.** It does not stop a persistent attacker who can retry, because
N is small. If you want "stopped" rather than "harder," the answer is a secret
encoding (Project 2) or detection (MVX) — not more ISAs.
