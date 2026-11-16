import { ArrowDown, ArrowUp, Columns2, Copy, LayoutDashboard, MoreHorizontal, Pencil, Plus, Square, Trash2, Users } from "lucide-react";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";

import {
  Badge,
  Button,
  Card,
  Dialog,
  DialogContent,
  DialogFooter,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  EmptyState,
  ErrorState,
  Skeleton,
  toast,
} from "@/components/ui";
import { UserChip } from "@/modules/organizations";
import { errorMessage, type Dashboard, type Widget } from "@/services/api";
import { useSession } from "@/state/session";
import { cn } from "@/utils/cn";

import { useCopyDashboard, useDashboard, useDeleteDashboard, useRemoveWidget, useUpdateDashboard, useUpdateWidget } from "../api";
import { DashboardDialog } from "../components/DashboardDialog";
import { WidgetDialog } from "../components/WidgetDialog";
import { useNames, WidgetView } from "../components/WidgetView";
import { describeQuery, withPeriod } from "../model";

/** How a viewer looks at a dashboard: a period for every chart, and how often to refresh. */
interface ViewSettings {
  /** Days; null keeps each chart's own period. */
  days: number | null;
  /** Minutes; 0 is off. */
  refresh: number;
}

const PERIOD_OPTIONS = [null, 7, 14, 30, 90, 180, 365] as const;
const REFRESH_OPTIONS = [0, 1, 5, 15] as const;

/** Remembered per dashboard in this browser only; a missing or blocked store means defaults. */
function useViewSettings(dashboardId: string): [ViewSettings, (next: ViewSettings) => void] {
  const key = `meridian.dashboard-view.${dashboardId}`;
  const [settings, setSettings] = useState<ViewSettings>(() => {
    try {
      const saved = JSON.parse(window.localStorage.getItem(key) ?? "null") as Partial<ViewSettings> | null;
      const days = PERIOD_OPTIONS.includes(saved?.days as never) ? (saved!.days ?? null) : null;
      const refresh = REFRESH_OPTIONS.includes(saved?.refresh as never) ? saved!.refresh! : 0;
      return { days, refresh };
    } catch {
      return { days: null, refresh: 0 };
    }
  });
  const save = (next: ViewSettings) => {
    setSettings(next);
    try {
      window.localStorage.setItem(key, JSON.stringify(next));
    } catch {
      // Not remembered; the choice still applies until the page is left.
    }
  };
  return [settings, save];
}

const selectClass = "h-8 rounded border border-line-strong bg-surface px-2 text-sm text-ink focus-visible:border-accent";

function ViewBar({ settings, onChange }: { settings: ViewSettings; onChange: (next: ViewSettings) => void }) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-4 text-sm text-ink-muted">
      <label className="flex items-center gap-2">
        Period
        <select
          className={selectClass}
          value={settings.days ?? ""}
          onChange={(e) => onChange({ ...settings, days: e.target.value ? Number(e.target.value) : null })}
        >
          {PERIOD_OPTIONS.map((d) => <option key={d ?? "own"} value={d ?? ""}>{d ? `Last ${d} days` : "As each chart is set"}</option>)}
        </select>
      </label>
      <label className="flex items-center gap-2">
        Refresh
        <select className={selectClass} value={settings.refresh} onChange={(e) => onChange({ ...settings, refresh: Number(e.target.value) })}>
          {REFRESH_OPTIONS.map((m) => <option key={m} value={m}>{m ? `Every ${m} min` : "Off"}</option>)}
        </select>
      </label>
    </div>
  );
}

function WidgetCard({ dashboard, widget, index, view }: { dashboard: Dashboard; widget: Widget; index: number; view: ViewSettings }) {
  const query = withPeriod(widget.query, view.days);
  const names = useNames();
  const update = useUpdateDashboard(dashboard.id);
  const resize = useUpdateWidget(dashboard.id);
  const remove = useRemoveWidget(dashboard.id);
  const count = dashboard.widgets.length;

  const move = (by: number) => {
    const order = dashboard.widgets.map((w) => w.id);
    const [moved] = order.splice(index, 1);
    order.splice(index + by, 0, moved!);
    update.mutate({ widget_order: order }, { onError: (e) => toast.error(errorMessage(e)) });
  };

  return (
    <Card className={cn("min-w-0", widget.size === "full" && "lg:col-span-2")}>
      <div className="flex items-start justify-between gap-3 border-b border-line px-4 py-3">
        <div className="min-w-0">
          <h2 className="truncate text-base font-semibold text-ink">{widget.title}</h2>
          <p className="mt-0.5 truncate text-xs text-ink-muted">{describeQuery(query, names.project)}</p>
        </div>
        {dashboard.editable && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon" variant="ghost" aria-label={`Actions for ${widget.title}`}><MoreHorizontal className="h-4 w-4" /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <WidgetDialog
                dashboardId={dashboard.id}
                widget={widget}
                trigger={<DropdownMenuItem onSelect={(e) => e.preventDefault()}><Pencil className="h-4 w-4" />Edit</DropdownMenuItem>}
              />
              <DropdownMenuItem
                onSelect={() => resize.mutate({ widgetId: widget.id, size: widget.size === "full" ? "half" : "full" }, { onError: (e) => toast.error(errorMessage(e)) })}
              >
                {widget.size === "full" ? <Columns2 className="h-4 w-4" /> : <Square className="h-4 w-4" />}
                {widget.size === "full" ? "Make half width" : "Make full width"}
              </DropdownMenuItem>
              <DropdownMenuItem disabled={index === 0} onSelect={() => move(-1)}><ArrowUp className="h-4 w-4" />Move earlier</DropdownMenuItem>
              <DropdownMenuItem disabled={index === count - 1} onSelect={() => move(1)}><ArrowDown className="h-4 w-4" />Move later</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem className="text-danger" onSelect={() => remove.mutate(widget.id, { onError: (e) => toast.error(errorMessage(e)) })}>
                <Trash2 className="h-4 w-4" />Remove
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>
      <div className="p-4">
        <WidgetView query={query} title={widget.title} refreshMs={view.refresh * 60_000} />
      </div>
    </Card>
  );
}

function DeleteDashboard({ dashboard, open, onOpenChange }: { dashboard: Dashboard; open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate();
  const remove = useDeleteDashboard();
  const confirm = () =>
    remove.mutate(dashboard.id, {
      onSuccess: () => {
        toast.success(`Deleted ${dashboard.name}`);
        navigate("/dashboards");
      },
      onError: (e) => toast.error(errorMessage(e)),
    });
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Delete dashboard?" description={`${dashboard.name} and its ${dashboard.widgets.length} charts are deleted for everyone. The issues they chart are not affected.`}>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="danger" loading={remove.isPending} onClick={confirm}>Delete</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function DashboardBody({ dashboard }: { dashboard: Dashboard }) {
  const me = useSession((s) => s.account?.id);
  const update = useUpdateDashboard(dashboard.id);
  const copy = useCopyDashboard();
  const navigate = useNavigate();
  const [deleting, setDeleting] = useState(false);
  const [view, setView] = useViewSettings(dashboard.id);
  const isOwner = dashboard.owner_id === me;

  const makeCopy = () =>
    copy.mutate(dashboard.id, {
      onSuccess: (d) => {
        toast.success(`Copied to ${d.name}`, "Only you can see the copy until you share it.");
        navigate(`/dashboards/${d.id}`);
      },
      onError: (e) => toast.error("Couldn't copy the dashboard", errorMessage(e)),
    });

  const toggleShared = () =>
    update.mutate(
      { shared: !dashboard.shared },
      {
        onSuccess: (d) => toast.success(d.shared ? "Everyone in the workspace can now see this dashboard" : "Only you can see this dashboard now"),
        onError: (e) => toast.error(errorMessage(e)),
      },
    );

  return (
    <div className="mx-auto w-full px-6 py-6">
      <nav aria-label="Breadcrumb" className="mb-1 text-sm text-ink-muted">
        <Link to="/dashboards" className="hover:text-accent hover:underline">Dashboards</Link>
      </nav>
      <header className="mb-5 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="flex items-center gap-2 text-2xl font-semibold tracking-tight text-ink">
            {dashboard.name}
            {dashboard.shared && <Badge><Users className="mr-1 h-3 w-3" aria-hidden />Shared</Badge>}
          </h1>
          {dashboard.description && <p className="mt-0.5 max-w-3xl text-sm text-ink-muted">{dashboard.description}</p>}
          {!isOwner && <p className="mt-1 flex items-center gap-1.5 text-xs text-ink-subtle">Owner <UserChip userId={dashboard.owner_id} /></p>}
        </div>
        <div className="flex items-center gap-2">
          <Button variant="secondary" loading={copy.isPending} onClick={makeCopy}><Copy className="h-4 w-4" />Make a copy</Button>
          {dashboard.editable && isOwner && (
            <Button variant="secondary" loading={update.isPending} onClick={toggleShared}>
              <Users className="h-4 w-4" />{dashboard.shared ? "Stop sharing" : "Share"}
            </Button>
          )}
          {dashboard.editable && (
            <>
              <WidgetDialog dashboardId={dashboard.id} trigger={<Button variant="primary"><Plus className="h-4 w-4" />Add chart</Button>} />
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon" variant="secondary" aria-label="Dashboard actions"><MoreHorizontal className="h-4 w-4" /></Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DashboardDialog
                    dashboard={dashboard}
                    trigger={<DropdownMenuItem onSelect={(e) => e.preventDefault()}><Pencil className="h-4 w-4" />Rename</DropdownMenuItem>}
                  />
                  <DropdownMenuSeparator />
                  <DropdownMenuItem className="text-danger" onSelect={() => setDeleting(true)}><Trash2 className="h-4 w-4" />Delete</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
              <DeleteDashboard dashboard={dashboard} open={deleting} onOpenChange={setDeleting} />
            </>
          )}
        </div>
      </header>
      {dashboard.widgets.length > 0 && <ViewBar settings={view} onChange={setView} />}

      {dashboard.widgets.length === 0 ? (
        <EmptyState
          className="rounded-lg border border-dashed border-line py-16"
          icon={LayoutDashboard}
          title="No charts yet"
          description={dashboard.editable ? "Add a chart to see how work is created, finished and spread across your projects." : "The owner has not added any charts."}
          action={dashboard.editable ? <WidgetDialog dashboardId={dashboard.id} trigger={<Button variant="primary">Add chart</Button>} /> : undefined}
        />
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {dashboard.widgets.map((w, i) => <WidgetCard key={w.id} dashboard={dashboard} widget={w} index={i} view={view} />)}
        </div>
      )}
    </div>
  );
}

export default function DashboardPage() {
  const { dashboardId = "" } = useParams();
  const dashboard = useDashboard(dashboardId);
  if (dashboard.isPending) return <div className="space-y-4 p-6" aria-busy><Skeleton className="h-10 w-72" /><Skeleton className="h-80" /></div>;
  if (dashboard.isError) return <ErrorState className="py-16" error={dashboard.error} onRetry={() => void dashboard.refetch()} />;
  // Keyed, so moving to another dashboard starts from that dashboard's own view settings.
  return <DashboardBody key={dashboard.data.id} dashboard={dashboard.data} />;
}
