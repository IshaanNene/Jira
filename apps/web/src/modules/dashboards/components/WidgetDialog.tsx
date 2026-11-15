import { useState, type FormEvent, type ReactNode } from "react";

import { Button, Dialog, DialogContent, DialogFooter, DialogTrigger, Field, Input, MultiSelect, Select } from "@/components/ui";
import { FormError } from "@/modules/auth";
import { useProjects } from "@/modules/projects";
import { errorMessage, type GroupBy, type Widget, type WidgetQuery, type WidgetSize } from "@/services/api";

import { useAddWidget, useUpdateWidget } from "../api";
import { defaultQuery, defaultTitle, GROUP_LABEL, KIND_DESCRIPTION, KIND_LABEL, PERIODS, queryProjects, type WidgetKind } from "../model";
import { WidgetView } from "./WidgetView";

const KINDS = (Object.keys(KIND_LABEL) as WidgetKind[]).map((kind) => ({ value: kind, label: KIND_LABEL[kind], description: KIND_DESCRIPTION[kind] }));
const GROUPS = (Object.keys(GROUP_LABEL) as GroupBy[]).map((g) => ({ value: g, label: GROUP_LABEL[g] }));
const SIZES = [
  { value: "half", label: "Half width" },
  { value: "full", label: "Full width" },
];

function periodOptions(kind: keyof typeof PERIODS) {
  const unit = kind === "created_vs_resolved" || kind === "cumulative_flow" ? "days" : kind === "velocity" ? "sprints" : "weeks";
  return PERIODS[kind].map((n) => ({ value: String(n), label: `Last ${n} ${unit}` }));
}

/** The kind-specific options of a query. */
function QueryFields({ query, onChange }: { query: WidgetQuery; onChange: (query: WidgetQuery) => void }) {
  const projects = useProjects();
  const options = (projects.data ?? []).map((p) => ({ value: p.id, label: `${p.name} (${p.key})` }));

  if (query.kind === "velocity") {
    return (
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Project">
          <Select value={query.project} onValueChange={(project) => onChange({ ...query, project })} options={options} />
        </Field>
        <Field label="Period">
          <Select value={String(query.sprints)} onValueChange={(v) => onChange({ ...query, sprints: Number(v) })} options={periodOptions("velocity")} />
        </Field>
      </div>
    );
  }

  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <Field label="Projects" hint="None chosen: every project you can see.">
        <MultiSelect aria-label="Projects" placeholder="All projects" values={query.projects ?? []} onChange={(p) => onChange({ ...query, projects: p })} options={options} />
      </Field>
      {query.kind === "created_vs_resolved" && (
        <Field label="Period">
          <Select value={String(query.days)} onValueChange={(v) => onChange({ ...query, days: Number(v) })} options={periodOptions("created_vs_resolved")} />
        </Field>
      )}
      {query.kind === "cumulative_flow" && (
        <Field label="Period">
          <Select value={String(query.days)} onValueChange={(v) => onChange({ ...query, days: Number(v) })} options={periodOptions("cumulative_flow")} />
        </Field>
      )}
      {query.kind === "time_in_status" && (
        <Field label="Period" hint="Stays that ended in this period.">
          <Select value={String(query.weeks)} onValueChange={(v) => onChange({ ...query, weeks: Number(v) })} options={periodOptions("time_in_status")} />
        </Field>
      )}
      {query.kind === "cycle_time" && (
        <Field label="Period">
          <Select value={String(query.weeks)} onValueChange={(v) => onChange({ ...query, weeks: Number(v) })} options={periodOptions("cycle_time")} />
        </Field>
      )}
      {query.kind === "breakdown" && (
        <>
          <Field label="Group by">
            <Select value={query.group_by} onValueChange={(v) => onChange({ ...query, group_by: v as GroupBy })} options={GROUPS} />
          </Field>
          <label className="flex items-center gap-2 text-sm text-ink sm:col-span-2">
            <input type="checkbox" checked={query.include_done ?? false} onChange={(e) => onChange({ ...query, include_done: e.target.checked })} className="h-4 w-4 accent-[hsl(var(--accent))]" />
            Include done issues
          </label>
        </>
      )}
    </div>
  );
}

/** Add a chart to a dashboard, or change one. Shows the chart as it will look. */
export function WidgetDialog({ dashboardId, widget, trigger }: { dashboardId: string; widget?: Widget; trigger: ReactNode }) {
  const projects = useProjects();
  const add = useAddWidget(dashboardId);
  const update = useUpdateWidget(dashboardId);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState<WidgetQuery>(widget?.query ?? defaultQuery("created_vs_resolved", []));
  const [size, setSize] = useState<WidgetSize>(widget?.size ?? "half");
  // Until someone types a title, it follows the chosen chart.
  const [title, setTitle] = useState<string | null>(widget?.title ?? null);
  const [error, setError] = useState<string | null>(null);
  const shownTitle = title ?? defaultTitle(query);
  const ready = query.kind !== "velocity" || Boolean(query.project);

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (next) {
      setQuery(widget?.query ?? defaultQuery("created_vs_resolved", []));
      setSize(widget?.size ?? "half");
      setTitle(widget?.title ?? null);
      setError(null);
    }
  };

  const changeKind = (kind: WidgetKind) => setQuery(defaultQuery(kind, queryProjects(query), projects.data?.[0]?.id));

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    const name = shownTitle.trim();
    if (!name) return setError("Give the chart a title.");
    setError(null);
    try {
      if (widget) await update.mutateAsync({ widgetId: widget.id, title: name, size, query });
      else await add.mutateAsync({ title: name, size, query });
      setOpen(false);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent title={widget ? "Edit chart" : "Add chart"} description="Charts count only issues in projects each viewer can see." className="top-[5%] max-h-[90vh] max-w-3xl overflow-y-auto">
        <form onSubmit={(e) => void onSubmit(e)} noValidate className="space-y-4">
          <FormError message={error} />
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Chart">
              <Select value={query.kind} onValueChange={(v) => changeKind(v as WidgetKind)} options={KINDS} />
            </Field>
            <Field label="Title">
              <Input value={shownTitle} maxLength={80} onChange={(e) => setTitle(e.target.value)} />
            </Field>
          </div>
          <QueryFields query={query} onChange={setQuery} />
          <Field label="Size">
            <Select className="sm:w-1/2" value={size} onValueChange={(v) => setSize(v as WidgetSize)} options={SIZES} />
          </Field>
          <section aria-label="Preview" className="rounded-lg border border-line p-3">
            <h3 className="mb-2 text-sm font-semibold text-ink">{shownTitle || "Preview"}</h3>
            <WidgetView query={ready ? query : null} title={shownTitle} height={220} />
          </section>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setOpen(false)}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!ready} loading={add.isPending || update.isPending}>{widget ? "Save" : "Add chart"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
