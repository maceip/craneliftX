import React, { useEffect, useRef, useState } from "react";
import { TransportIcon } from "./TransportIcon";

export function RestartConfirmation({
  stage,
  description,
  busy,
  paused,
  onConfirm,
  onCancel,
}: {
  stage: string;
  description: string;
  busy: boolean;
  paused: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirm = useRef<HTMLButtonElement>(null);
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const reveal = () => setReady(true);
    // The pill expands for 260ms. Reveal the action only after it has room,
    // and keep it out of keyboard navigation until then.
    const timer = window.setTimeout(reveal, paused || media.matches ? 0 : 280);
    const preference = () => {
      if (media.matches) reveal();
    };
    media.addEventListener("change", preference);
    return () => {
      clearTimeout(timer);
      media.removeEventListener("change", preference);
    };
  }, [paused]);
  useEffect(() => {
    if (ready) confirm.current?.focus({ preventScroll: true });
  }, [ready, stage]);
  useEffect(() => {
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener("keydown", dismiss);
    return () => window.removeEventListener("keydown", dismiss);
  }, [busy, onCancel]);
  return (
    <div
      id="restart-confirmation"
      role="region"
      className="restart-confirmation"
      aria-label="Restart analysis confirmation"
      data-ready={ready}
      inert={!ready}
    >
      <button
        ref={confirm}
        type="button"
        className="restart-confirm"
        disabled={busy}
        aria-label={
          busy ? "Restarting analysis…" : `Restart Analysis from ${stage} ?`
        }
        aria-describedby="restart-description"
        title={`Restart analysis from ${stage}. ${description}`}
        onClick={onConfirm}
      >
        {busy ? (
          "Restarting…"
        ) : (
          <>
            Restart analysis<span className="restart-stage"> from {stage}</span>
            ?
          </>
        )}
      </button>
      <p id="restart-description" className="visually-hidden">
        {description}
      </p>
      <button
        type="button"
        className="restart-cancel"
        aria-label="Cancel"
        title="Cancel restart"
        disabled={busy}
        onClick={onCancel}
      >
        <TransportIcon kind="close" />
      </button>
    </div>
  );
}
