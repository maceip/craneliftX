import React, { useCallback, useRef, useState } from "react";
import { Button } from "./components/ui/button";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "./components/ui/card";
import { Badge } from "./components/ui/badge";
import { Spinner } from "./components/ui/spinner";
import { Separator } from "./components/ui/separator";

/** Real tracer + lift-results rows. Nulls mean "not measured" — never invent. */
interface FunctionRow {
  name: string;
  addr: number;
  size_bytes: number;
  n_insn: number;
  signature: string;
  loop_density: number;
  call_fraction: number;
  network_score: number;
  decision: "LIFT" | "KEEP_NATIVE";
  reason: string;
  src_size: number;
  src_insn: number;
  wasm_size: number | null;
  runner_ms: number | null;
  expected: number | null;
  observed: number | null;
  expected_outs: number[] | null;
  passed: boolean | null;
  pulley_log: string | null;
  qemu_log: string | null;
  lifted_measured: boolean;
}

interface Placement {
  mode: string;
  selected: string[];
  held_native?: string[];
  eligible?: string[];
}

const fmt = (n: number | null | undefined, unit = "") =>
  n === null || n === undefined ? "—" : `${n}${unit}`;

const fmtMs = (n: number | null | undefined) =>
  n === null || n === undefined ? "—" : `${n.toFixed(2)} ms`;

export const App: React.FC = () => {
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [binaryName, setBinaryName] = useState<string | null>(null);
  const [rows, setRows] = useState<FunctionRow[]>([]);
  const [placement, setPlacement] = useState<Placement | null>(null);
  const [measuredCount, setMeasuredCount] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const fileRef = useRef<HTMLInputElement>(null);

  const pushLog = useCallback((line: string) => {
    setLog((prev) => [...prev.slice(-80), line]);
  }, []);

  const run = useCallback(
    async (file?: File) => {
      setRunning(true);
      setError(null);
      setRows([]);
      setPlacement(null);
      setMeasuredCount(0);
      setSelected(null);
      setLog([]);

      try {
        let id: string;
        let name: string;
        if (file) {
          const res = await fetch("/api/upload", {
            method: "POST",
            headers: { "X-Filename": file.name },
            body: file,
          });
          const data = await res.json();
          if (!res.ok) throw new Error(data.error || "upload failed");
          id = data.id;
          name = data.name;
        } else {
          const res = await fetch("/api/sample", { method: "POST" });
          const data = await res.json();
          if (!res.ok) throw new Error(data.error || "sample failed");
          id = data.id;
          name = data.name;
        }
        setBinaryName(name);
        pushLog(`job ${id} · ${name}`);

        const es = new EventSource(`/api/events/${id}`);
        es.onmessage = (ev) => {
          try {
            const msg = JSON.parse(ev.data);
            const { stage, payload } = msg;
            if (stage === "function_row") {
              setRows((prev) => [...prev, payload as FunctionRow]);
            } else if (stage === "placement") {
              setPlacement(payload as Placement);
              pushLog(
                `placement mode=${payload.mode} selected=${(payload.selected || []).length}`
              );
            } else if (stage === "memory_map") {
              pushLog(`memory map · ${payload.functions?.length ?? 0} functions`);
            } else if (stage === "perf") {
              setMeasuredCount(payload.measured_count ?? 0);
              pushLog(
                `perf · ${payload.measured_count ?? 0} lifted functions measured`
              );
            } else if (stage === "done") {
              setMeasuredCount(payload.measured_count ?? 0);
              pushLog("analysis complete");
              setRunning(false);
              es.close();
            } else if (stage === "error") {
              setError(payload.message || "analysis failed");
              setRunning(false);
              es.close();
            } else {
              pushLog(stage);
            }
          } catch {
            /* ignore malformed SSE frame */
          }
        };
        es.onerror = () => {
          es.close();
          setRunning(false);
          setError("event stream closed");
        };
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setRunning(false);
      }
    },
    [pushLog]
  );

  const span = rows.reduce((m, r) => Math.max(m, r.addr + r.size_bytes), 1);
  const active = rows.find((r) => r.name === selected) || null;

  return (
    <div className="min-h-screen bg-[#000000] text-[#f4f4f5] p-6 sm:p-10 selection:bg-[#fc4c01]/20">
      <div className="mx-auto max-w-5xl flex flex-col gap-8">
        {/* Header */}
        <header className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="size-8 rounded-full border border-white/10 grid place-items-center text-[#fc4c01] text-xs font-bold">
              CX
            </div>
            <div>
              <div className="text-sm font-semibold tracking-tight">CraneliftX</div>
              <div className="text-xs text-neutral-500 font-mono">
                {binaryName || "no binary loaded"}
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {running ? (
              <Badge variant="accent">
                <Spinner size="sm" /> running
              </Badge>
            ) : measuredCount > 0 ? (
              <Badge variant="outline">{measuredCount} lifts measured</Badge>
            ) : rows.length > 0 ? (
              <Badge variant="outline">analysis only</Badge>
            ) : (
              <Badge variant="outline">idle</Badge>
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={() => fileRef.current?.click()}
            >
              Upload
            </Button>
            <Button
              variant="accent"
              size="sm"
              disabled={running}
              onClick={() => run()}
            >
              {running ? <Spinner size="sm" /> : null}
              Run sample
            </Button>
            <input
              ref={fileRef}
              type="file"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) run(f);
                e.target.value = "";
              }}
            />
          </div>
        </header>

        <p className="text-sm text-neutral-400 max-w-2xl">
          Drop a native object to see where functions sit in the binary and which
          the tracer selects to lift onto Pulley. Metrics are Capstone static
          analysis; execution numbers come from{" "}
          <code className="text-neutral-300">make demo</code> lift results only.
          Fields that were not measured show as —.
        </p>

        {error && (
          <div className="border border-red-500/30 bg-red-500/10 text-red-200 text-sm rounded-lg px-4 py-3">
            {error}
          </div>
        )}

        {/* Memory map */}
        {rows.length > 0 && (
          <Card className="bg-[#121212] border-white/[0.08]">
            <CardHeader>
              <CardTitle className="text-sm">Memory map · lift placement</CardTitle>
              <CardDescription className="text-xs">
                Function extents in the object file. Orange = lifted to Pulley,
                gray = kept native. Width ∝ size_bytes.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-3">
              <div className="relative h-12 w-full rounded-lg border border-white/[0.06] bg-black/40 overflow-hidden">
                {rows.map((r) => {
                  const left = (r.addr / span) * 100;
                  const width = Math.max((r.size_bytes / span) * 100, 0.6);
                  const lift = r.decision === "LIFT";
                  return (
                    <button
                      key={r.name}
                      type="button"
                      title={`${r.name} @ 0x${r.addr.toString(16)} +${r.size_bytes}B`}
                      onClick={() => setSelected(r.name)}
                      className={`absolute top-0 h-full border-r border-black/40 transition-opacity ${
                        lift ? "bg-[#fc4c01]/80" : "bg-neutral-700/80"
                      } ${selected === r.name ? "opacity-100 ring-1 ring-white" : "opacity-90 hover:opacity-100"}`}
                      style={{ left: `${left}%`, width: `${width}%` }}
                    />
                  );
                })}
              </div>
              <div className="flex flex-wrap gap-4 text-[11px] text-neutral-400 font-mono">
                <span>
                  <span className="inline-block size-2 rounded-sm bg-[#fc4c01]/80 mr-1" />
                  LIFT
                </span>
                <span>
                  <span className="inline-block size-2 rounded-sm bg-neutral-700 mr-1" />
                  KEEP_NATIVE
                </span>
                <span>span 0x0 – 0x{span.toString(16)} ({span} B)</span>
                {placement && (
                  <span>
                    mode {placement.mode} · selected{" "}
                    {(placement.selected || []).join(", ") || "—"}
                  </span>
                )}
              </div>
              {active && (
                <div className="rounded-lg border border-white/[0.06] bg-black/30 p-3 text-xs font-mono text-neutral-300 space-y-1">
                  <div className="text-white text-sm font-semibold not-italic">
                    {active.name}{" "}
                    <Badge variant={active.decision === "LIFT" ? "accent" : "outline"}>
                      {active.decision}
                    </Badge>
                  </div>
                  <div>
                    0x{active.addr.toString(16)} · {active.size_bytes} B ·{" "}
                    {active.n_insn} insn · {active.signature}
                  </div>
                  <div>
                    loop {active.loop_density.toFixed(3)} · call{" "}
                    {active.call_fraction.toFixed(3)} · net{" "}
                    {active.network_score.toFixed(2)}
                  </div>
                  <div className="text-neutral-400">{active.reason}</div>
                </div>
              )}
            </CardContent>
          </Card>
        )}

        {/* Source vs destination */}
        {rows.length > 0 && (
          <Card className="bg-[#121212] border-white/[0.08]">
            <CardHeader>
              <CardTitle className="text-sm">Source vs destination</CardTitle>
              <CardDescription className="text-xs">
                Source = x86_64 machine-code window handed to the lifter.
                Destination = Pulley wasm artifact. runner_ms is wall time of the
                Pulley runner process from make demo — not a cycle estimate.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div className="overflow-x-auto">
                <table className="w-full text-xs font-mono">
                  <thead>
                    <tr className="text-neutral-500 text-left border-b border-white/[0.06]">
                      <th className="py-2 pr-3">function</th>
                      <th className="py-2 pr-3">decision</th>
                      <th className="py-2 pr-3">src B</th>
                      <th className="py-2 pr-3">src insn</th>
                      <th className="py-2 pr-3">wasm B</th>
                      <th className="py-2 pr-3">runner</th>
                      <th className="py-2 pr-3">expected</th>
                      <th className="py-2 pr-3">observed</th>
                      <th className="py-2">result</th>
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((r) => (
                      <tr
                        key={r.name}
                        className="border-b border-white/[0.04] hover:bg-white/[0.03] cursor-pointer"
                        onClick={() => setSelected(r.name)}
                      >
                        <td className="py-2 pr-3 text-neutral-200">{r.name}</td>
                        <td className="py-2 pr-3">
                          <span
                            className={
                              r.decision === "LIFT" ? "text-[#fc4c01]" : "text-neutral-500"
                            }
                          >
                            {r.decision}
                          </span>
                        </td>
                        <td className="py-2 pr-3">{fmt(r.src_size)}</td>
                        <td className="py-2 pr-3">{fmt(r.src_insn)}</td>
                        <td className="py-2 pr-3">{fmt(r.wasm_size)}</td>
                        <td className="py-2 pr-3">{fmtMs(r.runner_ms)}</td>
                        <td className="py-2 pr-3">{fmt(r.expected)}</td>
                        <td className="py-2 pr-3">{fmt(r.observed)}</td>
                        <td className="py-2">
                          {r.passed === true ? (
                            <span className="text-emerald-400">PASS</span>
                          ) : r.passed === false ? (
                            <span className="text-red-400">FAIL</span>
                          ) : (
                            <span className="text-neutral-600">not run</span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {active?.pulley_log && (
                <>
                  <Separator className="my-4 bg-white/[0.06]" />
                  <div className="text-[11px] text-neutral-500 mb-2 font-mono">
                    Pulley runner output · {active.name}
                  </div>
                  <pre className="text-[11px] font-mono text-neutral-300 bg-black/40 rounded-lg p-3 overflow-x-auto whitespace-pre-wrap">
                    {active.pulley_log}
                  </pre>
                </>
              )}
              {active && !active.lifted_measured && (
                <>
                  <Separator className="my-4 bg-white/[0.06]" />
                  <div className="text-xs text-neutral-400">
                    No lift measurement for this function. Run{" "}
                    <code className="text-neutral-300">make demo</code> to populate{" "}
                    <code className="text-neutral-300">lift_results.json</code>
                    {active.decision === "KEEP_NATIVE"
                      ? " — this function is kept native by the tracer."
                      : "."}
                  </div>
                </>
              )}
            </CardContent>
          </Card>
        )}

        {/* Log */}
        {log.length > 0 && (
          <Card className="bg-[#121212] border-white/[0.08]">
            <CardHeader>
              <CardTitle className="text-sm">Event log</CardTitle>
            </CardHeader>
            <CardContent>
              <pre className="text-[11px] font-mono text-neutral-400 space-y-0.5 max-h-48 overflow-y-auto">
                {log.map((l, i) => (
                  <div key={i}>{l}</div>
                ))}
              </pre>
            </CardContent>
          </Card>
        )}

        <footer className="text-[11px] text-neutral-600 font-mono pb-8">
          local demo · no auth · data stays on this machine · no synthetic metrics
        </footer>
      </div>
    </div>
  );
};
