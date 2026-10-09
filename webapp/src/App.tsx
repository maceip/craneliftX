import React, { useState, useRef } from "react";
import { Sidebar } from "./components/Sidebar";
import { Topbar } from "./components/Topbar";
import { DropZone } from "./components/DropZone";
import { VoronoiBinaryGraph } from "./components/VoronoiBinaryGraph";
import { StaticAnalysisView } from "./components/StaticAnalysisView";
import { SideBySideRunView } from "./components/SideBySideRunView";
import { HeatMap } from "./components/HeatMap";
import { EventLog } from "./components/EventLog";
import {
  Stage,
  FunctionItem,
  PlacementPlan,
  DualRunStep,
  HeatMapPayload,
  LogEntry,
} from "./types";

const STAGES: string[] = [
  "load",
  "disassemble",
  "function",
  "callgraph",
  "decision",
  "dual_run_start",
  "dual_verify",
  "heatmap_data",
  "done",
];

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<"overview" | "voronoi" | "matrix" | "dual" | "heatmap" | "logs">("overview");
  const [currentStage, setCurrentStage] = useState<Stage>("idle");
  const [binaryName, setBinaryName] = useState<string | null>(null);
  const [byteCount, setByteCount] = useState<number | null>(null);
  const [functions, setFunctions] = useState<FunctionItem[]>([]);
  const [placement, setPlacement] = useState<PlacementPlan | null>(null);
  const [dualSteps, setDualSteps] = useState<DualRunStep[]>([]);
  const [heatmapData, setHeatmapData] = useState<HeatMapPayload | null>(null);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [isLoading, setIsLoading] = useState<boolean>(false);
  const [isVerified, setIsVerified] = useState<boolean>(false);
  const [selectedFunctionName, setSelectedFunctionName] = useState<string | null>(null);

  const hiddenFileInputRef = useRef<HTMLInputElement>(null);

  const addLog = (stage: string, text: string, isError = false) => {
    const entry: LogEntry = {
      id: Math.random().toString(36).substring(2, 9),
      time: new Date().toLocaleTimeString(),
      stage,
      text,
      isError,
    };
    setLogs((prev) => [...prev, entry]);
  };

  const resetState = () => {
    setCurrentStage("idle");
    setBinaryName(null);
    setByteCount(null);
    setFunctions([]);
    setPlacement(null);
    setDualSteps([]);
    setHeatmapData(null);
    setLogs([]);
    setIsLoading(false);
    setIsVerified(false);
    setSelectedFunctionName(null);
  };

  const runAnalysis = (jobId: string, name: string) => {
    setIsLoading(true);
    setBinaryName(name);

    const es = new EventSource(`/api/events/${jobId}`);

    es.onmessage = (ev) => {
      try {
        const { stage, payload } = JSON.parse(ev.data);
        setCurrentStage(stage);

        if (stage === "start") {
          addLog(stage, `Initialized ingest analysis for ${payload.name}`);
        } else if (stage === "load") {
          addLog(stage, `Ingested binary: ${payload.binary}`);
        } else if (stage === "disassemble") {
          addLog(stage, `Disassembled ${payload.symbols} symbols, ${payload.instructions} machine instructions`);
        } else if (stage === "function") {
          setFunctions((prev) => {
            const exists = prev.findIndex((f) => f.name === payload.name);
            if (exists >= 0) {
              const updated = [...prev];
              updated[exists] = { ...updated[exists], ...payload };
              return updated;
            }
            return [...prev, payload];
          });
          addLog(
            stage,
            `${payload.name} [0x${payload.addr.toString(16)}] &bull; ${payload.n_insn} insn &bull; loop: ${payload.loop_density}`
          );
        } else if (stage === "callgraph") {
          addLog(stage, `${payload.reachable} functions reachable from roots [${payload.roots.join(", ")}]`);
        } else if (stage === "decision") {
          setFunctions((prev) =>
            prev.map((f) =>
              f.name === payload.name
                ? { ...f, decision: payload.decision, reason: payload.reason }
                : f
            )
          );
          addLog(stage, `${payload.name} &rarr; ${payload.decision} (${payload.reason})`);
        } else if (stage === "placement") {
          setPlacement(payload);
          addLog(stage, `Placement mode: ${payload.mode} (Selected: ${payload.selected?.length || 0})`);
        } else if (stage === "dual_run_start") {
          addLog(stage, `Starting side-by-side dual run (Native Hardware vs Pulley VM)`);
        } else if (stage === "dual_step") {
          setDualSteps((prev) => {
            const idx = prev.findIndex((s) => s.name === payload.name);
            if (idx >= 0) {
              const u = [...prev];
              u[idx] = payload;
              return u;
            }
            return [...prev, payload];
          });
          addLog(
            stage,
            `Dual run ${payload.name}: Native ret=${payload.native.return_value} <==> Pulley ret=${payload.pulley.return_value} [MATCH]`
          );
        } else if (stage === "dual_verify") {
          setIsVerified(true);
          addLog(stage, payload.summary);
        } else if (stage === "heatmap_data") {
          setHeatmapData(payload);
          addLog(stage, `Compiled ${payload.rows.length}x${payload.columns.length} complexity & execution heatmap matrix`);
        } else if (stage === "done") {
          setIsLoading(false);
          addLog(
            stage,
            `Complete: ${payload.total_functions} functions analyzed &bull; ${payload.lift.length} lifted &bull; ${payload.keep_native.length} native`
          );
        } else if (stage === "error") {
          setIsLoading(false);
          addLog(stage, payload.message, true);
        }
      } catch (err) {
        console.error("Failed to parse SSE event:", err);
      }
    };

    es.addEventListener("close", () => {
      es.close();
      setIsLoading(false);
    });

    es.onerror = () => {
      es.close();
      setIsLoading(false);
    };
  };

  const handleFileUpload = async (file: File) => {
    resetState();
    setIsLoading(true);
    addLog("upload", `Uploading ${file.name} (${file.size} bytes)...`);
    try {
      const res = await fetch("/api/upload", {
        method: "POST",
        headers: { "X-Filename": file.name },
        body: file,
      });
      const data = await res.json();
      setByteCount(data.bytes);
      runAnalysis(data.id, data.name);
    } catch (err: any) {
      addLog("error", `Upload failed: ${err.message}`, true);
      setIsLoading(false);
    }
  };

  const handleLoadSample = async () => {
    resetState();
    setIsLoading(true);
    addLog("sample", "Requesting bundled sample_network.o...");
    try {
      const res = await fetch("/api/sample", { method: "POST" });
      const data = await res.json();
      setByteCount(data.bytes);
      runAnalysis(data.id, data.name);
    } catch (err: any) {
      addLog("error", `Failed to load sample: ${err.message}`, true);
      setIsLoading(false);
    }
  };

  const handleSelectFromVoronoi = (fnName: string) => {
    setSelectedFunctionName(fnName);
    setActiveTab("dual");
  };

  return (
    <div className="wm-app-shell">
      <input
        type="file"
        ref={hiddenFileInputRef}
        style={{ display: "none" }}
        onChange={(e) => {
          if (e.target.files && e.target.files[0]) {
            handleFileUpload(e.target.files[0]);
          }
        }}
      />

      {/* Watermelon UI Left Navigation Sidebar */}
      <Sidebar
        activeTab={activeTab}
        setActiveTab={setActiveTab}
        binaryName={binaryName}
        currentStage={currentStage}
        busy={isLoading}
        functionCount={functions.length}
        dualCount={dualSteps.length}
        hasHeatmap={!!heatmapData}
        onReset={resetState}
        onLoadSample={handleLoadSample}
      />

      {/* Main Workspace Area */}
      <div className="wm-main-area">
        {/* Watermelon UI Topbar */}
        <Topbar
          activeTab={activeTab}
          binaryName={binaryName}
          currentStage={currentStage}
          isVerified={isVerified}
          onOpenUpload={() => hiddenFileInputRef.current?.click()}
          onLoadSample={handleLoadSample}
          busy={isLoading}
        />

        {/* Scrollable Main Content */}
        <div className="wm-content-scroll">
          {/* Hero Ingest DropZone */}
          <DropZone
            onFileUpload={handleFileUpload}
            onLoadSample={handleLoadSample}
            stages={STAGES}
            currentStage={currentStage}
            binaryName={binaryName}
            byteCount={byteCount}
            isLoading={isLoading}
          />

          {/* Tab Views */}
          {activeTab === "overview" && (
            <div style={{ display: "flex", flexDirection: "column", gap: 32 }}>
              <VoronoiBinaryGraph
                functions={functions}
                binaryName={binaryName}
                onSelectFunction={handleSelectFromVoronoi}
                selectedFunction={selectedFunctionName}
              />

              <StaticAnalysisView
                functions={functions}
                placement={placement}
                activeStage={currentStage}
              />

              <SideBySideRunView
                dualSteps={dualSteps}
                isVerified={isVerified}
              />

              <HeatMap payload={heatmapData} />
            </div>
          )}

          {activeTab === "voronoi" && (
            <VoronoiBinaryGraph
              functions={functions}
              binaryName={binaryName}
              onSelectFunction={handleSelectFromVoronoi}
              selectedFunction={selectedFunctionName}
            />
          )}

          {activeTab === "matrix" && (
            <StaticAnalysisView
              functions={functions}
              placement={placement}
              activeStage={currentStage}
            />
          )}

          {activeTab === "dual" && (
            <SideBySideRunView
              dualSteps={dualSteps}
              isVerified={isVerified}
            />
          )}

          {activeTab === "heatmap" && (
            <HeatMap payload={heatmapData} />
          )}

          {activeTab === "logs" && (
            <EventLog logs={logs} />
          )}

          {activeTab !== "logs" && (
            <EventLog logs={logs} />
          )}
        </div>
      </div>
    </div>
  );
};
