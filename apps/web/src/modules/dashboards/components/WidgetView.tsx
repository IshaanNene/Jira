import { BarChart3, Table2 } from "lucide-react";
import { useCallback, useMemo, useState } from "react";

import { Chart, type ChartTheme } from "@/components/charts/Chart";
import { ErrorState, Skeleton } from "@/components/ui";
import { useUserDirectory } from "@/modules/organizations";
import { useProjects } from "@/modules/projects";
import type { WidgetQuery } from "@/services/api";
import { cn } from "@/utils/cn";

import { useWidgetData } from "../api";
import { chartOption } from "../charts";
import { isEmpty, KIND_LABEL, tableOf, type Names } from "../model";

/** Project and person names for chart labels; ids that cannot be resolved read as such. */
export function useNames(): Names {
  const projects = useProjects();
  const { members } = useUserDirectory();
  const memberCount = members.length;
  return useMemo(() => {
    const projectNames = new Map((projects.data ?? []).map((p) => [p.id, p.name]));
    const people = new Map(members.map((m) => [m.user_id, m.display_name]));
    return {
      project: (id: string) => projectNames.get(id) ?? "Unknown project",
      user: (id: string) => people.get(id) ?? "Former member",
    };
    // `members` is a fresh empty array until the directory loads; its length marks the change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects.data, memberCount]);
}

type View = "chart" | "table";

function ViewToggle({ view, onChange }: { view: View; onChange: (view: View) => void }) {
  const options: { value: View; label: string; icon: typeof BarChart3 }[] = [
    { value: "chart", label: "Chart", icon: BarChart3 },
    { value: "table", label: "Table", icon: Table2 },
  ];
  return (
    <div className="inline-flex rounded border border-line-strong p-0.5" role="group" aria-label="Show as">
      {options.map(({ value, label, icon: Icon }) => (
        <button
          key={value}
          type="button"
          aria-pressed={view === value}
          title={label}
          onClick={() => onChange(value)}
          className={cn("inline-flex h-6 w-6 items-center justify-center rounded-sm", view === value ? "bg-accent-soft text-accent" : "text-ink-subtle hover:text-ink")}
        >
          <Icon className="h-3.5 w-3.5" aria-hidden />
          <span className="sr-only">{label}</span>
        </button>
      ))}
    </div>
  );
}

/** One chart and the table behind it. `toolbar` sits next to the chart/table switch. */
export function WidgetView({ query, title, height = 260 }: { query: WidgetQuery | null; title: string; height?: number }) {
  const data = useWidgetData(query);
  const names = useNames();
  const [view, setView] = useState<View>("chart");
  const answer = data.data;
  const build = useCallback((theme: ChartTheme) => (answer ? chartOption(answer, theme, names) : {}), [answer, names]);

  if (!query) return <p className="flex items-center justify-center text-sm text-ink-subtle" style={{ height }}>Choose a project to chart.</p>;
  if (data.isPending) return <Skeleton style={{ height }} className="w-full" />;
  if (data.isError) return <ErrorState error={data.error} onRetry={() => void data.refetch()} />;
  if (!answer || isEmpty(answer)) {
    return (
      <p className="flex items-center justify-center px-4 text-center text-sm text-ink-subtle" style={{ height }}>
        {answer?.kind === "velocity" ? "Nothing yet: velocity appears once a sprint is completed." : "No issues match in this period."}
      </p>
    );
  }

  const table = tableOf(answer, names);
  return (
    <div className={cn("relative", data.isFetching && "opacity-70 transition-opacity")}>
      <div className="absolute left-0 top-0 z-10">
        <ViewToggle view={view} onChange={setView} />
      </div>
      {view === "chart" ? (
        <div className="pt-8">
          <Chart build={build} label={`${title}: ${KIND_LABEL[answer.kind]} chart`} height={height} />
        </div>
      ) : (
        <div className="overflow-auto pt-8" style={{ maxHeight: height + 32 }}>
          <table className="w-full text-sm">
            <caption className="sr-only">{title}</caption>
            <thead className="sticky top-0 bg-surface">
              <tr className="border-b border-line text-left text-xs uppercase tracking-wide text-ink-subtle">
                {table.columns.map((c) => (
                  <th key={c.label} scope="col" className={cn("py-1.5 pr-2 font-semibold", c.numeric && "text-right")}>{c.label}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {table.rows.map((row) => (
                <tr key={row.key} className="border-b border-line last:border-0">
                  {row.cells.map((cell, i) => (
                    <td key={i} className={cn("py-1.5 pr-2", table.columns[i]?.numeric ? "text-right tabular-nums text-ink-muted" : "text-ink")}>{cell}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
