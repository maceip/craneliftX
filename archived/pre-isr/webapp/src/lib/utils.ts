import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Compose class names with clsx and resolve conflicting Tailwind utilities with tailwind-merge.
 * Source: https://fluent2-react-kit.graund.io/r/utils.json
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
