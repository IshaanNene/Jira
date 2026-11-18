import { X } from "lucide-react";

import { Button } from "@/components/ui";
import type { Epic } from "@/services/api";
import { cn } from "@/utils/cn";

import { ALL_EPICS, formatPoints, NO_EPIC } from "../model";

/** Share of an epic's issues that are done, as a whole percentage (0 for an empty epic). */
export function epicPercent(epic: Epic): number {
  return epic.issues === 0 ? 0 : Math.round((epic.done / epic.issues) * 100);
}

function EpicRow({ epic, selected, onSelect }: { epic: Epic; selected: boolean; onSelect: () => void }) {
  const percent = epicPercent(epic);
  return (
    <li>
      <button
        type="button"
        aria-pressed={selected}
        onClick={onSelect}
        className={cn(
          "w-full rounded border px-3 py-2 text-left transition-colors",
          selected ? "border-accent bg-accent-soft" : "border-transparent hover:bg-sunken",
        )}
      >
        <span className="flex items-baseline gap-2">
          <span className="truncate text-sm font-medium text-ink">{epic.summary}</span>
          {epic.status_category === "done" && <span className="ml-auto shrink-0 text-xs text-success">Done</span>}
        </span>
        <span className="block font-mono text-xs text-ink-subtle">{epic.key}</span>
        <span
          role="progressbar"
          aria-label={`${epic.key} progress`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          className="mt-2 block h-1.5 overflow-hidden rounded-full bg-sunken"
        >
          <span className="block h-full rounded-full bg-success" style={{ width: `${percent}%` }} />
        </span>
        <span className="mt-1 block text-xs text-ink-subtle">
          {epic.issues === 0
            ? "No issues yet"
            : `${epic.done} of ${epic.issues} ${epic.issues === 1 ? "issue" : "issues"} done`}
          {epic.points > 0 && ` · ${formatPoints(epic.done_points)} of ${formatPoints(epic.points)} pts`}
        </span>
      </button>
    </li>
  );
}

/**
 * The project's epics with how far along each is. Choosing one narrows the backlog to its
 * issues; choosing it again shows everything.
 */
export function EpicPanel({ epics, value, onChange, onClose }: { epics: Epic[]; value: string; onChange: (value: string) => void; onClose: () => void }) {
  const toggle = (key: string) => onChange(value === key ? ALL_EPICS : key);
  return (
    <aside aria-label="Epics" className="w-full shrink-0 self-start rounded md:w-64 border border-line bg-surface p-2">
      <header className="mb-1 flex items-center justify-between px-1">
        <h2 className="text-sm font-semibold text-ink">Epics</h2>
        <Button variant="ghost" size="icon" className="h-7 w-7" aria-label="Close epics" onClick={onClose}><X className="h-4 w-4" /></Button>
      </header>
      {epics.length === 0 ? (
        <p className="px-1 py-2 text-sm text-ink-subtle">This project has no open epics. Create an issue of type Epic to group work.</p>
      ) : (
        <ul className="space-y-1">
          {epics.map((epic) => <EpicRow key={epic.id} epic={epic} selected={value === epic.key} onSelect={() => toggle(epic.key)} />)}
          <li>
            <button
              type="button"
              aria-pressed={value === NO_EPIC}
              onClick={() => toggle(NO_EPIC)}
              className={cn(
                "w-full rounded border px-3 py-2 text-left text-sm transition-colors",
                value === NO_EPIC ? "border-accent bg-accent-soft text-ink" : "border-transparent text-ink-muted hover:bg-sunken",
              )}
            >
              Issues without epic
            </button>
          </li>
        </ul>
      )}
    </aside>
  );
}
