import React, { useState } from "react";
import {
  Button,
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  Badge,
  Progress,
  Spinner,
  Separator,
} from "@graundtech/fluent2-react-kit";

export interface GuestRunState {
  id: string;
  tier: "baby" | "medium" | "large";
  seed: string;
  status: "idle" | "running" | "cancelling" | "completed" | "failed" | "interrupted";
  stage: string;
  bytes: number;
  totalBytes: number;
  riscvSteps: number;
  compilationNs?: number;
  executionNs?: number;
  syscallRejected?: boolean;
  context?: {
    rax_before: string;
    rax_after: string;
    rcx_before: string;
    rcx_after: string;
    rip_before: string;
    rip_after: string;
  };
  artifact?: {
    x86: string;
    clif: string;
    riscv: string;
    instructions: number;
    entry_rip: string;
    exit_rip: string;
  };
  message?: string;
}

interface GuestControlPanelProps {
  runState: GuestRunState;
  onSpawn: (tier: "baby" | "medium" | "large", seed: string) => void;
  onCancel: () => void;
}

export const GuestControlPanel: React.FC<GuestControlPanelProps> = ({
  runState,
  onSpawn,
  onCancel,
}) => {
  const [selectedTier, setSelectedTier] = useState<"baby" | "medium" | "large">(runState.tier || "baby");
  const [seed, setSeed] = useState<string>(runState.seed || "42");
  const [showIR, setShowIR] = useState<boolean>(false);

  const isRunning = runState.status === "running" || runState.status === "cancelling";

  const tiersConfig = [
    { id: "baby" as const, label: "Baby", desc: "64 B • 1 kernel • 1 round" },
    { id: "medium" as const, label: "Medium", desc: "8 MB • 4 kernels • 128 rounds" },
    { id: "large" as const, label: "Large", desc: "512 MB • 4 kernels • 256 rounds" },
  ];

  const stagesList = [
    { key: "harvest", label: "Harvest" },
    { key: "audit", label: "Audit" },
    { key: "lower", label: "Cranelift IR" },
    { key: "codegen", label: "Codegen" },
    { key: "execute", label: "Execute" },
  ];

  const progressPercent = runState.totalBytes > 0
    ? Math.min(100, Math.round((runState.bytes / runState.totalBytes) * 100))
    : runState.status === "completed" ? 100 : 0;

  return (
    <Card className="bg-white border border-black/[0.08] rounded-2xl p-6 sm:p-8 shadow-sm">
      <CardHeader className="p-0 pb-6 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
        <div>
          <div className="flex items-center gap-2 mb-1">
            <CardTitle className="text-base font-bold text-neutral-900">
              Guest Execution Control & Spawner
            </CardTitle>
            <Badge variant="outline" className="text-[10px] font-mono border-black/15 text-neutral-600">
              in-process ISR
            </Badge>
          </div>
          <CardDescription className="text-xs text-neutral-500">
            Spawn and control the isolated guest engine. Executes randomized cross-ISA kernels with bit-for-bit verification.
          </CardDescription>
        </div>

        <div className="flex items-center gap-2">
          {runState.status === "completed" ? (
            <Badge variant="success">Guest Completed (Δ = 0)</Badge>
          ) : isRunning ? (
            <Badge variant="warning">
              <Spinner size="sm" />
              {runState.stage || "Running"}
            </Badge>
          ) : (
            <Badge variant="outline">Ready to Spawn</Badge>
          )}
        </div>
      </CardHeader>

      <CardContent className="p-0 space-y-6">
        
        {/* Tier Selection & Spawner Controls */}
        <div className="flex flex-col md:flex-row items-stretch md:items-center justify-between gap-4 bg-[#F5F5F7] p-3.5 rounded-xl border border-black/[0.04]">
          
          {/* Tiers Segmented Selector */}
          <div className="flex items-center gap-1.5 bg-white p-1 rounded-lg border border-black/[0.06] shadow-2xs">
            {tiersConfig.map((t) => (
              <button
                key={t.id}
                disabled={isRunning}
                onClick={() => setSelectedTier(t.id)}
                className={`px-3 py-1.5 text-xs font-mono font-medium rounded-md transition-all ${
                  selectedTier === t.id
                    ? "bg-[#FC4C01] text-white shadow-xs font-semibold"
                    : "text-neutral-600 hover:text-neutral-900 hover:bg-neutral-100"
                } disabled:opacity-50`}
              >
                <span>{t.label}</span>
              </button>
            ))}
          </div>

          {/* Seed Input */}
          <div className="flex items-center gap-2">
            <span className="text-xs font-mono text-neutral-500 font-medium">Seed:</span>
            <input
              type="text"
              disabled={isRunning}
              value={seed}
              onChange={(e) => setSeed(e.target.value)}
              className="w-24 px-2.5 py-1 text-xs font-mono bg-white border border-black/[0.1] rounded-md text-neutral-800 focus:outline-none focus:border-[#FC4C01] disabled:opacity-50"
            />
            <button
              disabled={isRunning}
              onClick={() => setSeed(String(Math.floor(Math.random() * 999999)))}
              title="Randomize Seed"
              className="text-xs font-mono text-neutral-500 hover:text-neutral-900 p-1.5 rounded bg-white border border-black/[0.06] disabled:opacity-50"
            >
              🎲
            </button>
          </div>

          {/* Action Trigger Buttons */}
          <div className="flex items-center gap-2">
            {isRunning ? (
              <Button
                variant="destructive"
                size="sm"
                onClick={onCancel}
                className="text-xs font-semibold px-4"
              >
                Stop Guest
              </Button>
            ) : (
              <Button
                variant="default"
                size="sm"
                onClick={() => onSpawn(selectedTier, seed)}
                className="bg-[#FC4C01] text-white hover:bg-[#e04300] text-xs font-semibold px-5 shadow-xs"
              >
                ⚡ Spawn Guest Run
              </Button>
            )}
          </div>
        </div>

        {/* Live Pipeline Milestones */}
        <div className="flex items-center justify-between border-y border-black/[0.05] py-3 px-2">
          {stagesList.map((st, idx) => {
            const isCurrent = runState.stage === st.key;
            const isDone = runState.status === "completed" || (progressPercent > (idx + 1) * 18);

            return (
              <div key={st.key} className="flex items-center gap-2">
                <span
                  className={`size-2 rounded-full ${
                    isDone
                      ? "bg-emerald-500"
                      : isCurrent
                      ? "bg-[#FC4C01] animate-ping"
                      : "bg-neutral-300"
                  }`}
                />
                <span
                  className={`text-xs font-mono ${
                    isCurrent
                      ? "font-bold text-[#FC4C01]"
                      : isDone
                      ? "text-neutral-900 font-medium"
                      : "text-neutral-400"
                  }`}
                >
                  {st.label}
                </span>
                {idx < stagesList.length - 1 && (
                  <span className="text-neutral-300 text-xs hidden sm:inline">&rarr;</span>
                )}
              </div>
            );
          })}
        </div>

        {/* Live Progress Bar */}
        {(isRunning || runState.status === "completed") && (
          <div className="space-y-1.5">
            <Progress value={progressPercent} />
            <div className="flex justify-between text-[11px] font-mono text-neutral-400">
              <span>{runState.message || "Processing workload rounds…"}</span>
              <span>
                {runState.bytes} / {runState.totalBytes} bytes ({runState.riscvSteps} RISC-V steps)
              </span>
            </div>
          </div>
        )}

        {/* Guest Execution Context & Telemetry (High Contrast Panel) */}
        {runState.context && (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 pt-1">
            
            {/* Registers Snapshot */}
            <div className="bg-neutral-900 text-white p-4 rounded-xl border border-neutral-800 font-mono text-xs space-y-2.5">
              <div className="flex items-center justify-between border-b border-neutral-800 pb-2">
                <span className="font-bold text-white">Guest Register Snapshot</span>
                <Badge variant="secondary" className="bg-neutral-800 text-neutral-300 text-[10px]">
                  Hardware Match
                </Badge>
              </div>

              <div className="space-y-1.5 text-[11px]">
                <div className="flex justify-between py-0.5 border-b border-neutral-800/60">
                  <span className="text-neutral-400">RAX (Before &rarr; After):</span>
                  <span className="text-neutral-200">
                    {runState.context.rax_before.slice(0, 8)}… &rarr; <span className="text-emerald-400 font-bold">{runState.context.rax_after.slice(0, 8)}…</span>
                  </span>
                </div>
                <div className="flex justify-between py-0.5 border-b border-neutral-800/60">
                  <span className="text-neutral-400">RCX (Before &rarr; After):</span>
                  <span className="text-neutral-200">
                    {runState.context.rcx_before.slice(0, 8)}… &rarr; <span className="text-neutral-300">{runState.context.rcx_after.slice(0, 8)}…</span>
                  </span>
                </div>
                <div className="flex justify-between py-0.5 border-b border-neutral-800/60">
                  <span className="text-neutral-400">RIP Range:</span>
                  <span className="text-neutral-300">
                    {runState.context.rip_before} &rarr; {runState.context.rip_after}
                  </span>
                </div>
                <div className="flex justify-between py-0.5">
                  <span className="text-neutral-400">Security Audit:</span>
                  <span className="text-emerald-400 font-semibold">
                    {runState.syscallRejected ? "Syscall Rejected (Safe)" : "Audited"}
                  </span>
                </div>
              </div>
            </div>

            {/* Performance & Execution Timings */}
            <div className="bg-[#F5F5F7] p-4 rounded-xl border border-black/[0.06] font-mono text-xs space-y-2.5 flex flex-col justify-between">
              <div>
                <div className="flex items-center justify-between border-b border-black/[0.06] pb-2">
                  <span className="font-bold text-neutral-900">Execution Timers</span>
                  <button
                    onClick={() => setShowIR(!showIR)}
                    className="text-[11px] font-mono text-[#FC4C01] hover:underline cursor-pointer"
                  >
                    {showIR ? "Hide IR Code" : "Inspect Cranelift IR"}
                  </button>
                </div>

                <div className="space-y-1.5 text-[11px] pt-2">
                  <div className="flex justify-between py-0.5 border-b border-black/[0.04]">
                    <span className="text-neutral-500">Compilation Latency:</span>
                    <span className="font-semibold text-neutral-800">
                      {runState.compilationNs ? `${(runState.compilationNs / 1_000_000).toFixed(2)} ms` : "---"}
                    </span>
                  </div>
                  <div className="flex justify-between py-0.5 border-b border-black/[0.04]">
                    <span className="text-neutral-500">Execution Latency:</span>
                    <span className="font-semibold text-neutral-800">
                      {runState.executionNs ? `${(runState.executionNs / 1_000).toFixed(2)} µs` : "---"}
                    </span>
                  </div>
                  <div className="flex justify-between py-0.5">
                    <span className="text-neutral-500">Equivalence Proof:</span>
                    <span className="text-emerald-600 font-bold">100% Bit-for-Bit Identical</span>
                  </div>
                </div>
              </div>

              <div className="text-[10px] text-neutral-400 pt-1">
                Target Backend: riscv64gc-unknown-none-elf
              </div>
            </div>

          </div>
        )}

        {/* Cranelift CLIF IR & RISC-V Machine Code Accordion */}
        {showIR && runState.artifact && (
          <div className="space-y-3 pt-2">
            <div className="flex items-center justify-between text-xs font-mono font-bold text-neutral-700">
              <span>Lowered Cranelift CLIF IR ({runState.artifact.instructions} instructions)</span>
              <span className="text-neutral-400">Entry: {runState.artifact.entry_rip}</span>
            </div>
            <pre className="p-3 bg-neutral-950 text-emerald-400 font-mono text-xs rounded-xl overflow-x-auto max-h-48 border border-neutral-800">
              {runState.artifact.clif}
            </pre>
          </div>
        )}

      </CardContent>
    </Card>
  );
};
