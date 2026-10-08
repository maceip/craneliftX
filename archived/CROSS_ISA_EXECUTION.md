# How does the CPU actually execute a different ISA?

This is the only question that matters until it is answered. Everything else —
entropy, attempt budgets, CVE selection, performance — is downstream.

## The correction first

**A hypervisor does not give you cross-ISA execution.**

Hardware virtualization (VMX / SVM / Hyperlight / OpenVMM) runs a guest of the
*same* ISA as the host, faster and more isolated. It provides **isolation**, not
**translation**. You cannot virtualize RISC-V on an x86-64 box.

So "be a hypervisor" is not an answer to "how does the CPU execute it." What a
hypervisor *is* good for is being a safe place to put the thing that does the
translation. Those are two different components and they should be chosen
separately.

## There are exactly three mechanisms

### 1. The CPU natively implements both ISAs

| Pairing | Available today? |
|---|---|
| **AArch64 + AArch32 (EL0)** | **Yes** — on ARMv8-A servers (Graviton, Ampere Altra / Neoverse N1) with `CONFIG_COMPAT`. Two genuine instruction sets on one core, natively, no emulation. |
| **x86-64 + x86-32 (compat mode)** | **Yes** — every x86-64 core. |
| **z/Architecture + AArch64** | Announced by IBM (Hot Chips 2026). At tape-out, "a few years away." |
| **Custom silicon / FPGA soft core** | FPGA: today, slow (100–300 MHz). Custom ASIC: years and capital. |

Note the first two: **the "custom cybersecurity chip" largely exists already, in
a boring form.** An ARMv8 server running both AArch64 and AArch32 is a CPU that
natively executes two instruction sets, purchasable now. The catch is that
Cranelift has no arm32 backend (removed), so you would compile one variant with
`gcc -marm` or add a backend.

### 2. Software binary translation

Fetch the foreign bytes, decode them, and either interpret them or translate them
into host code and execute that.

- **Interpreter** — decode-and-dispatch loop. Pulley is exactly this.
- **Dynamic binary translation (DBT)** — translate a block to host code, cache it,
  execute natively. QEMU TCG, and **Apple's Rosetta 2**, which translates
  x86-64 → arm64 on every Apple Silicon Mac and is routinely reported at roughly
  70–80% of native performance.

**Rosetta 2 is the existence proof that this works and is fast enough.** It is the
strongest counter to "you can't execute a foreign ISA": you can, in software, at
acceptable cost, and it ships in hundreds of millions of devices.

### 3. Send the code to hardware that has that ISA

HeterSec's approach: compile for both, run on two machines, migrate between them.
Real, proven, ~505 µs per switch. Requires two machines.

## Where a hypervisor does fit

Binary translation has a problem I documented earlier (§5.7): **a translator
faithfully executes whatever it is given.** If the attacker can point it at their
own bytes, they get correct execution of whatever they wrote — and worse, under
emulation they don't even need an executable page.

That is what the isolation layer is for. Put the translator in a micro-VM
(Hyperlight) or at minimum a separate hardened process, so:

- the translator's own state is not reachable from the application,
- the guest encoding / opcode map is not readable by an attacker who owns the
  application's memory,
- a compromise of the application does not hand over the translation engine.

**So the architecture is: translator (does the executing) + sandbox (makes it
safe to).** Both are needed; they are not substitutes.

## Recommendation

Since you asked me to pick one: **mechanism 2, inside a sandbox.**

- It is the only mechanism available today that gives you an arbitrary
  "different architecture" without buying hardware.
- Rosetta 2 proves the performance is acceptable — which is why performance is
  correctly *not* the goal; it's already solved well enough.
- Critically, **it is the only mechanism where you control the encoding.** Native
  hardware gives you fixed, public ISAs (1.58 bits for three). A translator lets
  you use a randomized, sparsified encoding — ~8 bits per opcode, with ~99.7% of
  wrong guesses trapping. That is where the entropy you keep asking about
  actually comes from.

Mechanism 1 (AArch64 + AArch32 on commodity ARM) is worth a parallel experiment:
it is native, cheap, and requires no translator — but it gives you 1 bit and
needs an arm32 compiler. Use it if you want to demonstrate "two ISAs on a real
CPU" without software translation.

Mechanism 3 is HeterSec and is only worth it if you specifically want the
two-machine result.

## "How would the hacker know?"

You keep asking this, so, directly:

The attacker does **not** need to detect that you are translating. Detecting is
trivial (CPUID, `uname`, `/proc/cpuinfo`, timing) but irrelevant. What they need
in order to write a working payload is **the encoding that the decoder will
apply to their bytes** — and that is precisely the thing being randomized and
hidden.

They learn it only by:
- reading it out of memory (arbitrary read) → defeated by the sandbox boundary, or
- guessing → defeated by entropy plus a sparsified encoding that traps on error, or
- not needing it at all, by reusing code in the always-present runtime → the real
  residual risk, and the reason the runtime must be isolated.

## Consequence for the rest of the work

Performance work is suspended — you are right that it is not the goal. The
HeterSec-2 RPC design and the 505 µs benchmark remain valid *if* we later want
the performance result, but they are not the blocker.

The blocker is: build the translator + sandbox, get two representations running
the same program, and show CVE-2025-62507's ROP payload succeeding on one and
failing on the other.
