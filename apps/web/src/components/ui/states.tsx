import { AlertTriangle, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { ApiError } from "@/services/api";
import { cn } from "@/utils/cn";

import { Button } from "./button";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description: string;
  action?: ReactNode;
  className?: string;
}

/** What a list shows when it has nothing in it yet: why, and what to do next. */
export function EmptyState({ icon: Icon, title, description, action, className }: EmptyStateProps) {
  return (
    <div className={cn("flex flex-col items-center px-6 py-12 text-center", className)}>
      <span className="mb-3 flex h-10 w-10 items-center justify-center rounded-lg bg-sunken text-ink-muted">
        <Icon className="h-5 w-5" aria-hidden />
      </span>
      <h3 className="text-base font-semibold text-ink">{title}</h3>
      <p className="mt-1 max-w-sm text-sm text-ink-muted">{description}</p>
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

interface ErrorStateProps {
  error: unknown;
  onRetry?: () => void;
  className?: string;
}

/** Inline failure for a region of the page, with a way to retry. */
export function ErrorState({ error, onRetry, className }: ErrorStateProps) {
  const message = error instanceof ApiError ? error.detail : "Something went wrong while loading this.";
  const requestId = error instanceof ApiError ? error.requestId : undefined;
  return (
    <div role="alert" className={cn("flex flex-col items-center px-6 py-10 text-center", className)}>
      <span className="mb-3 flex h-10 w-10 items-center justify-center rounded-lg bg-danger-soft text-danger">
        <AlertTriangle className="h-5 w-5" aria-hidden />
      </span>
      <h3 className="text-base font-semibold text-ink">Couldn&rsquo;t load this</h3>
      <p className="mt-1 max-w-sm text-sm text-ink-muted">{message}</p>
      {requestId && <p className="mt-1 font-mono text-2xs text-ink-subtle">Reference {requestId}</p>}
      {onRetry && (
        <Button className="mt-4" onClick={onRetry}>
          Try again
        </Button>
      )}
    </div>
  );
}
