import { Filter, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";

import { Button } from "@/components/ui";
import { ApiError } from "@/services/api";

import { useMatchingIds } from "./api";
import { MqlEditor, type QueryProblem } from "./MqlEditor";

/** Where `ORDER BY` starts outside a quoted string, or -1. */
function orderByAt(query: string): number {
  let quote: string | null = null;
  const words: { text: string; at: number }[] = [];
  let word = "";
  let start = 0;
  const flush = () => {
    if (word) words.push({ text: word.toLowerCase(), at: start });
    word = "";
  };
  for (let i = 0; i < query.length; i++) {
    const c = query[i]!;
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = null;
      continue;
    }
    if (/[\p{L}\p{N}_]/u.test(c)) {
      if (!word) start = i;
      word += c;
      continue;
    }
    flush();
    // Anything but white space keeps "order" and "by" apart.
    if (!/\s/.test(c)) words.push({ text: c, at: i });
    if (c === '"' || c === "'") quote = c;
  }
  flush();
  for (let i = 0; i + 1 < words.length; i++) {
    if (words[i]!.text === "order" && words[i + 1]!.text === "by") return words[i]!.at;
  }
  return -1;
}

/**
 * One MQL query that matches what every part matches: each part in brackets, joined with
 * AND. Sorting is dropped, since only which issues match matters to a filter.
 */
export function combineMql(parts: string[]): string {
  const conditions = parts
    .map((part) => {
      const at = orderByAt(part);
      return (at < 0 ? part : part.slice(0, at)).trim();
    })
    .filter(Boolean);
  return conditions.length === 1 ? conditions[0]! : conditions.map((c) => `(${c})`).join(" AND ");
}

/**
 * An MQL filter kept in the URL (`?mql=`) and the ids of the project's issues it matches.
 * `also` are further conditions in force (a board's quick filters), which every issue must
 * match as well.
 */
export function useMqlFilter(projectId: string, also: string[] = []) {
  const [params, setParams] = useSearchParams();
  const applied = params.get("mql")?.trim() ?? "";
  const query = also.length === 0 ? applied : combineMql([applied, ...also]);
  const ids = useMatchingIds(projectId, query);
  const apply = (mql: string) => {
    const next = new URLSearchParams(params);
    if (mql.trim()) next.set("mql", mql.trim());
    else next.delete("mql");
    setParams(next, { replace: true });
  };
  const invalid = ids.error instanceof ApiError && ids.error.status === 422 ? ids.error.fieldErrors.find((f) => f.field === "mql") : undefined;
  // A mistake can be pointed out in the editor only when its query is the one that ran.
  const problem: QueryProblem | undefined = invalid && query === applied ? { query: applied, message: invalid.message, span: invalid.span } : undefined;
  const quickProblem = invalid && query !== applied ? invalid.message : undefined;
  const matches = useMemo(() => (query && ids.data ? new Set(ids.data.ids) : null), [query, ids.data]);
  return {
    /** The query in force; empty when there is none. */
    applied,
    apply,
    /** Ids that match, or `null` while there is no filter (or it has not answered yet). */
    matches,
    truncated: Boolean(query && ids.data?.truncated),
    problem,
    /** A mistake in the conditions combined with the quick filters; the editor can't point at it. */
    quickProblem,
    failed: ids.isError && !invalid,
    loading: Boolean(query) && ids.isFetching,
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
