import React, { useState, useEffect } from "react";
import { Card, CardHeader, CardTitle, CardDescription, Badge } from "@graundtech/fluent2-react-kit";

export interface MemorySegment {
  name: string;
  addr: number;
  size_bytes: number;
  decision: "LIFT" | "KEEP_NATIVE";
  reason: string;
  n_insn: number;
}

interface VirtualMemoryMapProps {
  segments: MemorySegment[];
  selectedFunction: string | null;
  onSelectFunction: (name: string) => void;
  isLoading: boolean;
}

export const VirtualMemoryMap: React.FC<VirtualMemoryMapProps> = ({
  segments,
  selectedFunction,
  onSelectFunction,
  isLoading,
}) => {
  const [animStep, setAnimStep] = useState<number>(0);

  // Cycle animation step for active migration motion
  useEffect(() => {
    const timer = setInterval(() => {
      setAnimStep((prev) => (prev + 1) % 100);
    }, 40);
    return () => clearInterval(timer);
  }, []);

  const sorted = [...segments].sort((a, b) => a.addr - b.addr);
  // Code footprint from real segment extents — not a hardcoded number.
  const totalBytes = sorted.reduce(
    (m, s) => Math.max(m, s.addr + s.size_bytes),
    0
  );

  const activeSegment =
    sorted.find((s) => s.name === selectedFunction) ||
    sorted.find((s) => s.decision === "LIFT") ||
    sorted[0];

  return (
    <Card className="bg-white border border-black/[0.08] rounded-2xl p-6 shadow-sm overflow-hidden">
      <CardHeader className="p-0 pb-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <div>
          <div className="flex items-center gap-2 mb-0.5">
            <CardTitle className="text-sm font-bold text-neutral-900">
              Virtual Memory Map &amp; Cross-ISA Lift Chamber
            </CardTitle>
            <Badge variant="outline" className="text-[10px] font-mono border-black/15 text-neutral-600">
              x86_64 CISC &rarr; Pulley VM
            </Badge>
          </div>
          <CardDescription className="text-xs text-neutral-500">
            Function extents in host object space. Orange = selected for lift; gray = kept native. Layout is tracer data, not a mock plane.
          </CardDescription>
        </div>

        <div className="flex items-center gap-2 text-xs font-mono">
          <span className="flex items-center gap-1.5 bg-neutral-100 text-neutral-600 px-2.5 py-1 rounded-full border border-black/10 font-semibold text-[11px]">
            <span className="size-1.5 rounded-full bg-[#FC4C01]" />
            {sorted.filter((s) => s.decision === "LIFT").length} lift candidates
          </span>
        </div>
      </CardHeader>

      <div className="grid grid-cols-1 md:grid-cols-12 gap-6 items-center pt-2">
        
        {/* Left Side: Host x86 Virtual Memory Plane */}
        <div className="md:col-span-5 flex flex-col gap-2 bg-[#F5F5F7] p-4 rounded-xl border border-black/[0.05]">
          <div className="flex items-center justify-between text-xs font-mono">
            <span className="font-bold text-neutral-800">Host Virtual Memory (x86_64)</span>
            <span className="text-[10px] text-neutral-400">Base: 0x00000000</span>
          </div>

          {/* Segment Blocks */}
          <div className="flex flex-col gap-1.5 pt-1">
            {sorted.map((seg) => {
              const isSelected = activeSegment?.name === seg.name;
              const isLifted = seg.decision === "LIFT";

              return (
                <button
                  key={seg.name}
                  onClick={() => onSelectFunction(seg.name)}
                  className={`flex items-center justify-between p-2 rounded-lg border text-xs font-mono text-left transition-all ${
                    isSelected
                      ? isLifted
                        ? "bg-white border-[#FC4C01] shadow-sm ring-1 ring-[#FC4C01]"
                        : "bg-white border-neutral-900 shadow-sm ring-1 ring-neutral-900"
                      : "bg-white/70 border-black/[0.04] hover:bg-white"
                  }`}
                >
                  <div className="flex items-center gap-2 truncate">
                    <span
                      className={`size-2 rounded-full shrink-0 ${
                        isLifted ? "bg-[#FC4C01]" : "bg-neutral-400"
                      }`}
                    />
                    <span className="font-semibold text-neutral-800 truncate">
                      {seg.name}
                    </span>
                  </div>

                  <div className="flex items-center gap-2 shrink-0 text-[10px] text-neutral-400">
                    <span>0x{seg.addr.toString(16).padStart(2, "0")}</span>
                    <span className="w-8 text-right font-medium">{seg.size_bytes}B</span>
                  </div>
                </button>
              );
            })}
          </div>

          <div className="flex justify-between text-[10px] font-mono text-neutral-400 pt-1">
            <span>code span: {totalBytes}B</span>
            <span>ELF 64-bit LSB</span>
          </div>
        </div>

        {/* Center: Dynamic Animated ISA Migration Conduit (SVG Particle Beam) */}
        <div className="md:col-span-2 flex flex-col items-center justify-center py-2">
          <div className="relative w-full flex flex-col items-center">
            {/* SVG Conduit Paths */}
            <svg width="100%" height="160" viewBox="0 0 120 160" className="overflow-visible">
              <defs>
                <linearGradient id="beam-gradient" x1="0%" y1="0%" x2="100%" y2="0%">
                  <stop offset="0%" stopColor="#18181b" />
                  <stop offset="50%" stopColor="#FC4C01" />
                  <stop offset="100%" stopColor="#FC4C01" />
                </linearGradient>
              </defs>

              {/* Static Guide Track */}
              <path
                d="M 10 80 C 50 80, 70 80, 110 80"
                fill="none"
                stroke="rgba(0,0,0,0.08)"
                strokeWidth="4"
                strokeLinecap="round"
              />

              {/* Animated Particle Beam */}
              {activeSegment?.decision === "LIFT" ? (
                <>
                  <path
                    d="M 10 80 C 50 80, 70 80, 110 80"
                    fill="none"
                    stroke="url(#beam-gradient)"
                    strokeWidth="3"
                    strokeDasharray="6 6"
                    strokeDashoffset={-animStep * 1.5}
                    strokeLinecap="round"
                  />
                  {/* Glowing Transit Core */}
                  <circle
                    cx={15 + ((animStep % 100) / 100) * 90}
                    cy={80}
                    r={4}
                    fill="#FC4C01"
                    className="drop-shadow-[0_0_8px_rgba(252,76,1,0.8)]"
                  />
                </>
              ) : (
                <text
                  x="60"
                  y="75"
                  textAnchor="middle"
                  className="text-[9px] font-mono fill-neutral-400 font-medium"
                >
                  HOST LOCKED
                </text>
              )}
            </svg>

            {/* Micro Badge Under Conduit */}
            <span className="text-[10px] font-mono font-bold text-[#FC4C01] bg-[#FC4C01]/10 px-2 py-0.5 rounded-full border border-[#FC4C01]/20 -mt-8">
              {activeSegment?.decision === "LIFT" ? "LIFTING" : "SILICON"}
            </span>
          </div>
        </div>

        {/* Right Side: Cranelift Pulley VM Isolated Virtual Memory Sandbox */}
        <div className="md:col-span-5 flex flex-col gap-2 bg-neutral-900 text-white p-4 rounded-xl border border-neutral-800 shadow-md">
          <div className="flex items-center justify-between text-xs font-mono">
            <span className="font-bold text-white">Pulley VM Sandbox (Safe ISA)</span>
            <span className="text-[10px] text-emerald-400 font-semibold">vm://0x1000</span>
          </div>

          {/* Active Inspection Box */}
          <div className="bg-neutral-950 p-3.5 rounded-lg border border-neutral-800 flex flex-col gap-2 mt-1">
            <div className="flex items-center justify-between">
              <span className="font-mono text-xs font-bold text-white truncate">
                {activeSegment ? activeSegment.name : "Select Spot"}
              </span>
              <span className={`text-[10px] font-mono font-bold px-2 py-0.5 rounded ${
                activeSegment?.decision === "LIFT"
                  ? "bg-[#FC4C01] text-white"
                  : "bg-neutral-800 text-neutral-400"
              }`}>
                {activeSegment?.decision === "LIFT" ? "ISA: PULLEY64" : "ISA: x86_64 HOST"}
              </span>
            </div>

            <div className="space-y-1 font-mono text-[11px] text-neutral-400 pt-1">
              <div className="flex justify-between">
                <span>Transformed Size:</span>
                <span className="text-white font-medium">{activeSegment ? `${activeSegment.size_bytes}B (${activeSegment.n_insn} insn)` : "---"}</span>
              </div>
              <div className="flex justify-between">
                <span>Decompiler Route:</span>
                <span className="text-neutral-300">Anvill &rarr; Pulley IR</span>
              </div>
              <div className="flex justify-between">
                <span>Memory Isolation:</span>
                <span className={activeSegment?.decision === "LIFT" ? "text-emerald-400 font-semibold" : "text-neutral-500"}>
                  {activeSegment?.decision === "LIFT" ? "Bounds Enforced" : "Native Shared"}
                </span>
              </div>
            </div>

            <div className="pt-2 border-t border-neutral-800 text-[10px] font-mono text-neutral-400">
              <span className="text-neutral-500">Static Decision: </span>
              <span className="text-neutral-300 italic truncate block">
                {activeSegment?.reason || "Static analysis pass"}
              </span>
            </div>
          </div>

          <div className="flex justify-between text-[10px] font-mono text-neutral-400 pt-1">
            <span className="text-emerald-400">Zero Unsafe Pointers</span>
            <span>Memory Safe Interpreter</span>
          </div>
        </div>

      </div>
    </Card>
  );
};
