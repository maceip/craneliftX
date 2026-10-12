import { useEffect, useMemo, useRef, useState } from "react";
import { LedgerEvent, Run, terminal } from "./pipeline";

export const WALKTHROUGH_MS = 5200;
export type Walkthrough = { progress: number; phase: number; playing: boolean };

// This clock describes an illustration, never elapsed engine time or completion.
// It survives pause, but a newly selected card always receives its first frame.
export function useWalkthrough(key: string, active: boolean): Walkthrough {
  const clock = useRef({ key, elapsed: 0 });
  const [frame, setFrame] = useState({ key, progress: 0 });
  if (clock.current.key !== key) clock.current = { key, elapsed: 0 };
  useEffect(() => {
    if (!active) return;
    let raf = 0,
      previous = performance.now(),
      painted = 0;
    const tick = (now: number) => {
      // A queued rAF timestamp can precede effect setup in the same frame.
      clock.current.elapsed = Math.min(
        WALKTHROUGH_MS,
        clock.current.elapsed + Math.max(0, now - previous),
      );
      previous = now;
      if (now - painted >= 60 || clock.current.elapsed === WALKTHROUGH_MS) {
        painted = now;
        setFrame({ key, progress: clock.current.elapsed / WALKTHROUGH_MS });
      }
      if (clock.current.elapsed < WALKTHROUGH_MS)
        raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [key, active]);
  const progress = frame.key === key ? frame.progress : 0;
  return {
    progress,
    phase: Math.min(4, Math.floor(progress * 5)),
    playing: active && progress < 1,
  };
}

export type IllustrationEvidence = {
  syscalls: LedgerEvent[];
  blocks: any[];
  progress: LedgerEvent[];
  invalidations: LedgerEvent[];
  sampled: boolean;
};
const empty = (): IllustrationEvidence => ({
  syscalls: [],
  blocks: [],
  progress: [],
  invalidations: [],
  sampled: false,
});

function collect(data: IllustrationEvidence, event: LedgerEvent) {
  if (
    event.type === "syscall_completed" &&
    ["read", "write", "accept", "accept4", "connect"].includes(
      event.detail?.name,
    )
  )
    data.syscalls = [
      ...data.syscalls.filter((e) => e.seq !== event.seq),
      event,
    ].slice(-24);
  if (event.type === "cache_invalidation")
    data.invalidations = [
      ...data.invalidations.filter((e) => e.seq !== event.seq),
      event,
    ].slice(-8);
  if (event.type === "progress")
    data.progress = [
      ...data.progress.filter((e) => e.seq !== event.seq),
      event,
    ].slice(-80);
  if (event.type === "block_execution_totals") {
    const blocks = new Map(data.blocks.map((b) => [b.block_id, b]));
    for (const block of event.blocks ?? [])
      blocks.set(block.block_id, { ...block, seq: event.seq });
    // Keep the busiest recorded translations, not an unbounded per-step trace.
    data.blocks = [...blocks.values()]
      .sort((a, b) => b.rv64_steps - a.rv64_steps)
      .slice(0, 24);
  }
}

// Snapshots retain only 120 recent events. Read a bounded saved journal to recover
// meaningful TCP exchanges and hot blocks instead of illustrating only exit().
export function useIllustrationEvidence(run: Run | null) {
  const [saved, setSaved] = useState<{
    id: string;
    evidence: IllustrationEvidence;
  } | null>(null);
  const finished = !!run && terminal(run.status);
  useEffect(() => {
    if (!run || !finished) return;
    const controller = new AbortController(),
      id = run.id;
    void (async () => {
      const data = empty();
      try {
        const response = await fetch(`/api/pipeline/runs/${id}/journal`, {
          signal: controller.signal,
        });
        if (!response.ok || !response.body) return;
        const reader = response.body.getReader(),
          decoder = new TextDecoder();
        let pending = "",
          size = 0;
        try {
          while (true) {
            const { value, done } = await reader.read();
            if (done) break;
            size += value.length;
            if (size > 16 * 1024 * 1024) {
              data.sampled = true;
              break;
            }
            pending += decoder.decode(value, { stream: true });
            const lines = pending.split("\n");
            pending = lines.pop()!;
            for (const line of lines) if (line) collect(data, JSON.parse(line));
          }
        } finally {
          await reader.cancel();
        }
        if (!controller.signal.aborted) setSaved({ id, evidence: data });
      } catch {
        /* Snapshot evidence stays available if the journal is offline. */
      }
    })();
    return () => controller.abort();
  }, [run?.id, finished]);
  return useMemo(() => {
    const data =
      saved && saved.id === run?.id ? { ...saved.evidence } : empty();
    for (const event of run?.recent ?? []) collect(data, event);
    return data;
  }, [saved, run]);
}
