import React, { useState, useMemo } from "react";
import { voronoi, VoronoiPolygon } from "@visx/voronoi";
import { FunctionItem } from "../types";

interface VoronoiBinaryGraphProps {
  functions: FunctionItem[];
  binaryName: string | null;
  onSelectFunction?: (fnName: string) => void;
  selectedFunction?: string | null;
}

interface VoronoiNode {
  x: number;
  y: number;
  fn: FunctionItem;
  id: string;
}

export const VoronoiBinaryGraph: React.FC<VoronoiBinaryGraphProps> = ({
  functions,
  binaryName,
  onSelectFunction,
  selectedFunction,
}) => {
  const [hoveredNode, setHoveredNode] = useState<VoronoiNode | null>(null);
  const [showWireframe, setShowWireframe] = useState<boolean>(true);
  const [showCentroids, setShowCentroids] = useState<boolean>(true);

  const width = 860;
  const height = 340;
  const margin = { top: 24, right: 30, bottom: 24, left: 30 };

  // Map functions into 2D address x complexity space
  const nodes: VoronoiNode[] = useMemo(() => {
    if (!functions.length) return [];

    const minAddr = Math.min(...functions.map((f) => f.addr));
    const maxAddr = Math.max(...functions.map((f) => f.addr + f.size_bytes)) || 1;
    const addrSpan = Math.max(1, maxAddr - minAddr);

    // If few functions, add synthetic basic-block subdivisions to fill the visual topology
    const points: VoronoiNode[] = [];

    functions.forEach((f, idx) => {
      // Primary function centroid
      const nx =
        margin.left +
        ((f.addr - minAddr + f.size_bytes / 2) / addrSpan) *
          (width - margin.left - margin.right);
      const ny =
        margin.top +
        (1 - Math.min(0.9, Math.max(0.1, f.loop_density * 0.7 + f.call_fraction * 0.3))) *
          (height - margin.top - margin.bottom);

      points.push({
        x: Math.max(margin.left, Math.min(width - margin.right, nx)),
        y: Math.max(margin.top, Math.min(height - margin.bottom, ny)),
        fn: f,
        id: f.name,
      });

      // Synthetic basic-block sub-cells to represent internal control flow
      const subBlocks = Math.max(1, Math.min(4, Math.floor(f.n_insn / 8)));
      for (let b = 1; b < subBlocks; b++) {
        const offsetRatio = b / subBlocks;
        const subX =
          margin.left +
          ((f.addr - minAddr + f.size_bytes * offsetRatio) / addrSpan) *
            (width - margin.left - margin.right);
        const jitterY = (Math.sin(idx * 7 + b * 3) * 0.2 + 0.5) * (height - margin.top - margin.bottom);
        points.push({
          x: Math.max(margin.left, Math.min(width - margin.right, subX + (b % 2 === 0 ? 12 : -12))),
          y: Math.max(margin.top, Math.min(height - margin.bottom, margin.top + jitterY)),
          fn: f,
          id: `${f.name}-bb${b}`,
        });
      }
    });

    return points;
  }, [functions, width, height]);

  // Compute Voronoi diagram using @visx/voronoi
  const voronoiDiagram = useMemo(() => {
    if (nodes.length < 2) return null;
    const v = voronoi<VoronoiNode>({
      x: (d) => d.x,
      y: (d) => d.y,
      width,
      height,
    });
    return v(nodes);
  }, [nodes, width, height]);

  const polygons = useMemo(() => {
    return voronoiDiagram ? voronoiDiagram.polygons() : [];
  }, [voronoiDiagram]);

  if (!functions.length) {
    return (
      <div
        style={{
          background: "var(--surface-1)",
          border: "1px solid var(--line)",
          borderRadius: 12,
          padding: 40,
          textAlign: "center",
          color: "#94a3b8",
        }}
      >
        <div style={{ fontSize: 32, marginBottom: 12 }}>🕸️</div>
        <div style={{ fontSize: 16, fontWeight: 600, color: "#fff" }}>
          Voronoi Topology Awaiting Binary Ingest
        </div>
        <div style={{ fontSize: 13, marginTop: 6 }}>
          Run the static analysis to project the binary's address space and control-flow tessellation.
        </div>
      </div>
    );
  }

  const getCellFill = (fn: FunctionItem, isSelected: boolean, isHovered: boolean) => {
    if (fn.decision === "LIFT") {
      if (isSelected || isHovered) return "rgba(16, 185, 129, 0.45)";
      return "rgba(16, 185, 129, 0.16)";
    }
    if (fn.loop_density > 0.45) {
      if (isSelected || isHovered) return "rgba(239, 68, 68, 0.45)";
      return "rgba(239, 68, 68, 0.18)";
    }
    if (isSelected || isHovered) return "rgba(245, 158, 11, 0.45)";
    return "rgba(245, 158, 11, 0.16)";
  };

  const getCellStroke = (fn: FunctionItem, isSelected: boolean, isHovered: boolean) => {
    if (isSelected || isHovered) return "#38bdf8";
    if (fn.decision === "LIFT") return "rgba(16, 185, 129, 0.6)";
    if (fn.loop_density > 0.45) return "rgba(239, 68, 68, 0.6)";
    return "rgba(245, 158, 11, 0.6)";
  };

  return (
    <div
      style={{
        background: "var(--surface-1)",
        border: "1px solid var(--line)",
        borderRadius: 14,
        padding: "20px 24px",
        position: "relative",
        overflow: "hidden",
      }}
    >
      {/* Header Bar */}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 16,
        }}
      >
        <div>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <h2 style={{ margin: 0, fontSize: 18, fontWeight: 700, color: "#fff" }}>
              Binary Address Space &bull; Voronoi Topology
            </h2>
            <span
              style={{
                fontFamily: "var(--mono)",
                fontSize: 11,
                color: "#38bdf8",
                background: "rgba(56, 189, 248, 0.12)",
                padding: "2px 8px",
                borderRadius: 4,
                border: "1px solid rgba(56, 189, 248, 0.3)",
              }}
            >
              @visx/voronoi
            </span>
          </div>
          <div style={{ fontSize: 12, color: "#94a3b8", marginTop: 3 }}>
            Geometric partition of machine code extent &times; loop density &bull; Click any cell to inspect dual-run trace
          </div>
        </div>

        {/* Graph Controls */}
        <div style={{ display: "flex", gap: 8 }}>
          <button
            className="btn-secondary"
            onClick={() => setShowWireframe(!showWireframe)}
            style={{ fontSize: 11, padding: "4px 10px" }}
          >
            {showWireframe ? "Hide Mesh" : "Show Mesh"}
          </button>
          <button
            className="btn-secondary"
            onClick={() => setShowCentroids(!showCentroids)}
            style={{ fontSize: 11, padding: "4px 10px" }}
          >
            {showCentroids ? "Hide Centroids" : "Show Centroids"}
          </button>
        </div>
      </div>

      {/* Voronoi SVG Canvas */}
      <div style={{ position: "relative", width: "100%", overflowX: "auto" }}>
        <svg
          viewBox={`0 0 ${width} ${height}`}
          style={{
            width: "100%",
            height: "auto",
            display: "block",
            background: "#050912",
            borderRadius: 10,
            border: "1px solid #162238",
          }}
        >
          {/* Subtle background coordinate grid */}
          <defs>
            <pattern id="voronoi-grid" width="30" height="30" patternUnits="userSpaceOnUse">
              <path d="M 30 0 L 0 0 0 30" fill="none" stroke="rgba(255,255,255,0.03)" strokeWidth="1" />
            </pattern>
          </defs>
          <rect width={width} height={height} fill="url(#voronoi-grid)" />

          {/* Voronoi Polygons from @visx/voronoi */}
          {polygons.map((polygon, i) => {
            const node = polygon.data as VoronoiNode;
            if (!node || !node.fn) return null;
            const isHovered = hoveredNode?.id === node.id || hoveredNode?.fn.name === node.fn.name;
            const isSelected = selectedFunction === node.fn.name;

            return (
              <g key={node.id || i}>
                <VoronoiPolygon
                  polygon={polygon}
                  fill={getCellFill(node.fn, isSelected, isHovered)}
                  stroke={getCellStroke(node.fn, isSelected, isHovered)}
                  strokeWidth={isHovered || isSelected ? 2.5 : showWireframe ? 1 : 0.2}
                  style={{
                    cursor: "pointer",
                    transition: "fill 0.15s ease, stroke 0.15s ease",
                  }}
                  onMouseEnter={() => setHoveredNode(node)}
                  onMouseLeave={() => setHoveredNode(null)}
                  onClick={() => onSelectFunction?.(node.fn.name)}
                />
              </g>
            );
          })}

          {/* Function Centroid Nodes & Labels */}
          {showCentroids &&
            nodes.map((node) => {
              const isPrimary = !node.id.includes("-bb");
              const isHovered = hoveredNode?.fn.name === node.fn.name;
              const isSelected = selectedFunction === node.fn.name;
              if (!isPrimary && !isHovered) return null;

              const isLift = node.fn.decision === "LIFT";
              const isHot = node.fn.loop_density > 0.45;
              const dotColor = isLift ? "#10b981" : isHot ? "#ef4444" : "#f59e0b";

              return (
                <g key={node.id} style={{ pointerEvents: "none" }}>
                  <circle
                    cx={node.x}
                    cy={node.y}
                    r={isHovered || isSelected ? 6 : isPrimary ? 4 : 2}
                    fill={dotColor}
                    stroke="#fff"
                    strokeWidth={isHovered ? 2 : 1}
                  />

                  {isPrimary && (
                    <text
                      x={node.x}
                      y={node.y - 8}
                      textAnchor="middle"
                      fill="#e2e8f0"
                      fontSize="10"
                      fontFamily="var(--mono)"
                      fontWeight="600"
                      style={{
                        paintOrder: "stroke",
                        stroke: "#070b12",
                        strokeWidth: 3,
                        strokeLinejoin: "round",
                      }}
                    >
                      {node.fn.name}
                    </text>
                  )}
                </g>
              );
            })}
        </svg>

        {/* Hover Tooltip Card */}
        {hoveredNode && (
          <div
            style={{
              position: "absolute",
              top: 14,
              right: 14,
              background: "rgba(10, 16, 28, 0.94)",
              backdropFilter: "blur(8px)",
              border: "1px solid #38bdf8",
              borderRadius: 8,
              padding: "10px 14px",
              fontFamily: "var(--mono)",
              fontSize: 12,
              color: "#fff",
              pointerEvents: "none",
              boxShadow: "0 8px 24px rgba(0,0,0,0.5)",
              maxWidth: 320,
            }}
          >
            <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4 }}>
              <strong style={{ color: "#38bdf8" }}>{hoveredNode.fn.name}</strong>
              <span
                style={{
                  fontSize: 10,
                  fontWeight: 700,
                  padding: "1px 6px",
                  borderRadius: 4,
                  background:
                    hoveredNode.fn.decision === "LIFT"
                      ? "rgba(16, 185, 129, 0.2)"
                      : "rgba(245, 158, 11, 0.2)",
                  color: hoveredNode.fn.decision === "LIFT" ? "#10b981" : "#f59e0b",
                }}
              >
                {hoveredNode.fn.decision}
              </span>
            </div>

            <div style={{ fontSize: 11, color: "#94a3b8", marginBottom: 6 }}>
              0x{hoveredNode.fn.addr.toString(16)} (+{hoveredNode.fn.size_bytes}B) &bull;{" "}
              {hoveredNode.fn.n_insn} instructions
            </div>

            <div style={{ fontSize: 11, display: "flex", gap: 12, marginBottom: 6 }}>
              <span>Loop: <b style={{ color: hoveredNode.fn.loop_density > 0.45 ? "#ef4444" : "#fff" }}>{hoveredNode.fn.loop_density.toFixed(3)}</b></span>
              <span>Calls: <b>{hoveredNode.fn.call_fraction.toFixed(3)}</b></span>
              <span>Net: <b>{hoveredNode.fn.network_score.toFixed(2)}</b></span>
            </div>

            {hoveredNode.fn.reason && (
              <div style={{ fontSize: 10.5, color: "#cbd5e1", borderTop: "1px solid #1e293b", paddingTop: 4 }}>
                {hoveredNode.fn.reason}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Legend */}
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginTop: 12,
          fontSize: 11,
          fontFamily: "var(--mono)",
          color: "#94a3b8",
        }}
      >
        <div style={{ display: "flex", gap: 16 }}>
          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
            <span style={{ width: 10, height: 10, borderRadius: 2, background: "rgba(16, 185, 129, 0.8)" }} />
            LIFT (Pulley Portable VM)
          </span>
          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
            <span style={{ width: 10, height: 10, borderRadius: 2, background: "rgba(245, 158, 11, 0.8)" }} />
            KEEP NATIVE (Direct Host CPU)
          </span>
          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
            <span style={{ width: 10, height: 10, borderRadius: 2, background: "rgba(239, 68, 68, 0.8)" }} />
            REJECTED BY GUARDRAIL (Tight Loop &gt; 0.45)
          </span>
        </div>

        <div>
          Binary: <code>{binaryName || "sample_network.o"}</code>
        </div>
      </div>
    </div>
  );
};
