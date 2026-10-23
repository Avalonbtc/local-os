import { useEffect, useRef } from "react";
import { App } from "antd";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { client, unwrap, useFarmSettings, type StatusAlert } from "../../shared/api";

const SEEN_KEY = "rigdeck:last-alert";
/** At most this many pop-ups at once (e.g. after a switch failure); the rest are summarised. */
const MAX_POPUPS = 5;

function readSeen(): number | null {
  try {
    const value = Number(localStorage.getItem(SEEN_KEY));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}
function writeSeen(id: number) {
  try {
    localStorage.setItem(SEEN_KEY, String(id));
  } catch {
    /* private mode: alerts pop up again after a reload */
  }
}
function title(alert: StatusAlert) {
  if (alert.kind === "local") return alert.event === "offline" ? "矿机离线" : "矿机恢复在线";
  return alert.event === "offline" ? "矿池离线" : "矿池恢复";
}

/**
 * Pops up offline and recovery alerts while the console is open. The controller raises each
 * alert once per transition; this only remembers which ones this browser has already shown.
 */
export function AlertWatcher() {
  const { notification } = App.useApp();
  const navigate = useNavigate();
  const popup = useFarmSettings().data?.alerts?.popup ?? true;
  // Alerts after the last one this browser showed; on a first visit, only new ones.
  const seen = useRef<number | null>(readSeen());
  const alerts = useQuery({
    queryKey: ["status-alerts"],
    queryFn: ({ signal }) =>
      unwrap(
        client.GET("/api/v1/alerts", {
          signal,
          params: { query: seen.current === null ? { limit: 1 } : { after: seen.current, limit: 100 } },
        }),
      ),
    refetchInterval: 10000,
  });
  useEffect(() => {
    const items = alerts.data;
    if (!items) return;
    const newest = items.reduce((max, a) => Math.max(max, a.id), seen.current ?? 0);
    if (seen.current === null) {
      seen.current = newest;
      writeSeen(newest);
      return;
    }
    const fresh = items.filter((a) => a.id > (seen.current ?? 0));
    if (!fresh.length) return;
    seen.current = newest;
    writeSeen(newest);
    if (!popup) return;
    for (const alert of fresh.slice(-MAX_POPUPS)) {
      const offline = alert.event === "offline";
      notification[offline ? "error" : "success"]({
        key: `status-alert-${alert.id}`,
        message: `${title(alert)} · ${alert.machine_name}`,
        description: alert.message,
        placement: "bottomRight",
        // Offline stays until closed; recoveries clear themselves.
        duration: offline ? 0 : 8,
        onClick: () => navigate(`/machines/${alert.machine_id}`),
        style: { cursor: "pointer" },
      });
    }
    if (fresh.length > MAX_POPUPS)
      notification.warning({
        key: "status-alert-more",
        message: `另有 ${fresh.length - MAX_POPUPS} 条离线/恢复通知`,
        description: "在矿机列表用「掉线」「矿池离线」筛选查看",
        placement: "bottomRight",
        duration: 0,
      });
  }, [alerts.data, popup, notification, navigate]);
  return null;
}
