import { useState } from "react";
import {
  Alert,
  Button,
  Dropdown,
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
  CodeOutlined,
  DownloadOutlined,
  DownOutlined,
  FireFilled,
  InfoCircleOutlined,
  PlusOutlined,
  ReloadOutlined,
  RocketOutlined,
  SettingOutlined,
  SortAscendingOutlined,
  StarFilled,
  StarOutlined,
  ThunderboltFilled,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { Link, useNavigate } from "react-router-dom";
import {
  data,
  isFresh,
  serverNow,
  latest,
  machinePower,
  useMachines,
  useRefresh,
  useFleetTelemetry,
  type Machine,
  type Observation,
} from "../../shared/api";
import { hashrate, number, QueryState } from "../../shared/ui";
import { BatchModal, MachineEditor } from "./controls";
import { BatchAddModal } from "./import";
import { CoinBadge, IconBolt, IconCpu, IconEmptyRigs, IconGpu, IconPickaxe, IconRig, IconServer, IconYuan } from "../../shared/icons";

type Filter = "all" | "online" | "offline" | "problems" | "favorites";
type View = "dense" | "comfortable" | "grid";
type Sort = "name" | "uptime" | "hashrate" | "load";

type GpuReading = {
  id: string;
  bus?: number | null;
  model?: string;
  temperature_c?: number | null;
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
    isFresh(mining) && Array.isArray(reportedInstances)
      ? reportedInstances
      : [];
  const active = instances.filter(
    (instance) =>
      instance.desired === "running" &&
      instance.process_alive &&
      serverNow() / 1000 - (instance.stats_observed_at ?? 0) < 35,
  );
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
    powerW: machinePower(system).watts,
    powerLabel: machinePower(system).label,
    powerSource: machinePower(system).source,
    bmcOnline: isFresh(power, 90),
    powerState: isFresh(power, 90) ? data(power?.data).state : undefined,
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
function running(instance: Record<string, any>) {
  return (
    instance.desired === "running" &&
    instance.process_alive &&
    serverNow() / 1000 - (instance.stats_observed_at ?? 0) < 35
  );
}
function coinOf(instance: Record<string, any>) {
  return String(instance.stats?.coin ?? instance.configured_coin ?? "?").toUpperCase();
}

export function FleetSummary() {
  const machines = useMachines();
  const telemetry = useFleetTelemetry();
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
  const dailyCost = powerReadings.length ? (totalWatts / 1000) * 24 * 0.56 : undefined;
  return (
    <section className="hive-farm-summary" aria-label="矿场统计">
      <div className="hive-farm-summary-inner">
        <div className="hive-stat hive-stat-green">
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
        {(rateCards.length ? rateCards : [undefined]).map((card) => {
          const parts = card ? rateParts(card.hs) : undefined;
          return (
            <div className="hive-stat hive-stat-rate" key={card ? `${card.coin}|${card.algorithm}` : "none"}>
              <div className="hive-stat-line">
                <strong>{parts?.value ?? "—"}</strong>
                {parts && <small>{parts.unit}</small>}
              </div>
              <span>
                {card ? (
                  <>
                    <CoinBadge coin={card.coin} size={13} />
                    <b className="hive-stat-coin">{card.coin}</b> {card.algorithm} <small>{card.rigs.size} 台</small>
                  </>
                ) : (
                  <>
                    <IconPickaxe size={13} />算力
                  </>
                )}
              </span>
            </div>
          );
        })}
        <div className="hive-stat">
          <div className="hive-stat-line">
            <strong>{rows.filter((row) => row.bmcOnline).length}</strong>
            <small>/{machines.data?.filter((machine) => !!machine.bmc).length ?? "—"}</small>
          </div>
          <span>
            <IconServer size={13} />BMC 在线
          </span>
        </div>
        <div
          className="hive-stat"
          title={`按当前软件功耗持续 24 小时 × 0.56 元/度估算，不含电源损耗；不是当天累计电费。`}
        >
          <div className="hive-stat-line">
            <strong>{dailyCost === undefined ? "—" : `¥${number(dailyCost, 2)}`}</strong>
            <small>/日</small>
          </div>
          <span>
            <IconYuan size={13} />预估电费 <small>0.56 元/度</small>
          </span>
        </div>
      </div>
    </section>
  );
}

function temperatureClass(value?: number) {
  if (value === undefined) return "";
  return value >= 85 ? "hot" : value >= 75 ? "warm" : "ok";
}

function tempLevel(value: number | null | undefined, limit: number) {
  if (typeof value !== "number") return "none";
  return value > limit ? "hot" : value > limit - 10 ? "warm" : "ok";
}

/** HiveOS device column: the CPU bar, then one tile per GPU coloured by temperature. */
function DevicesCell({ row }: { row: FleetRow }) {
  if (!row.online) return <div className="hive-capacity" />;
  const pct = row.cpuPct;
  // Per-GPU hashrate, matched by PCI bus number (or by position when a miner gives none).
  const gpuRates = new Map<string, string[]>();
  for (const instance of row.instances) {
    if (!running(instance) || !Array.isArray(instance.stats?.gpu_hs)) continue;
    instance.stats.gpu_hs.forEach((item: Record<string, any>, index: number) => {
      const gpu =
        typeof item.bus === "number" ? row.gpus.find((g) => g.bus === item.bus) : row.gpus[index];
      if (!gpu || typeof item.hs !== "number") return;
      gpuRates.set(gpu.id, [...(gpuRates.get(gpu.id) ?? []), `${coinOf(instance)} ${hashrate(item.hs)}`]);
    });
  }
  return (
    <div className="hive-capacity hive-devices">
      <Tooltip
        title={`CPU ${pct === undefined ? "使用率未采集" : `使用率 ${number(pct, 0)}%`}${row.cpuTemperature !== undefined ? ` · ${number(row.cpuTemperature, 0)}°C` : ""}${row.logicalCpus ? ` · ${row.logicalCpus} 线程` : ""}`}
      >
        <span className={`hive-cpu-cell ${row.gpus.length ? "compact" : ""}`}>
          <span className="hive-cpu-bar">
            <i style={{ width: `${Math.max(0, Math.min(100, pct ?? 0))}%` }} />
          </span>
          <span className="hive-cpu-pct">{pct === undefined ? "—" : `${number(pct, 0)}%`}</span>
          {row.cpuTemperature !== undefined && (
            <span className={`hive-temp ${temperatureClass(row.cpuTemperature)}`}>
              {number(row.cpuTemperature, 0)}°
            </span>
          )}
          {!row.gpus.length && row.logicalCpus !== undefined && <small>{row.logicalCpus}T</small>}
        </span>
      </Tooltip>
      {row.gpus.length > 0 && (
        <span className="hive-gpu-tiles" aria-label={`${row.gpus.length} 块 GPU`}>
          {row.gpus.map((gpu, index) => {
            const level = tempLevel(gpu.temperature_c, row.gpuTempLimit);
            const lines = [
              `GPU ${index} · ${gpu.model ?? gpu.id}`,
              ...(gpuRates.get(gpu.id) ?? []),
              typeof gpu.temperature_c === "number" ? `温度 ${number(gpu.temperature_c, 0)}°C` : "温度未采集",
              typeof gpu.fan_pct === "number" ? `风扇 ${number(gpu.fan_pct, 0)}%` : undefined,
              typeof gpu.power_w === "number" ? `功耗 ${number(gpu.power_w, 0)} W` : undefined,
              typeof gpu.util_pct === "number" ? `负载 ${number(gpu.util_pct, 0)}%` : undefined,
              level === "hot" ? `超过高温阈值 ${row.gpuTempLimit}°C` : undefined,
            ].filter(Boolean);
            return (
              <Tooltip key={gpu.id} title={<div className="hive-issue-list">{lines.map((l) => <div key={l}>{l}</div>)}</div>}>
                <i className={`hive-gpu ${level}`}>
                  {typeof gpu.temperature_c === "number" ? number(gpu.temperature_c, 0) : "—"}
                </i>
              </Tooltip>
            );
          })}
        </span>
      )}
    </div>
  );
}

/** One line per miner, like HiveOS: coin, hashrate, device, shares, then the miner name. */
function MiningCell({ row }: { row: FleetRow }) {
  const shown = row.instances.filter((item) => item.desired === "running");
  const instances = shown.length ? shown : row.instances.slice(0, 1);
  if (!instances.length)
    return (
      <div className="hive-mining-cell">
        <span className="hive-row-muted">{row.online ? "未运行矿工" : "—"}</span>
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
  const [selected, setSelected] = useState<string[]>([]);
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
      {selected.length > 0 && (
        <div className="hive-batch-bar">
          <strong>已选 {selected.length} 台</strong>
          <Button
            size="small"
            icon={<RocketOutlined />}
            onClick={() => setBatch("apply")}
          >
            应用飞行表
          </Button>
          <Button
            size="small"
            icon={<CodeOutlined />}
            onClick={() => setBatch("command")}
          >
            执行命令
          </Button>
          <Button size="small" onClick={() => setBatch("miner")}>
            矿工控制
          </Button>
          <Link to={`/terminals?ids=${selected.join(",")}`}>
            <Button size="small">多机终端</Button>
          </Link>
          <Dropdown
            menu={{
              items: [
                {
                  key: "bootstrap",
                  label: "部署 / 更新运行层",
                  icon: <SettingOutlined />,
                },
                {
                  key: "power",
                  label: "BMC 电源操作",
                  icon: <ThunderboltFilled />,
                },
              ],
              onClick: ({ key }) => setBatch(key),
            }}
          >
            <Button size="small">
              更多 <DownOutlined />
            </Button>
          </Dropdown>
          <Button size="small" type="text" onClick={() => setSelected([])}>
            取消选择
          </Button>
        </div>
      )}
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
                  <Link to={`/machines/${row.machine.id}`}>
                    {row.machine.name}
                  </Link>
                  {row.machine.is_controller && (
                    <small className="hive-controller-mark">主控</small>
                  )}
                  <small className="hive-machine-ip">
                    {row.address ?? row.machine.host}
                  </small>
                </div>
                <div className="hive-machine-alert">
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
                <div className="hive-machine-uptime" title="矿工运行时间；多个矿工取最近启动的实例">
                  {uptimeLabel(row.uptime)}
                </div>
                <div
                  className={`hive-machine-status ${row.online ? "online" : ""}`}
                  title={row.online ? "CPU 节点在线" : "SSH 指标离线"}
                >
                  {row.online ? (
                    <>
                      <IconCpu size={13} />
                      {row.gpus.length > 0 && <IconGpu size={13} />}
                    </>
                  ) : (
                    "—"
                  )}
                </div>
                <MiningCell row={row} />
                <DevicesCell row={row} />
                <div className="hive-machine-load">
                  {row.load === undefined
                    ? "LA —"
                    : `LA ${number(row.load, 2)}`}
                </div>
                <div className="hive-machine-power" title={`${row.powerLabel} · ${row.powerSource}`}>
                  <IconBolt size={12} />{" "}
                  {row.powerW === undefined
                    ? "—"
                    : `${number(row.powerW, 0)} W`}
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
