// Wallets, like HiveOS: a coin symbol plus an address. Coins are just the symbols wallets use;
// there is no separate coin, pool or miner catalogue any more.
import { useMemo, useState } from "react";
import { App, AutoComplete, Button, Form, Input, Modal, Popconfirm, Space, Table, Tooltip } from "antd";
import { DeleteOutlined, EditOutlined, PlusOutlined } from "@ant-design/icons";
import {
  client,
  data,
  unwrap,
  useCatalog,
  useRefresh,
  type CatalogItem,
  type CatalogInput,
  type Data,
} from "../../shared/api";
import { PageHeader, QueryState } from "../../shared/ui";

/** Wallet id → coin symbol, resolved through the coin rows wallets reference. */
export function useWalletCoins() {
  const wallets = useCatalog("wallets");
  const coins = useCatalog("coins");
  const symbols: Map<string, string> = useMemo(() => {
    const byCoin = new Map<string, string>((coins.data ?? []).map((c) => [c.id, String(data(c.data).symbol ?? "")]));
    return new Map<string, string>(
      (wallets.data ?? []).map((w) => [
        w.id,
        byCoin.get(data(w.data).coin_id) || String(data(w.data).coin_symbol ?? "") || "未知币种",
      ]),
    );
  }, [wallets.data, coins.data]);
  return {
    wallets,
    coins,
    symbol: (walletId?: string) => (walletId && symbols.get(walletId)) || "未知币种",
    symbols: [...new Set(symbols.values())].sort(),
  };
}

export function WalletDialog({
  open,
  wallet,
  initialCoin,
  coinOptions,
  onClose,
  onSaved,
}: {
  open: boolean;
  wallet?: CatalogItem;
  initialCoin?: string;
  coinOptions: string[];
  onClose: () => void;
  onSaved?: (item: CatalogItem, coin: string) => void;
}) {
  const [form] = Form.useForm();
  const [busy, setBusy] = useState(false);
  const refresh = useRefresh();
  const { message } = App.useApp();
  const save = async (v: Data) => {
    setBusy(true);
    try {
      const coin = String(v.coin_symbol).trim().toUpperCase();
      const body: CatalogInput = {
        name: v.name,
        data: { coin_symbol: coin, address: v.address.trim(), source: v.source ?? "", notes: v.notes ?? "" },
        expected_revision: wallet?.revision ?? null,
      };
      const saved: CatalogItem = await unwrap(
        wallet
          ? client.PUT("/api/v1/catalog/{kind}/{id}", { params: { path: { kind: "wallets", id: wallet.id } }, body })
          : client.POST("/api/v1/catalog/{kind}", { params: { path: { kind: "wallets" } }, body }),
      );
      message.success("钱包已保存");
      await refresh();
      onSaved?.(saved, coin);
      onClose();
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      title={wallet ? "编辑钱包" : "添加钱包"}
      open={open}
      onCancel={onClose}
      onOk={() => form.submit()}
      confirmLoading={busy}
      className="hive-dialog"
      okText={wallet ? "保存" : "创建"}
      cancelText="取消"
      destroyOnHidden
      width={620}
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={save}
        preserve={false}
        initialValues={{
          coin_symbol: initialCoin,
          name: wallet?.name,
          address: data(wallet?.data).address,
          source: data(wallet?.data).source,
          notes: data(wallet?.data).notes,
        }}
      >
        <Form.Item name="coin_symbol" label="数字货币" rules={[{ required: true, message: "输入币种" }]} extra="任意币种符号，例如 XMR、ZEPH">
          <AutoComplete
            options={coinOptions.map((value) => ({ value }))}
            filterOption={(input, option) => String(option?.value).toLowerCase().includes(input.toLowerCase())}
            placeholder="XMR"
          />
        </Form.Item>
        <Form.Item name="address" label="地址" rules={[{ required: true, message: "输入收款地址或矿池账号" }]}>
          <Input.TextArea rows={3} />
        </Form.Item>
        <Form.Item name="name" label="名称" rules={[{ required: true, message: "输入钱包名称" }]}>
          <Input placeholder="例如：交易所 XMR" />
        </Form.Item>
        <Form.Item name="source" label="来源">
          <Input placeholder="交易所、钱包类型或矿池账号" />
        </Form.Item>
        <Form.Item name="notes" label="备注">
          <Input />
        </Form.Item>
      </Form>
    </Modal>
  );
}

export function WalletsPage() {
  const { wallets, coins, symbol, symbols } = useWalletCoins();
  const refresh = useRefresh();
  const { message } = App.useApp();
  const [editing, setEditing] = useState<CatalogItem | "new">();
  const [coinFilter, setCoinFilter] = useState<string>();
  const [search, setSearch] = useState("");
  const rows = (wallets.data ?? []).filter(
    (w) =>
      (!coinFilter || symbol(w.id) === coinFilter) &&
      `${w.name} ${symbol(w.id)} ${data(w.data).address} ${data(w.data).source ?? ""}`.toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <>
      <PageHeader
        title="钱包"
        description="收款地址或矿池账号；飞行表按币种选择钱包"
        extra={
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setEditing("new")}>
            添加钱包
          </Button>
        }
      />
      <div className="catalog-filters">
        <Button type={!coinFilter ? "primary" : "text"} onClick={() => setCoinFilter(undefined)}>
          全部
        </Button>
        {symbols.map((c) => (
          <Button key={c} type={coinFilter === c ? "primary" : "text"} onClick={() => setCoinFilter(c)}>
            {c}
          </Button>
        ))}
        <Input.Search aria-label="搜索钱包" placeholder="搜索名称、币种或地址" allowClear value={search} onChange={(e) => setSearch(e.target.value)} />
      </div>
      <QueryState loading={wallets.isLoading || coins.isLoading} error={wallets.error ?? coins.error}>
        <Table<CatalogItem>
          rowKey="id"
          size="small"
          className="catalog-table"
          dataSource={rows}
          pagination={{ pageSize: 20, hideOnSinglePage: true }}
          columns={[
            {
              title: "钱包",
              key: "name",
              width: "34%",
              render: (_, r) => (
                <div className="wallet-cell">
                  <strong className="coin-label">{symbol(r.id)}</strong>
                  <b>{r.name}</b>
                  <div className="muted">{data(r.data).source || "—"}</div>
                  <div className="wallet-address mono">{data(r.data).address}</div>
                </div>
              ),
            },
            {
              title: "地址",
              key: "address",
              responsive: ["sm"],
              render: (_, r) => <span className="wallet-address mono">{data(r.data).address}</span>,
            },
            {
              title: "",
              key: "actions",
              width: 84,
              align: "right",
              render: (_, r) => (
                <Space size={0}>
                  <Tooltip title="编辑">
                    <Button type="text" icon={<EditOutlined />} aria-label={`编辑 ${r.name}`} onClick={() => setEditing(r)} />
                  </Tooltip>
                  <Popconfirm
                    title="删除此钱包？"
                    description="被飞行表引用的钱包不能删除。"
                    onConfirm={async () => {
                      try {
                        await unwrap(client.DELETE("/api/v1/catalog/{kind}/{id}", { params: { path: { kind: "wallets", id: r.id } } }));
                        await refresh();
                      } catch (e) {
                        message.error((e as Error).message);
                      }
                    }}
                  >
                    <Button type="text" danger icon={<DeleteOutlined />} aria-label={`删除 ${r.name}`} />
                  </Popconfirm>
                </Space>
              ),
            },
          ]}
        />
      </QueryState>
      <WalletDialog
        open={!!editing}
        wallet={editing === "new" ? undefined : editing}
        initialCoin={editing && editing !== "new" ? symbol(editing.id) : coinFilter}
        coinOptions={symbols}
        onClose={() => setEditing(undefined)}
      />
    </>
  );
}
