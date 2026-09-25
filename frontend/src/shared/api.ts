import createClient from "openapi-fetch";
import type { components, paths } from "./schema";
import { useQuery, useQueryClient } from "@tanstack/react-query";

export type Machine = components["schemas"]["Machine"];
export type MachineInput = components["schemas"]["MachineInput"];
export type CatalogItem = components["schemas"]["CatalogItem"];
export type CatalogInput = components["schemas"]["CatalogInput"];
export type FlightSheet = components["schemas"]["FlightSheet"];
export type FlightInput = components["schemas"]["FlightInput"];
export type Job = components["schemas"]["Job"];
export type JobInput = components["schemas"]["JobInput"];
export type Observation = components["schemas"]["Observation"];
export type Session = components["schemas"]["Session"];
export type Adapter = components["schemas"]["AdapterDescription"];
export type TokenInfo = components["schemas"]["TokenInfo"];
export type AuditEvent = components["schemas"]["AuditEvent"];
export type MachineMessage = components["schemas"]["MachineMessage"];
export type MinerLog = components["schemas"]["MinerLog"];
export type Data = Record<string, any>;

let csrf = "";
let serverOffset = 0;
export const serverNow = () => Date.now() + serverOffset;
export const setCsrf = (value: string) => {
  csrf = value;
};
export const getCsrf = () => csrf;
export const client = createClient<paths>({
  baseUrl: "",
  credentials: "same-origin",
});
client.use({
  onRequest({ request }) {
    if (!["GET", "HEAD"].includes(request.method))
      request.headers.set("x-csrf-token", csrf);
    return request;
  },
  async onResponse({ response, request }) {
    const serverDate = response.headers.get("x-rigdeck-time");
    if (serverDate) serverOffset = Number(serverDate) - Date.now();
    if (response.status === 401 && !/\/login$/.test(new URL(request.url).pathname) && (!/\/me$/.test(new URL(request.url).pathname) || csrf)) {
      csrf = "";
      window.dispatchEvent(new Event("rigdeck-session-expired"));
    }
    if (!response.ok) {
      let message = `HTTP ${response.status}`;
      try {
        message = (await response.clone().json()).error ?? message;
      } catch {
        /* Non-JSON gateway errors retain HTTP status. */
      }
      throw new Error(message);
    }
    return response;
  },
});
export async function unwrap<T>(
  promise: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, response } = await promise;
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return data as T;
}
// Generic queries are limited to the generated path map. Mutations below use generated client signatures.
export function useMachines() {
  return useQuery({
    queryKey: ["machines"],
    staleTime: 30000,
    gcTime: 30 * 60 * 1000,
    queryFn: ({ signal }) => unwrap(client.GET("/api/v1/machines", { signal })),
    refetchInterval: 10000,
  });
}
// The list never needs full BMC event logs or hardware inventories.
export function useFleetTelemetry() {
  return useQuery({
    queryKey: ["fleet-telemetry"],
    queryFn: ({ signal }) => unwrap(client.GET("/api/v1/telemetry", {
      signal,
      params: { query: { summary: true } },
    })),
    staleTime: 10000,
    gcTime: 30 * 60 * 1000,
    refetchInterval: 10000,
  });
}
export function useTelemetry(machine_id?: string) {
  return useQuery({
    queryKey: ["telemetry", machine_id],
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/telemetry", { signal, params: { query: { machine_id, summary: !machine_id } } }),
      ),
    refetchInterval: 10000,
  });
}
export function useCatalog(kind: string) {
  return useQuery({
    queryKey: ["catalog", kind],
    queryFn: () =>
      unwrap(
        client.GET("/api/v1/catalog/{kind}", { params: { path: { kind } } }),
      ),
  });
}
export function useFlights() {
  return useQuery({
    queryKey: ["flights"],
    queryFn: () => unwrap(client.GET("/api/v1/flight-sheets")),
  });
}
export function useAdapters() {
  return useQuery({
    queryKey: ["adapters"],
    queryFn: () => unwrap(client.GET("/api/v1/adapters")),
  });
}
export function useRefresh() {
  const q = useQueryClient();
  return () => q.invalidateQueries();
}
export function data(value: unknown): Data {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Data)
    : {};
}
// Rows for 1 000 machines × 4 kinds are looked up once per machine and kind on every refresh.
// A linear `find` made that O(N²) (~16 M comparisons); index each response array once instead.
const observationIndex = new WeakMap<Observation[], Map<string, Observation>>();
export function latest(
  observations: Observation[] | undefined,
  id: string,
  kind: string,
) {
  if (!observations) return undefined;
  let index = observationIndex.get(observations);
  if (!index) {
    index = new Map(observations.map((o) => [`${o.machine_id}\u0000${o.kind}`, o]));
    observationIndex.set(observations, index);
  }
  return index.get(`${id}\u0000${kind}`);
}
export function isFresh(o: Observation | undefined, seconds = 35) {
  return (
    !!o &&
    !o.error &&
    serverNow() - new Date(o.observed_at).getTime() < seconds * 1000
  );
}
export function measuredPower(
  power: Observation | undefined,
  sensors: Observation | undefined,
): number | undefined {
  const dcmi = data(power?.data).power_w;
  if (isFresh(power, 90) && typeof dcmi === "number" && dcmi > 0)
    return dcmi;
  if (!isFresh(sensors, 150)) return undefined;
  const items = data(sensors?.data).items;
  if (!Array.isArray(items)) return undefined;
  const meter = items.find((item) => {
    const name = String(item?.name ?? "");
    const unit = String(item?.unit ?? "");
    return (
      (typeof item?.raw?.PowerConsumedWatts === "number" ||
        /power consumed|power consumption|system power|总功耗/i.test(name)) &&
      /^(w|watts?)$/i.test(unit)
    );
  });
  const reading = meter?.raw?.PowerConsumedWatts ?? meter?.reading;
  const value = typeof reading === "number" ? reading : Number(reading);
  return Number.isFinite(value) && value > 0 ? value : undefined;
}
const watt = (value: unknown) =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
/** Software power: CPU packages (RAPL) plus each GPU's own reading, like HiveOS's worker power. */
export function machinePower(system: Observation | undefined) {
  const d = data(system?.data);
  const fresh = isFresh(system);
  const cpu = fresh ? watt(d.cpu_power_w) : undefined;
  const gpuReadings: number[] = fresh
    ? (Array.isArray(d.gpus) ? d.gpus : []).map((g: Data) => watt(g.power_w)).filter((w: number | undefined): w is number => w !== undefined)
    : [];
  const gpu = gpuReadings.length ? gpuReadings.reduce((a, b) => a + b, 0) : undefined;
  const watts = cpu === undefined && gpu === undefined ? undefined : (cpu ?? 0) + (gpu ?? 0);
  const label = gpu === undefined ? "CPU 功耗" : cpu === undefined ? "GPU 功耗" : "CPU+GPU 功耗";
  const parts = [
    cpu !== undefined && `CPU ${Math.round(cpu)} W（RAPL）`,
    gpu !== undefined && `GPU ${Math.round(gpu)} W（${gpuReadings.length} 块显卡读数）`,
  ].filter(Boolean);
  return {
    watts,
    cpu,
    gpu,
    label,
    source: `${parts.join(" + ") || "未采集"}；软件读数，不含内存、风扇及电源损耗`,
  };
}
export async function submitJob(
  input: Omit<JobInput, "idempotency_key">,
  key = crypto.randomUUID(),
) {
  return unwrap(
    client.POST("/api/v1/jobs", { body: { ...input, idempotency_key: key } }),
  );
}
