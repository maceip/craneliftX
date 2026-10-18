# Chunder worked example

The same five pipeline stages now form an explorable walkthrough on the ready,
running, and saved-report views. Each stage explains its input and output in
plain language and connects that explanation to actual run artifacts.

Baby uses a real 64-byte record containing seven little-endian unsigned 64-bit
inputs, `1, 2, 3, 4, 5, 6, 7`, and an output word initialized to zero. With seed
42, the fixed kernel computes:

```text
sum inputs     28
XOR seed 42    54
shift left 1  108
add 17        125
OR with 5     125
```

The worker emits its captured before/after bytes. The browser decodes those bytes
with BigInt and derives the explanatory arithmetic from the captured inputs.
It displays the observed output separately and reports successful verification
only after the worker's completed result. The changed RAX, RCX, RIP, and output
word are shown alongside their meanings. The exit RIP remains at the RET boundary.

Before a run, the values are explicitly a preset preview. Medium and Large show
their own kernel 0 artifacts and aggregate verification; they do not present
Baby's values as observed network records. Older saved Baby reports use their
original captured inputs rather than the new preset. The separate SYSCALL
rejection test is clearly distinguished from the accepted example trace.

The stage selectors and previous/next controls are native keyboard-operable
buttons. The walkthrough has no timing simulation or artificial execution delay.
It uses the deployed Chunder design, existing React dependencies, and the same
engine event protocol. Three slice loops were updated to the standard-library
array-chunk API required by newer stable Clippy; their record boundaries and
workload remain the same.

## Source preservation

When this work began, the primary checkout contained an independent, unfinished
front-end redesign. Those files were preserved. The implementation was made in
`.isr-state/workspaces/chunder-explainer/`, based on verified public release
`20261011T040216Z-880b009502e2`.

The complete source delta is retained in
[`deploy/patches/worked-example.patch`](../deploy/patches/worked-example.patch).
Its base is that verified release, rather than the parallel redesign. The patch
contains the component, styles, integration, worker preset, compatibility fixture,
and browser tests. A dry-run application against the base snapshot is checked.
The release archive and its 53-file source manifest are retained in
`.isr-state/deploy/` and on the deployment host.

## Verification

`webapp/tests/walkthrough.spec.ts` runs the real Baby worker and checks all five
stages, artifact contents, register and memory changes, keyboard navigation,
mobile overflow, and persisted reload. Its separate compatibility test replays
an actual pre-change public Baby report as a labeled test fixture. The demo suite
also checks that network runs use aggregate evidence and cancelled runs never
claim a completed comparison.

Public deployment and final verification are recorded separately in the release
receipt, after the candidate passes its checks.
