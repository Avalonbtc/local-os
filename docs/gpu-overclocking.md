# 显卡超频（HiveOS 同款参数）

超频参数与 HiveOS 的 `nvidia-oc.conf` / `amd-oc.conf` 一致：字段名、单位、每卡取值方式都相同，可以把 HiveOS 矿机上的配置直接粘贴导入。执行部分是独立实现，不使用 HiveOS 的 `nvtool`、`amd-oc` 或其云端。

## 入口

* 机器详情 → **超频** 页签：查看每张卡的实时频率/功耗/风扇、编辑并应用、恢复默认、导入/导出 HiveOS 格式、另存为模板。
* 顶部 **超频** 页：超频模板的新建、编辑、删除，勾选多台机器批量应用（先一台验证，再并发执行）。

应用和恢复都是普通作业（`POST /api/v1/jobs`），与 BIOS、矿工操作一样有幂等、单机锁、审计和对账；结果显示在机器“消息”和超频页签的“上次结果”列。

```json
{"kind":"gpu_oc","profile_name":"3070Ti","config":{"nvidia":{"clock":[1500],"mem":[2400],"plimit":[220],"fan":[0]}}}
{"kind":"gpu_oc_reset"}
```

模板存在 `oc_profiles` 表，通过 `/api/v1/catalog/oc-profiles` 读写。作业提交时把完整参数写进作业，之后修改模板不影响已应用的矿机。

## 参数

每个参数是按显卡 PCI 总线顺序排列的列表，空格分隔。列表比显卡少时用最后一个值补齐（只填一个数就是所有卡），留空表示不修改。NVIDIA 与 AMD 各自单独编号。

| NVIDIA | 含义 |
|---|---|
| `CLOCK` | 核心偏移 MHz；**大于 500 表示锁定核心频率**（等同 `nvidia-smi -lgc`） |
| `MEM` | 显存偏移，HiveOS/Linux 数值，是 Windows Afterburner 的 2 倍（Win +1000 = 2000） |
| `PLIMIT` | 功耗墙 W，0 = 驱动默认 |
| `FAN` | 风扇 %，0 = 自动 |
| `RUNNING_DELAY` | 开机后等待多少秒再重新应用（最多 300） |

| AMD | 含义 |
|---|---|
| `CORE_CLOCK` / `CORE_VDDC` / `CORE_STATE` | 核心频率 MHz、电压 mV、DPM 状态（0 = 自动） |
| `MEM_CLOCK` / `MEM_STATE` | 显存频率 MHz、DPM 状态 |
| `PL` / `FAN` | 功耗墙 W（0 = 默认）、风扇 %（0 = 自动） |
| `REF` | 通过 `amdmemtweak --REF` 写入，矿机需自行安装 amdmemtweak |
| `MVDD` `VDDCI` `SOCCLK` `SOCVDDMAX` | 可导入和保存，但需要修改 PowerPlay 表，本版本**不写入**，结果中标为“跳过” |

输入范围只用来拦截输错的数，不是推荐值。

## 矿机上如何生效

运行层（`runtime/rig-runtime.py`，需要重新“部署运行层”到 0.3.0）：

* **NVIDIA**：通过 NVML（`libnvidia-ml.so.1`，ctypes 调用）设置功耗墙、核心/显存偏移（`nvmlDeviceSetGpcClkVfOffset` / `nvmlDeviceSetMemClkVfOffset`）、锁频和风扇，并开启持久模式。不需要 X 服务器。需要 **R520 或更新**的驱动；旧驱动会在结果里报告缺少的函数。
* **AMD**：写 amdgpu 的 sysfs。自动识别三种 OverDrive 格式：Polaris/Vega10（`s <状态> <频率> <电压>`）、Vega20/Navi10（`s 1`、`m 1`、`vc 2`）、RDNA2/3（`s 1`、`m 1`，不支持绝对电压）。需要内核参数 `amdgpu.ppfeaturemask=0xffffffff`，否则报告“未启用 OverDrive”。功耗墙写 `power1_cap`（不超过 `power1_cap_max`），风扇写 `pwm1` / `pwm1_enable`。
* 每张卡、每一步单独执行和报告，一张卡失败不影响其他卡；所有卡都失败时作业为失败。
* 参数保存在 `/var/lib/rigdeck/oc.json`。显卡设置在重启或驱动重载后会丢失，`rigdeck-restore` 开机时在后台重新应用（遵守 `RUNNING_DELAY`），不阻塞矿工恢复。
* “恢复默认”：NVIDIA 偏移清零、解除锁频、默认功耗墙、风扇自动；AMD 写 `r`/`c` 恢复默认频率电压、DPM 自动、风扇自动、默认功耗墙。并删除 `oc.json`。
* 矿机本地命令：`rig-miner oc-status` 查看已保存的参数和上次结果，`rig-miner oc-reapply` 立即重新应用。

## 用 RTX 3070 Ti 实机验证 NVIDIA

AMD 部分只经过模拟 sysfs 的测试（`tests/test_gpu_oc.py`），需要有 AMD 卡后再实机验证。NVIDIA 可以用本地 3070 Ti 按下面步骤验证：

1. 驱动 ≥ 520：`nvidia-smi` 查看 Driver Version。把这台机器加入 RigDeck 并“部署运行层”，确认 `rig-miner check` 显示 `0.3.0`。
2. 记录默认值：`nvidia-smi -q -d CLOCK,POWER | grep -E "Graphics|Memory|Power Limit"`。
3. 先用保守值：超频页签填 `核心 -100`、`显存 1000`、`功耗墙 200`、`风扇 60` 并应用。“上次结果”应全部为绿色。
4. 核对（需要有负载，例如开着矿工）：
   * `nvidia-smi --query-gpu=power.limit,fan.speed --format=csv` → 200 W、60 %。
   * 显存：`nvidia-smi --query-gpu=clocks.mem --format=csv` 在负载下应比默认高约 **500 MHz**（`MEM` 是 HiveOS 数值，nvidia-smi 显示的是一半）。如果高了 1000 MHz，说明显存偏移的换算不对，请停下来反馈。
5. 锁频：`核心` 改为 `1400` 并应用，负载下 `clocks.gr` 应稳定在 1400 附近。
6. “恢复默认”，确认功耗墙、频率回到第 2 步的值、风扇恢复自动。
7. 重新应用后 `sudo reboot`，开机后 `rig-miner oc-status` 与 `nvidia-smi` 应显示超频已重新生效，机器消息里有“开机后已重新应用超频”。

## 测试

```bash
python3 tests/test_gpu_oc.py                 # 运行层：HiveOS 补齐规则、NVML 调用、各代 AMD sysfs 写入
cargo test -p rig-domain                     # 参数校验
cd frontend && npx playwright test tests/overclock.spec.ts   # 页面（模拟 API）
```
