// HiveOS worker page: coloured header band, miner panel with per-device tiles, status line,
// recent-message chips, the device table (CPU + every GPU) and the system tiles at the bottom.
import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { App, Button, Modal, Popconfirm, Space, Tooltip } from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  client,
  data,
  isFresh,
  machinePower,
  minerStatsFresh,
  serverNow,
  unwrap,
  type Data,
  type Machine,
  type MachineMessage,
  type Observation,
} from "../../shared/api";
import { bytes, hashrate, number } from "../../shared/ui";
import {
  IconBolt,
  IconClose,
  IconCpu,
  IconGpu,
  IconPickaxe,
  IconRocket,
  IconTrash,
  IconTune,
} from "../../shared/icons";
import { CoinBadge } from "../../shared/coins";

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
/** GDDR6X throttles near 110 °C; from here the memory temperature is shown on the row. */
const MEM_HOT = 100;
function tempClass(value?: number, limit = 85) {
  if (value === undefined) return "";
  return value > limit ? "hot" : value > limit - 10 ? "warm" : "ok";
}
export const isRunning = minerStatsFresh;
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

/** HiveOS puts the worker action icons in the top navigation bar; phones keep them in the header. */
function TopTools({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const [wide, setWide] = useState(() => window.matchMedia("(min-width: 861px)").matches);
  useEffect(() => {
    setSlot(document.getElementById("rd-top-tools"));
    const query = window.matchMedia("(min-width: 861px)");
    const change = () => setWide(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  return slot && wide ? createPortal(children, slot) : <div className="wk-band-tools">{children}</div>;
}

const FAVORITES = "rigdeck:fleet-favorites";
/** The fleet list's favourites (☆), shared through localStorage like the list does. */
function useFavorite(id: string) {
  const read = () => {
    try {
      return (JSON.parse(localStorage.getItem(FAVORITES) ?? "[]") as string[]).includes(id);
    } catch {
      return false;
    }
  };
  const [on, setOn] = useState(read);
  const toggle = () => {
    try {
      const list = JSON.parse(localStorage.getItem(FAVORITES) ?? "[]") as string[];
      const next = list.includes(id) ? list.filter((x) => x !== id) : [...list, id];
      localStorage.setItem(FAVORITES, JSON.stringify(next));
      setOn(next.includes(id));
    } catch {
      setOn(!on);
    }
  };
  return [on, toggle] as const;
}

/**
 * HiveOS worker header, two lines. 1: ☆ name, flight sheet badge | LA and power.
 * 2: booted, miner state, miner names, runtime version | kernel, driver, device summary.
 */
export function WorkerBand({
  machine,
  system,
  power,
  instances,
  actions,
}: {
  machine: Machine;
  system?: Observation;
  power?: Observation;
  instances: Data[];
  actions?: ReactNode;
}) {
  const d = data(system?.data);
  const online = isFresh(system);
  const reading = machinePower(system, machine.policy);
  const off = isFresh(power, 90) && String(data(power?.data).state ?? "").toLowerCase() === "off";
  const [favorite, toggleFavorite] = useFavorite(machine.id);
  const sheets = [...new Map(instances.filter((i) => i.sheet_id).map((i) => [String(i.sheet_id), String(i.sheet_name ?? "飞行表")])).entries()];
  const running = instances.filter(isRunning);
  const minerUptime = running.map((i) => num(i.stats?.uptime)).filter((v): v is number => v !== undefined);
  const miners = [...new Set(instances.map((i) => [i.miner_name ?? i.instance, i.package_version].filter(Boolean).join("-")))];
  const gpus: Gpu[] = online && Array.isArray(d.gpus) ? d.gpus : [];
  const models = new Map<string, number>();
  for (const gpu of gpus) models.set(gpu.model ?? gpu.id, (models.get(gpu.model ?? gpu.id) ?? 0) + 1);
  const shortModel = (model: string) => model.replace(/^(NVIDIA\s+)?(GeForce\s+)?|^(AMD\s+)?(Radeon\s+)?/i, "");
  const drivers = Object.entries(data(d.gpu_drivers));
  const load: number[] = online && Array.isArray(d.load) ? d.load : [];
  return (
    <div className={`wk-band ${online ? "" : "offline"}`}>
      <TopTools>{actions}</TopTools>
      <div className="wk-hdr-line">
        <div className="wk-band-name">
          <button
            type="button"
            className={`wk-star ${favorite ? "on" : ""}`}
            aria-label={favorite ? "取消收藏" : "收藏"}
            aria-pressed={favorite}
            onClick={toggleFavorite}
          >
            {favorite ? "★" : "☆"}
          </button>
          <strong>{machine.name}</strong>
          {machine.is_controller && <small className="wk-controller">主控</small>}
          {sheets.length ? (
            sheets.map(([sheetId, name]) => (
              <Link key={sheetId} className="wk-fs" to={`/flight-sheets?edit=${sheetId}`}>
                <IconRocket size={12} /> {name}
              </Link>
            ))
          ) : (
            <span className="wk-fs none">
              <IconRocket size={12} /> 无飞行表
            </span>
          )}
          {!online && <span className="wk-band-state">{off ? "已关机" : "离线"}</span>}
        </div>
        <div className="wk-band-right">
          <span className="wk-la">
            <b>LA</b>
            {load.length ? load.map((n, i) => <i key={i}>{n.toFixed(2)}</i>) : <i>—</i>}
          </span>
          <Tooltip title={reading.source}>
            <span className="wk-power">
              <IconBolt size={12} /> {reading.watts === undefined ? "—" : reading.watts.toFixed(2)}
              <small>W</small>
            </span>
          </Tooltip>
        </div>
      </div>
      <div className="wk-hdr-line sub wk-status">
        <div>
          <span>
            <b>{online ? "已启动" : "离线"}</b> {online && num(d.uptime) !== undefined ? `${duration(d.uptime)} 前` : ""}
          </span>
          {online && (
            <span className={running.length ? "" : "wk-warn"}>
              <b>{running.length ? "挖矿软件正常运行" : instances.length ? "挖矿软件未运行" : "未运行矿工"}</b>{" "}
              {minerUptime.length ? duration(Math.min(...minerUptime)) : "--"}
            </span>
          )}
          {miners.length > 0 && (
            <span>
              <IconPickaxe size={11} /> {miners.join(", ")}
            </span>
          )}
          {d.runtime_version && <span className="wk-ver">↑ {d.runtime_version}</span>}
        </div>
        <div>
          {d.kernel && <span>{d.kernel}</span>}
          {drivers.map(([vendor, version]) => (
            <span key={vendor}>
              <em className={vendor === "NVIDIA" ? "nvidia" : "amd"}>{vendor.slice(0, 1)}</em>
              {String(version)}
            </span>
          ))}
          {models.size > 0
            ? [...models.entries()].map(([model, count]) => (
                <span key={model}>
                  <em className={gpus.find((g) => (g.model ?? g.id) === model)?.vendor === "AMD" ? "amd" : "nvidia"}>{shortModel(model)}</em> <b>× {count}</b>
                </span>
              ))
            : d.cpu_model && <span className="wk-model">{d.cpu_model}</span>}
        </div>
      </div>
    </div>
  );
}

/** Running miners, one row each: miner, version, shares, algorithm; coin, pool, hashrate. */
export function MinerPanel({ instances }: { instances: Data[] }) {
  if (!instances.length) return null;
  return (
    <section className="wk-head">
      {instances.map((i) => {
        const fresh = isRunning(i);
        const stats = fresh ? i.stats ?? {} : {};
        const pool = String(i.pool ?? "").replace(/^[a-z+]+:\/\//i, "");
        return (
          <div className="wk-miner" key={i.instance}>
            <div className="wk-miner-title">
              <IconPickaxe size={13} />
              <b>{i.miner_name ?? i.miner ?? i.instance}</b>
              <span className="muted">{stats.version ?? i.package_version ? ` ${stats.version ?? i.package_version}` : ""}</span>
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
    </section>
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
        <span className="right">HASH</span>
        <span className="center">TEMP</span>
        <span className="right" />
        <span className="right" />
        <span className="right">LOAD</span>
        <span className="right">CORE</span>
        <span className="right">MEM</span>
        <span className="center">{gpus.length > 0 ? tune("修改所有显卡参数") : null}</span>
      </div>
      {(gpus.length === 0 || cpuMiners.length > 0) && (() => {
        // GPU rigs list only their cards, like HiveOS; the CPU row is for CPU-only rigs or CPU mining.
        // One row for the whole machine; sockets show as "2 ×" and in the tooltip.
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
            <span className="right muted">—</span>
            <Tooltip title={packages.length > 1 ? packages.map((p, n) => `CPU ${n}: ${number(p.power_w, 0)} W`).join(" · ") : undefined}>
              <span className="right wk-dev-power" data-label="功耗">
                {num(d.cpu_power_w) !== undefined ? <><IconBolt size={11} /> {Number(d.cpu_power_w).toFixed(2)} <small>W</small></> : "—"}
              </span>
            </Tooltip>
            <span className="right" data-label="负载">{online ? cell(num(d.cpu_pct), "%") : "—"}</span>
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
            {/* HiveOS shows the core temperature; the memory junction is in the tooltip and only
                flagged on the row once it reaches MEM_HOT. */}
            <Tooltip title={num(gpu.memory_temperature_c) !== undefined ? `核心 ${cell(num(gpu.temperature_c), "°")} · 显存 ${cell(num(gpu.memory_temperature_c), "°")}` : undefined}>
              <span className="center wk-temp-pair" data-label="温度">
                <b className={`wk-temp ${tempClass(num(gpu.temperature_c), limit)}`}>{cell(num(gpu.temperature_c), "°")}</b>
                {(num(gpu.memory_temperature_c) ?? 0) >= MEM_HOT && <em className="wk-mem-hot">显存 {cell(num(gpu.memory_temperature_c), "°")}</em>}
              </span>
            </Tooltip>
            <span className="right" data-label="风扇">{cell(num(gpu.fan_pct), "%")}</span>
            <span className="right wk-dev-power" data-label="功耗" title={num(gpu.power_limit_w) !== undefined ? `功耗墙 ${number(gpu.power_limit_w, 0)} W` : undefined}>
              {num(gpu.power_w) !== undefined ? <><IconBolt size={11} /> {gpu.power_w!.toFixed(2)} <small>W</small></> : "—"}
            </span>
            <span className="right" data-label="负载">{cell(num(gpu.util_pct), "%")}</span>
            <span className="right" data-label="核心 MHz" title={num(gpu.core_mv) !== undefined ? `${number(gpu.core_mv, 0)} mV` : undefined}>
              {cell(num(gpu.core_mhz))}
            </span>
            <span className="right" data-label="显存 MHz">{cell(num(gpu.mem_mhz))}</span>
            <span className="center">{tune(`修改 GPU ${index} 参数`)}</span>
          </div>
        );
      })}
    </div>
  );
}

/** HiveOS bottom tiles: load averages, free memory (bar = share used), power, runtime, driver. */
export function SystemTiles({ system, policy }: { system?: Observation; policy?: unknown }) {
  const d = data(system?.data);
  const online = isFresh(system);
  const load: number[] = online && Array.isArray(d.load) ? d.load : [];
  const reading = machinePower(system, policy);
  const free = num(d.memory_total) !== undefined && num(d.memory_used) !== undefined ? d.memory_total - d.memory_used : undefined;
  const usedPct = online && free !== undefined && d.memory_total > 0 ? (100 * (d.memory_total - free)) / d.memory_total : undefined;
  const drivers = Object.entries(data(d.gpu_drivers));
  const [freeValue, freeUnit] = online && free !== undefined ? bytes(free).split(" ") : ["—", ""];
  return (
    <div className="wk-sys">
      <div className="wk-sys-tile wide">
        <div className="wk-sys-values">
          {["1分钟", "5分钟", "15分钟"].map((label, i) => (
            <span key={label}>
              <b>{load[i] !== undefined ? load[i].toFixed(2) : "—"}</b>
              <small>{label}</small>
            </span>
          ))}
        </div>
        <em>平均载入</em>
      </div>
      <div className="wk-sys-tile" style={usedPct !== undefined ? ({ "--used": `${usedPct}%` } as CSSProperties) : undefined}>
        {usedPct !== undefined && <i className="wk-sys-bar" aria-hidden />}
        <div className="wk-sys-values">
          <b>{freeValue}</b>
          <small>{freeUnit}</small>
        </div>
        <em>剩余内存</em>
      </div>
      <Tooltip title={reading.source}>
        <div className="wk-sys-tile">
          <div className="wk-sys-values">
            <IconBolt size={13} className="wk-sys-bolt" />
            <b>{reading.watts === undefined ? "—" : reading.watts.toFixed(2)}</b>
            <small>W</small>
          </div>
          <em>功耗</em>
        </div>
      </Tooltip>
      <div className="wk-sys-tile">
        <div className="wk-sys-values">
          <b>{d.runtime_version ?? "—"}</b>
        </div>
        <em>
          系统版本
          {d.transport === "push" ? " · 主动上报" : d.transport === "ssh" ? " · SSH 读取" : ""}
        </em>
      </div>
      {drivers.length > 0 && (
        <div className="wk-sys-tile">
          <div className="wk-sys-values">
            {drivers.map(([vendor, version]) => (
              <span key={vendor}>
                <em className={vendor === "NVIDIA" ? "nvidia" : "amd"}>{vendor.slice(0, 1)}</em> <b className="small">{String(version)}</b>
              </span>
            ))}
          </div>
          <em>驱动</em>
        </div>
      )}
    </div>
  );
}

/** HiveOS hardware lines under the tiles: two plain label / value columns. */
export function WorkerInfo({ machine, system }: { machine: Machine; system?: Observation }) {
  const d = data(system?.data);
  const topology = (d.topology ?? []) as Data[];
  const sockets = new Set(topology.map((c) => c.socket)).size;
  const cpu = d.cpu_model ? `${d.logical_cpus ?? topology.length ?? ""} × ${d.cpu_model}`.replace(/^ × /, "") : undefined;
  const line = (label: string, value: ReactNode) =>
    value === undefined || value === null || value === "" ? null : (
      <div className="wk-info-line" key={label}>
        <span>{label}</span>
        <b>{value}</b>
      </div>
    );
  return (
    <div className="wk-info">
      <div>
        {line("主板", d.board)}
        {line("CPU", cpu)}
        {line("拓扑", topology.length ? `${sockets} 路 / ${new Set(topology.map((c) => `${c.socket}:${c.core}`)).size} 核 / ${topology.length} 线程` : undefined)}
        {line("内存", num(d.memory_total) !== undefined ? bytes(d.memory_total) : undefined)}
        {line("BIOS", d.bios)}
        {line("BMC", machine.bmc?.url)}
      </div>
      <div>
        {line("系统类型", d.os)}
        {line("内核", d.kernel)}
        {line("内网 IP", (d.addresses ?? []).map((a: Data) => a.address).join(", ") || undefined)}
        {line("SSH", `${machine.username}@${machine.host}:${machine.port}`)}
        {line("标签", machine.tags.length ? machine.tags.join(", ") : undefined)}
        {line("最近采集", system?.observed_at ? new Date(system.observed_at).toLocaleString("zh-CN", { hour12: false }) : undefined)}
      </div>
    </div>
  );
}
