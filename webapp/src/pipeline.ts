export type LedgerEvent = {
  v: number;
  seq: number;
  run_id: string;
  type: string;
  group: string;
  group_order: number;
  timestamp_unix_ns: string;
  run_elapsed_ns: number;
  [key: string]: any;
};
export type CatalogGroup = { id: string; order: number; label: string };
export type Group = CatalogGroup & {
  count: number;
  active: number;
  started: number;
  completed: number;
  failed: number;
  duration_ns: number;
  last_event: LedgerEvent | null;
  output: LedgerEvent | null;
};
export type Run = {
  id: string;
  mode: string;
  input_name: string;
  status: string;
  revision: number;
  created_unix_ns: string;
  finished_unix_ns?: string;
  elapsed_ns: number;
  cursor: number;
  groups: Record<string, Group>;
  recent: LedgerEvent[];
  history: { seq: number; elapsed_ns: number; blocks: number; bytes: number }[];
  counters: Record<string, number | null>;
  artifact_count: number;
  engine_result: LedgerEvent | null;
  engine_run_id: string | null;
  engine_connection?: string;
  input?: LedgerEvent;
  message: string;
  diagnostics: string[];
  runner_sha256: string;
  verification: null | {
    source: string;
    status: string;
    verified_responses: number;
    payload_bytes: number;
    first_connection_responses: number;
    reconnects_on_first_connection: number;
  };
};
export type Config = {
  ready: boolean;
  error: string | null;
  groups: CatalogGroup[];
  server_available: boolean;
  profile: string;
  max_upload_bytes: number;
};
export type RunSummary = Pick<
  Run,
  "id" | "input_name" | "mode" | "status" | "cursor" | "created_unix_ns"
>;
export const terminal = (status: string) =>
  ["completed", "failed", "cancelled", "interrupted"].includes(status);
export function blankGroup(group: CatalogGroup): Group {
  return {
    ...group,
    count: 0,
    active: 0,
    started: 0,
    completed: 0,
    failed: 0,
    duration_ns: 0,
    last_event: null,
    output: null,
  };
}
export function groupState(group: Group, status?: string): string {
  if (group.failed) return "Failed";
  if (group.active)
    return status && terminal(status) ? "Interrupted" : "Active";
  if (group.completed) return "Completed";
  return group.count ? "Observed" : "Not exercised";
}
export function applyLedger(state: Run, event: LedgerEvent): Run {
  if (
    !Number.isSafeInteger(event.seq) ||
    typeof event.timestamp_unix_ns !== "string" ||
    !state.groups[event.group]
  )
    throw new Error("Invalid engine event");
  if (event.seq <= state.cursor) return state;
  if (event.seq !== state.cursor + 1)
    throw new Error("Event sequence gap; recovering the journal");
  if (state.engine_run_id && state.engine_run_id !== event.run_id)
    throw new Error("Engine run identity changed");
  const group = {
    ...state.groups[event.group],
    count: state.groups[event.group].count + 1,
    last_event: event,
  };
  const kind = event.type,
    status = event.status ?? event.state;
  const start =
    (kind === "stage" && status === "started") || kind === "syscall_started";
  const end =
    (kind === "stage" && ["completed", "failed"].includes(status)) ||
    ["syscall_completed", "syscall_failed"].includes(kind);
  if (start) {
    group.started++;
    group.active++;
  }
  if (end) {
    group.active = Math.max(0, group.active - 1);
    group.completed += Number(status !== "failed" && kind !== "syscall_failed");
    group.duration_ns += event.elapsed_ns ?? 0;
  }
  if (status === "failed" || kind === "syscall_failed") group.failed++;
  const preferred: Record<string, string> = {
    decode: "harvest",
    ingest: "input",
    cache: "compilation_progress",
    artifacts: "artifact",
  };
  if (
    !start &&
    (!group.output ||
      !preferred[event.group] ||
      preferred[event.group] === kind ||
      group.output.type !== preferred[event.group])
  )
    group.output = event;
  const next: Run = {
    ...state,
    cursor: event.seq,
    elapsed_ns: event.run_elapsed_ns,
    engine_run_id: event.run_id,
    groups: { ...state.groups, [event.group]: group },
    recent: [...state.recent, event].slice(-120),
  };
  if (kind === "input") next.input = event;
  if (
    ["progress", "compilation_progress", "execution", "result"].includes(
      kind,
    ) &&
    (event.counters || event.report)
  )
    next.counters = event.counters ?? event.report;
  if (kind === "compilation_progress")
    next.history = [
      ...state.history,
      {
        seq: event.seq,
        elapsed_ns: event.run_elapsed_ns,
        blocks: event.counters.compiled_blocks,
        bytes: event.counters.unique_lifted_bytes,
      },
    ].slice(-180);
  if (kind === "artifact") next.artifact_count++;
  if (kind === "result") next.engine_result = event;
  return next;
}
export const number = (value: unknown) =>
  typeof value === "number" ? value.toLocaleString("en-US") : "—";
export function bytes(value: unknown) {
  if (typeof value !== "number") return "—";
  return value < 1024
    ? `${number(value)} B`
    : value < 1048576
      ? `${(value / 1024).toFixed(1)} KiB`
      : `${(value / 1048576).toFixed(2)} MiB`;
}
export function duration(value: unknown) {
  if (typeof value !== "number") return "—";
  return value < 1e6
    ? `${(value / 1e3).toFixed(1)} µs`
    : value < 1e9
      ? `${(value / 1e6).toFixed(1)} ms`
      : `${(value / 1e9).toFixed(2)} s`;
}
export function timestamp(value: string) {
  try {
    return new Date(Number(BigInt(value) / 1000000n))
      .toISOString()
      .slice(11, 23);
  } catch {
    return "—";
  }
}
