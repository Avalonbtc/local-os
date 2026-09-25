# RigDeck

自托管 Ubuntu 矿机管理面板。React / TypeScript 前端，Rust 模块化后端，PostgreSQL 18 持久化。主控通过 SSH 管理机器，通过 Redfish 或 IPMI 访问 BMC。

矿工由 **GNU screen + 运行脚本 + 本地 watchdog** 管理；systemd 仅负责开机恢复和 watchdog。每个实例有独立 screen 会话中的 `miner` 窗口、目录、API 端口和停止标记。网页关闭、SSH 断开、主控离线不会主动停止矿工。

当前已实现登录、手动或按 IP 段批量添加机器、自定义币种与钱包、HiveOS 式飞行表（矿机自己下载矿工）、HiveOS 同款显卡超频（NVIDIA/AMD，见 [显卡超频](docs/gpu-overclocking.md)）、版本快照、批量命令、切换回退、指标、BMC、真实 PTY 终端、REST 与 MCP。隔离环境验证记录见 [验收报告](docs/validation.md)；真实 EPYC、具体 BMC 和 24 小时稳定性验收仍需接入硬件。

界面已按实际 HiveOS 钱包、飞行表与单机页面重新整理，见 [界面对照与验证记录](docs/ui-hiveos.md)。

## 在一台 Ubuntu 主控上部署（原生，不用 Docker）

主控直接从源码运行：systemd 服务每次启动前自动增量编译（Rust 后端 release + 前端），所以**改完代码只要重启服务**。需要 Ubuntu 22.04 / 24.04 或 Debian 12，主控能访问每台机器的 SSH 和 BMC 网段。推荐独立主控；若使用其中一台矿机，添加时标记“主控”。

### 一键安装

在全新的 Ubuntu 22.04 / 24.04 或 Debian 12 主控上执行（需要 systemd、网络和 sudo）：

```bash
curl -fsSL https://raw.githubusercontent.com/Avalonbtc/local-os/main/install.sh -o /tmp/local-os-install.sh && sudo bash /tmp/local-os-install.sh
```

默认源码目录 `/opt/local-os`，使用调用 sudo 的普通用户运行；直接以 root 执行时创建 `rigdeck` 用户。安装前会让你输入并确认管理员密码（至少 12 字符），用户名为 `admin`。安装已有配置、数据库或目标目录时会停止，不会覆盖；更新现有部署使用下面的 `restart.sh`。

SSH 隧道模式：在命令最后加 `--mode tunnel`。已有 Cloudflare Tunnel：加 `--mode cloudflared --host panel.example.com`，自行把域名转发到 `http://127.0.0.1:18082`；脚本不会创建 Cloudflare 隧道。可用 `--user 用户 --dir /安装目录` 自定义位置。

默认 HTTPS 使用 Caddy 内部 CA，需要让访问设备解析 `rigdeck.local` 到主控 IP，并信任下文的根证书。GitHub 仓库必须可访问；首次源码编译需要数分钟。仅提供原生部署，历史文档中的容器验收记录不再是安装步骤。

### 手动安装

克隆仓库后，在源码目录（须属于普通用户）执行：

```bash
sudo bash scripts/native/install.sh --host rigdeck.local
read -rsp '设置管理员密码（至少 12 字符）: ' P; printf '\n'
RIGDECK_ADMIN_PASSWORD="$P" bash scripts/native/rigdeck.sh create-admin admin; unset P
```

安装脚本会装好 PostgreSQL 18、Node.js 22、Rust 1.91.1（装在运行用户的 `~/.cargo`）、ipmitool、Caddy，创建数据库、`/etc/rigdeck/rigdeck.env`、主密钥 `/etc/rigdeck/master.key` 和 `rigdeck.service`，然后编译并启动。重复运行不会覆盖已有配置、密钥和数据库。

| 访问方式 | 参数 | 说明 |
|---|---|---|
| HTTPS（默认） | `--mode https --host rigdeck.local [--listen 局域网IP]` | Caddy 内部 CA 监听 80/443（`--listen` 只绑定一个地址），反向代理到 127.0.0.1:8080。把根证书 `/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt` 导入访问设备 |
| 仅 SSH 隧道 | `--mode tunnel` | 只监听主控回环 127.0.0.1:18082；访问电脑执行 `ssh -N -L 127.0.0.1:18081:127.0.0.1:18082 -p SSH端口 用户@主控`，再打开 `http://localhost:18081` |
| cloudflared | `--mode cloudflared --host 面板域名` | 只监听 127.0.0.1:18082，由主机上的 cloudflared 转发，信任其来源 IP 头 |

日常操作：

```bash
bash scripts/native/restart.sh     # 改完代码：先编译，成功后才重启；编译失败时旧进程继续服务
sudo systemctl restart rigdeck     # 也可以直接重启（启动前自动编译，编译失败则服务起不来）
journalctl -u rigdeck -f           # 查看日志（含编译输出）
sudo bash scripts/backup.sh        # 备份数据库、配置和数据目录（不含主密钥）
```

数据库迁移在服务启动时自动执行。主密钥丢失后，数据库里的 SSH/BMC 凭据无法解密，请单独备份 `/etc/rigdeck/master.key`。

## 接入机器并应用飞行表

1. 在“矿机”添加名称、SSH 地址、用户名和凭据。指纹可留空，首次保存会从主控获取并让你确认；也可在目标机控制台用 `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` 核对后填写。后续连接始终校验已保存指纹。认证方式选择“私钥”后可直接选择 OpenSSH / PEM 私钥文件，已加密的私钥还需填写口令；不要选择 `.pub` 公钥，PPK 需先转换。私钥保存到主控时加密存储。非 root SSH 用户若使用密码 sudo，可在机器设置中填写独立的“sudo 密码”；密码与 SSH 登录密码相同时可留空。也支持已配置的免密码 sudo。
2. 测试 SSH，点击“部署运行层”。目标机需有 Python 3；主控自动安装 screen、jq、curl、util-linux 等所需组件，部署版本化脚本和两个 systemd 入口。目标机不需要连接 HiveOS 云端，也不新增管理监听端口。
3. 在钱包页直接输入新币种符号和收款地址/矿池账号，保存后可立即用于飞行表。只填公开收款信息，不需要钱包私钥。
4. 飞行表和 HiveOS 一样：每个任务选币种、钱包，直接填写矿池地址（每行一个，第一行为主矿池），再选挖矿软件。XMRig 和 SRBMiner-MULTI 可直接选官方版本；cpuminer-opt 或镜像地址填写下载链接；Custom 为标准 HiveOS 自定义包。矿机按链接自己下载，主控不保存、不转发安装包。https 链接可不填 SHA256；http 链接必须填写。
5. 在“设定挖矿软件配置”里设置算法、钱包模板、密码、额外参数、线程数和 CPU 绑定。每个实例名称在机器内唯一。
6. 从机器列表勾选目标并应用；默认先一台验证、再并发 4 台。每台机器的结果、命令输出都显示在该机器的“消息”里。模板编辑不会改变已运行版本。

统计不支持的字段显示缺失；配置的算法不冒充矿工实际报告的算法。启动验证失败会回退，结果不明则保留机器操作锁，在该机器“消息”里点“核实与对账”。程序不会按矿工进程名称批量清理未知程序。

## 页面

顶栏：RigDeck · 我的矿场 ▾ · 选择矿机 ▾，右侧为飞行表、刷新和账户。矿场页签：矿机、钱包、飞行表、超频、BMC 设备、BIOS、多机终端、设定。

机器详情（HiveOS 布局）：顶部色条带操作图标（重启矿工、启停矿工、切换飞行表、矿工日志、矿工控制台、SSH 终端、系统电源、BMC 电源、BIOS、部署运行层、设定）；页签为概述、飞行表、统计、活动、超频、矿工控制台、SSH 终端、日志、运行软件、传感器、设定。选择多台机器后可打开分屏终端；同步输入需显式开启并勾选目标，断线不补发输入。

## 开发和验证

需要 Rust 1.91.1+、Node 22+、PostgreSQL 18。版本锁在 `Cargo.lock` 与 `frontend/package-lock.json`。

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python3 tests/check_boundaries.py
cargo run -q -p rigdeck -- openapi > openapi.json
cd frontend
npm ci
npm run types
npm run build
```

本地运行需要 `DATABASE_URL` 和 `RIGDECK_MASTER_KEY`，后者用 `rigdeck generate-key` 生成。仅限环回开发环境可以同时设置 `RIGDECK_BIND=127.0.0.1:8080`、`RIGDECK_ORIGIN=http://127.0.0.1:5173`、`RIGDECK_INSECURE_LOCALHOST=1`；生产环境使用 HTTPS。`npm run dev` 代理 `/api` 和 `/mcp` 到 8080。

- [架构与模块边界](docs/architecture.md)
- [API、MCP 与接口契约](docs/api.md)
- [矿工兼容、运行策略及扩展方法](docs/extensions.md)
- [升级、备份、恢复和故障处理](docs/operations.md)
- [已执行验证与实机验收步骤](docs/validation.md)
- [显卡超频（HiveOS 同款参数）](docs/gpu-overclocking.md)

KVM、系统重装、BIOS/固件升级、按收益切币不包含在本版本中。项目为独立实现，不依赖 HiveOS 云端或其私有组件；兼容能力见扩展文档。
