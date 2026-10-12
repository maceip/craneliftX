import React, { memo, useState } from "react";
import {
  bytes,
  duration,
  Group,
  LedgerEvent,
  number,
  Run,
  terminal,
} from "../pipeline";
import { Scene } from "../pipelineScenes";
import { policyLabels } from "../pipelineCopy";
import { IllustrationEvidence, Walkthrough } from "../walkthrough";
import { TranslationVisual } from "./TranslationVisual";
import { PerformanceSummary } from "./Telemetry";

export function currentOutput(group?: Group): LedgerEvent | null {
  if (!group) return null;
  if (
    group.last_event?.block_id !== undefined &&
    group.output?.block_id !== undefined &&
    group.last_event.block_id !== group.output.block_id
  )
    return null;
  return group.output;
}
const scripts: Record<string, string[]> = {
  ingest: [
    "Receive the executable",
    "Read the file bytes",
    "Identify the input",
    "Attach its fingerprint",
    "Input ready to map",
  ],
  initialize: [
    "Reserve the address space",
    "Copy file-backed regions",
    "Create zero-filled memory",
    "Apply permissions and prepare stacks",
    "Connect the entry point and guest state",
  ],
  decode: [
    "Read bytes at the reached address",
    "Separate instruction boundaries",
    "Decode each instruction",
    "Resolve the block boundary",
    "Bytes and instructions are linked",
  ],
  audit: [
    "Inspect this reached block",
    "Check instruction and syscall policy",
    "Check stack and memory rules",
    "Check executable targets",
    "Record findings and retain runtime guards",
  ],
  lower: [
    "Read the source block",
    "Load guest state into IR values",
    "Build the operation graph",
    "Write the resulting guest state",
    "Cranelift IR artifact recorded",
  ],
  codegen: [
    "Read the Cranelift IR block",
    "Compile for the RV64 target",
    "Assemble machine-code bytes",
    "Attach the output fingerprint",
    "Compiled artifact ready to execute",
  ],
  execute: [
    "Locate compiled translations",
    "Enter the RV64 interpreter",
    "Accumulate work in reached blocks",
    "Reuse translations; preserve guest state",
    "Inspect recorded execution totals",
  ],
  syscall: [
    "Select a recorded guest request",
    "Enter the mediated host bridge",
    "Perform the host operation",
    "Return the recorded result",
    "Resume the guest with its state intact",
  ],
  result: [
    "Collect the execution record",
    "Reconcile compilation totals",
    "Collect interpreted work",
    "Attach independent TCP checks",
    "Measured results ready to inspect",
  ],
};
function Reveal({
  ready,
  children,
  className = "",
}: {
  ready: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={`story-reveal ${className}`} data-ready={ready}>
      {children}
    </div>
  );
}
function Empty({ text }: { text: string }) {
  return <div className="story-empty">{text}</div>;
}
export function hasStoryData(scene: Scene, run: Run | null) {
  if (!run) return false;
  const output = currentOutput(scene.primary);
  if (scene.id === "initialize")
    return (
      !!currentOutput(run.groups.load)?.details &&
      currentOutput(run.groups.initialize)?.status === "completed"
    );
  if (scene.id === "result") return !!run.engine_result;
  if (["lower", "codegen"].includes(scene.id))
    return output?.status === "completed";
  if (scene.id === "execute") return (run.counters.rv64_steps ?? 0) > 0;
  return !!output;
}
function InputScene({ run, phase }: { run: Run; phase: number }) {
  const [inspected, setInspected] = useState(false);
  const count = run.input?.bytes ?? run.groups.ingest.output?.bytes;
  const chunks = Math.min(24, Math.ceil((count ?? 0) / 256));
  const mapped = run.groups.load.output?.details?.format === "ELF";
  return (
    <div className="intake-story" data-inspected={inspected}>
      <button
        type="button"
        className="executable-object"
        aria-label="Inspect executable identity"
        aria-pressed={inspected}
        onClick={(e) => {
          e.stopPropagation();
          setInspected((value) => !value);
        }}
      >
        <span>{mapped ? "ELF" : "FILE"}</span>
        <strong>{mapped ? "x86-64" : "Input"}</strong>
        <small>{bytes(count)}</small>
      </button>
      <div className="intake-body">
        <strong>{run.input_name}</strong>
        <Reveal ready={phase >= 1} className="byte-volume">
          <span style={{ transform: `scaleX(${phase >= 1 ? 1 : 0})` }} />
          <small>{number(count)} file bytes read</small>
        </Reveal>
        <div
          className="file-chunks"
          data-ready={phase >= 1}
          aria-label={`${chunks} groups representing ${count ?? 0} file bytes`}
        >
          {Array.from({ length: chunks }, (_, i) => (
            <i key={i} style={{ transitionDelay: `${i * 25}ms` }} />
          ))}
        </div>
        <Reveal ready={phase >= 2}>
          <div className="identity-tags">
            <span>{mapped ? "Static executable" : "File identity"}</span>
            <span>Recorded input</span>
          </div>
        </Reveal>
        <Reveal ready={phase >= 3}>
          <small className="story-label">SHA-256 · input identity</small>
          <code className="hash-strip" data-testid="ingest-hash">
            {run.input?.sha256 ?? "Awaiting fingerprint"}
          </code>
        </Reveal>
      </div>
    </div>
  );
}
function MemoryScene({ run, phase }: { run: Run; phase: number }) {
  const mapped = currentOutput(run.groups.load)?.details,
    state = currentOutput(run.groups.initialize);
  const [selected, setSelected] = useState<string | null>(null);
  if (!Array.isArray(mapped?.segments))
    return <Empty text="Waiting for recorded memory mappings" />;
  const segments = mapped.segments as any[];
  const region = segments.find((s) => s.address === selected) ?? segments[0];
  return (
    <div className="memory-story" data-inspected-region={region.address}>
      <div className="address-space">
        <small className="story-label">
          Guest address space · regions shown schematically
        </small>
        <div className="memory-regions">
          {segments.slice(0, 12).map((s) => (
            <button
              type="button"
              key={s.address}
              className="segment-row memory-region"
              data-address={s.address}
              data-bytes={s.memory_bytes}
              data-populated={phase >= (s.file_bytes ? 1 : 2)}
              data-selected={region.address === s.address}
              aria-label={`Inspect memory region ${s.address}`}
              aria-pressed={region.address === s.address}
              onClick={(event) => {
                event.stopPropagation();
                setSelected(s.address);
              }}
            >
              <code>{s.address}</code>
              <div className="region-chamber">
                <span
                  className="region-file"
                  style={{
                    width: `${s.memory_bytes ? (s.file_bytes / s.memory_bytes) * 100 : 0}%`,
                  }}
                />
                <span
                  className="region-zero"
                  style={{
                    width: `${s.memory_bytes ? ((s.memory_bytes - s.file_bytes) / s.memory_bytes) * 100 : 0}%`,
                  }}
                />
                <span className="region-caption">
                  {s.file_bytes
                    ? `${bytes(s.file_bytes)} from file`
                    : `${bytes(s.memory_bytes)} zero-filled`}
                </span>
              </div>
              <span className="region-permission" data-ready={phase >= 3}>
                R{s.writable ? "W" : "–"}
                {s.executable ? "X" : "–"}
              </span>
              <span
                className="entry-pin"
                data-ready={phase >= 4 && s.executable}
              >
                ← RIP
              </span>
            </button>
          ))}
        </div>
        <p className="region-detail">
          <code>{region.address}</code> · {bytes(region.file_bytes)} file +{" "}
          {bytes(region.memory_bytes - region.file_bytes)} zero-filled ·{" "}
          {bytes(region.memory_bytes)} mapped
        </p>
      </div>
      <div className="guest-state">
        <Reveal ready={phase >= 3} className="stack-pair">
          <div className="stack-object">
            <i />
            <i />
            <i />
            <strong>Guest stack</strong>
            <small>{bytes(mapped.stack_end - mapped.stack_start)}</small>
          </div>
          <div className="stack-object">
            <i />
            <i />
            <i />
            <strong>RV64 stack</strong>
            <small>{bytes(state?.rv64_stack_bytes)}</small>
          </div>
        </Reveal>
        <Reveal ready={phase >= 4} className="register-bank">
          <div className="state-slots">
            {Array.from(
              { length: Math.min(32, state?.state_words ?? 0) },
              (_, i) => (
                <i key={i} />
              ),
            )}
          </div>
          <small>{number(state?.state_words)} guest state slots</small>
          <code>RIP {state?.entry_rip ?? "—"}</code>
          <code>RSP {state?.guest_rsp ?? "—"}</code>
        </Reveal>
      </div>
    </div>
  );
}
function DecodeScene({
  output,
  phase,
}: {
  output: LedgerEvent | null;
  phase: number;
}) {
  const [picked, setPicked] = useState<number | null>(null);
  const instructions = output?.instructions;
  if (!Array.isArray(instructions))
    return <Empty text="Waiting for a reached block to be decoded" />;
  const index = Math.min(
    picked ?? Math.max(0, phase - 1),
    instructions.length - 1,
  );
  return (
    <div
      className="decode-story"
      data-source-seq={output!.seq}
      data-selected-ip={instructions[index]?.ip}
    >
      <div className="raw-byte-source">
        <small className="story-label">
          Original bytes · block {output!.block_id}
        </small>
        <div className="raw-byte-chips">
          {instructions.map((i, n) => (
            <button
              type="button"
              key={i.ip}
              data-linked={n === index}
              aria-label={`Inspect instruction ${i.ip}`}
              onClick={(e) => {
                e.stopPropagation();
                setPicked(n);
              }}
            >
              <code>{i.bytes}</code>
              <small>{i.bytes.replaceAll(" ", "").length / 2} B</small>
            </button>
          ))}
        </div>
      </div>
      <div className="decode-boundary" data-ready={phase >= 1}>
        <span>iced-x86</span>
        <i>→</i>
      </div>
      <div
        className="opcode-scroller"
        role="region"
        tabIndex={0}
        aria-label="Decoded instruction bytes"
      >
        <ol className="opcode-track">
          {instructions.map((i, n) => (
            <li
              key={i.ip}
              data-ip={i.ip}
              data-bytes={i.bytes}
              data-linked={n === index}
              data-ready={phase >= 2}
            >
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  setPicked(n);
                }}
                aria-label={`Highlight bytes for ${i.asm}`}
              >
                <code>{i.ip}</code>
                <span>{i.asm}</span>
              </button>
            </li>
          ))}
        </ol>
        <small>
          {instructions.length} instructions ·{" "}
          {phase >= 3 ? "Block boundary recorded" : "Reached block"}
        </small>
      </div>
    </div>
  );
}
function PolicyScene({
  output,
  phase,
}: {
  output: LedgerEvent | null;
  phase: number;
}) {
  const [inspected, setInspected] = useState<string | null>(null);
  if (!Array.isArray(output?.checks) || !output.checks.length)
    return <Empty text="No instruction-policy findings recorded" />;
  const checks = output.checks as any[],
    check =
      checks.find((c) => c.rule === inspected) ??
      checks[Math.min(phase, checks.length - 1)];
  return (
    <div className="policy-story" data-source-seq={output!.seq}>
      <div className="policy-input">
        <strong>{number(output!.instruction_count)}</strong>
        <span>reached instructions</span>
        <code>Block {output!.block_id}</code>
      </div>
      <div className="policy-gates">
        {checks.map((c, i) => (
          <button
            type="button"
            key={c.rule}
            className={`policy-gate policy-${c.verdict}`}
            data-ready={phase > Math.min(3, i)}
            data-selected={check.rule === c.rule}
            onClick={(e) => {
              e.stopPropagation();
              setInspected(c.rule);
            }}
          >
            <span className="gate-symbol">
              {c.verdict === "rejected"
                ? "×"
                : c.verdict === "guarded"
                  ? "◈"
                  : "✓"}
            </span>
            <strong>{policyLabels[c.rule] ?? c.label}</strong>
            <small>
              {c.verdict === "guarded"
                ? "Runtime guard armed"
                : c.verdict === "rejected"
                  ? "Rejected"
                  : "Passed"}
            </small>
          </button>
        ))}
      </div>
      <p className="policy-detail">
        {check.findings?.[0]?.instruction && (
          <code>{check.findings[0].instruction} · </code>
        )}
        {check.findings?.[0]?.detail ?? check.detail}
      </p>
    </div>
  );
}
function ExecuteScene({
  run,
  evidence,
  phase,
}: {
  run: Run;
  evidence: IllustrationEvidence;
  phase: number;
}) {
  const [picked, setPicked] = useState<number | null>(null);
  const blocks = evidence.blocks,
    peak = Math.max(1, ...blocks.map((b) => b.rv64_steps));
  const point =
    evidence.progress[
      Math.min(
        evidence.progress.length - 1,
        phase * Math.floor(evidence.progress.length / 5),
      )
    ];
  const inspected = blocks.find((b) => b.block_id === picked) ?? blocks[0];
  return (
    <div className="execute-story">
      <div className="block-field" data-ready={phase >= 1}>
        {blocks.length ? (
          blocks.slice(0, 18).map((b) => (
            <button
              type="button"
              key={b.block_id}
              data-block={b.block_id}
              data-work={b.rv64_steps}
              data-selected={inspected?.block_id === b.block_id}
              onClick={(e) => {
                e.stopPropagation();
                setPicked(b.block_id);
              }}
              style={{
                backgroundColor: `rgba(158, 186, 124, ${0.08 + (0.36 * Math.log1p(b.rv64_steps)) / Math.log1p(peak)})`,
              }}
            >
              <code>{b.rip}</code>
              <span>{number(b.executions)} runs</span>
              <i
                style={{
                  transform: `scaleX(${phase >= 2 ? Math.log1p(b.rv64_steps) / Math.log1p(peak) : 0})`,
                }}
              />
            </button>
          ))
        ) : (
          <div className="interpreter-fixture">
            <span>RV64</span>
            <strong>
              {point?.rip ??
                run.groups.execute.last_event?.rip ??
                "Interpreter"}
            </strong>
            <small>Live block totals are recorded at cache boundaries.</small>
          </div>
        )}
      </div>
      <div className="execution-readout">
        <small className="story-label">Measured work</small>
        <strong data-testid="timeline-rv64-steps">
          {number(run.counters.rv64_steps)}
        </strong>
        <span>RV64 instructions interpreted</span>
        <small>{number(run.counters.block_executions)} block executions</small>
        <Reveal ready={phase >= 3}>
          <div className="cache-state">
            <span>{number(run.counters.compiled_blocks)} compiled</span>
            <span>{number(run.counters.cache_invalidations)} cache resets</span>
            {evidence.invalidations.length > 0 && (
              <div
                className="cache-slates"
                data-cleared={phase === 3}
                aria-label="Recorded translation-cache reset"
              >
                {Array.from(
                  {
                    length: Math.min(
                      8,
                      evidence.invalidations.at(-1)?.blocks ?? 0,
                    ),
                  },
                  (_, i) => (
                    <i key={i} />
                  ),
                )}
              </div>
            )}
            <b>Guest state retained</b>
          </div>
        </Reveal>
      </div>
      <p className="execution-detail">
        {inspected
          ? `${inspected.rip} · ${number(inspected.rv64_steps)} RV64 steps · ${number(inspected.executions)} executions`
          : "Activity follows recorded execution counters"}{" "}
        · busiest recorded translations shown
      </p>
    </div>
  );
}
function SyscallScene({
  run,
  evidence,
  phase,
}: {
  run: Run;
  evidence: IllustrationEvidence;
  phase: number;
}) {
  const [picked, setPicked] = useState<number | null>(null);
  const exchanges = evidence.syscalls;
  const event =
    exchanges.find((e) => e.seq === picked) ??
    exchanges.find((e) => e.detail?.name === "write" && e.detail?.result > 0) ??
    exchanges[0] ??
    run.groups.syscall.last_event;
  if (!event) return <Empty text="No system call has been observed" />;
  const detail = event.detail,
    failed =
      event.type === "syscall_failed" ||
      (typeof detail?.result === "number" && detail.result < 0);
  const completed = event.type === "syscall_completed";
  return (
    <div
      className="syscall-story"
      data-source-seq={event.seq}
      data-syscall-name={detail?.name ?? event.number}
    >
      <div className="exchange-lane">
        <div className="exchange-end">
          <small>Guest</small>
          <strong>sys_{detail?.name ?? event.number ?? "—"}</strong>
          <code>
            {detail?.args?.[0]
              ? `arg 0: ${detail.args[0]}`
              : "Recorded request"}
          </code>
        </div>
        <div className="bridge-crossing">
          <div className="bridge-gate" data-ready={phase >= 1}>
            HOST BRIDGE
          </div>
          <span
            className="request-packet"
            data-ready={phase >= 1}
            data-arrived={phase >= 2}
          >
            request →
          </span>
          <span
            className="response-packet"
            data-ready={phase >= 3 && completed}
          >
            ← {failed ? "error" : `result ${detail?.result ?? "—"}`}
          </span>
        </div>
        <div className="exchange-end" data-ready={phase >= 2}>
          <small>Host</small>
          <strong>
            {completed
              ? failed
                ? "Error returned"
                : "Call completed"
              : failed
                ? "Call failed"
                : "Awaiting result"}
          </strong>
          <code>{duration(event.elapsed_ns)}</code>
        </div>
      </div>
      <div className="syscall-history" aria-label="Recorded host exchanges">
        {exchanges.slice(-8).map((e) => (
          <button
            key={e.seq}
            type="button"
            data-selected={event.seq === e.seq}
            onClick={(click) => {
              click.stopPropagation();
              setPicked(e.seq);
            }}
          >
            <span>{e.detail.name}</span>
            <code>{e.detail.result}</code>
          </button>
        ))}
      </div>
      <small className="visual-caption">
        Recorded request #{event.syscall_seq ?? "—"} · receipt #{event.seq} ·{" "}
        {completed
          ? "Return value from the host bridge"
          : (event.error ?? "Request has no completed receipt")}
      </small>
    </div>
  );
}
export const CardIllustration = memo(function CardIllustration({
  scene,
  run,
  clock,
  evidence,
  visible,
  selected,
  reduced,
  onEvidence,
}: {
  scene: Scene;
  run: Run | null;
  clock: Walkthrough;
  evidence: IllustrationEvidence;
  visible: boolean;
  selected: boolean;
  reduced: boolean;
  onEvidence: () => void;
}) {
  const output = currentOutput(scene.primary),
    blockId = scene.primary.last_event?.block_id;
  const recorded = hasStoryData(scene, run);
  const phase = !recorded ? 0 : reduced ? 4 : selected ? clock.phase : 0;
  const related = (id: string) => {
    const e = currentOutput(run?.groups[id]);
    return blockId !== undefined && e?.block_id === blockId ? e : null;
  };
  const captions = scripts[scene.id];
  const observed = scene.count > 0;
  let content: React.ReactNode;
  if (
    scene.id === "result" &&
    run &&
    terminal(run.status) &&
    !run.engine_result
  )
    content = <Empty text="Run ended without a terminal engine receipt" />;
  else if (!run || !observed)
    content = <Empty text="Waiting for this operation to be recorded" />;
  else
    switch (scene.id) {
      case "ingest":
        content = <InputScene run={run} phase={phase} />;
        break;
      case "initialize":
        content = <MemoryScene run={run} phase={phase} />;
        break;
      case "decode":
        content = <DecodeScene output={output} phase={phase} />;
        break;
      case "audit":
        content = <PolicyScene output={output} phase={phase} />;
        break;
      case "lower":
      case "codegen":
        content = (
          <TranslationVisual
            group={scene.primary}
            run={run}
            output={output}
            decoded={related("decode")}
            lowered={related("lower")}
            visible={visible}
            phase={phase}
          />
        );
        break;
      case "execute":
        content = <ExecuteScene run={run} evidence={evidence} phase={phase} />;
        break;
      case "syscall":
        content = <SyscallScene run={run} evidence={evidence} phase={phase} />;
        break;
      case "result":
        content = run.engine_result ? (
          <div className="result-story" data-reveal={phase}>
            <div className="result-tracks">
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onEvidence();
                }}
                data-ready={phase >= 1}
              >
                Compilation <i>→</i>
              </button>
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onEvidence();
                }}
                data-ready={phase >= 2}
              >
                Execution <i>→</i>
              </button>
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onEvidence();
                }}
                data-ready={phase >= 3}
              >
                TCP check <i>→</i>
              </button>
            </div>
            <PerformanceSummary run={run} />
          </div>
        ) : (
          <Empty
            text={
              terminal(run.status)
                ? "Run ended without a terminal engine receipt"
                : "Waiting for the engine's final result"
            }
          />
        );
        break;
    }
  return (
    <div
      className="card-story"
      data-scene={scene.id}
      data-phase={phase}
      data-playing={selected && clock.playing}
      data-observed={observed}
    >
      <div className="walkthrough-meta">
        <span>
          {run && terminal(run.status)
            ? "Recorded walkthrough"
            : "Illustrated operation"}
        </span>
        <div
          className="story-steps"
          aria-label={`Illustration step ${phase + 1} of 5`}
        >
          {captions.map((caption, i) => (
            <i key={caption} data-reached={phase >= i} title={caption} />
          ))}
        </div>
      </div>
      <div className="story-content">{content}</div>
      <p className="story-caption">
        {scene.state === "Failed" || output?.verdict === "rejected"
          ? "Recorded failure · inspect the finding in Evidence"
          : observed
            ? captions[phase]
            : "No result recorded"}
        {evidence.sampled && <small> · journal sample</small>}
      </p>
    </div>
  );
});
