import React from "react";

export function TransportIcon({
  kind,
}: {
  kind: "info" | "rewind" | "play" | "pause" | "stop" | "close";
}) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      {kind !== "close" && (
        <circle
          cx="12"
          cy="12"
          r="9.5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.7"
        />
      )}
      {kind === "info" && (
        <g fill="currentColor">
          <circle cx="12" cy="7.5" r="1.2" />
          <path d="M10.5 10.5h2.2v6h1.5V18H9.8v-1.5h1.3V12h-.6z" />
        </g>
      )}
      {kind === "rewind" && (
        <path d="m12 7-6 5 6 5V7Zm6 0-6 5 6 5V7Z" fill="currentColor" />
      )}
      {kind === "play" && <path d="m9 7 8 5-8 5V7Z" fill="currentColor" />}
      {kind === "pause" && (
        <path d="M8 7h3v10H8zm5 0h3v10h-3z" fill="currentColor" />
      )}
      {kind === "stop" && (
        <rect x="8" y="8" width="8" height="8" rx=".7" fill="currentColor" />
      )}
      {kind === "close" && (
        <path
          d="m7 7 10 10M17 7 7 17"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
        />
      )}
    </svg>
  );
}
