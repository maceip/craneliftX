import React, { useState, useEffect } from "react";
import { BotAvatar } from "bot-avatars";

interface HeaderProps {
  activeStage: string;
  binaryName: string | null;
  onReset: () => void;
  busy?: boolean;
}

export const Header: React.FC<HeaderProps> = ({
  activeStage,
  binaryName,
  onReset,
  busy = false,
}) => {
  const [time, setTime] = useState<string>("");
  const [cycles, setCycles] = useState<number>(14290);

  useEffect(() => {
    const update = () => {
      setTime(new Date().toLocaleTimeString());
      setCycles((prev) => prev + Math.floor(Math.random() * 85) + 12);
    };
    update();
    const timer = setInterval(update, 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <header className="header">
      <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
        <div style={{ flex: "none" }} title="Binary Recompiler Agent Ghost">
          <BotAvatar type="ghost" state={busy ? "working" : "default"} size={48} />
        </div>

        <div>
          <div className="brand-badge">
            <span className="dot" />
            <span>CRANELIFT-X // STATIC INGEST & HETEROGENEOUS DROP ENGINE</span>
          </div>
          <div className="title-row">
            <h1>Heterogeneous Binary Recompilation</h1>
            <span className="tagline">
              Ingest tracer static analysis &rarr; Keyed Placement &rarr; Cranelift Pulley Interpreter
            </span>
          </div>
        </div>
      </div>

      <div className="header-telemetry">
        <div className="telemetry-chip">
          <span className="label">TS Engine</span>
          <span className="val" style={{ color: "#38bdf8" }}>esbuild (Go)</span>
        </div>

        <div className="telemetry-chip">
          <span className="label">Kernel Cycles</span>
          <span className="val tabular">{cycles.toLocaleString()}</span>
        </div>

        <div className="telemetry-chip">
          <span className="label">Active State</span>
          <span
            className="val"
            style={{
              color: activeStage === "done" ? "#10b981" : activeStage === "idle" ? "#64748b" : "#38bdf8",
            }}
          >
            {activeStage.toUpperCase()}
          </span>
        </div>

        {binaryName && (
          <button className="btn-secondary" onClick={onReset} title="Load another binary">
            ↺ Reset
          </button>
        )}
      </div>
    </header>
  );
};
