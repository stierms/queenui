import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * Class-name join with Tailwind conflict resolution: later utilities win over
 * earlier ones of the same family, so a caller's `className` can override a
 * primitive's defaults instead of silently losing to source order.
 *
 * Its own module because `primitives.tsx` exports components; a value export
 * beside them switches React Fast Refresh off for that whole file.
 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
