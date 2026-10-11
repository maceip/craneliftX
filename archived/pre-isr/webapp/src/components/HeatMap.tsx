import React, { useState } from "react";
import { HeatMapPayload, HeatMapRow, HeatMapColumn } from "../types";

export interface HeatMapProps {
  payload: HeatMapPayload | null;
  /**
   * Adapter prop: When the custom React Heatmap library is provided,
   * pass it here or mount it inside this adapter boundary.
   */
  customRenderer?: React.ComponentType<{ data: HeatMapPayload }>;
}

export const HeatMap: React.FC<HeatMapProps> = ({ payload, customRenderer: CustomRenderer }) => {
  const [hoveredCell, setHoveredCell] = useState<{
    row: HeatMapRow;
    col: HeatMapColumn;
    val: number;
  } | null>(null);

  const [sortCol, setSortCol] = useState<string>("loop_density");

  if (!payload || !payload.rows || payload.rows.length === 0) {
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
        <div style={{ fontSize: 32, marginBottom: 12 }}>🔥</div>
        <div style={{ fontSize: 16, fontWeight: 600, color: "#fff" }}>
          Heatmap Matrix Awaiting Binary Analysis
        </div>
        <div style={{ fontSize: 13, marginTop: 6 }}>
          Run the static analysis to generate the function-level complexity and execution hotspot heatmap.
        </div>
      </div>
    );
  }

  // If user provides a custom React heatmap library component, render it through the adapter!
  if (CustomRenderer) {
    return (
      <div className="heatmap-container">
        <div className="heatmap-header">
          <div>
            <h2 style={{ margin: "0 0 4px", fontSize: 18, fontWeight: 700 }}>
              Function Execution & Complexity Heatmap
            </h2>
            <div style={{ fontSize: 12, color: "#94a3b8" }}>
              Rendered via custom React Heatmap Library plugin
            </div>
          </div>
          <div className="heatmap-adapter-notice">
            <span>⚡ Custom React Heatmap Library Active</span>
          </div>
        </div>
        <CustomRenderer data={payload} />
      </div>
    );
  }

  // Function to compute heat color based on value and threshold
  const getCellColor = (val: number, max: number, threshold: number) => {
    const ratio = Math.min(1, Math.max(0, val / (max || 1)));
    if (val > threshold) {
      // Hotspot / Guardrail trigger (Crimson glow)
      return {
        bg: `rgba(239, 68, 68, ${0.35 + ratio * 0.55})`,
        border: "1px solid rgba(239, 68, 68, 0.7)",
        color: "#fecaca",
        isHot: true,
      };
    }
    if (ratio > 0.4) {
      // Medium intensity (Amber)
      return {
        bg: `rgba(245, 158, 11, ${0.2 + ratio * 0.4})`,
        border: "1px solid rgba(245, 158, 11, 0.4)",
        color: "#fde68a",
        isHot: false,
      };
    }
    if (ratio > 0.05) {
      // Low-medium (Cyan / Emerald)
      return {
        bg: `rgba(56, 189, 248, ${0.15 + ratio * 0.3})`,
        border: "1px solid rgba(56, 189, 248, 0.3)",
        color: "#bae6fd",
        isHot: false,
      };
    }
    // Baseline / Cold (Subtle navy)
    return {
      bg: "rgba(30, 41, 59, 0.5)",
      border: "1px solid rgba(255, 255, 255, 0.05)",
      color: "#64748b",
      isHot: false,
    };
  };

  const sortedRows = [...payload.rows].sort((a, b) => {
    // @ts-ignore
    const valA = a[sortCol] ?? 0;
    // @ts-ignore
    const valB = b[sortCol] ?? 0;
    return valB - valA;
  });

  return (
    <div className="heatmap-container">
      <div className="heatmap-header">
        <div>
          <h2 style={{ margin: "0 0 4px", fontSize: 18, fontWeight: 700, color: "#fff" }}>
            Function Complexity & Execution Heatmap
          </h2>
          <div style={{ fontSize: 12, color: "#94a3b8" }}>
            Real-time heat distribution across AST metrics, loop density, and placement criteria
          </div>
        </div>

        <div className="heatmap-adapter-notice">
          <span>🔥 React Heatmap Engine (Adapter ready for plug-in React Heatmap library)</span>
        </div>
      </div>

      {/* Heatmap Matrix Table */}
      <div style={{ overflowX: "auto" }}>
        <table className="heatmap-table">
          <thead>
            <tr>
              <th className="heatmap-th" style={{ width: 180 }}>Function</th>
              <th className="heatmap-th" style={{ width: 80 }}>Addr</th>
              <th className="heatmap-th" style={{ width: 100 }}>Decision</th>
              {payload.columns.map((col) => {
                const isSorted = sortCol === col.id;
                return (
                  <th
                    key={col.id}
                    className="heatmap-th"
                    onClick={() => setSortCol(col.id)}
                    style={{
                      cursor: "pointer",
                      color: isSorted ? "#38bdf8" : undefined,
                      textDecoration: isSorted ? "underline" : undefined,
                    }}
                    title={`Click to sort by ${col.label} (Threshold: ${col.threshold})`}
                  >
                    {col.label} {isSorted && "▼"}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {sortedRows.map((row) => (
              <tr key={row.name}>
                <td
                  style={{
                    padding: "10px 12px",
                    fontFamily: "var(--mono)",
                    fontSize: 13,
                    fontWeight: 600,
                    color: "#fff",
                  }}
                >
                  {row.name}
                </td>
                <td
                  style={{
                    padding: "10px 12px",
                    fontFamily: "var(--mono)",
                    fontSize: 11,
                    color: "#94a3b8",
                  }}
                >
                  {row.addr_hex}
                </td>
                <td style={{ padding: "10px 12px" }}>
                  <span
                    style={{
                      fontSize: 10,
                      fontWeight: 700,
                      padding: "2px 6px",
                      borderRadius: 4,
                      fontFamily: "var(--mono)",
                      background: row.decision === "LIFT" ? "rgba(16, 185, 129, 0.2)" : "rgba(245, 158, 11, 0.2)",
                      color: row.decision === "LIFT" ? "#10b981" : "#f59e0b",
                      border: `1px solid ${row.decision === "LIFT" ? "rgba(16, 185, 129, 0.4)" : "rgba(245, 158, 11, 0.4)"}`,
                    }}
                  >
                    {row.decision}
                  </span>
                </td>

                {payload.columns.map((col) => {
                  // @ts-ignore
                  const val: number = row[col.id] ?? 0;
                  const style = getCellColor(val, col.max, col.threshold);

                  return (
                    <td
                      key={col.id}
                      className="heatmap-cell"
                      style={{
                        backgroundColor: style.bg,
                        border: style.border,
                        color: style.color,
                        fontWeight: style.isHot ? 700 : 400,
                      }}
                      onMouseEnter={() => setHoveredCell({ row, col, val })}
                      onMouseLeave={() => setHoveredCell(null)}
                    >
                      {val.toFixed(3)}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Hover Info Tooltip Bar */}
      <div
        style={{
          marginTop: 14,
          padding: "10px 16px",
          background: "#080e18",
          border: "1px solid var(--line)",
          borderRadius: 8,
          fontSize: 12,
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontFamily: "var(--mono)",
        }}
      >
        {hoveredCell ? (
          <div>
            <span style={{ color: "#38bdf8", fontWeight: 700 }}>{hoveredCell.row.name}</span>
            <span style={{ color: "#64748b" }}> &bull; Metric: </span>
            <span style={{ color: "#fff" }}>{hoveredCell.col.label} = {hoveredCell.val.toFixed(3)}</span>
            <span style={{ color: "#64748b" }}> (Guardrail Threshold: {hoveredCell.col.threshold}) &bull; </span>
            <span
              style={{
                color: hoveredCell.val > hoveredCell.col.threshold ? "#ef4444" : "#10b981",
                fontWeight: 600,
              }}
            >
              {hoveredCell.val > hoveredCell.col.threshold ? "EXCEEDS THRESHOLD (TIGHT/HIGH)" : "PASSED SAFETY GATE"}
            </span>
          </div>
        ) : (
          <div style={{ color: "#64748b" }}>
            Hover over any cell in the heatmap matrix to inspect fine-grained compiler metrics and threshold verdicts.
          </div>
        )}

        <div className="heatmap-legend">
          <span>Cold (0.0)</span>
          <div className="legend-bar" />
          <span>Hotspot (1.0)</span>
        </div>
      </div>
    </div>
  );
};
