import React, { useEffect, useState } from "react";
import { bytes, duration, Group, LedgerEvent, Run } from "../pipeline";

export function TranslationArrow() {
  return (
    <svg className="translation-arrow" viewBox="0 0 48 120" aria-hidden="true">
      <path className="beam-track" d="M4 60H40" />
      <path className="beam-dashes" d="M4 60H40" />
      <path className="beam-arrow" d="m34 54 6 6-6 6" />
    </svg>
  );
}

// Only accept an artifact for this exact block and RIP. Artifact receipts do
// not carry block_id; the engine emits both identifiers in its canonical name.
export function blockArtifact(
  run: Run | null,
  group: Group,
  extension: string,
) {
  const event = group.last_event;
  if (
    typeof event?.block_id !== "number" ||
    !/^0x[\da-f]+$/i.test(event.rip ?? "")
  )
    return null;
  const name = `block-${String(event.block_id).padStart(5, "0")}-${event.rip.slice(2)}.${extension}`;
  return (
    run?.recent.find((e) => e.type === "artifact" && e.name === name) ?? null
  );
}

function ArtifactPreview({
  runId,
  artifact,
  visible,
  kind,
}: {
  runId?: string;
  artifact: LedgerEvent | null;
  visible: boolean;
  kind: "clif" | "rv64";
}) {
  const [loaded, setLoaded] = useState<{
    key: string;
    text: string;
    error?: boolean;
  } | null>(null);
  const key =
    artifact && runId ? `${runId}/${artifact.name}/${artifact.sha256}` : "";
  useEffect(() => {
    if (!visible || !artifact || !runId) return;
    const controller = new AbortController();
    void (async () => {
      try {
        const response = await fetch(
          `/api/pipeline/runs/${runId}/artifacts/${encodeURIComponent(artifact.name)}`,
          { signal: controller.signal },
        );
        if (!response.ok || !response.body)
          throw new Error("Artifact unavailable");
        // Bound preview work, including when a large uploaded block is shown.
        // Full artifacts remain downloadable through Evidence.
        const limit = kind === "rv64" ? 48 : 8192;
        const reader = response.body.getReader();
        const preview = new Uint8Array(limit);
        let size = 0;
        try {
          while (size < limit) {
            const { done, value } = await reader.read();
            if (done) break;
            const chunk = value.subarray(0, limit - size);
            preview.set(chunk, size);
            size += chunk.length;
          }
        } finally {
          await reader.cancel();
        }
        const data = preview.subarray(0, size);
        const text =
          kind === "clif"
            ? new TextDecoder().decode(data).split("\n").slice(0, 7).join("\n")
            : Array.from({ length: Math.ceil(size / 8) }, (_, row) =>
                Array.from(data.subarray(row * 8, row * 8 + 8), (b) =>
                  b.toString(16).padStart(2, "0"),
                ).join(" "),
              ).join("\n");
        if (!controller.signal.aborted) setLoaded({ key, text });
      } catch {
        if (!controller.signal.aborted)
          setLoaded({
            key,
            text: "Artifact preview unavailable · open Evidence to retry",
            error: true,
          });
      }
    })();
    return () => controller.abort();
  }, [key, visible, kind]);
  const current = loaded?.key === key ? loaded : null;
  return (
    <div
      className="artifact-preview"
      data-artifact={artifact?.name}
      data-receipt={artifact?.seq}
    >
      <small>
        {kind === "clif" ? "Cranelift IR" : "RV64 machine bytes"}
        {artifact ? ` · ${bytes(artifact.bytes)}` : ""}
      </small>
      <pre
        data-testid={`${kind}-preview`}
        data-loaded={!!current && !current.error}
      >
        {current?.text ??
          (artifact
            ? "Reading saved artifact…"
            : "Awaiting this block’s artifact")}
      </pre>
      <small className="preview-note">
        {kind === "clif" ? "First 7 lines" : "First 48 bytes"} · full artifact
        in Evidence
      </small>
    </div>
  );
}

export function TranslationVisual({
  group,
  run,
  output,
  decoded,
  lowered,
  visible,
}: {
  group: Group;
  run: Run | null;
  output: LedgerEvent | null;
  decoded?: LedgerEvent | null;
  lowered?: LedgerEvent | null;
  visible: boolean;
}) {
  const lowering = group.id === "lower";
  const complete = output?.status === "completed";
  return (
    <div
      className="translation-visual"
      data-block-id={group.last_event?.block_id}
      data-source-seq={output?.seq}
    >
      <div className="translation-columns">
        {lowering ? (
          <div className="translation-source">
            <small>
              Source ·{" "}
              {decoded?.instructions
                ? `${decoded.instructions.length} x86 instructions`
                : "Awaiting decode"}
            </small>
            <ol className="semantic-lines">
              {decoded?.instructions?.map((i: any) => (
                <li key={i.ip}>{i.asm}</li>
              ))}
            </ol>
          </div>
        ) : (
          <ArtifactPreview
            runId={run?.id}
            artifact={blockArtifact(run, group, "clif")}
            visible={visible}
            kind="clif"
          />
        )}
        <TranslationArrow />
        <ArtifactPreview
          runId={run?.id}
          artifact={blockArtifact(run, group, lowering ? "clif" : "rv64")}
          visible={visible}
          kind={lowering ? "clif" : "rv64"}
        />
      </div>
      <small className="visual-caption">
        {lowering
          ? `${group.last_event?.rip ?? "No reached block"}${complete ? ` · Semantics lowered in ${duration(output.elapsed_ns)}` : ""}`
          : `${lowered?.status === "completed" ? bytes(lowered.clif_bytes) : "—"} IR → ${complete ? bytes(output.rv64_bytes) : "Awaiting RV64"} · ${output?.target ?? "Target not yet recorded"}`}
        {group.last_event?.block_id !== undefined
          ? ` · block ${group.last_event.block_id}`
          : ""}
      </small>
    </div>
  );
}
