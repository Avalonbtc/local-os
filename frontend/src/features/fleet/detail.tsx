import { lazy, Suspense, useState } from "react";
import {
  Alert,
  App,
  Button,
  Descriptions,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Spin,
  Table,
  Tabs,
  Tag,
} from "antd";
import {
  useParams,
  useSearchParams,
  Link,
} from "react-router-dom";
import {
  client,
  data,
  unwrap,
  useMachines,
  useTelemetry,
  latest,
  serverNow,
  machinePower,
  submitJob,
  type Data,
} from "../../shared/api";
import {
  QueryState,
  bytes,
  number,
  hashrate,
  Status,
  PowerState,
  JsonView,
} from "../../shared/ui";
import { WorkerOverclock } from "../overclock";
import {
  MessagesFeed,
  MinerLogViewer,
  PerformanceCharts,
  WatchdogSettings,
  WorkerActions,
} from "./worker";
import { MachineEditor, BatchModal } from "./controls";
import { DeviceTable, MinerPanel, RecentChips, StatusLine, SystemTiles, WorkerBand } from "./worker-view";

// xterm is only downloaded when a terminal tab is opened.
const TerminalPane = lazy(() => import("../terminal").then((m) => ({ default: m.TerminalPane })));
const terminalFallback = <Spin style={{ display: "block", margin: 40 }} />;

export function MachineDetail() {
  const { id } = useParams();
  const [params, setParams] = useSearchParams();
  const tab = params.get("tab") ?? "overview";
  const machines = useMachines();
  const telemetry = useTelemetry(id);
  const machine = machines.data?.find((m) => m.id === id);
  const system = latest(telemetry.data, id!, "system");
  const mining = latest(telemetry.data, id!, "mining");
  const sensors = latest(telemetry.data, id!, "sensors");
  const power = latest(telemetry.data, id!, "power");
  const software = latest(telemetry.data, id!, "software");
  const events = latest(telemetry.data, id!, "events");
  const d = data(system?.data);
  const powerReading = machinePower(system);
  const powerWatts = powerReading.watts;
  const instances: Data[] = data(mining?.data).instances ?? [];
  const [instance, setInstance] = useState<string>();
  const [editor, setEditor] = useState(false);
  const [logOpen, setLogOpen] = useState(false);
  const [batch, setBatch] = useState("");
  const [adopting, setAdopting] = useState<Data>();
  const [adoptForm] = Form.useForm();
  const [adoptBusy, setAdoptBusy] = useState(false);
  const { message } = App.useApp();
  if (!machine)
    return (
      <QueryState loading={machines.isLoading} error={machines.error}>
        <Alert type="info" message="未找到机器" />
      </QueryState>
    );
  const overview = (
    <>
      <DeviceTable machine={machine} system={system} instances={instances} />
      <SystemTiles system={system} />
      <Descriptions
        className="hardware-description"
        title="硬件与连接"
        column={{ xs: 1, sm: 1, md: 2 }}
        items={[
          { key: "cpu", label: "CPU", children: d.cpu_model ?? "待采集" },
          { key: "board", label: "主板", children: d.board ?? "未采集" },
          { key: "os", label: "操作系统", children: d.os ?? "未采集" },
          { key: "kernel", label: "内核", children: d.kernel ?? "未采集" },
          { key: "bios", label: "BIOS", children: d.bios ?? "未采集" },
          {
            key: "topology",
            label: "拓扑",
            children: d.topology
              ? `${new Set(d.topology.map((c: Data) => c.socket)).size} 路 / ${new Set(d.topology.map((c: Data) => `${c.socket}:${c.core}`)).size} 核 / ${d.logical_cpus} 线程`
              : "—",
          },
          { key: "memory", label: "物理内存", children: bytes(d.memory_total) },
          {
            key: "ip",
            label: "内网 IP",
            children: (d.addresses ?? []).map((a: Data) => a.address).join(", ") || "未采集",
          },
          {
            key: "ssh",
            label: "SSH",
            children: `${machine.username}@${machine.host}:${machine.port}`,
          },
          { key: "bmc", label: "BMC", children: machine.bmc?.url ?? "未配置" },
          {
            key: "tags",
            label: "分组 / 标签",
            children: (
              <Space>
                <Tag>{machine.group || "未分组"}</Tag>
                {machine.tags.map((t) => (
                  <Tag key={t}>{t}</Tag>
                ))}
              </Space>
            ),
          },
          {
            key: "updated",
            label: "最近采集",
            children: system?.observed_at
              ? new Date(system.observed_at).toLocaleString()
              : "尚无数据",
          },
        ]}
      />
    </>
  );
  const flight = (
    <>
      <Space className="detail-actions">
        <Button type="primary" onClick={() => setBatch("apply")}>
          应用飞行表
        </Button>
        <Button onClick={() => setBatch("miner")}>控制矿工</Button>
      </Space>
      <Table<Data>
        rowKey="instance"
        dataSource={instances}
        columns={[
          { title: "实例", dataIndex: "instance" },
          { title: "飞行表", dataIndex: "sheet_name", render: (name, i) => i.sheet_id ? <Link to={`/flight-sheets?edit=${i.sheet_id}`}>{name} · 编辑</Link> : name },
          {
            title: "目标状态",
            dataIndex: "desired",
            render: (v) => <Status value={v} />,
          },
          {
            title: "实际状态",
            key: "phase",
            render: (_, i) => (
              <Status
                value={
                  serverNow() / 1000 - (i.observed_at ?? 0) < 35
                    ? (i.phase === "running" ? "挖矿中" : i.phase)
                    : "数据过期"
                }
              />
            ),
          },
          {
            title: "算力 / 算法",
            key: "rate",
            render: (_, i) =>
              serverNow() / 1000 - (i.stats_observed_at ?? 0) < 35
                ? `${hashrate(i.stats?.hashrate_hs)} / ${i.stats?.algorithm ?? "未知"}`
                : "—",
          },
          {
            title: "份额",
            key: "shares",
            render: (_, i) =>
              `${i.stats?.accepted ?? "—"} / ${i.stats?.rejected ?? "—"}`,
          },
        ]}
        expandable={{ expandedRowRender: (i) => <JsonView value={i} /> }}
      />
    </>
  );
  return (
    <>
      <WorkerBand
        machine={machine}
        system={system}
        power={power}
        actions={
          <WorkerActions
            machine={machine}
            instances={instances}
            onBatch={setBatch}
            onOpen={(panel) =>
              panel === "log" ? setLogOpen(true) : panel === "settings" ? setEditor(true) : setParams({ tab: panel })
            }
          />
        }
      />
      <MinerPanel machine={machine} system={system} instances={instances} />
      <StatusLine machine={machine} system={system} instances={instances} />
      <RecentChips id={id!} />
      <Modal
        open={logOpen}
        onCancel={() => setLogOpen(false)}
        footer={null}
        width="min(1000px, 96vw)"
        title={`矿工日志 · ${machine.name}`}
        destroyOnHidden
        className="wk-log-modal"
      >
        <MinerLogViewer id={id!} instances={instances} live />
      </Modal>
      {(system?.error || telemetry.error) && (
        <Alert
          type="warning"
          showIcon
          message="采集不可用"
          description={system?.error ?? telemetry.error?.message}
        />
      )}
      <Tabs
        activeKey={tab}
        onChange={(key) => setParams({ tab: key })}
        destroyOnHidden
        items={[
          { key: "overview", label: "概述", children: overview },
          { key: "flight", label: "飞行表", children: flight },
          {
            key: "performance",
            label: "统计",
            children: <PerformanceCharts machine={machine} />,
          },
          { key: "messages", label: "活动", children: <MessagesFeed id={id!} /> },
          {
            key: "software",
            label: "运行软件",
            children: (
              <>
                <Alert
                  type="info"
                  message="未纳管矿工仅识别，不按进程名清理"
                  description="未知币种保留为未知。切换前需要明确现有程序的启停方式。"
                />
                <Table
                  rowKey="pid"
                  dataSource={data(software?.data).processes ?? []}
                  columns={[
                    { title: "PID", dataIndex: "pid" },
                    { title: "可执行文件", dataIndex: "executable" },
                    {
                      title: "托管实例",
                      dataIndex: "managed_instance",
                      render: (v) => v ?? "未纳管",
                    },
                    {
                      title: "币种",
                      dataIndex: "coin",
                      render: (v) => v ?? "未知",
                    },
                    {
                      title: "算法",
                      dataIndex: "algorithm",
                      render: (v) => v ?? "未知",
                    },
                    {
                      title: "管理",
                      key: "adopt",
                      render: (_, row: Data) =>
                        row.adoption_required && (
                          <Button
                            onClick={() => {
                              adoptForm.resetFields();
                              setAdopting(row);
                            }}
                          >
                            绑定启停方式
                          </Button>
                        ),
                    },
                  ]}
                />
              </>
            ),
          },
          {
            key: "overclock",
            label: "超频",
            children: <WorkerOverclock machine={machine} />,
          },
          {
            key: "sensors",
            label: "传感器",
            children: (
              <>
                {sensors?.error && (
                  <Alert type="warning" message={sensors.error} />
                )}
                <Space className="detail-actions">
                  <Button
                    disabled={!machine.bmc}
                    onClick={() => setBatch("power")}
                  >
                    BMC 电源操作
                  </Button>
                  <PowerState observation={power} />
                  <span title={powerReading.source}>{powerReading.label}：{powerWatts === undefined ? "未采集" : `${number(powerWatts, 0)} W`}</span>
                  <span className="muted">
                    传感器每 60 秒采集 ·{" "}
                    {sensors?.observed_at
                      ? new Date(sensors.observed_at).toLocaleString()
                      : "暂无数据"}
                  </span>
                </Space>
                <Table
                  rowKey="row_id"
                  dataSource={(data(sensors?.data).items ?? []).map(
                    (item: Data, index: number) => ({
                      ...item,
                      row_id: `${item.name ?? "sensor"}-${index}`,
                    }),
                  )}
                  columns={[
                    { title: "传感器", dataIndex: "name" },
                    {
                      title: "读数",
                      dataIndex: "reading",
                      render: (v) => v ?? "不支持 / 缺失",
                    },
                    { title: "单位", dataIndex: "unit" },
                    {
                      title: "健康",
                      dataIndex: "health",
                      render: (v) => v ?? "未知",
                    },
                  ]}
                />
                {data(sensors?.data).unsupported?.length > 0 && (
                  <Alert
                    type="info"
                    message={`未提供的能力：${data(sensors?.data).unsupported.join(", ")}`}
                  />
                )}
              </>
            ),
          },
          {
            key: "console",
            label: "矿工控制台",
            children: (
              <>
                <Select
                  placeholder="选择运行实例"
                  value={instance ?? instances[0]?.instance}
                  onChange={setInstance}
                  options={instances.map((i) => ({
                    value: i.instance,
                    label: i.instance,
                  }))}
                  style={{ minWidth: 220, marginBottom: 16 }}
                />
                {(instance ?? instances[0]?.instance) ? (
                  <Suspense fallback={terminalFallback}>
                    <TerminalPane
                      id={id!}
                      instance={instance ?? instances[0].instance}
                    />
                  </Suspense>
                ) : (
                  <Alert type="info" message="没有可连接的托管矿工实例" />
                )}
              </>
            ),
          },
          {
            key: "ssh",
            label: "SSH 终端",
            children: (
              <Suspense fallback={terminalFallback}>
                <TerminalPane id={id!} />
              </Suspense>
            ),
          },
          {
            key: "logs",
            label: "日志",
            children: (
              <>
                <h3>矿工日志</h3>
                <MinerLogViewer id={id!} instances={instances} />
                {machine.bmc && (
                  <details>
                    <summary>BMC 硬件事件</summary>
                    {events?.error && (
                      <Alert type="warning" message={events.error} />
                    )}
                    <JsonView value={data(events?.data).items ?? []} />
                  </details>
                )}
              </>
            ),
          },
          {
            key: "settings",
            label: "设定",
            children: (
              <>
                <Space>
                  <Button onClick={() => setEditor(true)}>
                    编辑连接与恢复策略
                  </Button>
                  <Button
                    onClick={async () => {
                      try {
                        const r = await unwrap(
                          client.POST("/api/v1/machines/{id}/test", {
                            params: { path: { id: id! } },
                          }),
                        );
                        message.info(JSON.stringify(r));
                      } catch (e) {
                        message.error((e as Error).message);
                      }
                    }}
                  >
                    测试 SSH 连接
                  </Button>
                </Space>
                <h3>看门狗与自动恢复</h3>
                <WatchdogSettings key={JSON.stringify(machine.policy)} machine={machine} />
              </>
            ),
          },
        ]}
      />
      <MachineEditor
        machine={machine}
        open={editor}
        onClose={() => setEditor(false)}
      />
      {batch && (
        <BatchModal ids={[id!]} kind={batch} onClose={() => setBatch("")} />
      )}
      <Modal
        open={!!adopting}
        title={`纳管已有程序 · PID ${adopting?.pid ?? ""}`}
        onCancel={() => setAdopting(undefined)}
        onOk={() => adoptForm.submit()}
        confirmLoading={adoptBusy}
        width={700}
        destroyOnHidden
      >
        <Alert
          type="info"
          message="绑定时只保存进程身份和控制命令。后续切换会执行停止命令，回退时执行启动和 PID 查询命令。"
        />
        <Form
          form={adoptForm}
          layout="vertical"
          onFinish={async (values) => {
            setAdoptBusy(true);
            try {
              await submitJob({
                machine_ids: [id!],
                action: {
                  kind: "adopt",
                  ...values,
                  pid: adopting!.pid,
                  start_identity: String(adopting!.start_identity),
                },
                concurrency: 1,
                canary: true,
                include_controller: false,
              });
              setAdopting(undefined);
              setParams({ tab: "messages" });
            } catch (e) {
              message.error((e as Error).message);
            } finally {
              setAdoptBusy(false);
            }
          }}
        >
          <Form.Item
            name="name"
            label="绑定名称"
            rules={[{ required: true, pattern: /^[a-zA-Z0-9_-]{1,40}$/ }]}
          >
            <Input />
          </Form.Item>
          <Form.Item
            name="stop_script"
            label="精确停止命令"
            rules={[{ required: true }]}
          >
            <Input.TextArea
              rows={3}
              placeholder="针对该服务或 PID 的停止命令；不要按矿工名称批量清理"
            />
          </Form.Item>
          <Form.Item
            name="start_script"
            label="启动命令"
            rules={[{ required: true }]}
          >
            <Input.TextArea rows={3} />
          </Form.Item>
          <Form.Item
            name="pid_script"
            label="查询启动后主进程 PID 的命令"
            rules={[{ required: true }]}
          >
            <Input.TextArea rows={2} placeholder="标准输出只能包含一个 PID" />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
