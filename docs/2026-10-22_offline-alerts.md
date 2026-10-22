# 2026-10-22 矿机离线检测与通知

数据库新增迁移 `0011_machine_status.sql`。运行层不用升级：判定都在主控上做，矿机照旧上报。

## 三种状态

| 状态 | 判定 | 通知 |
|---|---|---|
| 在线 | 最近 60 秒内收到过正常的遥测（主动上报或 SSH 读取） | — |
| 本地离线 | 超过「本地离线判定」分钟数（默认 2）没有任何正常遥测：断网、关机、看门狗停了且 SSH 也读不到 | 变为离线时一条，恢复上报时一条 |
| 矿池离线 | 本地在线、矿工进程在跑，但矿池持续「矿池离线判定」分钟数（默认 10）看不到这个 worker，或 worker 算力为 0 且 5 分钟内没有提交份额 | 变为矿池离线时一条，矿池重新看到算力时一条 |

- 本地离线优先：矿机离线期间不查矿池，矿池状态保持离线前的结论，不会额外再发一条矿池通知。
- 防抖：本地离线要连续静默满阈值；中间只要来一条新数据就算在线。矿池离线要连续满阈值都是坏读数，期间只要有一次正常读数就重新计时；矿池接口报错、超时不算证据，既不判离线也不判恢复。
- 主控刚启动时，停机期间的静默不算：每台矿机从主控启动起重新计时，避免升级或重启主控后满屏离线通知。
- 状态和通知在同一个事务里写入 `machine_status`、`status_alerts`，主控重启后不会重复发，也不会丢。通知保留 90 天。

## 矿池检测

只支持这四个矿池的公开矿工接口：

| 矿池 | 识别 | 接口 | 判断用的字段 |
|---|---|---|---|
| SupportXMR | `*.supportxmr.com`，XMR | `www.supportxmr.com/api/miner/{钱包}/stats/allWorkers` | 每个 worker 的 `hash`、`lts`（最后份额） |
| Kryptex | `*.kryptex.network` / `*.kryptex.com` | `pool.kryptex.com/{币}/api/v3/miner/workers/{钱包}` | `status`（offline 时算力按 0）、`avg_hashrate_30m`、`last_share` |
| HashVault | `*.hashvault.pro`，XMR | `api.hashvault.pro/v3/monero/wallet/{钱包}/stats?workers=true` | `collectiveWorkers` 的 `hashRate`、`lastShare` |
| Nanopool | `*.nanopool.org` | `api.nanopool.org/v1/{币}/workers/{钱包}` | `hashrate`、`lastShare` |

- 钱包和 worker 名取自这台矿机当前部署的飞行表渲染结果（`deployment_versions` 里展开后的 `CUSTOM_TEMPLATE`、`CUSTOM_PASS`、`CUSTOM_URL` 第一条），和矿工实际登录用的一致。worker 名同时认登录名里 `.` 或 `/` 后面的部分（Kryptex、Nanopool 的写法，例如 `%WAL%.%WORKER_NAME%`）和矿池密码（SupportXMR、HashVault 用密码当 worker 名，`x` 表示没设），不区分大小写。都没有时按整个钱包的算力判断。
- 其他矿池，包括自建代理（如 `mac.icuicu.de:6666`）、`IP:端口`、未知域名、Kryptex 的 `krx…` 用户名，都标为「不支持矿池检测」，不会报矿池离线。
- 同一个钱包在同一矿池只查一次，结果缓存 2 分钟；1000 台矿机用同一个钱包时也是每 2 分钟一个请求。请求超时 20 秒。

## 面板

- 矿机列表顶部多了「矿池离线」卡片（已核对台数、不支持检测台数），点击筛选；状态筛选多了「矿池离线」，矿机名旁有红色「矿池离线」标签，悬停显示原因；导出 CSV 的状态列同样区分。
- 设定 → 矿场 → 离线通知：本地离线判定（1–1440 分钟）、矿池检测开关、矿池离线判定（5–1440 分钟）、网页弹窗开关。
- 网页弹窗：控制台打开时每 10 秒查一次新通知，在右下角弹出。离线通知不自动消失，恢复通知 8 秒后消失，点击跳到该矿机。浏览器记住看过的最后一条（`localStorage`），重新打开时补弹期间的新通知，一次最多 5 条，其余合并成一条提示。

## 以后接 App 原生通知

`GET /api/v1/alerts?after=<上次的 id>` 返回之后的新通知（从旧到新），带令牌即可调用，和网页弹窗用的是同一份数据。App 自己记住最后一个 id，前台轮询或后台定时拉取后用系统通知展示即可，主控这边不用改。要做服务端推送（APNs / FCM）时，在 `App::run_status` 保存通知成功后加一个发送端口即可。

## 验证

- Rust 单元测试 `status::tests`：本地离线阈值与只通知一次、恢复、迟到的旧样本不算恢复、主控重启后重新计时、矿池离线防抖与中途 API 报错、单次正常读数重置、不支持的矿池从不通知、本地离线冻结矿池结论、worker 名匹配（登录名后缀、密码、大小写、份额时间窗、钱包总算力）、部署解析。`pools::tests`：四个矿池的识别和反例（自建代理、IP:端口、仿冒域名、非 XMR）、钱包解析、四种返回格式（按 2026-10-22 实际返回裁剪）。`settings::tests`：旧设定缺 `alerts` 时电价保留、阈值范围。
- `pools::tests::live_pool_apis_parse`（`--ignored`，需联网）：对四个真实接口各请求一次，全部解析成功。
- PostgreSQL 集成测试 `machine_status`：状态往返、未知机器的通知被跳过、`after`/`limit` 顺序、当前部署取最新一次；起真实的 `run_status`，用假矿池接口验证一次矿池离线通知、一次本地恢复通知、自建代理标为不支持、第二轮不重复通知、同一钱包只请求一次。其余集成测试（`postgres`、`job_dispatch`、`telemetry_push`、`telemetry_batch`、`snapshot_monitor`、`flight_updates`、`machine_batch`、`resilience`）在新库上通过。环境是 WSL 里的 PostgreSQL 14，与 CI 的 18 不同。
- 端到端：WSL 里启动真实 `rigdeck serve`，添加一台连不上的矿机，2 分钟后 `/api/v1/alerts` 出现一条本地离线通知，已打开的控制台右下角弹出「矿机离线 · rig-test-01」，之后不再重复；设定页离线通知表单读出默认值，保存后 `/api/v1/settings/farm` 返回新阈值。
- 没有在真实矿机和真实矿池账户上验证矿池离线（矿池部分用真实接口的返回格式和假矿池接口的集成测试覆盖）。
