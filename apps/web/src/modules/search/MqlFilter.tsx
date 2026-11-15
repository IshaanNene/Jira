import { Filter, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Button } from "@/components/ui";
import { ApiError } from "@/services/api";

import { useMatchingIds } from "./api";
import { MqlEditor, type QueryProblem } from "./MqlEditor";

/** An MQL filter kept in the URL (`?mql=`) and the ids of the project's issues it matches. */
export function useMqlFilter(projectId: string) {
  const [params, setParams] = useSearchParams();
  const applied = params.get("mql")?.trim() ?? "";
  const ids = useMatchingIds(projectId, applied);
  const apply = (mql: string) => {
    const next = new URLSearchParams(params);
    if (mql.trim()) next.set("mql", mql.trim());
    else next.delete("mql");
    setParams(next, { replace: true });
  };
  const invalid = ids.error instanceof ApiError && ids.error.status === 422 ? ids.error.fieldErrors.find((f) => f.field === "mql") : undefined;
  const problem: QueryProblem | undefined = invalid ? { query: applied, message: invalid.message, span: invalid.span } : undefined;
  const matches = useMemo(() => (applied && ids.data ? new Set(ids.data.ids) : null), [applied, ids.data]);
  return {
    /** The query in force; empty when there is none. */
    applied,
    apply,
    /** Ids that match, or `null` while there is no filter (or it has not answered yet). */
    matches,
    truncated: Boolean(applied && ids.data?.truncated),
    problem,
    failed: ids.isError && !problem,
    loading: Boolean(applied) && ids.isFetching,
  };
}

export type MqlFilterState = ReturnType<typeof useMqlFilter>;

/** A button that opens an MQL editor under the toolbar; Enter applies the query. */
export function MqlFilterButton({ filter, open, onOpenChange }: { filter: MqlFilterState; open: boolean; onOpenChange: (open: boolean) => void }) {
  const active = Boolean(filter.applied);
  return (
    <Button size="sm" variant={active ? "primary" : "secondary"} aria-expanded={open} onClick={() => onOpenChange(!open)}>
      <Filter className="h-4 w-4" aria-hidden />
      {active ? "MQL filter on" : "Filter with MQL"}
    </Button>
  );
}

export function MqlFilterPanel({ filter, label, onClose }: { filter: MqlFilterState; label: string; onClose: () => void }) {
  const [draft, setDraft] = useState(filter.applied);
  useEffect(() => setDraft(filter.applied), [filter.applied]);
  return (
    <section aria-label={`MQL filter for ${label}`} className="space-y-1.5 rounded border border-line bg-surface p-3">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <MqlEditor value={draft} onChange={setDraft} onRun={filter.apply} problem={filter.problem} />
        </div>
        {filter.applied && (
          <Button size="sm" variant="ghost" onClick={() => { setDraft(""); filter.apply(""); }}>Clear</Button>
        )}
        <Button size="icon" variant="ghost" className="h-8 w-8" aria-label="Close the MQL filter" onClick={onClose}>
          <X className="h-4 w-4" aria-hidden />
        </Button>
      </div>
      <p className="text-xs text-ink-subtle">
        Enter applies · only issues of this {label} that match stay visible · <code>ORDER BY</code> is ignored
      </p>
      {filter.failed && <p role="alert" className="text-sm text-danger">Couldn't apply the filter. Try again in a moment.</p>}
      {filter.truncated && <p className="text-sm text-ink-muted">More than 5000 issues match; only the first 5000 are shown.</p>}
    </section>
  );
}
