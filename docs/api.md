# REST 与 MCP

## 认证

浏览器调用 `POST /api/v1/login`，JSON 为 `username`、`password`。响应包含 actor、csrf、expires_at，并设置 HttpOnly Cookie。后续写操作发送 `X-CSRF-Token`；有 Origin 时须匹配配置。`POST /api/v1/logout` 注销会话。

设置页创建 AI 令牌，仅创建时显示明文。REST 与 `/mcp` 接受 `Authorization: Bearer ...`；MCP 不接受 Cookie 代替令牌。令牌具有管理员权限，可通过设置页撤销，新的请求立即失效，已打开终端最迟下次 15 秒身份检查断开。已提交持久化任务继续执行，取消需单独提交取消动作。

## REST 入口

| 路径 | 用途 |
|---|---|
| `/api/v1/machines`、`/{id}` | 列表、创建、编辑、删除 |
| `/api/v1/machines/{id}/test` | 只读 SSH 连接测试 |
| `/api/v1/ssh/host-key` | POST `{host, port}`，仅握手获取 SHA256 指纹与算法；不发送登录凭据、不自动信任，15 秒超时 |
| `/api/v1/catalog/{kind}`、`/{id}` | `coins`、`wallets`、`pools`、`miners` 目录 |
| `/api/v1/adapters` | 版本化矿工表单与能力 |
| `/api/v1/packages` | 旧接口：上传安装包到主控。矿机现在按飞行表里的链接自己下载，界面不再使用 |
| `/api/v1/flight-sheets`、`/{id}` | 飞行表编辑 |
| `/api/v1/jobs`、`/{id}`、`/{id}/cancel` | 创建、查询、取消批量任务 |
| `/api/v1/job-targets/{id}/resolve` | 重新对账，或记录人工核实结果 |
| `/api/v1/telemetry` | 最近样本，可按 machine_id 筛选 |
| `/api/v1/machines/{id}/history` | kind、hours 历史查询 |
| `/api/v1/machines/{id}/bmc/{kind}` | power、sensors、events |
| `/api/v1/audit`、`/tokens` | 操作记录、令牌管理 |
| `/api/v1/openapi.json` | 运行时生成 OpenAPI 3.1 |

准确参数以项目根目录 `openapi.json` 为准。生成命令 `cargo run -q -p rigdeck -- openapi > openapi.json`，随后 `npm --prefix frontend run types`。CI 校验重新生成后文件无变化。

创建自定义钱包示例：

```json
{"name":"自定义 CPU 钱包","data":{"coin_symbol":"MYCOIN","address":"pool-account-or-public-address"}}
```

提交命令任务示例（将 machine_ids 替换为机器列表返回的实际 ID，每次新的操作生成新的幂等键）：

```json
{
  "machine_ids": ["11111111-1111-4111-8111-111111111111"],
  "idempotency_key": "unique-operation-key",
  "concurrency": 4,
  "canary": true,
  "include_controller": false,
  "action": {"kind":"command","script":"uname -a","timeout_seconds":30}
}
```

返回 `{"job_id":"..."}`。job targets 含机器名、operation_id、status、output、output_truncated、result、error。命令退出码在远端 result 中，成功返回不代表挖矿成功。

动作类型为 bootstrap、command、apply、miner、power、adopt；miner.operation 为 start/stop/restart，power.operation 为 on/shutdown/reboot/force_off/force_restart。优雅动作不支持时不自动升级为强制动作。adopt 需要单机 PID、启动身份和明确的 start_script/stop_script/pid_script。

任务状态 queued/running/succeeded/failed/cancelled/unknown，目标还可出现 reconciling/blocked。unknown 默认不重试；resolve 的 decision 可以为 reconcile 或人工核实的 succeeded/failed/cancelled，必填 note。reconcile 只重新查询原操作，人工终结会解锁。

## MCP

使用支持 Streamable HTTP 的客户端，URL 为 `https://rigdeck.local/mcp`，提供 Bearer 令牌并信任主控 CA。支持标准 initialize、tools/list、tools/call。工具包括 fleet_list、catalog_list/save、flight_sheets_list、flight_sheet_save、job_submit/get/cancel、telemetry_latest、bmc_read、adapters_list；实际工具名及 JSON Schema 以 tools/list 为准。

REST 与 MCP 共用应用服务、事务、幂等键和审计。MCP 的 job_submit 接受同样的 JobInput；不能绕过未知结果的操作锁。

## 终端协议

WebSocket `/api/v1/machines/{id}/terminal?csrf=<当前会话CSRF>`，可选 `instance` 连接既有 screen。二进制帧为终端字节，文本帧 `{"type":"resize","cols":120,"rows":32}` 调整 PTY；服务器文本错误帧为 `{"error":"..."}`。Origin 必须匹配。终端仅对在线连接发送按键，不缓存或重播输入。

错误语义：401 未登录，403 权限/CSRF/Origin 不符，404 不存在，409 版本或幂等冲突，400 校验失败，503 后端或设备不可用。任务开始后设备失败通过任务状态返回，不能只检查提交 HTTP 状态。
