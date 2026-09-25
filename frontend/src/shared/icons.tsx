// Small line icons for the HiveOS-style pages (original drawings, 24×24 grid, currentColor).
import type { CSSProperties, ReactNode } from "react";

type IconProps = { size?: number; className?: string; style?: CSSProperties; title?: string };
function Svg({ size = 16, className, style, title, children }: IconProps & { children: ReactNode }) {
  return (
    <svg
      className={`rd-icon ${className ?? ""}`}
      style={style}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={title ? "img" : undefined}
      aria-hidden={title ? undefined : true}
      aria-label={title}
    >
      {children}
    </svg>
  );
}

export const IconCpu = (p: IconProps) => (
  <Svg {...p}>
    <rect x="6" y="6" width="12" height="12" rx="1.5" />
    <rect x="9.5" y="9.5" width="5" height="5" rx="0.5" />
    <path d="M9 3v3M12 3v3M15 3v3M9 18v3M12 18v3M15 18v3M3 9h3M3 12h3M3 15h3M18 9h3M18 12h3M18 15h3" />
  </Svg>
);
export const IconGpu = (p: IconProps) => (
  <Svg {...p}>
    <path d="M2 6h17a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H4" />
    <path d="M4 6v14M4 20h3" />
    <circle cx="10" cy="11.5" r="3" />
    <circle cx="16.5" cy="11.5" r="2" />
    <path d="M8 17v3M11 17v3M14 17v3" />
  </Svg>
);
export const IconThermometer = (p: IconProps) => (
  <Svg {...p}>
    <path d="M14 14.8V5a2 2 0 0 0-4 0v9.8a4 4 0 1 0 4 0z" />
    <path d="M12 9v7.5" />
  </Svg>
);
export const IconFan = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="1.8" />
    <path d="M12 10.2c-.6-3.2.4-6.2 3-6.2 2.2 0 2.8 3 .6 5l-1.8 1.4" />
    <path d="M13.6 12.9c3 1.1 5 3.6 3.7 5.8-1.1 1.9-4 .9-4.6-2l-.3-2.3" />
    <path d="M10.4 12.9c-2.5 2.1-5.6 2.7-6.9.5-1.1-1.9 1.2-3.9 4-3l2.1.9" />
  </Svg>
);
export const IconBolt = (p: IconProps) => (
  <Svg {...p}>
    <path d="M13 2 4.5 13.5H11L10 22l8.5-11.5H12z" />
  </Svg>
);
export const IconMemory = (p: IconProps) => (
  <Svg {...p}>
    <rect x="2.5" y="7" width="19" height="9" rx="1" />
    <path d="M6 10v3M9.5 10v3M13 10v3M16.5 10v3M5 16v3M19 16v3M9 16v2M15 16v2" />
  </Svg>
);
export const IconGauge = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 17a8 8 0 1 1 16 0" />
    <path d="M12 17l4-5" />
    <path d="M4 17h2M18 17h2M12 9V7M7 11.5 5.8 10.3M17 11.5l1.2-1.2" />
  </Svg>
);
export const IconClock = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3 2" />
  </Svg>
);
export const IconPickaxe = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 7c4-3.5 9.5-4 14-1" />
    <path d="M14.5 5.5 4 20" />
    <path d="M17 8.5c1.3 1.6 2 3.6 2 5.5" />
    <path d="M13 4.5l6 6" />
  </Svg>
);
export const IconNetwork = (p: IconProps) => (
  <Svg {...p}>
    <rect x="9" y="3" width="6" height="5" rx="1" />
    <rect x="3" y="16" width="6" height="5" rx="1" />
    <rect x="15" y="16" width="6" height="5" rx="1" />
    <path d="M12 8v4M6 16v-4h12v4" />
  </Svg>
);
export const IconLayers = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 3 3 8l9 5 9-5z" />
    <path d="m3 12 9 5 9-5" />
    <path d="m3 16 9 5 9-5" />
  </Svg>
);
export const IconChipDriver = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v3M12 19v3M4.9 4.9 7 7M17 17l2.1 2.1M2 12h3M19 12h3M4.9 19.1 7 17M17 7l2.1-2.1" />
  </Svg>
);
export const IconServer = (p: IconProps) => (
  <Svg {...p}>
    <rect x="3" y="4" width="18" height="7" rx="1.5" />
    <rect x="3" y="13" width="18" height="7" rx="1.5" />
    <path d="M7 7.5h.01M7 16.5h.01M11 7.5h6M11 16.5h6" />
  </Svg>
);
export const IconRig = (p: IconProps) => (
  <Svg {...p}>
    <rect x="2.5" y="5" width="19" height="14" rx="1.5" />
    <path d="M6 9h5v6H6zM14 9h4M14 12h4M14 15h4" />
  </Svg>
);
export const IconTrash = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6" />
  </Svg>
);
export const IconClose = (p: IconProps) => (
  <Svg {...p}>
    <path d="M6 6l12 12M18 6 6 18" />
  </Svg>
);
export const IconTune = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 17a8 8 0 1 1 16 0" />
    <path d="M12 17l-3.5-5" />
    <path d="M17 20h4M19 18v4" />
  </Svg>
);
export const IconYuan = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="9" />
    <path d="M8.5 7 12 12l3.5-5M12 12v5.5M9 12.5h6M9 15h6" />
  </Svg>
);
export const IconRestart = (p: IconProps) => (
  <Svg {...p}>
    <path d="M20 12a8 8 0 1 1-2.3-5.6" />
    <path d="M20 4v5h-5" />
  </Svg>
);
export const IconPlayStop = (p: IconProps) => (
  <Svg {...p}>
    <path d="M4 5v14l8-7z" />
    <rect x="15" y="6" width="5" height="12" rx="1" />
  </Svg>
);
export const IconRocket = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 2c3.5 2.5 5 6.5 4.5 11l-2.5 3h-4l-2.5-3C7 8.5 8.5 4.5 12 2z" />
    <circle cx="12" cy="9" r="1.8" />
    <path d="M7.5 13 4.5 16l1 3 3.5-2M16.5 13l3 3-1 3-3.5-2M10.5 19.5 12 22l1.5-2.5" />
  </Svg>
);
export const IconLog = (p: IconProps) => (
  <Svg {...p}>
    <path d="M6 2.5h8l4.5 4.5v14.5H6z" />
    <path d="M14 2.5V7h4.5M9 11h6M9 14.5h6M9 18h4" />
  </Svg>
);
export const IconScreen = (p: IconProps) => (
  <Svg {...p}>
    <rect x="2.5" y="4" width="19" height="13" rx="1.5" />
    <path d="M8 21h8M12 17v4M6.5 8.5l2.5 2.5-2.5 2.5M11 13.5h4" />
  </Svg>
);
export const IconTerminal = (p: IconProps) => (
  <Svg {...p}>
    <path d="m4 7 5 5-5 5M11 18h9" />
  </Svg>
);
export const IconPower = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 3v8" />
    <path d="M6.3 6.8a8 8 0 1 0 11.4 0" />
  </Svg>
);
export const IconPlug = (p: IconProps) => (
  <Svg {...p}>
    <path d="M9 2v5M15 2v5M6 7h12v4a6 6 0 0 1-12 0zM12 17v5" />
  </Svg>
);
export const IconSettings = (p: IconProps) => (
  <Svg {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
  </Svg>
);
export const IconBiosChip = (p: IconProps) => (
  <Svg {...p}>
    <rect x="5" y="5" width="14" height="14" rx="1.5" />
    <path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3M9 10h6M9 14h3" />
  </Svg>
);
export const IconUpload = (p: IconProps) => (
  <Svg {...p}>
    <path d="M12 16V4M7 9l5-5 5 5M4 16v4h16v-4" />
  </Svg>
);
export const IconEmptyRigs = ({ size = 120 }: { size?: number }) => (
  <svg className="rd-illustration" width={size} height={(size * 3) / 4} viewBox="0 0 160 120" fill="none" aria-hidden>
    <rect x="18" y="30" width="124" height="30" rx="4" fill="#2b3239" stroke="#4a5560" />
    <rect x="18" y="66" width="124" height="30" rx="4" fill="#2b3239" stroke="#4a5560" />
    <circle cx="34" cy="45" r="4" fill="#83c240" />
    <circle cx="34" cy="81" r="4" fill="#45515a" />
    <path d="M50 41h52M50 49h34M50 77h52M50 85h34" stroke="#5b6570" strokeWidth="3" strokeLinecap="round" />
    <path d="M118 38v14M126 38v14M118 74v14M126 74v14" stroke="#68bbf3" strokeWidth="3" strokeLinecap="round" />
    <path d="M80 104v10M72 110h16" stroke="#4a5560" strokeWidth="3" strokeLinecap="round" />
  </svg>
);

/** A coin badge coloured from the symbol (no third-party logos). */
export function CoinBadge({ coin, size = 16 }: { coin: string; size?: number }) {
  const symbol = coin.toUpperCase();
  let hash = 0;
  for (const ch of symbol) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  const known: Record<string, string> = { XMR: "#f26822", ETH: "#8c8cf0", ETC: "#3ab83a", RVN: "#384182", BTC: "#f7931a", ZEPH: "#6aa8ff", KAS: "#49eacb" };
  const color = known[symbol] ?? `hsl(${hash % 360} 55% 52%)`;
  return (
    <span className="rd-coin" style={{ width: size, height: size, background: color, fontSize: Math.round(size * 0.56) }} title={symbol}>
      {symbol.slice(0, 1)}
    </span>
  );
}
