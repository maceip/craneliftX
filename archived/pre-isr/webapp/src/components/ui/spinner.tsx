import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

/**
 * Spinner — Fluent 2-styled, shadcn-API loading indicator.
 * Rotating brand arc with faint track circle.
 * Source: https://fluent2-react-kit.graund.io/r/spinner.json
 */
const spinnerVariants = cva(
  ["shrink-0", "animate-spin"],
  {
    variants: {
      size: {
        sm: "size-3.5",
        default: "size-5",
        lg: "size-8",
        xl: "size-10",
      },
    },
    defaultVariants: {
      size: "default",
    },
  }
);

interface SpinnerProps
  extends ComponentProps<"span">,
    VariantProps<typeof spinnerVariants> {
  label?: string;
}

function Spinner({
  className,
  size = "default",
  label,
  "aria-label": ariaLabel,
  ...props
}: SpinnerProps) {
  const accessibleName = label || ariaLabel || "Loading";

  return (
    <span
      role="status"
      aria-label={accessibleName}
      data-slot="spinner"
      data-size={size}
      className={cn("inline-flex items-center gap-2", className)}
      {...props}
    >
      <svg
        viewBox="0 0 16 16"
        fill="none"
        aria-hidden="true"
        className={spinnerVariants({ size })}
      >
        <circle
          cx="8"
          cy="8"
          r="7"
          stroke="currentColor"
          strokeWidth="1.5"
          className="text-white/10"
        />
        <circle
          cx="8"
          cy="8"
          r="7"
          stroke="#fc4c01"
          strokeWidth="1.5"
          strokeLinecap="round"
          pathLength="100"
          strokeDasharray="28 72"
        />
      </svg>
      {label ? <span className="text-xs text-muted-foreground">{label}</span> : null}
    </span>
  );
}

export { Spinner, spinnerVariants };
