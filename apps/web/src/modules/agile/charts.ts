// ECharts options for the two sprint reports. One y-axis each; text uses ink tokens,
// only the marks carry series colour.
import type { EChartsCoreOption } from "echarts/core";

import type { VelocityReport } from "@/services/api";

import type { ChartTheme } from "./components/Chart";
import { formatPoints, type BurndownSeries, type BurndownUnit } from "./model";

const UNIT_LABEL: Record<BurndownUnit, string> = { points: "Story points", issues: "Issues" };

function chrome(theme: ChartTheme) {
  return {
    textStyle: { fontFamily: "inherit", color: theme.muted },
    grid: { left: 8, right: 24, top: 40, bottom: 8, containLabel: true },
    // Right-aligned so it never collides with the y-axis name at the top left.
    legend: { top: 0, right: 0, icon: "roundRect", itemWidth: 12, itemHeight: 12, textStyle: { color: theme.ink } },
    tooltip: {
      backgroundColor: theme.surface,
      borderColor: theme.line,
      textStyle: { color: theme.ink, fontSize: 12 },
      valueFormatter: (value: unknown) => (typeof value === "number" ? formatPoints(value) : String(value ?? "")),
    },
  };
}

const valueAxis = (theme: ChartTheme, name: string) => ({
  type: "value",
  name,
  nameTextStyle: { color: theme.muted, align: "left" },
  minInterval: 1,
  axisLabel: { color: theme.muted },
  splitLine: { lineStyle: { color: theme.line } },
});

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
  const bar = (name: string, color: string, data: number[]) => ({
    name,
    type: "bar",
    data,
    color,
    barMaxWidth: 28,
    barGap: "10%",
    itemStyle: { borderRadius: [4, 4, 0, 0] },
  });
  return {
    ...base,
    tooltip: { ...base.tooltip, trigger: "axis", axisPointer: { type: "shadow" } },
    xAxis: {
      type: "category",
      data: sprints.map((s) => s.name),
      axisLine: { lineStyle: { color: theme.line } },
      axisTick: { show: false },
      axisLabel: { color: theme.muted, hideOverlap: true },
    },
    yAxis: valueAxis(theme, UNIT_LABEL[unit]),
    series: [
      bar("Committed", theme.series[0], sprints.map((s) => (unit === "points" ? s.committed_points : s.committed_issues))),
      bar("Completed", theme.series[1], sprints.map((s) => (unit === "points" ? s.completed_points : s.completed_issues))),
    ],
  };
}
