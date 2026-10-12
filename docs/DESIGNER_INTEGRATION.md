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
| Card radii, borders, selected outline, typography, status colors | Shared styles across all 13 cards |
| Bytes → disassembly illustration | Actual `harvest` instructions and original bytes |
| x86 → CLIF → RV64 illustrations | `TranslationVisual.tsx`, previews of saved artifacts for the exact block ID and RIP |
| Dashed translation paths, scanning, execution bars | Existing visible/live span and fresh-receipt motion gates, with pause and reduced-motion support |
| Floating transport, dial and matrix summary | Existing controls and measured values, restyled to match the export |

The prototype combines thirteen engine groups into nine scenes. Production
keeps all thirteen, including run registration, separate mapping and context
initialization, artifacts and code-cache activity. Cards share dimensions and
retain the visible preceding/following card. The architecture rail is a read-only
diagram, not a second navigation bar.

The prototype's sample numbers, timestamps, illustrative IR/assembly,
scene-indexed timer, always-running animations and placeholder Evidence actions
are not imported. RV64 is displayed as saved machine bytes, not an invented
disassembly. A missing artifact has an explicit waiting/unavailable state. The
preview fetch is aborted on block/run changes and when offscreen; reads are
bounded to 8 KiB for IR and 48 bytes for machine code. Full downloads stay in
Evidence. Reading recorded output never changes engine time or replays execution.

## Acceptance

`npm --prefix webapp run test:experience` records desktop and mobile journeys
through all thirteen cards. It checks the no-input hold, exact forward ring
sequence, scrolling/click/touch/keyboard navigation, hover opacity, real clock,
gauge/card separation, self-hosted fonts and exact artifact preview contents
against independently downloaded, SHA-256-checked receipts. The full browser
suite also exercises live motion, reduced motion, failures, reconnects, uploads,
restart and the real Rust TCP server.

Compare reference and integrated screenshots, inspect all cards and recorded
transitions, then repeat browser acceptance against the exact deployed commit.
The local review evidence lives under `.isr-state/designer-import`; deployment
proofs live under `.isr-state/deploy`.
