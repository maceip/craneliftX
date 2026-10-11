import { useEffect, useRef, useState } from "react";
import { applyLedger, LedgerEvent, Run, terminal } from "./pipeline";
export async function request<T>(
  url: string,
  options?: RequestInit,
): Promise<T> {
  const result = await fetch(url, options);
  const body = await result.json();
  if (!result.ok)
    throw new Error(body.error ?? `Request failed (${result.status})`);
  return body;
}
export function usePipeline(id: string | null) {
  const [run, setRun] = useState<Run | null>(null),
    [connection, setConnection] = useState("idle");
  const [error, setError] = useState<string | null>(null),
    current = useRef<Run | null>(null);
  useEffect(() => {
    let disposed = false,
      source: EventSource | null = null,
      retry: ReturnType<typeof setTimeout> | undefined;
    let queued: LedgerEvent[] = [];
    current.current = null;
    setRun(null);
    setError(null);
    if (!id) {
      setConnection("idle");
      return;
    }
    const publish = () => {
      if (!disposed) setRun(current.current);
    };
    const flush = () => {
      if (!current.current || !queued.length) return;
      const batch = queued;
      queued = [];
      try {
        for (const event of batch)
          current.current = applyLedger(current.current!, event);
        publish();
      } catch (e) {
        source?.close();
        setError((e as Error).message);
        setConnection("recovering");
        void connect();
      }
    };
    async function connect() {
      source?.close();
      queued = [];
      try {
        const snapshot = await request<Run>(`/api/pipeline/runs/${id}`);
        if (disposed) return;
        current.current = snapshot;
        publish();
        setError(null);
        if (terminal(snapshot.status)) {
          setConnection("saved");
          return;
        }
        setConnection("connecting");
        source = new EventSource(
          `/api/pipeline/runs/${id}/events?after=${snapshot.cursor}`,
        );
        source.onopen = () => {
          if (!disposed) {
            setConnection("live");
            setError(null);
          }
        };
        source.addEventListener("ledger", (message) => {
          try {
            queued.push(JSON.parse((message as MessageEvent).data));
          } catch {
            setError("Malformed event received");
            source?.close();
            void connect();
          }
          if (queued.length >= 128) flush();
        });
        source.addEventListener("coordinator", (message) => {
          flush();
          if (!current.current || disposed) return;
          try {
            const meta = JSON.parse((message as MessageEvent).data);
            if (meta.id !== id)
              throw new Error("Coordinator run identity changed");
            current.current = { ...current.current, ...meta };
            publish();
            if (terminal(meta.status)) {
              source?.close();
              setConnection("saved");
            }
          } catch (e) {
            setError((e as Error).message);
          }
        });
        source.onerror = () => {
          if (!disposed) setConnection("reconnecting");
        };
      } catch (e) {
        if (disposed) return;
        setError((e as Error).message);
        setConnection("reconnecting");
        retry = setTimeout(() => void connect(), 1500);
      }
    }
    setConnection("connecting");
    void connect();
    const timer = setInterval(flush, 50);
    return () => {
      disposed = true;
      clearInterval(timer);
      clearTimeout(retry);
      source?.close();
    };
  }, [id]);
  return { run, connection, error };
}
