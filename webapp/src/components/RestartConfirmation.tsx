import React, { useEffect, useRef } from "react";

export function RestartConfirmation({
  stage,
  description,
  busy,
  onConfirm,
  onCancel,
}: {
  stage: string;
  description: string;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirm = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    confirm.current?.focus({ preventScroll: true });
  }, [stage]);
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
    >
      <button
        ref={confirm}
        type="button"
        className="restart-confirm"
        disabled={busy}
        aria-describedby="restart-description"
        onClick={onConfirm}
      >
        {busy ? "Restarting analysis…" : `Restart Analysis from ${stage} ?`}
      </button>
      <p id="restart-description">{description}</p>
      <button
        type="button"
        className="restart-cancel"
        disabled={busy}
        onClick={onCancel}
      >
        Cancel
      </button>
    </div>
  );
}
