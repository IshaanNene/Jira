// ECharts options for the two sprint reports. One y-axis each; text uses ink tokens,
// only the marks carry series colour.
import type { EChartsCoreOption } from "echarts/core";

import type { ChartTheme } from "@/components/charts/Chart";
import { bar, categoryAxis, chrome as sharedChrome, valueAxis } from "@/components/charts/options";
import type { VelocityReport } from "@/services/api";

import { formatPoints, type BurndownSeries, type BurndownUnit } from "./model";

const UNIT_LABEL: Record<BurndownUnit, string> = { points: "Story points", issues: "Issues" };

const chrome = (theme: ChartTheme) => sharedChrome(theme, formatPoints);

export function burndownOption(series: BurndownSeries, unit: BurndownUnit, theme: ChartTheme): EChartsCoreOption {
  const base = chrome(theme);
  return {
    ...base,
    tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "line", lineStyle: { color: theme.muted } } },
    xAxis: {
      type: "time",
      axisLine: { lineStyle: { color: theme.line } },
      axisLabel: { color: theme.muted, hideOverlap: true },
      splitLine: { show: false },
    },
    yAxis: valueAxis(theme, UNIT_LABEL[unit]),
    series: [
      {
        name: "Remaining",
        type: "line",
        data: series.remaining,
        color: theme.series[0],
        lineStyle: { width: 2 },
        symbol: "circle",
        symbolSize: 8,
        showSymbol: series.remaining.length <= 20,
        itemStyle: { borderColor: theme.surface, borderWidth: 2 },
        endLabel: { show: true, color: theme.ink, formatter: (p: { value: [number, number] }) => formatPoints(p.value[1]) },
      },
      {
        name: "Guideline",
        type: "line",
        data: series.guideline,
        color: theme.reference,
        lineStyle: { width: 2, type: "dashed" },
        showSymbol: false,
        tooltip: { show: false },
      },
    ],
  };
}

export function velocityOption(sprints: VelocityReport["sprints"], unit: BurndownUnit, theme: ChartTheme): EChartsCoreOption {
  const base = chrome(theme);
  return {
    ...base,
    tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "shadow" } },
    xAxis: categoryAxis(theme, sprints.map((s) => s.name)),
    yAxis: valueAxis(theme, UNIT_LABEL[unit]),
    series: [
      bar("Committed", theme.series[0], sprints.map((s) => (unit === "points" ? s.committed_points : s.committed_issues))),
      bar("Completed", theme.series[1], sprints.map((s) => (unit === "points" ? s.completed_points : s.completed_issues))),
    ],
  };
}
