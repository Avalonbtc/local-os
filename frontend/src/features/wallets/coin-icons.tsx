// The controller's coin icon library: upload a file, give a link (the controller downloads it
// once), or search CoinGecko on the controller and pick the right coin. Files dropped into the
// controller's icon directory show up here too. Browsers only load icons from the controller.
import { useRef, useState } from "react";
import { App, Button, Empty, Input, Modal, Space, Spin, Typography } from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { client, unwrap, type CoinCandidate, type CoinIconInput } from "../../shared/api";
import { CoinBadge, hasBundledIcon, iconUpload, useCoinIcons } from "../../shared/coins";

const SOURCE: Record<string, string> = {
  upload: "上传",
  url: "图片链接",
  coingecko: "CoinGecko",
  directory: "主控目录",
};

export function CoinIconsDialog({ open, symbols, onClose }: { open: boolean; symbols: string[]; onClose: () => void }) {
  const { message, modal } = App.useApp();
  const queryClient = useQueryClient();
  const library = useCoinIcons();
  const input = useRef<HTMLInputElement>(null);
  const [target, setTarget] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [extra, setExtra] = useState<string[]>([]);
  const [adding, setAdding] = useState("");
  const [linkFor, setLinkFor] = useState<string>();
  const [searchFor, setSearchFor] = useState<string>();
  const icons = library.data?.icons ?? [];
  const save = async (symbol: string, body: CoinIconInput) => {
    setBusy(symbol);
    try {
      await unwrap(client.PUT("/api/v1/coin-icons/{symbol}", { params: { path: { symbol } }, body }));
      await queryClient.invalidateQueries({ queryKey: ["coin-icons"] });
      message.success(`${symbol} 的图标已保存到主控`);
      return true;
    } catch (e) {
      message.error((e as Error).message);
      return false;
    } finally {
      setBusy(undefined);
    }
  };
  const rows = [
    ...new Set([...symbols.filter((s) => s !== "未知币种"), ...icons.map((i) => i.symbol), ...extra].map((s) => s.toUpperCase())),
  ].sort();
  return (
    <Modal open={open} title="币种图标" onCancel={onClose} footer={null} width={640} destroyOnHidden className="coin-icons-dialog">
      <p className="muted">
        图标保存在主控上，浏览器只从主控读取。显示顺序：主控图标库 → 内置 480 多个常见币种 → 首字母。
        {library.data?.directory && (
          <>
            {" "}
            也可以把 <code>KAS.svg</code>、<code>TSC.png</code> 这样的文件放进主控目录{" "}
            <Typography.Text code copyable>
              {library.data.directory}
            </Typography.Text>
            ，刷新后生效。
          </>
        )}
      </p>
      <Space.Compact className="coin-icons-add">
        <Input
          placeholder="添加币种，例如 KAS"
          value={adding}
          maxLength={20}
          onChange={(e) => setAdding(e.target.value.toUpperCase())}
          onPressEnter={() => adding.trim() && (setExtra([...extra, adding.trim()]), setAdding(""))}
        />
        <Button disabled={!adding.trim()} onClick={() => (setExtra([...extra, adding.trim()]), setAdding(""))}>
          添加
        </Button>
      </Space.Compact>
      <input
        ref={input}
        type="file"
        accept="image/png,image/svg+xml,image/webp,image/jpeg,image/*"
        style={{ display: "none" }}
        onChange={async (e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (!file || !target) return;
          try {
            await save(target, { from: "upload", ...(await iconUpload(file)) });
          } catch {
            message.error("无法读取这张图片");
          }
        }}
      />
      <ul className="coin-icon-list">
        {rows.map((symbol) => {
          const own = icons.find((i) => i.symbol === symbol);
          return (
            <li key={symbol}>
              <CoinBadge coin={symbol} size={24} />
              <b>{symbol}</b>
              <span className="muted" title={own?.source_ref ?? undefined}>
                {own ? SOURCE[own.source] ?? own.source : hasBundledIcon(symbol) ? "内置" : "无图标"}
              </span>
              <Space size={2} wrap>
                <Button
                  size="small"
                  loading={busy === symbol}
                  onClick={() => {
                    setTarget(symbol);
                    input.current?.click();
                  }}
                >
                  上传
                </Button>
                <Button size="small" onClick={() => setLinkFor(symbol)}>
                  链接
                </Button>
                {library.data?.search && (
                  <Button size="small" onClick={() => setSearchFor(symbol)}>
                    CoinGecko
                  </Button>
                )}
                {own && own.source !== "directory" && (
                  <Button
                    size="small"
                    type="text"
                    onClick={() =>
                      modal.confirm({
                        title: `删除 ${symbol} 的图标？`,
                        content: "删除后改用主控目录里的同名文件、内置图标或首字母。",
                        okText: "删除",
                        okButtonProps: { danger: true },
                        onOk: async () => {
                          try {
                            await unwrap(client.DELETE("/api/v1/coin-icons/{symbol}", { params: { path: { symbol } } }));
                            await queryClient.invalidateQueries({ queryKey: ["coin-icons"] });
                          } catch (e) {
                            message.error((e as Error).message);
                          }
                        },
                      })
                    }
                  >
                    恢复默认
                  </Button>
                )}
              </Space>
            </li>
          );
        })}
      </ul>
      <LinkDialog symbol={linkFor} busy={busy === linkFor} onClose={() => setLinkFor(undefined)} onSave={(url) => save(linkFor!, { from: "url", url })} />
      <SearchDialog
        symbol={searchFor}
        busy={busy === searchFor}
        onClose={() => setSearchFor(undefined)}
        onPick={(id) => save(searchFor!, { from: "coingecko", id })}
      />
    </Modal>
  );
}

function LinkDialog({ symbol, busy, onClose, onSave }: { symbol?: string; busy: boolean; onClose: () => void; onSave: (url: string) => Promise<boolean> }) {
  const [url, setUrl] = useState("");
  return (
    <Modal
      open={!!symbol}
      title={`${symbol ?? ""} 图标链接`}
      okText="下载到主控"
      confirmLoading={busy}
      okButtonProps={{ disabled: !/^https?:\/\/\S+$/.test(url.trim()) }}
      onCancel={onClose}
      afterOpenChange={(visible) => !visible && setUrl("")}
      onOk={async () => {
        if (await onSave(url.trim())) onClose();
      }}
    >
      <p className="muted">主控下载一次并保存，之后不再依赖这个链接。支持 PNG、JPEG、WebP、SVG，256 KB 以内。</p>
      <Input aria-label="图片链接" placeholder="https://…/kas.png" value={url} onChange={(e) => setUrl(e.target.value)} />
    </Modal>
  );
}

function SearchDialog({ symbol, busy, onClose, onPick }: { symbol?: string; busy: boolean; onClose: () => void; onPick: (id: string) => Promise<boolean> }) {
  const [query, setQuery] = useState<string>();
  const term = query ?? symbol ?? "";
  const [submitted, setSubmitted] = useState<string>();
  const search = useQuery({
    queryKey: ["coin-search", submitted],
    enabled: !!submitted,
    retry: false,
    staleTime: 10 * 60 * 1000,
    queryFn: ({ signal }) => unwrap(client.GET("/api/v1/coin-icons/search", { signal, params: { query: { q: submitted! } } })),
  });
  return (
    <Modal
      open={!!symbol}
      title={`从 CoinGecko 选择 ${symbol ?? ""} 的图标`}
      footer={null}
      onCancel={onClose}
      afterOpenChange={(visible) => {
        if (visible) setSubmitted(symbol);
        else {
          setQuery(undefined);
          setSubmitted(undefined);
        }
      }}
      width={560}
    >
      <p className="muted">搜索由主控发出。同一个符号可能对应多个币，请按名称和市值排名选对的那一个。</p>
      <Input.Search
        aria-label="搜索 CoinGecko"
        value={term}
        onChange={(e) => setQuery(e.target.value)}
        onSearch={(value) => value.trim() && setSubmitted(value.trim())}
        enterButton="搜索"
        loading={search.isFetching}
      />
      <div className="coin-search-results">
        {search.isFetching ? (
          <Spin style={{ display: "block", margin: 24 }} />
        ) : search.error ? (
          <Typography.Text type="danger">{(search.error as Error).message}</Typography.Text>
        ) : search.data && !search.data.length ? (
          <Empty description="没有找到" />
        ) : (
          (search.data ?? []).map((coin: CoinCandidate) => (
            <button key={coin.id} type="button" className="coin-search-hit" disabled={busy} onClick={async () => (await onPick(coin.id)) && onClose()}>
              {coin.thumb ? <img src={coin.thumb} width={28} height={28} alt="" /> : <span className="coin-search-blank" />}
              <span>
                <b>{coin.name}</b> <span className="muted">{coin.symbol}</span>
                <small>{coin.id}</small>
              </span>
              <em>{coin.market_cap_rank ? `#${coin.market_cap_rank}` : "—"}</em>
            </button>
          ))
        )}
      </div>
    </Modal>
  );
}
