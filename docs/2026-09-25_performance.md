# 2026-09-25 性能优化

运行层版本升到 `0.3.1`，数据库新增迁移 `0007_metrics_skip_gpu_oc.sql`。

| 范围 | 之前 | 现在 |
|---|---|---|
| 超频信息入库 | 每 10 秒的 system 样本连同 `gpu_oc`（模板、每卡结果）和 `gpu_drivers` 写入 `metrics` 和分钟汇总，8 卡矿机每台每天多约 10 MB | 只保存在 `latest_observations`，历史表不再重复保存 |
| NVIDIA 读数 | 缓存 9 秒、看门狗 10 秒一轮，实际每轮启动一次 `nvidia-smi`（多卡时零点几秒 CPU） | 看门狗进程内常驻 NVML（`libnvidia-ml.so.1`）直接读取；没有可用 NVML 时退回 `nvidia-smi` |
| 矿机频繁写入的缓存 | 每个实例的 `stats.json`、`cpu-sample.json`、`power-sample.json` 每 10 秒写 `/var/lib/rigdeck`（磁盘 / U 盘） | 移到内存盘 `/run/rigdeck/cache`；这些样本本来就按开机 ID 判断是否有效，重启丢失无影响 |
| 前端首屏脚本 | 所有页面、ECharts 全量包、xterm 都在首屏下载：约 2.9 MB（gzip 约 890 KB） | 矿机列表以外的页面按需加载，终端只在打开终端页签时下载，ECharts 只引入折线图并在首次画图时加载：首屏约 1.36 MB（gzip 约 430 KB）；图表块 1.1 MB → 495 KB |

## 升级

1. 更新主控（迁移 0007 启动时自动执行）。
2. 在矿机上重新“部署运行层”到 0.3.1。旧的 `/var/lib/rigdeck/instances/*/stats.json` 不再使用，可以删除，不删也不影响。

## 验证

- `python3 tests/test_gpu_oc.py`（新增 NVML 读数与 `nvidia-smi` 回退测试）、`tests/test_runtime_snapshot.py`、`tests/test_power.py`、`tests/test_runtime.py`（21 项，真实 screen）。
- PostgreSQL 16 上的 `postgres`、`telemetry_batch`、`snapshot_monitor`、`flight_updates` 集成测试；`metric_values` 已去掉 `gpu_oc` / `gpu_drivers`。
- Playwright：BIOS、超频、混合矿机、手机布局、功耗、单机页全部通过；`console.spec.ts` 需要后端的几项里，“钱包”一项在改动前的代码上同样失败（测试仍按旧表格查找只含币种的单元格），与本次改动无关，其余通过。
