import { useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  App,
  Button,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Select,
  Space,
  Table,
  Tag,
} from "antd";
import { CopyOutlined, PlusOutlined } from "@ant-design/icons";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import {
  client,
  serverNow,
  unwrap,
  useEnrollmentTokens,
  useMachines,
  type EnrollmentToken,
  type EnrollmentTokenInput,
} from "../../shared/api";
import { QueryState } from "../../shared/ui";

/** A key can still add machines: not revoked, not expired, uses left. */
export function usable(token: EnrollmentToken) {
  return (
    !token.revoked_at &&
    (!token.expires_at || new Date(token.expires_at).getTime() > serverNow()) &&
    (token.max_uses == null || token.uses < token.max_uses)
  );
}

function tokenState(token: EnrollmentToken) {
  if (token.revoked_at) return <Tag>已撤销</Tag>;
  if (token.expires_at && new Date(token.expires_at).getTime() <= serverNow()) return <Tag>已过期</Tag>;
  if (token.max_uses != null && token.uses >= token.max_uses) return <Tag>次数已用完</Tag>;
  return <Tag color="success">可用</Tag>;
}

export function CopyCommand({ command }: { command: string }) {
  const { message } = App.useApp();
  return (
    <div className="install-command">
      <code>{command}</code>
      <Button
        icon={<CopyOutlined />}
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(command);
            message.success("已复制安装命令");
          } catch {
            message.info("请手动选中命令复制");
          }
        }}
      >
        复制
      </Button>
    </div>
  );
}

/** HiveOS-style "add worker": one command on the rig and it joins the farm by itself. */
export function EnrollModal({
  open,
  onClose,
  onSsh,
  onSshBatch,
}: {
  open: boolean;
  onClose: () => void;
  onSsh: () => void;
  onSshBatch: () => void;
}) {
  const tokens = useEnrollmentTokens();
  const machines = useMachines();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { message } = App.useApp();
  const [chosen, setChosen] = useState<string>();
  const [creating, setCreating] = useState(false);
  const known = useRef<Set<string> | null>(null);
  const available = (tokens.data ?? []).filter(usable);
  const token = available.find((t) => t.id === chosen) ?? available[0];

  // The first time anyone adds a machine there is no key yet: make one.
  useEffect(() => {
    if (!open || tokens.isLoading || tokens.error || available.length || creating) return;
    setCreating(true);
    unwrap(client.POST("/api/v1/enrollment-tokens", { body: { name: "默认安装密钥", group: "", tags: [] } }))
      .then(() => queryClient.invalidateQueries({ queryKey: ["enrollment-tokens"] }))
      .catch((e) => message.error((e as Error).message))
      .finally(() => setCreating(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, tokens.isLoading, available.length]);

  // Show machines that join while the dialog is open.
  useEffect(() => {
    if (open && machines.data && !known.current) known.current = new Set(machines.data.map((m) => m.id));
    if (!open) known.current = null;
  }, [open, machines.data]);
  useEffect(() => {
    if (!open) return;
    const timer = window.setInterval(() => queryClient.invalidateQueries({ queryKey: ["machines"] }), 3000);
    return () => window.clearInterval(timer);
  }, [open, queryClient]);
  const joined = useMemo(
    () => (machines.data ?? []).filter((m) => known.current && !known.current.has(m.id)),
    [machines.data],
  );

  return (
    <Modal
      open={open}
      width={760}
      title="添加矿机"
      onCancel={onClose}
      footer={
        <Space wrap>
          <Button onClick={onSsh}>改用 SSH 添加</Button>
          <Button onClick={onSshBatch}>SSH 批量添加</Button>
          <Button type="primary" onClick={onClose}>
            完成
          </Button>
        </Space>
      }
      destroyOnHidden
    >
      <QueryState loading={tokens.isLoading || (creating && !token)} error={tokens.error}>
        <ol className="enroll-steps">
          <li>
            在矿机上用 root 执行下面这条命令（Ubuntu / Debian，需要 systemd，能访问本面板地址）：
            {available.length > 1 && (
              <Select
                className="enroll-token-select"
                value={token?.id}
                onChange={setChosen}
                options={available.map((t) => ({
                  value: t.id,
                  label: `${t.name}${t.group ? ` · 分组 ${t.group}` : ""}`,
                }))}
              />
            )}
            {token ? <CopyCommand command={token.command} /> : <Alert type="warning" message="没有可用的安装密钥" />}
          </li>
          <li>命令会安装运行依赖和运行层，用这把密钥登记本机，然后启动 rigdeck-agent。矿机主动连接面板，不需要开放任何端口，也不需要在这里填写 SSH 密码。</li>
          <li>几秒后矿机出现在列表里，名称取主机名；之后照常应用飞行表。同一台机器重装系统后再执行，会回到原来的记录（按主板和网卡识别）。</li>
        </ol>
        {joined.length > 0 && (
          <Alert
            type="success"
            showIcon
            message={`已加入 ${joined.length} 台`}
            description={
              <Space wrap>
                {joined.map((m) => (
                  <Button key={m.id} size="small" onClick={() => navigate(`/machines/${m.id}`)}>
                    {m.name}
                  </Button>
                ))}
              </Space>
            }
          />
        )}
        <p className="muted">
          安装密钥只能用来新增矿机，不能控制已有矿机。泄露后到 <Link to="/settings?tab=install" onClick={onClose}>设定 → 安装密钥</Link> 撤销并换一把。
        </p>
      </QueryState>
    </Modal>
  );
}

/** 设定 → 安装密钥: create, copy and revoke install keys. */
export function InstallKeys() {
  const tokens = useEnrollmentTokens();
  const queryClient = useQueryClient();
  const { message } = App.useApp();
  const [form] = Form.useForm();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const reload = () => queryClient.invalidateQueries({ queryKey: ["enrollment-tokens"] });
  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" icon={<PlusOutlined />} onClick={() => setOpen(true)}>
          新建安装密钥
        </Button>
      </Space>
      <QueryState loading={tokens.isLoading} error={tokens.error}>
        <Table
          rowKey="id"
          dataSource={tokens.data}
          pagination={false}
          expandable={{
            rowExpandable: usable,
            expandedRowRender: (token) => <CopyCommand command={token.command} />,
          }}
          columns={[
            { title: "名称", dataIndex: "name" },
            { title: "新机器分组", dataIndex: "group", render: (v: string, t) => [v, ...t.tags].filter(Boolean).join(" · ") || "—" },
            {
              title: "已用 / 上限",
              key: "uses",
              render: (_, t) => `${t.uses} / ${t.max_uses ?? "不限"}`,
            },
            {
              title: "过期时间",
              dataIndex: "expires_at",
              render: (v?: string | null) => (v ? new Date(v).toLocaleString() : "永不"),
            },
            { title: "状态", key: "state", render: (_, t) => tokenState(t) },
            {
              title: "操作",
              key: "action",
              render: (_, t) =>
                !t.revoked_at && (
                  <Popconfirm
                    title="撤销后，用这把密钥的安装命令将无法再加入矿机；已加入的矿机不受影响。"
                    okText="撤销"
                    cancelText="取消"
                    onConfirm={async () => {
                      try {
                        await unwrap(client.DELETE("/api/v1/enrollment-tokens/{id}", { params: { path: { id: t.id } } }));
                        message.success("已撤销");
                        reload();
                      } catch (e) {
                        message.error((e as Error).message);
                      }
                    }}
                  >
                    <Button size="small" danger>
                      撤销
                    </Button>
                  </Popconfirm>
                ),
            },
          ]}
        />
      </QueryState>
      <Modal
        open={open}
        title="新建安装密钥"
        onCancel={() => setOpen(false)}
        onOk={() => form.submit()}
        confirmLoading={busy}
        destroyOnHidden
      >
        <Form
          form={form}
          layout="vertical"
          initialValues={{ name: "", group: "", tags_text: "" }}
          onFinish={async (v) => {
            setBusy(true);
            try {
              const body: EnrollmentTokenInput = {
                name: v.name,
                group: v.group ?? "",
                tags: (v.tags_text ?? "").split(",").map((s: string) => s.trim()).filter(Boolean),
                expires_in_hours: v.expires_in_hours ?? null,
                max_uses: v.max_uses ?? null,
              };
              await unwrap(client.POST("/api/v1/enrollment-tokens", { body }));
              message.success("已创建，展开该行即可复制安装命令");
              setOpen(false);
              reload();
            } catch (e) {
              message.error((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="例如：二号矿场 10 月批次" />
          </Form.Item>
          <Form.Item name="group" label="新机器的分组（可选）">
            <Input />
          </Form.Item>
          <Form.Item name="tags_text" label="新机器的标签（英文逗号分隔，可选）">
            <Input />
          </Form.Item>
          <Space size="large">
            <Form.Item name="expires_in_hours" label="有效期（小时，留空永久）">
              <InputNumber min={1} max={87600} />
            </Form.Item>
            <Form.Item name="max_uses" label="最多加入台数（留空不限）">
              <InputNumber min={1} max={100000} />
            </Form.Item>
          </Space>
        </Form>
      </Modal>
    </>
  );
}
