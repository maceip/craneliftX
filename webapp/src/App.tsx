import React, { useEffect, useRef, useState } from "react";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";
import { Button } from "@graundtech/fluent2-react-kit";
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
import { PipelineTimeline } from "./components/PipelineTimeline";

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
  const [actionError, setActionError] = useState<string | null>(null),
    [setupError, setSetupError] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null),
    { run, connection, error, liveAfter } = usePipeline(id);
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
      setFollowing(false);
      setInspection(null);
      setId(new URLSearchParams(location.search).get("run"));
    };
    addEventListener("popstate", pop);
    return () => removeEventListener("popstate", pop);
  }, []);
  const latestGroup = useRef<string | undefined>(undefined);
  latestGroup.current = run?.recent.at(-1)?.group;
  useEffect(() => {
    if (!following) return;
    // Focus samples the latest actual group; the ledger and values still render
    // every batch. Do not queue a stage tour or chase every short-lived span.
    const focus = () => {
      if (latestGroup.current) {
        setSelected(latestGroup.current);
        setInspection(null);
      }
    };
    focus();
    const timer = setInterval(focus, 600);
    return () => clearInterval(timer);
  }, [following, run?.id]);
  function selectRun(next: string | null) {
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
      setFollowing(true);
      setSelected("ingest");
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
  const groups =
    config?.groups.map((g) => run?.groups[g.id] ?? blankGroup(g)) ?? [];
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
  const runStatus = run?.status
    ? run.status[0].toUpperCase() + run.status.slice(1)
    : submitting
      ? "Starting"
      : "Ready";
  return (
    <div className="app-shell">
      <a className="skip-link" href="#pipeline">
        Skip to pipeline
      </a>
      <header className="masthead">
        <a className="brand" href="/" aria-label="Chunder home">
          <span className="brand-mark" aria-hidden="true">
            c
          </span>
          chunder
          <span className="brand-divider" />
          <small>Execution observatory</small>
        </a>
        <div className="engine-identity">
          <span className={cn("signal-dot", config?.ready && "available")} />
          {config
            ? config.ready
              ? "RV64 runtime available"
              : "Runtime unavailable"
            : "Connecting to runtime"}
        </div>
      </header>
      <main>
        <div className="page-heading">
          <div>
            <div className="eyebrow">x86-64 → CLIF → RV64</div>
            <h1>Execution pipeline</h1>
            <p>Thirteen operation groups. One live execution ledger.</p>
          </div>
          <div className="run-actions">
            <input
              ref={file}
              type="file"
              aria-label="Choose an x86-64 ELF binary"
              className="visually-hidden"
              tabIndex={-1}
              onChange={(e) => {
                const b = e.target.files?.[0];
                if (b) void start(b);
              }}
            />
            <Button
              className="button secondary"
              disabled={busy || !config?.ready}
              onClick={() => file.current?.click()}
            >
              Open binary <span aria-hidden="true">↑</span>
            </Button>
            {busy && run ? (
              <Button
                className="button secondary"
                disabled={run.status === "cancelling"}
                onClick={() => void stop()}
              >
                Stop execution
              </Button>
            ) : (
              <Button
                className="button primary"
                disabled={busy || !config?.ready || !config.server_available}
                onClick={() => void start()}
              >
                <span aria-hidden="true">▶</span>
                {submitting ? "Starting…" : "Run Rust server"}
              </Button>
            )}
          </div>
        </div>
        <div className="profile-line">
          <span>
            Static Linux x86-64 ELF · scalar instructions · loopback TCP
          </span>
          <span>{bytes(config?.max_upload_bytes)} input limit</span>
        </div>
        {(setupError || actionError || config?.error) && (
          <div className="error-banner" role="alert">
            {actionError ?? setupError ?? config?.error}
          </div>
        )}
        <section className="run-bar" aria-label="Current execution">
          <div className="run-file">
            <span className="file-symbol" aria-hidden="true">
              ⌘
            </span>
            <div>
              <strong>{run?.input_name ?? "No binary running"}</strong>
              <small>
                {run
                  ? `${run.mode === "rust-server" ? "Freestanding Rust TCP server" : "Uploaded binary"} · ${bytes(run.input?.bytes)} input`
                  : "Run the Rust server or open a supported executable."}
              </small>
            </div>
          </div>
          <div className="run-bar-right">
            <Status value={runStatus} />
            <label className="history-select">
              <span className="visually-hidden">Saved runs</span>
              <select
                value={id && history.some((h) => h.id === id) ? id : ""}
                onChange={(e) => {
                  if (e.target.value) selectRun(e.target.value);
                }}
              >
                <option value="">Saved runs</option>
                {history.map((h) => (
                  <option key={h.id} value={h.id}>
                    {h.input_name} · {h.status} · {timestamp(h.created_unix_ns)}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </section>
        {error && (
          <div className="error-banner" role="status">
            {error}. The saved journal is retained.
          </div>
        )}
        {run && ["failed", "interrupted", "cancelled"].includes(run.status) && (
          <div className="error-banner" role="alert">
            {run.message}
          </div>
        )}
        <div className="runtime-overview" id="pipeline">
          <section className="runtime-panel" aria-label="Execution counters">
            <div className="section-heading">
              <h2>Runtime</h2>
              <span
                className={cn(
                  "connection-state",
                  streamConnection === "live" && "is-live",
                )}
                role="status"
              >
                <span aria-hidden="true" />
                {connectionLabel[streamConnection]}
              </span>
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
                <strong data-testid="event-sequence">{run?.cursor ?? 0}</strong>
              </span>
            </div>
          </section>
          <Activity run={run} />
        </div>
        <PipelineTimeline
          groups={groups}
          run={run}
          selected={selected}
          onSelect={(id) => {
            setFollowing(false);
            setSelected(id);
            setInspection(null);
          }}
          following={following}
          onFollow={setFollowing}
          connection={streamConnection}
          liveAfter={liveAfter}
        >
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
        </PipelineTimeline>
        <section className="ledger-panel" aria-label="Live event ledger">
          <div className="section-heading">
            <div>
              <h2>
                Event ledger{" "}
                <span className="count-label">{number(run?.cursor ?? 0)}</span>
              </h2>
              <p>Original engine events · newest first · timestamps in UTC</p>
            </div>
            <div className="ledger-controls">
              <label>
                <span className="visually-hidden">Filter ledger by group</span>
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
              Showing {events.length} of the latest 120 events. Complete history
              stays in the journal.
            </span>
            <span>Engine sequence · no generated stage transitions</span>
          </div>
        </section>
        <footer>
          <span>
            chunder <span className="footer-dot">·</span> iced-x86 / Cranelift /
            RV64 interpreter
          </span>
          <span>
            {run?.engine_run_id ? (
              <span className="mono">Run {run.engine_run_id}</span>
            ) : (
              "Static ELF process runtime"
            )}
          </span>
        </footer>
      </main>
    </div>
  );
}
