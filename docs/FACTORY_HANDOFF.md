# Binary factory correction handoff

We want to use the existing 45-degree isometric factory artwork. Please retain
its machinery, palette and mechanical motion, and update the story to match the
runtime below. The current delivery still says “LLVM compactor” and “Native
execution”; neither describes this demo.

The application has **nine cards**. Provide nine addressable presentation states
matching them; reuse the five existing machines where useful. Nine new models
are not required.

| Card / scene ID | Heading | Explanation | Suggested mechanical action |
| --- | --- | --- | --- |
| 01 / `ingest` | Ingest binary | Read and fingerprint a static Linux x86-64 executable. | Receive the binary cube at the intake and scan its identity. |
| 02 / `initialize` | Prepare guest | Map the executable and prepare guest memory, registers and stack. | Place the cube in a prepared fixture; illuminate the memory regions. |
| 03 / `decode` | Disassemble block | Decode the next reached block of x86-64 instructions with iced-x86. | Open or separate the block into readable instruction pieces. |
| 04 / `audit` | Verify safety policy | Check the reached instructions against the supported instruction policy. Runtime guards also apply during execution. | Scan the pieces through the policy gate. Show the actual accepted or rejected state. |
| 05 / `lower` | Lift to Cranelift IR | Express the supported x86-64 instruction semantics as Cranelift IR, called CLIF. | Use the gantry or compactor to reassemble the pieces into an IR block. |
| 06 / `codegen` | Compile to RV64 | Cranelift compiles the IR into RISC-V 64-bit machine code. | Use the press to stamp the translated RV64 block. |
| 07 / `execute` | Execute RV64 code | Run the generated instructions in the embedded RV64 interpreter, retaining guest state and reusing cached translations. | Animate an execution cycle with the translated block in the execution fixture. |
| 08 / `syscall` | Handle system call | Route supported guest system calls through the explicit host bridge, including TCP operations. | Send a request through a bridge or port and return its response. |
| 09 / `result` | Execution summary | Show measured execution results and independently checked TCP responses. | Deliver the block to the result bay and show the recorded outcome. |

## Corrections that apply throughout the delivery

- Replace the “Static binary translation” introduction with “Live binary ISA
  translation.” The runtime translates blocks as the program reaches them.
- Remove LLVM branding and the standalone LLVM optimization stage. Cranelift is
  the compiler used in this pipeline. The compactor can illustrate CLIF lowering;
  its physical animation does not need to be discarded.
- Replace “Native execution” with “Execution summary.” The emitted RV64 machine
  code runs in our interpreter, not directly on a physical RISC-V processor.
- Do not claim whole-program equivalence, comparison against a native original,
  or a universally safe binary. The demo checks specified TCP responses and
  records the actual instruction-policy and runtime outcomes.
- These are nine explanatory views of a repeating runtime. Blocks can cycle
  through decode, audit, lift, compile and execute; cached blocks are reused, and
  system calls happen during execution. Avoid implying one whole-file pass
  through a strictly linear assembly line.

## Animation and integration contract

- Let the host app select the scene and control whether it is active, paused or
  using reduced motion. The scene IDs above are the integration keys.
- Start or resume the scene when scrolling, clicking or keyboard focus selects
  its card. Keep processing loops available while that card is being read,
  including after the real engine has finished.
- Stop inactive and offscreen scenes. Neighboring cards must not consume their
  animations in advance. Also stop when the page is hidden or Evidence is open.
- Preserve the app's navigation. Do not auto-advance on a timer or require a
  second set of factory Continue buttons inside a card.
- Compilation and verification colors/status come from recorded runtime events.
  Elapsed animation time must never mark a block compiled, accepted or verified.
- Retain a standalone preview and editable copy, but deliver the renderer so it
  can be controlled by the application rather than owning the whole page.

Acceptance: allow a real run to finish while holding card 1; then visit all nine
cards by scroll, click/touch and keyboard focus. Each newly reached scene must
animate, inactive neighbors must remain still, and pause/reduced motion must
work on both live and saved results. Inspect desktop and mobile transitions.
