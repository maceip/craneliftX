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
    title: "A service allocates an input buffer.",
    description:
      "A service stores request data in a heap buffer. A nearby allocation holds a callback: a function pointer that the service will call later.",
    command: "./nginx -c nginx.conf",
    output: [
      "worker 01 · listening",
      "request.buffer  = allocate(32)",
      "request.handler = handle_request",
      "waiting for input…",
    ],
    label: "The callback has its own allocation.",
    status: "SERVICE RUNNING",
    detail:
      "The pointer on the stack refers to the input buffer. The callback belongs to a different allocation.",
    metric: "BUFFER CAPACITY",
    value: "32 bytes",
    note: "This example shows a simplified service using an Nginx command. It does not reproduce the memory layout of Nginx. Each heap block represents 8 bytes. Addresses and function names are examples.",
    source: repo + "tests/fixtures/heap_overflow.rs",
  },
  {
    id: "overflow",
    part: "exploit",
    actor: "attacker",
    kicker: "01 / CALLBACK OVERWRITE",
    title: "A 48-byte copy exceeds a 32-byte buffer.",
    description:
      "An unchecked copy writes 16 bytes past the buffer and overwrites a nearby callback. When the service calls that function pointer, it uses the attacker's address.",
    command: 'request.body = repeat("A", 48)',
    output: [
      "copy → buffer[0..32]     within bounds",
      "copy → buffer[32..48]    out of bounds",
      "callback: handle_request → other_target",
      "later: call(callback)   → redirected",
    ],
    label: "The overflow overwrites a callback.",
    status: "CALLBACK CORRUPTED",
    detail:
      "The red blocks are outside the input buffer. The red arrow shows the service calling the overwritten function pointer after the copy.",
    metric: "BYTES PAST BUFFER",
    value: "+16 bytes",
    note: "To redirect execution, the attacker must reach the callback and write a usable function address. The address must also pass any checks applied before the call. Overwriting the callback alone does not guarantee code execution.",
    source: repo + "tests/fixtures/pointer_substitution.rs",
  },
  {
    id: "data",
    part: "exploit",
    actor: "attacker",
    kicker: "02 / DATA-ONLY CORRUPTION",
    title: "An overflow can change a permission.",
    description:
      "An overflow changes a nearby permission field from DENY to ALLOW. The service then grants access when it reads that field. The attacker does not need to inject code.",
    command: "overflow.reaches = permission",
    output: [
      "input buffer             full",
      "neighbor.permission      DENY → ALLOW",
      "later: check_permission()",
      "result                   access granted",
    ],
    label: "The service reads the changed value.",
    status: "DECISION ALTERED",
    detail:
      "The permission field has changed. The service executes the same instructions, but its permission check now returns a different result.",
    metric: "PERMISSION",
    value: "DENY → ALLOW",
    note: "This example changes program data without changing code. Whether it works depends on the location and encoding of the field, any padding before it, and how the program uses the value.",
    source: repo + "tests/fixtures/heap_overflow.rs",
  },
  {
    id: "metadata",
    part: "exploit",
    actor: "attacker",
    kicker: "03 / ALLOCATOR METADATA",
    title: "An overflow can alter allocator records.",
    description:
      "Some allocators store allocation records near application data. An overflow can change a size or pointer in those records. A later allocation or free may use the corrupted record.",
    command: "overflow.reaches = chunk_record",
    output: [
      "recorded boundary        real → false",
      "later: allocate / free",
      "if accepted              wrong region",
      "if checked               operation rejected",
    ],
    label: "The allocator reads a changed boundary.",
    status: "BOOKKEEPING ALTERED",
    detail:
      "The overwritten record now describes an incorrect allocation boundary. The allocator may use that value or reject it, depending on its checks.",
    metric: "RECORDED BOUNDARY",
    value: "boundary changed",
    note: "This example shows an allocator that stores records beside application data. It does not reproduce a particular glibc or musl layout. Lockstep stores its own allocator records outside the memory the service can address.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "byte",
    part: "exploit",
    actor: "attacker",
    kicker: "04 / OFF-BY-ONE",
    title: "A terminator writes one byte too far.",
    description:
      "The input fills the 32-byte buffer. Writing its terminating zero at offset 32 exceeds the buffer by one byte. In this layout, that byte clears a nearby lock flag.",
    command: "buffer[32] = 0x00",
    output: [
      "buffer[0..31]            32 valid bytes",
      "terminator at +32        one extra byte",
      "neighbor.locked          01 → 00",
      "later: is_locked()       false",
    ],
    label: "The extra byte clears the lock flag.",
    status: "FLAG CLEARED",
    detail:
      "The thin red section represents the one byte that changed. The other bytes in the block are unchanged.",
    metric: "OVERWRITE WIDTH",
    value: "1 byte",
    note: "The buffer ends at a 16-byte tag boundary in this example. Lockstep checks tags for each 16-byte region, rather than bounds for individual bytes. An overflow within one tagged region can go undetected.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "wrap",
    part: "exploit",
    actor: "attacker",
    kicker: "05 / SIZE ARITHMETIC",
    title: "A size calculation can wrap to 32 bytes.",
    description:
      "The calculation needs to represent 288, but an unsigned 8-bit value wraps to 32. The service allocates 32 bytes and then copies the original 288 bytes into that buffer.",
    command: "size_u8 = 18 * 16",
    output: [
      "intended size            288 bytes",
      "8-bit result             32 bytes",
      "allocate(32); copy(288)",
      "256 bytes cross the allocation boundary",
    ],
    label: "The copy exceeds the allocated size.",
    status: "ALLOCATION TOO SMALL",
    detail:
      "The four green blocks hold 32 bytes. The 32 red blocks show the remaining 256 bytes written past the allocation.",
    metric: "ALLOCATION / COPY",
    value: "32 / 288 bytes",
    note: "This example uses unsigned 8-bit arithmetic to make the overflow easy to see. The size_t type used for real allocation sizes is wider. This is a general error pattern, not a reproduction of a specific product bug.",
    source: repo + "tests/fixtures/cve_2026_53362_ipv6_frag.rs",
  },
  {
    id: "tags",
    part: "defense",
    actor: "operator",
    kicker: "06 / SHADOW TAGS",
    title: "Lockstep rejects writes with the wrong tag.",
    description:
      "Lockstep checks the tag carried by a pointer against the tag stored for each 16-byte region it accesses. A write is rejected when the tags differ.",
    command: "./target/release/isr-process ./nginx --backend x86_64",
    output: [
      "pointer tag              A",
      "buffer shadow            A A",
      "neighbor shadow          B B",
      "write at +32             TagMismatch",
      "neighbor callback        unchanged",
    ],
    label: "The pointer tag must match the memory tag.",
    status: "WRITE REJECTED",
    detail:
      "The pointer has tag A, but the neighboring allocation has tag B. Lockstep rejects the write. The service cannot modify the shadow map that stores these tags.",
    metric: "FIRST INVALID WRITE",
    value: "stopped at +32",
    note: "Each check covers every 16-byte region touched by the operation. This example tracks a 32-byte allocation. The libc allocation hooks currently add padding and round allocations to 64-byte units; overruns within that tracked padding can go undetected. Optional Intel Linear Address Masking (LAM) changes where pointer tags are stored, but the tag checks still run.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "quarantine",
    part: "defense",
    actor: "attacker",
    kicker: "07 / FREE AND QUARANTINE",
    title: "Lockstep rejects access after free.",
    description:
      "When an allocation is freed, Lockstep marks it as quarantined and rejects access through the old pointer. Quarantine delays reuse of the memory and has a fixed capacity.",
    command: "free(buffer); use(stale_pointer)",
    output: [
      "allocation               released",
      "shadow                   A → quarantined",
      "stale access             UseAfterFree",
      "later reuse              fresh allocation tag",
    ],
    label: "The freed allocation is quarantined.",
    status: "STALE ACCESS REJECTED",
    detail:
      "The amber blocks are quarantined. The old pointer still contains their address, but Lockstep rejects access to them.",
    metric: "ALLOCATION STATE",
    value: "live → quarantined",
    note: "Quarantine has a fixed capacity, so freed memory is eventually reused with a new allocation tag. The number of possible tags is limited. A reused allocation can eventually have the same tag as an old pointer.",
    source: repo + "src/process/heap.rs",
  },
  {
    id: "placement",
    part: "defense",
    actor: "attacker",
    kicker: "08 / RANDOMIZED PLACEMENT",
    title: "Lockstep randomizes allocation slots.",
    description:
      "An attacker may allocate and free objects to place a target beside a vulnerable buffer. Lockstep uses a cryptographic random generator to choose allocation slots, making that sequence less predictable.",
    command: "allocate(); free(); allocate(target)",
    output: [
      "expected neighbor        target",
      "actual slot              randomly selected",
      "repeated allocations     placement may differ",
      "target adjacency         not guaranteed",
    ],
    label: "The target may occupy a different slot.",
    status: "PLACEMENT RANDOMIZED",
    detail:
      "The target may be placed in a different slot. Randomization makes adjacency less reliable. Tag checks still apply when a write reaches the target.",
    metric: "TARGET POSITION",
    value: "randomly selected",
    note: "Random placement can still put the target beside the buffer. It does not replace memory access checks. The animation shows possible layouts, not measured probabilities.",
    source: repo + "tests/fixtures/heap_grooming.rs",
  },
  {
    id: "pac",
    part: "defense",
    actor: "attacker",
    kicker: "09 / POINTER AUTHENTICATION",
    title: "Lockstep checks stored function pointers.",
    description:
      "A stored function pointer has an authentication code computed from its value and storage address. Copying its raw bytes to another address can cause the authentication check to fail.",
    command: "overwrite(slot_B, raw_bytes(slot_A))",
    output: [
      "raw signed bytes         submitted",
      "storage address          A → B",
      "authentication           does not match",
      "store / later load       PointerAuth",
    ],
    label: "The signature depends on the storage address.",
    status: "POINTER REJECTED",
    detail:
      "The red block shows where the attacker tries to copy the bytes. Lockstep can reject the store because the pointer was signed for a different address.",
    metric: "SIGNED FIELDS",
    value: "pointer + address",
    note: "This example copies raw bytes. A permitted load and store can authenticate and re-sign a pointer for its new address. Software pointer authentication uses a truncated code on 64-bit function pointers. Lockstep also tracks whether a slot previously held a pointer and where a replacement value came from. Ordinary data values are not authenticated.",
    source: repo + "src/process/pac.rs",
  },
  {
    id: "cfi",
    part: "defense",
    actor: "attacker",
    kicker: "10 / CONTROL-FLOW CHECKS",
    title: "Lockstep checks branch destinations.",
    description:
      "An attacker changes a branch address to point inside an instruction that Lockstep has already decoded. Lockstep records the start and end of each decoded instruction and rejects this branch.",
    command: "jump(decoded_instruction + 2)",
    output: [
      "known instruction        [start ... end)",
      "requested destination    inside instruction",
      "branch check             rejected",
      "target instructions      not executed",
    ],
    label: "This jump lands inside a known instruction.",
    status: "BRANCH REJECTED",
    detail:
      "The red destination is between the recorded start and end of an instruction. Lockstep rejects it before executing the destination block.",
    metric: "DESTINATION",
    value: "middle of instruction",
    note: "The current control-flow check rejects addresses inside known instructions. It does not have a complete list of legal branches for the program. A malicious destination at a valid instruction start can still pass this check.",
    source: repo + "src/process/cfi.rs",
  },
  {
    id: "diversify",
    part: "defense",
    actor: "operator",
    kicker: "11 / JIT DIVERSIFICATION",
    title: "Lockstep varies the compiled code.",
    description:
      "For each compilation, Lockstep randomizes the mapping from guest registers to compiler variables and adds random stack-frame padding. These inputs can change the layout of the generated native code.",
    command: "compile(block) → compile(block)",
    output: [
      "guest behavior           preserved",
      "variable mapping         randomized",
      "frame padding            randomized",
      "native code layout       may differ",
    ],
    label: "Compilation uses randomized inputs.",
    status: "COMPILER INPUTS RANDOMIZED",
    detail:
      "The blocks represent separate compilations of the same code. Randomization can change the layout while preserving behavior. Memory and branch checks still apply.",
    metric: "COMPILER INPUTS",
    value: "randomized",
    note: "Cranelift may optimize away some differences in its inputs, so different permutations can still produce identical machine code. Diversification does not guarantee that all instruction sequences useful to an attacker are removed.",
    source: repo + "src/process/diversify.rs",
  },
  {
    id: "isolation",
    part: "defense",
    actor: "attacker",
    kicker: "12 / EXECUTION BOUNDARIES",
    title: "Lockstep stores metadata in host memory.",
    description:
      "The service runs as a guest. Its addresses resolve only to guest memory mappings. Lockstep stores shadow tags and pointer records in host memory. Generated code is writable during compilation and executable afterward.",
    command: "write(runtime_metadata)",
    output: [
      "guest mapping            not found",
      "host metadata            not addressable",
      "compiled code pages      writable → executable",
      "guest system calls       handled by Lockstep",
    ],
    label: "Guest mappings exclude runtime metadata.",
    status: "ACCESS REJECTED",
    detail:
      "The separate host region stores the metadata used by the checks. Lockstep validates memory mappings, restricts instructions, checks stack boundaries, and handles guest system calls.",
    metric: "CODE PAGE PERMISSIONS",
    value: "RW → RX",
    note: "Lockstep rejects guest mappings that are both writable and executable. It includes code to detect and configure Intel Memory Protection Keys (MPK), but detection alone does not establish isolation. MPK is not shown here as an enforced defense.",
    source: repo + "src/process/memory.rs",
  },
  {
    id: "receipt",
    part: "evidence",
    actor: "operator",
    kicker: "13 / VERIFY THE RESULT",
    title: "Tests check which operation was rejected.",
    description:
      "Each test runs with the relevant defense enabled and disabled. The protected run must report the expected error. The control run must complete the operation that the defense is meant to stop.",
    command: "python3 scripts/verify_heap_mitigations.py",
    output: [
      "defense enabled          expected error",
      "defense disabled         test operation completes",
      "network test             response verified",
      "event log                events.jsonl",
    ],
    label: "Each test compares both configurations.",
    status: "TEST RESULTS RECORDED",
    detail:
      "The page shows simulated examples. The test harness in the repository runs guest binaries and checks their results.",
    metric: "TEST CONFIGURATIONS",
    value: "enabled / disabled",
    note: "Tests named after reported vulnerabilities reproduce general attack operations, not complete exploits against the original products. Lockstep accepts statically linked Linux x86-64 ELF binaries. The repository includes Nginx tests for both glibc and musl builds.",
    source: repo + "scripts/verify_heap_mitigations.py",
  },
];
