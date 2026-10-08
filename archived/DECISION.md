# Decision: ceremony-mode execution

Supersedes the performance discussion in the previous version of this file and
the kernel verdict in `SUBSTRATE_SPECTRUM.md`.

## The reframing

Not a faster general-purpose defense. A **one-shot, high-assurance execution
mode** for operations like a **key signing ceremony**.

That single change dissolves three objections at once — including the one I
called fatal.

### 1. Performance is irrelevant

The operation happens once. Even 100× slower is immaterial when the ceremony has
minutes of wall-clock and milliseconds of compute. Delete performance as a
criterion. Report the number once for completeness and move on.

### 2. The retry objection dies — and this was my strongest one

Throughout the analysis I argued that ISA choice caps at log₂(N) bits, so an
attacker who can retry wins in ~2–3 attempts. That was correct *for an always-on
server*.

In a ceremony the attacker gets **A ≈ 1**. Re-running my own attempt-budget
model (`MULTI_BACKEND_MIGRATION.md` §5.8):

```
required entropy ≥ log₂(A / T)
A = 1, T = 10⁻⁶   →   ~20 bits
A = 2, T = 10⁻⁶   →   ~21 bits
```

We are not *rate-limiting* retries. The operation is **structurally one-shot**:
there is no second attempt to make, because the ceremony has completed or has
been aborted.

Note the honest corollary: **two ISAs (1 bit) is still a coin flip** — 50% is not
good enough even at A = 1. The ceremony model makes the *scheme* viable; a
randomized encoding (~8 bits per opcode) is what makes it *strong*. Both are
needed.

### 3. The kernel objection dissolves too

A key signing ceremony already runs on a bespoke, air-gapped, single-purpose,
audited machine. Requiring a custom kernel there is completely normal — it is not
a deployability problem in the way it would be for production servers. So
HeterSec's kernel-mediated whole-process migration is back on the table, and it
remains the only approach that migrates a real application **unmodified**.

(It should still be demonstrated under a hypervisor in VMs — HeterSec already
ships QEMU images, so this is nearly free.)

## A new first-class requirement: crash must ABORT, not restart

HeterSec observed that the failed exploit "caused the Nginx process to crash and
restart." For an always-on server that is fine, even desirable.

**For a ceremony, restart is the wrong behaviour — it hands the attacker a
retry.** So the spec is:

> Wrong encoding → illegal-opcode trap → **ceremony aborts**, no automatic
> resume, alarm raised.

This is a genuine delta from the paper and it is what closes the retry hole
structurally rather than statistically.

## What is actually novel versus HeterSec

| | HeterSec (RAID 2020) | This work |
|---|---|---|
| Target | Always-on servers (Nginx, Redis) | One-shot high-assurance operations |
| Unpredictability | Choose between 2 ISAs — **1 bit** | Randomized, sparsified encoding — ~8 bits/opcode |
| Overhead tolerance | ~15% (hard constraint) | Irrelevant |
| Failure behaviour | Crash and **restart** | Trap and **abort** |
| Attempt budget | Unbounded (attacker retries) | **~1 by construction** |
| Possible addition | — | Post-hoc attestation of which representation ran |

## Demonstrating value

**Application**: OpenSSL, or a signing tool built on it. OpenSSL is the crypto
library everybody already trusts for signing, and CMS is the format for
signed/enveloped data — so a ceremony that parses CMS is exactly the shape we
want.

**Primary CVE — CVE-2025-15467.** Stack-based buffer overflow in OpenSSL CMS
parsing of `AuthEnvelopedData` / `EnvelopedData` with AEAD ciphers (AES-GCM
family), triggerable by a maliciously crafted message, unauthenticated, with
denial of service or remote code execution reported. Stack overflow → control-flow
hijack → ROP is precisely the class we need.

**Secondary, thematically perfect — CVE-2025-68973.** Buffer overflow in GnuPG's
`armor_filter` (`g10/armor.c`, ≤ 2.4.8) via ASCII-armor parsing. Key signing
ceremonies *import armored keys* — this is the ceremony workflow itself. Weaker
RCE path, so secondary.

*(Both need the RCE path verified before we rely on them. `MODERN_CVE_EVAL.md`
holds the confound checklist — stack canary, PAC, BTI, toolchain drift — which
applies to both.)*

**Retained for the HeterSec replication**: CVE-2025-62507 (Redis `XACKDEL` stack
overflow, JFrog-demonstrated RCE) — keep it if we also want the direct
apples-to-apples comparison against the paper's §4.1.

### The demo

1. Signing/verification path runs in ceremony mode.
2. Baseline: CVE-2025-15467 payload → RCE → key material compromised.
3. Ceremony mode: same payload → wrong encoding → trap → **ceremony aborted**,
   alarm raised, key material safe.
4. Report the empirical success rate over N trials under two conditions:
   - 2-ISA choice → expect ~50% (demonstrates why 1 bit is insufficient)
   - randomized encoding → expect ~0% (demonstrates the fix)
   
   That contrast is itself a publishable result.

## Next steps

1. Verify the RCE path for CVE-2025-15467 and confirm an affected OpenSSL version
   we can build for two architectures.
2. Stand HeterSec up in VMs under a hypervisor; kernel source already cloned at
   `hetersec2/src/hetersec-kernel`.
3. Build the ceremony harness: one-shot, abort-on-trap, no restart.
4. Run both conditions, report success rates.
5. Report overhead once, note it is out of scope, stop.
