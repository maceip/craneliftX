import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

/**
 * Separator — Fluent 2-styled hairline divider.
 * Source: https://fluent2-react-kit.graund.io/r/separator.json
 */
function Separator({
  className,
  orientation = "horizontal",
  decorative = true,
  ...props
}: ComponentProps<"div"> & {
  orientation?: "horizontal" | "vertical";
  decorative?: boolean;
}) {
  return (
    <div
      role={decorative ? "none" : "separator"}
      aria-orientation={
        !decorative && orientation === "vertical" ? "vertical" : undefined
      }
      data-slot="separator"
      data-orientation={orientation}
      className={cn(
        "shrink-0 bg-border/60 data-[orientation=horizontal]:h-px data-[orientation=horizontal]:w-full data-[orientation=vertical]:h-full data-[orientation=vertical]:w-px",
        className
      )}
      {...props}
    />
  );
}

export { Separator };
