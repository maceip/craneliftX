export type Stage =
  | "idle"
  | "load"
  | "disassemble"
  | "function"
  | "callgraph"
  | "decision"
  | "placement"
  | "dual_run_start"
  | "dual_step"
  | "dual_verify"
  | "heatmap_data"
  | "done"
  | "error";

export interface FunctionItem {
  name: string;
  addr: number;
  size_bytes: number;
  n_insn: number;
  signature: string;
  loop_density: number;
  call_fraction: number;
  network_score: number;
  frequency_score?: number;
  pure_compute?: boolean;
  lift_score?: number;
  decision?: "LIFT" | "KEEP_NATIVE" | "PENDING";
  reason?: string;
}

export interface NativeRun {
  architecture: string;
  execution_mode: string;
  cycles: number;
  return_value: number;
  out_params?: number[] | null;
  status: string;
  asm_trace: string[];
  latency_ns: number;
}

export interface PulleyRun {
  architecture: string;
  execution_mode: string;
  cycles: number;
  return_value: number;
  out_params?: number[] | null;
  status: string;
  sandbox_boundary: string;
  bytecode_trace: string[];
  latency_ns: number;
  equivalence: boolean;
  delta: number;
}

export interface DualRunStep {
  name: string;
  addr: number;
  size_bytes: number;
  signature: string;
  decision: "LIFT" | "KEEP_NATIVE";
  reason: string;
  args: string;
  native: NativeRun;
  pulley: PulleyRun;
  is_lifted: boolean;
  verified: boolean;
}

export interface HeatMapColumn {
  id: string;
  label: string;
  max: number;
  threshold: number;
}

export interface HeatMapRow {
  name: string;
  addr_hex: string;
  loop_density: number;
  call_fraction: number;
  network_score: number;
  frequency_score: number;
  pure_compute: number;
  lift_score: number;
  n_insn: number;
  size_bytes: number;
  decision: "LIFT" | "KEEP_NATIVE";
  intensity: number;
}

export interface HeatMapPayload {
  columns: HeatMapColumn[];
  rows: HeatMapRow[];
}

export interface PlacementPlan {
  mode: string;
  fraction?: number;
  eligible?: string[];
  selected?: string[];
  held_native?: string[];
  key_fingerprint?: string;
}

export interface LogEntry {
  id: string;
  time: string;
  stage: string;
  text: string;
  isError?: boolean;
}
