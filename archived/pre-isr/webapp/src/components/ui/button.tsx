import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

/**
 * Button — Fluent 2-styled, shadcn-API button.
 * Source: https://fluent2-react-kit.graund.io/r/button.json
 */
const buttonVariants = cva(
  [
    "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap",
    "rounded-md font-medium select-none cursor-pointer",
    "outline-none transition-colors duration-200",
    "disabled:pointer-events-none disabled:opacity-50",
    "focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
    "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  ],
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground hover:bg-primary/90 active:bg-primary/80",
        destructive:
          "bg-destructive text-destructive-foreground hover:bg-destructive/90 active:bg-destructive/80 focus-visible:ring-destructive",
        secondary:
          "border border-border bg-secondary text-secondary-foreground hover:bg-muted active:bg-muted/80",
        outline:
          "border border-input bg-transparent hover:bg-muted hover:text-foreground active:bg-muted/80",
        ghost:
          "hover:bg-muted hover:text-foreground active:bg-muted/80",
        accent:
          "bg-[#fc4c01] text-white hover:bg-[#e04300] active:bg-[#c93c00] shadow-[0_1px_2px_rgba(252,76,1,0.3)]",
        link: "text-[#fc4c01] underline-offset-4 hover:underline active:opacity-80",
      },
      size: {
        default: "h-8 px-3.5 text-sm has-[>svg]:px-2.5",
        sm: "h-6 gap-1.5 px-2.5 text-xs has-[>svg]:px-1.5",
        lg: "h-10 px-5 text-base has-[>svg]:px-3.5",
        icon: "size-8",
        "icon-sm": "size-6",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
);

interface ButtonProps
  extends ComponentProps<"button">,
    VariantProps<typeof buttonVariants> {}

function Button({
  className,
  variant = "default",
  size = "default",
  ...props
}: ButtonProps) {
  return (
    <button
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants };
