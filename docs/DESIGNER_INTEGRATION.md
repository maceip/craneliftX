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

| Designer detail | Production implementation |
| --- | --- |
| Forest/peach palette, radial background, 48px grid | `webapp/src/designer.css` |
| Inter and JetBrains Mono typography | Self-hosted Latin variable fonts, with OFL licenses in `webapp/public/fonts` |
| Four-node architecture rail | `ArchitectureRail.tsx`, tied to the viewed group, observed receipts and actual engine time |
| Card radii, borders, selected outline, typography, status colors | Shared styles across the designer's 9 cards |
| Bytes → disassembly illustration | Actual `harvest` instructions and original bytes |
| x86 → CLIF → RV64 illustrations | `TranslationVisual.tsx`, previews of saved artifacts for the exact block ID and RIP |
| Dashed translation paths, scanning, execution bars | Animate only on the visible, selected card, including recorded results, with pause and reduced-motion support |
| Floating transport, dial and matrix summary | Existing controls and measured values, restyled to match the export |

Production now follows the designer's nine-card composition, titles and numbering.
The thirteen engine groups remain unchanged. `pipelineScenes.ts` is a presentation
projection, with each original group assigned exactly once:

| Card | Title | Original trace groups |
| --- | --- | --- |
| 01 | Ingest binary | `run`, `ingest` |
| 02 | Prepare guest | `load`, `initialize` |
| 03 | Disassemble block | `decode` |
| 04 | Verify safety policy | `audit` |
| 05 | Lift to Cranelift IR | `lower` |
| 06 | Compile to RV64 | `codegen`, `artifacts` |
| 07 | Execute RV64 code | `execute`, `cache` |
| 08 | Handle system call | `syscall` |
| 09 | Execution summary | `result` |

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
rail remains a read-only diagram. The separate artist factory assets are not used.
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

## Acceptance

`npm --prefix webapp run test:experience` records desktop and mobile journeys
through all nine cards. It checks the no-input hold, exact forward ring
sequence, scrolling/click/touch/keyboard navigation, hover opacity, real clock,
gauge/card separation, self-hosted fonts and exact artifact preview contents
against independently downloaded, SHA-256-checked receipts. It also checks actual
animation starts on all nine cards after execution has finished, inactive
neighbors, focus without activation, moving scan frames, pause, reduced motion
and restored-run playback. The full browser suite also exercises motion during
live execution, failures, reconnects, uploads, restart and the real Rust TCP server.

Compare reference and integrated screenshots, inspect all cards and recorded
transitions, then repeat browser acceptance against the exact deployed commit.
The local review evidence lives under `.isr-state/designer-import`; deployment
proofs live under `.isr-state/deploy`.
