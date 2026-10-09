import React, { useState, useRef } from "react";
import { ThinkingOrb } from "thinking-orbs";
import { Stage } from "../types";

interface DropZoneProps {
  onFileUpload: (file: File) => void;
  onLoadSample: () => void;
  stages: string[];
  currentStage: Stage;
  binaryName: string | null;
  byteCount: number | null;
  isLoading: boolean;
}

export const DropZone: React.FC<DropZoneProps> = ({
  onFileUpload,
  onLoadSample,
  stages,
  currentStage,
  binaryName,
  byteCount,
  isLoading,
}) => {
  const [isDragging, setIsDragging] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(true);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
  };

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
    if (e.dataTransfer.files && e.dataTransfer.files[0]) {
      onFileUpload(e.dataTransfer.files[0]);
    }
  };

  const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files && e.target.files[0]) {
      onFileUpload(e.target.files[0]);
    }
  };

  return (
    <div className="hero-card">
      <div
        className={`hero-drop ${isDragging ? "dragging" : ""}`}
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        onClick={() => !isLoading && fileInputRef.current?.click()}
      >
        <input
          type="file"
          ref={fileInputRef}
          style={{ display: "none" }}
          onChange={handleFileChange}
        />

        {isLoading ? (
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 14 }}>
            <ThinkingOrb state="searching" size={64} />
            <div style={{ fontSize: "16px", fontWeight: 700, color: "#38bdf8" }}>
              Ingesting & Tracing: <code>{binaryName}</code>
            </div>
            <div style={{ color: "#94a3b8", fontSize: "12px", fontFamily: "var(--mono)" }}>
              Capstone disassembly &bull; CFG recovery &bull; Keyed placement decision matrix
            </div>
          </div>
        ) : (
          <>
            <div style={{ fontSize: "16px", fontWeight: 700, marginBottom: "6px" }}>
              {binaryName ? (
                <span style={{ color: "#38bdf8" }}>
                  Active Binary: <code>{binaryName}</code> ({byteCount?.toLocaleString()} bytes)
                </span>
              ) : (
                "Drop a Native Binary (Mach-O or ELF x86-64) to Lift and Recompile"
              )}
            </div>

            <div style={{ color: "#94a3b8", fontSize: "13px" }}>
              Runs local Capstone disassembly &rarr; Remill AST/CFG generation &rarr; Keyed Placement &rarr; Cranelift drop
            </div>
          </>
        )}
      </div>

      <div className="hero-actions">
        <button
          className="btn-sample"
          onClick={onLoadSample}
          disabled={isLoading}
        >
          <span>⚡ Run Bundled Binary Demo</span>
          <code style={{ fontSize: "11px", opacity: 0.9 }}>(sample_network.o)</code>
        </button>

        <button
          className="btn-primary"
          onClick={() => fileInputRef.current?.click()}
          disabled={isLoading}
        >
          <span>📂 Select Local Object File</span>
        </button>
      </div>

      <div className="stage-tracker">
        {stages.map((stage) => {
          const isActive = currentStage === stage;
          const isDone =
            stages.indexOf(stage) < stages.indexOf(currentStage) || currentStage === "done";
          return (
            <span
              key={stage}
              className={`stage-pill ${isActive ? "active" : ""} ${isDone ? "done" : ""}`}
            >
              {isDone ? "✓" : isActive ? "►" : "○"} {stage}
            </span>
          );
        })}
      </div>
    </div>
  );
};
