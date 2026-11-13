// ECharts options for each kind of dashboard chart. One value axis each; the two series
// colours come from the theme (checked for colour-blind separation in both themes), and
// text stays in ink tokens.
import type { EChartsCoreOption } from "echarts/core";

import type { ChartTheme } from "@/components/charts/Chart";
import { bar, categoryAxis, chrome, line, valueAxis } from "@/components/charts/options";
import type { WidgetData } from "@/services/api";

import { formatDays, formatShortDay, GROUP_LABEL, groupLabel, type Names } from "./model";

const axisTooltip = (theme: ChartTheme, format?: (value: number) => string) => {
  const base = chrome(theme, format);
  return { ...base, grid: { ...base.grid, right: 40 }, tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "line", lineStyle: { color: theme.muted } } } };
};

/** The last value next to the end of a line, so the newest number reads without hovering. */
const endLabel = (theme: ChartTheme, format: (value: number) => string = String) => ({
  show: true,
  color: theme.ink,
  formatter: (p: { value: number | null }) => (p.value == null ? "" : format(p.value)),
});

export function chartOption(data: WidgetData, theme: ChartTheme, names: Names): EChartsCoreOption {
  switch (data.kind) {
    case "created_vs_resolved": {
      const days = data.days.map((d) => formatShortDay(d.date));
      return {
        ...axisTooltip(theme),
        xAxis: { ...categoryAxis(theme, days), boundaryGap: false },
        yAxis: valueAxis(theme, "Issues"),
        series: [
          { ...line("Created", theme.series[0], data.days.map((d) => d.created), theme), endLabel: endLabel(theme) },
          { ...line("Resolved", theme.series[1], data.days.map((d) => d.resolved), theme), endLabel: endLabel(theme) },
        ],
      };
    }
    case "cycle_time": {
      // A week in which nothing finished has no cycle time: a gap, not a zero.
      const value = (w: (typeof data.weeks)[number], v: number) => (w.issues ? Math.round(v * 10) / 10 : null);
      return {
        ...axisTooltip(theme, formatDays),
        xAxis: { ...categoryAxis(theme, data.weeks.map((w) => formatShortDay(w.week))), boundaryGap: false },
        yAxis: valueAxis(theme, "Days", false),
        series: [
          { ...line("Median", theme.series[0], data.weeks.map((w) => value(w, w.median_days)), theme), endLabel: endLabel(theme, formatDays) },
          { ...line("85th percentile", theme.series[1], data.weeks.map((w) => value(w, w.p85_days)), theme), endLabel: endLabel(theme, formatDays) },
        ],
      };
    }
    case "breakdown": {
      // One series: the title names it, so there is no legend. Bars run sideways so long
      // names stay readable, largest first, each with its count at the end.
      const base = chrome(theme);
      const labels = data.groups.map((g) => groupLabel(data.group_by, g.key, names));
      return {
        ...base,
        legend: { show: false },
        grid: { ...base.grid, top: 8, right: 40 },
        tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "shadow" } },
        xAxis: { ...valueAxis(theme, ""), name: undefined },
        yAxis: { ...categoryAxis(theme, labels), inverse: true, axisLabel: { color: theme.ink, width: 140, overflow: "truncate" } },
        series: [
          {
            ...bar(GROUP_LABEL[data.group_by], theme.series[0], data.groups.map((g) => g.count), [0, 4, 4, 0]),
            name: "Issues",
            barMaxWidth: 20,
            label: { show: true, position: "right", color: theme.ink },
          },
        ],
      };
    }
    case "velocity": {
      const base = chrome(theme);
      return {
        ...base,
        tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "shadow" } },
        xAxis: categoryAxis(theme, data.sprints.map((s) => s.name)),
        yAxis: valueAxis(theme, "Issues"),
        series: [
          bar("Completed", theme.series[0], data.sprints.map((s) => s.completed)),
          bar("Not completed", theme.series[1], data.sprints.map((s) => s.incomplete)),
        ],
      };
    }
  }
}
