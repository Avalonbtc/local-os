// Only the pieces the statistics charts use (line series on a time axis). Loaded on demand the
// first time a chart renders, so pages without charts never download ECharts.
import { init, use } from "echarts/core";
import { LineChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

use([LineChart, GridComponent, TooltipComponent, CanvasRenderer]);

export { init };
