import React, { useCallback, useRef, useState } from "react";
import {
  Button,
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  Badge,
  Separator,
  Avatar,
  AvatarFallback,
} from "@graundtech/fluent2-react-kit";
import {
  PerformanceMetricsView,
  MetricItem,
} from "./components/PerformanceMetricsView";
import {
  VirtualMemoryMap,
  MemorySegment,
} from "./components/VirtualMemoryMap";

/** Real tracer + lift_results rows. Null = not measured. */
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
  riscv_size: number | null;
  riscv_passed: boolean | null;
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
}

const fmt = (n: number | null | undefined, unit = "") =>
  n === null || n === undefined ? "—" : `${n}${unit}`;

const fmtMs = (n: number | null | undefined) =>
  n === null || n === undefined ? "—" : `${n.toFixed(2)} ms`;

const STAGE_LABEL: Record<string, string> = {
  start: "job started",
  load: "binary loaded",
  disassemble: "disassembled",
  function: "function metrics",
  callgraph: "call graph",
  decision: "lift decisions",
  placement: "placement",
  function_row: "row",
  memory_map: "memory map",
  perf: "measurements",
  tracer_complete: "tracer finished",
  complete: "complete",
  error: "error",
};

export const App: React.FC = () => {
  const [isRunning, setIsRunning] = useState(false);
  const [stageLabel, setStageLabel] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [binaryName, setBinaryName] = useState<string>("sample_network.o");
  const [byteCount, setByteCount] = useState<number | null>(null);
  const [rows, setRows] = useState<FunctionRow[]>([]);
  const [placement, setPlacement] = useState<Placement | null>(null);
  const [measuredCount, setMeasuredCount] = useState(0);
  const [selectedFunctionName, setSelectedFunctionName] = useState<string | null>(
    null
  );
  const [selectedSample, setSelectedSample] = useState<"sample_network" | "sample_basic">("sample_network");
  const fileInputRef = useRef<HTMLInputElement>(null);

  const startJob = useCallback(async (file?: File, sampleId?: "sample_network" | "sample_basic") => {
    setIsRunning(true);
    setError(null);
    setRows([]);
    setPlacement(null);
    setMeasuredCount(0);
    setSelectedFunctionName(null);
    setStageLabel("starting");

    const targetSample = sampleId || selectedSample;
    if (sampleId) {
      setSelectedSample(sampleId);
    }

    try {
      let id: string;
      let name: string;
      let bytes: number | undefined;
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
        bytes = data.bytes;
      } else {
        const res = await fetch(`/api/sample?sample=${targetSample}`, { method: "POST" });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || "sample failed");
        id = data.id;
        name = data.name;
        bytes = data.bytes;
      }
      setBinaryName(name);
      if (typeof bytes === "number") setByteCount(bytes);

      const es = new EventSource(`/api/events/${id}`);
      es.onmessage = (ev) => {
        try {
          const { stage, payload } = JSON.parse(ev.data);
          setStageLabel(STAGE_LABEL[stage] || stage);
          if (stage === "function_row") {
            setRows((prev) => [...prev, payload as FunctionRow]);
          } else if (stage === "placement") {
            setPlacement(payload as Placement);
          } else if (stage === "perf") {
            setMeasuredCount(payload.measured_count ?? 0);
          } else if (stage === "complete") {
            setMeasuredCount(payload.measured_count ?? 0);
            setIsRunning(false);
            setStageLabel("complete");
            es.close();
          } else if (stage === "error") {
            setError(payload.message || "analysis failed");
            setIsRunning(false);
            setStageLabel("error");
            es.close();
          }
        } catch {
          /* ignore malformed frame */
        }
      };
      es.onerror = () => {
        es.close();
        setIsRunning(false);
      };
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setIsRunning(false);
      setStageLabel("error");
    }
  }, []);

  const sortedFunctions = [...rows].sort((a, b) => a.addr - b.addr);
  const totalAddressSpan =
    sortedFunctions.length > 0
      ? Math.max(...sortedFunctions.map((f) => f.addr + f.size_bytes))
      : 0;

  const activeFunction = selectedFunctionName
    ? sortedFunctions.find((f) => f.name === selectedFunctionName)
    : sortedFunctions.find((f) => f.decision === "LIFT") || sortedFunctions[0];

  const liftedCount = sortedFunctions.filter((f) => f.decision === "LIFT").length;
  const keptCount = sortedFunctions.filter(
    (f) => f.decision === "KEEP_NATIVE"
  ).length;

  // Real bytes + runner wall time only — never invent ns/cycles.
  const metricItems: MetricItem[] = sortedFunctions.map((fn) => ({
    name: fn.name,
    addr: fn.addr,
    srcSize: fn.src_size ?? fn.size_bytes,
    wasmSize: fn.wasm_size,
    riscvSize: fn.riscv_size,
    runnerMs: fn.runner_ms,
    isLifted: fn.decision === "LIFT",
    measured: fn.lifted_measured,
  }));

  const memorySegments: MemorySegment[] = sortedFunctions.map((fn) => ({
    name: fn.name,
    addr: fn.addr,
    size_bytes: fn.size_bytes,
    decision: fn.decision,
    reason: fn.reason,
    n_insn: fn.n_insn,
  }));

  return (
    <div className="min-h-screen bg-[#F5F5F7] text-[#1c1c1e] flex flex-col items-center py-8 sm:py-12 md:py-16 px-4 sm:px-6">
      <div className="w-full max-w-5xl flex flex-col gap-10 md:gap-12">
        {/* Topbar */}
        <header className="flex items-center justify-between rounded-full border border-black/[0.06] bg-white/90 backdrop-blur-md px-5 py-2.5 shadow-sm">
          <div className="flex items-center gap-3">
            <Avatar className="size-8">
              <AvatarFallback className="bg-neutral-900 text-white font-bold text-xs">
                M-ISA
              </AvatarFallback>
            </Avatar>
            <div className="flex items-center gap-2">
              <span className="font-semibold text-sm tracking-tight text-neutral-900">
                HeteroSec Drop
              </span>
              <span className="size-1.5 rounded-full bg-[#0284c7]" />
              <span className="text-xs text-neutral-500 font-mono">
                {binaryName} (ELF x86_64)
              </span>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Badge
              variant="outline"
              className="text-[11px] font-mono border-black/10 text-neutral-600"
            >
              {isRunning
                ? stageLabel || "working"
                : rows.length
                  ? `${rows.length} fn · ${measuredCount} lowered`
                  : "idle"}
            </Badge>
            <Badge
              variant="outline"
              className="text-[11px] font-mono border-[#0284c7]/20 text-[#0284c7] bg-[#0284c7]/5 hidden sm:inline-flex"
            >
              Targets: RISC-V 64 + Pulley
            </Badge>
            <Button
              size="sm"
              className="text-xs bg-[#0284c7] text-white hover:bg-[#0369a1]"
              disabled={isRunning}
              onClick={() => startJob()}
            >
              {isRunning ? "Lowering…" : "Run Multi-ISA Demo"}
            </Button>
          </div>
        </header>

        {/* Hero */}
        <section className="flex flex-col items-center text-center gap-3 py-2">
          <Badge
            variant="outline"
            className="text-[#0284c7] border-[#0284c7]/30 bg-[#0284c7]/10"
          >
            Multi-ISA Lowering
          </Badge>
          <h1 className="text-3xl sm:text-4xl md:text-5xl font-bold tracking-tight text-neutral-900 max-w-2xl leading-tight">
            Lower x86_64 to RISC-V 64 &amp; Pulley
          </h1>
          <p className="text-neutral-500 text-sm sm:text-base max-w-lg font-normal leading-relaxed">
            Lift x86_64 machine code via Remill &amp; Anvill into LLVM IR, then
            compile to 64-bit RISC-V ELF and Pulley bytecode. RISC-V runs under
            qemu-user only when that toolchain is installed; otherwise the UI
            reports compile-only.
          </p>

          {/* Sample Selection: Scaled Difficulty */}
          <div className="flex flex-col sm:flex-row items-center justify-center gap-2 pt-2">
            <span className="text-[10px] font-semibold text-neutral-400 uppercase tracking-wider">
              Demonstration Binary:
            </span>
            <div className="inline-flex rounded-xl p-1 bg-black/[0.04] border border-black/[0.06] gap-1">
              <button
                disabled={isRunning}
                onClick={() => startJob(undefined, "sample_basic")}
                className={`px-3 py-1.5 text-xs font-medium rounded-lg transition-all ${
                  selectedSample === "sample_basic"
                    ? "bg-white text-neutral-900 shadow-sm border border-black/5"
                    : "text-neutral-600 hover:text-neutral-900"
                }`}
              >
                Level 1: Basic ALU (sample_basic.o)
              </button>
              <button
                disabled={isRunning}
                onClick={() => startJob(undefined, "sample_network")}
                className={`px-3 py-1.5 text-xs font-medium rounded-lg transition-all ${
                  selectedSample === "sample_network"
                    ? "bg-white text-neutral-900 shadow-sm border border-black/5"
                    : "text-neutral-600 hover:text-neutral-900"
                }`}
              >
                Level 2: Network Data-Plane (sample_network.o)
              </button>
            </div>
          </div>

          {/* Architecture & Capability Frontier Note */}
          <div className="w-full max-w-2xl rounded-2xl border border-black/[0.06] bg-[#F5F5F7] p-3 text-left mt-1 text-xs text-neutral-600">
            <div className="flex items-center justify-between mb-1">
              <span className="font-semibold text-neutral-900 text-xs flex items-center gap-1.5">
                <span className="size-2 rounded-full bg-[#0284c7]" />
                Capability Frontier: Leaf Functions vs. Network Servers
              </span>
              <Badge variant="outline" className="text-[10px] font-mono text-neutral-500">
                Placement Model
              </Badge>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-3 text-[11px] leading-relaxed mt-1 text-neutral-600">
              <div>
                <b className="text-neutral-800">What is lowered:</b> Self-contained leaf routines (Level 1 ALU) and untrusted data-plane packet/protocol parsers (Level 2).
              </div>
              <div>
                <b className="text-neutral-800">Why not Caddy directly?</b> Caddy is a 43MB Go binary with goroutine runtimes and epoll loops. Pulley has no OS kernel. The heterogeneous model keeps socket loops native and isolates parsers.
              </div>
            </div>
          </div>
        </section>

        {error && (
          <div className="border border-red-200 bg-red-50 text-red-700 text-sm rounded-2xl px-4 py-3">
            {error}
          </div>
        )}

        {/* Real status line — no fake % bar */}
        {(isRunning || stageLabel) && (
          <div className="text-[11px] font-mono text-neutral-400 -my-4">
            status: {stageLabel || "—"}
            {byteCount != null && <span> · {byteCount} byte file</span>}
            {placement && (
              <span>
                {" "}
                · placement {placement.mode} ·{" "}
                {(placement.selected || []).join(", ") || "—"}
              </span>
            )}
          </div>
        )}

        {/* Virtual memory map (real extents) */}
        <VirtualMemoryMap
          segments={memorySegments}
          selectedFunction={selectedFunctionName}
          onSelectFunction={setSelectedFunctionName}
          isLoading={isRunning}
        />

        {/* Address space tape */}
        {sortedFunctions.length > 0 && (
          <Card className="bg-white border border-black/[0.08] rounded-2xl p-6 sm:p-8 shadow-sm">
            <CardHeader className="p-0 pb-5 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
              <div>
                <CardTitle className="text-base font-semibold text-neutral-900">
                  Function layout (0x0000 — 0x
                  {totalAddressSpan.toString(16).toUpperCase().padStart(4, "0")})
                </CardTitle>
                <CardDescription className="text-xs text-neutral-500">
                  Addresses from disassembly. Bar width is size in bytes.
                </CardDescription>
              </div>
              <div className="flex items-center gap-4 text-xs font-medium">
                <span className="flex items-center gap-1.5">
                  <span className="size-2.5 rounded-full bg-[#FC4C01]" />
                  Lifted ({liftedCount})
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="size-2.5 rounded-full bg-neutral-300" />
                  Kept native ({keptCount})
                </span>
              </div>
            </CardHeader>

            <CardContent className="p-0 space-y-4">
              <div className="w-full bg-[#F5F5F7] p-2 rounded-xl border border-black/[0.04]">
                <div className="w-full h-12 flex rounded-lg overflow-hidden gap-1">
                  {sortedFunctions.map((fn) => {
                    const widthPercent = Math.max(
                      6,
                      (fn.size_bytes / Math.max(totalAddressSpan, 1)) * 100
                    );
                    const isLifted = fn.decision === "LIFT";
                    const isSelected = activeFunction?.name === fn.name;
                    return (
                      <button
                        key={fn.name}
                        onClick={() => setSelectedFunctionName(fn.name)}
                        style={{ width: `${widthPercent}%` }}
                        title={`${fn.name} @ 0x${fn.addr.toString(16)} (${fn.size_bytes}B) → ${fn.decision}`}
                        className={`relative h-full flex flex-col items-center justify-center select-none cursor-pointer rounded px-1 ${
                          isLifted
                            ? isSelected
                              ? "bg-[#FC4C01] text-white shadow-md ring-2 ring-[#FC4C01] ring-offset-1"
                              : "bg-[#FC4C01]/85 text-white hover:bg-[#FC4C01]"
                            : isSelected
                              ? "bg-neutral-800 text-white shadow-md ring-2 ring-neutral-700 ring-offset-1"
                              : "bg-neutral-200 text-neutral-700 hover:bg-neutral-300"
                        }`}
                      >
                        <span className="text-[11px] font-mono font-semibold truncate w-full text-center">
                          {fn.name.replace("_", "")}
                        </span>
                        <span className="text-[9px] font-mono opacity-80 truncate">
                          0x{fn.addr.toString(16).padStart(2, "0")}
                        </span>
                      </button>
                    );
                  })}
                </div>
                <div className="flex justify-between text-[10px] font-mono text-neutral-400 mt-2 px-1">
                  <span>Offset 0x0000</span>
                  <span>
                    {byteCount != null ? `${byteCount} byte file` : "code span"} ·{" "}
                    {totalAddressSpan} B mapped
                  </span>
                  <span>
                    Offset 0x{totalAddressSpan.toString(16).toUpperCase()}
                  </span>
                </div>
              </div>
            </CardContent>
          </Card>
        )}

        {/* Performance — measured bytes + runner wall time */}
        <PerformanceMetricsView
          metrics={metricItems}
          selectedFunction={selectedFunctionName}
          onSelectFunction={setSelectedFunctionName}
        />

        {/* Active function detail — real fields only */}
        {activeFunction && (
          <Card className="bg-white border border-black/[0.08] rounded-2xl p-6 shadow-sm">
            <CardHeader className="p-0 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
              <div>
                <div className="flex items-center gap-2 mb-1">
                  <span className="font-mono text-sm font-bold text-neutral-900">
                    {activeFunction.name}
                  </span>
                  <Badge
                    variant={
                      activeFunction.decision === "LIFT" ? "default" : "secondary"
                    }
                  >
                    {activeFunction.decision === "LIFT"
                      ? "LIFT → PULLEY"
                      : "KEEP_NATIVE"}
                  </Badge>
                </div>
                <CardDescription className="text-xs text-neutral-500 font-mono">
                  0x{activeFunction.addr.toString(16).padStart(4, "0")} ·{" "}
                  {activeFunction.size_bytes} B · {activeFunction.n_insn} insn ·{" "}
                  {activeFunction.signature}
                </CardDescription>
              </div>
              <div className="flex items-center gap-4 text-xs font-mono bg-[#F5F5F7] px-3 py-2 rounded-lg">
                <div>
                  <span className="text-neutral-400 block text-[10px]">
                    LOOP
                  </span>
                  <span className="font-semibold text-neutral-800">
                    {activeFunction.loop_density.toFixed(3)}
                  </span>
                </div>
                <Separator orientation="vertical" className="h-6" />
                <div>
                  <span className="text-neutral-400 block text-[10px]">CALL</span>
                  <span className="font-semibold text-neutral-800">
                    {activeFunction.call_fraction.toFixed(3)}
                  </span>
                </div>
                <Separator orientation="vertical" className="h-6" />
                <div>
                  <span className="text-neutral-400 block text-[10px]">
                    RESULT
                  </span>
                  <span className="font-semibold text-neutral-800">
                    {activeFunction.passed === true
                      ? "PASS"
                      : activeFunction.passed === false
                        ? "FAIL"
                        : "not run"}
                  </span>
                </div>
              </div>
            </CardHeader>

            <CardContent className="p-0 pt-6">
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                {/* Source */}
                <div className="rounded-2xl border border-black/[0.06] bg-[#F5F5F7] p-4">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-xs font-semibold text-neutral-800">
                      Source · x86_64 CISC
                    </span>
                    <Badge variant="outline" className="text-[10px] font-mono">
                      {fmt(activeFunction.src_size)} B
                    </Badge>
                  </div>
                  <div className="space-y-1 font-mono text-[11px] text-neutral-600">
                    <div>
                      instructions <b>{fmt(activeFunction.src_insn)}</b>
                    </div>
                    <div>
                      signature <b>{activeFunction.signature}</b>
                    </div>
                    <div>
                      reason
                      <div className="text-neutral-500 mt-0.5 text-[10px]">
                        {activeFunction.reason}
                      </div>
                    </div>
                  </div>
                </div>

                {/* Target 1: RISC-V 64 */}
                <div className="rounded-2xl border border-[#0284c7]/20 bg-[#0284c7]/5 p-4">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-xs font-semibold text-[#0284c7]">
                      Target 1 · RISC-V 64 (QEMU)
                    </span>
                    <Badge
                      variant="outline"
                      className="text-[10px] font-mono text-[#0284c7] border-[#0284c7]/30"
                    >
                      {fmt(activeFunction.riscv_size)} B
                    </Badge>
                  </div>
                  <div className="space-y-1 font-mono text-[11px] text-neutral-700">
                    <div>
                      format <b>ELF 64-bit LSB</b>
                    </div>
                    <div>
                      ABI <b>lp64d (double float)</b>
                    </div>
                    <div>
                      execution{" "}
                      <b>
                        {activeFunction.qemu_log?.includes("QEMU ")
                          ? "QEMU verified"
                          : activeFunction.qemu_log?.includes("SKIP")
                            ? "not run (no qemu)"
                            : activeFunction.qemu_log || "ELF compiled only"}
                      </b>
                    </div>
                  </div>
                </div>

                {/* Target 2: Pulley VM */}
                <div className="rounded-2xl border border-[#FC4C01]/20 bg-[#FC4C01]/5 p-4">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-xs font-semibold text-[#FC4C01]">
                      Target 2 · Pulley64 VM
                    </span>
                    <Badge
                      variant="outline"
                      className="text-[10px] font-mono text-[#FC4C01] border-[#FC4C01]/30"
                    >
                      {fmt(activeFunction.wasm_size)} B
                    </Badge>
                  </div>
                  <div className="space-y-1 font-mono text-[11px] text-neutral-700">
                    <div>
                      run time <b>{fmtMs(activeFunction.runner_ms)}</b>
                    </div>
                    <div>
                      expected <b>{fmt(activeFunction.expected)}</b>
                    </div>
                    <div>
                      observed <b>{fmt(activeFunction.observed)}</b>
                    </div>
                  </div>
                </div>
              </div>

              {/* Multi-ISA Output Logs */}
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mt-6">
                {activeFunction.qemu_log && (
                  <div>
                    <div className="text-[11px] font-mono font-medium text-neutral-400 mb-1.5 flex items-center justify-between">
                      <span>RISC-V 64 QEMU OUTPUT</span>
                      <span className="text-[#0284c7] text-[10px]">qemu-riscv64</span>
                    </div>
                    <pre className="p-3 bg-[#F5F5F7] rounded-xl text-xs font-mono text-neutral-800 overflow-x-auto border border-black/[0.04] whitespace-pre-wrap min-h-[80px]">
                      {activeFunction.qemu_log}
                    </pre>
                  </div>
                )}

                {activeFunction.pulley_log && (
                  <div>
                    <div className="text-[11px] font-mono font-medium text-neutral-400 mb-1.5 flex items-center justify-between">
                      <span>PULLEY INTERPRETER OUTPUT</span>
                      <span className="text-[#FC4C01] text-[10px]">pulley64</span>
                    </div>
                    <pre className="p-3 bg-[#F5F5F7] rounded-xl text-xs font-mono text-neutral-800 overflow-x-auto border border-black/[0.04] whitespace-pre-wrap min-h-[80px]">
                      {activeFunction.pulley_log}
                    </pre>
                  </div>
                )}
              </div>

              {!activeFunction.lifted_measured && (
                <div className="mt-4 text-xs text-neutral-500">
                  {activeFunction.decision === "KEEP_NATIVE"
                    ? "Not lifted — this function stays on the host CPU."
                    : "No run saved. Run `make demo` first."}
                </div>
              )}
            </CardContent>
          </Card>
        )}

        <footer className="flex flex-col sm:flex-row items-center justify-between text-xs text-neutral-400 font-mono pt-4 border-t border-black/[0.05] gap-2">
          <span>CraneliftX · local analysis</span>
          <span>measured numbers only</span>
        </footer>
      </div>
    </div>
  );
};
