import { useCallback, useEffect, useRef, useState } from "react";
import { Run, terminal } from "./pipeline";

// Reading time for an observed operation, never an execution delay or a
// simulated span duration. All cards and the gauge keep consuming live data.
const READING_MS = 1400;

export function useStageFocus(
  run: Run | null,
  freshRun: string | null,
  following: boolean,
  paused: boolean,
  select: (group: string) => void,
) {
  const latest = useRef(run);
  latest.current = run;
  const progress = useRef({ overview: false, index: 0, since: 0 });
  const [reviewing, setReviewing] = useState(false);
  const endReview = useCallback(() => {
    progress.current.overview = false;
    setReviewing(false);
  }, []);

  useEffect(() => {
    const overview = !!run && run.id === freshRun;
    progress.current = { overview, index: 0, since: performance.now() };
    setReviewing(overview);
    select(overview ? "run" : (run?.recent.at(-1)?.group ?? "run"));
    // Initialize once per run. A changing snapshot must never reset the view.
  }, [run?.id]);

  useEffect(() => {
    if (!following || paused || !run) return;
    progress.current.since = performance.now();
    const timer = setInterval(() => {
      const state = latest.current;
      if (!state) return;
      const now = performance.now();
      if (document.visibilityState !== "visible") {
        progress.current.since = now;
        return;
      }
      const p = progress.current;
      if (!p.overview) {
        if (now - p.since >= 600) {
          const group = state.recent.at(-1)?.group;
          if (group) select(group);
          p.since = now;
        }
        return;
      }
      const groups = Object.values(state.groups).sort(
        (a, b) => a.order - b.order,
      );
      // Do not spend the reading interval on an empty startup placeholder.
      if (!groups[p.index]?.count && !terminal(state.status)) {
        p.since = now;
        return;
      }
      if (now - p.since < READING_MS) return;
      let next = p.index + 1;
      // Failed/cancelled runs may never exercise some operations. Do not
      // present those as having run, or wait forever for their receipts.
      while (terminal(state.status) && groups[next] && !groups[next].count)
        next++;
      if (next >= groups.length) {
        endReview();
      } else if (groups[next].count) {
        p.index = next;
        p.since = now;
        select(groups[next].id);
      }
    }, 100);
    return () => clearInterval(timer);
  }, [run?.id, following, paused, select, endReview]);

  return { reviewing, endReview };
}
