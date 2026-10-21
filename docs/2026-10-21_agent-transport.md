# 2026-10-21 矿机主动连接与一条命令加入

运行层版本升到 `0.5.0`，数据库新增迁移 `0011_agents_without_bmc.sql`。分支：`feature/agent-transport`。

## 改了什么

主控可以放在公网 VPS 上，矿机像 HiveOS 一样自己连上来：

| 项目 | 以前 | 现在 |
|---|---|---|
| 添加矿机 | 在面板填 SSH 地址、用户名、密码或私钥、核对指纹，再点“部署运行层” | 面板“添加矿机”复制一条命令，到矿机上用 root 执行一次，几秒后自动出现在列表 |
| 控制方向 | 主控通过 SSH 连到每台矿机，矿机必须让主控能访问 | 矿机上的 `rigdeck-agent` 主动连主控并保持一条 WebSocket，命令从这条连接下发；矿机不需要公网 IP、不开放端口 |
| 主控保存的东西 | 每台矿机的 SSH 密码 / 私钥、sudo 密码 | 一键加入的矿机不保存任何凭据；机器密钥由主密钥派生 |
| BMC / BIOS | Redfish、IPMI 电源与传感器，Supermicro SUM 改 BIOS | 已删除：公网主控访问不到矿场的管理网 |

已有的 SSH 矿机照常工作，两种机器可以在同一个面板里混用。

## 一条命令加入

```bash
curl -fsSL https://panel.example.com/agent/install.sh | sudo bash -s -- rgk_<安装密钥>
```

安装脚本由主控生成，里面写好了面板地址和运行层的 SHA256：

1. 检查系统（需要 systemd 的 Linux），安装 screen、python3、jq、curl、util-linux 等依赖（和 SSH 部署用的是同一份 `dependencies.sh`）。
2. 下载运行层并核对 SHA256，装到 `/opt/rigdeck/runtime/<hash>`，原子切换 `current`。
3. `rig-miner enroll` 带着安装密钥、硬件标识、主机名和地址登记，主控返回机器 ID、机器密钥和连接地址，写到 `/var/lib/rigdeck/agent.json`（0600）；同时写好统计上报配置。
4. 启用并启动 `rigdeck-restore`、`rigdeck-watchdog`、`rigdeck-agent`。

重复执行是安全的：同一台机器（按主板 UUID 和物理网卡 MAC 识别，重装系统也不变；同一个镜像刷到多台矿机也能区分）回到原来的记录，不会重复添加。机器名取主机名，重名时自动加 `-2`、`-3`。

**安装密钥**在“设定 → 安装密钥”管理，第一次点“添加矿机”时会自动建一把“默认安装密钥”。可以按批次新建：给加入的机器预设分组和标签、设置有效期和最多加入台数，泄露后随时撤销。密钥格式是 `rgk_` + 令牌 ID + 由主密钥派生的 HMAC，所以随时能重新显示命令，数据库里不存密钥本身。安装密钥只能新增机器，不能控制已有机器。

## 连接与安全

- agent 连接时，主控先发随机 nonce，agent 用本机密钥回 `HMAC-SHA256(key, "rigdeck-agent/v1|<机器ID>|<nonce>")`，主控核对后才登记这条连接。同一台机器的新连接会替换旧连接。
- 主控每 20 秒 ping 一次，70 秒没有任何消息就断开；agent 90 秒没收到消息也会重连，断线后按 1、2、4…60 秒退避，被拒绝（机器已删除、密钥不对）时每 5 分钟重试。
- 命令执行和 SSH 一样：`/bin/sh -c`，标准输出最多 2 MiB、错误输出 64 KiB，超时杀掉整个进程组。连接中途断开、命令结果没回来时，按“结果不明”处理，由原有的按操作 ID 对账逻辑核实，不会盲目重发。
- `rigdeck-agent` 和看门狗一样用 `KillMode=process`：经由 agent 启动的 screen 矿工在它的 cgroup 里，重启或升级 agent 不会带走矿工。经由 agent “部署运行层”时，agent 在回复结果后由一个 systemd 定时器重启自己。
- 终端在矿机本地开伪终端（`setsid -c`，有控制终端，screen 和作业控制正常），浏览器终端页和以前一样使用。

运行层自带一个只用标准库的 WebSocket 客户端，矿机不需要 pip，也不需要额外的软件源。

## 部署到公网 VPS

```bash
sudo bash install.sh --mode public --host panel.example.com --email you@example.com
```

域名先解析到 VPS 并开放 80/443。Caddy 自动申请和续期 Let's Encrypt 证书，矿机加入和连接时用它校验主控；Caddy 把真实来源 IP 交给后端，登录限速按真实 IP 计算。矿机通过 `RIGDECK_PUBLIC_URL`（默认等于 `RIGDECK_ORIGIN`）访问主控；统计上报默认也发到这个地址的 `/ingest/v1/push`，不再需要单独的 8091 端口（局域网仍可用 `RIGDECK_INGEST_BIND` 另开）。

局域网 HTTPS 模式用的是 Caddy 内部 CA，矿机不认这张证书；局域网里用一键加入，请改用 `--mode public`，或者把内部根证书导入矿机。

## 删除的内容

BMC 设备页、BIOS 页、单机页的 BMC 电源菜单和传感器页签、批量“BMC 电源操作”、Redfish/IPMI/SUM 适配和测试夹具、`/api/v1/machines/{id}/bmc/*`、`/api/v1/machines/{id}/bios`、MCP 的 `bmc_read`、`bios_read`。迁移 0011 删掉 `machines.bmc`、`machines.bmc_secret` 和最新值里的 BMC 读数；历史任务里的“读取 BIOS”“电源操作”记录仍按原名称显示。单机页“硬件”里的“BIOS 版本”是从系统读出的固件版本，不是 BIOS 管理，保留。

## 升级

1. 先备份（含 `/etc/rigdeck/master.key`），更新主控；迁移 0011 启动时执行。**迁移会删除 BMC 配置和 BMC 密码，无法恢复。**
2. 已有 SSH 矿机不用动，“部署 / 更新运行层”到 0.5.0 即可。想把它们切到主动连接，在矿机上执行一次安装命令：同一台机器会被识别为新机器（旧记录没有硬件标识），确认新记录在线后删除旧的 SSH 记录。
3. 公网部署按上节配置域名和证书。

## 验证

- Rust：`cargo test --workspace`（新增 `agent::tests`：命令、上传、错误、断线后结果不明、终端双向、连接地址）；`clippy -D warnings`、`fmt` 通过。
- PostgreSQL 集成测试（WSL 里的 PostgreSQL 14，CI 用 18）：新增 `enrollment`（密钥派生与重新显示、加入、重装回到原记录、台数上限、重名加后缀、无地址时用来源地址、伪造/格式错误/撤销的密钥、编辑 agent 机器不能改连接信息、连接挑战的 HMAC），以及原有的 `telemetry_push`、`snapshot_monitor`、`telemetry_batch`、`postgres`、`flight_updates`、`machine_batch`、`job_dispatch`、`resilience`，全部在新库上通过。
- Python：新增 `tests/test_agent.py`（测试内置一个 WebSocket 服务端扮演主控：HMAC 挑战、被拒绝、命令与标准输入、2 MiB 输出上限、超时杀进程组、上传与权限、ping/pong、伪终端；登记写入的文件与权限、错误提示；硬件标识对克隆镜像、重装系统、虚拟网卡的行为），以及原有 `test_push.py`、`test_runtime_snapshot.py`。
- 前端：类型检查、构建；Playwright 新增 `enroll.spec.ts`（首次自动建密钥、显示命令、新机器加入后在对话框里出现；安装密钥列表、展开复制、撤销），原有 13 个用例在去掉 BMC 后通过。
- 端到端（WSL，真实 `rigdeck serve` + 真实运行层代码当矿机）：登录、建密钥、安装脚本和运行层的 SHA256 一致且 `bash -n` 通过、伪造密钥被拒、登记、重复登记回到同一台、agent 连上、机器列表显示主动连接、“测试连接”经 agent 执行、浏览器终端经 agent 的伪终端回显、统计上报以 `transport=push` 入库、断开后面板发现并快速报“未连接”、撤销后的密钥被拒。
- 没有在真实矿机上跑过安装脚本本身（它会装系统包和 systemd 服务），也没有在公网 VPS 上验证 Let's Encrypt 流程。
