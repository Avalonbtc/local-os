import { useState } from "react";
import { Alert, Button, Form, Input } from "antd";
import { LockOutlined } from "@ant-design/icons";
import { client, unwrap, setCsrf } from "../../shared/api";
export function Login({ onLogin }: { onLogin: () => void }) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <div className="login">
      <div className="login-intro">
        <div className="brand">
          <span className="hive-brand-mark" aria-hidden="true"><i /><i /><i /></span>
          RigDeck
        </div>
        <div className="login-title">
          你的机器。
          <br />
          <span>你的控制中心。</span>
        </div>
        <p>Ubuntu 服务器 · 飞行表 · 带外管理</p>
        <div className="login-foot">SELF-HOSTED / PRIVATE INFRASTRUCTURE</div>
      </div>
      <div className="login-panel">
        <div className="brand login-mobile-brand">
          <span className="hive-brand-mark" aria-hidden="true"><i /><i /><i /></span>
          RigDeck
        </div>
        <div className="eyebrow">CONTROL PANEL</div>
        <h1>登录控制台</h1>
        <p className="muted">使用主控管理员账户登录</p>
        {error && <Alert type="error" message={error} showIcon />}
        <Form
          layout="vertical"
          onFinish={async (values) => {
            setBusy(true);
            setError("");
            try {
              const s = await unwrap(
                client.POST("/api/v1/login", { body: values }),
              );
              setCsrf(s.csrf);
              onLogin();
            } catch (e) {
              setError((e as Error).message);
            } finally {
              setBusy(false);
            }
          }}
        >
          <Form.Item
            name="username"
            label="用户名"
            rules={[{ required: true }]}
          >
            <Input autoComplete="username" size="large" />
          </Form.Item>
          <Form.Item name="password" label="密码" rules={[{ required: true }]}>
            <Input.Password autoComplete="current-password" size="large" />
          </Form.Item>
          <Button
            type="primary"
            htmlType="submit"
            size="large"
            block
            loading={busy}
            icon={<LockOutlined />}
          >
            登录
          </Button>
        </Form>
        <div className="login-hint">
          首次部署请在主控运行 create-admin 创建管理员。
        </div>
      </div>
    </div>
  );
}
