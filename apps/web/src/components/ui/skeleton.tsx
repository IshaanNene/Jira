import type { HTMLAttributes } from "react";

import { cn } from "@/utils/cn";

/** Placeholder block shown while content loads. Sized by the caller to match the real layout. */
export function Skeleton({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      aria-hidden
      className={cn(
        "relative overflow-hidden rounded bg-sunken",
        "after:absolute after:inset-0 after:-translate-x-full after:animate-shimmer",
        "after:bg-gradient-to-r after:from-transparent after:via-surface/60 after:to-transparent",
        className,
      )}
      {...props}
    />
  );
}
