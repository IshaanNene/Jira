import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ChartTheme } from "@/components/charts/Chart";
import { DashboardPage, DashboardsPage } from "@/modules/dashboards";
import { chartOption } from "@/modules/dashboards/charts";
import { describeQuery, groupLabel, tableOf, type Names } from "@/modules/dashboards/model";
import type { Dashboard, WidgetData } from "@/services/api";

import { fakeApi, MEMBERS, project, renderAt, signIn, USER_ID, type Call } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const PAY = project();
const OTHER = "00000000-0000-7000-8000-000000000009";
const names: Names = { project: (id) => (id === PAY.id ? "Payments" : "Unknown project"), user: (id) => (id === USER_ID ? "Ada Lovelace" : "Former member") };
const theme: ChartTheme = {
  series: ["#0C66E4", "#E56910"],
  stages: ["#85B8FF", "#388BFF", "#0055CC"],
  reference: "#8590A2", ink: "#172B4D", muted: "#44546F", line: "#DCDFE4", surface: "#FFFFFF" };

const BREAKDOWN: WidgetData = {
  kind: "breakdown",
  group_by: "assignee",
  groups: [
    { key: USER_ID, count: 3 },
    { key: "", count: 1 },
  ],
  total: 4,
};

describe("dashboard model", () => {
  it("names groups, scopes and table rows in plain words", () => {
    expect(groupLabel("assignee", "", names)).toBe("Unassigned");
    expect(groupLabel("status_category", "in_progress", names)).toBe("In progress");
    expect(groupLabel("project", PAY.id, names)).toBe("Payments");
    expect(groupLabel("priority", "", names)).toBe("None");
    expect(describeQuery({ kind: "created_vs_resolved", projects: [], days: 30 }, names.project)).toBe("All projects · last 30 days");
    expect(describeQuery({ kind: "velocity", project: PAY.id, sprints: 5 }, names.project)).toBe("Payments · last 5 sprints");
    expect(describeQuery({ kind: "breakdown", projects: ["a", "b", "c"], group_by: "type" }, names.project)).toBe("3 projects · open issues");

    const table = tableOf(BREAKDOWN, names);
    expect(table.columns.map((c) => c.label)).toEqual(["Assignee", "Issues", "Share"]);
    expect(table.rows.map((r) => r.cells)).toEqual([
      ["Ada Lovelace", "3", "75%"],
      ["Unassigned", "1", "25%"],
    ]);
    // A week with nothing finished has no cycle time rather than a zero one.
    const cycle = tableOf({ kind: "cycle_time", weeks: [{ week: "2026-11-02", issues: 0, median_days: 0, p85_days: 0 }] }, names);
    expect(cycle.rows[0]!.cells.slice(1)).toEqual(["0", "–", "–"]);
  });

  it("draws one axis, a legend only for two series, and text in ink", () => {
    const breakdown = chartOption(BREAKDOWN, theme, names) as Record<string, any>;
    expect(breakdown.legend.show).toBe(false);
    expect(breakdown.yAxis.data).toEqual(["Ada Lovelace", "Unassigned"]);
    expect(breakdown.series).toHaveLength(1);
    expect(breakdown.series[0].label.color).toBe(theme.ink);

    const days: WidgetData = { kind: "created_vs_resolved", days: [{ date: "2026-11-01", created: 2, resolved: 1 }] };
    const lines = chartOption(days, theme, names) as Record<string, any>;
    expect(Array.isArray(lines.yAxis)).toBe(false);
    expect(lines.legend.show).not.toBe(false);
    expect(lines.series.map((s: { name: string; color: string }) => [s.name, s.color])).toEqual([
      ["Created", theme.series[0]],
      ["Resolved", theme.series[1]],
    ]);
    expect(lines.series[0].lineStyle.width).toBe(2);
    expect(lines.series[0].symbolSize).toBeGreaterThanOrEqual(8);
  });

  it("plots each resolved issue on a control chart with its rolling average and the mean", () => {
    const data: WidgetData = {
      kind: "control_chart",
      issues: [
        { issue_id: "a", key: "PAY-1", resolved_at: "2026-11-02T10:00:00Z", days: 2, rolling_days: 2 },
        { issue_id: "b", key: "<b>PAY-2</b>", resolved_at: "2026-11-04T10:00:00Z", days: 4, rolling_days: 3 },
      ],
      mean_days: 3,
      truncated: false,
    };
    const option = chartOption(data, theme, names) as Record<string, any>;
    expect(option.xAxis.type).toBe("time");
    expect(Array.isArray(option.yAxis)).toBe(false);
    expect(option.series.map((s: { name: string; type: string }) => [s.name, s.type])).toEqual([
      ["Issues", "scatter"],
      ["Rolling average", "line"],
      ["Mean", "line"],
    ]);
    expect(option.series[0].data[1]).toEqual([Date.parse("2026-11-04T10:00:00Z"), 4]);
    expect(option.series[0].symbolSize).toBeGreaterThanOrEqual(8);
    expect(option.series[2].data.map((p: number[]) => p[1])).toEqual([3, 3]);
    // Issue keys are escaped in the tooltip.
    const tip = option.tooltip.formatter({ seriesName: "Issues", dataIndex: 1, value: [0, 4] });
    expect(tip).toContain("&lt;b&gt;PAY-2&lt;/b&gt;");
    expect(tip).toContain("4 days");
    const table = tableOf(data, names);
    expect(table.rows[0]!.cells[0]).toBe("<b>PAY-2</b>");
    expect(table.rows[0]!.cells.slice(2)).toEqual(["4", "3"]);
  });

  it("charts time in status as sideways bars of median days", () => {
    const data: WidgetData = {
      kind: "time_in_status",
      statuses: [
        { status: "in_review", median_days: 2.5, mean_days: 3.25, stays: 4 },
        { status: "todo", median_days: 1, mean_days: 1, stays: 2 },
      ],
    };
    const option = chartOption(data, theme, names) as Record<string, any>;
    expect(option.legend.show).toBe(false);
    expect(option.yAxis.data).toEqual(["In review", "Todo"]);
    expect(option.series).toHaveLength(1);
    expect(option.series[0].data).toEqual([2.5, 1]);
    expect(option.series[0].label.formatter({ value: 2.5 })).toBe("2.5");
    expect(describeQuery({ kind: "time_in_status", projects: [], weeks: 12 }, names.project)).toBe("All projects · stays ended in the last 12 weeks");
    expect(tableOf(data, names).rows[0]!.cells).toEqual(["In review", "4", "2.5", "3.3"]);
  });

  it("stacks cumulative flow with done at the bottom, in one hue stronger towards done", () => {
    const flow: WidgetData = {
      kind: "cumulative_flow",
      days: [
        { date: "2026-11-01", todo: 3, in_progress: 1, done: 0 },
        { date: "2026-11-02", todo: 2, in_progress: 1, done: 1 },
      ],
    };
    const option = chartOption(flow, theme, names) as Record<string, any>;
    expect(Array.isArray(option.yAxis)).toBe(false);
    expect(option.series.map((s: { name: string; color: string; stack: string }) => [s.name, s.color, s.stack])).toEqual([
      ["Done", theme.stages[2], "flow"],
      ["In progress", theme.stages[1], "flow"],
      ["To do", theme.stages[0], "flow"],
    ]);
    expect(option.series[0].data).toEqual([0, 1]);
    expect(option.series[0].lineStyle.color).toBe(theme.surface);
    expect(option.legend.data).toEqual(["To do", "In progress", "Done"]);
    expect(describeQuery({ kind: "cumulative_flow", projects: [], days: 90 }, names.project)).toBe("All projects · last 90 days");
    expect(tableOf(flow, names).rows[0]!.cells.slice(1)).toEqual(["2", "1", "1"]);
  });
});

function dashboard(overrides: Partial<Dashboard> = {}): Dashboard {
  return {
    id: "d1",
    name: "Team health",
    description: "How the team is doing",
    owner_id: USER_ID,
    shared: false,
    editable: true,
    version: 3,
    updated_at: "2026-11-01T00:00:00Z",
    widgets: [{ id: "w1", title: "Issues by assignee", size: "half", query: { kind: "breakdown", projects: [PAY.id], group_by: "assignee", include_done: false } }],
    ...overrides,
  };
}

const baseRoutes = {
  "GET /api/v1/organizations/current/members": MEMBERS,
  "GET /api/v1/projects": { items: [PAY] },
  "POST /api/v1/analytics/query": BREAKDOWN,
};

describe("dashboards", () => {
  it("lists dashboards with their owner and sharing", async () => {
    signIn("member");
    fakeApi({
      ...baseRoutes,
      "GET /api/v1/dashboards": {
        items: [
          { id: "d1", name: "Team health", description: "", owner_id: USER_ID, shared: false, editable: true, updated_at: "2026-11-01T00:00:00Z", widget_count: 2 },
          { id: "d2", name: "Release readiness", description: "", owner_id: OTHER, shared: true, editable: false, updated_at: "2026-11-01T00:00:00Z", widget_count: 4 },
        ],
      },
    });
    renderAt(<DashboardsPage />, "/dashboards", "/dashboards");
    const row = (await screen.findByRole("link", { name: "Release readiness" })).closest("tr")!;
    expect(within(row).getByText("Shared")).toBeInTheDocument();
    expect(within(row).getByText("4")).toBeInTheDocument();
    expect(within(screen.getByRole("link", { name: "Team health" }).closest("tr")!).getByText("You")).toBeInTheDocument();
  });

  it("shows each chart with its table, and lets the owner share it", async () => {
    signIn("member");
    const api = fakeApi({
      ...baseRoutes,
      "GET /api/v1/dashboards/d1": dashboard(),
      "PATCH /api/v1/dashboards/d1": (call: Call) => ({ body: dashboard({ shared: (call.body as { shared: boolean }).shared }) }),
    });
    const user = userEvent.setup();
    renderAt(<DashboardPage />, "/dashboards/:dashboardId", "/dashboards/d1");

    expect(await screen.findByRole("heading", { name: "Issues by assignee" })).toBeInTheDocument();
    expect(await screen.findByText("Payments · open issues")).toBeInTheDocument();
    await waitFor(() => expect(api.sent("POST", "/api/v1/analytics/query")).toHaveLength(1));
    expect(api.sent("POST", "/api/v1/analytics/query")[0]!.body).toEqual({ kind: "breakdown", projects: [PAY.id], group_by: "assignee", include_done: false });

    await user.click(await screen.findByRole("button", { name: "Table" }));
    const table = screen.getByRole("table", { name: "Issues by assignee" });
    expect(within(table).getByRole("cell", { name: "Ada Lovelace" })).toBeInTheDocument();
    expect(within(table).getByRole("cell", { name: "Unassigned" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Share" }));
    await waitFor(() => expect(api.sent("PATCH", "/api/v1/dashboards/d1")[0]?.body).toEqual({ shared: true }));
    expect(api.sent("PATCH", "/api/v1/dashboards/d1")[0]!.headers["If-Match"]).toBe('"3"');
    expect(await screen.findByRole("button", { name: "Stop sharing" })).toBeInTheDocument();
  });

  it("shows the latest version instead of overwriting someone else's change", async () => {
    signIn("member");
    const theirs = dashboard({ name: "Delivery health", version: 4 });
    fakeApi({
      ...baseRoutes,
      "GET /api/v1/dashboards/d1": dashboard(),
      "PATCH /api/v1/dashboards/d1": () => ({
        status: 409,
        body: { code: "version_conflict", detail: "The resource was changed by someone else. Review the latest version and retry.", current: theirs },
      }),
    });
    const user = userEvent.setup();
    renderAt(<DashboardPage />, "/dashboards/:dashboardId", "/dashboards/d1");
    await user.click(await screen.findByRole("button", { name: "Share" }));
    expect(await screen.findByRole("heading", { name: "Delivery health" })).toBeInTheDocument();
    expect(await screen.findByText(/changed by someone else/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Share" })).toBeInTheDocument();
  });

  it("is read-only for people who may only view it", async () => {
    signIn("member");
    fakeApi({ ...baseRoutes, "GET /api/v1/dashboards/d1": dashboard({ owner_id: OTHER, shared: true, editable: false }) });
    renderAt(<DashboardPage />, "/dashboards/:dashboardId", "/dashboards/d1");
    expect(await screen.findByRole("heading", { name: "Issues by assignee" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add chart" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Share" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions for/ })).not.toBeInTheDocument();
  });

  it("adds a chart after previewing it", async () => {
    signIn("member");
    const api = fakeApi({
      ...baseRoutes,
      "GET /api/v1/dashboards/d1": dashboard({ widgets: [] }),
      "POST /api/v1/analytics/query": { kind: "created_vs_resolved", days: [{ date: "2026-11-01", created: 2, resolved: 1 }] },
      "POST /api/v1/dashboards/d1/widgets": { status: 201, body: { id: "w2", title: "Created vs resolved", size: "half", query: { kind: "created_vs_resolved", projects: [], days: 30 } } },
    });
    const user = userEvent.setup();
    renderAt(<DashboardPage />, "/dashboards/:dashboardId", "/dashboards/d1");

    await user.click((await screen.findAllByRole("button", { name: /Add chart/ }))[0]!);
    const dialog = await screen.findByRole("dialog");
    // The preview runs the same query the chart will.
    await waitFor(() => expect(api.sent("POST", "/api/v1/analytics/query")[0]?.body).toEqual({ kind: "created_vs_resolved", projects: [], days: 30 }));
    expect(within(dialog).getByRole("textbox", { name: "Title" })).toHaveValue("Created vs resolved");
    await user.click(within(dialog).getByRole("button", { name: "Add chart" }));
    await waitFor(() =>
      expect(api.sent("POST", "/api/v1/dashboards/d1/widgets")[0]?.body).toEqual({ title: "Created vs resolved", size: "half", query: { kind: "created_vs_resolved", projects: [], days: 30 } }),
    );
    expect(api.sent("POST", "/api/v1/dashboards/d1/widgets")[0]!.headers["If-Match"]).toBe('"3"');
  });
});
