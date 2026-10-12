# Designer export integration

The 2026-10-11 `Real-Time Binary ISA Platform.zip` is a standalone design
prototype, not a checkout of the production application. Its SHA-256 is
`b04cc165d83f5f3a6d30b9b4f3f929a919a154debfe39649def8ac999cf46600`.
The original, unmodified page is preserved in
[`design-source/chunder-pipeline.dc.html`](design-source/chunder-pipeline.dc.html)
as a reference only. It is not shipped in the web build.

The integration starts from production commit `5670bdb`, retaining the live SSE
consumer, journal recovery, real runtime, latest copy, manual stage navigation,
stable thermometer, upload, restart confirmation and full Evidence inspector.
No backend or engine behavior changes.

## Visual mapping

| Designer detail                                                  | Production implementation                                                                                                                |
| ---------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Forest/peach palette, radial background, 48px grid               | `webapp/src/designer.css`                                                                                                                |
| Inter and JetBrains Mono typography                              | Self-hosted Latin variable fonts, with OFL licenses in `webapp/public/fonts`                                                             |
| Four-node architecture rail                                      | `ArchitectureRail.tsx`, tied to the viewed group, observed receipts and actual engine time                                               |
| Card radii, borders, selected outline, typography, status colors | Shared styles across the designer's 9 cards                                                                                              |
| Bytes → disassembly illustration                                 | Actual `harvest` instructions and original bytes                                                                                         |
| x86 → CLIF → RV64 illustrations                                  | `TranslationVisual.tsx`, previews of saved artifacts for the exact block ID and RIP                                                      |
| Operation illustrations                                          | Five-step reader-paced sequences for file identity, memory mapping, decode, policy, IR, RV64 emission, execution, host calls and results |
| Floating transport, dial and matrix summary                      | Existing controls and measured values, restyled to match the export                                                                      |

Production now follows the designer's nine-card composition, titles and numbering.
The thirteen engine groups remain unchanged. `pipelineScenes.ts` is a presentation
projection, with each original group assigned exactly once:

| Card | Title                | Original trace groups  |
| ---- | -------------------- | ---------------------- |
| 01   | Ingest binary        | `run`, `ingest`        |
| 02   | Prepare guest        | `load`, `initialize`   |
| 03   | Disassemble block    | `decode`               |
| 04   | Verify safety policy | `audit`                |
| 05   | Lift to Cranelift IR | `lower`                |
| 06   | Compile to RV64      | `codegen`, `artifacts` |
| 07   | Execute RV64 code    | `execute`, `cache`     |
| 08   | Handle system call   | `syscall`              |
| 09   | Execution summary    | `result`               |

Card event/span counts include their constituent groups; the footer cites the
newest original receipt. Nested span durations are not summed. Partial memory
preparation cannot claim the guest context is ready. Any constituent failure
remains visible. Translation previews retain the primary codegen block identity
rather than taking an artifact receipt as a new block.

The stage map, rail, reading order and restart prompts use nine cards. Evidence
continues to expose all thirteen original groups and the unmodified journal.
Selecting a raw group in Evidence returns to its containing card. Restart uses
the card's primary group ID and retains the existing fresh-execution contract.
Cards share dimensions and retain the preceding/following card. The architecture
rail remains a read-only diagram. The updated 2026-10-12 factory renderer is now used as a subdued background on the selected card. A single WebGL canvas moves between card hosts; a new run receives a fresh renderer. The foreground diagrams explain actual recorded operations.
The embedded renderer uses a 24 fps direct-render mode at a maximum 1x pixel
ratio. It preserves the delivered geometry, materials, camera and mechanics,
while omitting the standalone bloom, multisample and 2048 px shadow passes.
The original full-quality mode remains the default for standalone embedding.
The background pauses during restart confirmation so the controls can animate
without competing with it. CI explicitly exercises software-rendered WebGL;
`ISR_SOFTWARE_WEBGL=1` selects the same path in local browser checks.
The [factory correction handoff](FACTORY_HANDOFF.md) defines their nine scene
states, Cranelift/RV64 copy and reader-controlled animation contract before integration.

The prototype's sample numbers, timestamps, illustrative IR/assembly,
scene-indexed timer, always-running animations and placeholder Evidence actions
are not imported. RV64 is displayed as saved machine bytes, not an invented
disassembly. A missing artifact has an explicit waiting/unavailable state. The
preview fetch is aborted on block/run changes and when offscreen; reads are
bounded to 8 KiB for IR and 48 bytes for machine code. Full downloads stay in
Evidence. Reading recorded output never changes engine time or replays execution.
Illustration motion starts when scrolling, clicking or keyboard focus selects a
card, and stops when that card is left. The motion gate follows reading position,
independently of receipt timing and execution completion. Unvisited cards stay
still, even when visible as neighbors. Pause remains available for completed runs; hidden
tabs, the Evidence overlay and reduced-motion preferences suppress motion.

The floating controls use a 154 × 54 px icon pill: Evidence, New run (Stop while
executing), and Pause/Resume motion. Labels remain available on hover and to
assistive technology. A revisit expands the pill horizontally, up to 450 px and
bounded by the viewport, without increasing its height. The restart action
appears after the expansion and receives keyboard focus only once available.
Cancel and Escape dismiss it; reduced motion and Pause suppress the transition.
The full stage-specific restart label and description remain accessible even
when phone layouts shorten the visible label.

## Acceptance

`npm --prefix webapp run test:experience` records desktop and mobile journeys
through all nine cards. It checks the no-input hold, exact forward ring
sequence, scrolling/click/touch/keyboard navigation, hover opacity, real clock,
gauge/card separation, self-hosted fonts and exact artifact preview contents
against independently downloaded, SHA-256-checked receipts. It also checks actual
animation starts on all nine cards after execution has finished, inactive
neighbors, focus without activation, moving factory frames, pause, reduced motion
and restored-run playback. The full browser suite also exercises motion during
live execution, failures, reconnects, uploads, restart and the real Rust TCP server.

Compare reference and integrated screenshots, inspect all cards and recorded
transitions, then repeat browser acceptance against the exact deployed commit.
The local review evidence lives under `.isr-state/designer-import`; deployment
proofs live under `.isr-state/deploy`.

## Recorded operation walkthroughs

`CardIllustration.tsx` owns the foreground diagrams. `walkthrough.ts` supplies a
5.2-second presentation clock that starts only after the selected card has
recorded data, pauses with visibility/Evidence/motion controls, and resets when a
new card or run is selected. It never changes engine counters, status or time.
Reduced motion presents the complete still diagram. Raw receipts remain in
Evidence; captions identify recorded walkthroughs.

Memory diagrams distinguish file bytes and zero-fill, show actual permissions,
and separate the guest stack from the RV64 interpreter stack. Byte and
instruction selection is linked by recorded IP. Policy gates retain guarded,
passed and rejected distinctions. IR nodes are selected from the actual bounded
CLIF preview; their relationship to source is explicitly block-level. RV64
rows reveal the saved bytes. Execution tiles show the busiest recorded
translations, with logarithmic intensity and exact counts on inspection.
Instruction and execution previews fit inside the card so wheel and touch
gestures continue through the timeline. Large decoded blocks show their first
four and final instructions; execution shows the busiest nine translations
(eight on phones). The complete data remains in Evidence.

Completed runs recover TCP exchanges and hot blocks from a streaming journal
read capped at 16 MiB, retaining at most 24 exchanges, 24 translations, 80
progress samples and eight invalidations. Larger journals are explicitly
identified as samples. The original journal is unchanged. Snapshot evidence
remains usable if the download fails. Selecting a summary track opens Evidence.

The gauge has a peach engine hand (one revolution per recorded second) and a
separate mint walkthrough ring. The numeric time is always measured engine time;
a completed run keeps it fixed while the labeled walkthrough ring progresses.
Pause/reduced motion suppress visual interpolation without stopping telemetry.

A glowing double-down chevron appears below the selected card only after its
walkthrough finishes. It fades when the card moves out of the reading
position, and clicking or pressing Enter advances to the next card without
starting another execution. The next card earns its own cue after its sequence;
the summary has none. Pause freezes the small float, and reduced motion shows a
still cue. Its position stays separate from the compact, expanding toolbar.

`illustrations.spec.ts` captures before/operation/result frames for all nine
cards, checks visible changes and pause/resume, compares memory, instruction,
IR, RV64, syscall and execution data against independent receipts, verifies
engine-hand and walkthrough-ring behavior, and checks the shared factory
canvas on desktop and phone layouts. Different real ELF inputs must produce
different byte-group diagrams and actual memory sizes. These checks supplement
the existing real-engine, navigation, restart, recovery and upload acceptance.
