import React, { useEffect, useRef } from "react";
import { LogEntry } from "../types";

interface EventLogProps {
  logs: LogEntry[];
}

export const EventLog: React.FC<EventLogProps> = ({ logs }) => {
  const logContainerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs]);

  return (
    <div style={{ marginTop: 24 }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 10,
        }}
      >
        <h3
          style={{
            margin: 0,
            fontSize: 13,
            textTransform: "uppercase",
            letterSpacing: "0.06em",
            color: "#94a3b8",
            display: "flex",
            alignItems: "center",
            gap: 8,
          }}
        >
          <span
            style={{
              width: 7,
              height: 7,
              borderRadius: "50%",
              background: "#38bdf8",
              display: "inline-block",
            }}
          />
          Live Event Stream &bull; Trace Log
        </h3>
        <span style={{ fontSize: 11, color: "#64748b", fontFamily: "var(--mono)" }}>
          {logs.length} events logged
        </span>
      </div>

      <div className="log-box" ref={logContainerRef}>
        {logs.map((log) => (
          <div key={log.id} className={`log-entry ${log.isError ? "err" : ""}`}>
            <span className="time tabular">[{log.time}]</span>
            <span className="stage">&lt;{log.stage}&gt;</span>
            <span style={{ color: log.isError ? "#f87171" : "#cbd5e1" }}>{log.text}</span>
          </div>
        ))}
        {logs.length === 0 && (
          <div style={{ color: "#475569", padding: "16px 0", fontStyle: "italic" }}>
            Awaiting telemetry events...
          </div>
        )}
      </div>
    </div>
  );
};
