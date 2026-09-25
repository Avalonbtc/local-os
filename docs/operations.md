# 部署、升级与恢复

## 升级

主控以原生方式运行（`rigdeck.service`，源码目录直接启动）。更新前先备份，记录当前源码版本（`git rev-parse HEAD`）。SQLx 在启动时执行尚未应用的迁移；已应用的迁移不可修改，schema 变更新增编号文件。没有降级脚本。

```bash
sudo bash scripts/backup.sh
git pull                              # 或者复制新的源码
bash scripts/native/restart.sh        # 编译成功后才重启
journalctl -u rigdeck -n 100
```

数据库迁移通常不能靠回退代码撤销。有破坏性变更时，先把备份恢复到隔离库验证，再升级；失败时退回旧源码并恢复对应的数据库备份。恢复数据库前先停止主控（`sudo systemctl stop rigdeck`），避免旧任务或旧凭据状态重新驱动设备。

## 三部分备份

```bash
sudo bash scripts/backup.sh
sudo bash scripts/verify-backup.sh backups/本次目录/database.dump
```

备份目录包含：PG custom dump、配置归档（`/etc/rigdeck/rigdeck.env`，含数据库密码；以及 systemd 单元和 Caddyfile）、数据目录归档（`/var/lib/rigdeck-controller`：BIOS 快照和操作记录、旧的软件包缓存）和 SHA256SUMS。默认目录名带 UTC 时间，请保存在可信磁盘。验证脚本会创建独立的 `rigdeck_restore_*` 数据库，完整执行 pg_restore，查询机器、任务和审计数量，最后删除临时库；不会启动主控去执行恢复库里的任务。

`/etc/rigdeck/master.key` 必须单独保管并确认可读；备份包故意不包含它。请单独加密保存，并记录它与各数据库备份的对应关系。恢复验证不代表远端矿工的运行状态也已回退，恢复主控后仍要对账。

## 灾难恢复

1. 在隔离主机上放好同版本源码，运行 `sudo bash scripts/native/install.sh --no-start`，再恢复 `/etc/rigdeck/rigdeck.env` 和主密钥。暂时断开该主控到机器管理网的连接。
2. 清空数据库后恢复：`sudo -u postgres pg_restore --exit-on-error --no-owner --role=rigdeck -d rigdeck database.dump`。先用 verify-backup 验证备份，并核对重要记录的数量。
3. 把 data.tar 解到 `/var/lib/rigdeck-controller`，属主改为服务用户。运行 `bash scripts/native/rigdeck.sh migrate` 检查 schema。
4. 检查所有 queued、running、reconciling、unknown 状态的任务。租约过期的任务通过原操作 ID 对账，不要手工删除机器锁。确认不会从旧备份重放尚未执行的排队任务后，恢复主控网络，执行 `sudo systemctl start rigdeck`。
5. 逐台核对 screen、实例版本、目标状态、实际算法、算力和份额，然后再执行新的批量操作。

主密钥轮换目前没有自动命令。不能只替换 master.key 就继续使用原来的密文；应在维护窗口用原密钥解密并重新加密，或重新录入每台机器的凭据。

## 常见故障

| 现象 | 操作 |
|---|---|
| SSH host key mismatch | 从物理/BMC 控制台核实新指纹，再编辑机器；不关闭指纹检查 |
| sudo 不可用 | 在机器编辑页填写正确的 sudo 密码，或给专用管理账户配置免密码 sudo；保存后用“测试连接”验证 |
| apt 软件源超时 | 基础部署仅在缺少 screen 时安装软件包，并限制 apt 等待时间；HiveOS 自定义包缺少 jq 等依赖时按具体缺项提示，修复软件源后再安装 |
| 自定义包 mount/unshare 失败 | 检查目标机 mount namespace 权限；普通容器不具备所需权限 |
| 部署 unknown | 检查任务 operation_id 与 `sudo rig-miner operation-status <ID>`，在页面重新对账 |
| 矿工 faulted | 查看重启预算及原因；修复配置/矿池后显式启动或重启 |
| BMC TLS 失败 | 上传该 BMC 的可信 CA，或安装有效证书，不忽略校验 |
| Redfish 不支持优雅重启 | 使用受支持的操作，或明确选择强制重启；不自动升级动作 |
| PG 不可用 | `systemctl status postgresql`；修复 PG 和磁盘，主控暂不接受新写操作；本地 watchdog 不受影响 |
| 重启后服务起不来 | `journalctl -u rigdeck -n 200` 查看编译或启动错误；修好代码后 `bash scripts/native/restart.sh` |
| 统计过期 | 区分主控采集失败、矿工 API 失败和真实算力为零；面板不补假数据 |

任务输出每目标最多 1 MiB，达到上限标记截断。目标机 screen/custom `*.log` 超过 32 MiB 时保留约 8 MiB 尾部并加截断标记；软件包、配置版本及远端操作记录不自动删除，避免破坏回退与对账。需要定期关注这些目录空间，清理前确认不被运行版本和未完成操作引用。

主控被选入 BMC 关机/重启时默认排除，显式包含才最后执行。管理员自行提交的任意 shell 脚本不能可靠判断是否含关机命令，批量命令目标需自行确认。

## 24 小时观察

`scripts/soak.py` 只查询 API，不切换矿工或发送电源动作。设置令牌环境变量后运行：

```bash
export RIGDECK_TOKEN='设置页创建的令牌'
python3 scripts/soak.py --url https://rigdeck.local --ca rigdeck-root.crt --hours 24 --output soak.jsonl
unset RIGDECK_TOKEN
```

记录每分钟各机器的连接、样本时间、实例状态、算法、算力、份额和错误。退出码非零表示观察到 API 故障、系统离线或期望运行实例无有效新鲜统计；记录中的数据来自真实 API，不生成测试算力。完成后结合矿池有效份额、BMC 温度/功耗和重启次数判断稳定性。
# Runtime dependency installation

Bootstrap and flight-sheet application check `screen`, `python3`, `bash`, `unshare`, `mount`, `jq`, and `curl`. A complete host skips apt entirely. Missing tools are installed from the host's configured Ubuntu repositories before changing running miners. Package-provided arbitrary dependency names are not installed automatically.

Repository refresh has a 90-second deadline; installation has a 120-second deadline. Each command has a 10-second termination grace period, and apt lock waits are capped at 30 seconds. Failure stops deployment and includes installer output in the task error. Correct repository/network or package-manager errors before retrying. If a forced timeout interrupts dpkg, inspect its reported state and repair it before retrying; do not delete package-manager lock files.
