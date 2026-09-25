// HiveOS-style overclocking: per-worker editor (machine detail "超频" tab) and farm templates page.
import { useState } from "react";
import {
  Alert,
  App,
  Button,
  Collapse,
  Descriptions,
  Empty,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  client,
  data,
  submitJob,
  unwrap,
  useCatalog,
  useMachines,
  useTelemetry,
  latest,
  type CatalogItem,
  type Data,
  type Machine,
} from "../../shared/api";
import { PageHeader, QueryState } from "../../shared/ui";
import {
  FIELDS,
  configToText,
  emptyText,
  exportHiveConf,
  importHiveConf,
  summary,
  textToConfig,
  valueFor,
  type Field,
  type OcConfig,
  type OcText,
  type Vendor,
} from "./hiveos";

const KIND = "oc-profiles";
const VENDOR_NAME: Record<Vendor, string> = { nvidia: "NVIDIA", amd: "AMD" };

function FieldInput({ field, value, onChange }: { field: Field; value?: string; onChange: (v: string) => void }) {
  return (
    <Form.Item
      label={`${field.label}${field.unit ? ` (${field.unit})` : ""}`}
      tooltip={`${field.conf}：${field.help}`}
      style={{ marginBottom: 8, minWidth: 150, flex: "1 1 150px" }}
    >
      <Input
        aria-label={`${field.conf}`}
        placeholder="如 1200 或 1200 1300"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        allowClear
      />
    </Form.Item>
  );
}

/** HiveOS's OC form: one text box per parameter, space-separated per-card values. */
export function OcFields({ vendors, value, onChange }: { vendors: Vendor[]; value: OcText; onChange: (v: OcText) => void }) {
  const set = (vendor: Vendor, key: string, v: string) => onChange({ ...value, [vendor]: { ...value[vendor], [key]: v } });
  return (
    <Form layout="vertical">
      {vendors.map((vendor) => {
        const basic = FIELDS[vendor].filter((f) => !f.advanced);
        const advanced = FIELDS[vendor].filter((f) => f.advanced);
        return (
          <div key={vendor} style={{ marginBottom: 12 }}>
            <Typography.Title level={5}>{VENDOR_NAME[vendor]}</Typography.Title>
            <div style={{ display: "flex", flexWrap: "wrap", gap: "0 12px" }}>
              {basic.map((f) => (
                <FieldInput key={f.key} field={f} value={value[vendor][f.key]} onChange={(v) => set(vendor, f.key, v)} />
              ))}
              {vendor === "nvidia" && (
                <Form.Item label="开机延迟 (秒)" tooltip="RUNNING_DELAY：开机后等待多少秒再应用，最多 300" style={{ marginBottom: 8, minWidth: 150, flex: "1 1 150px" }}>
                  <Input aria-label="RUNNING_DELAY" value={value.running_delay} onChange={(e) => onChange({ ...value, running_delay: e.target.value })} />
                </Form.Item>
              )}
            </div>
            {advanced.length > 0 && (
              <Collapse
                size="small"
                ghost
                items={[{
                  key: "advanced",
                  label: "高级（PowerPlay 表参数，仅保存）",
                  children: (
                    <div style={{ display: "flex", flexWrap: "wrap", gap: "0 12px" }}>
                      {advanced.map((f) => (
                        <FieldInput key={f.key} field={f} value={value[vendor][f.key]} onChange={(v) => set(vendor, f.key, v)} />
                      ))}
                    </div>
                  ),
                }]}
              />
            )}
          </div>
        );
      })}
    </Form>
  );
}

function ImportButton({ onImport }: { onImport: (text: OcText) => void }) {
  const { message } = App.useApp();
  const [open, setOpen] = useState(false);
  const [conf, setConf] = useState("");
  return (
    <>
      <Button onClick={() => setOpen(true)}>导入 HiveOS 配置</Button>
      <Modal
        title="导入 HiveOS 超频配置"
        open={open}
        onCancel={() => setOpen(false)}
        okText="导入"
        onOk={() => {
          const { text, ignored } = importHiveConf(conf);
          onImport(text);
          setOpen(false);
          if (ignored.length) message.warning(`未导入：${ignored.join(", ")}`);
        }}
      >
        <Typography.Paragraph type="secondary">
          粘贴 HiveOS 矿机上 <code>/hive-config/nvidia-oc.conf</code> 或 <code>/hive-config/amd-oc.conf</code> 的内容（可同时粘贴两份）。
        </Typography.Paragraph>
        <Input.TextArea
          aria-label="HiveOS 配置"
          rows={10}
          value={conf}
          onChange={(e) => setConf(e.target.value)}
          placeholder={'CLOCK="1500"\nMEM="2400"\nPLIMIT="220"\nFAN="70"'}
          style={{ fontFamily: "monospace" }}
        />
      </Modal>
    </>
  );
}

function ExportButton({ text }: { text: OcText }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button onClick={() => setOpen(true)}>查看 HiveOS 格式</Button>
      <Modal title="HiveOS 格式" open={open} onCancel={() => setOpen(false)} footer={null}>
        {(["nvidia", "amd"] as Vendor[]).map((vendor) => (
          <div key={vendor}>
            <Typography.Text strong>{vendor}-oc.conf</Typography.Text>
            <Typography.Paragraph copyable={{ text: exportHiveConf(text, vendor) }}>
              <pre style={{ margin: 0 }}>{exportHiveConf(text, vendor)}</pre>
            </Typography.Paragraph>
          </div>
        ))}
      </Modal>
    </>
  );
}

function SaveTemplateButton({ config, disabled }: { config: () => OcConfig | undefined; disabled?: boolean }) {
  const { message } = App.useApp();
  const cache = useQueryClient();
  const [name, setName] = useState("");
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button disabled={disabled} onClick={() => setOpen(true)}>另存为模板</Button>
      <Modal
        title="另存为超频模板"
        open={open}
        onCancel={() => setOpen(false)}
        okButtonProps={{ disabled: !name.trim() }}
        onOk={async () => {
          const value = config();
          if (!value) return;
          try {
            await unwrap(client.POST("/api/v1/catalog/{kind}", {
              params: { path: { kind: KIND } },
              body: { name: name.trim(), data: value as unknown as Record<string, never> },
            }));
            await cache.invalidateQueries({ queryKey: ["catalog", KIND] });
            message.success("模板已保存");
            setOpen(false);
          } catch (error) {
            message.error((error as Error).message);
          }
        }}
      >
        <Input aria-label="模板名称" placeholder="如 3070Ti-ETC" value={name} onChange={(e) => setName(e.target.value)} />
      </Modal>
    </>
  );
}

function ResultTags({ result }: { result?: Data }) {
  if (!result) return <Typography.Text type="secondary">—</Typography.Text>;
  return (
    <Space size={[4, 4]} wrap>
      {(result.applied ?? []).map((item: string) => <Tag key={item} color="green">{item}</Tag>)}
      {(result.errors ?? []).map((item: string) => <Tooltip key={item} title={item}><Tag color="red" style={{ maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis" }}>{item}</Tag></Tooltip>)}
      {(result.skipped ?? []).map((item: string) => <Tooltip key={item} title={item}><Tag color="orange" style={{ maxWidth: 280, overflow: "hidden", textOverflow: "ellipsis" }}>{item}</Tag></Tooltip>)}
    </Space>
  );
}

const fmt = (value: unknown, unit = "") => (typeof value === "number" ? `${Math.round(value)}${unit && ` ${unit}`}` : "—");

/** Machine detail "超频" tab. */
export function WorkerOverclock({ machine }: { machine: Machine }) {
  const { message, modal } = App.useApp();
  const telemetry = useTelemetry(machine.id);
  const profiles = useCatalog(KIND);
  const system = data(latest(telemetry.data, machine.id, "system")?.data);
  const gpus: Data[] = Array.isArray(system.gpus) ? system.gpus : [];
  const saved = data(system.gpu_oc);
  const [text, setText] = useState<OcText | null>(null);
  const [profile, setProfile] = useState<string | undefined>(saved.profile_name ?? undefined);
  const [busy, setBusy] = useState(false);
  const current = text ?? configToText(saved.config as OcConfig | undefined);
  const byVendor: Record<Vendor, Data[]> = {
    nvidia: gpus.filter((g) => g.vendor === "NVIDIA").sort((a, b) => String(a.id).localeCompare(String(b.id))),
    amd: gpus.filter((g) => g.vendor === "AMD").sort((a, b) => String(a.id).localeCompare(String(b.id))),
  };
  const vendors = (["nvidia", "amd"] as Vendor[]).filter((v) => byVendor[v].length);
  const shownVendors: Vendor[] = vendors.length ? vendors : ["nvidia", "amd"];
  const results: Data[] = Array.isArray(saved.results) ? saved.results : [];

  const config = () => {
    try {
      const value = textToConfig(current);
      if (!value.nvidia && !value.amd) throw new Error("没有填写任何超频参数");
      return value;
    } catch (error) {
      message.error((error as Error).message);
      return undefined;
    }
  };
  async function run(action: Parameters<typeof submitJob>[0]["action"], done: string) {
    setBusy(true);
    try {
      await submitJob({ machine_ids: [machine.id], action, concurrency: 1, canary: false, include_controller: true });
      message.success(done);
    } catch (error) {
      message.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  function apply() {
    const value = config();
    if (!value) return;
    modal.confirm({
      title: `对 ${machine.name} 应用超频？`,
      content: <><div>{summary(value)}</div><div style={{ marginTop: 8 }}>超频立即生效并保存在矿机上，开机后自动重新应用。</div></>,
      okText: "应用",
      onOk: () => run({ kind: "gpu_oc", profile_name: profile ?? null, config: value }, "已提交，结果显示在本页和机器消息中"),
    });
  }

  const planned = (vendor: Vendor, index: number) =>
    FIELDS[vendor].filter((f) => !f.advanced && valueFor(current[vendor][f.key], index) !== undefined)
      .map((f) => `${f.label} ${valueFor(current[vendor][f.key], index)}`).join("，");

  return (
    <QueryState loading={telemetry.isLoading} error={telemetry.error}>
      <Space direction="vertical" style={{ width: "100%" }} size="middle">
        {!gpus.length && <Alert type="info" showIcon message="未采集到 NVIDIA / AMD 显卡" description="可以先编辑并保存参数；应用前请确认显卡驱动已加载。" />}
        <Descriptions size="small" column={{ xs: 1, sm: 3 }}>
          <Descriptions.Item label="当前超频">{saved.applied_at ? (saved.profile_name ?? "自定义") : "未应用（驱动默认）"}</Descriptions.Item>
          <Descriptions.Item label="应用时间">{saved.applied_at ? new Date(saved.applied_at * 1000).toLocaleString() : "—"}</Descriptions.Item>
          <Descriptions.Item label="驱动">{Object.entries(data(system.gpu_drivers)).map(([k, v]) => `${k} ${v}`).join("，") || "—"}</Descriptions.Item>
        </Descriptions>
        <Space wrap>
          <Select
            aria-label="超频模板"
            placeholder="选择模板"
            allowClear
            style={{ width: 220 }}
            value={profile}
            loading={profiles.isLoading}
            options={(profiles.data ?? []).map((p: CatalogItem) => ({ value: p.name, label: p.name }))}
            onChange={(name?: string) => {
              setProfile(name);
              const item = profiles.data?.find((p: CatalogItem) => p.name === name);
              if (item) setText(configToText(item.data as OcConfig));
            }}
          />
          <ImportButton onImport={(t) => { setText(t); setProfile(undefined); }} />
          <ExportButton text={current} />
          <SaveTemplateButton config={config} />
        </Space>
        <OcFields vendors={shownVendors} value={current} onChange={(t) => { setText(t); setProfile(undefined); }} />
        <Table
          rowKey={(g: Data) => String(g.id)}
          size="small"
          pagination={false}
          scroll={{ x: true }}
          dataSource={shownVendors.flatMap((v) => byVendor[v].map((g, index) => ({ ...g, _vendor: v, _index: index })))}
          locale={{ emptyText: <Empty description="没有显卡" /> }}
          columns={[
            { title: "#", render: (_, g: Data) => `${VENDOR_NAME[g._vendor as Vendor]} ${g._index}` },
            { title: "总线", dataIndex: "id" },
            { title: "型号", dataIndex: "model" },
            { title: "温度", render: (_, g: Data) => fmt(g.temperature_c, "°C") },
            { title: "核心/显存", render: (_, g: Data) => `${fmt(g.core_mhz)} / ${fmt(g.mem_mhz, "MHz")}` },
            { title: "功耗/功耗墙", render: (_, g: Data) => `${fmt(g.power_w)} / ${fmt(g.power_limit_w, "W")}` },
            { title: "风扇", render: (_, g: Data) => fmt(g.fan_pct, "%") },
            { title: "将应用", render: (_, g: Data) => planned(g._vendor, g._index) || "不修改" },
            { title: "上次结果", render: (_, g: Data) => <ResultTags result={results.find((r) => r.bus_id === g.id)} /> },
          ]}
        />
        <Space wrap>
          <Button type="primary" loading={busy} onClick={apply}>应用超频</Button>
          <Popconfirm
            title="恢复驱动默认频率、功耗墙和自动风扇？"
            onConfirm={() => run({ kind: "gpu_oc_reset" }, "已提交恢复默认")}
          >
            <Button danger loading={busy}>恢复默认</Button>
          </Popconfirm>
          <Button disabled={!text} onClick={() => { setText(null); setProfile(saved.profile_name ?? undefined); }}>放弃修改</Button>
        </Space>
        <Typography.Text type="secondary">
          每个参数按显卡总线顺序填写，空格分隔；只填一个数表示所有卡相同，留空表示不修改。数值范围只是防止输错，不是推荐值。
        </Typography.Text>
      </Space>
    </QueryState>
  );
}

function TemplateEditor({ item, onClose }: { item: CatalogItem | "new"; onClose: () => void }) {
  const { message } = App.useApp();
  const cache = useQueryClient();
  const existing = item === "new" ? undefined : item;
  const [name, setName] = useState(existing?.name ?? "");
  const [text, setText] = useState<OcText>(() => (existing ? configToText(existing.data as OcConfig) : emptyText()));
  return (
    <Modal
      title={existing ? `编辑模板：${existing.name}` : "新建超频模板"}
      open
      width={860}
      onCancel={onClose}
      okButtonProps={{ disabled: !name.trim() }}
      onOk={async () => {
        try {
          const config = textToConfig(text);
          const body = { name: name.trim(), data: config as unknown as Record<string, never>, expected_revision: existing?.revision ?? null };
          if (existing) {
            await unwrap(client.PUT("/api/v1/catalog/{kind}/{id}", { params: { path: { kind: KIND, id: existing.id } }, body }));
          } else {
            await unwrap(client.POST("/api/v1/catalog/{kind}", { params: { path: { kind: KIND } }, body }));
          }
          await cache.invalidateQueries({ queryKey: ["catalog", KIND] });
          message.success("模板已保存");
          onClose();
        } catch (error) {
          message.error((error as Error).message);
        }
      }}
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        <Input aria-label="模板名称" placeholder="模板名称" value={name} onChange={(e) => setName(e.target.value)} />
        <Space wrap>
          <ImportButton onImport={setText} />
          <ExportButton text={text} />
        </Space>
        <OcFields vendors={["nvidia", "amd"]} value={text} onChange={setText} />
      </Space>
    </Modal>
  );
}

function ApplyTemplate({ item, onClose }: { item: CatalogItem; onClose: () => void }) {
  const { message } = App.useApp();
  const machines = useMachines();
  const [targets, setTargets] = useState<string[]>([]);
  const [concurrency, setConcurrency] = useState(4);
  return (
    <Modal
      title={`应用模板：${item.name}`}
      open
      onCancel={onClose}
      okText="应用"
      okButtonProps={{ disabled: !targets.length }}
      onOk={async () => {
        try {
          await submitJob({
            machine_ids: targets,
            action: { kind: "gpu_oc", profile_name: item.name, config: item.data as OcConfig },
            concurrency,
            canary: true,
            include_controller: true,
          });
          message.success("已提交；先在第一台验证，成功后再并发执行");
          onClose();
        } catch (error) {
          message.error((error as Error).message);
        }
      }}
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        <Typography.Text>{summary(item.data as OcConfig)}</Typography.Text>
        <Select
          aria-label="目标机器"
          mode="multiple"
          placeholder="选择机器"
          style={{ width: "100%" }}
          optionFilterProp="label"
          value={targets}
          onChange={setTargets}
          options={(machines.data ?? []).map((m) => ({ value: m.id, label: m.name }))}
        />
        <Space>
          并发
          <InputNumber aria-label="并发" min={1} max={8} value={concurrency} onChange={(v) => setConcurrency(v ?? 4)} />
        </Space>
      </Space>
    </Modal>
  );
}

/** Farm-level "超频模板" page. */
export function OverclockPage() {
  const { message } = App.useApp();
  const cache = useQueryClient();
  const profiles = useCatalog(KIND);
  const [editing, setEditing] = useState<CatalogItem | "new" | null>(null);
  const [applying, setApplying] = useState<CatalogItem | null>(null);
  return (
    <>
      <PageHeader title="超频模板" description="HiveOS 同款参数：NVIDIA 核心/显存/功耗墙/风扇，AMD 频率/电压/状态/功耗墙/风扇/REF" />
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setEditing("new")}>新建模板</Button>
        <Typography.Text type="secondary">单台机器的超频在机器详情的“超频”页签修改。</Typography.Text>
      </Space>
      <QueryState loading={profiles.isLoading} error={profiles.error}>
        <Table
          rowKey="id"
          dataSource={profiles.data ?? []}
          pagination={false}
          scroll={{ x: true }}
          columns={[
            { title: "名称", dataIndex: "name" },
            { title: "参数", render: (_, item: CatalogItem) => summary(item.data as OcConfig) },
            {
              title: "操作",
              render: (_, item: CatalogItem) => (
                <Space wrap>
                  <Button size="small" type="primary" onClick={() => setApplying(item)}>应用到机器</Button>
                  <Button size="small" onClick={() => setEditing(item)}>编辑</Button>
                  <Popconfirm
                    title="删除这个模板？已应用到矿机的超频不受影响。"
                    onConfirm={async () => {
                      try {
                        await unwrap(client.DELETE("/api/v1/catalog/{kind}/{id}", { params: { path: { kind: KIND, id: item.id } } }));
                        await cache.invalidateQueries({ queryKey: ["catalog", KIND] });
                      } catch (error) {
                        message.error((error as Error).message);
                      }
                    }}
                  >
                    <Button size="small" danger>删除</Button>
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </QueryState>
      {editing && <TemplateEditor item={editing} onClose={() => setEditing(null)} />}
      {applying && <ApplyTemplate item={applying} onClose={() => setApplying(null)} />}
      <Alert
        style={{ marginTop: 16 }}
        type="info"
        showIcon
        message="矿机要求"
        description={<>NVIDIA 需要 R520 以上驱动（通过 NVML，无需 X 服务器）；AMD 需要内核参数 <code>amdgpu.ppfeaturemask=0xffffffff</code>。升级后请在机器上重新“部署运行层”。详见 <Link to="/machines">矿机</Link> 详情的超频页签。</>}
      />
    </>
  );
}
