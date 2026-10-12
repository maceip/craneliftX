import { blankGroup, CatalogGroup, Group, groupState, Run } from "./pipeline";
import { stageStatus } from "./pipelineCopy";

// The designer's nine reading scenes project the thirteen original trace groups.
// IDs remain the primary engine group so restart requests keep their contract.
// Every trace group belongs to exactly one scene; no ledger data is rewritten.
export const sceneCatalog = [
  { id: "ingest", label: "Ingest binary", sources: ["run", "ingest"] },
  { id: "initialize", label: "Prepare guest", sources: ["load", "initialize"] },
  { id: "decode", label: "Disassemble block", sources: ["decode"] },
  { id: "audit", label: "Verify safety policy", sources: ["audit"] },
  { id: "lower", label: "Lift to Cranelift IR", sources: ["lower"] },
  {
    id: "codegen",
    label: "Compile to RV64",
    sources: ["codegen", "artifacts"],
  },
  { id: "execute", label: "Execute RV64 code", sources: ["execute", "cache"] },
  { id: "syscall", label: "Handle system call", sources: ["syscall"] },
  { id: "result", label: "Execution summary", sources: ["result"] },
] as const;

export type Scene = Group & { sources: Group[]; primary: Group; state: string };
export function sceneForGroup(id: string) {
  return (
    sceneCatalog.find((scene) => scene.sources.some((source) => source === id))
      ?.id ?? "ingest"
  );
}
export function sceneLabel(id: string) {
  return sceneCatalog.find((scene) => scene.id === sceneForGroup(id))!.label;
}

export function pipelineScenes(
  run: Run | null,
  catalog: CatalogGroup[],
): Scene[] {
  if (!catalog.length) return [];
  return sceneCatalog.map((scene, index) => {
    const sources = scene.sources.map((id) => {
      const definition = catalog.find((group) => group.id === id);
      if (!definition) throw new Error(`Missing trace group: ${id}`);
      return run?.groups[id] ?? blankGroup(definition);
    });
    const primary = sources.find((group) => group.id === scene.id)!;
    const sum = (
      key: "count" | "active" | "started" | "completed" | "failed",
    ) => sources.reduce((total, group) => total + group[key], 0);
    const states = sources.map((group) => groupState(group, run?.status));
    const state = states.includes("Failed")
      ? "Failed"
      : states.includes("Interrupted")
        ? "Interrupted"
        : states.includes("Active")
          ? "Active"
          : states.every((value) => ["Completed", "Observed"].includes(value))
            ? groupState(primary, run?.status)
            : sum("count")
              ? "Observed"
              : "Not exercised";
    return {
      ...primary,
      id: scene.id,
      label: scene.label,
      order: index + 1,
      sources,
      primary,
      state,
      count: sum("count"),
      active: sum("active"),
      started: sum("started"),
      completed: sum("completed"),
      failed: sum("failed"),
      // A footer cites the newest constituent receipt; previews still use the
      // primary group so artifact/cache events cannot change the block identity.
      last_event: sources
        .map((group) => group.last_event)
        .sort((a, b) => (b?.seq ?? 0) - (a?.seq ?? 0))[0],
      // Nested span durations cannot be added. Keep the primary duration; cards
      // display counts, while Evidence exposes each original group's span time.
      duration_ns: primary.duration_ns,
    };
  });
}

export function sceneStatus(scene: Scene, runStatus?: string): string {
  if (["Failed", "Interrupted", "Not exercised"].includes(scene.state))
    return scene.state;
  if (scene.id === "initialize") {
    return scene.state === "Completed"
      ? "Context Ready"
      : scene.state === "Active"
        ? "Preparing"
        : "Partially recorded";
  }
  if (
    scene.id === "codegen" &&
    scene.state === "Active" &&
    !scene.primary.active
  )
    return "Saving artifacts";
  if (
    scene.id === "execute" &&
    scene.state === "Active" &&
    !scene.primary.active
  )
    return "Cache active";
  if (scene.id === "decode" && scene.state === "Completed") return "Decoded";
  if (scene.id === "lower" && scene.state === "Completed") return "Lowered";
  if (scene.state === "Observed" && !scene.primary.count) return "Preparing";
  return stageStatus(scene.primary, runStatus);
}
