import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn `cn` helper: merge conditional class names, Tailwind-aware. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
