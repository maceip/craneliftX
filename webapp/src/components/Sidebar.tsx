import React from "react";
import { BotAvatar } from "bot-avatars";
import {
  Activity,
  Layers,
  Zap,
  Split,
  Flame,
  Terminal,
  Cpu,
  RefreshCw,
  FolderOpen,
  Sparkles,
} from "lucide-react";

interface SidebarProps {
  activeTab: string;
  setActiveTab: (tab: any) => void;
  binaryName: string | null;
  currentStage: string;
  busy: boolean;
  functionCount: number;
  dualCount: number;
  hasHeatmap: boolean;
  onReset: () => void;
  onLoadSample: () => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeTab,
  setActiveTab,
  binaryName,
  currentStage,
  busy,
  functionCount,
  dualCount,
  hasHeatmap,
  onReset,
  onLoadSample,
}) => {
  const navItems = [
    {
      id: "overview",
      label: "Executive Overview",
      icon: Layers,
      badge: "ALL",
      color: "#38bdf8",
    },
    {
      id: "voronoi",
      label: "Voronoi Topology",
      icon: Activity,
      badge: functionCount ? `${functionCount}` : undefined,
      color: "#38bdf8",
    },
    {
      id: "matrix",
      label: "Where to Put It",
      icon: Split,
      badge: functionCount ? `${functionCount}` : undefined,
      color: "#f59e0b",
    },
    {
      id: "dual",
      label: "Side-by-Side Dual Run",
      icon: Zap,
      badge: dualCount ? `${dualCount}` : undefined,
      color: "#10b981",
    },
    {
      id: "heatmap",
      label: "React Heatmap Matrix",
      icon: Flame,
      badge: hasHeatmap ? "READY" : undefined,
      color: "#f43f5e",
    },
    {
      id: "logs",
      label: "Live Event Stream",
      icon: Terminal,
      badge: undefined,
      color: "#94a3b8",
    },
  ];

  return (
    <aside className="wm-sidebar">
      {/* Brand Header */}
      <div className="wm-sidebar-header">
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <div className="wm-logo-icon">
            <Cpu size={20} color="#38bdf8" />
          </div>
          <div>
            <div className="wm-brand-name">CRANELIFT-X</div>
            <div className="wm-brand-sub">LIFT & DROP ENGINE</div>
          </div>
        </div>

        {/* Bot Avatar Ghost */}
        <div title="Recompilation Agent Ghost">
          <BotAvatar type="ghost" state={busy ? "working" : "default"} size={36} />
        </div>
      </div>

      {/* Project Selector Box */}
      <div className="wm-project-box">
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 4 }}>
          <span style={{ fontSize: 10, textTransform: "uppercase", letterSpacing: "0.06em", color: "#64748b" }}>
            Target Binary
          </span>
          <span
            style={{
              fontSize: 10,
              padding: "1px 5px",
              borderRadius: 3,
              background: busy ? "rgba(56, 189, 248, 0.15)" : "rgba(16, 185, 129, 0.15)",
              color: busy ? "#38bdf8" : "#10b981",
              fontFamily: "var(--mono)",
            }}
          >
            {busy ? "ANALYZING" : binaryName ? "MOUNTED" : "READY"}
          </span>
        </div>
        <div style={{ fontSize: 13, fontWeight: 700, color: "#fff", fontFamily: "var(--mono)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {binaryName || "sample_network.o"}
        </div>
        <div style={{ fontSize: 11, color: "#94a3b8", marginTop: 2 }}>
          Mach-O / ELF x86_64 &rarr; Pulley
        </div>
      </div>

      {/* Navigation Links */}
      <nav className="wm-nav-list">
        <div className="wm-nav-section-title">ENGINE SURFACES</div>
        {navItems.map((item) => {
          const isActive = activeTab === item.id;
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              className={`wm-nav-item ${isActive ? "active" : ""}`}
              onClick={() => setActiveTab(item.id)}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <Icon size={16} color={isActive ? item.color : "#94a3b8"} />
                <span>{item.label}</span>
              </div>
              {item.badge && (
                <span
                  className="wm-nav-badge"
                  style={{
                    color: isActive ? item.color : "#94a3b8",
                    borderColor: isActive ? `rgba(56, 189, 248, 0.3)` : "transparent",
                  }}
                >
                  {item.badge}
                </span>
              )}
            </button>
          );
        })}
      </nav>

      {/* Quick Action Button */}
      <div style={{ padding: "12px 14px", borderTop: "1px solid var(--wm-border)" }}>
        <button
          className="btn-sample"
          style={{ width: "100%", justifyContent: "center", fontSize: 12 }}
          onClick={onLoadSample}
          disabled={busy}
        >
          <Sparkles size={14} />
          <span>⚡ Run Bundled Demo</span>
        </button>
      </div>

      {/* Sidebar Footer: System Telemetry */}
      <div className="wm-sidebar-footer">
        <div className="wm-telemetry-row">
          <span className="k">TS Compiler</span>
          <span className="v" style={{ color: "#38bdf8" }}>esbuild (Go)</span>
        </div>
        <div className="wm-telemetry-row">
          <span className="k">Drop Target</span>
          <span className="v" style={{ color: "#10b981" }}>pulley64 (VM)</span>
        </div>
        <div className="wm-telemetry-row">
          <span className="k">Engine Stage</span>
          <span className="v">{currentStage.toUpperCase()}</span>
        </div>

        {binaryName && (
          <button className="wm-reset-btn" onClick={onReset}>
            <RefreshCw size={12} />
            <span>Reset Analysis</span>
          </button>
        )}
      </div>
    </aside>
  );
};
