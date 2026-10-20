export type Step = { title: string; text: string; action: string };
export type Lesson = {
  id: string;
  title: string;
  short: string;
  subtitle: string;
  object: string;
  target: string;
  targetLabel: string;
  capacity: number;
  writes: number;
  impact: string;
  condition: string;
  repair: string;
  limit: string;
  source: string;
  sourceLabel: string;
  steps: Step[];
};

// Schematic layouts, not a model of a particular malloc implementation.
// A parcel represents one byte except in the explicitly scaled size-arithmetic scene.
export const lessons: Lesson[] = [
  {
    id: "neighbor",
    title: "The spill next door",
    short: "Adjacent data",
    subtitle: "A long delivery changes someone else’s order.",
    object: "Sorting trays",
    target: "A neighboring permission flag",
    targetLabel: "ACCESS FLAG",
    capacity: 8,
    writes: 12,
    impact: "A data-only attack",
    condition:
      "The overflowing buffer must reach a useful field. Its bytes must form a value the program later trusts. No redirected execution is required.",
    repair:
      "Check the destination capacity before each copy, including space for any terminator.",
    limit:
      "Separate allocations are shown adjacent for clarity. Real spacing, padding, allocation order, and attacker influence determine whether the flag is reachable.",
    source: "https://cwe.mitre.org/data/definitions/122.html",
    sourceLabel: "CWE-122 · Heap-based buffer overflow",
    steps: [
      {
        title: "Every delivery has a tray.",
        text: "The orange tray is a heap buffer with room for eight bytes. The teal tray belongs to another object. A boundary separates their ownership, even when their addresses are close.",
        action: "allocate buffer: 8 bytes",
      },
      {
        title: "The first eight bytes fit.",
        text: "An unchecked copy starts filling the buffer. So far, every byte lands inside the allocation. The delivery still has four bytes to go.",
        action: "write bytes 0…7 → inside buffer",
      },
      {
        title: "The copy keeps going.",
        text: "Four extra bytes cross the allocation boundary and overwrite neighboring data. Memory does not have physical tray walls; the program needs an actual check.",
        action: "write bytes 8…11 → neighbor",
      },
      {
        title: "The program trusts a changed flag.",
        text: "In this toy layout, the extra bytes change an access flag from DENY to ALLOW. A later permission check reads the corrupted value and makes the wrong decision.",
        action: "read corrupted flag → access allowed",
      },
    ],
  },
  {
    id: "callback",
    title: "The switched route",
    short: "Callback pointer",
    subtitle: "An overflow rewrites where the next job goes.",
    object: "A railway switch",
    target: "A neighboring callback address",
    targetLabel: "CALLBACK",
    capacity: 8,
    writes: 16,
    impact: "Control-flow redirection",
    condition:
      "The write must reach a callable pointer and supply a usable destination. The program must later call it, and applicable pointer or control-flow checks must permit it.",
    repair:
      "Prevent the overwrite. Pointer integrity and control-flow checks can also constrain the later indirect call.",
    limit:
      "The switch represents an address, not executable bytes. Redirecting a pointer does not automatically bypass ASLR, non-executable memory, PAC, or CFI.",
    source: "https://cwe.mitre.org/data/definitions/122.html",
    sourceLabel: "CWE-122 · Function-pointer consequences",
    steps: [
      {
        title: "An address is a routing instruction.",
        text: "The tray holds input. Beside it, a callback pointer tells the program which function to call next. The railway switch is that pointer, aimed at the intended handler.",
        action: "callback → intended handler",
      },
      {
        title: "The input fills its own space.",
        text: "Eight input bytes fill the buffer. The callback is still intact. The vulnerable copy has no capacity check and more bytes remain.",
        action: "write bytes 0…7 → inside buffer",
      },
      {
        title: "Extra bytes rewrite the address.",
        text: "The overflowing copy reaches the callback. In this schematic 64-bit layout, the next eight bytes replace the address and swing the route toward a different destination.",
        action: "write bytes 8…15 → callback address",
      },
      {
        title: "A later call takes the wrong route.",
        text: "When the program invokes the corrupted callback, execution can reach an attacker-selected callable target. The overwrite and the later use are two separate events.",
        action: "indirect call → substituted handler",
      },
    ],
  },
  {
    id: "metadata",
    title: "The forged inventory",
    short: "Allocator metadata",
    subtitle: "The warehouse’s own records become the target.",
    object: "An inventory clipboard",
    target: "Allocator bookkeeping",
    targetLabel: "CHUNK RECORD",
    capacity: 8,
    writes: 12,
    impact: "Allocator state corruption",
    condition:
      "The allocator must place useful bookkeeping within reach. Corrupted values must survive consistency checks before a later allocation or free uses them.",
    repair:
      "Enforce write bounds; allocator consistency checks and protected or separated metadata provide additional defenses.",
    limit:
      "The clipboard abstracts chunk sizes and free-list links. This is a vulnerable-allocator model: modern allocator checks often abort instead of accepting forged records.",
    source:
      "https://sourceware.org/glibc/manual/latest/html_node/Heap-Consistency-Checking.html",
    sourceLabel: "GNU C Library · Heap consistency checking",
    steps: [
      {
        title: "The warehouse keeps records.",
        text: "An allocator tracks which chunks exist, their sizes, and which space is reusable. The clipboard stands for those records; the connected trays stand for memory chunks.",
        action: "record → real chunk boundary",
      },
      {
        title: "The buffer receives its delivery.",
        text: "Eight bytes fit. The inventory record belongs to the allocator, not to the application’s input buffer.",
        action: "write bytes 0…7 → inside buffer",
      },
      {
        title: "The delivery reaches the clipboard.",
        text: "An overflow changes a chunk size or link. The record now describes a false boundary. The visible connector is a stored relationship, like an address in a free list.",
        action: "overflow → corrupted chunk record",
      },
      {
        title: "A later operation trusts the forgery.",
        text: "In a vulnerable allocator, using the forged record can make allocations overlap or send bookkeeping writes to the wrong place. A hardened allocator may detect the inconsistency and stop.",
        action: "allocate / free → invalid bookkeeping",
      },
    ],
  },
  {
    id: "off-by-one",
    title: "One byte too many",
    short: "Off-by-one / NUL",
    subtitle: "Even the end-of-string marker needs a space.",
    object: "A single extra parcel",
    target: "The first byte after a buffer",
    targetLabel: "LOCK FLAG",
    capacity: 8,
    writes: 9,
    impact: "A small write, a meaningful change",
    condition:
      "The byte immediately after the buffer must matter. A NUL overwrite can only write zero; it cannot choose an arbitrary value or a whole address.",
    repair:
      "Reserve space for the string terminator and use the correct exclusive loop bound.",
    limit:
      "Here the adjacent one-byte field means 1 = locked, 0 = unlocked. In real layouts the extra byte may hit padding, data, or allocator metadata, with very different outcomes.",
    source: "https://cwe.mitre.org/data/definitions/193.html",
    sourceLabel: "CWE-193 · Off-by-one error",
    steps: [
      {
        title: "Eight slots, eight characters.",
        text: "A heap buffer reserves eight bytes for a string. A separate lock flag happens to follow it in this toy layout. The flag starts at 1: locked.",
        action: "capacity = 8; lock = 1",
      },
      {
        title: "The characters fit exactly.",
        text: "All eight character bytes fit. But a C string also needs a terminating zero byte. There is no ninth slot in this allocation.",
        action: "8 characters → 8 occupied bytes",
      },
      {
        title: "The terminator lands next door.",
        text: "The code appends a NUL at index 8. Valid indexes were 0 through 7. That single zero overwrites the adjacent lock flag.",
        action: "buffer[8] = 0 → out of bounds",
      },
      {
        title: "Zero changes the decision.",
        text: "A later check reads the flag as unlocked. The overflow was only one byte; its impact depends on what that byte represents.",
        action: "lock: 1 → 0 → unlocked",
      },
    ],
  },
  {
    id: "size-wrap",
    title: "The undersized order",
    short: "Size arithmetic",
    subtitle: "The order is large. The calculated box is tiny.",
    object: "A wrapping order counter",
    target: "Memory beyond an undersized allocation",
    targetLabel: "NEIGHBOR",
    capacity: 2,
    writes: 10,
    impact: "Arithmetic becomes a memory bug",
    condition:
      "A size calculation must wrap or truncate, and a later write must still use the larger intended size. Checked multiplication can reject the request before allocation.",
    repair:
      "Check size arithmetic before allocating and ensure the allocation and copy use the same validated length.",
    limit:
      "The counter deliberately uses 8-bit unsigned arithmetic: 18 × 16 = 288, which wraps to 32. Real size_t widths are larger. Each parcel here represents a 16-byte group.",
    source: "https://cwe.mitre.org/data/definitions/680.html",
    sourceLabel: "CWE-680 · Integer overflow to buffer overflow",
    steps: [
      {
        title: "An order needs eighteen parcels.",
        text: "The order requests 18 groups of 16 bytes: 288 bytes total. To make wraparound visible, this miniature example uses an unsigned 8-bit size counter.",
        action: "18 × 16 = 288 bytes needed",
      },
      {
        title: "The counter wraps to thirty-two.",
        text: "An 8-bit counter holds 0 through 255. Storing 288 wraps to 32. The allocator receives 32 and correctly provides a small buffer: only two parcel spaces.",
        action: "288 mod 256 = 32 bytes allocated",
      },
      {
        title: "The delivery uses the original order.",
        text: "The copy still attempts to write 288 bytes. After the first 32, it crosses the small allocation’s boundary. Only the beginning of the oversized delivery is drawn.",
        action: "write 288 bytes into 32-byte allocation",
      },
      {
        title: "The mismatch corrupts the neighbor.",
        text: "The resulting overwrite can damage adjacent data or pointers. The integer wrap is the cause; the later write past the heap buffer is the memory-safety failure.",
        action: "size mismatch → heap buffer overflow",
      },
    ],
  },
];
