import { Alert, Empty, Spin, Tag } from "antd";
import type { ReactNode } from "react";
import { useEffect, useRef } from "react";
import * as echarts from "echarts";
import { data, isFresh, type Observation } from "./api";

export function PageHeader({
  title,
  description,
  extra,
}: {
  title: string;
  description?: string;
  extra?: ReactNode;
}) {
  return (
    <div className="page-header">
      <div>
        <h1>{title}</h1>
        {description && <p>{description}</p>}
      </div>
      <div className="page-actions">{extra}</div>
    </div>
  );
}
export function QueryState({
  loading,
  error,
  empty,
  children,
}: {
  loading?: boolean;
  error?: Error | null;
  empty?: boolean;
  children: ReactNode;
}) {
  if (loading)
    return (
      <div className="query-state">
        <Spin />
      </div>
    );
  if (error)
    return (
      <Alert
        type="error"
        showIcon
        message="读取失败"
        description={error.message}
      />
    );
  if (empty) return <Empty description="暂无记录" />;
  return <>{children}</>;
}
const statuses: Record<string, [string, string]> = {
  running: ["processing", "执行中"],
  挖矿中: ["success", "挖矿中"],
  queued: ["default", "排队"],
  reconciling: ["warning", "对账中"],
  unknown: ["warning", "结果待核实"],
  failed: ["error", "失败"],
  succeeded: ["success", "成功"],
  cancelled: ["default", "已取消"],
  blocked: ["warning", "未执行"],
  stopped: ["default", "已停止"],
  starting: ["processing", "启动中"],
  cooldown: ["warning", "冷却中"],
  faulted: ["error", "恢复已暂停"],
};
export function Status({ value }: { value?: string | null }) {
  const [color, label] = statuses[value ?? ""] ?? ["default", value ?? "未知"];
  return <Tag color={color}>{label}</Tag>;
}
export function PowerState({ observation }: { observation?: Observation }) {
  if (!isFresh(observation, 90))
    return <Tag color={observation?.error ? "warning" : "default"}>{observation?.error ? "读取失败" : "未采集"}</Tag>;
  const state = String(data(observation?.data).state ?? "").toLowerCase();
  if (state === "on") return <Tag color="success">已开机</Tag>;
  if (state === "off") return <Tag>已关机</Tag>;
  return <Tag color="warning">状态未知</Tag>;
}
export const number = (n: unknown, digits = 1) =>
  typeof n === "number"
    ? n.toLocaleString("zh-CN", { maximumFractionDigits: digits })
    : "—";
export function bytes(n: unknown) {
  if (typeof n !== "number") return "—";
  return n >= 1024 ** 3
    ? `${number(n / 1024 ** 3)} GiB`
    : `${number(n / 1024 ** 2)} MiB`;
}
export function hashrate(n: unknown) {
  if (typeof n !== "number") return "—";
  for (const [scale, unit] of [
    [1e12, "TH/s"],
    [1e9, "GH/s"],
    [1e6, "MH/s"],
    [1e3, "kH/s"],
    [1, "H/s"],
  ] as const)
    if (n >= scale || scale === 1) return `${number(n / scale, 2)} ${unit}`;
  return "—";
}
export function JsonView({ value }: { value: unknown }) {
  return <pre className="json-view">{JSON.stringify(value, null, 2)}</pre>;
}
export function Chart({
  points,
  label,
  unit = "%",
}: {
  points: [string, number | null][];
  label: string;
  unit?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!ref.current) return;
    const chart = echarts.init(ref.current, "dark");
    chart.setOption({
      backgroundColor: "transparent",
      grid: { left: 55, right: 20, top: 35, bottom: 35 },
      tooltip: { trigger: "axis" },
      xAxis: {
        type: "time",
        axisLine: { lineStyle: { color: "#424650" } },
        splitLine: { show: false },
      },
      yAxis: {
        type: "value",
        name: unit,
        splitLine: { lineStyle: { color: "#252932" } },
      },
      series: [
        {
          name: label,
          type: "line",
          showSymbol: false,
          connectNulls: false,
          data: points,
          lineStyle: { color: "#c6dc62", width: 2 },
          areaStyle: { color: "rgba(198,220,98,.06)" },
        },
      ],
    });
    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(ref.current);
    return () => {
      observer.disconnect();
      chart.dispose();
    };
  }, [points, label, unit]);
  return (
    <figure className="metric-chart">
      <figcaption>{label}</figcaption>
      {!points.some(([, v]) => v !== null) && (
        <span className="muted">暂无统计数据</span>
      )}
      <div className="chart" ref={ref} />
    </figure>
  );
}
