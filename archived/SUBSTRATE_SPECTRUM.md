# The substrate spectrum: where should this work live?

## Reframe

The goal is **to demonstrate the concept** — that moving execution between code
representations makes an attacker's payload fail. HeterSec (RAID 2020) is the
existing proof, not the thing we are extending.

Two things have changed since 2020, and both are disqualifying for their
approach:

1. **Nobody writes kernel modules for this any more.** Partly loss of skill,
   partly because a kernel module is the *worst* place to put new attack surface.
   Your rule — "we don't want to be in the kernel" — is correct on security
   grounds alone, independent of fashion.
2. **The isolation question has moved.** In 2020 the runtime lived in the same
   address space as the application. That is exactly the weakness that makes
   secret-encoding defenses fail against an attacker with a read primitive.

## The spectrum

| Layer | Privilege | Can the application read the secret? | Verdict |
|---|---|---|---|
| **Kernel module** (HeterSec 2020) | Ring 0 | n/a | **Not off the table — and for a demonstration, probably the right base.** It is the only approach that migrates a real, **unmodified** application. Cost: highest privilege, unmaintainable outside a research group, 505 µs. Privilege is addressable by demonstrating under a hypervisor in VMs (HeterSec already ships QEMU images). See `DECISION.md`. |
| **Userland, same process** (HeterSec-2 RPC design) | Ring 3 | **Yes — arbitrary read recovers the opcode map** | Fastest and most portable, but the secret is exposed. Fine for cost/perf work; weak for the security claim. |
| **Micro-VM / embeddable VMM** (Hyperlight) | Hardware-isolated guest | **No** | **Sweet spot.** Isolation for the runtime, no kernel, low overhead, embeddable as a library. |
| **Full VMM** (OpenVMM) | Hardware-isolated guest | No | Capable and can host a whole guest OS; heavier than needed for function-level work. |
| **Emulator** (QEMU) | Ring 3 | n/a | Wrong trust boundary — removes the W+X requirement and gives the attacker a fixed host-ISA target. Use only as a correctness oracle. |

## The load-bearing insight

Every secret-encoding defense dies to the same objection: **the mapping is in
memory, so an attacker with arbitrary read recovers it.** I flagged this as the
standard limitation throughout the earlier analysis and had no answer for it.

A micro-VM answers it. If the interpreter and the opcode mapping live inside a
Hyperlight sandbox, they are in a *different hardware-isolated address space*.
An attacker with full arbitrary read **in the application** still cannot read
them. That upgrades the defense from "holds against a blind attacker" to "holds
against an attacker who owns the guest's memory."

That is the argument for putting this work at the micro-VM layer rather than in
userland or in the kernel — and it is a stronger argument than simple
modernisation.

## Recommended demonstration

```
  Redis 8.2.2 (C, stock clang)
        |
        |  function boundary, policy decides
        +-------------------------------+
        |                               |
        v                               v
  native execution              Hyperlight micro-VM
  (aarch64, Cranelift/LLVM)     Pulley bytecode
                                per-process randomized opcode map
                                <-- hardware isolated: secret not
                                    readable from the application
```

1. Application runs natively.
2. At selected function boundaries, execution moves into the sandbox, which runs
   the same function as Pulley bytecode under a randomized encoding.
3. Attacker fires **CVE-2025-62507** (stack buffer overflow in Redis `XACKDEL`,
   ROP to RCE) with a payload authored for the native ISA.
4. If the function is executing under the randomized encoding, the payload is
   meaningless → illegal-opcode trap → crash → detection.

This is HeterSec's §4.1 experiment — a real ROP exploit succeeding on one
representation and failing on another — with four upgrades:

- **2025 CVE**, not 2013 (see `MODERN_CVE_EVAL.md`; Redis is also HeterSec's own
  benchmark, so the comparison is direct).
- **No kernel module.**
- **No second machine** — the sandbox is local.
- **The secret is isolated**, so the result does not depend on assuming a blind
  attacker.

## Where Cranelift fits

Cranelift is the natural fit here, and this is the version of the original
concept that survives:

- **Pulley** is the arch-neutral bytecode and is already a Cranelift `TargetIsa`.
- **Randomized opcode map** over Pulley's 220 primary opcodes (verified:
  `for_each_op!`, `pulley/src/lib.rs:86–622`), sparsified to 2-byte opcodes so
  wrong guesses trap ~99.7% of the time.
- **Cranelift** compiles the same CLIF for whichever native target is needed.

So: original concept (runtime migration between backends) → narrowed to what is
actually defensible (migration between *representations*, one of which has a
secret encoding, isolated in a micro-VM).

## What I need to know from you

You mentioned existing work in the Hyperlight / OpenVMM direction. Before I build
on that assumption:

- Is there an existing repo or branch I should read?
- Is the preference **Hyperlight** (embedding, function-level, minimal) or
  **OpenVMM** (full VMM, guest OS)?
- Does the existing work already cover sandbox embedding, or would I be starting
  that from scratch?

I have not assumed an answer to any of these.

## Open items

- Verify the Hyperlight embedding API and its actual per-call overhead before
  committing to the function-boundary switch; the demo's credibility rests on
  that number.
- `MODERN_CVE_EVAL.md` lists the confounds that must be controlled for
  CVE-2025-62507 (stack canary, PAC, BTI, toolchain drift). Those apply here too.
- The HeterSec-2 RPC design (`HETSEC2_DESIGN.md`) remains the right answer if you
  want the *performance* result specifically — it is the path to beating 505 µs,
  independent of where the sandbox lives.
