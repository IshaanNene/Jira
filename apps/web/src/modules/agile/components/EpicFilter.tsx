import { Select } from "@/components/ui";
import type { Epic } from "@/services/api";

import { ALL_EPICS, NO_EPIC } from "../model";

/** Narrows a board or backlog to one epic's issues, or to issues outside any epic. */
export function EpicFilter({ epics, value, onChange }: { epics: Epic[]; value: string; onChange: (value: string) => void }) {
  if (epics.length === 0) return null;
  const options = [
    { value: ALL_EPICS, label: "All epics" },
    ...epics.map((e) => ({ value: e.key, label: `${e.key} · ${e.summary}${e.status_category === "done" ? " (done)" : ""}` })),
    { value: NO_EPIC, label: "Issues without epic" },
  ];
  // An epic finished long ago, or deleted, is no longer offered: show everything again.
  const current = options.some((o) => o.value === value) ? value : ALL_EPICS;
  return <Select size="sm" className="w-56" aria-label="Filter by epic" value={current} onValueChange={onChange} options={options} />;
}

/** The filter value in effect, given the epics on offer. */
export function effectiveEpic(epics: Epic[], value: string): string {
  return value === ALL_EPICS || value === NO_EPIC || epics.some((e) => e.key === value) ? value : ALL_EPICS;
}
