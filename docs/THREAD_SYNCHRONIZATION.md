# Native thread synchronization

`--thread-sync adaptive` selects the default native policy. An uncontended
executor keeps inline memory checks under one block-wide guard. When another
native thread enters, ordinary blocks use individually locked memory helpers,
so native computation can overlap. `--thread-sync block` retains the serialized
reference path; `--thread-sync access` forces the per-access path whenever the
process has multiple guest threads. The server accepts the same values as
`execution.thread_sync` in TOML. These choices do not disable any mitigation.

The process address-space mutex remains the authority for guest bytes, mapping
lifetime, allocation/shadow state and PAC metadata. A checked scalar operation
holds it through the entire check, access and metadata update. A free, unmap,
permission change or reuse cannot slip between the check and the access. Rust
references and host mapping pointers remain protected by this mutex; a native
block executing without it has its inline memory path disabled and no published
memory views. Changing native thread ownership invalidates cached views, as do
borrowed slices and mapping changes. Syscall access relinquishes native ownership.

Blocks containing a decoded `LOCK` prefix or an implicit memory `XCHG` retain
the guard for the whole block. Their read/modify/write operations, tag checks
and pointer authentication therefore remain indivisible, including on faults.
Ordinary loads/stores are ordered by the same mutex. This is stronger ordering
than x86 TSO, so supported `MFENCE`, `LFENCE` and `SFENCE` need no extra host
instruction. A normal non-LOCK read/modify/write may interleave with another
thread, as an x86 program must already allow. Bulk copies retain their guarded
operation, checked prefix and precise fault behavior. RV64 retains serialized
block execution.

Thread entry/exit counters are scoped. A failed host spawn or child error does
not permanently force the multithreaded path. A host fork resets the child's
executor counters. This does not establish general multithreaded-fork safety;
the Nginx validation forks its workers before creating their thread pools.
Dispatch cache hits no longer acquire the address-space lock, and native stack
checks reuse the block guard when present. CFI lookup still runs on every dispatch.

Reports distinguish `native_concurrent_blocks` from `native_serialized_blocks`;
child totals appear in `thread_execution_totals`. The first is a count of blocks
using per-access synchronization, not a measurement of simultaneous CPU activity
or a promise of multicore scaling. Guest memory operations still serialize.
Forced per-access synchronization can lose throughput on memory-heavy workloads;
the adaptive policy exists to avoid paying that cost merely because idle I/O
threads exist.

## Verification

```sh
cargo test --locked
# On a Linux x86-64 host with gcc and musl-gcc:
cargo build --release --locked --bin isr-process
python3 scripts/verify_thread_synchronization.py \
  --runner target/release/isr-process --output /tmp/lockstep-thread-proof
python3 scripts/verify_heap_mitigations.py --backend x86_64 --keep-ledgers
```

The Rust tests execute translated code on host threads: native-entry overlap,
contended locked arithmetic across all scalar widths and an unaligned page
boundary, implicit-lock exchange, compare/exchange flags, ordered publication,
mapping/permission changes during stores, freed-memory rejection, pointer
rejection, checked-copy prefixes, and return to warm inline views. These tests
check behavior and faults rather than treating a successful build as proof.

The C fixture is compiled with the on-host glibc and musl toolchains. The harness
requires the exact native result under all three synchronization policies,
including four actual guest thread starts, atomic totals, TLS values, mutex and
condition-variable coordination, allocation/free and thread joins. Hardened runs
retain all mitigation flags. Nginx measurements separately verify every response
and both workers with their four-thread pools. These are bounded validation
workloads, not a proof of all Linux programs or all possible thread schedules.

## Recorded result, 2026-10-20

[Measured results and validation receipt](evidence/2026-10-20-thread-synchronization.json)
cover the same static glibc and musl Nginx binaries before and after this change.
Two alternating 20-second trials per runner/libc produced 26,714 verified
responses with zero failures. Throughput stayed broadly in the previous range;
the glibc baseline varied substantially, so no statistically significant speedup
is established. This change preserves a fast uncontended path while adding
safe concurrent native computation; it does not eliminate serialization of
guest-memory operations.
