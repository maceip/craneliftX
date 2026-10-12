import { CatalogGroup, Group, groupState, terminal } from "./pipeline";

// Presentation copy only. Group IDs and all stored engine receipts stay intact,
// including journals recorded before these labels were introduced.
const labels: Record<string, string> = {
  run: "Initialize Pipeline",
  ingest: "Read ELF Image",
  load: "Validate and isolate memory layout",
  initialize: "Instantiate isolated guest context",
  decode: "Ingest and decode basic block",
  audit: "Evaluate control-flow & memory invariants",
  lower: "Lift semantics to Cranelift IR",
  codegen: "JIT-compile to RV64 machine code",
  artifacts: "Commit translation artifacts",
  execute: "Execute re-targeted execution stream",
  syscall: "Trap and mediate host syscall",
  cache: "Maintain dynamic code cache",
  result: "Runtime evidence & execution summary",
};
export const stageLabel = (group: CatalogGroup) =>
  labels[group.id] ?? group.label;

const activeLabels: Record<string, string> = {
  ingest: "Reading",
  load: "Mapping",
  initialize: "Initializing",
  decode: "Decoding",
  audit: "Checking",
  lower: "Lifting",
  codegen: "Compiling",
  artifacts: "Committing",
  execute: "Running",
  syscall: "Handling",
  cache: "Active",
};
const recordedLabels: Record<string, string> = {
  run: "Initialized",
  ingest: "Ingested",
  load: "Enforced",
  initialize: "Initialized",
  decode: "Parsed",
  audit: "Verified",
  lower: "Lifted",
  codegen: "Emitted",
  artifacts: "Committed",
  execute: "Executed",
  syscall: "Intercepted",
};
export function stageStatus(group: Group, runStatus?: string): string {
  const state = groupState(group, runStatus);
  if (state === "Active") return activeLabels[group.id] ?? state;
  if (!["Completed", "Observed"].includes(state)) return state;
  if (group.id === "result")
    return group.output?.status === "completed" ? "Execution Complete" : state;
  if (group.id === "cache")
    return runStatus && !terminal(runStatus) ? "Active" : "Recorded";
  return recordedLabels[group.id] ?? state;
}

export const policyLabels: Record<string, string> = {
  privileged_control: "Syscall boundary enforcement",
  stack_policy: "Stack pointer integrity",
  memory_policy: "Memory operand bounds",
  control_flow: "Target branch validation",
};
