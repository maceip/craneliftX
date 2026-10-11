import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

/**
 * Progress — Fluent 2-styled thin progress bar (2px track).
 * Source: https://fluent2-react-kit.graund.io/r/progress.json
 */
interface ProgressProps extends ComponentProps<"div"> {
  value?: number | null;
  size?: "default" | "lg";
  variant?: "default" | "accent" | "success" | "warning";
}

function Progress({
  className,
  value,
  size = "default",
  variant = "accent",
  ...props
}: ProgressProps) {
  const percent = value == null ? 0 : Math.min(100, Math.max(0, value));

  const variantColor = {
    default: "bg-primary",
    accent: "bg-[#fc4c01]",
    success: "bg-emerald-500",
    warning: "bg-amber-500",
  }[variant];

  return (
    <div
      role="progressbar"
      aria-valuenow={value ?? undefined}
      aria-valuemin={0}
      aria-valuemax={100}
      data-slot="progress"
      className={cn(
        "relative w-full overflow-hidden rounded-full bg-neutral-800",
        size === "lg" ? "h-1.5" : "h-1",
        className
      )}
      {...props}
    >
      <div
        data-slot="progress-indicator"
        className={cn(
          "h-full rounded-full transition-all duration-300 ease-out",
          variantColor,
          value == null && "w-1/3 animate-indeterminate"
        )}
        style={{ width: value != null ? `${percent}%` : undefined }}
      />
    </div>
  );
}

export { Progress };
