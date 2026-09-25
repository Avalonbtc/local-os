// HiveOS overclocking fields and their nvidia-oc.conf / amd-oc.conf names. Values are typed the
// HiveOS way: one number per card separated by spaces, a single number for all cards.
import type { components } from "../../shared/schema";

export type OcConfig = components["schemas"]["GpuOcConfig"];
export type Vendor = "nvidia" | "amd";
export type Field = { key: string; conf: string; label: string; unit?: string; help: string; advanced?: boolean };

export const NVIDIA_FIELDS: Field[] = [
  { key: "clock", conf: "CLOCK", label: "核心", unit: "MHz", help: "核心偏移；大于 500 为锁定核心频率（如 1500）" },
  { key: "mem", conf: "MEM", label: "显存", unit: "MHz", help: "显存偏移，HiveOS/Linux 数值（Windows Afterburner 的 2 倍）" },
  { key: "plimit", conf: "PLIMIT", label: "功耗墙", unit: "W", help: "0 = 驱动默认" },
  { key: "fan", conf: "FAN", label: "风扇", unit: "%", help: "0 = 自动" },
];
export const AMD_FIELDS: Field[] = [
  { key: "core_clock", conf: "CORE_CLOCK", label: "核心频率", unit: "MHz", help: "0 = 不修改" },
  { key: "core_state", conf: "CORE_STATE", label: "核心状态", help: "DPM 状态，0 = 自动" },
  { key: "core_vddc", conf: "CORE_VDDC", label: "核心电压", unit: "mV", help: "0 = 不修改；RDNA2/3 不支持绝对电压" },
  { key: "mem_clock", conf: "MEM_CLOCK", label: "显存频率", unit: "MHz", help: "0 = 不修改" },
  { key: "mem_state", conf: "MEM_STATE", label: "显存状态", help: "0 = 自动" },
  { key: "fan", conf: "FAN", label: "风扇", unit: "%", help: "0 = 自动" },
  { key: "pl", conf: "PL", label: "功耗墙", unit: "W", help: "0 = 驱动默认" },
  { key: "ref", conf: "REF", label: "REF", help: "需要矿机上安装 amdmemtweak" },
  { key: "mvdd", conf: "MVDD", label: "MVDD", unit: "mV", help: "需要改 PowerPlay 表，本版本仅保存不写入", advanced: true },
  { key: "vddci", conf: "VDDCI", label: "VDDCI", unit: "mV", help: "需要改 PowerPlay 表，本版本仅保存不写入", advanced: true },
  { key: "soc_clk", conf: "SOCCLK", label: "SOC 频率", unit: "MHz", help: "需要改 PowerPlay 表，本版本仅保存不写入", advanced: true },
  { key: "soc_vdd_max", conf: "SOCVDDMAX", label: "SOC 电压", unit: "mV", help: "需要改 PowerPlay 表，本版本仅保存不写入", advanced: true },
];
export const FIELDS: Record<Vendor, Field[]> = { nvidia: NVIDIA_FIELDS, amd: AMD_FIELDS };

/** Form state: the text the user typed per field, exactly like HiveOS's inputs. */
export type OcText = Record<Vendor, Record<string, string>> & { running_delay: string };

export const emptyText = (): OcText => ({ nvidia: {}, amd: {}, running_delay: "" });

export function parseList(text: string | undefined): number[] {
  const parts = (text ?? "").trim().split(/[\s,]+/).filter(Boolean);
  return parts.map((part) => {
    if (!/^-?\d+$/.test(part)) throw new Error(`“${part}” 不是整数`);
    return Number(part);
  });
}

export function textToConfig(text: OcText): OcConfig {
  const config: Record<string, Record<string, unknown>> = {};
  for (const vendor of ["nvidia", "amd"] as Vendor[]) {
    const section: Record<string, unknown> = {};
    for (const field of FIELDS[vendor]) {
      try {
        const values = parseList(text[vendor][field.key]);
        if (values.length) section[field.key] = values;
      } catch (error) {
        throw new Error(`${vendor === "nvidia" ? "NVIDIA" : "AMD"} ${field.label}：${(error as Error).message}`);
      }
    }
    if (vendor === "nvidia" && text.running_delay.trim()) section.running_delay = parseList(text.running_delay)[0] ?? 0;
    if (Object.keys(section).length) config[vendor] = section;
  }
  return config as OcConfig;
}

export function configToText(config: OcConfig | undefined | null): OcText {
  const text = emptyText();
  const source = (config ?? {}) as Record<string, Record<string, unknown> | undefined>;
  for (const vendor of ["nvidia", "amd"] as Vendor[]) {
    for (const field of FIELDS[vendor]) {
      const values = source[vendor]?.[field.key];
      if (Array.isArray(values) && values.length) text[vendor][field.key] = values.join(" ");
    }
  }
  const delay = source.nvidia?.running_delay;
  if (typeof delay === "number" && delay > 0) text.running_delay = String(delay);
  return text;
}

/** Reads a pasted nvidia-oc.conf and/or amd-oc.conf. Unknown keys are listed, not guessed. */
export function importHiveConf(conf: string): { text: OcText; ignored: string[] } {
  const text = emptyText();
  const ignored: string[] = [];
  const amdOnly = new Set(AMD_FIELDS.map((f) => f.conf).filter((c) => !["FAN"].includes(c)));
  const lines = conf.split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith("#"));
  const pairs = lines.flatMap((line) => {
    const match = line.match(/^(?:export\s+)?([A-Z_][A-Z0-9_]*)=(?:"([^"]*)"|'([^']*)'|(\S*))/);
    return match ? [[match[1], (match[2] ?? match[3] ?? match[4] ?? "").trim()] as const] : [];
  });
  // FAN is in both files: it belongs to AMD when the paste contains AMD-only keys.
  const isAmd = pairs.some(([key]) => amdOnly.has(key));
  const isNvidia = pairs.some(([key]) => ["CLOCK", "MEM", "PLIMIT"].includes(key));
  for (const [key, value] of pairs) {
    if (key === "RUNNING_DELAY") {
      text.running_delay = value;
      continue;
    }
    const nvidia = NVIDIA_FIELDS.find((f) => f.conf === key);
    const amd = AMD_FIELDS.find((f) => f.conf === key);
    if (key === "FAN") {
      if (isNvidia || !isAmd) text.nvidia.fan = value;
      if (isAmd) text.amd.fan = value;
    } else if (nvidia) text.nvidia[nvidia.key] = value;
    else if (amd) text.amd[amd.key] = value;
    else if (value) ignored.push(key);
  }
  return { text, ignored };
}

export function exportHiveConf(text: OcText, vendor: Vendor): string {
  const lines = FIELDS[vendor].map((f) => `${f.conf}="${(text[vendor][f.key] ?? "").trim()}"`);
  if (vendor === "nvidia") lines.push(`RUNNING_DELAY="${text.running_delay.trim()}"`);
  return lines.join("\n");
}

/** HiveOS padding: missing cards reuse the last value. */
export function valueFor(text: string | undefined, index: number): number | undefined {
  try {
    const values = parseList(text);
    return values.length ? values[Math.min(index, values.length - 1)] : undefined;
  } catch {
    return undefined;
  }
}

export function summary(config: OcConfig | undefined | null): string {
  const text = configToText(config);
  const parts: string[] = [];
  for (const vendor of ["nvidia", "amd"] as Vendor[]) {
    const items = FIELDS[vendor].filter((f) => text[vendor][f.key]).map((f) => `${f.label} ${text[vendor][f.key]}`);
    if (items.length) parts.push(`${vendor === "nvidia" ? "NVIDIA" : "AMD"}：${items.join("，")}`);
  }
  return parts.join(" · ") || "未设置";
}
