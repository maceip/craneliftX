import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { fibDir, makeProj } from "thinking-orbs";
import { PerformanceSummary } from "./Telemetry";
import { policyLabels, stageStatus } from "../pipelineCopy";
import {
  bytes,
  duration,
  Group,
  groupState,
  LedgerEvent,
  number,
  Run,
  terminal,
  timestamp,
} from "../pipeline";

// Display only output belonging to the currently observed block. In particular,
// never join block B's decode with block A's completed lowering/code generation.
export function currentOutput(group?: Group): LedgerEvent | null {
  if (!group) return null;
  const last = group.last_event,
    output = group.output;
  if (
    last?.block_id !== undefined &&
    output?.block_id !== undefined &&
    last.block_id !== output.block_id
  )
    return null;
  return output;
}
function related(run: Run | null, id: string, blockId: unknown) {
  const event = currentOutput(run?.groups[id]);
  return blockId !== undefined && event?.block_id === blockId ? event : null;
}
function Orb({ items, label }: { items: string[]; label: string }) {
  const project = makeProj(0.45, 0.25, 36, 36, 27);
  // Reuse the recovered orb's sphere geometry. Each dot corresponds to an
  // actual observed group/instruction; there are no decorative filler dots.
  return (
    <svg
      className="ledger-orb"
      viewBox="0 0 72 72"
      role="img"
      aria-label={label}
    >
      <circle className="orb-guide" cx="36" cy="36" r="28" />
      <g className="orb-dots">
        {items.map((item, index) => {
          const [x, y, z] = project(...fibDir(index, items.length));
          return (
            <circle
              key={item}
              data-ledger-dot={item}
              cx={x}
              cy={y}
              r={z > 0 ? 1.8 : 1.2}
              opacity={z > 0 ? 1 : 0.4}
            >
              <title>{item}</title>
            </circle>
          );
        })}
      </g>
    </svg>
  );
}
function Beam({
  from,
  to,
  source,
  destination,
}: {
  from: string;
  to: string;
  source: string;
  destination: string;
}) {
  return (
    <div className="receipt-beam">
      <div className="beam-end">
        <small>{from}</small>
        <strong>{source}</strong>
      </div>
      <svg viewBox="0 0 100 30" aria-hidden="true" className="beam-wire">
        <path className="beam-track" d="M4 15H96" />
        <path className="beam-dashes" d="M4 15H96" />
        <circle className="beam-particle" cx="7" cy="15" r="3" />
        <path className="beam-arrow" d="m89 9 7 6-7 6" />
      </svg>
      <div className="beam-end">
        <small>{to}</small>
        <strong>{destination}</strong>
      </div>
    </div>
  );
}
// These outlines describe operations, not measurements. Data-bearing marks
// (segments, opcodes, checks, dots) are rendered only from actual receipts.
const operationOutlines = {
  memory: "M8 9H56V53H8ZM8 23H56M8 38H56M17 15H23M17 30H30M17 45H24",
  registers:
    "M12 14H52V50H12ZM21 22H43V29H21ZM21 35H43V42H21ZM21 6V14M32 6V14M43 6V14M21 50V58M32 50V58M43 50V58",
  decode:
    "M7 14H24V27H7ZM7 37H24V50H7ZM30 32H55M47 24L55 32L47 40M39 13H57M39 51H57",
  policy: "M8 12H56L38 34V49L26 55V34ZM17 21H47",
  files: "M19 8H48V46H19ZM12 16V53H41M26 19H40M26 26H40M26 33H34",
  result: "M32 8A24 24 0 1 1 31.99 8M32 18V32L41 39",
  syscall: "M6 15H24V49H6ZM40 15H58V49H40ZM24 27H40M34 21L40 27L34 33M40 39H24",
};
function Waiting({
  children,
  kind,
}: {
  children: React.ReactNode;
  kind: keyof typeof operationOutlines;
}) {
  return (
    <div className="visual-waiting">
      <svg className="waiting-diagram" viewBox="0 0 64 64" aria-hidden="true">
        <path d={operationOutlines[kind]} />
        <path className="operation-scan" d="M6 8H58" />
      </svg>
      <span>{children}</span>
    </div>
  );
}
function StageVisual({ group, run }: { group: Group; run: Run | null }) {
  const output = currentOutput(group);
  const event = group.active ? group.last_event : (output ?? group.last_event);
  const complete = output?.status === "completed";
  const blockId = group.last_event?.block_id;
  switch (group.id) {
    case "run": {
      const observed = Object.values(run?.groups ?? {}).filter(
        (g) => g.count > 0,
      );
      return (
        <div className="orb-caption">
          <Orb
            items={observed.map((g) => g.id)}
            label={`${observed.length} operation groups have recorded events`}
          />
          <div>
            <strong>
              {Array.isArray(output?.groups)
                ? `${output.groups.length} trace groups registered`
                : "Waiting for a run"}
            </strong>
            <code>
              Engines: {output?.decoder ?? "—"} · {output?.backend ?? "—"}
            </code>
            <small>
              {Array.isArray(output?.groups)
                ? "Group tracking enabled"
                : "Awaiting trace group registration"}
            </small>
          </div>
        </div>
      );
    }
    case "ingest":
      return (
        <div className="input-illustration">
          <div className="binary-sheet" aria-hidden="true">
            <span />
            <span />
            <span />
            <i className="scan-light" />
          </div>
          <div>
            <strong>{run?.input_name ?? "Open a binary to begin"}</strong>
            <span>
              Payload: {bytes(run?.input?.bytes)}
              {run?.input ? " read" : " · input size not recorded"}
            </span>
            <code className="hash-strip" data-testid="ingest-hash">
              {run?.input?.sha256 ?? "Awaiting the input fingerprint"}
            </code>
            <small>SHA-256</small>
          </div>
        </div>
      );
    case "load": {
      const segments = output?.details?.segments;
      if (!Array.isArray(segments))
        return (
          <Waiting kind="memory">
            {group.active
              ? "Validating the ELF and mapping its load segments"
              : "Load mappings have not been recorded"}
          </Waiting>
        );
      const largest = Math.max(...segments.map((s) => s.memory_bytes));
      return (
        <div className="segment-illustration" data-source-seq={output?.seq}>
          {segments.slice(0, 4).map((s) => (
            <div
              className="segment-row"
              key={s.address}
              data-address={s.address}
              data-bytes={s.memory_bytes}
            >
              <code>{s.address}</code>
              <div className="segment-track">
                <span
                  style={{
                    width: `${largest ? (s.memory_bytes / largest) * 100 : 0}%`,
                  }}
                />
                <i className="scan-light" />
              </div>
              <span>
                {bytes(s.memory_bytes)} · R{s.writable ? "W" : "–"}
                {s.executable ? "X" : "–"}
              </span>
            </div>
          ))}
          <small>
            Virtual address space configured · {segments.length} mapped segments
            {segments.length > 4 ? " · first four shown" : ""}
          </small>
        </div>
      );
    }
    case "initialize":
      return complete ? (
        <div className="register-illustration" data-source-seq={output.seq}>
          {[
            ["Guest RIP", output.entry_rip],
            ["Guest RSP", output.guest_rsp],
            ["Host RV64 Stack", bytes(output.rv64_stack_bytes)],
          ].map(([key, value]) => (
            <div key={key}>
              <small>{key}</small>
              <code>{value}</code>
            </div>
          ))}
          <small>
            {number(output.state_words)} architectural registers initialized
          </small>
        </div>
      ) : (
        <Waiting kind="registers">
          {group.active
            ? "Initializing guest registers and stacks"
            : "Guest state has not been initialized"}
        </Waiting>
      );
    case "decode": {
      const instructions =
        output?.type === "harvest" ? output.instructions : null;
      if (!Array.isArray(instructions))
        return (
          <Waiting kind="decode">
            {group.active
              ? `Decoding the reached block at ${group.last_event?.rip ?? "—"}`
              : "No decoded instructions yet"}
          </Waiting>
        );
      return (
        <div
          className="decode-illustration"
          data-source-seq={output!.seq}
          data-block-id={output!.block_id}
        >
          <Orb
            items={instructions.map((i) => i.ip)}
            label={`${instructions.length} dots, one per decoded instruction`}
          />
          <div
            className="opcode-scroller"
            role="region"
            tabIndex={0}
            aria-label="Decoded instruction bytes"
          >
            <ol className="opcode-track">
              {instructions.map((i) => (
                <li key={i.ip} data-ip={i.ip} data-bytes={i.bytes}>
                  <code>{i.ip}</code>
                  <strong>{i.bytes}</strong>
                  <span>{i.asm}</span>
                </li>
              ))}
            </ol>
            <small>
              {instructions.length} instructions · block {output!.block_id} ·
              Receipt: #{output!.seq}
            </small>
          </div>
        </div>
      );
    }
    case "audit":
      return Array.isArray(output?.checks) ? (
        <ul className="policy-illustration" data-source-seq={output.seq}>
          {output.checks.map((check: any) => (
            <li key={check.rule} className={`policy-${check.verdict}`}>
              <span aria-hidden="true" />
              <strong title={check.detail ?? check.label}>
                {policyLabels[check.rule] ?? check.label}
              </strong>
              <small>
                {check.verdict === "rejected"
                  ? "Rejected"
                  : check.verdict === "guarded"
                    ? "Runtime checked"
                    : "Passed"}
              </small>
            </li>
          ))}
        </ul>
      ) : (
        <Waiting kind="policy">
          {group.active
            ? "Inspecting instruction, stack, memory and branch policy"
            : "No instruction-policy findings recorded"}
        </Waiting>
      );
    case "lower": {
      const decoded = related(run, "decode", blockId);
      return (
        <div data-block-id={blockId} data-source-seq={output?.seq}>
          <Beam
            from="Source"
            to="Output · Cranelift IR"
            source={
              decoded?.instructions
                ? `${decoded.instructions.length} x86 instructions`
                : "—"
            }
            destination={complete ? bytes(output?.clif_bytes) : "Awaiting CLIF"}
          />
          <small className="visual-caption">
            {group.last_event?.rip ?? "No reached block"}
            {blockId !== undefined ? ` · block ${blockId}` : ""}
            {complete ? ` · Duration: ${duration(output?.elapsed_ns)}` : ""}
          </small>
        </div>
      );
    }
    case "codegen": {
      const lowered = related(run, "lower", blockId);
      return (
        <div data-block-id={blockId} data-source-seq={output?.seq}>
          <Beam
            from="Input · Cranelift IR"
            to="Output · RV64 machine code"
            source={
              lowered?.status === "completed" ? bytes(lowered.clif_bytes) : "—"
            }
            destination={complete ? bytes(output?.rv64_bytes) : "Awaiting RV64"}
          />
          <small className="visual-caption">
            {complete
              ? `Target: ${output?.target}`
              : "Waiting for code-generation output"}
            {blockId !== undefined ? ` · block ${blockId}` : ""}
          </small>
        </div>
      );
    }
    case "artifacts": {
      const files = [
        ...new Map(
          [
            ...(run?.recent ?? []).filter((e) => e.type === "artifact"),
            ...(output?.type === "artifact" ? [output] : []),
          ].map((e) => [e.seq, e]),
        ).values(),
      ]
        .sort((a, b) => b.seq - a.seq)
        .slice(0, 3);
      return files.length ? (
        <ul className="file-illustration">
          {files.map((file) => (
            <li key={file.seq} data-source-seq={file.seq}>
              <span className="receipt-file" aria-hidden="true">
                ↳
              </span>
              <div>
                <code>{file.name}</code>
                <small>
                  {bytes(file.bytes)} · SHA-256 {file.sha256.slice(0, 12)}…
                </small>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <Waiting kind="files">
          {group.active
            ? "Persisting source, CLIF and RV64 artifacts"
            : "No artifacts saved"}
        </Waiting>
      );
    }
    case "execute":
      return (
        <div className="execution-illustration">
          <div className="execution-register" aria-hidden="true">
            <span />
            <span />
            <span />
          </div>
          <div>
            <strong data-testid="timeline-rv64-steps">
              {number(run?.counters.rv64_steps)}
            </strong>
            <span>RV64 instructions interpreted</span>
            <small>
              {number(run?.counters.block_executions)} basic block executions
            </small>
          </div>
        </div>
      );
    case "syscall":
      return event ? (
        <div data-source-seq={event.seq}>
          <Beam
            from="Guest Call"
            to="Host Intercept"
            source={
              event.detail?.name
                ? `sys_${event.detail.name}${typeof event.detail.number === "number" ? ` (0x${event.detail.number.toString(16)})` : ""}`
                : event.number !== undefined
                  ? `#${event.number}`
                  : "—"
            }
            destination={
              event.type === "syscall_completed"
                ? `Returned ${event.detail?.result ?? "—"}`
                : event.type === "syscall_failed"
                  ? "Failed"
                  : "Awaiting result"
            }
          />
          <small className="visual-caption">
            {event.syscall_rip ?? event.detail?.rip ?? ""}
            {event.elapsed_ns !== undefined
              ? ` · ${duration(event.elapsed_ns)}`
              : ""}{" "}
            · Receipt: #{event.seq}
          </small>
        </div>
      ) : (
        <Waiting kind="syscall">No system call has been observed</Waiting>
      );
    case "cache":
      return (
        <div className="cache-illustration">
          <div className="cache-tiles">
            {(run?.history ?? []).slice(-4).map((p) => (
              <div key={p.seq} data-source-seq={p.seq}>
                <small>Compilation</small>
                <strong>{p.blocks}</strong>
                <code>{bytes(p.bytes)} source reached</code>
              </div>
            ))}
          </div>
          <small>
            {number(run?.counters.compiled_blocks)} compiled blocks ·{" "}
            {number(run?.counters.cache_invalidations)} invalidations · guest
            context intact
          </small>
        </div>
      );
    case "result":
      return run?.engine_result ? (
        <PerformanceSummary run={run} />
      ) : (
        <Waiting kind="result">
          {run && terminal(run.status)
            ? "Run ended without a terminal engine receipt"
            : "Waiting for the engine's final result"}
        </Waiting>
      );
    default:
      return null;
  }
}
function TimelineStep({
  group,
  run,
  selected,
  context,
  onSelect,
  enabled,
  liveAfter,
  reviewing,
  confirming,
}: {
  group: Group;
  run: Run | null;
  selected: boolean;
  context: string;
  onSelect: () => void;
  enabled: boolean;
  liveAfter: number | null;
  reviewing: boolean;
  confirming: boolean;
}) {
  const element = useRef<HTMLLIElement>(null);
  const [visible, setVisible] = useState(false),
    [flash, setFlash] = useState(false),
    [prominence, setProminence] = useState(0);
  const seq = group.last_event?.seq ?? 0,
    seen = useRef(seq);
  const state = groupState(group, run?.status);
  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) =>
      setVisible(entry.isIntersecting),
    );
    if (element.current) observer.observe(element.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let observer: IntersectionObserver;
    const observe = () => {
      observer?.disconnect();
      // The center 60% of the viewport controls readability, independently of
      // selection and engine activity. No scroll listener or animation loop.
      const inset = Math.round(innerHeight * 0.2);
      observer = new IntersectionObserver(
        ([entry]) => {
          setProminence(entry.intersectionRatio);
        },
        {
          rootMargin: `-${inset}px 0px -${inset}px 0px`,
          threshold: Array.from({ length: 21 }, (_, i) => i / 20),
        },
      );
      if (element.current) observer.observe(element.current);
    };
    observe();
    window.addEventListener("resize", observe);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", observe);
    };
  }, []);
  const motion = enabled && visible;
  const reviewed = useRef(false);
  useEffect(() => {
    const fresh = seq !== seen.current;
    const enteredReview = reviewing && selected && !reviewed.current;
    reviewed.current = reviewing && selected;
    seen.current = seq;
    const pulse =
      motion &&
      ((fresh && liveAfter !== null && seq > liveAfter) ||
        (enteredReview && seq > 0));
    setFlash(pulse);
    if (pulse) {
      const timer = setTimeout(() => setFlash(false), 220);
      return () => clearTimeout(timer);
    }
  }, [seq, motion, liveAfter, reviewing, selected]);
  const busy =
    motion &&
    !!run &&
    !terminal(run.status) &&
    group.active > 0 &&
    state === "Active";
  return (
    <li
      ref={element}
      id={`timeline-${group.id}`}
      className={`timeline-step ${selected ? "is-current" : ""}`}
      data-group={group.id}
      data-context={context}
      data-state={state}
      data-motion={busy ? "active-span" : flash ? "receipt" : "off"}
      data-motion-seq={seq || undefined}
      data-prominence={prominence.toFixed(2)}
      data-completed={state === "Completed"}
      style={
        {
          "--card-opacity": 0.42 + Math.min(1, prominence / 0.75) * 0.58,
        } as React.CSSProperties
      }
    >
      <span className="timeline-node" aria-hidden="true">
        {String(group.order).padStart(2, "0")}
      </span>
      {["Completed", "Observed"].includes(state) && (
        <span
          className="completion-detent"
          aria-label={
            state === "Completed"
              ? "Completed in this run"
              : "Recorded in this run"
          }
        />
      )}
      <div className="timeline-card" onClick={onSelect}>
        <button
          type="button"
          className="timeline-stage-button"
          data-testid={`group-${group.id}`}
          data-state={state}
          aria-pressed={selected}
          aria-expanded={confirming}
          aria-controls={confirming ? "restart-confirmation" : undefined}
        >
          <span>
            <strong>{group.label}</strong>
            <small>
              {group.count
                ? `${number(group.count)} ${group.count === 1 ? "event" : "events"} · ${number(group.completed)} completed ${group.completed === 1 ? "span" : "spans"}`
                : "No events recorded"}
            </small>
          </span>
          <span
            className={`timeline-state state-${state.toLowerCase().replaceAll(" ", "-")}`}
          >
            <i aria-hidden="true" />
            {stageStatus(group, run?.status)}
          </span>
        </button>
        <div className="stage-illustration" data-stage-visual={group.id}>
          <StageVisual group={group} run={run} />
          <span
            key={`${seq}:${selected}`}
            className="receipt-glow"
            aria-hidden="true"
          />
        </div>
        <div className="timeline-provenance">
          <span>
            {group.last_event
              ? `${timestamp(group.last_event.timestamp_unix_ns)} UTC · Event #${seq}`
              : "Awaiting an engine event"}
          </span>
          <span>
            {group.last_event?.block_id !== undefined
              ? `Block ${group.last_event.block_id}`
              : group.last_event?.type === "stage"
                ? group.last_event.stage
                : (group.last_event?.type ?? "")}
          </span>
        </div>
      </div>
    </li>
  );
}
export function PipelineTimeline({
  groups,
  run,
  selected,
  onSelect,
  following,
  onFollow,
  connection,
  liveAfter,
  controls,
  status,
  obscured,
  reviewing,
  paused,
  onPause,
  confirmation,
  confirmationFor,
}: {
  groups: Group[];
  run: Run | null;
  selected: string;
  onSelect: (id: string) => void;
  following: boolean;
  onFollow: (value: boolean) => void;
  connection: string;
  liveAfter: number | null;
  controls: React.ReactNode;
  status: React.ReactNode;
  obscured: boolean;
  reviewing: boolean;
  paused: boolean;
  onPause: (value: boolean) => void;
  confirmation?: React.ReactNode;
  confirmationFor: string | null;
}) {
  const [reduced, setReduced] = useState(false),
    [visible, setVisible] = useState(true);
  useEffect(() => {
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => {
        setReduced(media.matches);
      },
      visibility = () => setVisible(document.visibilityState === "visible");
    update();
    visibility();
    media.addEventListener("change", update);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      media.removeEventListener("change", update);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, []);
  const index = groups.findIndex((g) => g.id === selected);
  const previousFocus = useRef({ id: "", index: -1 });
  useLayoutEffect(() => {
    if (!run || obscured) return;
    if (
      !following &&
      previousFocus.current.id === run.id &&
      previousFocus.current.index === index
    )
      return;
    const step = document.getElementById(`timeline-${selected}`);
    if (!step) return;
    // Move naturally between adjacent observed groups. The overview's reading
    // pace is independent of execution; it never creates a group receipt.
    const adjacent =
      following &&
      !paused &&
      !reduced &&
      previousFocus.current.id === run.id &&
      Math.abs(index - previousFocus.current.index) === 1;
    previousFocus.current = { id: run.id, index };
    const center = (behavior: ScrollBehavior = "instant") => {
      const rect = step.getBoundingClientRect();
      window.scrollTo({
        top: window.scrollY + rect.top + rect.height / 2 - innerHeight / 2,
        behavior,
      });
    };
    center(adjacent ? "smooth" : "instant");
    let measured = false;
    const observer = new ResizeObserver(() => {
      if (measured) center();
      measured = true;
    });
    observer.observe(step);
    return () => observer.disconnect();
  }, [following, selected, run?.id, obscured, paused, reduced]);
  useEffect(() => {
    const manual = () => onFollow(false);
    const keyboard = (e: KeyboardEvent) => {
      if (
        [
          "PageDown",
          "PageUp",
          "Home",
          "End",
          "ArrowDown",
          "ArrowUp",
          " ",
        ].includes(e.key) &&
        !["INPUT", "SELECT", "BUTTON", "TEXTAREA"].includes(
          (e.target as HTMLElement)?.tagName,
        )
      )
        manual();
    };
    window.addEventListener("wheel", manual, { passive: true });
    window.addEventListener("touchmove", manual, { passive: true });
    window.addEventListener("keydown", keyboard);
    return () => {
      window.removeEventListener("wheel", manual);
      window.removeEventListener("touchmove", manual);
      window.removeEventListener("keydown", keyboard);
    };
  }, [onFollow]);
  const enabled =
    !paused &&
    !obscured &&
    !reduced &&
    visible &&
    (connection === "live" ||
      (reviewing && following && connection === "saved")) &&
    !!run &&
    (!terminal(run.status) || (reviewing && following));
  const showPlayback = reviewing || (!!run && !terminal(run.status));
  return (
    <section
      className="pipeline-timeline"
      aria-label="Connected pipeline timeline"
      data-testid="pipeline-timeline"
      data-motion-enabled={enabled}
      data-following={following}
      data-reviewing={reviewing}
      data-paused={paused}
    >
      <div className="focus-transport" data-expanded={!!confirmation}>
        {confirmation}
        <div className="focus-stream-caption">{status}</div>
        <div className="timeline-controls">
          {showPlayback && (
            <>
              <button
                type="button"
                aria-label={
                  reviewing ? "Follow stage review" : "Follow live events"
                }
                aria-pressed={following}
                onClick={() => onFollow(!following)}
              >
                {reviewing ? "Follow stages" : "Follow live"}
              </button>
              <button
                type="button"
                aria-label={paused ? "Resume animations" : "Pause animations"}
                aria-pressed={paused}
                onClick={() => onPause(!paused)}
              >
                {paused ? "Resume motion" : "Pause motion"}
              </button>
            </>
          )}
          {controls}
        </div>
      </div>
      <div className="timeline-layout">
        <ol className="timeline-list" aria-label="Pipeline operation groups">
          {groups.map((group, i) => (
            <TimelineStep
              key={`${run?.id ?? "idle"}:${group.id}`}
              group={group}
              run={run}
              selected={group.id === selected}
              context={
                i === index
                  ? "current"
                  : i === index - 1
                    ? "previous"
                    : i === index + 1
                      ? "next"
                      : "distant"
              }
              onSelect={() => onSelect(group.id)}
              enabled={enabled}
              liveAfter={liveAfter}
              reviewing={reviewing && following}
              confirming={confirmationFor === group.id}
            />
          ))}
        </ol>
      </div>
      <p className="visually-hidden">
        Motion indicates an open engine span or an observed receipt. Stage
        review shows recorded operations at a readable pace; values and engine
        time continue updating independently from the ledger.{" "}
        {reduced ? "Reduced motion is enabled." : ""}
      </p>
    </section>
  );
}
