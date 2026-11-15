import { BarChart, LineChart } from "echarts/charts";
import { GridComponent, LegendComponent, TooltipComponent } from "echarts/components";
import { init, use as registerCharts, type EChartsCoreOption } from "echarts/core";
import { SVGRenderer } from "echarts/renderers";
import { useEffect, useRef } from "react";

import { useUi } from "@/state/ui";

registerCharts([LineChart, BarChart, GridComponent, LegendComponent, TooltipComponent, SVGRenderer]);

/** Colours for chart chrome and series, in the current theme. */
export interface ChartTheme {
  /** Two series colours, checked for colour-blind separation against each theme's surface. */
  series: [string, string];
  /** Ordered stages (to do, in progress, done): one hue, stronger towards done. */
  stages: [string, string, string];
  reference: string;
  ink: string;
  muted: string;
  line: string;
  surface: string;
}

function readTheme(): ChartTheme {
  const root = document.documentElement;
  const dark = root.classList.contains("dark");
  const token = (name: string, fallback: string) => {
    const value = getComputedStyle(root).getPropertyValue(name).trim();
    return value ? `hsl(${value.replace(/ /g, ", ")})` : fallback;
  };
  return {
    series: dark ? ["#3B82F6", "#D97706"] : ["#0C66E4", "#E56910"],
    stages: dark ? ["#0055CC", "#388BFF", "#85B8FF"] : ["#85B8FF", "#388BFF", "#0055CC"],
    reference: dark ? "#8C9BAB" : "#8590A2",
    ink: token("--ink", dark ? "#C7D1DB" : "#172B4D"),
    muted: token("--ink-muted", dark ? "#9FADBC" : "#44546F"),
    line: token("--line", dark ? "#38414A" : "#DCDFE4"),
    surface: token("--surface", dark ? "#1D2125" : "#FFFFFF"),
  };
}

/**
 * An ECharts chart that follows the app theme and its container's width. The data
 * behind every chart is also shown as a table on the page, so `label` only names it.
 */
export function Chart({ build, label, height = 300 }: { build: (theme: ChartTheme) => EChartsCoreOption; label: string; height?: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const theme = useUi((s) => s.theme);

  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const chart = init(node, undefined, { renderer: "svg" });
    chart.setOption(build(readTheme()));
    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(node);
    return () => {
      observer.disconnect();
      chart.dispose();
    };
  }, [build, theme]);

  return <div ref={ref} role="img" aria-label={label} style={{ height }} className="w-full" />;
}
