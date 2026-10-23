import type React from "react";
import { useEffect, useState } from "react";
import {
  Alert,
  Button,
  Empty,
  Input,
  Modal,
  Select,
  Space,
  Tooltip,
} from "antd";
import {
  AppstoreOutlined,
  ArrowsAltOutlined,
  BarsOutlined,
  DownloadOutlined,
  FireFilled,
  InfoCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  SortAscendingOutlined,
  StarFilled,
  StarOutlined,
  UnorderedListOutlined,
  RocketFilled,
} from "@ant-design/icons";
import { Link, useNavigate } from "react-router-dom";
import {
  data,
  isFresh,
  latest,
  machinePower,
  MINER_FRESH_SECONDS,
  minerStatsFresh,
  serverNow,
  useMachines,
  useRefresh,
  useFarmSettings,
  useFleetTelemetry,
  type Machine,
  type Observation,
} from "../../shared/api";
import { hashrate, number, QueryState } from "../../shared/ui";
import { BatchModal, MachineEditor } from "./controls";
import { BulkActions } from "./bulk";
import { setSelection, useSelection } from "../../shared/selection";
import { BatchAddModal } from "./import";
import { IconBolt, IconCpu, IconEmptyRigs, IconGpu, IconPickaxe, IconRig, IconServer, IconYuan } from "../../shared/icons";
import { CoinBadge } from "../../shared/coins";

type Filter = "all" | "online" | "offline" | "problems" | "favorites" | "selected";
type View = "dense" | "comfortable" | "grid";
type Sort = "name" | "uptime" | "hashrate" | "load";

type GpuReading = {
  id: string;
  vendor?: string;
  bus?: number | null;
  model?: string;
  temperature_c?: number | null;
  memory_temperature_c?: number | null;
  fan_pct?: number | null;
  power_w?: number | null;
  util_pct?: number | null;
};
type FleetRow = {
  machine: Machine;
  online: boolean;
  issue: boolean;
  issues: string[];
  address?: string;
  cpuTemperature?: number;
  uptime?: number;
  cpuPct?: number;
  gpus: GpuReading[];
  gpuTempLimit: number;
  logicalCpus?: number;
  load?: number;
  powerW?: number;
  powerLabel: string;
  powerSource: string;
  bmcOnline: boolean;
  powerState?: string;
  /** When the rig last reported (system observation time), for offline rows. */
  lastSeen?: string;
  instances: Record<string, any>[];
  hashrateHs: number;
};

function model(
  machine: Machine,
  observations: Observation[] | undefined,
): FleetRow {
  const system = latest(observations, machine.id, "system");
  const mining = latest(observations, machine.id, "mining");
  const power = latest(observations, machine.id, "power");
  const online = isFresh(system);
  const systemData = online ? data(system?.data) : {};
  const reportedInstances = data(mining?.data).instances;
  const instances: Record<string, any>[] =
    isFresh(mining, MINER_FRESH_SECONDS) && Array.isArray(reportedInstances)
      ? reportedInstances
      : [];
  const active = instances.filter(minerStatsFresh);
  const minerUptimes = active
    .map((instance) => instance.stats?.uptime)
    .filter((value): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0);
  const issues: string[] = [];
  if (system?.error) issues.push(`采集失败：${system.error}`);
  else if (system && !online) issues.push("超过 35 秒没有新数据");
  if (mining?.error) issues.push(`矿工统计：${mining.error}`);
  if (power?.error) issues.push(`BMC：${power.error}`);
  for (const instance of instances) {
    if (instance.phase === "faulted" || instance.phase === "failed")
      issues.push(`${instance.instance}：多次自动恢复失败，已停止重启`);
    else if (instance.desired === "running" && !instance.process_alive)
      issues.push(`${instance.instance}：矿工进程未运行`);
    else if (instance.error) issues.push(`${instance.instance}：${instance.error}`);
    else if (instance.stats?.connected === false)
      issues.push(`${instance.instance}：矿池未连接`);
  }
  const addresses = Array.isArray(systemData.addresses) ? systemData.addresses : [];
  return {
    machine,
    online,
    issue: issues.length > 0,
    issues,
    address: addresses[0]?.address,
    cpuTemperature:
      typeof systemData.cpu_temperature === "number"
        ? systemData.cpu_temperature
        : undefined,
    uptime: minerUptimes.length ? Math.min(...minerUptimes) : undefined,
    cpuPct:
      typeof systemData.cpu_pct === "number" ? systemData.cpu_pct : undefined,
    gpus: Array.isArray(systemData.gpus) ? systemData.gpus : [],
    gpuTempLimit: typeof data(machine.policy).gpu_temp_limit_c === "number"
      ? data(machine.policy).gpu_temp_limit_c
      : 85,
    logicalCpus:
      typeof systemData.logical_cpus === "number"
        ? systemData.logical_cpus
        : undefined,
    load:
      typeof systemData.load?.[0] === "number" ? systemData.load[0] : undefined,
    powerW: machinePower(system, machine.policy).watts,
    powerLabel: machinePower(system, machine.policy).label,
    powerSource: machinePower(system, machine.policy).source,
    bmcOnline: isFresh(power, 90),
    powerState: isFresh(power, 90) ? data(power?.data).state : undefined,
    lastSeen: system?.observed_at,
    instances,
    hashrateHs: active.reduce(
      (sum, instance) =>
        sum +
        (typeof instance.stats?.hashrate_hs === "number"
          ? instance.stats.hashrate_hs
          : 0),
      0,
    ),
  };
}

/** HiveOS shortens long worker names in the middle: "docker-3070ti-monitor" → "docker-..monitor". */
function shortName(name: string) {
  return name.length > 17 ? `${name.slice(0, 7)}..${name.slice(-7)}` : name;
}

/** HiveOS marks which devices a rig has: N (NVIDIA), A (AMD), I (Intel); C for a CPU-only rig. */
function VendorMarks({ row }: { row: FleetRow }) {
  const vendors = [...new Set(row.gpus.map((g) => g.vendor).filter(Boolean))] as string[];
  if (!vendors.length) return <span className="hive-mark cpu">C</span>;
  return (
    <>
      {vendors.map((v) => (
        <span key={v} className={`hive-mark ${v.toLowerCase()}`}>
          {v.slice(0, 1)}
        </span>
      ))}
    </>
  );
}

function uptimeLabel(seconds?: number) {
  if (seconds === undefined) return "—";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return days
    ? `${days}d ${hours}h`
    : hours
      ? `${hours}h ${minutes}m`
      : `${minutes}m`;
}

// The summary bar and the list render the same rows; build them once per data refresh.
let rowCache: { machines?: Machine[]; telemetry?: Observation[]; rows: FleetRow[] } = {
  rows: [],
};
function fleetRows(machines?: Machine[], telemetry?: Observation[]) {
  if (rowCache.machines !== machines || rowCache.telemetry !== telemetry)
    rowCache = {
      machines,
      telemetry,
      rows: (machines ?? []).map((machine) => model(machine, telemetry)),
    };
  return rowCache.rows;
}

export function FleetDataCache() {
  useMachines();
  useFleetTelemetry();
  return null;
}

/** Hashrate split into value and unit so a card can show the unit small, HiveOS-style. */
function rateParts(hs: number) {
  const [value, unit] = hashrate(hs).split(" ");
  return { value, unit };
}
const running = minerStatsFresh;
function coinOf(instance: Record<string, any>) {
  return String(instance.stats?.coin ?? instance.configured_coin ?? "?").toUpperCase();
}

export function FleetSummary() {
  const machines = useMachines();
  const telemetry = useFleetTelemetry();
  const price = useFarmSettings().data?.electricity_price;
  const rows = fleetRows(machines.data, telemetry.data);
  const online = rows.filter((row) => row.online).length;
  const threads = rows.reduce((sum, row) => sum + (row.logicalCpus ?? 0), 0);
  const gpus = rows.flatMap((row) => (row.online ? row.gpus : []));
  const hotGpus = rows.reduce(
    (sum, row) =>
      sum + (row.online ? row.gpus.filter((g) => typeof g.temperature_c === "number" && g.temperature_c > row.gpuTempLimit).length : 0),
    0,
  );
  const powerReadings = rows
    .map((row) => row.powerW)
    .filter((value): value is number => value !== undefined);
  const totalWatts = powerReadings.reduce((a, b) => a + b, 0);
  // One card per coin + algorithm (CPU XMR and GPU coins side by side), largest first.
  const rates = new Map<string, { coin: string; algorithm: string; hs: number; rigs: Set<string> }>();
  for (const row of rows) {
    for (const instance of row.instances) {
      if (!running(instance) || typeof instance.stats?.hashrate_hs !== "number") continue;
      const coin = coinOf(instance);
      const algorithm = instance.stats.algorithm ?? instance.configured_algorithm ?? "";
      const key = `${coin}|${algorithm}`;
      const entry = rates.get(key) ?? { coin, algorithm, hs: 0, rigs: new Set<string>() };
      entry.hs += instance.stats.hashrate_hs;
      entry.rigs.add(row.machine.id);
      rates.set(key, entry);
    }
  }
  const rateCards = [...rates.values()].sort((a, b) => b.rigs.size - a.rigs.size || b.hs - a.hs);
  const dailyCost =
    powerReadings.length && price !== undefined ? (totalWatts / 1000) * 24 * price : undefined;
  return (
    <section className="hive-farm-summary" aria-label="矿场统计">
      <div className="hive-farm-summary-inner">
        {/* HiveOS: a thin bar on top, green for the online share and orange for the rest. */}
        <div
          className="hive-stat hive-stat-green hive-stat-ratio"
          style={{ "--online": `${rows.length ? (online / rows.length) * 100 : 0}%` } as React.CSSProperties}
        >
          <div className="hive-stat-line">
            <strong>{machines.data?.length ?? "—"}</strong>
            <em>{machines.data ? rows.length - online : "—"}</em>
          </div>
          <span>
            <IconRig size={13} />矿机 <small>离线</small>
          </span>
        </div>
        <div className="hive-stat hive-stat-green">
          <div className="hive-stat-line">
            <strong>{threads || "—"}</strong>
            <em className="stat-red">
              {machines.data ? rows.filter((row) => row.issue).length : "—"}
            </em>
          </div>
          <span>
            <IconCpu size={13} />CPU 线程 <small>告警</small>
          </span>
        </div>
        {gpus.length > 0 && (
          <div className="hive-stat hive-stat-green">
            <div className="hive-stat-line">
              <strong>{gpus.length}</strong>
              <em className={hotGpus ? "stat-red" : ""}>{hotGpus}</em>
            </div>
            <span>
              <IconGpu size={13} />GPU <small>高温</small>
            </span>
          </div>
        )}
        <div className="hive-stat" title={`${powerReadings.length}/${rows.length} 台有软件功耗读数（CPU RAPL + 显卡自带读数），不含内存、风扇及电源损耗`}>
          <div className="hive-stat-line">
            <strong>
              <IconBolt size={18} className="power-symbol" /> {powerReadings.length ? number(totalWatts / 1000, 2) : "—"}
            </strong>
            <small>kW</small>
          </div>
          <span>
            <IconBolt size={13} />功耗 <small>{powerReadings.length}/{rows.length} 台</small>
          </span>
        </div>
        {/* One hashrate card for every coin (HiveOS keeps the summary to a single row): one coin
            shows big, two share the card, more show the first and "+N"; the tooltip lists all. */}
        <Tooltip
          title={
            rateCards.length > 1 && (
              <div className="hive-issue-list">
                {rateCards.map((card) => (
                  <div key={`${card.coin}|${card.algorithm}`}>
                    {card.coin} {hashrate(card.hs)} · {card.algorithm} · {card.rigs.size} 台
                  </div>
                ))}
              </div>
            )
          }
        >
          <div className={`hive-stat hive-stat-rate ${rateCards.length > 1 ? "hive-stat-rates" : ""}`}>
            {rateCards.length <= 1 ? (
              <div className="hive-stat-line">
                <strong>{rateCards[0] ? rateParts(rateCards[0].hs).value : "—"}</strong>
                {rateCards[0] && <small>{rateParts(rateCards[0].hs).unit}</small>}
              </div>
            ) : (
              (rateCards.length > 2 ? rateCards.slice(0, 1) : rateCards).map((card) => (
                <div className="hive-rate-line" key={`${card.coin}|${card.algorithm}`}>
                  <CoinBadge coin={card.coin} size={13} />
                  <b>{card.coin}</b>
                  <strong>{rateParts(card.hs).value}</strong>
                  <small>{rateParts(card.hs).unit}</small>
                </div>
              ))
            )}
            {rateCards.length > 2 && <div className="hive-rate-line more">+{rateCards.length - 1} 种币</div>}
            <span>
              {rateCards.length === 1 ? (
                <>
                  <CoinBadge coin={rateCards[0].coin} size={13} />
                  <b className="hive-stat-coin">{rateCards[0].coin}</b> {rateCards[0].algorithm}
                </>
              ) : (
                <>
                  <IconPickaxe size={13} />算力
                </>
              )}
              <small>{new Set(rateCards.flatMap((card) => [...card.rigs])).size} 台</small>
            </span>
          </div>
        </Tooltip>
        <div className="hive-stat">
          <div className="hive-stat-line">
            <strong>{rows.filter((row) => row.bmcOnline).length}</strong>
            <small>/{machines.data?.filter((machine) => !!machine.bmc).length ?? "—"}</small>
          </div>
          <span>
            <IconServer size={13} />BMC 在线
          </span>
        </div>
        <Link
          className="hive-stat hive-stat-link"
          to="/settings?tab=farm"
          title={`按当前软件功耗持续 24 小时 × ${price === undefined ? "电价" : `${number(price, 4)} 元/度`}估算，不含电源损耗；不是当天累计电费。点击修改电价`}
        >
          <div className="hive-stat-line">
            <strong>{dailyCost === undefined ? "—" : `¥${number(dailyCost, 2)}`}</strong>
            <small>/日</small>
          </div>
          <span>
            <IconYuan size={13} />预估电费 <small>{price === undefined ? "—" : number(price, 4)} 元/度</small>
          </span>
        </Link>
      </div>
    </section>
  );
}

function tempLevel(value: number | null | undefined, limit: number) {
  if (typeof value !== "number") return "none";
  return value > limit ? "hot" : value > limit - 10 ? "warm" : "ok";
}

/** HiveOS device column: one small square per GPU (or one for the CPU), hover for the readings. */
function DevicesCell({ row }: { row: FleetRow }) {
  if (!row.online) return <div className="hive-capacity" />;
  // Per-GPU hashrate, matched by PCI bus number (or by position when a miner gives none).
  const gpuRates = new Map<string, string[]>();
  for (const instance of row.instances) {
    if (!running(instance) || !Array.isArray(instance.stats?.gpu_hs)) continue;
    instance.stats.gpu_hs.forEach((item: Record<string, any>, index: number) => {
      const gpu =
        typeof item.bus === "number" ? row.gpus.find((g) => g.bus === item.bus) : row.gpus[index];
      if (!gpu || typeof item.hs !== "number") return;
      gpuRates.set(gpu.id, [...(gpuRates.get(gpu.id) ?? []), hashrate(item.hs)]);
    });
  }
  const cpuRate = row.instances
    .filter((i) => running(i) && !String(i.stats?.device ?? "cpu").startsWith("gpu") && typeof i.stats?.hashrate_hs === "number")
    .map((i) => hashrate(i.stats.hashrate_hs));
  const tip = (parts: [string, string | undefined][]) => (
    <span className="hive-dev-tip-body">
      {parts
        .filter(([, v]) => v !== undefined)
        .map(([label, value]) => (
          <span key={label}>
            {label} {value}
          </span>
        ))}
    </span>
  );
  const showCpu = row.gpus.length === 0 || cpuRate.length > 0;
  return (
    <div className="hive-capacity hive-devices" aria-label={`${row.gpus.length} 块 GPU`}>
      {showCpu && (
        <Tooltip
          placement="topLeft"
          arrow={false}
          rootClassName="hive-dev-tip"
          title={tip([
            ["CPU", undefined],
            ["TEMP", row.cpuTemperature !== undefined ? `${number(row.cpuTemperature, 0)}°` : "—"],
            ["LOAD", row.cpuPct !== undefined ? `${number(row.cpuPct, 0)}%` : "—"],
            ["", cpuRate.join(" + ") || undefined],
          ])}
        >
          <i className={`hive-dev cpu ${tempLevel(row.cpuTemperature, 85)}`} aria-label="CPU" />
        </Tooltip>
      )}
      {row.gpus.map((gpu, index) => (
        <Tooltip
          key={gpu.id}
          placement="topLeft"
          arrow={false}
          rootClassName="hive-dev-tip"
          title={tip([
            [`GPU ${index}`, ""],
            ["TEMP", typeof gpu.temperature_c === "number" ? `${number(gpu.temperature_c, 0)}°` : "—"],
            ["FAN", typeof gpu.fan_pct === "number" ? `${number(gpu.fan_pct, 0)}%` : undefined],
            ["MEM", typeof gpu.memory_temperature_c === "number" ? `${number(gpu.memory_temperature_c, 0)}°` : undefined],
            ["", gpuRates.get(gpu.id)?.join(" + ")],
          ])}
        >
          <i className={`hive-dev ${tempLevel(gpu.temperature_c, row.gpuTempLimit)}`} aria-label={`GPU ${index}`} />
        </Tooltip>
      ))}
    </div>
  );
}

/** One line per miner, like HiveOS: coin, hashrate, device, shares, then the miner name. */
function MiningCell({ row }: { row: FleetRow }) {
  const shown = row.instances.filter((item) => item.desired === "running");
  const instances = shown.length ? shown : row.instances.slice(0, 1);
  if (!row.online)
    return (
      <div className="hive-mining-cell">
        <span className="hive-offline-note">
          {row.lastSeen
            ? `掉线 · 最后在线 ${new Date(row.lastSeen).toLocaleString("zh-CN", { hour12: false, month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })}`
            : "掉线 · 从未上报"}
        </span>
      </div>
    );
  if (!instances.length)
    return (
      <div className="hive-mining-cell">
        <span className="hive-row-muted">无飞行表</span>
      </div>
    );
  return (
    <div className="hive-mining-cell">
      {instances.map((instance) => {
        const fresh = running(instance);
        const stats = fresh ? instance.stats ?? {} : {};
        const coin = coinOf(instance);
        const accepted = typeof stats.accepted === "number" ? stats.accepted : undefined;
        const rejected = typeof stats.rejected === "number" ? stats.rejected : undefined;
        const rejectPct =
          accepted !== undefined && rejected !== undefined && accepted + rejected > 0
            ? (rejected / (accepted + rejected)) * 100
            : undefined;
        const device = String(stats.device ?? (Array.isArray(stats.gpu_hs) && stats.gpu_hs.length ? "gpu" : "cpu"));
        const version = stats.version ?? instance.package_version;
        return (
          <div className="hive-miner-line" key={instance.instance}>
            <CoinBadge coin={coin} size={14} />
            <b className="hive-miner-coin">{coin}</b>
            <b className={fresh ? "hive-miner-rate" : "hive-miner-rate stale"}>
              {fresh ? hashrate(stats.hashrate_hs) : instance.process_alive ? "等待统计" : "已停止"}
            </b>
            {row.gpus.length > 0 && <span className="hive-device-tag">{device.toUpperCase()}</span>}
            {accepted !== undefined && (
              <Tooltip title={`接受 ${accepted} · 拒绝 ${rejected ?? "—"}`}>
                <span className={`hive-shares ${rejectPct !== undefined && rejectPct >= 5 ? "bad" : ""}`}>
                  A{accepted}
                  {rejectPct !== undefined && rejectPct > 0 && ` R${number(rejectPct, 1)}%`}
                </span>
              </Tooltip>
            )}
            <Tooltip title={[stats.algorithm ?? instance.configured_algorithm, instance.sheet_name && `飞行表 ${instance.sheet_name}`, `实例 ${instance.instance}`].filter(Boolean).join(" · ")}>
              <span className="hive-row-muted">
                {instance.miner_name ?? instance.miner ?? instance.instance}
                {version ? ` ${version}` : ""}
              </span>
            </Tooltip>
          </div>
        );
      })}
    </div>
  );
}

export function FleetPage() {
  const machines = useMachines();
  const telemetry = useFleetTelemetry();
  const refresh = useRefresh();
  const navigate = useNavigate();
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  const [group, setGroup] = useState<string>();
  const [showFilters, setShowFilters] = useState(false);
  // Shared with the top bar, which turns into the HiveOS bulk bar while workers are ticked.
  const selected = useSelection();
  const setSelected = setSelection;
  useEffect(() => () => setSelection([]), []);
  const [favorites, setFavorites] = useState<string[]>(() => {
    try {
      return JSON.parse(
        localStorage.getItem("rigdeck:fleet-favorites") ?? "[]",
      );
    } catch {
      return [];
    }
  });
  const [sort, setSort] = useState<Sort>("name");
  const [ascending, setAscending] = useState(true);
  const [view, setView] = useState<View>("dense");
  const [wide, setWide] = useState(false);
  const [pageSize, setPageSize] = useState(250);
  const [page, setPage] = useState(1);
  const [editor, setEditor] = useState(false);
  const [importing, setImporting] = useState(false);
  const [batch, setBatch] = useState("");
  const [help, setHelp] = useState(false);
  const rows = fleetRows(machines.data, telemetry.data);
  const groups = [
    ...new Set(rows.map((row) => row.machine.group).filter(Boolean)),
  ];
  const filtered = rows
    .filter((row) => {
      const machine = row.machine;
      if (group && machine.group !== group) return false;
      if (
        search &&
        !`${machine.name} ${machine.host} ${machine.tags.join(" ")}`
          .toLowerCase()
          .includes(search.toLowerCase())
      )
        return false;
      if (filter === "online") return row.online;
      if (filter === "offline") return !row.online;
      if (filter === "problems") return row.issue;
      if (filter === "favorites") return favorites.includes(machine.id);
      if (filter === "selected") return selected.includes(machine.id);
      return true;
    })
    .sort((a, b) => {
      const result =
        sort === "name"
          ? a.machine.name.localeCompare(b.machine.name, undefined, {
              numeric: true,
            })
          : (a[
              sort === "uptime"
                ? "uptime"
                : sort === "hashrate"
                  ? "hashrateHs"
                  : "load"
            ] ?? -1) -
            (b[
              sort === "uptime"
                ? "uptime"
                : sort === "hashrate"
                  ? "hashrateHs"
                  : "load"
            ] ?? -1);
      return ascending ? result : -result;
    });
  const visible = filtered.slice((page - 1) * pageSize, page * pageSize);
  const selectedOnPage =
    visible.length > 0 &&
    visible.every((row) => selected.includes(row.machine.id));
  const setFavorite = (id: string) => {
    const next = favorites.includes(id)
      ? favorites.filter((item) => item !== id)
      : [...favorites, id];
    setFavorites(next);
    try {
      localStorage.setItem("rigdeck:fleet-favorites", JSON.stringify(next));
    } catch {
      /* Keep the current view usable if storage is unavailable. */
    }
  };
  const setStatus = (value: Filter) => {
    setFilter(value);
    setPage(1);
  };
  const exportCsv = () => {
    const cell = (value: unknown) =>
      `"${String(value ?? "").replaceAll('"', '""')}"`;
    const csv = [
      [
        "机器",
        "SSH 地址",
        "状态",
        "运行时间",
        "CPU 使用率",
        "算力 H/s",
        "负载",
        "功耗 W",
      ]
        .map(cell)
        .join(","),
      ...filtered.map((row) =>
        [
          row.machine.name,
          row.machine.host,
          row.online ? "在线" : "离线",
          uptimeLabel(row.uptime),
          row.cpuPct,
          row.hashrateHs,
          row.load,
          row.powerW,
        ]
          .map(cell)
          .join(","),
      ),
    ].join("\r\n");
    const url = URL.createObjectURL(
      new Blob(["\ufeff", csv], { type: "text/csv;charset=utf-8" }),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = "rigdeck-machines.csv";
    link.click();
    URL.revokeObjectURL(url);
  };
  return (
    <div className={`hive-fleet ${wide ? "hive-fleet-wide" : ""}`}>
      <div className="hive-fleet-toolbar">
        <div className="hive-status-tabs">
          <input
            aria-label="选择本页所有矿机"
            type="checkbox"
            checked={selectedOnPage}
            onChange={() =>
              setSelected(
                selectedOnPage
                  ? selected.filter(
                      (id) => !visible.some((row) => row.machine.id === id),
                    )
                  : [
                      ...new Set([
                        ...selected,
                        ...visible.map((row) => row.machine.id),
                      ]),
                    ],
              )
            }
          />
          <div className="hive-filter-pills" role="group" aria-label="状态筛选">
            {(
              [
                ["all", "所有", rows.length],
                ["online", "在线", rows.filter((row) => row.online).length],
                ["offline", "掉线", rows.filter((row) => !row.online).length],
                ["problems", "有问题", rows.filter((row) => row.issue).length],
                ["favorites", "收藏", favorites.length],
                ...(selected.length ? [["selected", "已选择", selected.length]] : []),
              ] as [Filter, string, number][]
            ).map(([value, label, count]) => (
              <button
                key={value}
                className={filter === value ? "active" : ""}
                aria-pressed={filter === value}
                onClick={() => setStatus(value)}
              >
                {label}
                {count > 0 && <b>{count}</b>}
              </button>
            ))}
          </div>
        </div>
        <div className="hive-primary-actions">
          <Button
            className="hive-add-machine"
            type="primary"
            icon={<PlusOutlined />}
            onClick={() => setEditor(true)}
          >
            添加矿机
          </Button>
          <Button className="hive-add-machine" onClick={() => setImporting(true)}>
            批量添加
          </Button>
          <Button
            className="hive-show-filters"
            onClick={() => setShowFilters(!showFilters)}
          >
            {showFilters ? "隐藏筛选" : "筛选"}
          </Button>
        </div>
        <div className="hive-toolbar-actions">
          <button className="hive-help" onClick={() => setHelp(true)}>
            <InfoCircleOutlined /> <span>说明</span>
          </button>
          <Tooltip title="展开列表">
            <button
              className="hive-icon-button"
              aria-label="展开列表"
              onClick={() => setWide(!wide)}
            >
              <ArrowsAltOutlined />
            </button>
          </Tooltip>
          <Tooltip title={ascending ? "升序" : "降序"}>
            <button
              className="hive-icon-button"
              aria-label="切换排序方向"
              onClick={() => setAscending(!ascending)}
            >
              <SortAscendingOutlined />
            </button>
          </Tooltip>
          <Select
            size="small"
            aria-label="排序字段"
            value={sort}
            onChange={setSort}
            options={[
              { value: "name", label: "名称" },
              { value: "uptime", label: "矿工运行时间" },
              { value: "hashrate", label: "算力" },
              { value: "load", label: "负载" },
            ]}
          />
          <Tooltip title="导出当前列表 CSV">
            <button
              className="hive-icon-button"
              aria-label="导出矿机"
              onClick={exportCsv}
            >
              <DownloadOutlined />
            </button>
          </Tooltip>
          <div className="hive-view-switch" aria-label="列表视图">
            <button
              aria-label="紧凑列表"
              className={view === "dense" ? "active" : ""}
              onClick={() => setView("dense")}
            >
              <BarsOutlined />
            </button>
            <button
              aria-label="宽松列表"
              className={view === "comfortable" ? "active" : ""}
              onClick={() => setView("comfortable")}
            >
              <UnorderedListOutlined />
            </button>
            <button
              aria-label="网格视图"
              className={view === "grid" ? "active" : ""}
              onClick={() => setView("grid")}
            >
              <AppstoreOutlined />
            </button>
          </div>
        </div>
      </div>
      {showFilters && (
        <div className="hive-expanded-filters">
          <Input.Search
            placeholder="搜索名称、IP 或标签"
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setPage(1);
            }}
          />
          <Select
            allowClear
            placeholder="全部分组"
            value={group}
            onChange={(value) => {
              setGroup(value);
              setPage(1);
            }}
            options={groups.map((value) => ({ value, label: value }))}
          />
          <Button icon={<ReloadOutlined />} onClick={() => refresh()}>
            刷新
          </Button>
          <Button icon={<PlusOutlined />} onClick={() => setEditor(true)}>
            添加机器
          </Button>
        </div>
      )}
      <div className="hive-list-meta">
        <span>每页</span>
        <Select
          size="small"
          value={pageSize}
          onChange={(value) => {
            setPageSize(value);
            setPage(1);
          }}
          options={[25, 50, 100, 250].map((value) => ({
            value,
            label: String(value),
          }))}
        />
        <span className="hive-range">
          {filtered.length
            ? `${(page - 1) * pageSize + 1}–${Math.min(page * pageSize, filtered.length)}`
            : "0"}{" "}
          of {filtered.length}
        </span>
      </div>
      {selected.length > 0 && <BulkActions ids={selected} onBatch={setBatch} />}
      {telemetry.error && (
        <Alert
          type="warning"
          showIcon
          message="指标暂时无法刷新"
          description={telemetry.error.message}
        />
      )}
      <QueryState loading={machines.isLoading} error={machines.error}>
        {!visible.length ? (
          <div className="hive-fleet-empty">
            <Empty
              image={<IconEmptyRigs />}
              description={
                machines.data?.length
                    ? "暂无匹配的矿机"
                    : "还没有添加矿机"
              }
            />
            {!machines.data?.length && (
              <Space>
                <Button type="primary" icon={<PlusOutlined />} onClick={() => setEditor(true)}>
                  添加第一台矿机（可配置 BMC）
                </Button>
                <Button onClick={() => setImporting(true)}>按 IP 段批量添加</Button>
              </Space>
            )}
          </div>
        ) : (
          <div
            className={
              view === "grid"
                ? "hive-fleet-grid"
                : `hive-fleet-list ${view === "comfortable" ? "comfortable" : ""}`
            }
          >
            {visible.map((row) => (
              <div
                key={row.machine.id}
                className={`hive-machine-row ${!row.online ? "offline" : ""} ${row.issue ? "issue" : ""}`}
                onClick={(event) => {
                  if (
                    !(event.target as HTMLElement).closest("button, a, input")
                  )
                    navigate(`/machines/${row.machine.id}`);
                }}
              >
                <input
                  type="checkbox"
                  aria-label={`选择 ${row.machine.name}`}
                  checked={selected.includes(row.machine.id)}
                  onClick={(event) => event.stopPropagation()}
                  onChange={() =>
                    setSelected(
                      selected.includes(row.machine.id)
                        ? selected.filter((id) => id !== row.machine.id)
                        : [...selected, row.machine.id],
                    )
                  }
                />
                <div className="hive-machine-name">
                  <button
                    className={
                      favorites.includes(row.machine.id) ? "is-favorite" : ""
                    }
                    aria-label={`${favorites.includes(row.machine.id) ? "取消收藏" : "收藏"} ${row.machine.name}`}
                    onClick={() => setFavorite(row.machine.id)}
                  >
                    {favorites.includes(row.machine.id) ? (
                      <StarFilled />
                    ) : (
                      <StarOutlined />
                    )}
                  </button>
                  <Link to={`/machines/${row.machine.id}`} title={`${row.machine.name} · ${row.address ?? row.machine.host}`}>
                    {shortName(row.machine.name)}
                  </Link>
                  {row.machine.is_controller && (
                    <small className="hive-controller-mark">主控</small>
                  )}
                </div>
                <div className="hive-machine-alert">
                  {row.online && !row.instances.length && (
                    <Tooltip title="无飞行表">
                      <RocketFilled className="hive-no-fs" />
                    </Tooltip>
                  )}
                  {row.issue && (
                    <Tooltip
                      title={
                        <div className="hive-issue-list">
                          {row.issues.map((text) => (
                            <div key={text}>{text}</div>
                          ))}
                        </div>
                      }
                    >
                      <FireFilled />
                    </Tooltip>
                  )}
                </div>
                {row.online ? (
                  <div className="hive-machine-uptime" title="矿工运行时间；多个矿工取最近启动的实例">
                    {uptimeLabel(row.uptime)}
                  </div>
                ) : (
                  <div className="hive-machine-uptime offline" title="距最后一次上报">
                    {row.lastSeen ? uptimeLabel((serverNow() - Date.parse(row.lastSeen)) / 1000) : "—"}
                  </div>
                )}
                <div className="hive-machine-status" title={row.online ? "在线设备" : "离线"}>
                  {row.online ? <VendorMarks row={row} /> : "—"}
                </div>
                <MiningCell row={row} />
                <DevicesCell row={row} />
                <div className="hive-machine-load">
                  {row.online && (
                    <>
                      <b>LA</b> {row.load === undefined ? "—" : number(row.load, 2)}
                    </>
                  )}
                </div>
                <div className="hive-machine-fan">
                  {row.gpus.some((g) => typeof g.fan_pct === "number") ? (
                    <>
                      <b>FAN</b> {number(Math.max(...row.gpus.map((g) => g.fan_pct ?? 0)), 0)}%
                    </>
                  ) : row.gpus.length === 0 && row.cpuPct !== undefined ? (
                    <>
                      <b>CPU</b> {number(row.cpuPct, 0)}%
                    </>
                  ) : null}
                </div>
                <div className="hive-machine-power" title={`${row.powerLabel} · ${row.powerSource}`}>
                  {row.online && (
                    <>
                      <IconBolt size={12} />{" "}
                      {row.powerW === undefined ? "—" : row.powerW.toFixed(2)}
                      {row.powerW !== undefined && <small> W</small>}
                    </>
                  )}
                  {row.powerState === "Off" && <span className="hive-power-state">已关机</span>}
                </div>
              </div>
            ))}
          </div>
        )}
      </QueryState>
      {filtered.length > pageSize && (
        <div className="hive-bottom-pages">
          <Button
            size="small"
            disabled={page === 1}
            onClick={() => setPage(page - 1)}
          >
            上一页
          </Button>
          <span>
            {page} / {Math.ceil(filtered.length / pageSize)}
          </span>
          <Button
            size="small"
            disabled={page * pageSize >= filtered.length}
            onClick={() => setPage(page + 1)}
          >
            下一页
          </Button>
        </div>
      )}
      <MachineEditor open={editor} onClose={() => setEditor(false)} />
      <BatchAddModal open={importing} onClose={() => setImporting(false)} />
      {batch && (
        <BatchModal ids={selected} kind={batch} onClose={() => setBatch("")} />
      )}
      <Modal
        open={help}
        title="矿机列表说明"
        footer={null}
        onCancel={() => setHelp(false)}
      >
        <p>
          单击矿机行进入详情。状态、运行时间、算力、CPU、负载和功耗只显示已采集且新鲜的数据。
        </p>
        <p>
          勾选矿机后可批量应用飞行表、执行命令或控制矿工；星标仅保存在当前浏览器。
        </p>
      </Modal>
    </div>
  );
}
