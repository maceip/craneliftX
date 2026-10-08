# Plan — ceremony-mode execution

**Goal:** demonstrate that a one-shot high-assurance operation (key signing
ceremony) can run under an unpredictable code representation, so a real
memory-corruption exploit fails and aborts instead of succeeding.

**Not the goal:** performance. One-shot operation; even 100× slower is
immaterial. Report the number once and move on.

---

## Order of operations

| # | Step | Blocked by | Needs |
|---|---|---|---|
| 1 | Verify pulley-interpreter takes a custom opcode map | — | source read only |
| 2 | Build randomized opcode permutation | 1 | Rust |
| 3 | Build per-block integrity tripwire | 2 | Rust |
| 4 | Build ceremony harness (one-shot, abort-on-trap) | 2, 3 | Rust |
| 5 | **Measure success rate: choice vs randomized** | 4 | one machine |
| 6 | Verify CVE-2025-15467 RCE path, build OpenSSL | — | research + build |
| 7 | Run real-CVE ceremony demo | 5, 6 | — |
| 8 | HeterSec comparison in VMs | *deferred* | external artifacts |

Steps 1–5 are the demonstration. Steps 6–7 upgrade it to a real CVE. Step 8 is
only for the delta versus the RAID 2020 paper and is not needed to demonstrate
the concept.

Tracked as tasks #7–#14.

---

## What each step actually is

**Step 1 — is the opcode map pluggable?**
`pulley-interpreter` currently has a fixed encoding. Determine whether bytecode
can be decoded under a per-process permutation without forking the crate
(inspect `pulley/src/decode.rs`, `opcode.rs`). This is the only genuine unknown
inside step 1 and it sets the effort for everything after it.

**Step 2 — randomized opcode permutation.** Per-process random injective remap
of Pulley's **220 primary opcodes**, keyed from a CSPRNG at ceremony start. This
is the entropy source: ~8 bits per opcode the attacker must guess.

**Step 3 — make a wrong guess fail loudly.** Preferred: a **keyed integrity
check over each bytecode block**, verified by the interpreter before execution.
Attacker-authored bytecode fails the check and traps.

This is better than my earlier sparsification idea and should replace it: it is
simpler, stronger, and — importantly — needs **no change to Pulley's instruction
format**. Sparsifying to a 2-byte opcode space would require modifying the
encoder/decoder. Keep sparsification only as a fallback.

**Step 4 — ceremony harness.** One-shot wrapper. Central requirement: **a trap
must ABORT and raise an alarm; it must not restart.** That is the structural fix
for the retry objection (attempt budget A ≈ 1) and is a deliberate delta from
HeterSec, which observed crash-and-restart.

**Step 5 — the headline result.** Run N trials against (a) a choice between two
representations and (b) the randomized encoding. Expect ~50% versus ~0%. That
contrast is the whole argument.

**Step 6/7 — real CVE.** CVE-2025-15467 (OpenSSL CMS stack overflow) is ideal
because CMS is the format for signed/enveloped data, so it is the ceremony
workflow itself. Fallback: CVE-2025-62507 (Redis `XACKDEL`, JFrog-demonstrated
RCE) if the OpenSSL RCE path does not hold up.

---

## Dependencies

**Have already:**
- Cranelift HEAD + Pulley cloned and analysed (`upstream/`)
- `probe/` — all five backends compile one block in one process (measured)
- Pulley opcode space enumerated: 220 primary + 323 extended
- HeterSec source + 700 MB kernel (`hetersec2/`)
- rustc 1.96 (published cranelift requires it)
- Prior art: HIPStR, Popcorn, HeterSec
- Threat model, entropy budget, attempt budget

**Need to build (nothing exists yet):**
1. Randomized opcode permutation
2. Per-block integrity tripwire
3. Ceremony harness
4. CVE-2025-15467 reproduction

---

## Are we blocked?

**Not on steps 1–5.** Everything needed is local.

**Blocked / at risk on later steps:**

| Risk | Status |
|---|---|
| pulley-interpreter may need forking for a custom opcode map | **Unknown — step 1 resolves it.** Only real blocker in the critical path. |
| CVE-2025-15467 may be DoS-only, not RCE | **Unknown.** Sources say "DoS or RCE". Step 6 resolves; fallback CVE ready. |
| HeterSec VM images (Google Drive) may be gone | **External.** Affects step 8 only. |
| `popcorn-compiler` / its docker image may be unavailable | **External.** Affects step 8 only. |

---

## What we need to figure out

1. Is the Pulley decode path pluggable? (step 1) — decides effort
2. Does CVE-2025-15467 have a working control-flow-hijack path? (step 6)
3. Can the interpreter enforce an integrity check cheaply enough? (step 3)

## What we deliberately do NOT need

For steps 1–5: **no second machine, no custom kernel, no popcorn-compiler, no
HeterSec, no hypervisor, no performance work.**

One simplification worth flagging: in ceremony mode the "attacker has arbitrary
read" concern is much weaker — a ceremony box is already air-gapped,
single-purpose and controlled. So the sandbox/hypervisor isolation that
`SUBSTRATE_SPECTRUM.md` argues for is **not required** for this demonstration.
That removes a whole dependency.
