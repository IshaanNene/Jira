import { Slot } from "@radix-ui/react-slot";
import { Loader2 } from "lucide-react";
import { forwardRef, type ButtonHTMLAttributes } from "react";

import { cn } from "@/utils/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md" | "lg" | "icon";

const variants: Record<Variant, string> = {
  primary: "bg-accent text-accent-ink shadow-xs hover:bg-accent-hover",
  secondary: "border border-line-strong bg-surface text-ink shadow-xs hover:bg-sunken",
  ghost: "text-ink-muted hover:bg-sunken hover:text-ink",
  danger: "bg-danger text-white shadow-xs hover:opacity-90",
};

const sizes: Record<Size, string> = {
  sm: "h-7 gap-1.5 px-2.5 text-sm",
  md: "h-8 gap-2 px-3 text-sm",
  lg: "h-10 gap-2 px-4 text-base",
  icon: "h-8 w-8",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  /** Shows a spinner and blocks interaction while an operation is in flight. */
  loading?: boolean;
  /** Render the child element (e.g. a router Link) with button styling. */
  asChild?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant = "secondary", size = "md", loading = false, disabled, asChild = false, children, type, ...props },
  ref,
) {
  const classes = cn(
    "inline-flex select-none items-center justify-center whitespace-nowrap rounded font-medium",
    "transition-colors duration-100 disabled:pointer-events-none disabled:opacity-50",
    variants[variant],
    sizes[size],
    className,
  );
  if (asChild) {
    return (
      <Slot ref={ref} className={classes} {...props}>
        {children}
      </Slot>
    );
  }
  return (
    <button ref={ref} type={type ?? "button"} className={classes} disabled={disabled || loading} aria-busy={loading || undefined} {...props}>
      {loading && <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />}
      {children}
    </button>
  );
});
