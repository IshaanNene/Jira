// Pieces every chart in the app shares: recessive chrome, ink-coloured text, one value axis.
import type { ChartTheme } from "./Chart";

/** Text, grid, legend and tooltip. Text wears ink tokens; only marks carry series colour. */
export function chrome(theme: ChartTheme, format: (value: number) => string = String) {
  return {
    textStyle: { fontFamily: "inherit", color: theme.muted },
    grid: { left: 8, right: 24, top: 40, bottom: 8, containLabel: true },
    // Right-aligned so it never collides with the y-axis name at the top left.
    legend: { top: 0, right: 0, icon: "roundRect", itemWidth: 12, itemHeight: 12, textStyle: { color: theme.ink } },
    tooltip: {
      backgroundColor: theme.surface,
      borderColor: theme.line,
      textStyle: { color: theme.ink, fontSize: 12 },
      valueFormatter: (value: unknown) => (typeof value === "number" ? format(value) : String(value ?? "")),
    },
  };
}

export const valueAxis = (theme: ChartTheme, name: string, integer = true) => ({
  type: "value",
  name,
  nameTextStyle: { color: theme.muted, align: "left" },
  minInterval: integer ? 1 : undefined,
  axisLabel: { color: theme.muted },
  splitLine: { lineStyle: { color: theme.line } },
});

export const categoryAxis = (theme: ChartTheme, data: string[]) => ({
  type: "category",
  data,
  axisLine: { lineStyle: { color: theme.line } },
  axisTick: { show: false },
  axisLabel: { color: theme.muted, hideOverlap: true },
});

/** A 2px line with 8px markers ringed in the surface colour; markers hide when crowded. */
export const line = (name: string, color: string, data: unknown[], theme: ChartTheme) => ({
  name,
  type: "line",
  data,
  color,
  lineStyle: { width: 2 },
  symbol: "circle",
  symbolSize: 8,
  showSymbol: data.length <= 20,
  itemStyle: { borderColor: theme.surface, borderWidth: 2 },
});

/** A bar with a 4px rounded data end and a 2px gap to its neighbour. */
export const bar = (name: string, color: string, data: number[], radius: number[] = [4, 4, 0, 0]) => ({
  name,
  type: "bar",
  data,
  color,
  barMaxWidth: 28,
  barGap: "10%",
  itemStyle: { borderRadius: radius },
});
