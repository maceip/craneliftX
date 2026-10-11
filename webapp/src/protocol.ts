export type Tier = "baby" | "medium" | "large";
export type Status =
  | "running"
  | "cancelling"
  | "completed"
  | "cancelled"
  | "failed"
  | "interrupted";
export type EngineEvent = {
  v: 1;
  type: string;
  run_id: string;
  seq: number;
  time: number;
  [key: string]: unknown;
};
export interface Snapshot {
  id: string;
  tier: Tier;
  seed: string;
  status: Status;
  cursor: number;
  created: number;
  message?: string;
  worker_sha256: string;
  requested: { bytes: number; connections: number; kernels: number };
  stages: Record<string, EngineEvent>;
  artifacts: Record<string, EngineEvent>;
  logs: EngineEvent[];
  progress_history?: { seconds: number; bytes: number }[];
  engine?: EngineEvent;
  progress?: EngineEvent;
  security?: EngineEvent;
  context?: EngineEvent;
  result?: EngineEvent;
}
export const terminal = (status: Status) =>
  ["completed", "cancelled", "failed", "interrupted"].includes(status);
export function applyEvent(state: Snapshot, event: EngineEvent): Snapshot {
  if (
    event.v !== 1 ||
    event.run_id !== state.id ||
    !Number.isSafeInteger(event.seq)
  )
    throw new Error("Invalid engine event");
  if (event.seq <= state.cursor) return state;
  if (event.seq !== state.cursor + 1)
    throw new Error("Event gap; recovering snapshot");
  const next = { ...state, cursor: event.seq };
  switch (event.type) {
    case "started":
      next.engine = event;
      break;
    case "stage":
      next.stages = {
        ...state.stages,
        [`${event.kernel}:${event.stage}`]: event,
      };
      break;
    case "artifact":
      next.artifacts = { ...state.artifacts, [String(event.kernel)]: event };
      break;
    case "progress":
      next.progress = event;
      next.progress_history = [
        ...(state.progress_history ?? []),
        {
          seconds: number(event, "elapsed_ms") / 1000,
          bytes: number(event, "bytes"),
        },
      ].slice(-120);
      break;
    case "security":
      next.security = event;
      break;
    case "context":
      next.context = event;
      break;
    case "result":
      next.result = event;
      break;
    case "terminal":
      next.status = event.status as Status;
      next.message = String(event.message ?? "");
      break;
    case "cancelling":
      next.status = "cancelling";
      break;
    case "error":
    case "log":
      next.logs = [...state.logs, event].slice(-80);
      break;
    case "accepted":
      break;
    default:
      throw new Error(`Unknown event: ${event.type}`);
  }
  return next;
}
export const number = (event: EngineEvent | undefined, key: string) =>
  typeof event?.[key] === "number" ? (event[key] as number) : 0;
export const text = (event: EngineEvent | undefined, key: string) =>
  typeof event?.[key] === "string" ? (event[key] as string) : "";
