# 主动采集审查与部署记录（2026-10-18）

主控及 7702-01 至 7702-08 已部署运行层 0.4.0。数据库迁移 0010 成功。

## 审查修复

- 指标事务成功提交后才确认推送、推进序号/增量基准和新鲜度；数据库失败不会消耗有效序号。按机器独立串行提交，避免同机并发乱序。
- 无效或过期的新开机快照不会污染已有会话；开机周期切换重置事件确认，保留旧周期的防重放记录。
- 200 和 409 回复使用绑定请求签名、HTTP 状态码、原始回复内容的 HMAC；矿机校验后才采用策略、epoch 和确认序号。
- HTTP 与软件/硬件清单采集放入独立发送线程，待发队列最多保留一份最新快照，避免阻塞本地看门狗。
- 修改地址/密钥重置发送状态及退避；控制器只在目标摘要匹配且样本本身新鲜时跳过 SSH。
- 固定 Rust 1.91.1，与 CI 一致。
- 修复部署输入的 SQL 换行差异：保留生产历史迁移的原始字节，按生产迁移账本逐一核对 SHA-384，在停止旧服务之前运行新二进制的迁移预检。未修改生产迁移账本的历史校验值。

## 验证

- Python 运行层测试 85 项通过；新增推送相关测试共 9 项。
- Rust 工作区普通测试、Clippy（警告视为错误）、模块边界检查通过。
- 在主控的隔离 PostgreSQL 18.6 测试库运行 telemetry_push 集成测试通过：包括注入写入失败后的同请求重试、拒绝过期开机周期、增量还原、事件确认、策略回复、正常推送时跳过 SSH。测试程序在 Ubuntu 22.04 WSL 编译，运行于 Ubuntu 22.04 主控。
- Windows 与生产主控 Linux 的前端构建通过。
- 首次切换因 SQL 换行校验失败自动回退，旧服务恢复；修复后第二次切换成功。公网健康检查及浏览器登录页正常。
- 1 号主动上报故障演练：临时将推送地址指向不可达端口 55 秒，自动回退 SSH；恢复地址后重新推送。矿工始终运行。
- 最终八台均为 push / 0.4.0，观测延迟约 0–10 秒；前后矿工 PID、启动标识、目标状态、飞行表版本完全一致。八台 watchdog 均 active，2 号 thermal-guard 和 fanb-full 均 active。

没有进行千台规模压力测试；实时指标仍采用“失败后发送下一份最新样本”的策略，不承诺保存网络故障期间的每一条样本。事件另行确认重发。

## 生产配置与证据

- 监听 `10.168.2.112:8091`，推送地址 `http://10.168.2.112:8091/ingest/v1/push`；配置在 `/etc/systemd/system/rigdeck.service.d/telemetry-push.conf`。
- 发布标识：`27538eace1ba0d6cffe35acd176acc239673d537+reviewed-push040-20261018`，代表该基线上的本地改动，不是新 Git commit。
- 运行层 SHA-256：`7a04db3627e71272c9540ef9b9fd86afae4681fe2eeab89a636d0cf4fe98537a`。
- 主控前版本：`/home/avalon/rigdeck-rollback-20261018-push`。
- 数据库及配置备份：`/home/avalon/rigdeck-backup-20261018-push`（root 专用目录；备份目录校验通过，未做完整数据库恢复演练）。
- 前后核对：`/home/avalon/release-audit-push-before.json`、`/home/avalon/push-audit-after.json`。
- 回退演练：`/home/avalon/push-fallback.log`、`/home/avalon/push-fallback-observations.log`。
- 测试及部署日志：`/home/avalon/push-pg-test.log`、`/home/avalon/push-build-lf.log`、`/home/avalon/push-cutover-final.log`。

回滚主控时保留当前目录，恢复上述前版本目录并移除本次 listener drop-in，再重启 `rigdeck`。迁移 0010 只是指标字段裁剪函数的兼容扩展，无须还原数据库；0.4.0 矿机在旧主控下可继续通过 SSH 被采集。不要停止 `rigdeck-restore`，以免影响其 cgroup 内的矿工。
