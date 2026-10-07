import React, { lazy, Suspense, useEffect, useRef } from "react";
import ReactDOM from "react-dom/client";
import {
  App as AntApp,
  ConfigProvider,
  Dropdown,
  Select,
  Spin,
  Tooltip,
  theme,
} from "antd";
import zhCN from "antd/locale/zh_CN";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import {
  BrowserRouter,
  Link,
  Navigate,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom";
import {
  DownOutlined,
  LogoutOutlined,
  RocketOutlined,
  SettingOutlined,
  SyncOutlined,
} from "@ant-design/icons";
import { client, setCsrf, unwrap, useMachines } from "./shared/api";
import { Login } from "./features/identity";
import { FleetPage, FleetSummary, FleetDataCache, MachineDetail } from "./features/fleet";
import "./styles.css";

// The machine list is the landing page; every other farm tab is fetched when first opened.
const page = <K extends string>(load: () => Promise<Record<K, React.ComponentType>>, name: K) =>
  lazy(() => load().then((module) => ({ default: module[name] })));
const WalletsPage = page(() => import("./features/wallets"), "WalletsPage");
const FlightsPage = page(() => import("./features/flight_sheets"), "FlightsPage");
const BmcPage = page(() => import("./features/bmc"), "BmcPage");
const BiosPage = page(() => import("./features/bios"), "BiosPage");
const OverclockPage = page(() => import("./features/overclock"), "OverclockPage");
const SettingsPage = page(() => import("./features/settings"), "SettingsPage");
const TerminalsPage = page(() => import("./features/terminal"), "TerminalsPage");

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, refetchOnWindowFocus: false, staleTime: 3000 },
  },
});
/** HiveOS farm tabs. Rigs download their own miners, so there is no coin, pool or miner catalogue. */
const farmTabs = [
  ["/machines", "矿机"],
  ["/wallets", "钱包"],
  ["/flight-sheets", "飞行表"],
  ["/overclocking", "超频"],
  ["/bmc", "BMC 设备"],
  ["/bios", "BIOS"],
  ["/terminals", "多机终端"],
  ["/settings", "设定"],
] as const;

function WorkerPicker({ current }: { current?: string }) {
  const machines = useMachines();
  const navigate = useNavigate();
  return (
    <Select
      className="rd-worker-picker"
      variant="borderless"
      size="small"
      showSearch
      optionFilterProp="label"
      placeholder="选择矿机"
      value={current}
      popupMatchSelectWidth={260}
      suffixIcon={<DownOutlined />}
      aria-label="选择矿机"
      options={(machines.data ?? [])
        .slice()
        .sort((a, b) => a.name.localeCompare(b.name, "zh-CN", { numeric: true }))
        .map((m) => ({ value: m.id, label: m.name }))}
      onChange={(id: string) => navigate(`/machines/${id}`)}
    />
  );
}

function TopBar({ username, logout }: { username: string; logout: () => void }) {
  const location = useLocation();
  const navigate = useNavigate();
  const worker = /^\/machines\/([0-9a-f-]{36})/.exec(location.pathname)?.[1];
  return (
    <header className="rd-top">
      <div className="rd-top-inner">
        <nav className="rd-crumb" aria-label="位置">
          <Link to="/machines" className="rd-logo" aria-label="RigDeck">
            <span className="hive-brand-mark" aria-hidden="true"><i /><i /><i /></span>
            RigDeck
          </Link>
          <span className="rd-dot">·</span>
          <Dropdown
            trigger={["click"]}
            menu={{
              items: farmTabs.map(([path, label]) => ({ key: path, label })),
              onClick: ({ key }) => navigate(key),
            }}
          >
            <button className="rd-crumb-button">
              我的矿场 <DownOutlined />
            </button>
          </Dropdown>
          <span className="rd-dot">·</span>
          <WorkerPicker current={worker} />
        </nav>
        <div className="rd-top-actions">
          <Tooltip title="飞行表">
            <Link to="/flight-sheets" aria-label="飞行表"><RocketOutlined /></Link>
          </Tooltip>
          <Tooltip title="刷新数据">
            <button aria-label="刷新数据" onClick={() => queryClient.invalidateQueries()}>
              <SyncOutlined />
            </button>
          </Tooltip>
          <Dropdown
            trigger={["click"]}
            menu={{
              items: [
                { key: "settings", label: "设定", icon: <SettingOutlined /> },
                { key: "logout", label: "退出登录", icon: <LogoutOutlined /> },
              ],
              onClick: ({ key }) => (key === "logout" ? logout() : navigate("/settings")),
            }}
          >
            <button className="rd-user" data-initial={username.slice(0, 1)} aria-label={`账户 ${username}`}>
              {username}
            </button>
          </Dropdown>
        </div>
      </div>
    </header>
  );
}

function FarmTabs({ pathname }: { pathname: string }) {
  const ref = useRef<HTMLElement>(null);
  // On narrow screens the tab row scrolls; keep the active tab in view.
  useEffect(() => {
    const nav = ref.current;
    const active = nav?.querySelector<HTMLElement>("a.active");
    if (!nav || !active || nav.scrollWidth <= nav.clientWidth) return;
    nav.scrollLeft = active.offsetLeft - (nav.clientWidth - active.offsetWidth) / 2;
  }, [pathname]);
  return (
    <nav className="rd-tabs" aria-label="矿场页面" ref={ref}>
      <div className="rd-tabs-inner">
        {farmTabs.map(([path, label]) => {
          const active = pathname === path || pathname.startsWith(`${path}/`);
          return (
            <Link key={path} to={path} className={active ? "active" : undefined} aria-current={active ? "page" : undefined}>
              {label}
            </Link>
          );
        })}
      </div>
    </nav>
  );
}

function Root() {
  const session = useQuery({
    queryKey: ["session"],
    queryFn: async () => {
      const value = await unwrap(client.GET("/api/v1/me"));
      setCsrf(value.csrf);
      return value;
    },
    retry: false,
  });
  const location = useLocation();
  useEffect(() => {
    const expired = () => {
      setCsrf("");
      queryClient.setQueryData(["session"], null);
      queryClient.removeQueries({ predicate: (query) => query.queryKey[0] !== "session" });
    };
    window.addEventListener("rigdeck-session-expired", expired);
    return () => window.removeEventListener("rigdeck-session-expired", expired);
  }, []);
  if (session.isLoading)
    return (
      <div className="app-loading">
        <Spin size="large" />
      </div>
    );
  if (!session.data)
    return (
      <Login
        onLogin={() => {
          queryClient.clear();
          session.refetch();
        }}
      />
    );
  // Worker pages have their own header, like HiveOS; every farm page gets the band and tabs.
  const farmPage = !/^\/machines\/[^/]+/.test(location.pathname);
  const logout = async () => {
    await unwrap(client.POST("/api/v1/logout"));
    setCsrf("");
    queryClient.clear();
    window.location.reload();
  };
  return (
    <div className="rd-shell">
      <FleetDataCache />
      <TopBar username={session.data.actor.name} logout={logout} />
      {farmPage && (
        <>
          <div className="rd-band">
            <FleetSummary />
          </div>
          <FarmTabs pathname={location.pathname} />
        </>
      )}
      <main className={`rd-main ${location.pathname === "/machines" ? "rd-main-fleet" : ""}`}>
        <Suspense fallback={<Spin style={{ display: "block", margin: 60 }} />}>
        <Routes>
          <Route path="/" element={<Navigate replace to="/machines" />} />
          <Route path="/machines" element={<FleetPage />} />
          <Route path="/machines/:id" element={<MachineDetail />} />
          <Route path="/wallets" element={<WalletsPage />} />
          <Route path="/flight-sheets" element={<FlightsPage />} />
          <Route path="/bmc" element={<BmcPage />} />
          <Route path="/overclocking" element={<OverclockPage />} />
          <Route path="/bios" element={<BiosPage />} />
          <Route path="/bios/:id" element={<BiosPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/terminals" element={<TerminalsPage />} />
          {/* Old bookmarks: these pages no longer exist. */}
          {["/jobs", "/coins", "/pools", "/miners"].map((path) => (
            <Route key={path} path={path} element={<Navigate replace to="/machines" />} />
          ))}
          <Route
            path="*"
            element={
              <div className="not-found">
                <h1>页面不存在</h1>
                <Link to="/machines">返回矿机列表</Link>
              </div>
            }
          />
        </Routes>
        </Suspense>
      </main>
    </div>
  );
}
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ConfigProvider
      locale={zhCN}
      theme={{
        algorithm: theme.darkAlgorithm,
        token: {
          colorPrimary: "#f5a524",
          colorInfo: "#52a8e8",
          colorSuccess: "#7cc242",
          colorWarning: "#f5c945",
          colorError: "#f0555f",
          colorLink: "#f5a524",
          colorBgBase: "#13161a",
          colorBgContainer: "#1f2429",
          colorBgElevated: "#262c32",
          colorBgLayout: "#13161a",
          colorBorder: "#353c44",
          colorBorderSecondary: "#2b3138",
          colorText: "#e7eaee",
          colorTextSecondary: "#aeb7c2",
          colorTextTertiary: "#7d8894",
          borderRadius: 6,
          borderRadiusLG: 10,
          controlHeight: 32,
          fontFamily:
            "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Noto Sans SC', sans-serif",
          fontSize: 13,
        },
        components: {
          Table: {
            headerBg: "#191d22",
            rowHoverBg: "#262c32",
            cellPaddingBlock: 10,
          },
          Button: { primaryColor: "#1a1405", primaryShadow: "none", defaultShadow: "none" },
          Modal: { contentBg: "#1f2429", headerBg: "#1f2429" },
          Tabs: { itemColor: "#aeb7c2" },
          Segmented: { itemSelectedBg: "#2f363d", trackBg: "#1f2429" },
        },
      }}
    >
      <AntApp>
        <QueryClientProvider client={queryClient}>
          <BrowserRouter>
            <Root />
          </BrowserRouter>
        </QueryClientProvider>
      </AntApp>
    </ConfigProvider>
  </React.StrictMode>,
);
