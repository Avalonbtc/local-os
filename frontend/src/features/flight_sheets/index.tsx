// HiveOS flight sheets: the "add new flight sheet" panel (coin → wallet → pool → miner per task),
// then the sheet list with Applied / Not Applied filters, coin / wallet / miner chips and a row
// menu. Pools live in the miner config, as HiveOS's "configure in miner" option.
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { App, Button, Dropdown, Empty, Form, Input, Modal, Pagination, Popover, Select, Tag } from "antd";
import { DeleteOutlined } from "@ant-design/icons";
import { useSearchParams } from "react-router-dom";
import {
  client,
  data,
  unwrap,
  useAdapters,
  useFleetTelemetry,
  useFlights,
  useMachines,
  useRefresh,
  submitJob,
  type FlightSheet,
  type FlightInput,
  type Data,
} from "../../shared/api";
import { QueryState } from "../../shared/ui";
import { IconPickaxe } from "../../shared/icons";
import { WalletDialog, useWalletCoins } from "../wallets";
import { CoinBadge, CoinLabel } from "../../shared/coins";
import { MinerDialog, defaultConfig, defaultMiner } from "./miner-dialog";

function StoredValue(_props: { value?: unknown }) {
  return null;
}

const newTask = (n: number): Data => ({
  instance: `cpu-${n}`,
  config: {},
});
const lines = (text?: string) =>
  (text ?? "")
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
function poolLabel(config: Data) {
  const urls = (config.urls_text !== undefined ? lines(config.urls_text) : (config.urls ?? [])) as string[];
  if (!urls.length) return "已在挖矿软件内配置";
  return urls[0].replace(/^[a-z+]+:\/\//i, "") + (urls.length > 1 ? ` +${urls.length - 1}` : "");
}
/** Devices each miner drives in RigDeck: the built-ins run CPU only, Custom packages decide. */
const devicesOf = (adapter?: string) => (adapter === "hive-custom" || adapter === "custom" ? ["NVIDIA", "AMD", "CPU"] : ["CPU"]);
function DeviceTags({ adapter, short = false }: { adapter?: string; short?: boolean }) {
  return (
    <span className="fs-devices">
      {devicesOf(adapter).map((d) => (
        <em key={d} className={d.toLowerCase()}>
          {short ? d.slice(0, 1) : d}
        </em>
      ))}
    </span>
  );
}

/** A RigDeck flight sheet export: enough to rebuild the editor on this or another console. */
type SheetExport = {
  rigdeck_flight_sheet: 1;
  name: string;
  tasks: { instance: string; coin: string; wallet_id: string; wallet_name?: string; miner: Data; config: Data }[];
};

export function FlightsPage() {
  const flights = useFlights(),
    adapters = useAdapters(),
    machines = useMachines(),
    telemetry = useFleetTelemetry(),
    { wallets, coins, symbol, symbols } = useWalletCoins();
  const refresh = useRefresh(),
    { message, modal } = App.useApp();
  const [params, setParams] = useSearchParams();
  const [form] = Form.useForm();
  const tasks: Data[] = Form.useWatch("tasks", form) ?? [newTask(1)];
  const [edit, setEdit] = useState<FlightSheet>(),
    [busy, setBusy] = useState(false),
    [apply, setApply] = useState<FlightSheet>(),
    [targets, setTargets] = useState<string[]>([]);
  const [configIndex, setConfigIndex] = useState<number>();
  const [addingWallet, setAddingWallet] = useState<{ index: number; coin?: string }>(),
    [search, setSearch] = useState(""),
    [page, setPage] = useState(1);
  const [status, setStatus] = useState<"all" | "applied" | "unapplied">("all");
  const [chip, setChip] = useState<string>();
  const [pasting, setPasting] = useState(false);
  const [coinSearch, setCoinSearch] = useState<Record<number, string>>({});
  const [pasted, setPasted] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const builtins = (adapters.data ?? []).filter((a) => a.id !== "hive-custom");
  const adapterOf = (choice?: string) => builtins.find((a) => a.id === choice);
  const walletName = (id?: string) => wallets.data?.find((w) => w.id === id)?.name;
  // Sheets some rig currently runs (from the rigs' own reports).
  const applied = useMemo(
    () =>
      new Set(
        (telemetry.data ?? [])
          .filter((o) => o.kind === "mining")
          .flatMap((o) => (data(o.data).instances ?? []).map((i: Data) => i.sheet_id))
          .filter(Boolean),
      ),
    [telemetry.data],
  );
  const reset = () => {
    setEdit(undefined);
    form.resetFields();
  };
  const fillEditor = (name: string, sheetTasks: { instance: string; wallet_id?: string; coin?: string; miner: Data; config: Data }[]) =>
    form.setFieldsValue({
      name,
      tasks: sheetTasks.map((t) => {
        const miner = data(t.miner);
        const config = data(t.config);
        return {
          instance: t.instance,
          wallet_id: t.wallet_id,
          coin: t.coin ?? symbol(t.wallet_id),
          miner_choice: miner.adapter === "hive-custom" ? "custom" : miner.adapter,
          miner,
          config: {
            ...config,
            urls_text: (config.urls ?? []).join("\n"),
            cpus_text: (config.cpus ?? []).join(","),
            extra_text: (config.extra_args ?? []).join(" "),
          },
        };
      }),
    });
  const openEditor = (sheet: FlightSheet, copy = false) => {
    setEdit(copy ? undefined : sheet);
    fillEditor(copy ? `${sheet.name} 副本` : sheet.name, sheet.tasks as never);
    window.scrollTo({ top: 0, behavior: "smooth" });
  };
  useEffect(() => {
    const id = params.get("edit");
    const sheet = flights.data?.find((f) => f.id === id);
    if (sheet && wallets.data && coins.data) {
      openEditor(sheet);
      setParams({}, { replace: true });
    }
  }, [params, flights.data, wallets.data, coins.data]); // eslint-disable-line react-hooks/exhaustive-deps
  const setTask = (index: number, patch: Data) =>
    form.setFieldValue(["tasks", index], { ...form.getFieldValue(["tasks", index]), ...patch });

  const exportOf = (sheet: FlightSheet): SheetExport => ({
    rigdeck_flight_sheet: 1,
    name: sheet.name,
    tasks: sheet.tasks.map((t) => ({
      instance: t.instance,
      coin: symbol(t.wallet_id),
      wallet_id: t.wallet_id,
      wallet_name: walletName(t.wallet_id),
      miner: data(t.miner),
      config: data(t.config),
    })),
  });
  /** Load an export into the editor; wallets match by id, else by coin and wallet name. */
  const importSheet = (text: string) => {
    let parsed: SheetExport;
    try {
      parsed = JSON.parse(text);
      if (parsed.rigdeck_flight_sheet !== 1 || !Array.isArray(parsed.tasks)) throw new Error();
    } catch {
      message.error("不是 RigDeck 导出的飞行表");
      return;
    }
    const missing: string[] = [];
    setEdit(undefined);
    fillEditor(
      parsed.name ?? "",
      parsed.tasks.map((t) => {
        const wallet =
          wallets.data?.find((w) => w.id === t.wallet_id) ??
          wallets.data?.find((w) => symbol(w.id) === t.coin && w.name === t.wallet_name);
        if (!wallet) missing.push(`${t.coin}${t.wallet_name ? `（${t.wallet_name}）` : ""}`);
        return { ...t, wallet_id: wallet?.id, coin: t.coin };
      }),
    );
    if (missing.length) message.warning(`没有找到这些钱包，请重新选择：${missing.join("、")}`);
    else message.success("已载入，检查后点「创建飞行表」");
  };
  const copyToClipboard = async (sheet: FlightSheet) => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(exportOf(sheet), null, 2));
      message.success("已复制到剪贴板");
    } catch {
      message.error("浏览器不允许写入剪贴板，请改用「导出」");
    }
  };
  const download = (sheet: FlightSheet) => {
    const url = URL.createObjectURL(new Blob([JSON.stringify(exportOf(sheet), null, 2)], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${sheet.name}.flightsheet.json`;
    link.click();
    URL.revokeObjectURL(url);
  };

  const save = async (v: Data) => {
    setBusy(true);
    try {
      const body: FlightInput = {
        name: v.name,
        expected_version: edit?.version ?? null,
        tasks: v.tasks.map((t: Data, index: number) => {
          const miner = data(t.miner);
          if (!t.miner_choice || !miner.url)
            throw new Error(`第 ${index + 1} 个挖矿软件还没设定，请点「设定挖矿软件配置」`);
          const config: Data = { ...t.config };
          config.urls = lines(config.urls_text);
          if (config.cpus_text !== undefined)
            config.cpus = String(config.cpus_text).trim()
              ? String(config.cpus_text)
                  .split(",")
                  .map((s: string) => {
                    const n = Number(s.trim());
                    if (!s.trim() || !Number.isInteger(n) || n < 0)
                      throw new Error("CPU 编号应为非负整数，用逗号分隔");
                    return n;
                  })
              : [];
          if (config.extra_text !== undefined)
            config.extra_args = String(config.extra_text).split(/\s+/).filter(Boolean);
          delete config.urls_text;
          delete config.cpus_text;
          delete config.extra_text;
          if (config.threads == null) delete config.threads;
          if (miner.adapter !== "hive-custom" && !config.algorithm)
            throw new Error(`第 ${index + 1} 个挖矿软件缺少算法，请在配置里选择`);
          if (miner.adapter !== "hive-custom" && !config.urls.length)
            throw new Error(`第 ${index + 1} 个挖矿软件缺少矿池地址，请在「设定挖矿软件配置」里填写`);
          return { instance: t.instance, wallet_id: t.wallet_id, miner, config };
        }),
      };
      const saved = await unwrap(
        edit
          ? client.PUT("/api/v1/flight-sheets/{id}", {
              params: { path: { id: edit.id } },
              body,
            })
          : client.POST("/api/v1/flight-sheets", { body }),
      );
      await refresh();
      reset();
      message.success(saved.apply_job_id ? "飞行表已保存，正在更新关联矿机，结果见各矿机的「消息」" : "飞行表已保存");
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  // Chips: every coin, wallet and miner used by a sheet (HiveOS shows the same three kinds).
  const chips = useMemo(() => {
    const seen = new Map<string, { key: string; kind: "coin" | "wallet" | "miner"; label: string; adapter?: string }>();
    for (const f of flights.data ?? [])
      for (const t of f.tasks) {
        const coin = symbol(t.wallet_id);
        seen.set(`coin:${coin}`, { key: `coin:${coin}`, kind: "coin", label: coin });
        seen.set(`wallet:${t.wallet_id}`, { key: `wallet:${t.wallet_id}`, kind: "wallet", label: walletName(t.wallet_id) ?? "未知钱包" });
        const miner = String(data(t.miner).name ?? "未知矿工");
        seen.set(`miner:${miner}`, { key: `miner:${miner}`, kind: "miner", label: miner });
      }
    return [...seen.values()];
  }, [flights.data, wallets.data, coins.data]); // eslint-disable-line react-hooks/exhaustive-deps
  const matchesChip = (f: FlightSheet) => {
    if (!chip) return true;
    const [kind, value] = [chip.slice(0, chip.indexOf(":")), chip.slice(chip.indexOf(":") + 1)];
    return f.tasks.some((t) =>
      kind === "coin" ? symbol(t.wallet_id) === value : kind === "wallet" ? t.wallet_id === value : String(data(t.miner).name ?? "未知矿工") === value,
    );
  };
  const filtered =
    flights.data?.filter(
      (f) =>
        (status === "all" || (status === "applied") === applied.has(f.id)) &&
        matchesChip(f) &&
        `${f.name} ${f.tasks.map((t) => `${symbol(t.wallet_id)} ${data(t.miner).name ?? ""}`).join(" ")}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    ) ?? [];
  const configTask = configIndex !== undefined ? tasks[configIndex] : undefined;
  const label = (text: string, extra?: ReactNode) => (
    <span className="fs-card-label">
      <span>{text}</span>
      {extra}
    </span>
  );
  return (
    <div className="flights-page">
      <section className="flight-composer">
        <div className="composer-heading">
          <h1>{edit ? `编辑飞行表 · ${edit.name}` : "添加新的飞行表"}</h1>
          {edit && <Tag>v{edit.version}</Tag>}
          <Popover
            placement="bottomRight"
            content={
              <div className="fs-help">
                <p>飞行表可以应用到多台矿机；编辑保存后，正在使用它的矿机会自动更新。</p>
                <p>每个挖矿软件依次选择：数字货币 → 钱包 → 矿池 → 挖矿软件。矿池地址在「设定挖矿软件配置」里填写。</p>
                <p>矿机按飞行表里的地址自己下载挖矿软件，主控不保存安装包。</p>
              </div>
            }
          >
            <button type="button" className="fs-help-link">
              <i>i</i> Help
            </button>
          </Popover>
        </div>
        <Form
          form={form}
          layout="vertical"
          requiredMark={false}
          initialValues={{ tasks: [newTask(1)] }}
          onFinish={save}
        >
          <Form.List name="tasks">
            {(fields, { remove }) => (
              <>
                {fields.map(({ key, name }) => {
                  const task = tasks[name] ?? {};
                  const config = data(task.config);
                  return (
                    <div className="flight-stage" key={key}>
                      <span className="stage-number">{name + 1}</span>
                      <div className="flight-selectors">
                        <div className="flight-field">
                          <Form.Item
                            name={[name, "coin"]}
                            label={label("数字货币")}
                            rules={[{ required: true, message: "选择币种" }]}
                          >
                            <Select
                              showSearch
                              placeholder="输入币种"
                              options={symbols.map((c) => ({ value: c, label: <CoinLabel coin={c} /> }))}
                              optionFilterProp="value"
                              onChange={() => setTask(name, { wallet_id: undefined })}
                              onSearch={(value) => setCoinSearch({ ...coinSearch, [name]: value })}
                              notFoundContent={
                                <Button
                                  type="link"
                                  size="small"
                                  onMouseDown={(e) => e.preventDefault()}
                                  onClick={() =>
                                    setAddingWallet({ index: name, coin: coinSearch[name]?.trim().toUpperCase() || undefined })
                                  }
                                >
                                  添加 {coinSearch[name]?.trim().toUpperCase() || "新币种"} 钱包
                                </Button>
                              }
                            />
                          </Form.Item>
                        </div>
                        <div className={`flight-field ${task.coin ? "" : "disabled"}`}>
                          <Form.Item
                            name={[name, "wallet_id"]}
                            label={label(
                              "钱包",
                              task.coin && (
                                <a className="fs-card-link" onClick={() => setAddingWallet({ index: name, coin: task.coin })}>
                                  添加钱包
                                </a>
                              ),
                            )}
                            rules={[{ required: true, message: "选择钱包" }]}
                          >
                            <Select
                              showSearch
                              optionFilterProp="label"
                              disabled={!task.coin}
                              placeholder="选择钱包"
                              options={wallets.data
                                ?.filter((w) => symbol(w.id) === task.coin)
                                .map((w) => ({ value: w.id, label: w.name }))}
                            />
                          </Form.Item>
                        </div>
                        <div className={`flight-field ${task.miner_choice ? "" : "disabled"}`}>
                          <div className="ant-form-item fs-static">
                            {label(
                              "矿池",
                              task.miner_choice && (
                                <a className="fs-card-link" onClick={() => setConfigIndex(name)}>
                                  编辑
                                </a>
                              ),
                            )}
                            <button
                              type="button"
                              className="fs-pool"
                              disabled={!task.miner_choice}
                              title={lines(config.urls_text).join("\n") || undefined}
                              onClick={() => setConfigIndex(name)}
                            >
                              {task.miner_choice ? poolLabel(config) : "选择矿池"}
                            </button>
                          </div>
                        </div>
                        <div className={`flight-field ${task.wallet_id ? "" : "disabled"}`}>
                          <Form.Item
                            name={[name, "miner_choice"]}
                            label={label(
                              "挖矿软件",
                              task.miner_choice && (
                                <a className="fs-card-link" onClick={() => setConfigIndex(name)}>
                                  设定挖矿软件配置
                                </a>
                              ),
                            )}
                            rules={[{ required: true, message: "选择挖矿软件" }]}
                          >
                            <Select
                              showSearch
                              optionFilterProp="title"
                              placeholder="选择挖矿软件"
                              loading={adapters.isLoading}
                              popupClassName="fs-miner-options"
                              options={[
                                ...builtins.map((a) => ({
                                  value: a.id,
                                  title: a.name,
                                  label: (
                                    <span className="fs-miner-option">
                                      <span>{a.releases.length ? a.name : `${a.name}（自备下载地址）`}</span>
                                      <DeviceTags adapter={a.id} />
                                    </span>
                                  ),
                                })),
                                {
                                  value: "custom",
                                  title: "Custom",
                                  label: (
                                    <span className="fs-miner-option">
                                      <span>Custom</span>
                                      <DeviceTags adapter="custom" />
                                    </span>
                                  ),
                                },
                              ]}
                              onChange={(choice: string) => {
                                const adapter = adapterOf(choice);
                                const current = data(form.getFieldValue(["tasks", name, "config"]));
                                setTask(name, {
                                  miner: adapter ? defaultMiner(adapter) : {},
                                  config: { ...defaultConfig(choice), urls_text: current.urls_text },
                                });
                                setConfigIndex(name);
                              }}
                            />
                          </Form.Item>
                        </div>
                      </div>
                      <Form.Item name={[name, "instance"]} hidden>
                        <Input />
                      </Form.Item>
                      <Form.Item name={[name, "config"]} hidden>
                        <StoredValue />
                      </Form.Item>
                      <Form.Item name={[name, "miner"]} hidden>
                        <StoredValue />
                      </Form.Item>
                      {fields.length > 1 && (
                        <Button
                          aria-label={`移除矿工 ${name + 1}`}
                          className="stage-remove"
                          icon={<DeleteOutlined />}
                          type="text"
                          danger
                          onClick={() => remove(name)}
                        />
                      )}
                    </div>
                  );
                })}
              </>
            )}
          </Form.List>
          <div className="composer-bottom">
            <Form.Item
              name="name"
              label="名称"
              rules={[{ required: true, message: "输入飞行表名称" }]}
            >
              <Input placeholder="输入飞行表的名称" />
            </Form.Item>
            <div className="add-miner">
              <span className="muted">如果您要运行多个挖矿软件</span>
              <Button
                className="fs-dark-button"
                onClick={() => {
                  const used = new Set(tasks.map((t) => t.instance));
                  let n = 1;
                  while (used.has(`cpu-${n}`)) n++;
                  form.setFieldValue("tasks", [...tasks, newTask(n)]);
                }}
              >
                添加挖矿软件
              </Button>
            </div>
          </div>
          <div className="composer-footer">
            <span className="fs-import">
              添加自
              <Button className="fs-dark-button" onClick={() => fileInput.current?.click()}>
                文件
              </Button>
              <Button
                className="fs-dark-button"
                onClick={async () => {
                  try {
                    importSheet(await navigator.clipboard.readText());
                  } catch {
                    setPasted("");
                    setPasting(true);
                  }
                }}
              >
                剪贴板
              </Button>
              <input
                ref={fileInput}
                type="file"
                accept="application/json,.json"
                style={{ display: "none" }}
                onChange={async (e) => {
                  const file = e.target.files?.[0];
                  e.target.value = "";
                  if (file) importSheet(await file.text());
                }}
              />
            </span>
            <span className="fs-footer-actions">
              <Button type="text" onClick={reset}>
                {edit ? "取消编辑" : "重置"}
              </Button>
              <Button className="fs-create" htmlType="submit" loading={busy}>
                {edit ? "保存并更新矿机" : "创建飞行表"}
              </Button>
            </span>
          </div>
        </Form>
      </section>
      <div className="fs-filters">
        {(
          [
            ["all", "所有"],
            ["applied", "Applied"],
            ["unapplied", "Not Applied"],
          ] as const
        ).map(([value, text]) => (
          <button
            key={value}
            type="button"
            className={status === value ? "active" : ""}
            aria-pressed={status === value}
            onClick={() => {
              setStatus(value);
              setPage(1);
            }}
          >
            {text}
          </button>
        ))}
        <Input.Search
          aria-label="搜索飞行表"
          placeholder="搜索名称或币种"
          size="small"
          value={search}
          allowClear
          onChange={(e) => {
            setSearch(e.target.value);
            setPage(1);
          }}
        />
      </div>
      {chips.length > 0 && (
        <div className="fs-chips">
          {chips.map((c) => (
            <button
              key={c.key}
              type="button"
              className={chip === c.key ? "active" : ""}
              aria-pressed={chip === c.key}
              onClick={() => {
                setChip(chip === c.key ? undefined : c.key);
                setPage(1);
              }}
            >
              {c.kind === "coin" ? <CoinBadge coin={c.label} size={14} /> : c.kind === "wallet" ? <span className="fs-wallet-icon" /> : <IconPickaxe size={13} />}
              {c.label}
            </button>
          ))}
        </div>
      )}
      <QueryState
        loading={flights.isLoading || wallets.isLoading || coins.isLoading}
        error={flights.error ?? wallets.error ?? coins.error ?? adapters.error}
      >
        {!filtered.length ? (
          <Empty description="暂无匹配的飞行表" />
        ) : (
          filtered.slice((page - 1) * 12, page * 12).map((f) => (
            <article className="flight-card" key={f.id}>
              <div className="flight-card-body">
                {f.tasks.map((t, index) => (
                  <div className="flight-card-row" key={t.instance}>
                    <div className="fs-row-coin">
                      <strong className="coin-label">
                        <CoinBadge coin={symbol(t.wallet_id)} /> {symbol(t.wallet_id)}
                      </strong>
                      {index === 0 && (
                        <small>
                          {f.name} <span>v{f.version}</span>
                        </small>
                      )}
                    </div>
                    <span>{walletName(t.wallet_id) ?? "未知钱包"}</span>
                    <span title={(data(t.config).urls ?? []).join("\n")}>{poolLabel(data(t.config))}</span>
                    <span>
                      {data(t.miner).name ?? "未知矿工"}
                      {data(t.miner).version ? <span className="muted"> {data(t.miner).version}</span> : null}{" "}
                      <DeviceTags adapter={data(t.miner).adapter} short />
                    </span>
                  </div>
                ))}
              </div>
              <Dropdown
                trigger={["click"]}
                placement="bottomRight"
                menu={{
                  items: [
                    { key: "apply", label: "应用到矿机" },
                    { key: "edit", label: "编辑" },
                    { key: "copy", label: "复制" },
                    { key: "export", label: "导出" },
                    { key: "clipboard", label: "剪贴板" },
                    { type: "divider" },
                    { key: "delete", label: "移除", danger: true },
                  ],
                  onClick: ({ key }) => {
                    if (key === "apply") {
                      setTargets([]);
                      setApply(f);
                    } else if (key === "edit") openEditor(f);
                    else if (key === "copy") openEditor(f, true);
                    else if (key === "export") download(f);
                    else if (key === "clipboard") void copyToClipboard(f);
                    else
                      modal.confirm({
                        title: `移除飞行表「${f.name}」？`,
                        content: "已提交的部署快照仍会保留。",
                        okText: "移除",
                        okButtonProps: { danger: true },
                        onOk: async () => {
                          try {
                            await unwrap(client.DELETE("/api/v1/flight-sheets/{id}", { params: { path: { id: f.id } } }));
                            await refresh();
                          } catch (e) {
                            message.error((e as Error).message);
                          }
                        },
                      });
                  },
                }}
              >
                <button type="button" className="fs-more" aria-label={`${f.name} 操作`}>
                  •••
                </button>
              </Dropdown>
            </article>
          ))
        )}
        <Pagination
          current={page}
          pageSize={12}
          total={filtered.length}
          onChange={setPage}
          hideOnSinglePage
        />
      </QueryState>
      <MinerDialog
        open={configIndex !== undefined}
        adapter={adapterOf(configTask?.miner_choice)}
        miner={data(configTask?.miner)}
        config={data(configTask?.config)}
        onCancel={() => setConfigIndex(undefined)}
        onApply={(miner, config) => {
          setTask(configIndex!, { miner, config });
          setConfigIndex(undefined);
        }}
      />
      <WalletDialog
        open={addingWallet !== undefined}
        initialCoin={addingWallet?.coin}
        lockCoin={!!addingWallet?.coin}
        coinOptions={symbols}
        onClose={() => setAddingWallet(undefined)}
        onSaved={(item, coin) => {
          if (addingWallet !== undefined) setTask(addingWallet.index, { wallet_id: item.id, coin });
        }}
      />
      <Modal
        title="从剪贴板添加"
        open={pasting}
        okText="载入"
        okButtonProps={{ disabled: !pasted.trim() }}
        onCancel={() => setPasting(false)}
        onOk={() => {
          importSheet(pasted);
          setPasting(false);
        }}
      >
        <p className="muted">浏览器不允许直接读取剪贴板，请把导出的内容粘贴到这里。</p>
        <Input.TextArea rows={8} value={pasted} onChange={(e) => setPasted(e.target.value)} className="mono" />
      </Modal>
      <Modal
        title={`应用飞行表：${apply?.name ?? ""}`}
        open={!!apply}
        onCancel={() => setApply(undefined)}
        confirmLoading={busy}
        okText="应用到所选矿机"
        okButtonProps={{ disabled: targets.length === 0 }}
        onOk={async () => {
          setBusy(true);
          try {
            await submitJob({
              action: { kind: "apply", sheet_id: apply!.id },
              machine_ids: targets,
              include_controller: false,
            });
            setApply(undefined);
            message.success(targets.length === 1
              ? "已下发，结果见矿机的「消息」"
              : `已下发到 ${targets.length} 台矿机，结果见各矿机的「消息」`);
          } catch (e) {
            message.error((e as Error).message);
          } finally {
            setBusy(false);
          }
        }}
      >
        <p className="muted">同时下发到所选矿机，结果见各矿机的「消息」。</p>
        <Select
          mode="multiple"
          placeholder="选择目标机器"
          style={{ width: "100%" }}
          value={targets}
          onChange={setTargets}
          options={machines.data?.map((m) => ({ value: m.id, label: m.name }))}
        />
      </Modal>
    </div>
  );
}
