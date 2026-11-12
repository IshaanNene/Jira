import { forwardRef, type InputHTMLAttributes } from "react";

import { cn } from "@/utils/cn";

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  invalid?: boolean;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input({ className, invalid, ...props }, ref) {
  return (
    <input
      ref={ref}
      aria-invalid={invalid || undefined}
      className={cn(
        "h-9 w-full rounded border bg-surface px-3 text-base text-ink shadow-xs",
        "placeholder:text-ink-subtle transition-colors duration-100",
        "focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent/25 focus-visible:ring-offset-0",
        "disabled:cursor-not-allowed disabled:bg-sunken disabled:opacity-70",
        invalid ? "border-danger" : "border-line-strong hover:border-ink-subtle",
        className,
      )}
      {...props}
    />
  );
});
