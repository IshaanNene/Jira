import type { ReactNode } from "react";

import { cn } from "@/utils/cn";

/** Standard page frame: title row with actions, then content at a readable width. */
export function Page({ title, description, actions, children, width = "default" }: {
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
  width?: "default" | "narrow" | "wide";
}) {
  return (
    <div className={cn("mx-auto w-full px-6 py-6", width === "narrow" ? "max-w-2xl" : width === "wide" ? "max-w-none" : "max-w-5xl")}>
      <header className="mb-5 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-2xl font-semibold tracking-tight text-ink">{title}</h1>
          {description && <p className="mt-0.5 text-sm text-ink-muted">{description}</p>}
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </header>
      {children}
    </div>
  );
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <section className={cn("rounded-lg border border-line bg-surface shadow-xs", className)}>{children}</section>;
}

export function CardHeader({ title, description, actions }: { title: string; description?: string; actions?: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3 border-b border-line px-4 py-3">
      <div>
        <h2 className="text-base font-semibold text-ink">{title}</h2>
        {description && <p className="mt-0.5 text-sm text-ink-muted">{description}</p>}
      </div>
      {actions}
    </div>
  );
}
