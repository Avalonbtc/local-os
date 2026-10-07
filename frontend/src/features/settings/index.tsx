import { useState } from "react";
import {
  Alert,
  App,
  Button,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Space,
  Table,
  Tabs,
  Typography,
} from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import {
  client,
  unwrap,
  useFarmSettings,
  useRefresh,
  type FarmSettings,
  type TokenInfo,
  type AuditEvent,
} from "../../shared/api";
import { PageHeader, QueryState } from "../../shared/ui";

/** Farm preferences; the electricity price drives the summary bar's daily cost estimate. */
function FarmSettingsForm() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const settings = useFarmSettings();
  const [busy, setBusy] = useState(false);
  return (
    <QueryState loading={settings.isLoading} error={settings.error}>
      <Form<FarmSettings>
        className="farm-settings"
        layout="vertical"
        initialValues={settings.data}
        onFinish={async (values) => {
          setBusy(true);
          try {
            const saved = await unwrap(client.PUT("/api/v1/settings/farm", { body: values }));
            queryClient.setQueryData(["farm-settings"], saved);
            message.success("已保存，矿机列表的预估电费已按新电价计算");
          } catch (e) {
            message.error((e as Error).message);
          } finally {
            setBusy(false);
          }
        }}
      >
        <Form.Item
          name="electricity_price"
          label="电价"
          extra="每千瓦时（度）的电费。矿机列表顶部的「预估电费」按当前软件功耗持续 24 小时乘以这个电价估算。"
          rules={[{ required: true, message: "输入电价" }]}
        >
          <InputNumber min={0} max={100} step={0.01} precision={4} addonAfter="元/度" style={{ width: "100%" }} />
        </Form.Item>
        <Button type="primary" htmlType="submit" loading={busy}>
          保存
        </Button>
      </Form>
    </QueryState>
  );
}

export function SettingsPage() {
  const { message } = App.useApp();
  const [params, setParams] = useSearchParams();
  const refresh = useRefresh();
  const [name, setName] = useState("");
  const [token, setToken] = useState("");
  const tokens = useQuery({
    queryKey: ["tokens"],
    queryFn: () => unwrap(client.GET("/api/v1/tokens")),
  });
  const audit = useQuery({
    queryKey: ["audit"],
    queryFn: () => unwrap(client.GET("/api/v1/audit")),
  });
  return (
    <>
      <PageHeader title="设置" description="矿场、AI 接口、访问令牌与审计记录" />
      <Tabs
        activeKey={params.get("tab") ?? "farm"}
        onChange={(tab) => setParams({ tab }, { replace: true })}
        items={[
          {
            key: "farm",
            label: "矿场",
            children: <FarmSettingsForm />,
          },
          {
            key: "tokens",
            label: "AI / API 令牌",
            children: (
              <>
                <Alert
                  type="info"
                  showIcon
                  message="令牌具有完整管理权限，可随时撤销"
                  description={`REST：${location.origin}/api/v1 · MCP：${location.origin}/mcp。使用 Authorization: Bearer <token>。`}
                />
                <Space className="settings-create">
                  <Input
                    aria-label="令牌名称"
                    placeholder="令牌名称，例如本地 AI Agent"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    style={{ width: "min(320px, 100%)" }}
                  />
                  <Button
                    type="primary"
                    disabled={!name.trim()}
                    onClick={async () => {
                      try {
                        const result = await unwrap(
                          client.POST("/api/v1/tokens", { body: { name } }),
                        );
                        setToken(String(result.token));
                        setName("");
                        await refresh();
                      } catch (e) {
                        message.error((e as Error).message);
                      }
                    }}
                  >
                    创建令牌
                  </Button>
                </Space>
                <Table<TokenInfo>
                  rowKey="id"
                  loading={tokens.isLoading}
                  dataSource={tokens.data}
                  columns={[
                    { title: "名称", dataIndex: "name" },
                    {
                      title: "创建时间",
                      dataIndex: "created_at",
                      render: (v) => new Date(v).toLocaleString(),
                    },
                    {
                      title: "状态",
                      key: "status",
                      render: (_, r) => (r.revoked_at ? "已撤销" : "有效"),
                    },
                    {
                      title: "操作",
                      key: "action",
                      render: (_, r) => (
                        <Popconfirm
                          title="撤销此令牌？"
                          onConfirm={async () => {
                            try {
                              await unwrap(
                                client.DELETE("/api/v1/tokens/{id}", {
                                  params: { path: { id: r.id } },
                                }),
                              );
                              await refresh();
                            } catch (e) {
                              message.error((e as Error).message);
                            }
                          }}
                        >
                          <Button danger disabled={!!r.revoked_at}>
                            撤销
                          </Button>
                        </Popconfirm>
                      ),
                    },
                  ]}
                />
              </>
            ),
          },
          {
            key: "audit",
            label: "审计记录",
            children: (
              <Table<AuditEvent>
                rowKey="id"
                loading={audit.isLoading}
                dataSource={audit.data}
                pagination={{ pageSize: 25 }}
                columns={[
                  {
                    title: "时间",
                    dataIndex: "created_at",
                    render: (v) => new Date(v).toLocaleString(),
                  },
                  { title: "操作者", dataIndex: "actor" },
                  { title: "动作", dataIndex: "action" },
                  { title: "目标", dataIndex: "target", ellipsis: true },
                ]}
              />
            ),
          },
          {
            key: "retention",
            label: "保留与备份",
            children: (
              <div className="settings-text">
                <h3>默认保留周期</h3>
                <Table
                  pagination={false}
                  rowKey="name"
                  dataSource={[
                    { name: "原始指标", value: "7 天" },
                    { name: "分钟聚合", value: "90 天" },
                    { name: "任务输出", value: "30 天 / 每机每任务最多 1 MiB" },
                    { name: "审计记录", value: "180 天" },
                  ]}
                  columns={[
                    { title: "数据", dataIndex: "name" },
                    { title: "保留策略", dataIndex: "value" },
                  ]}
                />
                <p>
                  部署目录 scripts/backup.sh
                  创建数据库与配置备份。主密钥须单独保存；恢复步骤见
                  docs/operations.md。
                </p>
                <Typography.Link href="/api/v1/openapi.json" target="_blank">
                  查看 OpenAPI 文档
                </Typography.Link>
              </div>
            ),
          },
        ]}
      />
      <Modal
        open={!!token}
        title="新令牌（仅显示一次）"
        onCancel={() => setToken("")}
        onOk={() => setToken("")}
        cancelButtonProps={{ style: { display: "none" } }}
      >
        <Alert type="warning" message="请保存在你的密码管理器中" />
        <Typography.Paragraph copyable className="token-value">
          {token}
        </Typography.Paragraph>
      </Modal>
    </>
  );
}
