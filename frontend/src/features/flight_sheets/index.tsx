import { useEffect, useState } from "react";
import {
  App,
  Button,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Tag,
  Empty,
  Pagination,
  Popconfirm,
} from "antd";
import {
  PlusOutlined,
  RocketOutlined,
  DeleteOutlined,
  CopyOutlined,
  EditOutlined,
} from "@ant-design/icons";
import {
  client,
  data,
  unwrap,
  useAdapters,
  useFlights,
  useMachines,
  useRefresh,
  submitJob,
  type FlightSheet,
  type FlightInput,
  type Data,
} from "../../shared/api";
import { QueryState } from "../../shared/ui";
import { WalletDialog, useWalletCoins } from "../wallets";
import { MinerDialog, defaultConfig, defaultMiner } from "./miner-dialog";
import { useSearchParams } from "react-router-dom";

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
  const first = (config.urls_text !== undefined ? lines(config.urls_text) : (config.urls ?? []))[0] as string | undefined;
  if (!first) return "在挖矿软件中配置";
  return first.replace(/^[a-z+]+:\/\//i, "");
}
export function FlightsPage() {
  const flights = useFlights(),
    adapters = useAdapters(),
    machines = useMachines(),
    { wallets, coins, symbol, symbols } = useWalletCoins();
  const refresh = useRefresh(),
    { message } = App.useApp();
  const [params, setParams] = useSearchParams();
  const [form] = Form.useForm();
  const tasks: Data[] = Form.useWatch("tasks", form) ?? [newTask(1)];
  const [edit, setEdit] = useState<FlightSheet>(),
    [busy, setBusy] = useState(false),
    [apply, setApply] = useState<FlightSheet>(),
    [targets, setTargets] = useState<string[]>([]);
  const [configIndex, setConfigIndex] = useState<number>();
  const [addingWallet, setAddingWallet] = useState<number>(),
    [search, setSearch] = useState(""),
    [page, setPage] = useState(1);
  const [coinFilter, setCoinFilter] = useState<string>();
  const builtins = (adapters.data ?? []).filter((a) => a.id !== "hive-custom");
  const adapterOf = (choice?: string) => builtins.find((a) => a.id === choice);
  const reset = () => {
    setEdit(undefined);
    form.resetFields();
  };
  const openEditor = (sheet: FlightSheet, copy = false) => {
    setEdit(copy ? undefined : sheet);
    form.setFieldsValue({
      name: copy ? `${sheet.name} 副本` : sheet.name,
      tasks: sheet.tasks.map((t) => {
        const miner = data(t.miner);
        const config = data(t.config);
        return {
          instance: t.instance,
          wallet_id: t.wallet_id,
          coin: symbol(t.wallet_id),
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
            throw new Error(`第 ${index + 1} 个挖矿软件缺少矿池地址`);
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
  const filtered =
    flights.data?.filter(
      (f) =>
        (!coinFilter || f.tasks.some((t) => symbol(t.wallet_id) === coinFilter)) &&
        `${f.name} ${f.tasks.map((t) => `${symbol(t.wallet_id)} ${data(t.miner).name ?? ""}`).join(" ")}`
          .toLowerCase()
          .includes(search.toLowerCase()),
    ) ?? [];
  const configTask = configIndex !== undefined ? tasks[configIndex] : undefined;
  return (
    <div className="flights-page">
      <div className="hive-note">
        飞行表可应用到多个矿机。编辑保存后，自动更新正在使用它的矿机。
      </div>
      <section className="flight-composer">
        <div className="composer-heading">
          <h1>{edit ? `编辑飞行表 · ${edit.name}` : "添加新的飞行表"}</h1>
          {edit && <Tag>v{edit.version}</Tag>}
        </div>
        <Form
          form={form}
          layout="vertical"
          initialValues={{ tasks: [newTask(1)] }}
          onFinish={save}
        >
          <Form.List name="tasks">
            {(fields, { remove }) => (
              <>
                {fields.map(({ key, name }) => {
                  const task = tasks[name] ?? {};
                  return (
                    <div className="flight-stage" key={key}>
                      <span className="stage-number">{name + 1}</span>
                      <div className="flight-selectors">
                        <div className="flight-field">
                          <Form.Item
                            name={[name, "coin"]}
                            label="数字货币"
                            rules={[{ required: true, message: "选择币种" }]}
                          >
                            <Select
                              showSearch
                              placeholder="输入币种"
                              options={symbols.map((c) => ({ value: c, label: c }))}
                              onChange={() => setTask(name, { wallet_id: undefined })}
                            />
                          </Form.Item>
                          <Button type="link" size="small" onClick={() => setAddingWallet(name)}>
                            自定义币种 / 钱包
                          </Button>
                        </div>
                        <div className="flight-field">
                          <Form.Item
                            name={[name, "wallet_id"]}
                            label="钱包"
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
                          <Button type="link" size="small" onClick={() => setAddingWallet(name)}>
                            添加钱包
                          </Button>
                        </div>
                        <div className="flight-field">
                          <Form.Item
                            name={[name, "config", "urls_text"]}
                            label="矿池"
                            tooltip="每行一个地址，第一行为主矿池；Custom 矿工也可以在其他配置参数里设置"
                          >
                            <Input.TextArea
                              autoSize={{ minRows: 1, maxRows: 4 }}
                              placeholder="stratum+tcp://pool:3333"
                              className="mono"
                            />
                          </Form.Item>
                        </div>
                        <div className="flight-field">
                          <Form.Item
                            name={[name, "miner_choice"]}
                            label="挖矿软件"
                            rules={[{ required: true, message: "选择挖矿软件" }]}
                          >
                            <Select
                              showSearch
                              optionFilterProp="label"
                              placeholder="选择挖矿软件"
                              loading={adapters.isLoading}
                              options={[
                                ...builtins.map((a) => ({
                                  value: a.id,
                                  label: a.releases.length ? a.name : `${a.name}（自备下载地址）`,
                                })),
                                { value: "custom", label: "Custom" },
                              ]}
                              onChange={(choice: string) => {
                                const adapter = adapterOf(choice);
                                const current = data(form.getFieldValue(["tasks", name, "config"]));
                                setTask(name, {
                                  miner: adapter ? defaultMiner(adapter) : {},
                                  config: { ...defaultConfig(choice), urls_text: current.urls_text },
                                });
                                if (!adapter || !adapter.releases.length) setConfigIndex(name);
                              }}
                            />
                          </Form.Item>
                          <Space size={0}>
                            <Button
                              type="link"
                              size="small"
                              disabled={!task.miner_choice}
                              onClick={() => setConfigIndex(name)}
                            >
                              设定挖矿软件配置
                            </Button>
                            {data(task.miner).version && (
                              <span className="muted">v{data(task.miner).version}</span>
                            )}
                          </Space>
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
                icon={<PlusOutlined />}
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
            <span className="muted">矿机按飞行表里的地址自己下载挖矿软件</span>
            <Space>
              <Button type="text" onClick={reset}>
                {edit ? "取消编辑" : "重置"}
              </Button>
              <Button type="primary" htmlType="submit" loading={busy}>
                {edit ? "保存并更新矿机" : "创建飞行表"}
              </Button>
            </Space>
          </div>
        </Form>
      </section>
      <div className="sheet-list-heading">
        <h3>我的飞行表</h3>
        <Input.Search
          aria-label="搜索飞行表"
          placeholder="搜索名称或币种"
          value={search}
          allowClear
          onChange={(e) => {
            setSearch(e.target.value);
            setPage(1);
          }}
        />
      </div>
      <div className="catalog-filters">
        <Button
          type={!coinFilter ? "primary" : "text"}
          onClick={() => {
            setCoinFilter(undefined);
            setPage(1);
          }}
        >
          全部
        </Button>
        {[
          ...new Set(
            (flights.data ?? []).flatMap((f) =>
              f.tasks.map((t) => symbol(t.wallet_id)),
            ),
          ),
        ].map((c) => (
          <Button
            key={c}
            type={coinFilter === c ? "primary" : "text"}
            onClick={() => {
              setCoinFilter(c);
              setPage(1);
            }}
          >
            {c}
          </Button>
        ))}
      </div>
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
                {f.tasks.map((t) => (
                  <div className="flight-card-row" key={t.instance}>
                    <strong className="coin-label">{symbol(t.wallet_id)}</strong>
                    <span>{wallets.data?.find((w) => w.id === t.wallet_id)?.name ?? "未知钱包"}</span>
                    <span className="mono" title={(data(t.config).urls ?? []).join("\n")}>{poolLabel(data(t.config))}</span>
                    <span>
                      {data(t.miner).name ?? "未知矿工"}
                      {data(t.miner).version ? <span className="muted"> {data(t.miner).version}</span> : null}
                    </span>
                  </div>
                ))}
                <div className="flight-card-meta">
                  {f.name}
                  <span>v{f.version}</span>
                </div>
              </div>
              <Space className="flight-card-actions">
                <Button
                  icon={<RocketOutlined />}
                  aria-label={`应用 ${f.name}`}
                  onClick={() => {
                    setTargets([]);
                    setApply(f);
                  }}
                >
                  应用
                </Button>
                <Button
                  type="text"
                  icon={<EditOutlined />}
                  aria-label={`编辑 ${f.name}`}
                  onClick={() => openEditor(f)}
                >编辑</Button>
                <Button
                  type="text"
                  icon={<CopyOutlined />}
                  aria-label={`复制 ${f.name}`}
                  onClick={() => openEditor(f, true)}
                />
                <Popconfirm
                  title="删除飞行表模板？"
                  description="已提交的部署快照仍会保留。"
                  onConfirm={async () => {
                    try {
                      await unwrap(
                        client.DELETE("/api/v1/flight-sheets/{id}", {
                          params: { path: { id: f.id } },
                        }),
                      );
                      await refresh();
                    } catch (e) {
                      message.error((e as Error).message);
                    }
                  }}
                >
                  <Button
                    type="text"
                    danger
                    icon={<DeleteOutlined />}
                    aria-label={`删除 ${f.name}`}
                  />
                </Popconfirm>
              </Space>
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
        initialCoin={addingWallet !== undefined ? tasks[addingWallet]?.coin : undefined}
        coinOptions={symbols}
        onClose={() => setAddingWallet(undefined)}
        onSaved={(item, coin) => {
          if (addingWallet !== undefined) setTask(addingWallet, { wallet_id: item.id, coin });
        }}
      />
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
              concurrency: 4,
              canary: true,
              include_controller: false,
            });
            setApply(undefined);
            message.success(targets.length === 1
              ? "已下发，结果见矿机的「消息」"
              : `已下发到 ${targets.length} 台矿机（先在 1 台上验证），结果见各矿机的「消息」`);
          } catch (e) {
            message.error((e as Error).message);
          } finally {
            setBusy(false);
          }
        }}
      >
        <p className="muted">先验证一台，再以 4 台并发继续。主控默认排除。</p>
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
