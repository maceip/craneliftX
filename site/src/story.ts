export type View =
  | "service"
  | "overflow"
  | "data"
  | "metadata"
  | "byte"
  | "wrap"
  | "tags"
  | "quarantine"
  | "placement"
  | "pac"
  | "cfi"
  | "diversify"
  | "isolation"
  | "receipt";
export type Chapter = {
  id: View;
  part: "exploit" | "defense" | "evidence";
  actor: "operator" | "attacker";
  kicker: string;
  title: string;
  description: string;
  command: string;
  output: string[];
  label: string;
  status: string;
  detail: string;
  metric: string;
  value: string;
  note: string;
  source: string;
};
const repo = "https://github.com/maceip/lockstep/blob/main/";
export const chapters: Chapter[] = [
  {
    id: "service",
    part: "exploit",
    actor: "operator",
    kicker: "00 / THE SERVICE",
    title: "It starts with ordinary memory.",
    description:
      "A service receives a request. A buffer holds its input. Nearby, another object holds the address of the function it will call next.",
    command: "./nginx -c nginx.conf",
    output: [
      "worker 01 · listening",
      "request.buffer  = allocate(32)",
      "request.handler = handle_request",
      "waiting for input…",
    ],
    label: "Two allocations. Two owners.",
    status: "SERVICE RUNNING",
    detail:
      "The pointer on the stack refers to the input buffer. The callback belongs to a different allocation.",
    metric: "BUFFER CAPACITY",
    value: "32 bytes",
    note: "A schematic Nginx-style service, not an exact Nginx object layout. Each visible heap block represents 8 bytes. Addresses and callback names are illustrative.",
    source: repo + "tests/fixtures/heap_overflow.rs",
  },
  {
    id: "overflow",
    part: "exploit",
    actor: "attacker",
    kicker: "01 / CALLBACK OVERWRITE",
    title: "Send 48 bytes. Own only 32.",
    description:
      "An unchecked copy crosses the allocation boundary. The excess bytes replace a callback. A later call follows the changed address.",
    command: 'request.body = repeat("A", 48)',
    output: [
      "copy → buffer[0..32]     within bounds",
      "copy → buffer[32..48]    out of bounds",
      "callback: handle_request → other_target",
      "later: call(callback)   → redirected",
    ],
    label: "The write changes the next call.",
    status: "CALLBACK CORRUPTED",
    detail:
      "Red blocks lie outside the input buffer. The red route is a later use of the changed callback—not the write itself.",
    metric: "OUTSIDE OWNERSHIP",
    value: "+16 bytes",
    note: "The attacker must reach the field and produce a usable address. The target must be callable and survive applicable checks; corruption does not automatically yield execution.",
    source: repo + "tests/fixtures/pointer_substitution.rs",
  },
  {
    id: "data",
    part: "exploit",
    actor: "attacker",
    kicker: "02 / DATA-ONLY CORRUPTION",
    title: "The code can stay exactly the same.",
    description:
      "A neighboring field controls a decision. Change DENY to ALLOW and the service may do the wrong thing without ever jumping to injected code.",
    command: "overflow.reaches = permission",
    output: [
      "input buffer             full",
      "neighbor.permission      DENY → ALLOW",
      "later: check_permission()",
      "result                   access granted",
    ],
    label: "A decision is data, too.",
    status: "DECISION ALTERED",
    detail:
      "The overwritten field is a permission value. The same program instructions now read a different answer.",
    metric: "PERMISSION",
    value: "DENY → ALLOW",
    note: "A generic data-only attack. Actual exploitability depends on the field, padding, encoding, and the program's later use.",
    source: repo + "tests/fixtures/heap_overflow.rs",
  },
  {
    id: "metadata",
    part: "exploit",
    actor: "attacker",
    kicker: "03 / ALLOCATOR METADATA",
    title: "Corrupt the map of the heap.",
    description:
      "Some allocators keep bookkeeping near application data. An overflow can change a recorded size or relationship before a later allocation or free consults it.",
    command: "overflow.reaches = chunk_record",
    output: [
      "recorded boundary        real → false",
      "later: allocate / free",
      "if accepted              wrong region",
      "if checked               operation rejected",
    ],
    label: "A false description of memory.",
    status: "BOOKKEEPING ALTERED",
    detail:
      "The changed record describes the wrong boundary. Whether the allocator accepts it is a separate question.",
    metric: "CHUNK RECORD",
    value: "boundary changed",
    note: "This is a vulnerable-allocator teaching model, not a claim about a particular glibc or musl layout. Lockstep keeps its allocator bookkeeping outside guest-addressable memory.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "byte",
    part: "exploit",
    actor: "attacker",
    kicker: "04 / OFF-BY-ONE",
    title: "One byte can be enough.",
    description:
      "The data fits. Its terminating zero does not. In this layout, that single extra byte clears a neighboring lock flag.",
    command: "buffer[32] = 0x00",
    output: [
      "buffer[0..31]            32 valid bytes",
      "terminator at +32        one extra byte",
      "neighbor.locked          01 → 00",
      "later: is_locked()       false",
    ],
    label: "Only one byte changed.",
    status: "FLAG CLEARED",
    detail:
      "The thin red slice is one byte, not an entire block or a freely chosen pointer.",
    metric: "OVERWRITE WIDTH",
    value: "1 byte",
    note: "The buffer ends at a tag-granule boundary here. Tag checks are not byte-precise bounds checks: an overflow that stays inside the same tagged granule can escape this check.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "wrap",
    part: "exploit",
    actor: "attacker",
    kicker: "05 / SIZE ARITHMETIC",
    title: "A small allocation. A much bigger copy.",
    description:
      "The size calculation wraps, but the copy still uses the original amount. The allocator gives the service what it asked for—not what it meant.",
    command: "size_u8 = 18 * 16",
    output: [
      "intended size            288 bytes",
      "8-bit result             32 bytes",
      "allocate(32); copy(288)",
      "256 bytes cross the allocation boundary",
    ],
    label: "The numbers disagree.",
    status: "ALLOCATION TOO SMALL",
    detail:
      "Four green blocks hold 32 bytes. The 32 red blocks show the other 256 bytes crossing into later memory. The error starts before the write.",
    metric: "ALLOCATE / COPY",
    value: "32 / 288 bytes",
    note: "Deliberately reduced to unsigned 8-bit arithmetic so the wrap is visible. Real size_t is wider. This example explains the pattern, not a specific product bug.",
    source: repo + "tests/fixtures/cve_2026_53362_ipv6_frag.rs",
  },
  {
    id: "tags",
    part: "defense",
    actor: "operator",
    kicker: "06 / SHADOW TAGS",
    title: "Stop the write at the boundary.",
    description:
      "Run the service through Lockstep. The pointer carries a tag; each 16-byte heap granule has a matching shadow tag. The first mismatch stops the operation.",
    command: "./target/release/isr-process ./nginx --backend x86_64",
    output: [
      "pointer tag              A",
      "buffer shadow            A A",
      "neighbor shadow          B B",
      "write at +32             TagMismatch",
      "neighbor callback        unchanged",
    ],
    label: "A pointer is not permission for everything.",
    status: "WRITE REJECTED",
    detail:
      "Tag A cannot authorize a write into tag B. The shadow map is host-only; the overflow cannot rewrite its own permission.",
    metric: "FIRST INVALID WRITE",
    value: "stopped at +32",
    note: "Tags check every touched 16-byte granule. This model has a 32-byte tracked extent; libc interception currently pads and rounds allocations to 64-byte lines, so an overrun within that tracked padding can escape detection. Optional Intel LAM changes tag-bit placement, not the check.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "quarantine",
    part: "defense",
    actor: "attacker",
    kicker: "07 / FREE + QUARANTINE",
    title: "An old pointer loses its welcome.",
    description:
      "Freeing an allocation changes its shadow state and places it in a bounded quarantine. Keeping the old address does not keep the old access rights.",
    command: "free(buffer); use(stale_pointer)",
    output: [
      "allocation               released",
      "shadow                   A → quarantined",
      "stale access             UseAfterFree",
      "later reuse              fresh allocation tag",
    ],
    label: "An address can outlive its ownership.",
    status: "STALE ACCESS REJECTED",
    detail:
      "Amber blocks are quarantined. The old pointer still points there, but its tag no longer matches the memory's state.",
    metric: "LIFETIME",
    value: "live → quarantined",
    note: "Quarantine is bounded and eventual reuse is retagged. Tags have finite width; this is not a claim of permanent temporal identity or an unlimited quarantine.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "placement",
    part: "defense",
    actor: "attacker",
    kicker: "08 / RANDOMIZED PLACEMENT",
    title: "Make the neighbor hard to arrange.",
    description:
      "Heap grooming tries to place a useful target next to the vulnerable buffer. Randomized slab-slot selection disrupts a predictable sequence.",
    command: "allocate(); free(); allocate(target)",
    output: [
      "expected neighbor        target",
      "actual slot              chosen by CSPRNG",
      "repeat                   different placement",
      "adjacency                no guarantee",
    ],
    label: "The attacker cannot choose this layout.",
    status: "PLACEMENT RANDOMIZED",
    detail:
      "The target moves among valid slots. Placement randomization reduces reliable adjacency; tags still decide whether a write is allowed.",
    metric: "TARGET POSITION",
    value: "not predictable",
    note: "Randomization is probabilistic. It does not make adjacency impossible and does not replace access checks. The animation depicts possible layouts, not measured probabilities.",
    source: repo + "tests/fixtures/heap_grooming.rs",
  },
  {
    id: "pac",
    part: "defense",
    actor: "attacker",
    kicker: "09 / POINTER AUTHENTICATION",
    title: "The right pointer. The wrong place.",
    description:
      "Try to transplant raw signed pointer bytes into another slot. Their authentication no longer matches: the signature binds the value to its storage address.",
    command: "overwrite(slot_B, raw_bytes(slot_A))",
    output: [
      "raw signed bytes         submitted",
      "storage address          A → B",
      "authentication           does not match",
      "store / later load       PointerAuth",
    ],
    label: "Copying the bytes does not copy the authority.",
    status: "POINTER REJECTED",
    detail:
      "Red marks the attempted transplant. Pointer checks can reject it before writing; the signature belongs to the original slot.",
    metric: "SIGNATURE BINDS",
    value: "value + slot",
    note: "This is a raw-byte transplant. A legitimate authenticated load/store can re-sign a pointer for its new slot. Software PAC uses a truncated code on 64-bit code pointers; history and provenance also constrain replacement. It does not authenticate every data value.",
    source: repo + "src/process/pac.rs",
  },
  {
    id: "cfi",
    part: "defense",
    actor: "attacker",
    kicker: "10 / CONTROL-FLOW CHECKS",
    title: "An instruction has a beginning.",
    description:
      "A changed address may aim inside an already decoded instruction. Lockstep records instruction extents and rejects that transfer before executing the next block.",
    command: "jump(decoded_instruction + 2)",
    output: [
      "known instruction        [start … end)",
      "requested destination    inside the body",
      "control-flow check       rejected",
      "target instructions      not executed",
    ],
    label: "A byte inside an instruction is not its entry.",
    status: "TRANSFER REJECTED",
    detail:
      "The red destination falls between instruction boundaries. The recorded extent makes that destination invalid.",
    metric: "DESTINATION",
    value: "middle of instruction",
    note: "Current CFI rejects interiors of known instructions. It is not a complete source-level control-flow graph, and it does not reject every malicious but valid instruction-start target.",
    source: repo + "src/process/cfi.rs",
  },
  {
    id: "diversify",
    part: "defense",
    actor: "operator",
    kicker: "11 / JIT DIVERSIFICATION",
    title: "Killing heap memory exploits for good",
    description:
      "Each native compilation starts with a fresh register-variable permutation and randomized frame padding. A fixed view of the original binary is a poorer guide to the translated layout.",
    command: "compile(block) → compile(block)",
    output: [
      "guest behavior           preserved",
      "variable mapping         permuted",
      "frame padding            randomized",
      "result                   translated layout varies",
    ],
    label: "The layout changes. The program's meaning stays.",
    status: "COMPILATION DIVERSIFIED",
    detail:
      "These code blocks represent separate compilations. Diversification complements memory and control-flow checks; it is not a proof that all gadgets disappear.",
    metric: "PER COMPILATION",
    value: "fresh arrangement",
    note: "Cranelift may optimize away redundant construction. The runtime randomizes lowering inputs; the illustration does not claim every permutation produces unique machine code.",
    source: repo + "src/process/diversify.rs",
  },
  {
    id: "isolation",
    part: "defense",
    actor: "attacker",
    kicker: "12 / EXECUTION BOUNDARIES",
    title: "The guards live outside the heap.",
    description:
      "Guest addresses resolve only to guest mappings. Shadow tags and pointer metadata are host-only. Generated code becomes executable only after its writable phase ends.",
    command: "write(runtime_metadata)",
    output: [
      "guest mapping            not found",
      "host shadow / PAC maps   not addressable",
      "JIT pages                write → execute",
      "guest syscall            mediated by runtime",
    ],
    label: "Protect the checks, not just the checked bytes.",
    status: "ACCESS REJECTED",
    detail:
      "The separate host region contains the metadata. Mapping checks, instruction policy, stack guards, and syscall mediation keep guest operations on their supported path.",
    metric: "JIT PAGE PERMISSIONS",
    value: "RW → RX",
    note: "Writable-and-executable guest mappings are refused. MPK probing/plumbing exists, but a successful capability probe is not evidence of enforced domain isolation; this page does not present MPK as an active defense.",
    source: repo + "src/process/memory.rs",
  },
  {
    id: "receipt",
    part: "evidence",
    actor: "operator",
    kicker: "13 / VERIFY THE RESULT",
    title: "A rejection needs a reason.",
    description:
      "Run the exploit-shaped fixture with its defense enabled and disabled. A specific mitigation fault—and a successful control—shows what the check actually prevented.",
    command: "python3 scripts/verify_heap_mitigations.py",
    output: [
      "hardened                 expected fault",
      "control                  primitive completes",
      "network regression       verified response",
      "receipt                  events.jsonl",
    ],
    label: "Follow the operation. Inspect the evidence.",
    status: "OUTCOMES RECORDED",
    detail:
      "This page is an illustrated model. The repository's harness runs actual guest binaries and checks specific outcomes.",
    metric: "EVIDENCE",
    value: "hardened + control",
    note: "The CVE-shaped fixtures reproduce generic primitives, not complete exploits against the original products. Supported service inputs are static Linux x86-64 ELFs; glibc and musl Nginx are tested.",
    source: repo + "scripts/verify_heap_mitigations.py",
  },
];
