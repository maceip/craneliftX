# Execution substrate: emulated target vs real silicon

The question: does the migration target have to be emulated, or can it be a real
different chip?

**Answer: both are possible, and they deliver completely different security
properties.** The substrate choice is not an implementation detail — it decides
whether this is a defense or merely a relocation of the attack surface.

---

## 1. Why the substrate dominates

Your goal is an "unknown ability quotient": the attacker cannot know what the
bytes they inject will mean when executed.

That property has two possible sources:

| Source of unguessability | Requires | Entropy |
|---|---|---|
| A *different but public* ISA | Real heterogeneous hardware | log₂(N) — **1.58 bits for 3 ISAs** |
| A *secret* encoding | Nothing special | ~8 bits per opcode; 64–80 bits for a payload |

The first needs exotic hardware and is nearly worthless (§5.3 of the main
report — public encodings, polyglot shellcode). The second needs no hardware at
all and is strong. **This asymmetry should drive your decision.**

---

## 2. Option matrix

### Option A — Emulate the target ISA (QEMU / TCG)

Run the foreign-ISA blocks under QEMU user-mode or a TCG-style translator.

- **Feasibility:** high. QEMU user-mode already runs foreign binaries. You would
  orchestrate at process granularity — two emulator instances, migrate between
  them — because TCG cannot switch guest ISA mid-execution.
- **Cost:** translation overhead, plus QEMU is a very large codebase in your
  trust boundary.
- **Security verdict: fails your threat model**, for two distinct reasons:
  1. **It removes the W+X requirement.** The emulator fetches guest instructions
     from ordinary writable memory. An attacker with a write primitive and
     control of the guest PC gets their bytes executed with no executable page
     anywhere. NX no longer protects you.
  2. **It adds a large, always-resident, host-ISA target.** The emulator is
     native host code, is public, and is gadget-rich — and it must be present
     for your scheme to work at all. The attacker does not need to *detect* the
     emulator (though CPUID/timing/uname make that trivial); they simply reuse
     code in it. Your migrating blocks may be unknowable; the machinery running
     them is not.

  Note the guest-ISA guessing defense *does* still function under emulation —
  attackers must still guess the guest ISA to author executable bytes. What
  emulation breaks is everything else. See §5.9 of the main report.

Emulation is the right tool if your goal is *correctness research*. It is the
wrong tool if your goal is *defeating shellcode*.

### Option B — Pulley with a per-process randomized encoding

Cranelift's own portable bytecode, with the opcode mapping shuffled per process.

- **Feasibility:** high, and it is in-tree. Verified: 220 primary opcodes in a
  `#[repr(u8)]` 256-entry space (`for_each_op!`, `pulley/src/lib.rs:86–622`),
  plus 323 extended opcodes (from line 623). A random injective remap costs the
  attacker ~8 bits per opcode they need.
- **Cost:** interpretation overhead (Pulley is designed as a fallback, not a
  primary engine), and the interpreter becomes the trust boundary.
- **Security verdict: this is the only option that delivers real entropy on one
  machine.** Two structural advantages:
  1. ~64–80 bits for a working payload, versus 1.58 bits for ISA choice.
  2. **Pulley bytecode is data, decoded by a fixed loop** — there is no
     writable-and-executable region holding user code, so "write bytes, jump to
     them" has nothing to jump into.
- **Caveat:** holds against a *blind* attacker. Arbitrary read recovers the
  dispatch table.

### Option C — Two real machines, different ISAs

This is exactly what HeterSec built.

- **Feasibility: proven.** HeterSec ran x86_64 + ARM64 servers over InfiniBand,
  ~505 µs per switch, ~15% overhead on Nginx.
- **Cost:** you inherit a distributed-systems problem. HeterSec needed a
  distributed kernel with per-process page-table sync. A lighter user-space-only
  variant (shared-memory state struct + RPC, migrate at function boundaries)
  avoids kernel work and is far more tractable.
- **Security verdict: real ISA diversity, and it is genuine.** But remember the
  entropy ceiling: log₂(N) bits. Two machines = 1 bit. It raises cost against
  mass-produced payloads; it does not stop a bespoke attacker.

### Option D — FPGA fabric with a soft core

Zynq UltraScale+, Versal, PolarFire SoC — a hard ARM or RISC-V host plus
programmable fabric in which you synthesize a second ISA.

- **Feasibility:** real but slow. Soft cores run at roughly 100–300 MHz, and
  *you* own cache coherence and memory sharing.
- **Security verdict:** genuinely single-chip heterogeneous, which is nice
  rhetorically, but the soft core is slow enough that you will run almost
  nothing there, and you have hand-written the coherence layer — a large new
  attack surface of your own making.

### Option E — AArch64 + AArch32 on the same ARM core

Real ISA diversity on cheap hardware, natively, with no emulation.

- **Feasibility:** real on ARMv8 cores that retain AArch32 at EL0 (with
  `CONFIG_COMPAT`), e.g. Cortex-A53/A72 class parts.
- **Security verdict:** attractive in principle, but **Cranelift has no arm32
  backend** — it was removed. You would have to write one. That is a large
  project and it does not advance the core migration problem.

### Option F — A real dual-ISA chip

IBM announced the first dual-ISA core at Hot Chips 2026 (August): a next-gen
Z/LinuxONE core that natively executes both **z/Architecture and AArch64** on
the same core, 2nm, planning KVM guests of both.

- **Security verdict:** this is the hardware you actually want, and notably
  **Cranelift already has backends for both of its ISAs** (`s390x` and
  `aarch64`).
- **Availability: not purchasable.** Reporting is explicit that the chip is
  going to tape-out and is "still a few years away." Treat it as a signal that
  dual-ISA silicon is arriving, not as a platform you can build on this year.

---

## 3. Recommendation

**Do not make real silicon a precondition.** It is the expensive path and,
because public ISAs cap you at log₂(N) bits, it is also the *weak* path.

Build in this order:

1. **Now, one machine: Pulley with randomized encoding.** This is where the
   entropy actually is. No exotic hardware, no kernel work, and it gives you the
   property you asked for — an attacker cannot write bytes that work.
2. **Then, if you want amplification: add a second real ISA.** Two boards
   networked is the cheapest real-silicon path and is proven by HeterSec. Treat
   it as *additional* diversity layered on top of the secret encoding, not as
   the primary mechanism.
3. **Use emulation only as a test oracle**, never as the deployed substrate.
   QEMU is the wrong trust boundary.

The hybrid that best matches your stated goal: run ordinary code natively for
speed, and migrate the security-critical windows into randomized-encoding
interpreted blocks. You get the "unknown ability quotient" on commodity
hardware, and real silicon becomes optional amplification rather than a blocker.

---

## 4. Roughly what real silicon costs (verify current pricing)

| Path | Example | Note |
|---|---|---|
| Second machine, ARM | Raspberry Pi 5 / Radxa / Orange Pi | Cheapest real second ISA |
| Second machine, RISC-V | VisionFive 2, Banana Pi F3, Milk-V Jupiter | RV64GC; F3 is the budget option |
| Second machine, faster link | 10 GbE or USB4 between two boxes | HeterSec used InfiniBand (17.6 µs ping-pong); Ethernet is slower but fine at ~10 switches/sec |
| FPGA fabric | Kria KV260 (Zynq UltraScale+), PolarFire SoC Icicle | Soft cores ~100–300 MHz; you write coherence |
| Dual-ISA silicon | IBM next-gen Z / LinuxONE | Tape-out; years away |

---

## 5. What the probe establishes

`probe/` compiles one basic block for every backend in a single process and
reports how each backend physically places the same logical state.

```
cd probe
RUSTC="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin/rustc" \
  $HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin/cargo run --release
```

Two gotchas: published `cranelift-codegen` 0.136.2 requires **rustc 1.96.0**
(the system default may be older), and `machine_code_cfg_info` must be set to
`true` or `bb_starts` comes back empty.

Measured output (saved in `probe/RESULTS.txt`):

```
target       bytes     fp_off     ss32_off   relocs    calls   blocks
------------------------------------------------------------------
x86_64          69         48            0        1        1        1
aarch64         72         48            0        1        1        1
riscv64        100         48            0        1        1        1
s390x           58        192          160        1        1        1
pulley64        30         48            0        1        1        1
```

Three conclusions, all substrate-independent:

1. **All five backends coexist in one process.** The compilation half is solved
   regardless of which substrate you choose to execute on.
2. **Code size varies 30–100 bytes for one block** — 5 distinct. Any scheme that
   assumes uniform block sizes across targets is wrong.
3. **s390x is the outlier, and it is an outlier by exactly the register save
   area.** SP-to-FP distance 192 vs 48, and the 32-byte stack slot lands at
   offset 160 instead of 0 — matching `REG_SAVE_AREA_SIZE = 160` from the ABI
   analysis. x86_64, aarch64 and riscv64 happen to agree at 48 for *this* block,
   but that is a coincidence of a trivial frame, not a guarantee: the real
   divergence (setup area 16 vs 0, differing callee-saved sets, differing
   clobber encodings) surfaces in frames with more live state.

That last point is the caution: **do not conclude from this table that three of
the four ISAs agree.** They agree because the block is small. The migration
frame must treat every target as independently laid out.
