import React, { useMemo } from "react";
import { Group } from "@visx/group";
import { scaleBand, scaleLinear } from "@visx/scale";
import { Bar } from "@visx/shape";
import { LinearGradient } from "@visx/gradient";
import { Card, CardHeader, CardTitle, CardDescription, Badge } from "@graundtech/fluent2-react-kit";

export interface MetricItem {
  name: string;
  addr: number;
  /** Source x86 window bytes (measured by the tracer / lift). */
  srcSize: number;
  /** Destination Pulley wasm bytes — null if not lifted/measured. */
  wasmSize: number | null;
  /** Destination RISC-V 64 ELF bytes — null if not compiled/measured. */
  riscvSize: number | null;
  /** Wall time of the Pulley runner process in ms — null if not measured. */
  runnerMs: number | null;
  isLifted: boolean;
  /** True when make demo actually executed this lift. */
  measured: boolean;
}

interface PerformanceMetricsViewProps {
  metrics: MetricItem[];
  selectedFunction: string | null;
  onSelectFunction: (name: string) => void;
}

export const PerformanceMetricsView: React.FC<PerformanceMetricsViewProps> = ({
  metrics,
  selectedFunction,
  onSelectFunction,
}) => {
  // Fixed compact dimensions for the Visx chart
  const width = 480;
  const height = 140;
  const margin = { top: 12, right: 12, bottom: 28, left: 12 };

  const xMax = width - margin.left - margin.right;
  const yMax = height - margin.top - margin.bottom;

  // Scales
  const xScale = useMemo(
    () =>
      scaleBand<string>({
        domain: metrics.map((d) => d.name),
        range: [0, xMax],
        padding: 0.28,
      }),
    [metrics, xMax]
  );

  const maxLatency = useMemo(() => {
    const maxVal = Math.max(
      ...metrics.map((d) => Math.max(d.srcSize || 0, d.wasmSize || 0, d.riscvSize || 0)),
      1
    );
    return maxVal * 1.15;
  }, [metrics]);

  const yScale = useMemo(
    () =>
      scaleLinear<number>({
        domain: [0, maxLatency],
        range: [yMax, 0],
      }),
    [maxLatency, yMax]
  );

  const activeItem = metrics.find((m) => m.name === selectedFunction) || metrics[0];

  return (
    <Card className="bg-white border border-black/[0.08] rounded-2xl p-6 shadow-sm">
      <CardHeader className="p-0 pb-4 flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <div>
          <div className="flex items-center gap-2 mb-0.5">
            <CardTitle className="text-sm font-bold text-neutral-900">
              Cross-ISA lowered binary sizes
            </CardTitle>
            <Badge variant="outline" className="text-[10px] font-mono border-black/15 text-neutral-600">
              measured bytes
            </Badge>
          </div>
          <CardDescription className="text-xs text-neutral-500">
            Native x86_64 source vs lowered RISC-V 64 ELF and Pulley Wasm.
          </CardDescription>
        </div>

        {/* Legend */}
        <div className="flex items-center gap-4 text-xs font-mono font-medium">
          <div className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-sm bg-neutral-900" />
            <span className="text-neutral-700">x86_64</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-sm bg-[#0284c7]" />
            <span className="text-[#0284c7]">RISC-V</span>
          </div>
          <div className="flex items-center gap-1.5">
            <span className="size-2.5 rounded-sm bg-[#FC4C01]" />
            <span className="text-[#FC4C01]">Pulley</span>
          </div>
        </div>
      </CardHeader>

      <div className="grid grid-cols-1 md:grid-cols-12 gap-6 items-center pt-2">

        {/* Compact Airbnb Visx Bar Chart */}
        <div className="md:col-span-7 flex flex-col items-center justify-center bg-[#F5F5F7] p-3 rounded-xl border border-black/[0.04]">
          <svg width="100%" height={height} viewBox={`0 0 ${width} ${height}`} className="overflow-visible">
            <LinearGradient id="visx-native-grad" from="#18181b" to="#27272a" />
            <LinearGradient id="visx-riscv-grad" from="#0284c7" to="#38bdf8" />
            <LinearGradient id="visx-pulley-grad" from="#FC4C01" to="#FF6B2C" />

            <Group left={margin.left} top={margin.top}>
              {metrics.map((d) => {
                const groupX = xScale(d.name) ?? 0;
                const bandWidth = xScale.bandwidth();
                const barWidth = Math.max(3, (bandWidth - 6) / 3);
                const isSelected = selectedFunction === d.name;

                const srcH = yMax - (yScale(d.srcSize || 0) ?? 0);
                const riscvVal = d.riscvSize;
                const riscvH =
                  riscvVal === null || riscvVal === undefined
                    ? 0
                    : yMax - (yScale(riscvVal) ?? 0);
                const wasmVal = d.wasmSize;
                const wasmH =
                  wasmVal === null || wasmVal === undefined
                    ? 0
                    : yMax - (yScale(wasmVal) ?? 0);

                return (
                  <Group
                    key={d.name}
                    left={groupX}
                    className="cursor-pointer transition-opacity hover:opacity-80"
                    onClick={() => onSelectFunction(d.name)}
                  >
                    {/* Active highlight pill behind bars */}
                    {isSelected && (
                      <rect
                        x={-4}
                        y={-6}
                        width={bandWidth + 8}
                        height={yMax + 10}
                        rx={6}
                        fill="#0284c7"
                        fillOpacity={0.08}
                        stroke="#0284c7"
                        strokeWidth={1}
                        strokeDasharray="2 2"
                      />
                    )}

                    {/* Source x86 bytes */}
                    <Bar
                      x={0}
                      y={yMax - srcH}
                      width={barWidth}
                      height={Math.max(2, srcH)}
                      fill="url(#visx-native-grad)"
                      rx={2}
                    />

                    {/* RISC-V bytes */}
                    <Bar
                      x={barWidth + 2}
                      y={yMax - riscvH}
                      width={barWidth}
                      height={
                        riscvVal === null || riscvVal === undefined
                          ? 2
                          : Math.max(2, riscvH)
                      }
                      fill={
                        riscvVal === null || riscvVal === undefined
                          ? "#d4d4d8"
                          : "url(#visx-riscv-grad)"
                      }
                      rx={2}
                    />

                    {/* Wasm bytes — only when measured */}
                    <Bar
                      x={(barWidth + 2) * 2}
                      y={yMax - wasmH}
                      width={barWidth}
                      height={
                        wasmVal === null || wasmVal === undefined
                          ? 2
                          : Math.max(2, wasmH)
                      }
                      fill={
                        wasmVal === null || wasmVal === undefined
                          ? "#d4d4d8"
                          : "url(#visx-pulley-grad)"
                      }
                      rx={2}
                    />

                    {/* Label */}
                    <text
                      x={bandWidth / 2}
                      y={yMax + 16}
                      textAnchor="middle"
                      className={`text-[9px] font-mono select-none ${
                        isSelected ? "fill-[#0284c7] font-bold" : "fill-neutral-500"
                      }`}
                    >
                      {d.name.replace("_", "").slice(0, 7)}
                    </text>
                  </Group>
                );
              })}
            </Group>
          </svg>
        </div>

        {/* High-Contrast Numeric Comparison Card */}
        <div className="md:col-span-5 flex flex-col justify-between gap-3 bg-neutral-900 text-white p-4 rounded-xl shadow-md border border-neutral-800">
          <div className="flex items-center justify-between border-b border-neutral-800 pb-2">
            <span className="font-mono text-xs font-bold truncate text-white">
              {activeItem ? activeItem.name : "Pick a function"}
            </span>
            <Badge
              variant={activeItem?.isLifted ? "default" : "secondary"}
              className={activeItem?.isLifted ? "bg-[#FC4C01] text-white border-transparent text-[10px]" : "bg-neutral-800 text-neutral-300 text-[10px]"}
            >
              {activeItem?.isLifted ? "LIFTED" : "KEEP_NATIVE"}
            </Badge>
          </div>

          <div className="grid grid-cols-3 gap-2 text-xs font-mono">
            <div>
              <span className="text-[9px] text-neutral-400 block">x86_64</span>
              <span className="text-sm font-bold text-white">
                {activeItem ? `${activeItem.srcSize} B` : "---"}
              </span>
              <span className="text-[9px] text-neutral-400 block mt-0.5 truncate">
                {activeItem ? `0x${activeItem.addr.toString(16)}` : "---"}
              </span>
            </div>

            <div>
              <span className="text-[9px] text-neutral-400 block">RISC-V 64</span>
              <span className={`text-sm font-bold ${activeItem?.riscvSize != null ? "text-[#38bdf8]" : "text-neutral-500"}`}>
                {activeItem?.riscvSize != null ? `${activeItem.riscvSize} B` : "—"}
              </span>
              <span className="text-[9px] text-neutral-400 block mt-0.5">
                ELF binary
              </span>
            </div>

            <div>
              <span className="text-[9px] text-neutral-400 block">PULLEY</span>
              <span className={`text-sm font-bold ${activeItem?.wasmSize != null ? "text-[#FC4C01]" : "text-neutral-500"}`}>
                {activeItem?.wasmSize != null ? `${activeItem.wasmSize} B` : "—"}
              </span>
              <span className="text-[9px] text-neutral-400 block mt-0.5">
                {activeItem?.runnerMs != null
                  ? `${activeItem.runnerMs.toFixed(1)} ms`
                  : "—"}
              </span>
            </div>
          </div>

          <div className="flex items-center justify-between pt-1 border-t border-neutral-800 text-[11px] font-mono text-neutral-400">
            <span>Source:</span>
            <span className="text-neutral-300">
              {activeItem?.measured
                ? "make demo lift_results"
                : "static analysis only"}
            </span>
          </div>
        </div>

      </div>
    </Card>
  );
};
