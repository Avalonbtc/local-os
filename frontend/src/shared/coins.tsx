/// <reference types="vite/client" />
// Coin icons like HiveOS shows next to every coin. Order: the controller's icon library (icons
// added on the console or dropped into its icon directory), the bundled cryptocurrency-icons set
// (CC0), then a letter badge. Browsers only ever load icons from the controller.
import { useQuery } from "@tanstack/react-query";
import { client, unwrap, type CoinIconList } from "./api";

const bundled: Record<string, string> = Object.fromEntries(
  Object.entries(
    import.meta.glob("../../node_modules/cryptocurrency-icons/svg/color/*.svg", {
      eager: true,
      query: "?no-inline",
      import: "default",
    }) as Record<string, string>,
  )
    .map(([path, url]) => [path.split("/").pop()!.replace(".svg", "").toUpperCase(), url] as const)
    .filter(([symbol]) => symbol !== "GENERIC"),
);

export const hasBundledIcon = (coin: string) => coin.toUpperCase() in bundled;

const ICON_LIST_KEY = "rigdeck:coin-icons";
function rememberedIcons(): CoinIconList | undefined {
  try {
    const saved = JSON.parse(localStorage.getItem(ICON_LIST_KEY) ?? "null");
    return saved && Array.isArray(saved.icons) ? saved : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The controller's icon list. The last list is remembered in the browser so icons show at once
 * when a page opens (no letter badge first); it is refreshed in the background right away.
 */
export function useCoinIcons() {
  return useQuery<CoinIconList>({
    queryKey: ["coin-icons"],
    queryFn: async ({ signal }) => {
      const list = await unwrap(client.GET("/api/v1/coin-icons", { signal }));
      try {
        localStorage.setItem(ICON_LIST_KEY, JSON.stringify(list));
      } catch {
        // Private mode or full storage: icons still load, just not instantly next time.
      }
      return list;
    },
    initialData: rememberedIcons,
    initialDataUpdatedAt: 0,
    staleTime: 5 * 60 * 1000,
  });
}

/** The controller URL of a library icon; the timestamp busts the browser cache after a change. */
export const libraryIconUrl = (symbol: string, updatedAt: string) =>
  `/api/v1/coin-icons/${encodeURIComponent(symbol)}?v=${Date.parse(updatedAt) || 0}`;

/** The image URL for a coin, or undefined when only the letter badge is available. */
export function useCoinIcon(coin: string) {
  const library = useCoinIcons().data?.icons;
  const symbol = coin.toUpperCase();
  const own = library?.find((icon) => icon.symbol === symbol);
  return own ? libraryIconUrl(symbol, own.updated_at) : bundled[symbol];
}

function LetterBadge({ symbol, size }: { symbol: string; size: number }) {
  let hash = 0;
  for (const ch of symbol) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return (
    <span
      className="rd-coin"
      style={{ width: size, height: size, background: `hsl(${hash % 360} 55% 45%)`, fontSize: Math.round(size * 0.56) }}
      title={symbol}
    >
      {symbol.slice(0, 1)}
    </span>
  );
}

export function CoinBadge({ coin, size = 16 }: { coin: string; size?: number }) {
  const symbol = coin.toUpperCase();
  const icon = useCoinIcon(symbol);
  if (!icon || symbol === "?" || symbol === "未知币种") return <LetterBadge symbol={symbol} size={size} />;
  return <img className="rd-coin-img" src={icon} width={size} height={size} alt={symbol} title={symbol} />;
}

/** HiveOS coin label: icon followed by the symbol. */
export function CoinLabel({ coin, size = 16 }: { coin: string; size?: number }) {
  return (
    <span className="rd-coin-label">
      <CoinBadge coin={coin} size={size} />
      <span>{coin.toUpperCase()}</span>
    </span>
  );
}

const UPLOAD_TYPES = ["image/svg+xml", "image/png", "image/webp", "image/jpeg"];
const UPLOAD_MAX = 256 * 1024;

/**
 * Body for uploading an image file to the icon library: small PNG / JPEG / WebP / SVG files go
 * as they are, anything else is redrawn as a 128 px PNG first.
 */
export async function iconUpload(file: File): Promise<{ content_type: string; data_base64: string }> {
  const base64 = (blob: Blob) =>
    new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(blob);
    });
  if (UPLOAD_TYPES.includes(file.type) && file.size <= UPLOAD_MAX)
    return { content_type: file.type, data_base64: await base64(file) };
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 128;
    const scale = Math.min(128 / image.naturalWidth, 128 / image.naturalHeight);
    const w = image.naturalWidth * scale;
    const h = image.naturalHeight * scale;
    canvas.getContext("2d")!.drawImage(image, (128 - w) / 2, (128 - h) / 2, w, h);
    const png = await new Promise<Blob>((resolve, reject) =>
      canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("无法转换图片"))), "image/png"),
    );
    return { content_type: "image/png", data_base64: await base64(png) };
  } finally {
    URL.revokeObjectURL(url);
  }
}
