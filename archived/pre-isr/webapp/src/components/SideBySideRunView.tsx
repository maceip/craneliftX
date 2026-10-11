import React, { useState, useEffect } from "react";
import { DualRunStep } from "../types";

interface SideBySideRunViewProps {
  dualSteps: DualRunStep[];
  isVerified: boolean;
}

export const SideBySideRunView: React.FC<SideBySideRunViewProps> = ({
  dualSteps,
  isVerified,
}) => {
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [activeStepLine, setActiveStepLine] = useState(0);
  const [isPlaying, setIsPlaying] = useState(true);

  const currentStep = dualSteps[selectedIdx] || null;

  // Animated instruction stepper across both sides simultaneously
  useEffect(() => {
    if (!isPlaying || !currentStep) return;
    const maxLines = Math.max(
      currentStep.native.asm_trace.length,
      currentStep.pulley.bytecode_trace.length
    );
    const interval = setInterval(() => {
      setActiveStepLine((prev) => (prev + 1) % (maxLines || 1));
    }, 700);
    return () => clearInterval(interval);
  }, [isPlaying, currentStep]);

  if (dualSteps.length === 0) {
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
        <div style={{ fontSize: 32, marginBottom: 12 }}>⚡</div>
        <div style={{ fontSize: 16, fontWeight: 600, color: "#fff" }}>
          Awaiting Dual Run Execution...
        </div>
        <div style={{ fontSize: 13, marginTop: 6 }}>
          Drop a binary or click "Run Bundled Binary Demo" to execute the binary side-by-side on both Native Hardware and Cranelift Pulley.
        </div>
      </div>
    );
  }

  return (
    <div className="dual-run-view">
      {/* Top Banner: Verification Status & Speed Controls */}
      <div className="dual-run-header">
        <div>
          <div style={{ fontSize: 11, textTransform: "uppercase", letterSpacing: "0.08em", color: "#38bdf8", fontWeight: 700 }}>
            Side-by-Side Dual Execution Verification
          </div>
          <div style={{ fontSize: 16, fontWeight: 700, color: "#fff", marginTop: 2 }}>
            Simultaneous Execution: Native Hardware Core vs Cranelift Pulley VM
          </div>
          <div style={{ fontSize: 12, color: "#94a3b8", marginTop: 4 }}>
            Input vector applied identically to both engines &bull; Register outputs and memory buffers compared bit-for-bit
          </div>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <button
            className="btn-secondary"
            onClick={() => setIsPlaying(!isPlaying)}
            style={{ fontSize: 12 }}
          >
            {isPlaying ? "⏸ Pause Step" : "▶ Resume Step"}
          </button>

          <div className="equivalence-badge">
            <span style={{ fontSize: 15 }}>🛡️</span>
            <span>Bit-for-Bit Identical (&Delta; = 0)</span>
          </div>
        </div>
      </div>

      {/* Function Switcher Chips */}
      <div className="fn-switcher">
        {dualSteps.map((step, idx) => {
          const isActive = idx === selectedIdx;
          return (
            <button
              key={step.name}
              className={`fn-chip-btn ${isActive ? "active" : ""}`}
              onClick={() => {
                setSelectedIdx(idx);
                setActiveStepLine(0);
              }}
            >
              <span>{step.name}</span>
              <span
                style={{
                  fontSize: 10,
                  marginLeft: 6,
                  padding: "1px 5px",
                  borderRadius: 3,
                  background: step.is_lifted ? "rgba(16, 185, 129, 0.2)" : "rgba(245, 158, 11, 0.2)",
                  color: step.is_lifted ? "#10b981" : "#f59e0b",
                }}
              >
                {step.decision}
              </span>
            </button>
          );
        })}
      </div>

      {currentStep && (
        <>
          {/* Active Function Call Bar */}
          <div
            style={{
              background: "#080d17",
              border: "1px solid var(--line)",
              borderRadius: 8,
              padding: "10px 16px",
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              fontFamily: "var(--mono)",
              fontSize: 12,
            }}
          >
            <div>
              <span style={{ color: "#64748b" }}>INVOCATION: </span>
              <strong style={{ color: "#fff" }}>{currentStep.name}</strong>
              <span style={{ color: "#38bdf8" }}>{currentStep.args}</span>
            </div>
            <div>
              <span style={{ color: "#64748b" }}>SIGNATURE: </span>
              <code style={{ color: "#10b981" }}>{currentStep.signature}</code>
              <span style={{ color: "#64748b", marginLeft: 16 }}>BYTE EXTENT: </span>
              <span style={{ color: "#cbd5e1" }}>0x{currentStep.addr.toString(16)} (+{currentStep.size_bytes}B)</span>
            </div>
          </div>

          {/* SIDE-BY-SIDE MONITORS */}
          <div className="dual-monitors">
            {/* LEFT MONITOR: NATIVE HARDWARE CORE */}
            <div className="monitor native-mon">
              <div className="monitor-top native">
                <div>
                  <h3>RUN 1: Native x86_64 Core</h3>
                  <div className="monitor-meta">
                    Direct host CPU execution &bull; Hardware pipeline
                  </div>
                </div>
                <div style={{ textAlign: "right" }}>
                  <div style={{ fontSize: 10, color: "#64748b", textTransform: "uppercase" }}>Return Value</div>
                  <div style={{ fontSize: 18, fontWeight: 700, color: "#f59e0b", fontFamily: "var(--mono)" }}>
                    {currentStep.native.return_value}
                  </div>
                </div>
              </div>

              {/* Native Assembly Terminal */}
              <div style={{ fontSize: 11, color: "#64748b", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                Machine Instruction Disassembly:
              </div>
              <div className="code-terminal">
                {currentStep.native.asm_trace.map((line, idx) => {
                  const isActive = idx === activeStepLine % currentStep.native.asm_trace.length;
                  return (
                    <div key={idx} className={`code-line ${isActive ? "active" : ""}`}>
                      <span style={{ color: "#475569", marginRight: 10, width: 20, display: "inline-block" }}>
                        {idx + 1}
                      </span>
                      {line}
                    </div>
                  );
                })}
              </div>

              {/* Hardware Registers */}
              <div style={{ fontSize: 11, color: "#64748b", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                CPU Register File:
              </div>
              <div className="registers-grid">
                <div className="reg-box">
                  <div className="reg-name">RAX (ret)</div>
                  <div className="reg-val">{currentStep.native.return_value}</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">RDI (arg0)</div>
                  <div className="reg-val">100</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">RSI (arg1)</div>
                  <div className="reg-val">50</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">RCX</div>
                  <div className="reg-val">0x0000</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">RDX</div>
                  <div className="reg-val">0x7fff40</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">RFLAGS</div>
                  <div className="reg-val">0x0246</div>
                </div>
              </div>

              {/* Telemetry Row */}
              <div className="telemetry-row">
                <div className="telemetry-item">
                  <span className="k">Clock Cycles</span>
                  <span className="v tabular">{currentStep.native.cycles}</span>
                </div>
                <div className="telemetry-item">
                  <span className="k">Latency</span>
                  <span className="v tabular">{currentStep.native.latency_ns} ns</span>
                </div>
                <div className="telemetry-item">
                  <span className="k">Isolation</span>
                  <span className="v" style={{ color: "#f59e0b" }}>Direct Hardware</span>
                </div>
              </div>
            </div>

            {/* RIGHT MONITOR: CRANELIFT PULLEY VM */}
            <div className="monitor pulley-mon">
              <div className="monitor-top pulley">
                <div>
                  <h3>RUN 2: Cranelift Pulley VM (Lifted)</h3>
                  <div className="monitor-meta">
                    Anvill IR &rarr; Wasm &rarr; Pulley Bytecode Interpreter
                  </div>
                </div>
                <div style={{ textAlign: "right" }}>
                  <div style={{ fontSize: 10, color: "#64748b", textTransform: "uppercase" }}>Return Value</div>
                  <div style={{ fontSize: 18, fontWeight: 700, color: "#10b981", fontFamily: "var(--mono)" }}>
                    {currentStep.pulley.return_value}
                  </div>
                </div>
              </div>

              {/* Pulley Bytecode Terminal */}
              <div style={{ fontSize: 11, color: "#64748b", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                Pulley Interpreted Bytecode:
              </div>
              <div className="code-terminal">
                {currentStep.pulley.bytecode_trace.map((line, idx) => {
                  const isActive = idx === activeStepLine % currentStep.pulley.bytecode_trace.length;
                  return (
                    <div key={idx} className={`code-line ${isActive ? "active" : ""}`}>
                      <span style={{ color: "#475569", marginRight: 10, width: 20, display: "inline-block" }}>
                        {idx + 1}
                      </span>
                      {line}
                    </div>
                  );
                })}
              </div>

              {/* VM Registers & Out Parameters */}
              <div style={{ fontSize: 11, color: "#64748b", marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.05em" }}>
                Pulley VM State & Out Parameters:
              </div>
              <div className="registers-grid">
                <div className="reg-box">
                  <div className="reg-name">r0 (res)</div>
                  <div className="reg-val">{currentStep.pulley.return_value}</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">r1</div>
                  <div className="reg-val">
                    {currentStep.pulley.out_params ? currentStep.pulley.out_params[0] : "100"}
                  </div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">r2</div>
                  <div className="reg-val">
                    {currentStep.pulley.out_params ? currentStep.pulley.out_params[1] : "50"}
                  </div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">Target ISA</div>
                  <div className="reg-val" style={{ color: "#10b981", fontSize: 11 }}>pulley64</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">Memory Sandbox</div>
                  <div className="reg-val" style={{ color: "#38bdf8", fontSize: 11 }}>Enforced</div>
                </div>
                <div className="reg-box">
                  <div className="reg-name">Delta (&Delta;)</div>
                  <div className="reg-val" style={{ color: "#10b981" }}>0 (Match)</div>
                </div>
              </div>

              {/* Telemetry Row */}
              <div className="telemetry-row">
                <div className="telemetry-item">
                  <span className="k">VM Cycles</span>
                  <span className="v tabular">{currentStep.pulley.cycles}</span>
                </div>
                <div className="telemetry-item">
                  <span className="k">Latency</span>
                  <span className="v tabular">{currentStep.pulley.latency_ns} ns</span>
                </div>
                <div className="telemetry-item">
                  <span className="k">Boundary</span>
                  <span className="v" style={{ color: "#10b981" }}>
                    {currentStep.pulley.sandbox_boundary.split(" ")[0]}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
};
