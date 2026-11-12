import { BarChart3 } from "lucide-react";
import { useCallback, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";

import { Button, Card, CardHeader, EmptyState, ErrorState, Select, Skeleton } from "@/components/ui";
import { IssueTypeIcon } from "@/modules/issues";
import type { Board, Sprint, SprintReport } from "@/services/api";
import { formatDate } from "@/utils/format";

import { useSprintReport, useSprints, useVelocity } from "../api";
import { burndownOption, velocityOption } from "../charts";
import { AgileFrame } from "../components/AgileFrame";
import { Chart, type ChartTheme } from "../components/Chart";
import { burndownSeries, formatPoints, type BurndownUnit } from "../model";

const UNITS: { value: BurndownUnit; label: string }[] = [
  { value: "points", label: "Story points" },
  { value: "issues", label: "Issue count" },
];

function UnitToggle({ unit, onChange }: { unit: BurndownUnit; onChange: (unit: BurndownUnit) => void }) {
  return (
    <div className="inline-flex rounded border border-line-strong p-0.5" role="group" aria-label="Measure">
      {UNITS.map((u) => (
        <Button key={u.value} size="sm" variant={unit === u.value ? "primary" : "ghost"} aria-pressed={unit === u.value} className="h-6" onClick={() => onChange(u.value)}>
          {u.label}
        </Button>
      ))}
    </div>
  );
}

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="rounded-lg border border-line bg-surface px-4 py-3">
      <dt className="text-sm text-ink-muted">{label}</dt>
      <dd className="text-2xl font-semibold text-ink">{value}</dd>
      {hint && <p className="text-xs text-ink-subtle">{hint}</p>}
    </div>
  );
}

function IssueTable({ title, issues, projectKey }: { title: string; issues: SprintReport["outcome"]["completed"]; projectKey: string }) {
  return (
    <Card>
      <CardHeader title={`${title} (${issues.length})`} />
      {issues.length === 0 ? (
        <p className="px-4 py-3 text-sm text-ink-subtle">None.</p>
      ) : (
        <table className="w-full text-sm">
          <thead className="sr-only"><tr><th>Issue</th><th>Summary</th><th>Story points</th></tr></thead>
          <tbody>
            {issues.map((issue) => (
              <tr key={issue.id} className="border-b border-line last:border-0">
                <td className="w-28 px-4 py-2">
                  <span className="inline-flex items-center gap-2">
                    <IssueTypeIcon type={issue.type} />
                    <Link to={`/projects/${projectKey}/issues/${issue.key}`} className="font-medium text-ink-muted hover:text-accent hover:underline">{issue.key}</Link>
                  </span>
                </td>
                <td className="px-2 py-2 text-ink">{issue.summary}</td>
                <td className="w-16 px-4 py-2 text-right tabular-nums text-ink-muted">{issue.story_points != null ? formatPoints(issue.story_points) : "–"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function SprintSection({ board, sprint, unit }: { board: Board; sprint: Sprint; unit: BurndownUnit }) {
  const report = useSprintReport(board.project_key, sprint.id);
  const samples = report.data?.outcome.burndown;
  const endDate = sprint.end_date;
  const build = useCallback(
    (theme: ChartTheme) => burndownOption(burndownSeries(samples ?? [], { end_date: endDate }, unit), unit, theme),
    [samples, endDate, unit],
  );

  if (report.isPending) return <Skeleton className="h-80" />;
  if (report.isError) return <ErrorState error={report.error} onRetry={() => void report.refetch()} />;
  const { outcome } = report.data;
  const commitment = sprint.commitment;
  const pick = (points: number, issues: number) => (unit === "points" ? formatPoints(points) : String(issues));
  const period = sprint.started_at ? `${formatDate(sprint.started_at)} – ${sprint.completed_at ? formatDate(sprint.completed_at) : sprint.end_date ? formatDate(sprint.end_date) : "now"}` : "";

  return (
    <div className="space-y-4">
      <dl className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Stat label="Committed at start" value={commitment ? pick(commitment.points, commitment.issues) : "–"} hint={period} />
        <Stat label="Completed" value={pick(outcome.completed_points, outcome.completed.length)} />
        <Stat label={sprint.state === "closed" ? "Not completed" : "Remaining"} value={pick(outcome.incomplete_points, outcome.incomplete.length)} />
      </dl>
      <Card>
        <CardHeader
          title="Burndown"
          description={sprint.state === "closed" ? "Remaining work during the sprint, as it stood when the sprint was completed." : "Remaining work in the running sprint. The dashed line is an even pace to the end date."}
        />
        <div className="p-4">
          <Chart build={build} label={`Burndown chart for ${sprint.name}`} />
        </div>
      </Card>
      <div className="grid gap-4 lg:grid-cols-2">
        <IssueTable title="Completed issues" issues={outcome.completed} projectKey={board.project_key} />
        <IssueTable title={sprint.state === "closed" ? "Issues not completed" : "Open issues"} issues={outcome.incomplete} projectKey={board.project_key} />
      </div>
    </div>
  );
}

function VelocitySection({ board, unit }: { board: Board; unit: BurndownUnit }) {
  const velocity = useVelocity(board);
  const sprints = velocity.data;
  const build = useCallback((theme: ChartTheme) => velocityOption(sprints ?? [], unit, theme), [sprints, unit]);
  if (velocity.isPending) return <Skeleton className="h-64" />;
  if (velocity.isError) return <ErrorState error={velocity.error} onRetry={() => void velocity.refetch()} />;
  const value = (points: number, issues: number) => (unit === "points" ? formatPoints(points) : String(issues));
  return (
    <Card>
      <CardHeader title="Velocity" description="What each completed sprint committed to at its start, and what it finished." />
      {velocity.data.length === 0 ? (
        <p className="px-4 py-6 text-sm text-ink-subtle">Velocity appears after the first sprint is completed.</p>
      ) : (
        <div className="p-4">
          <Chart build={build} label="Velocity chart: committed and completed work per sprint" height={260} />
          <table className="mt-3 w-full text-sm">
            <thead>
              <tr className="border-b border-line text-left text-xs uppercase tracking-wide text-ink-subtle">
                <th className="py-1.5 font-semibold">Sprint</th>
                <th className="py-1.5 text-right font-semibold">Committed</th>
                <th className="py-1.5 text-right font-semibold">Completed</th>
              </tr>
            </thead>
            <tbody>
              {velocity.data.map((s) => (
                <tr key={s.sprint_id} className="border-b border-line last:border-0">
                  <td className="py-1.5 text-ink">{s.name}</td>
                  <td className="py-1.5 text-right tabular-nums text-ink-muted">{value(s.committed_points, s.committed_issues)}</td>
                  <td className="py-1.5 text-right tabular-nums text-ink-muted">{value(s.completed_points, s.completed_issues)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

function ReportsBody({ board }: { board: Board }) {
  const sprints = useSprints(board);
  const [params, setParams] = useSearchParams();
  const [unit, setUnit] = useState<BurndownUnit>("points");

  if (board.kind !== "scrum") {
    return <EmptyState className="py-20" icon={BarChart3} title="Reports are for Scrum boards" description="Burndown and velocity describe sprints. This Kanban board has none." />;
  }
  if (sprints.isPending) return <div className="px-6" aria-busy><Skeleton className="h-80" /></div>;
  if (sprints.isError) return <ErrorState className="py-16" error={sprints.error} onRetry={() => void sprints.refetch()} />;

  const started = sprints.data.filter((s) => s.state !== "future");
  const sprint = started.find((s) => s.id === params.get("sprint")) ?? started[0];
  const choose = (id: string) => {
    const next = new URLSearchParams(params);
    next.set("sprint", id);
    setParams(next, { replace: true });
  };

  return (
    <div className="h-full space-y-6 overflow-y-auto px-6 pb-8">
      <div className="flex flex-wrap items-center gap-3">
        {sprint && (
          <Select
            size="sm"
            className="w-56"
            aria-label="Sprint"
            value={sprint.id}
            onValueChange={choose}
            options={started.map((s) => ({ value: s.id, label: s.state === "active" ? `${s.name} (running)` : s.name }))}
          />
        )}
        <UnitToggle unit={unit} onChange={setUnit} />
      </div>
      {sprint ? (
        <SprintSection board={board} sprint={sprint} unit={unit} />
      ) : (
        <EmptyState icon={BarChart3} title="No sprint has started yet" description="The burndown chart appears once a sprint is running." action={<Button asChild><Link to={`/projects/${board.project_key}/backlog?board=${board.id}`}>Go to backlog</Link></Button>} />
      )}
      <VelocitySection board={board} unit={unit} />
    </div>
  );
}

export default function ReportsPage() {
  return <AgileFrame title="Reports">{(board) => <ReportsBody board={board} />}</AgileFrame>;
}
