import React, { useEffect, useRef, useState } from "react";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";
import {
  blankGroup,
  bytes,
  Config,
  duration,
  groupState,
  LedgerEvent,
  number,
  Run,
  RunSummary,
  terminal,
  timestamp,
} from "./pipeline";
import { request, usePipeline } from "./usePipeline";
import { useStageFocus } from "./useStageFocus";
import { PipelineTimeline } from "./components/PipelineTimeline";
import { TelemetryGauge } from "./components/Telemetry";
import { RestartConfirmation } from "./components/RestartConfirmation";
import { stageLabel } from "./pipelineCopy";

const cn = (...values: Parameters<typeof clsx>) => twMerge(clsx(...values));
const descriptions: Record<string, string> = {
  run: "Run identity, execution options, and the engine configuration emitted at startup.",
  ingest:
    "The actual input file, its exact byte length, and its SHA-256 fingerprint.",
  load: "Validated ELF segments mapped into the guest address space.",
  initialize: "Initial guest registers, stack, state, and execution budgets.",
  decode:
    "Instructions reached by the guest and decoded by iced-x86, with their original addresses and bytes.",
  audit:
    "A lightweight inspection of each reached block before compilation: instruction policy, stack operations, memory bounds, and known branch boundaries.",
  lower: "x86 instruction semantics lowered into Cranelift IR for this block.",
  codegen: "Machine code emitted by Cranelift for the RV64 interpreter.",
  artifacts:
    "Source bytes, CLIF, and RV64 code saved with verifiable paths and hashes.",
  execute:
    "RV64 instructions executed, guest state resumed, and cumulative runtime counters.",
  syscall:
    "Guest operating-system requests serviced by the explicit host syscall bridge.",
  cache:
    "Reached blocks compiled, reused, or invalidated while guest state and sockets persist.",
  result:
    "The engine's terminal result. A missing result leaves the run incomplete.",
};
function Status({ value }: { value: string }) {
  return (
    <span
      className={cn(
        "status",
        `status-${value.toLowerCase().replaceAll(" ", "-")}`,
      )}
    >
      <span aria-hidden="true" />
      {value}
    </span>
  );
}
function Metric({
  label,
  value,
  note,
  testId,
}: {
  label: string;
  value: string;
  note: string;
  testId?: string;
}) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong data-testid={testId}>{value}</strong>
      <small>{note}</small>
    </div>
  );
}
function Activity({ run }: { run: Run | null }) {
  const points = run?.history ?? [],
    max = Math.max(1, ...points.map((p) => p.bytes));
  const first = points[0]?.elapsed_ns ?? 0,
    last = points.at(-1)?.elapsed_ns ?? 1;
  const plotted = points
    .map(
      (p) =>
        `${8 + ((p.elapsed_ns - first) / Math.max(1, last - first)) * 704},${78 - (p.bytes / max) * 60}`,
    )
    .join(" ");
  return (
    <section className="activity-panel" aria-label="Live translation activity">
      <div className="section-heading">
        <h2>Translation activity</h2>
        <span className="mono">
          {points.length
            ? `${number(run?.counters.compiled_blocks)} blocks compiled`
            : "Awaiting engine events"}
        </span>
      </div>
      <svg
        viewBox="0 0 720 96"
        role="img"
        aria-label={
          points.length
            ? "Cumulative unique source bytes lifted over elapsed execution time"
            : "No translation activity recorded"
        }
      >
        {[18, 48, 78].map((y) => (
          <line
            key={y}
            x1="8"
            x2="712"
            y1={y}
            y2={y}
            stroke="currentColor"
            className="chart-grid"
          />
        ))}
        {points.length > 1 && (
          <polyline
            points={plotted}
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            className="chart-line"
          />
        )}
        {points.length > 0 && (
          <circle
            cx={points.length === 1 ? 8 : 712}
            cy={78 - (points.at(-1)!.bytes / max) * 60}
            r="3.5"
            className="chart-dot"
          />
        )}
      </svg>
      <div className="chart-caption">
        <span>
          {points.length
            ? `Unique source lifted · ${bytes(points.at(-1)?.bytes)}`
            : "The graph fills from compilation_progress events."}
        </span>
        <span>
          {points.length
            ? `${duration(first)} — ${duration(last)}`
            : "No estimated completion percentage"}
        </span>
      </div>
    </section>
  );
}
function EventOutput({ event, runId }: { event: LedgerEvent; runId?: string }) {
  if (event.type === "security" && Array.isArray(event.checks))
    return (
      <div className="audit-output" data-testid="audit-output">
        <div className="audit-summary">
          <strong>
            {event.verdict === "rejected"
              ? "Instruction policy rejected"
              : "Block accepted by policy"}
          </strong>
          <span className="mono">
            {event.phase === "execute"
              ? "Runtime guard"
              : `${event.instruction_count} / ${event.max_instructions} instructions inspected`}
          </span>
        </div>
        <ul className="audit-checks" aria-label="Instruction policy checks">
          {event.checks.map((check: any) => (
            <li
              key={check.rule}
              className={`audit-check audit-${check.verdict}`}
              data-verdict={check.verdict}
            >
              <div className="audit-check-heading">
                <strong>{check.label}</strong>
                <span className="audit-verdict">
                  {check.verdict === "rejected"
                    ? "Rejected"
                    : check.verdict === "guarded"
                      ? "Runtime checked"
                      : "Passed"}
                </span>
              </div>
              <p>{check.detail}</p>
              {check.findings?.length > 0 && (
                <details open={check.verdict === "rejected"}>
                  <summary>
                    {check.findings.length} instruction{" "}
                    {check.findings.length === 1 ? "finding" : "findings"}
                  </summary>
                  <ul className="audit-findings">
                    {check.findings.map((finding: any, index: number) => (
                      <li key={index}>
                        <code>
                          {finding.rip} · {finding.instruction}
                        </code>
                        <span>{finding.detail}</span>
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </li>
          ))}
        </ul>
        <p className="audit-scope">
          Scope: reached instructions. Runtime checks apply when the block
          executes. No whole-binary taint or gadget-chain classification.
        </p>
      </div>
    );
  const instructions = event.instructions ?? event.details?.instructions,
    mappings = event.details?.segments;
  if (Array.isArray(instructions))
    return (
      <div className="output-table">
        <table>
          <thead>
            <tr>
              <th>Source address</th>
              <th>Bytes</th>
              <th>Instruction</th>
            </tr>
          </thead>
          <tbody>
            {instructions.map((i: any, index: number) => (
              <tr key={index}>
                <td>{i.ip ?? i.rip}</td>
                <td className="encoded">{i.bytes}</td>
                <td>{i.asm ?? i.assembly}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  if (Array.isArray(mappings))
    return (
      <div className="output-table">
        <table>
          <thead>
            <tr>
              <th>Guest address</th>
              <th>File</th>
              <th>Memory</th>
              <th>Permissions</th>
            </tr>
          </thead>
          <tbody>
            {mappings.map((s: any) => (
              <tr key={s.address}>
                <td>{s.address}</td>
                <td>{bytes(s.file_bytes)}</td>
                <td>{bytes(s.memory_bytes)}</td>
                <td>
                  R{s.writable ? "W" : "–"}
                  {s.executable ? "X" : "–"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  if (event.type === "artifact")
    return (
      <div className="artifact-output">
        <div className="file-icon" aria-hidden="true">
          ↳
        </div>
        <div>
          <strong>{event.name}</strong>
          <p>{bytes(event.bytes)} · SHA-256</p>
          <code>{event.sha256}</code>
          {runId && (
            <a
              href={`/api/pipeline/runs/${runId}/artifacts/${encodeURIComponent(event.name)}`}
            >
              Download artifact ↗
            </a>
          )}
        </div>
      </div>
    );
  const excluded = new Set([
    "v",
    "seq",
    "type",
    "group",
    "group_order",
    "timestamp_unix_ns",
    "run_elapsed_ns",
    "run_id",
    "groups",
  ]);
  return (
    <dl className="output-values">
      {Object.entries(event)
        .filter(([key]) => !excluded.has(key))
        .map(([key, value]) => (
          <div key={key}>
            <dt>{key.replaceAll("_", " ")}</dt>
            <dd>
              {typeof value === "object" && value !== null ? (
                <pre>{JSON.stringify(value, null, 2)}</pre>
              ) : (
                <code>{String(value)}</code>
              )}
            </dd>
          </div>
        ))}
    </dl>
  );
}
export function App() {
  const [config, setConfig] = useState<Config | null>(null),
    [history, setHistory] = useState<RunSummary[]>([]);
  const [id, setId] = useState<string | null>(() =>
    new URLSearchParams(location.search).get("run"),
  );
  const [selected, setSelected] = useState("run"),
    [inspection, setInspection] = useState<LedgerEvent | null>(null);
  const [following, setFollowing] = useState(
    () => !new URLSearchParams(location.search).has("run"),
  );
  const [raw, setRaw] = useState(false),
    [filter, setFilter] = useState("all"),
    [submitting, setSubmitting] = useState(false);
  const evidence = useRef<HTMLDialogElement>(null);
  const [evidenceOpen, setEvidenceOpen] = useState(false);
  const [initial, setInitial] = useState<Run | null>(null);
  const [paused, setPaused] = useState(false);
  const [restartGroup, setRestartGroup] = useState<string | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null),
    [setupError, setSetupError] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null),
    { run, connection, error, liveAfter } = usePipeline(id, initial);
  const { reviewing, endReview } = useStageFocus(
    run,
    initial?.id ?? null,
    following,
    paused || evidenceOpen,
    setSelected,
  );
  const busy = submitting || (!!run && !terminal(run.status));
  useEffect(() => {
    let alive = true;
    Promise.all([
      request<Config>("/api/pipeline/config"),
      request<RunSummary[]>("/api/pipeline/runs"),
    ])
      .then(([c, h]) => {
        if (alive) {
          setConfig(c);
          setHistory(h);
        }
      })
      .catch((e) => {
        if (alive) setSetupError(e.message);
      });
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    if (run && terminal(run.status))
      void request<RunSummary[]>("/api/pipeline/runs")
        .then(setHistory)
        .catch(() => {});
  }, [run?.status]);
  useEffect(() => {
    const pop = () => {
      setRestartGroup(null);
      setInitial(null);
      setFollowing(false);
      setInspection(null);
      setId(new URLSearchParams(location.search).get("run"));
    };
    addEventListener("popstate", pop);
    return () => removeEventListener("popstate", pop);
  }, []);
  useEffect(() => {
    if (run && !terminal(run.status)) setFollowing(true);
  }, [run?.id]);
  function selectRun(next: string | null) {
    setRestartGroup(null);
    setInitial(null);
    setPaused(false);
    setFollowing(false);
    setInspection(null);
    setActionError(null);
    setId(next);
    const url = new URL(location.href);
    if (next) url.searchParams.set("run", next);
    else url.searchParams.delete("run");
    window.history.pushState(null, "", url);
  }
  async function start(binary?: File) {
    setActionError(null);
    if (binary && binary.size > (config?.max_upload_bytes ?? 0)) {
      setActionError(
        `Choose a binary of ${bytes(config?.max_upload_bytes)} or smaller.`,
      );
      return;
    }
    setSubmitting(true);
    try {
      const state = await request<Run>("/api/pipeline/runs", {
        method: "POST",
        headers: binary
          ? {
              "Content-Type": "application/octet-stream",
              "X-Binary-Name": encodeURIComponent(binary.name),
            }
          : { "Content-Type": "application/json" },
        body: binary ?? JSON.stringify({ mode: "rust-server" }),
      });
      selectRun(state.id);
      setInitial(state);
      setFollowing(true);
      setSelected("run");
      setRaw(false);
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setSubmitting(false);
      if (file.current) file.current.value = "";
    }
  }
  async function stop() {
    if (!run) return;
    try {
      await request(`/api/pipeline/runs/${run.id}/cancel`, { method: "POST" });
    } catch (e) {
      setActionError((e as Error).message);
    }
  }
  function dismissRestart() {
    const group = restartGroup;
    setRestartGroup(null);
    if (group)
      document
        .getElementById(`timeline-${group}`)
        ?.querySelector<HTMLButtonElement>("button")
        ?.focus({ preventScroll: true });
  }
  async function restart() {
    if (!run || !restartGroup || restarting) return;
    setRestarting(true);
    setActionError(null);
    try {
      const state = await request<Run>(`/api/pipeline/runs/${run.id}/restart`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ focus_group: restartGroup }),
      });
      selectRun(state.id);
      setInitial(state);
      setFollowing(true);
      setSelected(restartGroup);
      setRaw(false);
    } catch (e) {
      setActionError((e as Error).message);
    } finally {
      setRestarting(false);
    }
  }
  const groups =
    config?.groups.map((g) => ({
      ...(run?.groups[g.id] ?? blankGroup(g)),
      label: stageLabel(g),
    })) ?? [];
  const group = groups.find((g) => g.id === selected),
    shown =
      inspection ??
      (group?.active ? group.last_event : (group?.output ?? group?.last_event));
  const active = groups.filter((g) => groupState(g, run?.status) === "Active");
  const events =
    run?.recent
      .filter((e) => filter === "all" || e.group === filter)
      .slice(-60)
      .reverse() ?? [];
  const connectionLabel: Record<string, string> = {
    idle: "Stream idle",
    live: "Live stream",
    saved: "Journal saved",
    connecting: "Connecting",
    reconnecting: "Reconnecting",
    recovering: "Recovering journal",
    "upstream-reconnecting": "Reconnecting to engine",
  };
  const streamConnection =
    connection === "live" && run?.engine_connection === "reconnecting"
      ? "upstream-reconnecting"
      : connection;
  function openEvidence() {
    setRestartGroup(null);
    setFollowing(false);
    endReview();
    setEvidenceOpen(true);
    evidence.current?.showModal();
  }
  return (
    <div className="focus-shell">
      <input
        ref={file}
        type="file"
        aria-label="Choose an x86-64 ELF binary"
        className="visually-hidden"
        tabIndex={-1}
        onChange={(e) => {
          const binary = e.target.files?.[0];
          if (binary) void start(binary);
        }}
      />
      {!id ? (
        <main className="launch-screen">
          <img
            className="launch-logo"
            src="/dist/chunder-icon.png"
            alt="Chunder"
            width={28}
            height={28}
          />
          <h1>
            Translate and execute <span>x86-64</span> binaries on{" "}
            <span>RISC-V.</span>
          </h1>
          <p className="launch-description">
            x86-64 → iced-x86 → Cranelift IR → RV64
          </p>
          <div className="launch-actions">
            <button
              aria-label="Run demo"
              className="launch-action launch-primary"
              disabled={busy || !config?.ready || !config.server_available}
              onClick={() => void start()}
            >
              <span className="launch-symbol" aria-hidden="true">
                ▷
              </span>
              <strong>{submitting ? "Starting…" : "Run demo"}</strong>
              <span>Live Rust TCP server translated to RV64</span>
            </button>
            <button
              aria-label="Upload binary"
              className="launch-action"
              disabled={busy || !config?.ready}
              onClick={() => file.current?.click()}
            >
              <span className="launch-symbol" aria-hidden="true">
                ↑
              </span>
              <strong>Upload binary</strong>
              <span>
                Static Linux x86-64 ELF (up to{" "}
                {bytes(config?.max_upload_bytes).replace(/\.00 MiB$/, " MiB")})
              </span>
            </button>
          </div>
          <p className="launch-caption">
            {config
              ? config.ready
                ? "Live engine events. Actual instructions. Preserved connections."
                : "Runtime unavailable"
              : "Connecting to runtime…"}
          </p>
          {history.length > 0 && (
            <details className="launch-history">
              <summary>Previous runs</summary>
              <label>
                <span className="visually-hidden">Saved runs</span>
                <select
                  value=""
                  onChange={(e) => {
                    if (e.target.value) selectRun(e.target.value);
                  }}
                >
                  <option value="">Choose a recorded run</option>
                  {history.map((h) => (
                    <option key={h.id} value={h.id}>
                      {h.input_name} · {h.status} ·{" "}
                      {timestamp(h.created_unix_ns)}
                    </option>
                  ))}
                </select>
              </label>
            </details>
          )}
          {(setupError || actionError || config?.error) && (
            <p className="focus-error" role="alert">
              {actionError ?? setupError ?? config?.error}
            </p>
          )}
        </main>
      ) : (
        <main className="execution-screen" aria-label="Live execution">
          {(actionError || error) && (
            <p className="focus-error" role="status">
              {actionError ?? error}
            </p>
          )}
          {run &&
            ["failed", "interrupted", "cancelled"].includes(run.status) && (
              <p className="focus-error" role="alert">
                {run.message}
              </p>
            )}
          <TelemetryGauge run={run} stage={selected} reviewing={reviewing} />
          <PipelineTimeline
            groups={groups}
            run={run}
            selected={selected}
            onSelect={(id) => {
              setFollowing(false);
              endReview();
              setSelected(id);
              setInspection(null);
              setRestartGroup(id);
            }}
            following={following}
            onFollow={(value) => {
              setFollowing(value);
              if (value) setInspection(null);
            }}
            reviewing={reviewing}
            paused={paused}
            onPause={setPaused}
            confirmationFor={restartGroup}
            confirmation={
              restartGroup && run ? (
                <RestartConfirmation
                  stage={
                    groups.find((g) => g.id === restartGroup)?.label ??
                    restartGroup
                  }
                  busy={restarting}
                  description={`${!terminal(run.status) ? "Stops the current run. " : ""}Starts a new execution of this binary, with the view beginning here. Required earlier stages run again.`}
                  onConfirm={() => void restart()}
                  onCancel={dismissRestart}
                />
              ) : null
            }
            connection={streamConnection}
            liveAfter={liveAfter}
            obscured={evidenceOpen}
            status={
              <>
                <span role="status">
                  {reviewing
                    ? `${
                        run && terminal(run.status)
                          ? run.status === "completed"
                            ? "Engine complete"
                            : `Run ${run.status}`
                          : connectionLabel[streamConnection]
                      } · ${following && !paused ? "Reviewing recorded stages" : "Stage review paused"}`
                    : connectionLabel[streamConnection]}
                </span>
              </>
            }
            controls={
              <>
                <button
                  type="button"
                  onClick={openEvidence}
                  aria-label="View evidence"
                >
                  Evidence
                </button>
                {busy && run ? (
                  <button
                    type="button"
                    aria-label="Stop execution"
                    disabled={run.status === "cancelling" || restarting}
                    onClick={() => void stop()}
                  >
                    Stop
                  </button>
                ) : (
                  <button
                    type="button"
                    aria-label="Start a new run"
                    disabled={restarting}
                    onClick={() => selectRun(null)}
                  >
                    New run
                  </button>
                )}
              </>
            }
          />
        </main>
      )}
      <dialog
        ref={evidence}
        className="evidence-dialog"
        aria-labelledby="evidence-title"
        onClose={() => setEvidenceOpen(false)}
      >
        <div className="evidence-heading">
          <h2 id="evidence-title">Execution evidence</h2>
          <button type="button" onClick={() => evidence.current?.close()}>
            Back to execution
          </button>
        </div>
        <div className="evidence-body">
          <p className="evidence-run-caption mono">
            {run?.input_name ?? "Starting execution"} · Event #
            <span data-testid="event-sequence">{run?.cursor ?? 0}</span>
          </p>
          <label className="evidence-group">
            Inspect operation group
            <select
              aria-label="Inspect operation group"
              value={selected}
              onChange={(e) => {
                setSelected(e.target.value);
                setInspection(null);
              }}
            >
              {groups.map((g) => (
                <option value={g.id} key={g.id}>
                  {g.order}. {g.label}
                </option>
              ))}
            </select>
          </label>
          <section className="detail-panel" aria-label="Operation inspector">
            <div className="section-heading">
              <div className="detail-title">
                <span className="detail-index">
                  {String(group?.order ?? 5).padStart(2, "0")}
                </span>
                <div>
                  <span className="eyebrow">
                    {inspection ? "Selected event" : "Operation inspector"}
                  </span>
                  <h2>{group?.label ?? "Loading operation catalog"}</h2>
                </div>
              </div>
              {group && <Status value={groupState(group, run?.status)} />}
            </div>
            <p className="detail-description">{descriptions[selected]}</p>
            {group && group.count > 0 && (
              <div className="group-stats">
                <span>
                  <strong>{number(group.count)}</strong> events
                </span>
                <span>
                  <strong>{number(group.completed)}</strong> completed spans
                </span>
                <span>
                  <strong>{duration(group.duration_ns)}</strong> recorded span
                  time
                </span>
                {group.active > 0 && (
                  <span>
                    <strong>{group.active}</strong> open spans
                  </span>
                )}
              </div>
            )}
            {shown ? (
              <>
                <div className="output-toolbar">
                  <div className="mono">
                    <span>#{shown.seq}</span> {shown.type}
                    {shown.block_id !== undefined
                      ? ` · block ${shown.block_id}`
                      : ""}
                    {shown.rip ? ` · ${shown.rip}` : ""}
                  </div>
                  <button
                    className="text-button"
                    aria-pressed={raw}
                    onClick={() => setRaw(!raw)}
                  >
                    {raw ? "View output" : "View raw JSON"}
                  </button>
                </div>
                {raw ? (
                  <pre className="raw-output" data-testid="raw-event">
                    {JSON.stringify(shown, null, 2)}
                  </pre>
                ) : (
                  <EventOutput event={shown} runId={run?.id} />
                )}
                <div className="event-provenance">
                  <span>Emitted {timestamp(shown.timestamp_unix_ns)} UTC</span>
                  <span>+{duration(shown.run_elapsed_ns)}</span>
                </div>
              </>
            ) : (
              <div className="empty-output">
                <span className="empty-glyph" aria-hidden="true">
                  {selected === "audit" ? "—" : "⌁"}
                </span>
                <h3>
                  {selected === "audit"
                    ? "No instruction inspection recorded"
                    : "No events for this operation yet"}
                </h3>
                <p>
                  {selected === "audit"
                    ? "New process runs inspect each decoded block before lowering. Older saved runs may have no audit events."
                    : "Actual output will appear here when the engine emits an event for this group."}
                </p>
              </div>
            )}
          </section>
          {run?.verification && (
            <section
              className="network-panel"
              aria-label="Network verification"
            >
              <div>
                <span className="eyebrow">Host TCP test client</span>
                <h2>
                  {run.verification.status === "passed"
                    ? "Persistent connection verified"
                    : terminal(run.status)
                      ? "Stopped before verification finished"
                      : "Exercising the Rust server"}
                </h2>
                <p>
                  {number(run.verification.verified_responses)} responses
                  checked · {bytes(run.verification.payload_bytes)} payload ·{" "}
                  {run.verification.first_connection_responses} requests on the
                  first connection
                </p>
              </div>
              <Status
                value={
                  run.verification.status === "passed"
                    ? "Passed"
                    : terminal(run.status)
                      ? "Interrupted"
                      : "Running"
                }
              />
            </section>
          )}
          <section className="runtime-panel" aria-label="Execution counters">
            <div className="section-heading">
              <h2>Runtime</h2>
            </div>
            <div className="metrics-grid">
              <Metric
                label="Compiled blocks"
                value={number(run?.counters.compiled_blocks)}
                note="Includes recompilation"
                testId="compiled-blocks"
              />
              <Metric
                label="Source lifted"
                value={bytes(run?.counters.unique_lifted_bytes)}
                note={`${number(run?.counters.unique_lifted_instructions)} unique instructions`}
              />
              <Metric
                label="RV64 instructions"
                value={number(run?.counters.rv64_steps)}
                note="Executed by the interpreter"
                testId="rv64-steps"
              />
              <Metric
                label="Elapsed"
                value={duration(run?.elapsed_ns)}
                note="Last emitted engine timestamp"
              />
            </div>
            <div className="runtime-caption">
              <span>
                Active:{" "}
                <strong>
                  {active.length
                    ? active.map((g) => g.id).join(" / ")
                    : run && terminal(run.status)
                      ? "Execution ended"
                      : "Awaiting events"}
                </strong>
              </span>
              <span className="mono">
                seq{" "}
                <strong data-evidence-sequence="true">
                  {run?.cursor ?? 0}
                </strong>
              </span>
            </div>
          </section>
          <Activity run={run} />
          <section className="ledger-panel" aria-label="Live event ledger">
            <div className="section-heading">
              <div>
                <h2>
                  Event ledger{" "}
                  <span className="count-label">
                    {number(run?.cursor ?? 0)}
                  </span>
                </h2>
                <p>Original engine events · newest first · timestamps in UTC</p>
              </div>
              <div className="ledger-controls">
                <label>
                  <span className="visually-hidden">
                    Filter ledger by group
                  </span>
                  <select
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                  >
                    <option value="all">All groups</option>
                    {groups.map((g) => (
                      <option key={g.id} value={g.id}>
                        {g.id}
                      </option>
                    ))}
                  </select>
                </label>
                {run && run.cursor > 0 && (
                  <a href={`/api/pipeline/runs/${run.id}/journal`}>
                    Download JSONL ↗
                  </a>
                )}
              </div>
            </div>
            <div className="ledger-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Sequence</th>
                    <th>Timestamp</th>
                    <th>Group</th>
                    <th>Event</th>
                    <th>State / detail</th>
                    <th>Source / block</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map((e) => (
                    <tr
                      key={e.seq}
                      className={cn(inspection?.seq === e.seq && "inspected")}
                    >
                      <td>
                        <button
                          className="event-link"
                          onClick={() => {
                            setFollowing(false);
                            setSelected(e.group);
                            setInspection(e);
                            setRaw(true);
                            document
                              .querySelector(".detail-panel")
                              ?.scrollIntoView({
                                behavior: "instant",
                                block: "nearest",
                              });
                          }}
                          aria-label={`Inspect event ${e.seq}`}
                        >
                          #{e.seq}
                        </button>
                      </td>
                      <td>{timestamp(e.timestamp_unix_ns)}</td>
                      <td>
                        <span className="group-tag">{e.group}</span>
                      </td>
                      <td>{e.type === "stage" ? e.stage : e.type}</td>
                      <td
                        className={
                          e.status === "failed" || e.type === "syscall_failed"
                            ? "failed-text"
                            : ""
                        }
                      >
                        {e.status ??
                          e.state ??
                          e.detail?.name ??
                          (e.type === "artifact" ? e.name : "recorded")}
                      </td>
                      <td>
                        {e.rip ?? e.block_rip ?? "—"}
                        {e.block_id !== undefined ? ` / ${e.block_id}` : ""}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!events.length && (
                <div className="ledger-empty">
                  {run
                    ? "No matching events in the recent window. The complete journal is available to download."
                    : "Start a run to see the engine's events as they are emitted."}
                </div>
              )}
            </div>
            <div className="ledger-footer">
              <span>
                Showing {events.length} of the latest 120 events. Complete
                history stays in the journal.
              </span>
              <span>Engine sequence · no generated stage transitions</span>
            </div>
          </section>

          <p className="evidence-note">
            Original engine receipts · repeating groups follow reached blocks ·
            animation never advances a counter.
          </p>
        </div>
      </dialog>
    </div>
  );
}
