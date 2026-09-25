# 矿工运行与扩展

## 控制语义

```bash
sudo rig-miner status
sudo rig-miner start cpu1
sudo rig-miner stop cpu1
sudo rig-miner restart cpu1
sudo rig-miner log cpu1
sudo rig-miner attach cpu1
```

start 用已部署版本启动；stop 持久化停止意图，watchdog 和开机恢复均不重新启动它。attach 进入真实 screen 窗口，退出连接用 screen 的 Ctrl+A、D；Ctrl+C 会送给矿工。status 读取实例状态；详细实时样本通过 collect mining 或控制台查看。

本地路径为 `/opt/rigdeck/runtime/<hash>`、`/opt/rigdeck/current`、`/var/lib/rigdeck/{cache,instances,operations}`。配置、意图和操作记录原子落盘；实例配置目录和脚本版本可供回退。每机写操作串行。

机器策略 JSON 支持 `max_restarts`（默认 5）、`restart_window_seconds`、`cooldown_seconds`（默认 30）、`failure_seconds`（默认 120）、`min_hashrate_hs`（默认 0.01）、`allow_host_reboot`（默认 false）、`host_reboot_cooldown_seconds`（默认 3600）。飞行表任务支持 warmup_seconds（默认 60）、verification_seconds（默认 300）。以 runtime 的 budget/watchdog 实现为准，切换与人工停止期间抑制恢复。

## 原生适配器

| 标识 | 配置生成 | 统计来源 |
|---|---|---|
| xmrig | 矿池、钱包、算法、线程、环回 HTTP API | `/2/summary` |
| srbminer | 算法、矿池、钱包、API 端口 | HTTP 根路径，algorithms[].hashrate.now 等 |
| cpuminer-opt | 算法、URL、钱包、密码、api-bind | 本机 TCP summary API |
| hive-custom | CUSTOM_* 环境、HiveOS 标准钩子 | source h-stats.sh 的 khs、stats |

原生适配器不内置挖矿二进制；用户选择版本并固定安装包 SHA256。新版本的参数/API 变化需跑适配器契约测试和单机验证。配置中的算法不是实际统计值；API 不报告算法时显示未知，配置了预期算法的部署不会把未知判定为匹配。

统计归一为 H/s，Hive 自定义 khs 按 KH/s 转换，hs 数组按 hs_units 转换。算力与有效份额分别保存；连接字段缺失时，启动验证需看到大于零的 accepted。该判断不能证明矿池账本已入账，仍需实机核对矿池。

## 接入 HiveOS 自定义包

包必须是 tar/tar.gz 等 tarfile 支持的格式，内有一个矿工目录，例如 `myminer/h-manifest.conf`、`h-config.sh`、`h-run.sh`、`h-stats.sh`，可选 `h-stop.sh`。压缩包上限 512 MiB，解包上限 8 GiB/50000 项；当前拒绝链接、设备和越界路径，请提供普通文件包。

每实例用私有挂载命名空间把自身工作树映射至 `/hive`、日志映射至 `/var/log/miner`，配置、运行、统计进入同一环境。支持 source 钩子、函数局部变量、manifest 环境、mkfile_from_symlink、miner_ver、message。CPU 路径的 GPU_COUNT=0、gpu-detect 返回空数组，未实现 HiveOS GPU 驱动/超频/私有 GPU 辅助库。

矿工包必须支持实例独立 API 端口，在能力配置中明确 `instance_api_port: true`。传入 `CUSTOM_API_PORT`、`MINER_API_PORT`、`API_PORT`，用户配置中的 `%API_PORT%` 也会替换。硬编码全局 API 端口的包须调整后接入；仅有挂载命名空间不能隔离 TCP 端口。

能力声明还可包含 `architectures:["x86_64"]`、`cpu_flags:["avx2"]`、`commands:["numactl"]`，运行前检查。未声明的包依赖可能在配置钩子或启动阶段报错；项目不宣称自动识别所有私有依赖。

钱包/Worker 模板支持 `%WAL%`、`%WORKER_NAME%`、`%COIN%`、`%URL%`、`%URL_HOST%`、`%URL_PORT%`、`%POOL%`。这些值用于配置和参数展开，不由 shell 二次解释。自定义钩子本身是管理员提供的可执行代码。

## 新增矿工示例

通常通过自定义包即可，无需更改 Rust。`examples/custom-xmrig/` 提供真实 XMRig 标准钩子示例；把官方 XMRig 二进制放进目录、打包、上传，在矿工页声明自定义包及可变 API 端口。示例从本机 API 读取真实统计，不返回固定测试算力。

需要原生适配器时，实现 `domain::MinerAdapter` 的 describe/render，在 `infrastructure::adapters::registry()` 注册。参数描述含 schema_version 和 JSON Schema，前端动态生成表单。统计解析目前位于 Python 运行层 normalize，新增适配器需同步添加采集/归一化分支与契约测试；它不涉及任务队列的条件分支。Rust 原生适配器和目标机统计插件分属两个执行环境，不能只增加 render 而遗漏运行层解析。

## 新增 BMC Provider 示例

例如给一个兼容标准 Redfish 的品牌注册独立标识：定义 `struct VendorBmc;`，实现 `BmcProvider`，id 返回新标识，read/power 委托 `bmc::Redfish`；在 registry 注册 `Arc::new(VendorBmc)`。如果厂商在 OEM 路径提供功耗，仅在这个 Provider 中扩展 read("sensors") 并归一化 name/reading/unit/health，保留原始数据。加入 `tests/redfish/run.py` 类似的 TLS fixture 验证路径与重置语义，然后接真实 BMC。

通用 Redfish 要求一个 ComputerSystem，支持 Sensors 集合、传统 Thermal/Power、ComputerSystem.LogServices；不自动探测所有 OEM 路径。IPMI 是显式选择的 lanplus Provider，不会在 Redfish 电源操作结果不明时自动改用 IPMI 重发。多系统机箱、Manager 专属事件和厂商私有传感器需扩展 Provider。

## 新页面与新任务类型

页面示例：在 `features/inventory/index.tsx` 导出 `InventoryPage`，调用 shared/api 的 useMachines/useTelemetry，使用 QueryState 包裹 Table，然后在路由和导航注册。只能从其他 feature 的公开 index 导入。若需要新数据，在应用服务增加方法、HTTP/OpenAPI 注册、生成 TS 客户端后调用，不能直接在前端定义第二套请求模型。

任务示例：新增“采集诊断包”时，为 domain::Action 添加结构化参数，application/jobs 校验大小与机器范围，runtime operation_run 添加幂等处理、取消检查和持久化结果。远端文件以 operation_id 命名避免重试覆盖；REST 与 MCP 使用同一个 JobInput 自动获得类型。添加失联对账、取消、重复请求测试，调度器仍不认识具体矿工和 BMC 品牌。

## 实现依据

实现行为参照 [HiveOS miner 控制源码](https://github.com/minershive/hiveos-linux/blob/72cae73d1f2788b999df30773091cad72e068de7/hive/bin/miner)、[miner-run](https://github.com/minershive/hiveos-linux/blob/72cae73d1f2788b999df30773091cad72e068de7/hive/bin/miner-run) 和 [官方自定义包规范](https://github.com/minershive/hiveos-linux/blob/master/hive/miners/custom/README.md)。原生统计参考 [SRBMiner 参数](https://github.com/doktor83/SRBMiner-Multi/blob/master/Parameters) 与 [cpuminer-opt API](https://github.com/JayDDee/cpuminer-opt/blob/master/api.c)。这是独立兼容实现，未安装 HiveOS 系统。
