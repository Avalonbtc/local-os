// HiveOS worker tabs RigDeck had no equivalent for: Autofan (critical temperature), the day-by-day
// statistics page with its event timeline, and the in-page worker settings.
import { useMemo, useState, type ReactNode } from "react";
import { Alert, App, Button, Input, InputNumber, Segmented, Select, Space, Switch, Tooltip, Typography } from "antd";
import { DownloadOutlined } from "@ant-design/icons";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  client,
  data,
  isFresh,
  unwrap,
  useMachines,
  type Data,
  type Machine,
  type MachineMessage,
  type Observation,
} from "../../shared/api";
import { Chart, QueryState, number } from "../../shared/ui";
import { updateMachine } from "./worker";

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

/** Merge `patch` into the machine policy; `undefined` removes a key. */
async function savePolicy(machine: Machine, patch: Data) {
  const policy: Data = { ...data(machine.policy) };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined || value === null) delete policy[key];
    else policy[key] = value;
  }
  await updateMachine(machine, { policy });
}

function useSave(machine: Machine) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const run = async (save: () => Promise<void>, done: string) => {
    setBusy(true);
    try {
      await save();
      await queryClient.invalidateQueries({ queryKey: ["machines"] });
      message.success(done);
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return { busy, run, policy: (patch: Data, done: string) => run(() => savePolicy(machine, patch), done) };
}

/** One HiveOS settings row: label and help on the left, control on the right. */
function Row({ label, help, children }: { label: ReactNode; help?: ReactNode; children: ReactNode }) {
  return (
    <div className="wk-set-row">
      <div className="wk-set-label">
        <b>{label}</b>
        {help && <small>{help}</small>}
      </div>
      <div className="wk-set-control">{children}</div>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Autofan
// ---------------------------------------------------------------------------------------------

const CRITICAL_ACTIONS = [
  { value: "stop_miner", label: "停止挖矿软件" },
  { value: "reboot", label: "重启" },
  { value: "shutdown", label: "关机" },
];

export function AutofanSettings({ machine, system }: { machine: Machine; system?: Observation }) {
  const policy = data(machine.policy);
  const [enabled, setEnabled] = useState(num(policy.critical_temp_c) !== undefined);
  const [critical, setCritical] = useState<number>(num(policy.critical_temp_c) ?? 90);
  const [action, setAction] = useState<string>(policy.critical_action ?? "stop_miner");
  const [warn, setWarn] = useState<number>(num(policy.gpu_temp_limit_c) ?? 85);
  const save = useSave(machine);
  const d = data(system?.data);
  const online = isFresh(system);
  const readings = [
    num(d.cpu_temperature) !== undefined && { name: "CPU", value: d.cpu_temperature as number },
    ...(Array.isArray(d.gpus) ? d.gpus : []).map((g: Data, i: number) =>
      num(g.temperature_c) !== undefined && { name: `GPU ${i}`, value: g.temperature_c as number },
    ),
  ].filter(Boolean) as { name: string; value: number }[];
  const hottest = readings.reduce<{ name: string; value: number } | undefined>(
    (max, r) => (!max || r.value > max.value ? r : max),
    undefined,
  );
  const dirty =
    enabled !== (num(policy.critical_temp_c) !== undefined) ||
    (enabled && critical !== policy.critical_temp_c) ||
    action !== (policy.critical_action ?? "stop_miner") ||
    warn !== (num(policy.gpu_temp_limit_c) ?? 85);
  const reset = () => {
    setEnabled(num(policy.critical_temp_c) !== undefined);
    setCritical(num(policy.critical_temp_c) ?? 90);
    setAction(policy.critical_action ?? "stop_miner");
    setWarn(num(policy.gpu_temp_limit_c) ?? 85);
  };
  return (
    <div className="wk-settings">
      <Alert
        type="info"
        showIcon
        message="风扇转速由超频配置、显卡驱动或 BMC 管理"
        description="这里设定的临界温度保护在矿机本地每 10 秒检查一次 CPU 和全部显卡的最高温度，主控离线时照常生效。"
      />
      <Row label="当前最高温度" help="CPU 与各显卡读数中的最大值">
        {online && hottest ? (
          <span className={hottest.value >= critical && enabled ? "wk-temp hot" : "wk-temp"}>
            {number(hottest.value, 0)}°C <small className="muted">（{hottest.name}）</small>
          </span>
        ) : (
          <span className="muted">—</span>
        )}
      </Row>
      <Row label="温度保护" help="关闭后不再因温度停止挖矿或重启">
        <Switch checked={enabled} onChange={setEnabled} />
      </Row>
      <Row label="Critical temp" help="任一设备达到此温度即执行临界动作（40–120°C）">
        <InputNumber min={40} max={120} value={critical} onChange={(v) => setCritical(v ?? 90)} disabled={!enabled} addonAfter="°C" />
      </Row>
      <Row
        label="Critical action"
        help={action === "stop_miner" ? "温度降到临界温度以下 10°C 后自动恢复挖矿" : "与算力监视器共用整机重启间隔，避免反复重启"}
      >
        <Select style={{ width: 200 }} value={action} onChange={setAction} options={CRITICAL_ACTIONS} disabled={!enabled} />
      </Row>
      <Row label="高温提示线" help="矿机列表和设备表达到此温度时标红，不触发任何动作">
        <InputNumber min={40} max={120} value={warn} onChange={(v) => setWarn(v ?? 85)} addonAfter="°C" />
      </Row>
      <Space className="wk-set-actions">
        <Button onClick={reset} disabled={!dirty}>
          Reset changes
        </Button>
        <Button
          type="primary"
          loading={save.busy}
          disabled={!dirty}
          onClick={() =>
            save.policy(
              {
                critical_temp_c: enabled ? critical : undefined,
                critical_action: enabled ? action : undefined,
                gpu_temp_limit_c: warn,
              },
              "温度保护已保存，约 1 分钟内下发到矿机",
            )
          }
        >
          Save settings
        </Button>
      </Space>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Statistics: one or three days ending at the selected day, event timeline above the charts.
// ---------------------------------------------------------------------------------------------

const DAY = 86400000;
const startOfDay = (t: number) => {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};
const LEVELS = [
  { value: "all", label: "All" },
  { value: "info", label: "Info" },
  { value: "success", label: "Success" },
  { value: "warning", label: "Warning" },
  { value: "error", label: "Danger" },
];

function useWindowHistory(id: string, kind: string, hours: number, until: string, live: boolean) {
  return useQuery({
    queryKey: ["history", id, kind, hours, until],
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/machines/{id}/history", {
          signal,
          params: { path: { id }, query: { kind, hours, until, summary: true } },
        }),
      ),
    refetchInterval: live ? 30000 : false,
  });
}

export function WorkerStats({ machine }: { machine: Machine }) {
  const today = startOfDay(Date.now());
  const [day, setDay] = useState(today);
  const [span, setSpan] = useState(1);
  const [level, setLevel] = useState("all");
  const end = day + DAY;
  const from = end - span * DAY;
  const until = new Date(end).toISOString();
  const live = day === today;
  const system = useWindowHistory(machine.id, "system", span * 24, until, live);
  const mining = useWindowHistory(machine.id, "mining", span * 24, until, live);
  const messages = useQuery({
    queryKey: ["messages", machine.id, 500],
    queryFn: ({ signal }) =>
      unwrap(client.GET("/api/v1/machines/{id}/messages", { signal, params: { path: { id: machine.id }, query: { limit: 500 } } })),
    refetchInterval: live ? 30000 : false,
  });
  const at = (o: Observation) => (data(o.data)._sample_observed_at as string | undefined) ?? o.observed_at;
  const series = (key: string) =>
    (system.data ?? []).map((o) => [at(o), num(data(o.data)[key]) ?? null] as [string, number | null]);
  const algorithms = useMemo(
    () =>
      [
        ...new Set(
          (mining.data ?? []).flatMap((o) =>
            (data(o.data).instances ?? []).map((i: Data) => i.stats?.algorithm).filter(Boolean),
          ),
        ),
      ] as string[],
    [mining.data],
  );
  const rate = (algorithm: string) =>
    (mining.data ?? []).map((o) => {
      const values = (data(o.data).instances ?? []).filter(
        (i: Data) =>
          i.stats?.algorithm === algorithm &&
          i.process_alive &&
          typeof i.stats?.hashrate_hs === "number" &&
          Math.abs(new Date(at(o)).getTime() / 1000 - (i.stats_observed_at ?? 0)) < 60,
      );
      return [at(o), values.length ? values.reduce((n: number, i: Data) => n + i.stats.hashrate_hs, 0) : null] as [string, number | null];
    });
  const events = (messages.data ?? []).filter((m: MachineMessage) => {
    const t = new Date(m.at).getTime();
    return t >= from && t <= end && (level === "all" || m.level === level);
  });
  const range: [number, number] = [from, end];
  const exportCsv = () => {
    const rows: string[] = ["time,metric,value"];
    const push = (metric: string, points: [string, number | null][]) =>
      points.forEach(([t, v]) => v !== null && rows.push(`${t},${metric},${v}`));
    algorithms.forEach((a) => push(`hashrate_hs:${a}`, rate(a)));
    ["cpu_pct", "cpu_temperature", "memory_pct", "cpu_power_w"].forEach((k) => push(k, series(k)));
    const url = URL.createObjectURL(new Blob([rows.join("\n")], { type: "text/csv" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${machine.name}-${new Date(day).toISOString().slice(0, 10)}-${span}d.csv`;
    link.click();
    URL.revokeObjectURL(url);
  };
  return (
    <>
      <div className="wk-stats-bar">
        <Button size="small" onClick={() => setDay(day - DAY)}>
          上一个
        </Button>
        <span className="wk-stats-day">
          {new Date(day).toLocaleDateString("zh-CN", { year: "numeric", month: "long", day: "numeric" })}
        </span>
        <Button size="small" disabled={live} onClick={() => setDay(Math.min(today, day + DAY))}>
          下一个
        </Button>
        <Segmented
          size="small"
          value={span}
          onChange={(v) => setSpan(Number(v))}
          options={[
            { value: 1, label: "1d" },
            { value: 3, label: "3d" },
          ]}
        />
        <Tooltip title="导出 CSV">
          <Button size="small" icon={<DownloadOutlined />} onClick={exportCsv} aria-label="导出 CSV" />
        </Tooltip>
      </div>
      <section className="wk-timeline-box">
        <header>
          <span>活动</span>
          <Segmented size="small" value={level} onChange={(v) => setLevel(String(v))} options={LEVELS} />
        </header>
        <div className="wk-timeline" aria-label="事件时间轴">
          {events.map((m) => (
            <Tooltip
              key={`${m.source}-${m.id}`}
              title={`${new Date(m.at).toLocaleString("zh-CN", { hour12: false })} · ${m.message}`}
            >
              <i
                className={`wk-event ${m.level}`}
                style={{ left: `${((new Date(m.at).getTime() - from) / (end - from)) * 100}%` }}
              />
            </Tooltip>
          ))}
        </div>
        <div className="wk-timeline-axis">
          {Array.from({ length: 7 }, (_, i) => from + ((end - from) * i) / 6).map((t) => (
            <span key={t}>
              {span > 1 && <small>{new Date(t).toLocaleDateString("zh-CN", { month: "numeric", day: "numeric" })} </small>}
              {new Date(t).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false })}
            </span>
          ))}
        </div>
        {events.length === 0 && <div className="muted wk-timeline-empty">这段时间没有事件</div>}
      </section>
      <QueryState loading={system.isLoading || mining.isLoading} error={system.error ?? mining.error}>
        <div className="overview-history">
          {algorithms.map((algorithm) => (
            <Chart key={algorithm} label={`算力 · ${algorithm}`} unit="H/s" points={rate(algorithm)} range={range} />
          ))}
          <Chart label="CPU 使用率" points={series("cpu_pct")} range={range} />
          <Chart label="CPU 温度" unit="°C" points={series("cpu_temperature")} range={range} />
          <Chart label="内存使用率" points={series("memory_pct")} range={range} />
          <Chart label="CPU 软件功耗" unit="W" points={series("cpu_power_w")} range={range} />
        </div>
      </QueryState>
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Worker settings
// ---------------------------------------------------------------------------------------------

export function WorkerOptions({ machine, connection }: { machine: Machine; connection: ReactNode }) {
  const policy = data(machine.policy);
  const machines = useMachines();
  const known = [...new Set((machines.data ?? []).flatMap((m) => m.tags))].sort();
  const [name, setName] = useState(machine.name);
  const [description, setDescription] = useState<string>(policy.description ?? "");
  const [tags, setTags] = useState<string[]>(machine.tags);
  const [maintenance, setMaintenance] = useState(policy.maintenance_mode === true);
  const [delay, setDelay] = useState<number | null>(num(policy.miner_delay_seconds) ?? null);
  const [hardware, setHardware] = useState<number | null>(num(policy.hardware_power_w) ?? null);
  const [psu, setPsu] = useState<number | null>(num(policy.psu_efficiency_pct) ?? null);
  const save = useSave(machine);
  return (
    <div className="wk-settings">
      <Row label="ID">
        <Typography.Text copyable className="wk-mono">
          {machine.id}
        </Typography.Text>
      </Row>
      <Row label="名称">
        <Input value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
      </Row>
      <Row label="描述">
        <Input.TextArea rows={3} maxLength={1000} value={description} onChange={(e) => setDescription(e.target.value)} />
      </Row>
      <Row label="标签" help="给矿机添加自定义标签">
        <Select
          mode="tags"
          style={{ width: "100%" }}
          placeholder="选择或开始输入"
          value={tags}
          onChange={setTags}
          options={known.map((t) => ({ value: t, label: t }))}
        />
      </Row>
      <Row label="">
        <Button
          type="primary"
          loading={save.busy}
          disabled={!name.trim()}
          onClick={() =>
            save.run(
              () =>
                updateMachine(machine, {
                  name: name.trim(),
                  tags: tags.map((t) => t.trim()).filter(Boolean),
                  policy: { ...policy, description: description.trim() || undefined },
                }),
              "矿机已更新",
            )
          }
        >
          更新矿机
        </Button>
      </Row>
      <hr />
      <Row label="维护模式" help="开机时不启动挖矿软件，算力监视器暂停。温度保护仍然生效；手动启动或应用飞行表照常执行。">
        <Space>
          <Switch checked={maintenance} onChange={setMaintenance} />
          <Button
            size="small"
            loading={save.busy}
            disabled={maintenance === (policy.maintenance_mode === true)}
            onClick={() => save.policy({ maintenance_mode: maintenance || undefined }, maintenance ? "已开启维护模式" : "已关闭维护模式")}
          >
            更新
          </Button>
        </Space>
      </Row>
      <Row label="挖矿软件延迟" help="设备开机时挖矿软件的延迟启动秒数（0–3600）">
        <Space>
          <InputNumber min={0} max={3600} value={delay} onChange={setDelay} addonAfter="秒" placeholder="0" />
          <Button
            size="small"
            loading={save.busy}
            disabled={(delay ?? null) === (num(policy.miner_delay_seconds) ?? null)}
            onClick={() => save.policy({ miner_delay_seconds: delay || undefined }, "已更新开机延迟")}
          >
            更新
          </Button>
        </Space>
      </Row>
      <hr />
      <Row label="硬件功耗，瓦" help="风扇、主板、内存等没有读数的部分，加到测得的 CPU/GPU 功耗上">
        <InputNumber min={0} max={10000} value={hardware} onChange={setHardware} addonAfter="W" />
      </Row>
      <Row label="供电单元效率，%" help="总功耗按此效率折算为墙插功耗（50–100）">
        <Space>
          <InputNumber min={50} max={100} value={psu} onChange={setPsu} addonAfter="%" />
          <Button
            size="small"
            loading={save.busy}
            onClick={() =>
              save.policy({ hardware_power_w: hardware || undefined, psu_efficiency_pct: psu && psu < 100 ? psu : undefined }, "功耗设定已更新")
            }
          >
            更新
          </Button>
        </Space>
      </Row>
      <hr />
      {connection}
    </div>
  );
}
