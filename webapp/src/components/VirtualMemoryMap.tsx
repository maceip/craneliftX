import React from "react";
import { Card, CardHeader, CardTitle, CardDescription, Badge } from "@graundtech/fluent2-react-kit";
import { ThinkingOrb } from "thinking-orbs";

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
              Function map
            </CardTitle>
            <Badge variant="outline" className="text-[10px] font-mono border-black/15 text-neutral-600">
              x86_64 → RISC-V 64 / Pulley
            </Badge>
          </div>
          <CardDescription className="text-xs text-neutral-500">
            Each block is a function in the object file. Orange is selected to lift; gray stays native.
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
            <span className="font-bold text-neutral-800">Object file (x86_64)</span>
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

        {/* Center: lift indicator (ThinkingOrb) */}
        <div className="md:col-span-2 flex flex-col items-center justify-center py-2">
          <div className="relative w-full flex flex-col items-center gap-3">
            <div className="flex items-center justify-center h-[160px]">
              <ThinkingOrb
                state={activeSegment?.decision === "LIFT" ? "connecting" : "breathing"}
                size={64}
                color="#FC4C01"
                theme="light"
                aria-label={
                  activeSegment?.decision === "LIFT"
                    ? "Lift path active"
                    : "Function stays native"
                }
              />
            </div>
            <span className="text-[10px] font-mono font-bold text-[#FC4C01] bg-[#FC4C01]/10 px-2 py-0.5 rounded-full border border-[#FC4C01]/20">
              {activeSegment?.decision === "LIFT" ? "LIFT" : "NATIVE"}
            </span>
          </div>
        </div>

        {/* Right Side: selected function (real fields only) */}
        <div className="md:col-span-5 flex flex-col gap-2 bg-neutral-900 text-white p-4 rounded-xl border border-neutral-800 shadow-md">
          <div className="flex items-center justify-between text-xs font-mono">
            <span className="font-bold text-white">Selected function</span>
            <span className="text-[10px] text-neutral-400 font-semibold">
              {activeSegment
                ? `0x${activeSegment.addr.toString(16)}`
                : "—"}
            </span>
          </div>

          {/* Active Inspection Box */}
          <div className="bg-neutral-950 p-3.5 rounded-lg border border-neutral-800 flex flex-col gap-2 mt-1">
            <div className="flex items-center justify-between">
              <span className="font-mono text-xs font-bold text-white truncate">
                {activeSegment ? activeSegment.name : "Select a function"}
              </span>
              <span className={`text-[10px] font-mono font-bold px-2 py-0.5 rounded ${
                activeSegment?.decision === "LIFT"
                  ? "bg-[#FC4C01] text-white"
                  : "bg-neutral-800 text-neutral-400"
              }`}>
                {activeSegment?.decision === "LIFT" ? "LIFT → RISC-V / PULLEY" : "KEEP_NATIVE"}
              </span>
            </div>

            <div className="space-y-1 font-mono text-[11px] text-neutral-400 pt-1">
              <div className="flex justify-between">
                <span>Size:</span>
                <span className="text-white font-medium">
                  {activeSegment
                    ? `${activeSegment.size_bytes}B · ${activeSegment.n_insn} insn`
                    : "—"}
                </span>
              </div>
              <div className="flex justify-between">
                <span>Decision:</span>
                <span className="text-neutral-300">
                  {activeSegment?.decision || "—"}
                </span>
              </div>
              <div className="flex justify-between">
                <span>Address:</span>
                <span className="text-neutral-300">
                  {activeSegment
                    ? `0x${activeSegment.addr.toString(16)}–0x${(
                        activeSegment.addr + activeSegment.size_bytes
                      ).toString(16)}`
                    : "—"}
                </span>
              </div>
            </div>

            <div className="pt-2 border-t border-neutral-800 text-[10px] font-mono text-neutral-400">
              <span className="text-neutral-500">Tracer reason: </span>
              <span className="text-neutral-300 italic truncate block">
                {activeSegment?.reason || "—"}
              </span>
            </div>
          </div>

          <div className="flex justify-between text-[10px] font-mono text-neutral-500 pt-1">
            <span>from disassembly</span>
            <span>Capstone</span>
          </div>
        </div>

      </div>
    </Card>
  );
};
