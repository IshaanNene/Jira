import {
  ArrowUp,
  Bookmark,
  Check,
  ChevronDown,
  ChevronsDown,
  ChevronsUp,
  ChevronUp,
  Circle,
  Equal,
  ListTree,
  Plus,
  Zap,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/utils/cn";
import { titleCase } from "@/utils/format";

const TYPE: Record<string, { icon: LucideIcon; color: string }> = {
  epic: { icon: Zap, color: "#8F4FD9" },
  story: { icon: Bookmark, color: "#3FA44A" },
  task: { icon: Check, color: "#2684FF" },
  bug: { icon: Circle, color: "#E2483D" },
  feature: { icon: Plus, color: "#1D9AAA" },
  improvement: { icon: ArrowUp, color: "#3FA44A" },
  subtask: { icon: ListTree, color: "#4BADE8" },
};

/** Small coloured square identifying the issue type at a glance. */
export function IssueTypeIcon({ type, className }: { type: string; className?: string }) {
  const meta = TYPE[type] ?? TYPE.task!;
  const Icon = meta.icon;
  return (
    <span role="img" aria-label={titleCase(type)} title={titleCase(type)} className={cn("inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm", className)} style={{ backgroundColor: meta.color }}>
      <Icon className="h-3 w-3 text-white" strokeWidth={3} fill={type === "bug" ? "white" : "none"} aria-hidden />
    </span>
  );
}

const PRIORITY: Record<string, { icon: LucideIcon; color: string }> = {
  highest: { icon: ChevronsUp, color: "#C9372C" },
  high: { icon: ChevronUp, color: "#E2483D" },
  medium: { icon: Equal, color: "#E38A1B" },
  low: { icon: ChevronDown, color: "#2684FF" },
  lowest: { icon: ChevronsDown, color: "#2684FF" },
};

export function PriorityIcon({ priority, withLabel = false }: { priority: string; withLabel?: boolean }) {
  const meta = PRIORITY[priority] ?? PRIORITY.medium!;
  const Icon = meta.icon;
  return (
    <span className="inline-flex items-center gap-1.5 text-sm text-ink" title={`${titleCase(priority)} priority`}>
      <Icon className="h-4 w-4 shrink-0" style={{ color: meta.color }} strokeWidth={2.5} aria-hidden />
      {withLabel ? titleCase(priority) : <span className="sr-only">{titleCase(priority)} priority</span>}
    </span>
  );
}

const LOZENGE: Record<string, string> = {
  todo: "bg-line text-ink-muted",
  in_progress: "bg-accent-soft text-accent",
  done: "bg-success-soft text-success",
};

/** Status label coloured by category: grey to do, blue in progress, green done. */
export function StatusLozenge({ name, category, className }: { name: string; category: string; className?: string }) {
  return (
    <span className={cn("inline-flex h-5 max-w-full items-center truncate rounded-sm px-1.5 text-2xs font-bold uppercase tracking-wide", LOZENGE[category] ?? LOZENGE.todo, className)}>
      {name}
    </span>
  );
}
