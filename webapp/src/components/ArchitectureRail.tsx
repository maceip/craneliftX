import React from "react";
import { CatalogGroup, Run } from "../pipeline";

// Diagram positions describe the selected operation, never engine progress.
const positions: Record<string, number> = {
  run: 0,
  ingest: 0,
  load: 0,
  initialize: 0,
  decode: 0.5,
  audit: 1,
  lower: 1.5,
  codegen: 2.5,
  artifacts: 3,
  execute: 3,
  syscall: 3,
  cache: 3,
  result: 3,
};
const nodes = [
  { label: "x86-64", caption: "Source", group: "ingest" },
  { label: "iced-x86", caption: "Decode", group: "decode" },
  { label: "Cranelift IR", caption: "Lower", group: "lower" },
  { label: "RV64", caption: "Machine", group: "codegen" },
];

export function ArchitectureRail({
  run,
  groups,
  stage,
  animate = false,
}: {
  run: Run | null;
  groups: CatalogGroup[];
  stage?: string;
  animate?: boolean;
}) {
  const selected = groups.find((g) => g.id === stage);
  const position = selected ? positions[selected.id] : -1;
  return (
    <div
      role="group"
      className="architecture-rail"
      aria-label="Pipeline architecture"
      data-stage={stage ?? ""}
      data-motion={animate}
    >
      <div className="rail-location">
        <img
          src="/dist/ski-icon.png"
          alt="Ski"
          width={18}
          height={18}
        />
        <span>
          {selected
            ? `VIEW ${String(selected.order).padStart(2, "0")}`
            : "READY"}
          <span className="rail-total"> / {groups.length || "—"}</span>
        </span>
      </div>
      <div className="rail-flow">
        {nodes.map((node, i) => (
          <React.Fragment key={node.group}>
            {i > 0 && (
              <span
                className="rail-link"
                data-current={position === i - 0.5}
                aria-hidden="true"
              >
                <i />
              </span>
            )}
            <div
              className="rail-node"
              data-current={position === i}
              data-observed={(run?.groups[node.group]?.count ?? 0) > 0}
            >
              <small>{node.caption}</small>
              <strong>{node.label}</strong>
            </div>
          </React.Fragment>
        ))}
      </div>
      <div className="rail-time">
        <strong>{run ? `${(run.elapsed_ns / 1e9).toFixed(1)}s` : "—"}</strong>
        <small>ENGINE TIME</small>
      </div>
    </div>
  );
}
