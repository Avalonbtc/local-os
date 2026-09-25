// HiveOS "Setup Miner Config": version, algorithm, wallet template, pool URLs, password and
// extra arguments. Built-in miners download their official Linux build from GitHub; Custom is a
// HiveOS custom-miner package by URL. Either way the rig downloads it itself.
import { useEffect } from "react";
import { Button, Form, Input, InputNumber, Modal, Select, Tooltip } from "antd";
import { InfoCircleFilled } from "@ant-design/icons";
import type { Adapter, Data } from "../../shared/api";

export const CUSTOM_VERSION = "__custom_url__";

export function inferMinerName(url: string): string {
  try {
    const filename = decodeURIComponent(new URL(url).pathname.split("/").pop() ?? "");
    return filename
      .replace(/\.(tar\.(gz|xz|bz2|zst)|tgz|txz|zip)$/i, "")
      .replace(/[-_]v?\d+(?:\.\d+)*(?:[-_.].*)?$/i, "");
  } catch {
    return "";
  }
}

const DEFAULT_ALGORITHM: Record<string, string> = { xmrig: "rx/0", srbminer: "randomx", "cpuminer-opt": "" };
const ALGORITHMS = ["rx/0", "randomx", "rx/wow", "cn/gpu", "ghostrider", "yespower", "argon2d", "sha256", "scrypt"];

/** Miner spec for a built-in adapter at its newest release (what selecting it in the sheet does). */
export function defaultMiner(adapter: Adapter): Data {
  const release = adapter.releases[0];
  return release
    ? { adapter: adapter.id, name: adapter.name, version: release.version, url: release.url, executable: release.executable, sha256: release.sha256 ?? undefined }
    : { adapter: adapter.id, name: adapter.name, version: "", url: "", executable: "" };
}
export function defaultConfig(adapterId: string): Data {
  return { algorithm: DEFAULT_ALGORITHM[adapterId] ?? "", wallet_template: "%WAL%.%WORKER_NAME%", password: "x" };
}

export function MinerDialog({
  open,
  adapter,
  miner,
  config,
  onApply,
  onCancel,
}: {
  open: boolean;
  /** Undefined means Custom (HiveOS custom miner package). */
  adapter?: Adapter;
  miner: Data;
  config: Data;
  onApply: (miner: Data, config: Data) => void;
  onCancel: () => void;
}) {
  const [form] = Form.useForm();
  const custom = !adapter;
  const releases = adapter?.releases ?? [];
  const version = Form.useWatch("version", form);
  const manualUrl = custom || !releases.length || version === CUSTOM_VERSION;
  const seed = JSON.stringify({ miner, config, id: adapter?.id });
  useEffect(() => {
    if (!open) return;
    const { miner: m, config: c } = JSON.parse(seed) as { miner: Data; config: Data };
    const known = releases.some((r) => r.version === m.version && r.url === m.url);
    form.resetFields();
    form.setFieldsValue({
      name: m.name,
      version: custom ? undefined : known ? m.version : releases.length ? CUSTOM_VERSION : undefined,
      custom_version: known ? undefined : m.version,
      url: m.url,
      previous_url: m.url,
      executable: m.executable,
      sha256: m.sha256,
      algorithm: c.algorithm || undefined,
      wallet_template: c.wallet_template ?? "%WAL%.%WORKER_NAME%",
      password: c.password ?? "x",
      urls_text: c.urls_text ?? (c.urls ?? []).join("\n"),
      extra_text: (c.extra_args ?? []).join(" "),
      user_config: c.user_config,
      threads: c.threads,
      cpus_text: c.cpus_text ?? (c.cpus ?? []).join(","),
    });
  }, [open, seed, form]); // eslint-disable-line react-hooks/exhaustive-deps
  const label = (text: string, help: string) => (
    <>
      {text}{" "}
      <Tooltip title={help}>
        <InfoCircleFilled className="custom-info" />
      </Tooltip>
    </>
  );
  const apply = (v: Data) => {
    const release = releases.find((r) => r.version === v.version);
    const nextMiner: Data = custom
      ? {
          adapter: "hive-custom",
          name: v.name.trim(),
          custom_name: v.name.trim(),
          url: v.url.trim(),
          version: v.custom_version?.trim() || "",
          sha256: v.sha256?.trim() || undefined,
          // Same package: keep what it declared (e.g. its own stats port for multi-instance).
          ...(miner.capabilities && miner.url === v.url.trim() ? { capabilities: miner.capabilities } : {}),
        }
      : release && !manualUrl
        ? { adapter: adapter!.id, name: adapter!.name, version: release.version, url: release.url, executable: release.executable, sha256: release.sha256 ?? undefined }
        : { adapter: adapter!.id, name: adapter!.name, version: v.custom_version?.trim() || "custom", url: v.url.trim(), executable: v.executable.trim(), sha256: v.sha256?.trim() || undefined };
    const nextConfig: Data = {
      ...config,
      algorithm: v.algorithm ?? "",
      wallet_template: v.wallet_template ?? "",
      password: v.password ?? "",
      urls_text: v.urls_text ?? "",
    };
    if (custom) nextConfig.user_config = v.user_config ?? "";
    else {
      nextConfig.extra_text = v.extra_text ?? "";
      nextConfig.threads = v.threads ?? undefined;
      nextConfig.cpus_text = v.cpus_text ?? "";
    }
    onApply(nextMiner, nextConfig);
  };
  return (
    <Modal
      className="hive-custom-dialog"
      title={custom ? "Custom 配置" : `${adapter!.name} 配置`}
      open={open}
      onCancel={onCancel}
      width={682}
      style={{ top: 24 }}
      maskClosable={false}
      destroyOnHidden={false}
      forceRender
      footer={
        <div className="custom-footer">
          <Button autoInsertSpace={false} onClick={() => form.resetFields()}>
            清除
          </Button>
          <div>
            <Button type="text" onClick={onCancel}>
              取消
            </Button>
            <Button type="primary" onClick={() => form.submit()}>
              应用更改
            </Button>
          </div>
        </div>
      }
    >
      {custom && (
        <p className="custom-intro">
          HiveOS 自定义矿工包（含 h-manifest.conf）。矿机会直接从安装链接下载，
          参见 <a href="https://github.com/minershive/hiveos-linux/tree/master/hive/miners/custom" target="_blank" rel="noreferrer">说明</a>。
        </p>
      )}
      <Form form={form} layout="vertical" requiredMark={false} onFinish={apply}>
        {custom ? (
          <Form.Item name="name" label={label("挖矿软件名称", "根据安装链接自动识别，也可自行修改")} rules={[{ required: true, message: "请输入挖矿软件名称" }]}>
            <Input />
          </Form.Item>
        ) : releases.length > 0 ? (
          <Form.Item name="version" label="版本" rules={[{ required: true }]}>
            <Select
              options={[
                ...releases.map((r, i) => ({ value: r.version, label: i === 0 ? `${r.version}（最新）` : r.version })),
                { value: CUSTOM_VERSION, label: "其他版本 / 镜像地址…" },
              ]}
            />
          </Form.Item>
        ) : null}
        {manualUrl && (
          <>
            <Form.Item
              name="url"
              label={label("安装链接", "矿机直接下载这个地址；国内可填镜像地址")}
              rules={[{ required: true, message: "请输入安装链接" }, { pattern: /^https?:\/\/\S+$/, message: "需要 http(s) 地址" }]}
            >
              <Input
                placeholder="https://…/miner.tar.gz"
                onChange={(event) => {
                  if (!custom) return;
                  const name = form.getFieldValue("name");
                  if (!name || name === inferMinerName(form.getFieldValue("previous_url") ?? ""))
                    form.setFieldValue("name", inferMinerName(event.target.value));
                  form.setFieldValue("previous_url", event.target.value);
                }}
              />
            </Form.Item>
            <Form.Item name="previous_url" hidden>
              <Input />
            </Form.Item>
            {!custom && (
              <Form.Item name="executable" label={label("执行文件", "安装包内的相对路径（最外层目录会自动去掉）")} rules={[{ required: true, message: "例如 xmrig" }]}>
                <Input placeholder="xmrig" />
              </Form.Item>
            )}
            <Form.Item name="custom_version" label="版本（可选，仅显示用）">
              <Input />
            </Form.Item>
            <Form.Item
              name="sha256"
              label={label("SHA256（可选）", "填写后矿机下载完会校验；http 链接必须填写")}
              rules={[{ pattern: /^[a-fA-F0-9]{64}$/, message: "64 位十六进制" }]}
            >
              <Input className="mono" />
            </Form.Item>
          </>
        )}
        <Form.Item name="algorithm" label="加密算法" rules={custom ? [] : [{ required: true, message: "请选择算法" }]}>
          <Select
            showSearch
            allowClear
            placeholder="----"
            options={Array.from(new Set([form.getFieldValue("algorithm"), ...ALGORITHMS].filter(Boolean))).map((value) => ({ value, label: value }))}
            popupRender={(menu) => (
              <>
                {menu}
                <Input
                  className="custom-algorithm-input"
                  placeholder="输入其他算法，回车确认"
                  onKeyDown={(e) => {
                    // Keep Enter away from the Select, which would pick the highlighted option.
                    e.stopPropagation();
                    if (e.key === "Enter") {
                      e.preventDefault();
                      form.setFieldValue("algorithm", e.currentTarget.value.trim() || undefined);
                    }
                  }}
                />
              </>
            )}
          />
        </Form.Item>
        <Form.Item name="wallet_template" label={label("钱包与矿机模板", "支持 %WAL%、%WORKER_NAME%、%COIN% 等变量")}>
          <Input />
        </Form.Item>
        <Form.Item
          name="urls_text"
          className="custom-multiline"
          label={label("矿池地址", "每行一个，第一行为主矿池")}
          rules={custom ? [] : [{ required: true, message: "至少填写一个矿池地址" }]}
        >
          <Input.TextArea rows={2} placeholder="stratum+tcp://pool.example.com:3333" />
        </Form.Item>
        <Form.Item name="password" label="密码">
          <Input />
        </Form.Item>
        {custom ? (
          <Form.Item name="user_config" className="custom-multiline" label={label("其他配置参数", "传给 HiveOS 自定义包的配置参数")}>
            <Input.TextArea rows={2} />
          </Form.Item>
        ) : (
          <>
            <Form.Item name="extra_text" label={label("额外参数", "追加到命令行，以空格分隔")}>
              <Input placeholder="--cpu-priority 2" />
            </Form.Item>
            <div className="form-grid">
              <Form.Item name="threads" label="线程数（可选）">
                <InputNumber min={1} max={1024} style={{ width: "100%" }} />
              </Form.Item>
              <Form.Item name="cpus_text" label={label("绑定 CPU（可选）", "CPU 编号，用逗号分隔")}>
                <Input placeholder="0,1,2,3" />
              </Form.Item>
            </div>
          </>
        )}
        <p className="muted">支持 %WAL% · %WORKER_NAME% · %COIN% · %URL%</p>
      </Form>
    </Modal>
  );
}
