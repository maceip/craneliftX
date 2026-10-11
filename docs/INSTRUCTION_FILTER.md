# Lightweight instruction screening

The process runner inspects each reached x86 block after decoding and before
CLIF lowering. Policy `process-block-v1` uses iced-x86 instruction metadata,
at most 32 instructions per cache miss, and the existing instruction-boundary
map. It does not disassemble the whole input or run taint/data-flow analysis.
Cached execution does not repeat the static scan; recompilation does.

This restores the lightweight shellcode-oriented inspection idea for the RV64
process path. A policy rejection identifies an unsupported or prohibited
instruction pattern, not proof of malicious intent. Passing does not establish
that a binary is shellcode-free. Raw byte streams are not a new input format:
this path still accepts the supported static x86-64 ELF profile.

| Check | Compile-time policy | Runtime enforcement |
| --- | --- | --- |
| Instructions and syscall boundary | Reject privileged instructions, interrupts, far transfers and VM calls. Allow Linux `SYSCALL`. | The existing bridge validates the actual syscall number, arguments, guest buffers and descriptor table. |
| Stack-pointer operations | Allow full-RSP immediate ADD/SUB, alignment AND, MOV from RBP, and LEA from RSP/RBP without an index. Reject other explicit RSP/ESP/SP reassignments. Read-only comparisons are allowed. | Generated RV64 checks every RSP write, including PUSH/POP/CALL/RET, against the fixed guest stack. The first invalid value stops the block, even if a later instruction would restore it. |
| Memory operands | Check the full width and permissions of fixed absolute/RIP-relative accesses. Reject segment-relative accesses. LEA is address calculation, not a memory access. | Register-derived accesses use the checked RV64 memory bus. |
| Control flow | Direct targets must be executable and outside known instruction interiors. Decoded instructions cannot overlap previously compiled boundaries. | Newly reached indirect targets are decoded and inspected before compilation; the cache retains known boundaries across invalidation. |

Boundaries are learned from reached, successfully compiled code. An unseen
address is not proven to be an instruction start in the original program.
This is not general control-flow integrity, a gadget-chain detector, an unpacker,
or signature matching. Only the reached instructions are inspected. Unsupported
semantics can still fail at lowering after passing this policy. The fixed stack
profile excludes alternate stacks and arbitrary stack-pointer assignment.

## Live evidence and colors

Each compilation emits `audit` started/completed/failed spans plus a `security`
event containing policy version, block ID, source RIP, instruction count,
per-rule verdicts and address/assembly/reason for each finding. Sequence numbers,
wall-clock timestamps and monotonic timing come from the existing durable
ledger. Findings stream through the existing SSE journal/replay path.

The dashboard shows **Passed** in green, **Runtime checked** in amber, and
**Rejected** in red, always with text labels. Amber records that a runtime guard
applies; it is not a claim that execution has already passed that guard. An
actual stack violation emits a red `security` event with `phase: execute`, the
offending RSP/RIP and `status: failed`. Other runtime failures retain their
existing execute/syscall error records. Old journals receive no invented audit.

The real Rust TCP acceptance requires one accepted inspection per compiled
block, ordering before lowering, and live inspection delivery while the first
TCP connection is open. It still verifies twelve responses on that connection
without reconnecting, cache invalidation, and fifteen responses overall.
Negative CLI/browser cases exercise actual forbidden instructions and a
transient stack escape through generated RV64 code.
