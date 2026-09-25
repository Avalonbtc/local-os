// HiveOS worker page: coloured header band, miner panel with per-device tiles, status line,
// recent-message chips, the device table (CPU + every GPU) and the system tiles at the bottom.
import { useState, type ReactNode } from "react";
import { App, Button, Modal, Popconfirm, Space, Tooltip } from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  client,
  data,
  isFresh,
  machinePower,
  serverNow,
  unwrap,
  type Data,
  type Machine,
  type MachineMessage,
  type Observation,
} from "../../shared/api";
import { bytes, hashrate, number } from "../../shared/ui";
import {
  CoinBadge,
  IconBolt,
  IconChipDriver,
  IconClock,
  IconClose,
  IconCpu,
  IconFan,
  IconGauge,
  IconGpu,
  IconLayers,
  IconMemory,
  IconNetwork,
  IconPickaxe,
  IconThermometer,
  IconTrash,
  IconTune,
} from "../../shared/icons";

type Gpu = {
  id: string;
  bus?: number | null;
  vendor?: string;
  model?: string;
  temperature_c?: number | null;
  memory_temperature_c?: number | null;
  fan_pct?: number | null;
  power_w?: number | null;
  power_limit_w?: number | null;
  util_pct?: number | null;
  core_mhz?: number | null;
  mem_mhz?: number | null;
  core_mv?: number | null;
  vram_mb?: number | null;
};

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);
export function duration(seconds?: number) {
  if (seconds === undefined || seconds < 0) return "—";
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : m ? `${m}m ${s}s` : `${s}s`;
}
function tempClass(value?: number, limit = 85) {
  if (value === undefined) return "";
  return value > limit ? "hot" : value > limit - 10 ? "warm" : "ok";
}
export function isRunning(i: Data) {
  return i.desired === "running" && i.process_alive && serverNow() / 1000 - (i.stats_observed_at ?? 0) < 35;
}
const coinOf = (i: Data) => String(i.stats?.coin ?? i.configured_coin ?? "?").toUpperCase();

/** Hashrate per GPU id, from every running miner that reports per-card rates. */
function gpuRates(gpus: Gpu[], instances: Data[]) {
  const rates = new Map<string, { coin: string; hs: number }[]>();
  for (const instance of instances) {
    if (!isRunning(instance) || !Array.isArray(instance.stats?.gpu_hs)) continue;
    instance.stats.gpu_hs.forEach((item: Data, index: number) => {
      const gpu = typeof item.bus === "number" ? gpus.find((g) => g.bus === item.bus) : gpus[index];
      if (gpu && typeof item.hs === "number")
        rates.set(gpu.id, [...(rates.get(gpu.id) ?? []), { coin: coinOf(instance), hs: item.hs }]);
    });
  }
  return rates;
}

export function WorkerBand({
  machine,
  system,
  power,
  actions,
}: {
  machine: Machine;
  system?: Observation;
  power?: Observation;
  actions?: ReactNode;
}) {
  const d = data(system?.data);
  const online = isFresh(system);
  const reading = machinePower(system);
  const off = isFresh(power, 90) && String(data(power?.data).state ?? "").toLowerCase() === "off";
  return (
    <div className={`wk-band ${online ? "" : "offline"}`}>
      <div className="wk-band-name">
        <strong>{machine.name}</strong>
        {machine.is_controller && <small className="wk-controller">主控</small>}
        {!online && <span className="wk-band-state">{off ? "已关机" : "离线"}</span>}
      </div>
      <div className="wk-band-tools">{actions}</div>
      <div className="wk-band-right">
        <span>
          <IconGauge size={14} /> LA {online ? (d.load ?? []).map((n: number) => number(n, 2)).join(" ") : "—"}
        </span>
        <Tooltip title={reading.source}>
          <span className="wk-power">
            <IconBolt size={14} /> {reading.watts === undefined ? "—" : `${number(reading.watts, 1)}W`}
          </span>
        </Tooltip>
      </div>
    </div>
  );
}

/** Left: flight sheet, miner name / version / shares, coin + pool + hashrate. Right: device tiles. */
export function MinerPanel({ machine, system, instances }: { machine: Machine; system?: Observation; instances: Data[] }) {
  const d = data(system?.data);
  const online = isFresh(system);
  const gpus: Gpu[] = online && Array.isArray(d.gpus) ? d.gpus : [];
  const rates = gpuRates(gpus, instances);
  const limit = num(data(machine.policy).gpu_temp_limit_c) ?? 85;
  const sheets = [...new Map(instances.filter((i) => i.sheet_id).map((i) => [i.sheet_id, i.sheet_name])).entries()];
  return (
    <section className="wk-head">
      <div className="wk-miners">
        {sheets.length ? (
          sheets.map(([sheetId, name]) => (
            <div className="wk-sheet" key={String(sheetId)}>
              <b>{String(name ?? "飞行表")}</b> 飞行表{" "}
              <Link to={`/flight-sheets?edit=${sheetId}`} className="wk-edit">编辑</Link>
            </div>
          ))
        ) : (
          <div className="wk-sheet muted">未应用飞行表</div>
        )}
        {instances.map((i) => {
          const fresh = isRunning(i);
          const stats = fresh ? i.stats ?? {} : {};
          const pool = String(i.pool ?? "").replace(/^[a-z+]+:\/\//i, "");
          return (
            <div className="wk-miner" key={i.instance}>
              <div className="wk-miner-title">
                <IconPickaxe size={13} />
                <b>{i.miner_name ?? i.miner ?? i.instance}</b>
                <span className="muted">{stats.version ?? i.package_version ? ` v${stats.version ?? i.package_version}` : ""}</span>
                {typeof stats.accepted === "number" && (
                  <span className="wk-shares">
                    A {stats.accepted}
                    {typeof stats.rejected === "number" && stats.rejected > 0 && <em> R {stats.rejected}</em>}
                  </span>
                )}
                <span className="wk-algo">{String(stats.algorithm ?? i.configured_algorithm ?? "").toUpperCase()}</span>
              </div>
              <div className="wk-miner-coin">
                <CoinBadge coin={coinOf(i)} />
                <b>{coinOf(i)}</b>
                <span className="wk-pool" title={String(i.pool ?? "")}>{pool || "—"}</span>
                <b className={`wk-rate ${fresh ? "" : "stale"}`}>
                  {fresh ? hashrate(stats.hashrate_hs) : i.process_alive ? "等待统计" : i.desired === "running" ? "未运行" : "已停止"}
                </b>
              </div>
            </div>
          );
        })}
        {!instances.length && <div className="muted">尚无托管矿工</div>}
      </div>
      <div className="wk-tiles" aria-label="设备读数">
        <div className="wk-tile">
          <span className={`wk-temp ${tempClass(num(d.cpu_temperature))}`}>
            {num(d.cpu_temperature) !== undefined ? `${number(d.cpu_temperature, 0)}°` : "—"}
          </span>
          <span className="wk-fan">{online && num(d.cpu_pct) !== undefined ? `${number(d.cpu_pct, 0)}%` : "—"}</span>
          <span className="wk-hs">
            <IconCpu size={12} /> CPU
          </span>
        </div>
        {gpus.map((gpu, index) => {
          const own = rates.get(gpu.id) ?? [];
          return (
            <Tooltip key={gpu.id} title={`GPU ${index} · ${gpu.model ?? gpu.id}`}>
              <div className="wk-tile">
                <span className={`wk-temp ${tempClass(num(gpu.temperature_c), limit)}`}>
                  {num(gpu.temperature_c) !== undefined ? `${number(gpu.temperature_c, 0)}°` : "—"}
                </span>
                <span className="wk-fan">{num(gpu.fan_pct) !== undefined ? `${number(gpu.fan_pct, 0)}%` : "—"}</span>
                <span className="wk-hs">{own.length ? shortRate(own[0].hs) : `GPU ${index}`}</span>
              </div>
            </Tooltip>
          );
        })}
      </div>
    </section>
  );
}
function shortRate(hs: number) {
  const [value] = hashrate(hs).split(" ");
  return value;
}

export function StatusLine({ machine, system, instances }: { machine: Machine; system?: Observation; instances: Data[] }) {
  const d = data(system?.data);
  const online = isFresh(system);
  const running = instances.filter(isRunning);
  const minerUptime = running.map((i) => num(i.stats?.uptime)).filter((v): v is number => v !== undefined);
  const gpus: Gpu[] = online && Array.isArray(d.gpus) ? d.gpus : [];
  const models = new Map<string, number>();
  for (const gpu of gpus) models.set(gpu.model ?? gpu.id, (models.get(gpu.model ?? gpu.id) ?? 0) + 1);
  const drivers = Object.entries(data(d.gpu_drivers)).map(([vendor, version]) => `${vendor} ${version}`);
  return (
    <div className="wk-status">
      <span>
        <IconClock /> <b>{online ? "已启动" : "离线"}</b> {online && num(d.uptime) !== undefined ? `${duration(d.uptime)} 前` : ""}
      </span>
      {online && (
        <span className={running.length ? "wk-ok" : "wk-warn"}>
          <IconPickaxe /> <b>{running.length ? "挖矿软件正常运行" : instances.length ? "挖矿软件未运行" : "未运行矿工"}</b>{" "}
          {minerUptime.length ? duration(Math.min(...minerUptime)) : ""}
        </span>
      )}
      <span>
        <IconNetwork /> IP <b>{data(d.addresses?.[0]).address ?? machine.host}</b>
      </span>
      {d.runtime_version && (
        <span>
          <IconLayers /> {d.runtime_version}
        </span>
      )}
      {d.kernel && <span className="muted">{d.kernel}</span>}
      {drivers.length > 0 && (
        <span className="muted">
          <IconChipDriver /> {drivers.join(" · ")}
        </span>
      )}
      {d.cpu_model && (
        <span className="wk-model">
          <IconCpu /> {d.cpu_model}
        </span>
      )}
      {[...models.entries()].map(([model, count]) => (
        <span className="wk-model gpu" key={model}>
          <IconGpu /> {model} × {count}
        </span>
      ))}
    </div>
  );
}

function ago(at: string) {
  return duration(Math.max(0, (serverNow() - new Date(at).getTime()) / 1000));
}
const LEVEL_LABEL: Record<string, string> = { success: "成功", info: "信息", warning: "警告", error: "错误" };
const SOURCE_LABEL: Record<string, string> = { runtime: "矿机", job: "任务", audit: "操作" };
const keyOf = (m: MachineMessage) => `${m.source}:${m.id}`;

/** HiveOS message chips: click to read the whole message, × closes one, the bin clears all. */
export function RecentChips({ id }: { id: string }) {
  const queryClient = useQueryClient();
  const { message: toast } = App.useApp();
  const [open, setOpen] = useState<MachineMessage>();
  const query = useQuery({
    queryKey: ["messages", id, "unread"],
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/machines/{id}/messages", {
          signal,
          params: { path: { id }, query: { limit: 6, unread: true } },
        }),
      ),
    refetchInterval: 15000,
  });
  const dismiss = async (key?: string) => {
    // Optimistic: the chip disappears at once, the list is refetched after.
    queryClient.setQueryData<MachineMessage[]>(["messages", id, "unread"], (items) =>
      key ? (items ?? []).filter((m) => keyOf(m) !== key) : [],
    );
    try {
      await unwrap(client.POST("/api/v1/machines/{id}/messages/dismiss", { params: { path: { id } }, body: { key: key ?? null } }));
    } catch (e) {
      toast.error((e as Error).message);
    }
    await queryClient.invalidateQueries({ queryKey: ["messages", id, "unread"] });
  };
  const items: MachineMessage[] = query.data ?? [];
  const detail = data(open?.detail);
  return (
    <>
      {items.length > 0 && (
        <div className="wk-chips">
          {items.map((m) => (
            <span key={keyOf(m)} className={`wk-chip ${m.level}`}>
              <button className="wk-chip-open" onClick={() => setOpen(m)} title="查看详情">
                <span>&gt; {m.message}</span>
                <small>{ago(m.at)}</small>
              </button>
              <button className="wk-chip-close" aria-label={`关闭消息 ${m.message}`} onClick={() => dismiss(keyOf(m))}>
                <IconClose size={11} />
              </button>
            </span>
          ))}
          <Popconfirm title="清除全部消息条？" description="只是不在这里显示，「活动」里的记录会保留。" okText="清除" cancelText="取消" onConfirm={() => dismiss()}>
            <button className="wk-chip-clear" aria-label="清除全部消息条">
              <IconTrash size={15} />
            </button>
          </Popconfirm>
        </div>
      )}
      <Modal
        open={!!open}
        onCancel={() => setOpen(undefined)}
        footer={
          <Space>
            <Button
              onClick={() => {
                if (open) void dismiss(keyOf(open));
                setOpen(undefined);
              }}
            >
              关闭这条消息
            </Button>
            <Button type="primary" onClick={() => setOpen(undefined)}>
              确定
            </Button>
          </Space>
        }
        title={
          <span className={`wk-msg-title ${open?.level ?? ""}`}>
            {LEVEL_LABEL[open?.level ?? ""] ?? "消息"} · {SOURCE_LABEL[open?.source ?? ""] ?? open?.source}
          </span>
        }
        width={640}
      >
        {open && (
          <div className="wk-msg-detail">
            <p className="wk-msg-text">{open.message}</p>
            <dl>
              <dt>时间</dt>
              <dd>
                {new Date(open.at).toLocaleString("zh-CN", { hour12: false })}（{ago(open.at)} 前）
              </dd>
              {open.instance && (
                <>
                  <dt>实例</dt>
                  <dd>{open.instance}</dd>
                </>
              )}
              <dt>类型</dt>
              <dd className="mono">{open.kind}</dd>
              {detail.actor && (
                <>
                  <dt>操作人</dt>
                  <dd>{String(detail.actor)}</dd>
                </>
              )}
            </dl>
            {typeof detail.output === "string" && detail.output && (
              <pre className="wk-msg-output">
                {detail.output}
                {detail.output_truncated ? "\n[输出过长，已截断]" : ""}
              </pre>
            )}
            {open.source !== "job" && open.detail != null && Object.keys(detail).length > 0 && (
              <pre className="wk-msg-output">{JSON.stringify(open.detail, null, 2)}</pre>
            )}
          </div>
        )}
      </Modal>
    </>
  );
}

/** Overview table: one CPU row per socket, one row per GPU, HiveOS columns. */
export function DeviceTable({ machine, system, instances }: { machine: Machine; system?: Observation; instances: Data[] }) {
  const d = data(system?.data);
  const online = isFresh(system);
  const gpus: Gpu[] = online && Array.isArray(d.gpus) ? d.gpus : [];
  const rates = gpuRates(gpus, instances);
  const limit = num(data(machine.policy).gpu_temp_limit_c) ?? 85;
  const sockets = [...new Set(((d.topology ?? []) as Data[]).map((c) => c.socket))];
  const cpuMiners = instances.filter(
    (i) => isRunning(i) && !String(i.stats?.device ?? "cpu").startsWith("gpu") && typeof i.stats?.hashrate_hs === "number",
  );
  const tune = (label: string) => (
    <Tooltip title={`${label}：显卡超频在下一轮提供`}>
      <span className="wk-tune disabled" aria-label={label}>
        <IconTune size={15} />
      </span>
    </Tooltip>
  );
  const cell = (value: number | undefined, suffix = "") => (value === undefined ? "—" : `${number(value, 0)}${suffix}`);
  return (
    <div className="wk-devices" role="table" aria-label="设备">
      <div className="wk-dev-row head" role="row">
        <span />
        <span />
        <span className="right">算力</span>
        <span className="center">
          <IconThermometer size={13} /> 温度
        </span>
        <span className="right">负载</span>
        <span className="right">
          <IconBolt size={13} /> 功耗
        </span>
        <span className="right">
          <IconFan size={13} /> 风扇
        </span>
        <span className="right">核心</span>
        <span className="right">VDD</span>
        <span className="right">显存</span>
        <span className="center">{gpus.length > 0 ? tune("修改所有显卡参数") : null}</span>
      </div>
      {(() => {
        // One CPU row for the whole machine, like HiveOS; sockets show as "2 ×" and in the tooltip.
        const topology = (d.topology ?? []) as Data[];
        const cores = new Set(topology.map((c) => `${c.socket}:${c.core}`)).size;
        const packages = ((d.cpu_power_packages ?? []) as Data[]).filter((p) => num(p.power_w) !== undefined);
        return (
          <div className="wk-dev-row cpu" role="row">
            <span className="wk-dev-index">
              <span>
                <IconCpu size={14} /> CPU
              </span>
              <small>
                {topology.length ? `${cores} 核 / ${topology.length} 线程` : `${d.logical_cpus ?? "—"} 线程`}
              </small>
            </span>
            <span className="wk-dev-model cpu">
              {sockets.length > 1 ? `${sockets.length} × ` : ""}
              {d.cpu_model ?? "CPU 型号待采集"}
            </span>
            <span className="right" data-label="算力">
              {cpuMiners.length
                ? cpuMiners.map((i) => (
                    <div key={i.instance}>
                      <b>{hashrate(i.stats.hashrate_hs)}</b> <small>{coinOf(i)}</small>
                    </div>
                  ))
                : "—"}
            </span>
            <span className={`center wk-temp ${tempClass(num(d.cpu_temperature))}`} data-label="温度">{cell(num(d.cpu_temperature), "°")}</span>
            <span className="right" data-label="负载">{online ? cell(num(d.cpu_pct), "%") : "—"}</span>
            <Tooltip title={packages.length > 1 ? packages.map((p, n) => `CPU ${n}: ${number(p.power_w, 0)} W`).join(" · ") : undefined}>
              <span className="right" data-label="功耗">{cell(num(d.cpu_power_w), " W")}</span>
            </Tooltip>
            <span className="right muted">—</span>
            <span className="right muted">—</span>
            <span className="right muted">—</span>
            <span className="right muted">—</span>
            <span />
          </div>
        );
      })()}
      {gpus.map((gpu, index) => {
        const own = rates.get(gpu.id) ?? [];
        return (
          <div className="wk-dev-row" role="row" key={gpu.id}>
            <span className="wk-dev-index">
              <span>
                <IconGpu size={14} /> GPU {index}
              </span>
              <small>{gpu.id.replace(/^0000:/, "")}</small>
            </span>
            <span className="wk-dev-model">
              <b className={gpu.vendor === "NVIDIA" ? "nvidia" : gpu.vendor === "AMD" ? "amd" : ""}>{gpu.model ?? gpu.id}</b>
              {num(gpu.vram_mb) !== undefined && <small> {number(gpu.vram_mb, 0)} MB</small>}
            </span>
            <span className="right" data-label="算力">
              {own.length
                ? own.map((r, i) => (
                    <div key={i}>
                      <b>{hashrate(r.hs)}</b> <small>{r.coin}</small>
                    </div>
                  ))
                : "—"}
            </span>
            <span className="center wk-temp-pair" data-label="温度">
              <b className={`wk-temp ${tempClass(num(gpu.temperature_c), limit)}`}>{cell(num(gpu.temperature_c), "°")}</b>
              {num(gpu.memory_temperature_c) !== undefined && (
                <small className={`wk-temp ${tempClass(num(gpu.memory_temperature_c), 100)}`}>{cell(num(gpu.memory_temperature_c), "°")}</small>
              )}
            </span>
            <span className="right" data-label="负载">{cell(num(gpu.util_pct), "%")}</span>
            <span className="right" data-label="功耗" title={num(gpu.power_limit_w) !== undefined ? `功耗墙 ${number(gpu.power_limit_w, 0)} W` : undefined}>
              {cell(num(gpu.power_w), " W")}
            </span>
            <span className="right" data-label="风扇">{cell(num(gpu.fan_pct), "%")}</span>
            <span className="right" data-label="核心 MHz">{cell(num(gpu.core_mhz))}</span>
            <span className="right" data-label="VDD mV">{cell(num(gpu.core_mv))}</span>
            <span className="right" data-label="显存 MHz">{cell(num(gpu.mem_mhz))}</span>
            <span className="center">{tune(`修改 GPU ${index} 参数`)}</span>
          </div>
        );
      })}
    </div>
  );
}

/** Bottom tiles: load, CPU temperature, free memory, power, runtime and GPU driver. */
export function SystemTiles({ system }: { system?: Observation }) {
  const d = data(system?.data);
  const online = isFresh(system);
  const load: number[] = online && Array.isArray(d.load) ? d.load : [];
  const reading = machinePower(system);
  const free = num(d.memory_total) !== undefined && num(d.memory_used) !== undefined ? d.memory_total - d.memory_used : undefined;
  const drivers = Object.entries(data(d.gpu_drivers));
  return (
    <div className="wk-sys">
      <div className="wk-sys-tile wide">
        <IconGauge size={26} className="wk-sys-icon" />
        {["1 分钟", "5 分钟", "15 分钟"].map((label, i) => (
          <span key={label}>
            <b>{load[i] !== undefined ? number(load[i], 2) : "—"}</b>
            <small>{label}</small>
          </span>
        ))}
        <em>平均负载</em>
      </div>
      <div className="wk-sys-tile">
        <IconThermometer size={26} className="wk-sys-icon" />
        <b>{online && num(d.cpu_temperature) !== undefined ? `${number(d.cpu_temperature, 0)}°` : "—"}</b>
        <em>CPU 温度</em>
      </div>
      <div className="wk-sys-tile">
        <IconMemory size={26} className="wk-sys-icon" />
        <b>{online && free !== undefined ? bytes(free) : "—"}</b>
        <em>剩余内存</em>
      </div>
      <div className="wk-sys-tile">
        <IconBolt size={26} className="wk-sys-icon power" />
        <b>
          {reading.watts === undefined ? "—" : `${number(reading.watts, 1)} W`}
        </b>
        <em>{reading.label}</em>
      </div>
      <div className="wk-sys-tile">
        <IconLayers size={26} className="wk-sys-icon" />
        <b>{d.runtime_version ?? "—"}</b>
        <em>运行层版本</em>
      </div>
      {drivers.length > 0 && (
        <div className="wk-sys-tile">
          <IconChipDriver size={26} className="wk-sys-icon" />
          {drivers.map(([vendor, version]) => (
            <b key={vendor}>
              <small>{vendor.slice(0, 1)}</small> {String(version)}
            </b>
          ))}
          <em>驱动</em>
        </div>
      )}
    </div>
  );
}
