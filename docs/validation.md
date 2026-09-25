# 验收记录

> 历史验证记录。当前安装仅使用 [原生安装入口](../README.md#一键安装)，下文 Docker 部署结果不代表当前安装方式，相关生产部署脚本已移除。

日期：2026-09-23（Asia/Shanghai）。以下记录区分隔离环境验证和真实服务器验收，不把合成统计当作实际挖矿能力。

## 已执行

| 范围 | 环境与结果 |
|---|---|
| Rust | Cargo workspace 构建、fmt、Clippy `-D warnings`、3 个单元测试通过 |
| 模块边界 | 后端依赖、API 只能调用应用层、前端跨功能导入检查通过 |
| PostgreSQL | PG 18 实库集成测试通过：重复迁移、外键、并发自定义币种、版本冲突、幂等键、先行验证、并发领取、租约过期、原操作对账、未知锁、并发完成汇总、取消、聚合、历史输出与审计保留 |
| 本地运行层 | WSL Ubuntu 22.04、真实 GNU screen、mount namespace；12 项测试通过，含停止语义、崩溃恢复、恢复预算、失败回退、启动恢复意图、远端幂等/取消、自定义包两实例隔离、统计单位和进程身份检查 |
| 8 节点流程 | 8 个 Ubuntu 24.04 SSH 容器，逐台核实 SSH 指纹；实际 API → PG → SSH → screen 路径完成批量命令、1 台验证后并发 4 台应用、并发 8 台停止；重复 API 返回原任务 |
| BMC | 本地 HTTPS Redfish 模拟器通过 CA 校验、错误凭据拒绝、传感器分页/缺失值、事件、Reset action 能力及只发送明确优雅关机的契约测试 |
| 浏览器 | Chrome / Playwright，3 项端到端测试通过：登录、自定义钱包创建币种、功能导航、移动布局、令牌创建/撤销、CSRF/Origin、幂等请求、MCP 实际目录写入、真实 SSH PTY、机器分页、连接原 screen 窗口及关闭控制台后矿工继续运行 |
| 部署 | 多阶段 Docker 生产镜像构建通过；隔离 Compose 的 PG 18、应用、Caddy 启动，可信 CA 的 HTTPS 登录、安全 Cookie、自定义钱包写入通过 |
| 生产前端 | 另一个 Chrome 测试验证 Docker 内构建的静态资源、CSP 下登录与页面导航；浏览器仅为测试 CA 忽略系统信任库，Python 部署测试独立验证证书链 |
| 备份恢复 | 实际 pg_dump custom archive → 新隔离数据库 pg_restore → 钱包记录查询通过，恢复库随后清理；未向恢复库启动控制器 |
| 故障注入 | 停止隔离 PG 容器后 API 写入返回 503；目标机测试矿工仍能崩溃恢复并刷新本地统计；重启 PG 后控制器自动重连 |

机器资源和系统信息来自容器所在的本地 Linux 环境。测试矿工为 HTTP 协议 fixture，报告合成 `rx/0` 算力和份额，既不连接真实矿池，也不挖取收益。自定义包测试使用标准钩子结构与真正独立挂载命名空间，但不证明任意 HiveOS 包都兼容。

机器和部署测试的机器可读记录位于 [evidence](evidence/)。界面截图在 `screenshots/`，LAB 和 offline-test 都是隔离数据。

## 复现

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python3 tests/check_boundaries.py
TEST_DATABASE_URL=postgres://rigdeck:你的测试密码@localhost:5432/rigdeck_test cargo test -p rig-infrastructure --test postgres -- --ignored
python3 tests/redfish/run.py
sudo python3 tests/test_runtime.py
```

PG 集成测试拒绝名称不是 `rigdeck_test` 的数据库。runtime 测试使用临时目录和自己的 screen 会话，需 Linux root/mount namespace 权限。Redfish fixture 的证书与私钥仅用于环回测试，不用于生产。

`tests/dev_server.py` 与 `tests/lab/run.py` 是本地验收工具，使用固定的测试管理员和环回 SSH 端口，不用于生产初始化。先准备 `rigdeck-dev-pg`（PG18、127.0.0.1:55432）、数据库 rigdeck 和 rigdeck_test，再 build 并启动 dev_server；lab/run.py 创建专用 `rigdeck-lab` Compose 项目及 8 节点。测试数据应保留在独立环境。

```bash
python3 tests/dev_server.py --work /tmp/rigdeck-test
python3 tests/lab/run.py --work /tmp/rigdeck-test
RIGDECK_LAB=1 npm --prefix frontend run test:e2e
python3 tests/failure_lab.py --work /tmp/rigdeck-test
(cd frontend && RIGDECK_PROD=1 npx playwright test tests/production.spec.ts)
```

浏览器测试需另行启动 `npm --prefix frontend run dev` 和 Chrome。failure_lab 只操作固定名称、指定镜像和环回端口的测试容器，停止 PG 后在 finally 中恢复它。部署 smoke 使用独立 `rigdeck-smoke`，HTTPS 端口 18443。CI 包含编译、接口生成一致性、边界、PG、Redfish、运行层和浏览器/API 测试；本次没有远程推送或声称 GitHub Actions 已运行。

## 真实硬件仍待验收

没有接入用户的 SSH/BMC 凭据，因此以下不能标为通过：

1. 一台实际双路 EPYC Ubuntu 的自动安装、systemd 开机恢复、NUMA/CPU 绑定、硬件传感器和实际矿工 API。
2. 用户指定的 XMRig、SRBMiner-MULTI、cpuminer-opt 版本及 HiveOS 自定义安装包，对真实矿池的算法、版本、正算力和有效份额。
3. 具体 BMC 厂商的 Redfish/IPMI 电源、温度、风扇、功率与事件。IPMI 尚未连接真实硬件；OEM 私有路径需按设备扩展。
4. 实机断网、主控关机、机器重启、切换中断及回退；容器没有以 systemd 为 PID 1，不能替代实际开机验收。
5. 单机验证后扩展到 8 台物理服务器，再用 `scripts/soak.py` 记录 24 小时。还需核对矿池侧有效份额与 BMC 温度/功耗，没有在本次执行中完成 24 小时观察。

实施验收按“单机接入 → 标准包 → 自定义包 → 切换回退 → BMC → 8 台并行 → 24 小时”推进。主控的持续资源容量、硬盘保留占用及路由器部署能力未做实机基准，不把当前电脑的测试结果外推到低配设备。
