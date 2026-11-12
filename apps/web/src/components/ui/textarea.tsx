import { forwardRef, type TextareaHTMLAttributes } from "react";

import { cn } from "@/utils/cn";

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement> & { invalid?: boolean }>(
  function Textarea({ className, invalid, ...props }, ref) {
    return (
      <textarea
        ref={ref}
        aria-invalid={invalid || undefined}
        className={cn(
          "min-h-[5rem] w-full rounded border bg-surface px-3 py-2 text-base text-ink shadow-xs placeholder:text-ink-subtle",
          "focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent/25 focus-visible:ring-offset-0",
          invalid ? "border-danger" : "border-line-strong hover:border-ink-subtle",
          className,
        )}
        {...props}
      />
    );
  },
);
