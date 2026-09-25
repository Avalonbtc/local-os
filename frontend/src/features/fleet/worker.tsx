// HiveOS-style worker page building blocks: message feed, read-only miner log,
// watchdog form, one-click actions and ranged performance charts.
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Alert,
  App,
  Button,
  Dropdown,
  Empty,
  Form,
  Input,
  InputNumber,
  Modal,
  Segmented,
  Select,
  Space,
  Switch,
  Tag,
  Tooltip,
} from "antd";
import {
  CheckCircleFilled,
  CloseCircleFilled,
  ExclamationCircleFilled,
  InfoCircleFilled,
  ReloadOutlined,
} from "@ant-design/icons";
import type { MenuProps } from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import {
  IconBiosChip,
  IconLog,
  IconPlayStop,
  IconPlug,
  IconPower,
  IconRestart,
  IconRocket,
  IconScreen,
  IconSettings,
  IconTerminal,
  IconUpload,
} from "../../shared/icons";
import {
  client,
  data,
  submitJob,
  unwrap,
  type Data,
  type Machine,
  type MachineInput,
  type MachineMessage,
} from "../../shared/api";
import { Chart, QueryState } from "../../shared/ui";

const levelIcon: Record<string, ReactNode> = {
  success: <CheckCircleFilled className="msg-icon success" />,
  warning: <ExclamationCircleFilled className="msg-icon warning" />,
  error: <CloseCircleFilled className="msg-icon error" />,
  info: <InfoCircleFilled className="msg-icon info" />,
};
const sourceLabel: Record<string, string> = {
  runtime: "矿机",
  job: "任务",
  audit: "操作",
};

function timeLabel(at: string) {
  const date = new Date(at);
  const today = new Date();
  return date.toDateString() === today.toDateString()
    ? date.toLocaleTimeString("zh-CN", { hour12: false })
    : date.toLocaleString("zh-CN", { hour12: false });
}

export function MessagesFeed({ id, limit = 200 }: { id: string; limit?: number }) {
  const [level, setLevel] = useState<string>("all");
  const query = useQuery({
    queryKey: ["messages", id, limit],
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/machines/{id}/messages", {
          signal,
          params: { path: { id }, query: { limit } },
        }),
      ),
    refetchInterval: 15000,
  });
  const rows = (query.data ?? []).filter(
    (m) => level === "all" || (level === "problems" ? m.level === "warning" || m.level === "error" : m.source === level),
  );
  return (
    <QueryState loading={query.isLoading} error={query.error}>
      {limit > 20 && (
        <Segmented
          className="msg-filter"
          size="small"
          value={level}
          onChange={(v) => setLevel(String(v))}
          options={[
            { value: "all", label: "全部" },
            { value: "problems", label: "告警与错误" },
            { value: "runtime", label: "矿机事件" },
            { value: "job", label: "任务" },
            { value: "audit", label: "操作记录" },
          ]}
        />
      )}
      {rows.length === 0 ? (
        <Empty description="暂无消息" />
      ) : (
        <ul className="msg-feed">
          {rows.map((m: MachineMessage) => (
            <li key={`${m.source}-${m.id}`} className={`msg-${m.level}`}>
              {levelIcon[m.level] ?? levelIcon.info}
              <time>{timeLabel(m.at)}</time>
              <Tag className="msg-source">{sourceLabel[m.source] ?? m.source}</Tag>
              {m.instance && <Tag className="msg-instance">{m.instance}</Tag>}
              <span className="msg-text">
                {m.message}
                {m.source === "job" && <JobMessageDetail id={id} detail={data(m.detail)} />}
              </span>
            </li>
          ))}
        </ul>
      )}
    </QueryState>
  );
}

/** Command output and the cancel / reconcile controls that used to live on the jobs page. */
function JobMessageDetail({ id, detail }: { id: string; detail: Data }) {
  const { message, modal } = App.useApp();
  const queryClient = useQueryClient();
  const [resolving, setResolving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [form] = Form.useForm();
  const status = String(detail.status ?? "");
  const output = typeof detail.output === "string" ? detail.output : "";
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["messages", id] });
  return (
    <>
      {output && (
        <details className="msg-output">
          <summary>输出</summary>
          <pre>
            {output}
            {detail.output_truncated && "\n[输出过长，已截断]"}
          </pre>
        </details>
      )}
      {["queued", "running"].includes(status) && detail.job_id && (
        <Button
          type="link"
          size="small"
          danger
          onClick={() =>
            modal.confirm({
              title: "取消这个任务？",
              content: "同一批次里还没开始的矿机都不会再执行；已经在执行的会跑完。",
              okButtonProps: { danger: true },
              onOk: async () => {
                try {
                  await unwrap(client.POST("/api/v1/jobs/{id}/cancel", { params: { path: { id: String(detail.job_id) } } }));
                  message.success("已请求取消");
                  await refresh();
                } catch (e) {
                  message.error((e as Error).message);
                }
              },
            })
          }
        >
          取消
        </Button>
      )}
      {status === "unknown" && detail.target_id && (
        <Button type="link" size="small" onClick={() => { form.resetFields(); setResolving(true); }}>
          核实与对账
        </Button>
      )}
      <Modal
        title="处理结果不明的操作"
        open={resolving}
        onCancel={() => setResolving(false)}
        onOk={() => form.submit()}
        confirmLoading={busy}
        destroyOnHidden
      >
        <Alert
          type="warning"
          message="结果不明时这台矿机会保持锁定。重新对账只读取原操作记录；人工标记会解除锁定，请先核实进程、运行版本和电源状态。"
        />
        <Form
          form={form}
          layout="vertical"
          initialValues={{ decision: "reconcile" }}
          onFinish={async (values) => {
            setBusy(true);
            try {
              await unwrap(
                client.POST("/api/v1/job-targets/{id}/resolve", {
                  params: { path: { id: String(detail.target_id) } },
                  body: values,
                }),
              );
              setResolving(false);
              message.success("已记录");
              await refresh();
            } catch (e) {
              message.error((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          <Form.Item name="decision" label="处理方式" rules={[{ required: true }]}>
            <Select
              options={[
                { value: "reconcile", label: "重新查询原操作" },
                { value: "succeeded", label: "已人工核实：成功" },
                { value: "failed", label: "已人工核实：失败" },
                { value: "cancelled", label: "已人工核实：未执行" },
              ]}
            />
          </Form.Item>
          <Form.Item name="note" label="核实依据（记入操作记录）" rules={[{ required: true, min: 10 }]}>
            <Input.TextArea rows={3} placeholder="例如检查了哪些命令、看到的运行版本，至少 10 个字符" />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}

export function MinerLogViewer({ id, instances, live = false }: { id: string; instances: Data[]; live?: boolean }) {
  const [instance, setInstance] = useState<string | undefined>(instances[0]?.instance);
  const [lines, setLines] = useState(200);
  const [follow, setFollow] = useState(live);
  const selected = instance ?? instances[0]?.instance;
  const query = useQuery({
    queryKey: ["miner-log", id, selected, lines],
    enabled: !!selected,
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/machines/{id}/instances/{instance}/log", {
          signal,
          params: { path: { id, instance: selected! }, query: { lines } },
        }),
      ),
    refetchInterval: follow ? 5000 : false,
  });
  // Like HiveOS: the log opens at its newest lines and stays there while following.
  const pre = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (pre.current) pre.current.scrollTop = pre.current.scrollHeight;
  }, [query.data?.text]);
  if (!instances.length) return <Alert type="info" message="没有托管矿工实例" />;
  return (
    <>
      <Space className="detail-actions" wrap>
        <Select
          value={selected}
          onChange={setInstance}
          style={{ minWidth: 200 }}
          options={instances.map((i) => ({ value: i.instance, label: i.instance }))}
        />
        <Select
          value={lines}
          onChange={setLines}
          options={[100, 200, 500, 1000].map((n) => ({ value: n, label: `最近 ${n} 行` }))}
        />
        <Button icon={<ReloadOutlined />} onClick={() => query.refetch()} loading={query.isFetching}>
          刷新
        </Button>
        <Space>
          <Switch size="small" checked={follow} onChange={setFollow} />
          每 5 秒自动刷新
        </Space>
      </Space>
      <QueryState loading={query.isLoading} error={query.error}>
        <pre className="miner-log" ref={pre}>
          {query.data?.text || "日志为空"}
        </pre>
      </QueryState>
    </>
  );
}

type WatchdogValues = {
  watchdog_enabled: boolean;
  min_hashrate: number | null;
  min_hashrate_unit: number;
  failure_minutes: number;
  max_restarts: number;
  restart_window_minutes: number;
  cooldown_seconds: number;
  allow_host_reboot: boolean;
  host_reboot_cooldown_minutes: number;
};

function machineInput(machine: Machine, policy: Data): MachineInput {
  return {
    name: machine.name,
    host: machine.host,
    port: machine.port,
    username: machine.username,
    host_key: machine.host_key,
    group: machine.group,
    tags: machine.tags,
    is_controller: machine.is_controller,
    bmc: machine.bmc ?? null,
    credential: null,
    sudo_password: null,
    bmc_credential: null,
    policy,
  };
}

const UNITS = [
  { value: 1, label: "H/s" },
  { value: 1e3, label: "kH/s" },
  { value: 1e6, label: "MH/s" },
  { value: 1e9, label: "GH/s" },
];

export function WatchdogSettings({ machine }: { machine: Machine }) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const policy = data(machine.policy);
  const minimum = typeof policy.min_hashrate_hs === "number" ? policy.min_hashrate_hs : null;
  const unit = minimum && minimum >= 1e9 ? 1e9 : minimum && minimum >= 1e6 ? 1e6 : minimum && minimum >= 1e3 ? 1e3 : 1;
  const initial: WatchdogValues = {
    watchdog_enabled: policy.watchdog_enabled !== false,
    min_hashrate: minimum === null ? null : minimum / unit,
    min_hashrate_unit: unit,
    failure_minutes: (policy.failure_seconds ?? 120) / 60,
    max_restarts: policy.max_restarts ?? 5,
    restart_window_minutes: (policy.restart_window_seconds ?? 3600) / 60,
    cooldown_seconds: policy.cooldown_seconds ?? 30,
    allow_host_reboot: policy.allow_host_reboot === true,
    host_reboot_cooldown_minutes: (policy.host_reboot_cooldown_seconds ?? 3600) / 60,
  };
  return (
    <Form<WatchdogValues>
      className="watchdog-form"
      layout="vertical"
      initialValues={initial}
      onFinish={async (v) => {
        setBusy(true);
        try {
          const next: Data = {
            ...policy,
            watchdog_enabled: v.watchdog_enabled,
            failure_seconds: Math.round(v.failure_minutes * 60),
            max_restarts: v.max_restarts,
            restart_window_seconds: Math.round(v.restart_window_minutes * 60),
            cooldown_seconds: v.cooldown_seconds,
            allow_host_reboot: v.allow_host_reboot,
            host_reboot_cooldown_seconds: Math.round(v.host_reboot_cooldown_minutes * 60),
          };
          if (v.min_hashrate === null || v.min_hashrate === undefined) delete next.min_hashrate_hs;
          else next.min_hashrate_hs = v.min_hashrate * v.min_hashrate_unit;
          await unwrap(
            client.PUT("/api/v1/machines/{id}", {
              params: { path: { id: machine.id } },
              body: machineInput(machine, next),
            }),
          );
          await queryClient.invalidateQueries({ queryKey: ["machines"] });
          message.success("看门狗设置已保存，约 1 分钟内下发到矿机，无需重新应用飞行表");
        } catch (e) {
          message.error((e as Error).message);
        } finally {
          setBusy(false);
        }
      }}
    >
      <Alert
        type="info"
        showIcon
        message="看门狗在矿机本地运行，主控离线时照常生效"
        description="算力低于阈值、统计接口无响应或矿池断开持续超过设定时间，就重启矿工；恢复次数用尽后可选择重启整机。"
      />
      <Form.Item name="watchdog_enabled" label="启用算力看门狗" valuePropName="checked">
        <Switch />
      </Form.Item>
      <Form.Item label="最低算力（留空表示只检查“有算力”）">
        <Space.Compact>
          <Form.Item name="min_hashrate" noStyle>
            <InputNumber min={0} style={{ width: 160 }} placeholder="例如 20" />
          </Form.Item>
          <Form.Item name="min_hashrate_unit" noStyle>
            <Select style={{ width: 100 }} options={UNITS} />
          </Form.Item>
        </Space.Compact>
      </Form.Item>
      <div className="form-grid">
        <Form.Item name="failure_minutes" label="异常持续多久后重启矿工（分钟）">
          <InputNumber min={0.5} max={120} step={0.5} />
        </Form.Item>
        <Form.Item name="cooldown_seconds" label="两次重启最短间隔（秒）">
          <InputNumber min={10} max={3600} />
        </Form.Item>
        <Form.Item name="max_restarts" label="时间窗内最多自动重启（次）">
          <InputNumber min={1} max={50} />
        </Form.Item>
        <Form.Item name="restart_window_minutes" label="重启次数统计窗口（分钟）">
          <InputNumber min={5} max={1440} />
        </Form.Item>
      </div>
      <Form.Item name="allow_host_reboot" label="重启次数用尽后重启整机" valuePropName="checked">
        <Switch />
      </Form.Item>
      <Form.Item name="host_reboot_cooldown_minutes" label="整机重启最短间隔（分钟）">
        <InputNumber min={10} max={1440} />
      </Form.Item>
      <Button type="primary" htmlType="submit" loading={busy}>
        保存看门狗设置
      </Button>
    </Form>
  );
}

// Detached so the job reports success before the host goes down (otherwise it ends "unknown").
const REBOOT = "setsid -f sh -c 'sleep 3; systemctl reboot' >/dev/null 2>&1 </dev/null";
const SHUTDOWN = "setsid -f sh -c 'sleep 3; systemctl poweroff' >/dev/null 2>&1 </dev/null";

export type WorkerPanel = "log" | "console" | "ssh" | "settings";

/** HiveOS-style icon toolbar on the worker header: every action is an icon with a tooltip. */
export function WorkerActions({
  machine,
  instances,
  onBatch,
  onOpen,
}: {
  machine: Machine;
  instances: Data[];
  onBatch: (kind: string) => void;
  onOpen: (panel: WorkerPanel) => void;
}) {
  const { message, modal } = App.useApp();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const names = instances.map((i) => i.instance as string).filter(Boolean);
  const run = (title: string, action: Parameters<typeof submitJob>[0]["action"], danger = false) =>
    modal.confirm({
      title,
      content: `目标：${machine.name}${machine.is_controller ? "（主控机器）" : ""}`,
      okButtonProps: { danger },
      onOk: async () => {
        try {
          await submitJob({
            machine_ids: [machine.id],
            action,
            concurrency: 1,
            canary: false,
            include_controller: machine.is_controller,
          });
          await queryClient.invalidateQueries({ queryKey: ["messages", machine.id] });
          message.success("已提交，结果会出现在消息里");
        } catch (e) {
          message.error((e as Error).message);
        }
      },
    });
  const miner = (operation: "start" | "stop" | "restart", label: string) =>
    run(`${label}全部矿工（${names.join(", ")}）？`, { kind: "miner", operation, instances: names }, operation === "stop");
  const tool = (title: string, icon: ReactNode, onClick?: () => void, disabled = false, danger = false) => (
    <Tooltip title={title}>
      <button
        type="button"
        className={`wk-tool ${danger ? "danger" : ""}`}
        aria-label={title}
        disabled={disabled}
        onClick={onClick}
      >
        {icon}
      </button>
    </Tooltip>
  );
  const menuTool = (title: string, icon: ReactNode, items: MenuProps["items"], onClick: MenuProps["onClick"], disabled = false) => (
    <Dropdown trigger={["click"]} disabled={disabled} menu={{ items, onClick }}>
      <Tooltip title={title}>
        <button type="button" className="wk-tool" aria-label={title} disabled={disabled}>
          {icon}
        </button>
      </Tooltip>
    </Dropdown>
  );
  return (
    <div className="wk-tools" role="toolbar" aria-label="矿机操作">
      {tool("重启矿工", <IconRestart />, () => miner("restart", "重启"), !names.length)}
      {menuTool(
        "启动 / 停止矿工",
        <IconPlayStop />,
        [
          { key: "start", label: "启动全部矿工" },
          { key: "stop", label: "停止全部矿工", danger: true },
          { type: "divider" },
          { key: "pick", label: "逐个实例控制…" },
        ],
        ({ key }) => (key === "pick" ? onBatch("miner") : miner(key as "start" | "stop", key === "start" ? "启动" : "停止")),
        !names.length,
      )}
      {tool("切换飞行表", <IconRocket />, () => onBatch("apply"))}
      {tool("矿工日志", <IconLog />, () => onOpen("log"), !names.length)}
      {tool("矿工控制台", <IconScreen />, () => onOpen("console"), !names.length)}
      {tool("SSH 终端", <IconTerminal />, () => onOpen("ssh"))}
      {menuTool(
        "系统",
        <IconPower />,
        [
          { key: "reboot", label: "重启系统" },
          { key: "shutdown", label: "关机", danger: true },
          { type: "divider" },
          { key: "command", label: "执行命令…" },
          { key: "bootstrap", label: "部署 / 更新运行层" },
        ],
        ({ key }) => {
          if (key === "reboot")
            run("重启系统？矿工会在开机后按期望状态自动恢复。", { kind: "command", script: REBOOT, timeout_seconds: 60 });
          else if (key === "shutdown")
            run("关机？关机后只能通过 BMC 或现场开机。", { kind: "command", script: SHUTDOWN, timeout_seconds: 60 }, true);
          else onBatch(key);
        },
      )}
      {menuTool(
        machine.bmc ? "BMC 电源" : "BMC 电源（未配置 BMC）",
        <IconPlug />,
        [
          { key: "on", label: "开机" },
          { key: "shutdown", label: "优雅关机" },
          { key: "force_restart", label: "强制重启（断电重启）", danger: true },
          { key: "force_off", label: "强制断电", danger: true },
        ],
        ({ key }) =>
          run(
            `通过 BMC 执行「${{ on: "开机", shutdown: "优雅关机", force_restart: "强制重启", force_off: "强制断电" }[key]}」？`,
            { kind: "power", operation: key },
            key.startsWith("force"),
          ),
        !machine.bmc,
      )}
      {tool("BIOS", <IconBiosChip />, () => navigate(`/bios/${machine.id}`), !machine.bmc)}
      {tool("部署 / 更新运行层", <IconUpload />, () => onBatch("bootstrap"))}
      {tool("设定", <IconSettings />, () => onOpen("settings"))}
    </div>
  );
}

const RANGES = [
  { value: 1, label: "1 小时" },
  { value: 6, label: "6 小时" },
  { value: 24, label: "24 小时" },
  { value: 168, label: "7 天" },
  { value: 720, label: "30 天" },
];

function useHistory(id: string, kind: string, hours: number, enabled = true) {
  return useQuery({
    queryKey: ["history", id, kind, hours],
    enabled,
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/machines/{id}/history", {
          signal,
          params: { path: { id }, query: { kind, hours, summary: true } },
        }),
      ),
    refetchInterval: hours <= 24 ? 30000 : 300000,
  });
}

export function PerformanceCharts({ machine }: { machine: Machine }) {
  const [hours, setHours] = useState(6);
  const system = useHistory(machine.id, "system", hours);
  const mining = useHistory(machine.id, "mining", hours);
  const at = (o: { observed_at: string; data: unknown }) =>
    (data(o.data)._sample_observed_at as string | undefined) ?? o.observed_at;
  const algorithms = [
    ...new Set(
      (mining.data ?? []).flatMap((o) =>
        (data(o.data).instances ?? []).map((i: Data) => i.stats?.algorithm).filter(Boolean),
      ),
    ),
  ] as string[];
  const series = (key: string) =>
    (system.data ?? []).map((o) => [at(o), typeof data(o.data)[key] === "number" ? data(o.data)[key] : null] as [string, number | null]);
  return (
    <>
      <Segmented
        className="range-switch"
        value={hours}
        onChange={(v) => setHours(Number(v))}
        options={RANGES}
      />
      <QueryState loading={system.isLoading} error={system.error}>
        <div className="overview-history">
          {algorithms.map((algorithm) => (
            <Chart
              key={algorithm}
              label={`算力 · ${algorithm}`}
              unit="H/s"
              points={(mining.data ?? []).map((o) => {
                const values = (data(o.data).instances ?? []).filter(
                  (i: Data) =>
                    i.stats?.algorithm === algorithm &&
                    i.process_alive &&
                    typeof i.stats?.hashrate_hs === "number" &&
                    Math.abs(new Date(at(o)).getTime() / 1000 - (i.stats_observed_at ?? 0)) < 60,
                );
                return [at(o), values.length ? values.reduce((n: number, i: Data) => n + i.stats.hashrate_hs, 0) : null];
              })}
            />
          ))}
          <Chart label="CPU 使用率" points={series("cpu_pct")} />
          <Chart label="CPU 温度" unit="°C" points={series("cpu_temperature")} />
          <Chart label="内存使用率" points={series("memory_pct")} />
          <Chart label="CPU 软件功耗" unit="W" points={series("cpu_power_w")} />
        </div>
      </QueryState>
    </>
  );
}
