import { useRef, useState } from "react";
import {
  Alert,
  App,
  AutoComplete,
  Button,
  Checkbox,
  Form,
  Input,
  InputNumber,
  Modal,
  Select,
  Space,
  Switch,
  Upload,
} from "antd";
import { useNavigate } from "react-router-dom";
import {
  client,
  unwrap,
  useFlights,
  useMachines,
  useTelemetry,
  useRefresh,
  latest,
  data,
  submitJob,
  type Machine,
  type MachineInput,
  type JobInput,
} from "../../shared/api";
import { PowerState } from "../../shared/ui";

export function MachineEditor({
  machine,
  open,
  onClose,
  initialBmcEnabled = false,
}: {
  machine?: Machine;
  open: boolean;
  onClose: () => void;
  initialBmcEnabled?: boolean;
}) {
  const [form] = Form.useForm();
  const [busy, setBusy] = useState(false);
  const [probing, setProbing] = useState(false);
  const [keyFileName, setKeyFileName] = useState("");
  const keyReadVersion = useRef(0);
  const { message, modal } = App.useApp();
  const refresh = useRefresh();
  const auth = Form.useWatch("auth", form) ?? "password";
  const bmcEnabled = Form.useWatch("bmc_enabled", form);
  const bmcProvider = Form.useWatch("bmc_provider", form) ?? "redfish";
  const probeHostKey = async (): Promise<string | null> => {
    const version = keyReadVersion.current;
    const values = await form.validateFields(["host", "port"]);
    const host = values.host.trim();
    const port = values.port;
    setProbing(true);
    try {
      const result = await unwrap(client.POST("/api/v1/ssh/host-key", { body: { host, port } }));
      const unchanged = () => version === keyReadVersion.current && form.getFieldValue("host")?.trim() === host && form.getFieldValue("port") === port;
      if (!unchanged()) throw new Error("SSH 地址已修改，请重新获取指纹");
      const accepted = await new Promise<boolean>((resolve) => {
        modal.confirm({
          title: "确认服务器身份",
          content: <div>
            <p>主控已连接 {host}:{port}，获取到以下 {result.algorithm} 主机指纹：</p>
            <code style={{ overflowWrap: "anywhere" }}>{result.fingerprint}</code>
            <p>请确认这是要管理的服务器。保存后指纹变化会拒绝连接。</p>
            {form.getFieldValue("host_key") && form.getFieldValue("host_key") !== result.fingerprint &&
              <Alert type="warning" message="与当前填写的指纹不同，请核实是否重装系统或更换了服务器。" />}
          </div>,
          okText: "信任此服务器",
          cancelText: "取消",
          onOk: () => resolve(true),
          onCancel: () => resolve(false),
        });
      });
      if (!accepted) return null;
      if (!unchanged()) throw new Error("SSH 地址已修改，请重新获取指纹");
      form.setFieldValue("host_key", result.fingerprint);
      return result.fingerprint;
    } finally {
      setProbing(false);
    }
  };
  return (
    <Modal
      width={760}
      title={machine ? "编辑机器" : initialBmcEnabled ? "添加矿机并配置 BMC" : "添加矿机"}
      open={open}
      onCancel={() => { keyReadVersion.current++; onClose(); }}
      onOk={() => form.submit()}
      confirmLoading={busy}
      okButtonProps={{ disabled: probing }}
      destroyOnHidden
      styles={{ body: { maxHeight: "calc(100vh - 220px)", overflowY: "auto" } }}
      afterOpenChange={(visible) => {
        if (visible) {
          keyReadVersion.current++;
          setKeyFileName("");
          form.resetFields();
          form.setFieldsValue({
            ...machine,
            auth: "password",
            port: machine?.port ?? 22,
            username: machine?.username ?? "root",
            tags_text: machine?.tags.join(", "),
            bmc_enabled: initialBmcEnabled || !!machine?.bmc,
            bmc_url: machine?.bmc?.url,
            bmc_user: machine?.bmc?.username,
            bmc_provider: machine?.bmc?.provider ?? "redfish",
            ca_pem: machine?.bmc?.ca_pem,
            gpu_temp_limit_c: typeof data(machine?.policy).gpu_temp_limit_c === "number"
              ? data(machine?.policy).gpu_temp_limit_c
              : 85,
            policy: JSON.stringify(
              machine?.policy ?? {
                max_restarts: 5,
                cooldown_seconds: 30,
                failure_seconds: 120,
                allow_host_reboot: false,
              },
              null,
              2,
            ),
          });
        }
      }}
    >
      <Form
        form={form}
        layout="vertical"
        onValuesChange={(changed) => {
          if ("host" in changed || "port" in changed) form.setFieldValue("host_key", "");
          if ("auth" in changed) {
            keyReadVersion.current++;
            setKeyFileName("");
            form.setFieldsValue({ secret: undefined, passphrase: undefined });
          }
        }}
        onFinish={async (v) => {
          setBusy(true);
          try {
            const hostKey = v.host_key?.trim() || await probeHostKey();
            if (!hostKey) return;
            const input: MachineInput = {
              name: v.name,
              host: v.host.trim(),
              port: v.port,
              username: v.username,
              host_key: hostKey,
              group: v.group ?? "",
              tags: (v.tags_text ?? "")
                .split(",")
                .map((s: string) => s.trim())
                .filter(Boolean),
              is_controller: !!v.is_controller,
              credential: v.secret
                ? v.auth === "password"
                  ? { kind: "password", password: v.secret, sudo_password: null }
                  : {
                      kind: "private_key",
                      private_key: v.secret,
                      passphrase: v.passphrase || null,
                      sudo_password: null,
                    }
                : null,
              sudo_password: v.sudo_password || null,
              bmc: v.bmc_enabled
                ? {
                    url: v.bmc_url,
                    username: v.bmc_user,
                    provider: v.bmc_provider,
                    ca_pem: v.ca_pem || null,
                  }
                : null,
              bmc_credential: v.bmc_password
                ? { kind: "password", password: v.bmc_password, sudo_password: null }
                : null,
              policy: { ...JSON.parse(v.policy || "{}"), gpu_temp_limit_c: v.gpu_temp_limit_c },
            };
            await unwrap(
              machine
                ? client.PUT("/api/v1/machines/{id}", {
                    params: { path: { id: machine.id } },
                    body: input,
                  })
                : client.POST("/api/v1/machines", { body: input }),
            );
            await refresh();
            message.success("机器已保存");
            onClose();
          } catch (e) {
            message.error((e as Error).message);
          } finally {
            setBusy(false);
          }
        }}
      >
        <div className="form-grid">
          <Form.Item name="name" label="机器名称" rules={[{ required: true }]}>
            <Input placeholder="epyc-01" />
          </Form.Item>
          <Form.Item name="group" label="分组">
            <AutoComplete options={[{ value: "EPYC 双路" }]} />
          </Form.Item>
          <Form.Item name="host" label="SSH 地址" rules={[{ required: true }]}>
            <Input placeholder="192.168.1.101" />
          </Form.Item>
          <Form.Item name="port" label="SSH 端口" rules={[{ required: true }]}>
            <InputNumber min={1} max={65535} />
          </Form.Item>
          <Form.Item
            name="username"
            label="SSH 用户"
            rules={[{ required: true }]}
          >
            <Input />
          </Form.Item>
          <Form.Item name="auth" label="认证方式">
            <Select
              options={[
                { value: "password", label: "密码" },
                { value: "private_key", label: "私钥" },
              ]}
            />
          </Form.Item>
        </div>
        <Form.Item
          name="host_key"
          label="SSH 主机指纹（首次自动获取）"
          extra="可以留空，保存时自动获取并让你确认。已知指纹也可直接填写。"
        >
          <Input placeholder="留空自动获取，或填写 SHA256:…" />
        </Form.Item>
        <Button loading={probing} disabled={busy} onClick={() => probeHostKey().catch((e) => {
          if (e instanceof Error) message.error(e.message);
        })} style={{ marginBottom: 20 }}>获取并确认指纹</Button>
        {auth === "private_key" && (
          <div style={{ marginBottom: 16 }}>
            <Space wrap>
              <Upload showUploadList={false} beforeUpload={async (file) => {
                const version = ++keyReadVersion.current;
                try {
                  if (file.size > 65536) throw new Error("私钥文件不能超过 64 KB");
                  const content = await file.text();
                  if (!/^\s*-----BEGIN (?:OPENSSH |RSA |EC |DSA |ENCRYPTED )?PRIVATE KEY-----/.test(content))
                    throw new Error("请选择 OpenSSH / PEM 私钥文件，不是 .pub 公钥。PPK 请先导出为 OpenSSH 格式。");
                  if (version !== keyReadVersion.current) return false;
                  form.setFieldValue("secret", content);
                  setKeyFileName(file.name);
                  await form.validateFields(["secret"]);
                } catch (e) {
                  if (version === keyReadVersion.current) message.error((e as Error).message);
                }
                return false;
              }}>
                <Button>选择私钥文件</Button>
              </Upload>
              {keyFileName && <><span>已选择：{keyFileName}</span><Button type="text" onClick={() => {
                keyReadVersion.current++;
                setKeyFileName("");
                form.setFieldValue("secret", undefined);
              }}>移除</Button></>}
            </Space>
            <p className="muted">选择 id_ed25519、id_rsa 或 PEM 私钥；保存时发送到你的主控并加密存储。</p>
          </div>
        )}
        <Form.Item
          name="secret"
          hidden={auth === "private_key" && !!keyFileName}
          label={
            machine
              ? "新凭据（留空保留现有）"
              : auth === "password"
                ? "SSH 密码"
                : "SSH 私钥"
          }
          rules={[{ required: !machine }]}
        >
          {auth === "password" ? (
            <Input.Password autoComplete="new-password" />
          ) : (
            <Input.TextArea rows={4} placeholder="选择私钥文件，或在此粘贴私钥内容" />
          )}
        </Form.Item>
        {auth === "private_key" && (
          <Form.Item name="passphrase" label="私钥口令（密钥已加密时填写）">
            <Input.Password />
          </Form.Item>
        )}
        <Form.Item name="sudo_password" label="sudo 密码（可选）"
          extra="SSH 用户不是 root 且 sudo 要求密码时填写；密码登录通常可留空。编辑机器时留空会保留原设置。">
          <Input.Password autoComplete="new-password" />
        </Form.Item>
        <Form.Item name="tags_text" label="标签（英文逗号分隔）">
          <Input />
        </Form.Item>
        <Space size="large">
          <Form.Item name="is_controller" valuePropName="checked">
            <Checkbox>这是主控机器</Checkbox>
          </Form.Item>
          <Form.Item name="bmc_enabled" valuePropName="checked">
            <Checkbox>配置 BMC 带外管理</Checkbox>
          </Form.Item>
        </Space>
        {bmcEnabled && (
          <>
            <div className="form-grid">
              <Form.Item name="bmc_provider" label="协议">
                <Select
                  options={[
                    { value: "redfish", label: "Redfish" },
                    { value: "ipmi", label: "IPMI LANplus" },
                  ]}
                />
              </Form.Item>
              <Form.Item
                name="bmc_url"
                label="BMC 地址"
                rules={[{ required: true }]}
                extra={bmcProvider === "ipmi"
                  ? "IPMI LANplus 使用 BMC 主机 IP/域名（UDP 623）；也接受现有的 https:// 地址，并自动提取主机。自定义端口可填 ipmi://地址:端口。"
                  : "Redfish 请填写 BMC 的 HTTPS 地址。"}
              >
                <Input placeholder={bmcProvider === "ipmi" ? "192.168.1.201" : "https://192.168.1.201"} />
              </Form.Item>
              <Form.Item
                name="bmc_user"
                label="BMC 用户名"
                rules={[{ required: true }]}
              >
                <Input />
              </Form.Item>
              <Form.Item
                name="bmc_password"
                label={machine?.bmc ? "BMC 密码（留空保留现有）" : "BMC 密码"}
                rules={[{ required: !machine?.bmc }]}
              >
                <Input.Password />
              </Form.Item>
            </div>
            <Form.Item name="ca_pem" label="自签证书 CA（PEM）">
              <Input.TextArea rows={3} />
            </Form.Item>
          </>
        )}
        <Form.Item name="gpu_temp_limit_c" label="GPU 高温阈值（°C）"
          rules={[{ required: true, message: "请设置 GPU 高温阈值" }]}
          extra="仅用于矿机列表中标记高温 GPU；未采集到 GPU 时不显示温度条。">
          <InputNumber min={40} max={120} />
        </Form.Item>
        <Form.Item name="policy" label="恢复策略 JSON">
          <Input.TextArea className="code-input" rows={5} />
        </Form.Item>
      </Form>
    </Modal>
  );
}

export function BatchModal({
  ids,
  kind,
  onClose,
}: {
  ids: string[];
  kind: string;
  onClose: () => void;
}) {
  const [form] = Form.useForm();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const flights = useFlights();
  const machines = useMachines();
  const telemetry = useTelemetry();
  const selectedMachines = ids.map((id) => machines.data?.find((machine) => machine.id === id));
  const navigate = useNavigate();
  const { message } = App.useApp();
  const [requestKey] = useState(() => crypto.randomUUID());
  return (
    <Modal
      open
      title={`${kind === "power" ? "BMC 电源操作" : "批量操作"} · ${ids.length} 台机器`}
      onCancel={onClose}
      onOk={() => form.submit()}
      okText="创建任务"
      confirmLoading={busy}
      width={650}
    >
      <Form
        form={form}
        layout="vertical"
        initialValues={{
          concurrency: kind === "power" ? Math.min(ids.length, 8) : 4,
          canary: kind !== "power",
          timeout_seconds: 300,
          operation: kind === "power" ? "on" : "restart",
        }}
        onFinish={async (v) => {
          setBusy(true);
          setError("");
          try {
            let action: JobInput["action"];
            if (kind === "apply")
              action = { kind: "apply", sheet_id: v.sheet_id };
            else if (kind === "command")
              action = {
                kind: "command",
                script: v.script,
                timeout_seconds: v.timeout_seconds,
              };
            else if (kind === "miner")
              action = {
                kind: "miner",
                operation: v.operation,
                instances: v.instances
                  .split(",")
                  .map((s: string) => s.trim())
                  .filter(Boolean),
              };
            else if (kind === "power")
              action = { kind: "power", operation: v.operation };
            else action = { kind: "bootstrap" };
            await submitJob(
              {
                machine_ids: ids,
                action,
                concurrency: kind === "power" ? Math.min(ids.length, 8) : v.concurrency,
                canary: kind === "power" ? false : v.canary,
                include_controller: kind === "power"
                  ? selectedMachines.some((machine) => machine?.is_controller)
                  : false,
              },
              requestKey,
            );
            onClose();
            if (ids.length === 1) navigate(`/machines/${ids[0]}?tab=messages`);
            else message.success(`已下发到 ${ids.length} 台矿机，结果见各矿机的「消息」`);
          } catch (e) {
            setError((e as Error).message);
          } finally {
            setBusy(false);
          }
        }}
      >
        {error && <Alert type="error" message={error} showIcon />}
        {kind === "power" && (
          <div className="bmc-power-targets">
            <p>所选机器同时提交电源操作；若包含主控，主控最后执行。</p>
            {selectedMachines.map((machine, index) => (
              <div key={ids[index]} className="bmc-power-target">
                <span>{machine?.name ?? ids[index]}</span>
                <PowerState observation={latest(telemetry.data, ids[index], "power")} />
              </div>
            ))}
          </div>
        )}
        {kind === "bootstrap" && (
          <Alert
            type="info"
            message="通过 SSH 自动部署 screen、运行脚本和本地 watchdog"
            description="此操作不会启动矿工。目标 Ubuntu 用户需能使用 sudo；需要密码时可在机器设置中填写。"
          />
        )}
        {kind === "apply" && (
          <Form.Item
            name="sheet_id"
            label="选择飞行表"
            rules={[{ required: true }]}
          >
            <Select
              showSearch
              optionFilterProp="label"
              loading={flights.isLoading}
              options={flights.data?.map((f) => ({
                value: f.id,
                label: `${f.name} · v${f.version} · ${f.tasks.length} 个任务`,
              }))}
            />
          </Form.Item>
        )}
        {kind === "command" && (
          <>
            <Form.Item
              name="script"
              label="Shell 脚本"
              rules={[{ required: true }]}
            >
              <Input.TextArea
                autoSize={{ minRows: 8 }}
                className="code-input"
                placeholder="uname -a"
              />
            </Form.Item>
            <Form.Item name="timeout_seconds" label="超时（秒）">
              <InputNumber min={1} max={86400} />
            </Form.Item>
          </>
        )}
        {(kind === "miner" || kind === "power") && (
          <Form.Item name="operation" label="操作">
            <Select
              options={(kind === "miner"
                ? [
                    ["start", "启动"],
                    ["stop", "停止"],
                    ["restart", "重启矿工"],
                  ]
                : [
                    ["on", "开机"],
                    ["shutdown", "优雅关机"],
                    ["reboot", "优雅重启"],
                    ["force_off", "强制断电"],
                    ["force_restart", "强制重启"],
                  ]
              ).map(([value, label]) => ({ value, label }))}
            />
          </Form.Item>
        )}
        {kind === "miner" && (
          <Form.Item
            name="instances"
            label="实例名称（英文逗号分隔）"
            rules={[{ required: true }]}
          >
            <Input placeholder="cpu-1,cpu-2" />
          </Form.Item>
        )}
        {kind !== "power" && (
          <div className="form-grid">
            <Form.Item name="concurrency" label="并发机器数">
              <InputNumber min={1} max={8} />
            </Form.Item>
            <Form.Item name="canary" valuePropName="checked" label="分批执行">
              <Switch checkedChildren="先验证一台" unCheckedChildren="直接并行" />
            </Form.Item>
          </div>
        )}
      </Form>
    </Modal>
  );
}
