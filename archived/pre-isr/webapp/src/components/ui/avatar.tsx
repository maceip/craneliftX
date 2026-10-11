import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

/**
 * Avatar — Fluent 2-styled avatar with fallback.
 * Source: https://fluent2-react-kit.graund.io/r/avatar.json
 */
function Avatar({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="avatar"
      className={cn(
        "relative flex size-8 shrink-0 overflow-hidden rounded-full ring-1 ring-border/50",
        className
      )}
      {...props}
    />
  );
}

function AvatarImage({
  className,
  src,
  alt = "",
  ...props
}: ComponentProps<"img">) {
  return (
    <img
      data-slot="avatar-image"
      src={src}
      alt={alt}
      className={cn("aspect-square size-full object-cover", className)}
      {...props}
    />
  );
}

function AvatarFallback({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="avatar-fallback"
      className={cn(
        "flex size-full items-center justify-center rounded-full bg-neutral-900 text-xs font-semibold text-neutral-300 select-none",
        className
      )}
      {...props}
    />
  );
}

export { Avatar, AvatarImage, AvatarFallback };
