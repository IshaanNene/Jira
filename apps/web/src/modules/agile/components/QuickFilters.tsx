import { useSearchParams } from "react-router-dom";

import { Button } from "@/components/ui";
import type { MqlFilterState } from "@/modules/search";
import type { Board } from "@/services/api";

const PARAM = "qf";

/**
 * The board's quick filters that are switched on, kept in the URL (`?qf=` with their ids)
 * so a link shows the same issues. Ids the board no longer has are ignored.
 */
export function useQuickFilters(board: Board) {
  const [params, setParams] = useSearchParams();
  const wanted = new Set((params.get(PARAM) ?? "").split(",").filter(Boolean));
  const active = board.quick_filters.filter((f) => wanted.has(f.id));
  const write = (ids: string[]) => {
    const next = new URLSearchParams(params);
    if (ids.length > 0) next.set(PARAM, ids.join(","));
    else next.delete(PARAM);
    setParams(next, { replace: true });
  };
  const isOn = (id: string) => active.some((f) => f.id === id);
  return {
    isOn,
    toggle: (id: string) => write(board.quick_filters.filter((f) => (f.id === id ? !isOn(id) : isOn(f.id))).map((f) => f.id)),
    clear: () => write([]),
    /** The queries in force, in the board's order. */
    queries: active.map((f) => f.query),
  };
}

export type QuickFilterState = ReturnType<typeof useQuickFilters>;

/** One toggle per quick filter of the board; nothing when the board has none. */
export function QuickFilterBar({ board, quick, mql }: { board: Board; quick: QuickFilterState; mql: MqlFilterState }) {
  if (board.quick_filters.length === 0) return null;
  return (
    <div role="group" aria-label="Quick filters" className="flex flex-wrap items-center gap-1.5">
      {board.quick_filters.map((f) => (
        <Button key={f.id} size="sm" variant={quick.isOn(f.id) ? "primary" : "secondary"} aria-pressed={quick.isOn(f.id)} title={f.query} onClick={() => quick.toggle(f.id)}>
          {f.name}
        </Button>
      ))}
      {quick.queries.length > 0 && <Button size="sm" variant="ghost" onClick={quick.clear}>Clear</Button>}
      {mql.quickProblem && (
        <p role="alert" className="basis-full text-sm text-danger">
          These filters can't run together: {mql.quickProblem} A board admin can fix the quick filter in Board settings.
        </p>
      )}
    </div>
  );
}
