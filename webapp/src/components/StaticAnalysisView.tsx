import React, { useState, useEffect } from "react";
import { FunctionItem, PlacementPlan } from "../types";

interface StaticAnalysisViewProps {
  functions: FunctionItem[];
  placement: PlacementPlan | null;
  activeStage: string;
}

export const StaticAnalysisView: React.FC<StaticAnalysisViewProps> = ({
  functions,
  placement,
  activeStage,
}) => {
  const [tickerOffset, setTickerOffset] = useState(0);

  useEffect(() => {
    const interval = setInterval(() => {
      setTickerOffset((prev) => (prev + 1) % 100);
    }, 80);
    return () => clearInterval(interval);
  }, []);

  const lifted = functions.filter((f) => f.decision === "LIFT");
  const keptNative = functions.filter((f) => f.decision === "KEEP_NATIVE");
  const pending = functions.filter((f) => !f.decision || f.decision === "PENDING");

  const opcodes = [
    "0x55 48 89 e5", "31 f7 69 cf b1 79", "37 9e 0f b7 c1", "c1 e9 10 31 c8",
    "5d c3 8d 04 37", "31 c0 83 fe 04", "72 4a 44 0f b6", "49 39 f3 75 f2",
    "44 89 02 44 89", "09 5b 5d c3"
  ];

  return (
    <div>
      {/* Live Moving Scanner Marquee */}
      <div className="scanner-banner">
        <div className="scanner-tag">
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: "50%",
              background: "#38bdf8",
              display: "inline-block",
              boxShadow: "0 0 8px #38bdf8",
            }}
          />
          Live Opcode Stream
        </div>
        <div className="scanner-stream">
          <div className="stream-inner">
            {opcodes.concat(opcodes).map((op, i) => (
              <span key={i} style={{ marginRight: 24, letterSpacing: "0.08em" }}>
                <span style={{ color: "#38bdf8", opacity: 0.7 }}>[0x{(0x60 + i * 16).toString(16)}]</span> {op}
              </span>
            ))}
          </div>
        </div>
        <div style={{ fontSize: 11, color: "#64748b", fontFamily: "var(--mono)", flex: "none" }}>
          STAGE: <strong style={{ color: "#fff" }}>{activeStage.toUpperCase()}</strong>
        </div>
      </div>

      {/* Decision Engine Architecture Banner */}
      <div
        style={{
          background: "linear-gradient(90deg, rgba(16, 185, 129, 0.12) 0%, rgba(56, 189, 248, 0.1) 50%, rgba(245, 158, 11, 0.12) 100%)",
          border: "1px solid var(--line)",
          borderRadius: 12,
          padding: "16px 20px",
          marginBottom: 24,
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          position: "relative",
          overflow: "hidden",
        }}
      >
        <div>
          <div style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.08em", color: "#38bdf8", fontWeight: 700 }}>
            Dynamic Placement Decision Engine
          </div>
          <div style={{ fontSize: 15, fontWeight: 700, color: "#fff", marginTop: 2 }}>
            Where to Put It: Safety Guardrails & Keyed Allocation
          </div>
          <div style={{ fontSize: 12, color: "#94a3b8", marginTop: 4 }}>
            Loops &gt; 0.45 density kept on hardware &bull; Memory & compute lifted to Pulley VM &bull; Placement Key:{" "}
            <code>{placement?.key_fingerprint || placement?.mode || "deterministic"}</code>
          </div>
        </div>

        <div style={{ display: "flex", gap: 12 }}>
          <div
            style={{
              padding: "6px 14px",
              background: "rgba(16, 185, 129, 0.2)",
              border: "1px solid rgba(16, 185, 129, 0.5)",
              borderRadius: 8,
              textAlign: "center",
            }}
          >
            <div style={{ fontSize: 10, color: "#10b981", textTransform: "uppercase" }}>Lifted</div>
            <div style={{ fontSize: 16, fontWeight: 700, color: "#fff", fontFamily: "var(--mono)" }}>
              {lifted.length}
            </div>
          </div>

          <div
            style={{
              padding: "6px 14px",
              background: "rgba(245, 158, 11, 0.2)",
              border: "1px solid rgba(245, 158, 11, 0.5)",
              borderRadius: 8,
              textAlign: "center",
            }}
          >
            <div style={{ fontSize: 10, color: "#f59e0b", textTransform: "uppercase" }}>Native</div>
            <div style={{ fontSize: 16, fontWeight: 700, color: "#fff", fontFamily: "var(--mono)" }}>
              {keptNative.length}
            </div>
          </div>
        </div>
      </div>

      {/* Split Routing Matrix: LIFT BAY vs KEEP NATIVE BAY */}
      <div className="routing-container">
        {/* LIFT TO PULLEY BAY */}
        <div className="decision-bay lift-bay">
          <div className="bay-header">
            <div className="bay-title lift">
              <span style={{ fontSize: 16 }}>⚡</span>
              <span>LIFT &rarr; Cranelift Pulley VM</span>
            </div>
            <span className="bay-count lift tabular">{lifted.length} Functions Selected</span>
          </div>
          <div style={{ fontSize: 12, color: "#94a3b8", marginBottom: 14 }}>
            Pure compute & unrolled routines lifted to portable bytecode. Isolated memory, CFI verified.
          </div>

          <div className="function-card-list">
            {lifted.map((fn) => (
              <FunctionCard key={fn.name} fn={fn} />
            ))}
            {lifted.length === 0 && (
              <div style={{ padding: "30px", textAlign: "center", color: "#64748b", fontStyle: "italic" }}>
                Awaiting tracer selection...
              </div>
            )}
          </div>
        </div>

        {/* KEEP NATIVE BAY */}
        <div className="decision-bay native-bay">
          <div className="bay-header">
            <div className="bay-title native">
              <span style={{ fontSize: 16 }}>🛡️</span>
              <span>KEEP NATIVE &rarr; Direct Silicon Core</span>
            </div>
            <span className="bay-count native tabular">{keptNative.length} Functions Kept</span>
          </div>
          <div style={{ fontSize: 12, color: "#94a3b8", marginBottom: 14 }}>
            Tight loops and high fan-out orchestration glue preserved on native silicon for zero latency loss.
          </div>

          <div className="function-card-list">
            {keptNative.map((fn) => (
              <FunctionCard key={fn.name} fn={fn} />
            ))}
            {keptNative.length === 0 && (
              <div style={{ padding: "30px", textAlign: "center", color: "#64748b", fontStyle: "italic" }}>
                Awaiting tracer selection...
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Pending / In-Flight Functions */}
      {pending.length > 0 && (
        <div style={{ marginTop: 20 }}>
          <h3 style={{ fontSize: 14, color: "#94a3b8", textTransform: "uppercase", letterSpacing: "0.05em" }}>
            Currently Ingesting / Evaluating ({pending.length})
          </h3>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))", gap: 10 }}>
            {pending.map((fn) => (
              <FunctionCard key={fn.name} fn={fn} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

const FunctionCard: React.FC<{ fn: FunctionItem }> = ({ fn }) => {
  return (
    <div className="function-card">
      <div className="card-top">
        <div className="fn-name">{fn.name}</div>
        <span className={`badge ${fn.decision || "PENDING"}`}>
          {fn.decision || "ANALYZING..."}
        </span>
      </div>

      <div className="fn-meta">
        0x{fn.addr.toString(16)} &bull; {fn.size_bytes}B &bull; {fn.n_insn} insn &bull;{" "}
        <code style={{ color: "#38bdf8" }}>{fn.signature}</code>
      </div>

      <div className="metric-bar-group">
        <div className="metric-bar">
          <span className="lbl">Loop Dens</span>
          <div className="track">
            <div
              className={`fill ${fn.loop_density > 0.45 ? "hot" : "normal"}`}
              style={{ width: `${Math.min(100, fn.loop_density * 100)}%` }}
            />
          </div>
          <span className="val tabular">{fn.loop_density.toFixed(3)}</span>
        </div>

        <div className="metric-bar">
          <span className="lbl">Call Frac</span>
          <div className="track">
            <div
              className={`fill ${fn.call_fraction > 0.4 ? "hot" : "normal"}`}
              style={{ width: `${Math.min(100, fn.call_fraction * 100)}%` }}
            />
          </div>
          <span className="val tabular">{fn.call_fraction.toFixed(3)}</span>
        </div>

        <div className="metric-bar">
          <span className="lbl">Net Score</span>
          <div className="track">
            <div
              className="fill normal"
              style={{ width: `${Math.min(100, fn.network_score * 100)}%` }}
            />
          </div>
          <span className="val tabular">{fn.network_score.toFixed(2)}</span>
        </div>
      </div>

      {fn.reason && <div className="card-reason">{fn.reason}</div>}
    </div>
  );
};
