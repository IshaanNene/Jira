// What each kind of chart is called, its default options, and how its labels read.
import type { GroupBy, WidgetData, WidgetQuery } from "@/services/api";
import { titleCase } from "@/utils/format";

export type WidgetKind = WidgetQuery["kind"];

export const KIND_LABEL: Record<WidgetKind, string> = {
  created_vs_resolved: "Created vs resolved",
  cycle_time: "Cycle time",
  breakdown: "Issue breakdown",
  cumulative_flow: "Cumulative flow",
  time_in_status: "Time in status",
  velocity: "Sprint velocity",
};

export const KIND_DESCRIPTION: Record<WidgetKind, string> = {
  created_vs_resolved: "Issues created and resolved each day.",
  cycle_time: "Days from first start to done, per week the issues were finished.",
  breakdown: "Open issues counted by a field.",
  cumulative_flow: "Issues to do, in progress and done at the end of each day.",
  time_in_status: "Median days issues stayed in each status before moving on.",
  velocity: "Issues completed and left over in each completed sprint.",
};

export const GROUP_LABEL: Record<GroupBy, string> = {
  status_category: "Status category",
  status: "Status",
  type: "Issue type",
  priority: "Priority",
  assignee: "Assignee",
  project: "Project",
};

export const PERIODS = {
  created_vs_resolved: [7, 14, 30, 90, 180, 365],
  cycle_time: [4, 8, 12, 26, 52],
  cumulative_flow: [14, 30, 90, 180, 365],
  time_in_status: [4, 8, 12, 26, 52],
  velocity: [3, 5, 7, 10, 20],
} as const;

/** A sensible first version of each kind; velocity needs a project to start from. */
export function defaultQuery(kind: WidgetKind, projects: string[], firstProject?: string): WidgetQuery {
  switch (kind) {
    case "created_vs_resolved":
      return { kind, projects, days: 30 };
    case "cycle_time":
      return { kind, projects, weeks: 12 };
    case "breakdown":
      return { kind, projects, group_by: "status_category", include_done: false };
    case "cumulative_flow":
      return { kind, projects, days: 30 };
    case "time_in_status":
      return { kind, projects, weeks: 12 };
    case "velocity":
      return { kind, project: projects[0] ?? firstProject ?? "", sprints: 7 };
  }
}

/** Projects the query reads; empty means every project the viewer can browse. */
export function queryProjects(query: WidgetQuery): string[] {
  return query.kind === "velocity" ? (query.project ? [query.project] : []) : (query.projects ?? []);
}

export function defaultTitle(query: WidgetQuery): string {
  if (query.kind === "breakdown") return `Issues by ${GROUP_LABEL[query.group_by].toLowerCase()}`;
  return KIND_LABEL[query.kind];
}

/** One line under the title saying what the chart covers. */
export function describeQuery(query: WidgetQuery, projectName: (id: string) => string): string {
  const projects = queryProjects(query);
  const scope = projects.length === 0 ? "All projects" : projects.length <= 2 ? projects.map(projectName).join(", ") : `${projects.length} projects`;
  switch (query.kind) {
    case "created_vs_resolved":
      return `${scope} · last ${query.days} days`;
    case "cycle_time":
      return `${scope} · last ${query.weeks} weeks`;
    case "breakdown":
      return `${scope} · ${query.include_done ? "all issues" : "open issues"}`;
    case "cumulative_flow":
      return `${scope} · last ${query.days} days`;
    case "time_in_status":
      return `${scope} · stays ended in the last ${query.weeks} weeks`;
    case "velocity":
      return `${scope} · last ${query.sprints} sprints`;
  }
}

export interface Names {
  project: (id: string) => string;
  user: (id: string) => string;
}

export const CATEGORY_LABEL: Record<string, string> = { todo: "To do", in_progress: "In progress", done: "Done" };

/** The readable name of a breakdown group. An empty key is "none" (e.g. unassigned). */
export function groupLabel(groupBy: GroupBy, key: string, names: Names): string {
  if (!key) return groupBy === "assignee" ? "Unassigned" : "None";
  switch (groupBy) {
    case "status_category":
      return CATEGORY_LABEL[key] ?? titleCase(key);
    case "assignee":
      return names.user(key);
    case "project":
      return names.project(key);
    default:
      return titleCase(key);
  }
}

const dayFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeZone: "UTC" });
const shortDayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", timeZone: "UTC" });

/** A `YYYY-MM-DD` day from the server. Days are UTC days, so they are formatted in UTC. */
export const formatDay = (day: string) => dayFormat.format(new Date(`${day}T00:00:00Z`));
export const formatShortDay = (day: string) => shortDayFormat.format(new Date(`${day}T00:00:00Z`));

/** Formats a day count with one decimal, dropping a trailing ".0". */
export function formatDays(days: number): string {
  const rounded = Math.round(days * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

/** Whether the answer has anything to draw. */
export function isEmpty(data: WidgetData): boolean {
  switch (data.kind) {
    case "created_vs_resolved":
      return data.days.every((d) => d.created === 0 && d.resolved === 0);
    case "cycle_time":
      return data.weeks.every((w) => w.issues === 0);
    case "breakdown":
      return data.total === 0;
    case "cumulative_flow":
      return data.days.every((d) => d.todo + d.in_progress + d.done === 0);
    case "time_in_status":
      return data.statuses.length === 0;
    case "velocity":
      return data.sprints.length === 0;
  }
}

export interface Table {
  columns: { label: string; numeric?: boolean }[];
  rows: { key: string; cells: string[] }[];
}

/** The numbers behind the chart, as a table. */
export function tableOf(data: WidgetData, names: Names): Table {
  switch (data.kind) {
    case "created_vs_resolved":
      return {
        columns: [{ label: "Day" }, { label: "Created", numeric: true }, { label: "Resolved", numeric: true }],
        rows: [...data.days].reverse().map((d) => ({ key: d.date, cells: [formatDay(d.date), String(d.created), String(d.resolved)] })),
      };
    case "cycle_time":
      return {
        columns: [{ label: "Week of" }, { label: "Issues", numeric: true }, { label: "Median days", numeric: true }, { label: "85th percentile days", numeric: true }],
        rows: [...data.weeks].reverse().map((w) => ({
          key: w.week,
          cells: [formatDay(w.week), String(w.issues), w.issues ? formatDays(w.median_days) : "–", w.issues ? formatDays(w.p85_days) : "–"],
        })),
      };
    case "breakdown":
      return {
        columns: [{ label: GROUP_LABEL[data.group_by] }, { label: "Issues", numeric: true }, { label: "Share", numeric: true }],
        rows: data.groups.map((g) => ({
          key: g.key || "none",
          cells: [groupLabel(data.group_by, g.key, names), String(g.count), data.total ? `${Math.round((g.count / data.total) * 100)}%` : "–"],
        })),
      };
    case "cumulative_flow":
      return {
        columns: [{ label: "Day" }, { label: "To do", numeric: true }, { label: "In progress", numeric: true }, { label: "Done", numeric: true }],
        rows: [...data.days].reverse().map((d) => ({ key: d.date, cells: [formatDay(d.date), String(d.todo), String(d.in_progress), String(d.done)] })),
      };
    case "time_in_status":
      return {
        columns: [{ label: "Status" }, { label: "Stays", numeric: true }, { label: "Median days", numeric: true }, { label: "Mean days", numeric: true }],
        rows: data.statuses.map((s) => ({
          key: s.status,
          cells: [titleCase(s.status), String(s.stays), formatDays(s.median_days), formatDays(s.mean_days)],
        })),
      };
    case "velocity":
      return {
        columns: [{ label: "Sprint" }, { label: "Completed", numeric: true }, { label: "Not completed", numeric: true }],
        rows: [...data.sprints].reverse().map((s) => ({ key: s.sprint_id, cells: [s.name, String(s.completed), String(s.incomplete)] })),
      };
  }
}
