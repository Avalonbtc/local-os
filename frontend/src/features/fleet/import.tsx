// Batch onboarding: list or IP range → fetch every SSH fingerprint → confirm → create.
import { useMemo, useState } from "react";
import {
  Alert,
  App,
  Button,
  Checkbox,
  Form,
  Input,
  InputNumber,
  Modal,
  Radio,
  Space,
  Steps,
  Table,
  Tag,
} from "antd";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { client, unwrap, type MachineInput } from "../../shared/api";

export type BatchRow = {
  key: string;
  name: string;
  host: string;
  port: number;
  username: string;
  fingerprint?: string | null;
  algorithm?: string | null;
  probeError?: string | null;
  include: boolean;
  result?: string;
  machineId?: string | null;
};

const MAX = 256;
const ipv4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;

function octets(ip: string) {
  const m = ipv4.exec(ip.trim());
  if (!m) return undefined;
  const parts = m.slice(1).map(Number);
  return parts.every((p) => p <= 255) ? parts : undefined;
}
const toNumber = (p: number[]) => ((p[0] << 24) >>> 0) + (p[1] << 16) + (p[2] << 8) + p[3];
const toIp = (n: number) => [n >>> 24, (n >>> 16) & 255, (n >>> 8) & 255, n & 255].join(".");

/** Expand `10.0.0.5`, `10.0.0.5-40`, `10.0.0.5-10.0.1.20` or a `/24`-style CIDR. */
export function expandHosts(token: string): string[] {
  const text = token.trim();
  const cidr = /^(.+)\/(\d{1,2})$/.exec(text);
  if (cidr) {
    const base = octets(cidr[1]);
    const bits = Number(cidr[2]);
    if (!base || bits < 22 || bits > 32) throw new Error(`${text}：CIDR 只支持 /22 到 /32`);
    const size = 2 ** (32 - bits);
    const start = toNumber(base) & ~(size - 1);
    const hosts = [];
    for (let n = start; n < start + size; n++) {
      if (size > 2 && (n === start || n === start + size - 1)) continue;
      hosts.push(toIp(n >>> 0));
    }
    return hosts;
  }
  const range = /^([\d.]+)\s*-\s*([\d.]+)$/.exec(text);
  if (range) {
    const first = octets(range[1]);
    if (!first) throw new Error(`${text}：起始地址无效`);
    const endParts = range[2].includes(".") ? octets(range[2]) : [...first.slice(0, 3), Number(range[2])];
    if (!endParts || endParts[3] > 255) throw new Error(`${text}：结束地址无效`);
    const [a, b] = [toNumber(first), toNumber(endParts)];
    if (b < a) throw new Error(`${text}：结束地址小于起始地址`);
    if (b - a >= MAX) throw new Error(`${text}：一次最多 ${MAX} 台`);
    return Array.from({ length: b - a + 1 }, (_, i) => toIp(a + i));
  }
  if (!/^[A-Za-z0-9.:-]+$/.test(text)) throw new Error(`${text}：地址无效`);
  return [text];
}

/**
 * One line per entry:
 *   192.168.1.10-60                 range (name from template)
 *   192.168.1.0/24                  CIDR
 *   rig-07,192.168.1.17[,port[,user]]  explicit name
 */
export function parseTargets(
  text: string,
  template: string,
  defaults: { port: number; username: string },
): { name: string; host: string; port: number; username: string }[] {
  const rows: { name: string; host: string; port: number; username: string }[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/#.*/, "").trim();
    if (!line) continue;
    const fields = line.split(/[,\t;]/).map((f) => f.trim());
    if (fields.length >= 2) {
      const [name, host, port, username] = fields;
      rows.push({
        name,
        host: expandHosts(host)[0],
        port: port ? Number(port) : defaults.port,
        username: username || defaults.username,
      });
      continue;
    }
    for (const host of expandHosts(fields[0])) {
      const parts = host.split(".");
      const index = rows.length + 1;
      const name = template
        .replaceAll("{ip}", host.replaceAll(".", "-"))
        .replaceAll("{last}", parts[parts.length - 1] ?? host)
        .replaceAll("{last2}", parts.slice(-2).join("-"))
        .replace(/\{n(?::(\d+))?\}/g, (_, width) => String(index).padStart(Number(width ?? 0), "0"));
      rows.push({ name, host, port: defaults.port, username: defaults.username });
    }
    if (rows.length > MAX) throw new Error(`一次最多 ${MAX} 台`);
  }
  for (const row of rows)
    if (!Number.isInteger(row.port) || row.port < 1 || row.port > 65535)
      throw new Error(`${row.name}：端口无效`);
  return rows;
}

type Setup = {
  targets: string;
  template: string;
  port: number;
  username: string;
  group?: string;
  tags?: string;
  auth: "password" | "private_key";
  secret?: string;
  passphrase?: string;
  sudo_password?: string;
  bootstrap: boolean;
};

export function BatchAddModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<Setup>();
  const [step, setStep] = useState(0);
  const [rows, setRows] = useState<BatchRow[]>([]);
  const [busy, setBusy] = useState(false);
  const [jobId, setJobId] = useState<string | null>();
  const [keyName, setKeyName] = useState("");
  const auth = Form.useWatch("auth", form);
  const reset = () => {
    setStep(0);
    setRows([]);
    setJobId(undefined);
    setKeyName("");
    form.resetFields();
  };
  const included = rows.filter((r) => r.include && r.fingerprint);
  const summary = useMemo(
    () => ({
      ok: rows.filter((r) => r.fingerprint).length,
      failed: rows.filter((r) => r.probeError).length,
    }),
    [rows],
  );

  const probe = async () => {
    const setup = await form.validateFields();
    let parsed;
    try {
      parsed = parseTargets(setup.targets, setup.template, { port: setup.port, username: setup.username });
    } catch (e) {
      message.error((e as Error).message);
      return;
    }
    if (!parsed.length) {
      message.warning("没有可添加的地址");
      return;
    }
    setBusy(true);
    try {
      const results = await unwrap(
        client.POST("/api/v1/ssh/host-keys", {
          body: { targets: parsed.map((r) => ({ host: r.host, port: r.port })) },
        }),
      );
      setRows(
        parsed.map((r, i) => ({
          ...r,
          key: `${i}-${r.host}:${r.port}`,
          fingerprint: results[i]?.fingerprint,
          algorithm: results[i]?.algorithm,
          probeError: results[i]?.error,
          include: !!results[i]?.fingerprint,
        })),
      );
      setStep(1);
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const create = async () => {
    const setup = form.getFieldsValue(true) as Setup;
    const tags = (setup.tags ?? "").split(",").map((t) => t.trim()).filter(Boolean);
    const credential =
      setup.auth === "password"
        ? { kind: "password" as const, password: setup.secret!, sudo_password: null }
        : { kind: "private_key" as const, private_key: setup.secret!, passphrase: setup.passphrase || null, sudo_password: null };
    const machines: MachineInput[] = included.map((r) => ({
      name: r.name,
      host: r.host,
      port: r.port,
      username: r.username,
      host_key: r.fingerprint!,
      group: setup.group ?? "",
      tags,
      is_controller: false,
      credential,
      sudo_password: setup.sudo_password || null,
      bmc: null,
      bmc_credential: null,
      policy: {},
    }));
    setBusy(true);
    try {
      const result = await unwrap(
        client.POST("/api/v1/machines/batch", { body: { machines, bootstrap: setup.bootstrap } }),
      );
      // Results come back in request order: pair them with the submitted rows by position.
      const outcome = new Map(included.map((row, i) => [row.key, result.results[i]]));
      setRows((current) =>
        current.map((row) => {
          const r = outcome.get(row.key);
          return r ? { ...row, result: r.error ?? "已添加", machineId: r.machine_id } : row;
        }),
      );
      setJobId(result.bootstrap_job_id);
      if (result.bootstrap_error)
        message.warning(`机器已添加，但部署运行层没有提交：${result.bootstrap_error}。可在矿机列表全选后重新部署。`, 8);
      const ok = result.results.filter((r) => !r.error).length;
      await queryClient.invalidateQueries({ queryKey: ["machines"] });
      message.success(`已添加 ${ok} 台，失败 ${result.results.length - ok} 台`);
      setStep(2);
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const columns = [
    { title: "名称", dataIndex: "name", width: 170 },
    { title: "地址", key: "addr", width: 170, render: (_: unknown, r: BatchRow) => `${r.username}@${r.host}:${r.port}` },
    {
      title: "SSH 主机指纹",
      key: "fp",
      render: (_: unknown, r: BatchRow) =>
        r.fingerprint ? (
          <code className="fingerprint">
            {r.algorithm} {r.fingerprint}
          </code>
        ) : (
          <span className="text-error">{r.probeError ?? "—"}</span>
        ),
    },
    ...(step === 2
      ? [
          {
            title: "结果",
            key: "result",
            width: 200,
            render: (_: unknown, r: BatchRow) =>
              r.machineId ? (
                <Link to={`/machines/${r.machineId}`}>已添加</Link>
              ) : r.result ? (
                <span className="text-error">{r.result}</span>
              ) : (
                <Tag>未添加</Tag>
              ),
          },
        ]
      : []),
  ];

  return (
    <Modal
      open={open}
      width={980}
      title="批量添加矿机"
      onCancel={() => {
        onClose();
        reset();
      }}
      destroyOnHidden
      footer={
        step === 0 ? (
          <Button type="primary" loading={busy} onClick={probe}>
            获取指纹
          </Button>
        ) : step === 1 ? (
          <Space>
            <Button onClick={() => setStep(0)}>返回修改</Button>
            <Button type="primary" loading={busy} disabled={!included.length} onClick={create}>
              添加 {included.length} 台
            </Button>
          </Space>
        ) : (
          <Space>
            {jobId && <span className="muted">运行层正在部署，结果会显示在每台矿机的「消息」里</span>}
            <Button type="primary" onClick={() => { onClose(); reset(); }}>
              完成
            </Button>
          </Space>
        )
      }
    >
      <Steps
        size="small"
        current={step}
        className="batch-steps"
        items={[{ title: "地址与凭据" }, { title: "核对指纹" }, { title: "结果" }]}
      />
      <Form<Setup>
        form={form}
        layout="vertical"
        hidden={step !== 0}
        initialValues={{ template: "rig-{last}", port: 22, username: "root", auth: "password", bootstrap: true }}
      >
        <Form.Item
          name="targets"
          label="地址（每行一项）"
          rules={[{ required: true, message: "请填写地址" }]}
          extra="支持 192.168.1.10、192.168.1.10-60、192.168.1.10-192.168.2.20、192.168.1.0/24，或「名称,地址[,端口[,用户]]」。# 后为注释。最多 256 台。"
        >
          <Input.TextArea rows={6} placeholder={"192.168.1.10-60\nepyc-a01,10.0.3.21,22,miner"} />
        </Form.Item>
        <div className="form-grid">
          <Form.Item
            name="template"
            label="名称模板"
            extra="{last} 末段 · {last2} 末两段 · {ip} 完整地址 · {n} 或 {n:3} 序号"
            rules={[{ required: true }]}
          >
            <Input />
          </Form.Item>
          <Form.Item name="group" label="分组">
            <Input placeholder="例如 A 排" />
          </Form.Item>
          <Form.Item name="port" label="默认 SSH 端口" rules={[{ required: true }]}>
            <InputNumber min={1} max={65535} />
          </Form.Item>
          <Form.Item name="username" label="默认用户" rules={[{ required: true, pattern: /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$/ }]}>
            <Input />
          </Form.Item>
          <Form.Item name="tags" label="标签（逗号分隔）">
            <Input />
          </Form.Item>
        </div>
        <Form.Item name="auth" label="共用凭据">
          <Radio.Group
            options={[
              { value: "password", label: "密码" },
              { value: "private_key", label: "私钥" },
            ]}
            onChange={() => {
              setKeyName("");
              form.setFieldsValue({ secret: undefined, passphrase: undefined });
            }}
          />
        </Form.Item>
        {auth === "private_key" ? (
          <>
            <Form.Item name="secret" rules={[{ required: true, message: "请选择私钥文件" }]} hidden>
              <Input />
            </Form.Item>
            <Form.Item label="私钥文件" required extra="OpenSSH / PEM 格式；不要选择 .pub 公钥">
              <input
                type="file"
                onChange={async (event) => {
                  const file = event.target.files?.[0];
                  if (!file) return;
                  if (file.size > 64 * 1024) {
                    message.error("私钥文件过大");
                    return;
                  }
                  form.setFieldsValue({ secret: await file.text() });
                  setKeyName(file.name);
                }}
              />
              {keyName && <span className="muted"> 已读取 {keyName}</span>}
            </Form.Item>
            <Form.Item name="passphrase" label="私钥口令（如有）">
              <Input.Password autoComplete="off" />
            </Form.Item>
          </>
        ) : (
          <Form.Item name="secret" label="SSH 密码" rules={[{ required: true, message: "请填写密码" }]}>
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        )}
        <Form.Item name="sudo_password" label="sudo 密码（非 root 且与登录密码不同时填写）">
          <Input.Password autoComplete="new-password" />
        </Form.Item>
        <Form.Item name="bootstrap" valuePropName="checked">
          <Checkbox>添加后自动部署运行层（先部署一台验证，再并发 8 台）</Checkbox>
        </Form.Item>
      </Form>
      {step > 0 && (
        <>
          {step === 1 && (
            <Alert
              showIcon
              type={summary.failed ? "warning" : "info"}
              message={`${summary.ok} 台已取得指纹，${summary.failed} 台无法连接`}
              description="指纹是第一次连接时读取的。机房网络可信时可直接确认；否则请与机器控制台上 ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub 的输出核对。无法连接的机器不会被添加。"
            />
          )}
          <Table<BatchRow>
            className="batch-table"
            rowKey="key"
            size="small"
            pagination={rows.length > 50 ? { pageSize: 50 } : false}
            dataSource={rows}
            columns={columns}
            rowSelection={
              step === 1
                ? {
                    selectedRowKeys: rows.filter((r) => r.include).map((r) => r.key),
                    getCheckboxProps: (r) => ({ disabled: !r.fingerprint }),
                    onChange: (keys) =>
                      setRows((current) => current.map((r) => ({ ...r, include: keys.includes(r.key) }))),
                  }
                : undefined
            }
          />
        </>
      )}
    </Modal>
  );
}
