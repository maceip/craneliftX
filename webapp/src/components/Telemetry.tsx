import React from "react";
import { number, Run, terminal } from "../pipeline";
import { Scene, sceneStatus } from "../pipelineScenes";
import { Walkthrough } from "../walkthrough";

// A display font, not telemetry: lit pixels spell values from engine receipts.
const digits: Record<string, string[]> = {
  "0": ["01110", "10001", "10011", "10101", "11001", "10001", "01110"],
  "1": ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
  "2": ["01110", "10001", "00001", "00010", "00100", "01000", "11111"],
  "3": ["11110", "00001", "00001", "01110", "00001", "00001", "11110"],
  "4": ["00010", "00110", "01010", "10010", "11111", "00010", "00010"],
  "5": ["11111", "10000", "10000", "11110", "00001", "00001", "11110"],
  "6": ["01110", "10000", "10000", "11110", "10001", "10001", "01110"],
  "7": ["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
  "8": ["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
  "9": ["01110", "10001", "10001", "01111", "00001", "00001", "01110"],
  ".": ["00000", "00000", "00000", "00000", "00000", "00110", "00110"],
  ",": ["00000", "00000", "00000", "00000", "00110", "00110", "00100"],
  "—": ["00000", "00000", "00000", "11111", "00000", "00000", "00000"],
};
export function MatrixNumber({ value }: { value: string }) {
  return (
    <svg
      className="matrix-number"
      viewBox={`0 0 ${value.length * 18 - 3} 21`}
      role="img"
      aria-label={value}
    >
      {Array.from(value).flatMap((char, index) =>
        (digits[char] ?? digits["—"]).flatMap((row, y) =>
          Array.from(row).map((pixel, x) => (
            <circle
              key={`${index}-${y}-${x}`}
              cx={index * 18 + x * 3 + 1.5}
              cy={y * 3 + 1.5}
              r=".95"
              fill="currentColor"
              opacity={pixel === "1" ? 1 : 0.08}
            />
          )),
        ),
      )}
    </svg>
  );
}
export function TelemetryGauge({
  run,
  stage,
  groups,
  clock,
  enabled,
  reduced,
}: {
  run: Run | null;
  stage: string;
  groups: Scene[];
  clock: Walkthrough;
  enabled: boolean;
  reduced: boolean;
}) {
  const elapsed = run ? run.elapsed_ns / 1e9 : null;
  // A one-second sweep makes short engine runs visible. The outer mint ring
  // is explicitly the reader's walkthrough; it never changes engine time.
  const finished = !!run && terminal(run.status);
  const angle = elapsed === null ? null : elapsed * 360;
  const presentation = reduced ? 1 : clock.progress;
  // The map locates the stage being presented, not each sub-millisecond engine
  // transition. Observed dots stay lit when that group opens another span.
  const current = groups.some((group) => group.id === stage) ? stage : null;
  return (
    <div className="telemetry-stack">
      <ol
        className="stage-thermometer"
        aria-label="Timeline stage map"
        data-testid="stage-thermometer"
        data-current-stage={current ?? ""}
        data-mode="inspection"
        data-cursor={run?.cursor ?? 0}
      >
        {groups.map((group) => {
          const state = group.state;
          return (
            <li
              key={group.id}
              data-group={group.id}
              data-state={state}
              data-observed={group.count > 0}
              data-current={current === group.id}
              aria-current={current === group.id ? "step" : undefined}
              title={`${group.order}. ${group.label} · ${sceneStatus(group, run?.status)}`}
              aria-label={`${group.label}: ${sceneStatus(group, run?.status)}${current === group.id ? ", displayed stage" : ""}`}
            >
              <span className="thermometer-dot" />
            </li>
          );
        })}
      </ol>
      <aside
        className="telemetry-gauge"
        aria-label="Recorded engine time"
        data-testid="telemetry-gauge"
        data-elapsed-ns={run?.elapsed_ns}
        data-terminal={run ? terminal(run.status) : false}
        data-clock-mode={finished ? "recorded" : "live"}
        data-walkthrough-progress={presentation.toFixed(3)}
        data-moving={enabled}
      >
        <svg
          className="telemetry-dial"
          viewBox="0 0 160 160"
          aria-hidden="true"
        >
          <circle className="dial-outline" cx="80" cy="80" r="69" />
          <circle
            className="walkthrough-ring"
            cx="80"
            cy="80"
            r="74"
            pathLength="100"
            strokeDasharray={`${presentation * 100} 100`}
            transform="rotate(-90 80 80)"
          />
          {Array.from({ length: 60 }, (_, i) => {
            const a = (i * Math.PI) / 30,
              inner = i % 5 === 0 ? 57 : 62;
            return (
              <line
                key={i}
                x1={80 + Math.sin(a) * inner}
                y1={80 - Math.cos(a) * inner}
                x2={80 + Math.sin(a) * 66}
                y2={80 - Math.cos(a) * 66}
              />
            );
          })}
          {angle !== null && (
            <g
              className="engine-hand"
              style={{ transform: `rotate(${angle}deg)` }}
            >
              <line x1="80" y1="16" x2="80" y2="30" />
              <circle className="dial-position" cx="80" cy="16" r="3" />
            </g>
          )}
        </svg>
        <div className="dial-readout">
          <small>{finished ? "RECORDED TIME" : "ENGINE TIME"}</small>
          <MatrixNumber value={elapsed === null ? "—" : elapsed.toFixed(1)} />
          <span>seconds</span>
        </div>
        <div className="gauge-legend">
          <span className="engine-legend">
            {finished ? "Engine stopped" : "1 turn / second"}
          </span>
          <span className="walkthrough-legend">
            {reduced
              ? "Static view"
              : clock.progress >= 1
                ? "View complete"
                : `View ${groups.findIndex((g) => g.id === stage) + 1} walkthrough`}
          </span>
        </div>
      </aside>
    </div>
  );
}
export function PerformanceSummary({ run }: { run: Run }) {
  const result = run.engine_result!;
  const metrics = [
    {
      label: "Elapsed seconds",
      value: (run.elapsed_ns / 1e9).toFixed(3),
      color: "amber",
      key: "elapsed_ns",
      raw: run.elapsed_ns,
    },
    {
      label: "Target instructions (RV64)",
      value: number(run.counters.rv64_steps),
      color: "mint",
      key: "rv64_steps",
      raw: run.counters.rv64_steps,
    },
    {
      label: "Blocks compiled",
      value: number(run.counters.compiled_blocks),
      color: "coral",
      key: "compiled_blocks",
      raw: run.counters.compiled_blocks,
    },
    {
      label: "Source lifted (x86 instructions)",
      value: number(run.counters.unique_lifted_instructions),
      color: "ice",
      key: "unique_lifted_instructions",
      raw: run.counters.unique_lifted_instructions,
    },
    {
      label: "Source lifted (bytes)",
      value: number(run.counters.unique_lifted_bytes),
      color: "lilac",
      key: "unique_lifted_bytes",
      raw: run.counters.unique_lifted_bytes,
    },
    {
      label: "Cache invalidations",
      value: number(run.counters.cache_invalidations),
      color: "lime",
      key: "cache_invalidations",
      raw: run.counters.cache_invalidations,
    },
  ];
  return (
    <section
      className="performance-summary"
      aria-label="Measured performance"
      data-testid="performance-summary"
      data-source-seq={result.seq}
    >
      <div className="performance-heading">
        <span className="matrix-chip" aria-hidden="true">
          RV64
        </span>
        <div>
          <strong>
            {result.status === "completed"
              ? "Execution Complete"
              : "Execution failed"}
          </strong>
          <small>
            {result.status === "completed"
              ? "Measured by the runtime"
              : result.error}
          </small>
        </div>
      </div>
      <div className="matrix-metrics">
        {metrics.map((metric) => (
          <div
            key={metric.key}
            className={`matrix-metric led-${metric.color}`}
            data-metric={metric.key}
            data-value={metric.raw ?? ""}
          >
            <MatrixNumber value={metric.value} />
            <span>{metric.label}</span>
          </div>
        ))}
      </div>
      {run.verification && (
        <div className="matrix-network">
          <span className={`network-led ${run.verification.status}`} />
          <div>
            <strong>
              {run.verification.status === "passed"
                ? "Persistent connection verified"
                : "Connection verification incomplete"}
            </strong>
            <small>
              {run.verification.verified_responses} responses checked ·{" "}
              {run.verification.first_connection_responses} on the first
              connection · {run.verification.reconnects_on_first_connection}{" "}
              reconnects
            </small>
          </div>
        </div>
      )}
    </section>
  );
}
