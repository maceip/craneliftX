import React, { useState, useEffect } from "react";
import { ShieldCheck, Cpu, Play, Upload } from "lucide-react";

interface TopbarProps {
  activeTab: string;
  binaryName: string | null;
  currentStage: string;
  isVerified: boolean;
  onOpenUpload: () => void;
  onLoadSample: () => void;
  busy: boolean;
}

export const Topbar: React.FC<TopbarProps> = ({
  activeTab,
  binaryName,
  currentStage,
  isVerified,
  onOpenUpload,
  onLoadSample,
  busy,
}) => {
  const [time, setTime] = useState("");
  const [cycles, setCycles] = useState(14820);

  useEffect(() => {
    const update = () => {
      setTime(new Date().toLocaleTimeString());
      setCycles((prev) => prev + Math.floor(Math.random() * 64) + 12);
    };
    update();
    const interval = setInterval(update, 1000);
    return () => clearInterval(interval);
  }, []);

  const tabLabels: Record<string, string> = {
    overview: "Executive Overview",
    voronoi: "Voronoi Topology (@visx)",
    matrix: "Where to Put It",
    dual: "Side-by-Side Dual Run",
    heatmap: "React Heatmap Matrix",
    logs: "Live Event Stream",
  };

  return (
    <header className="wm-topbar">
      {/* Breadcrumb Path */}
      <div className="wm-breadcrumb">
        <span className="crumb">CraneliftX</span>
        <span className="sep">/</span>
        <span className="crumb">{binaryName || "sample_network.o"}</span>
        <span className="sep">/</span>
        <span className="crumb current">{tabLabels[activeTab] || activeTab}</span>
      </div>

      {/* Right Controls and Status Pills */}
      <div className="wm-topbar-actions">
        {isVerified && (
          <div className="wm-status-pill verified">
            <ShieldCheck size={14} color="#10b981" />
            <span>&Delta; = 0 Equivalence Verified</span>
          </div>
        )}

        <div className="wm-status-pill">
          <Cpu size={14} color="#38bdf8" />
          <span className="tabular">{cycles.toLocaleString()} cycles</span>
        </div>

        <div className="wm-status-pill">
          <span
            style={{
              width: 7,
              height: 7,
              borderRadius: "50%",
              background: busy ? "#38bdf8" : currentStage === "done" ? "#10b981" : "#64748b",
              display: "inline-block",
            }}
          />
          <span style={{ textTransform: "uppercase" }}>{currentStage}</span>
        </div>

        <button
          className="btn-secondary"
          style={{ fontSize: 12, padding: "5px 12px", display: "inline-flex", alignItems: "center", gap: 6 }}
          onClick={onOpenUpload}
          disabled={busy}
        >
          <Upload size={13} />
          <span>Upload</span>
        </button>

        <button
          className="btn-primary"
          style={{ fontSize: 12, padding: "5px 14px", display: "inline-flex", alignItems: "center", gap: 6 }}
          onClick={onLoadSample}
          disabled={busy}
        >
          <Play size={13} />
          <span>Demo</span>
        </button>
      </div>
    </header>
  );
};
