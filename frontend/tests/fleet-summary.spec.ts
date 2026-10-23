import { test, expect } from "@playwright/test";

// One hashrate card for every coin (HiveOS keeps the summary on one row), and offline rows say
// how long ago the rig last reported instead of showing a row of dashes.
test("hashrate card merges coins and offline rows show when the rig was last seen", async ({ page }) => {
  const now = Date.now();
  const seconds = now / 1000;
  const id = (n: number) => `00000000-0000-4000-8000-00000000000${n}`;
  const instance = (coin: string, algorithm: string, hs: number) => ({
    instance: `${coin.toLowerCase()}-1`, desired: "running", process_alive: true, stats_observed_at: seconds, configured_coin: coin,
    stats: { coin, algorithm, hashrate_hs: hs, accepted: 1, uptime: 600 },
  });
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id: id(1), name: "fixture", token_id: null }, csrf: "t", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
    if (path.endsWith("/status")) body = [{ machine_id: id(1), state: "pool_offline", local: "online", pool: "offline", pool_checks: [{ result: "offline", message: "No recent pool shares" }] }];
    if (path.endsWith("/machines"))
      body = [1, 2, 3].map((n) => ({ id: id(n), name: `rig-${n}`, host: `10.0.0.${n}`, tags: [], policy: {}, bmc: null, group: "" }));
    if (path.includes("telemetry"))
      body = [
        { machine_id: id(1), kind: "system", observed_at: new Date(now).toISOString(), error: null, data: { cpu_pct: 90, load: [1, 1, 1], gpus: [] } },
        { machine_id: id(1), kind: "mining", observed_at: new Date(now).toISOString(), error: null, data: { instances: [instance("XMR", "rx/0", 68e3), instance("ZEPH", "rx/0", 30e3), instance("KAS", "kheavyhash", 2e9)] } },
        { machine_id: id(3), kind: "system", observed_at: new Date(now - 3 * 3600e3).toISOString(), error: null, data: { cpu_pct: 50, load: [2, 2, 2], gpus: [] } },
      ];
    await route.fulfill({ json: body });
  });
  await page.goto("/machines");
  const summary = page.getByRole("region", { name: "矿场统计" });
  const rates = summary.locator(".hive-stat-rate");
  await expect(rates).toHaveCount(1);
  await expect(rates).toContainText("+2 种币");
  await rates.hover();
  await expect(page.getByRole("tooltip")).toContainText("KAS");
  await expect(page.getByRole("tooltip")).toContainText("ZEPH");

  const offline = page.locator(".hive-machine-row", { hasText: "rig-3" });
  await expect(offline).toContainText("掉线 · 最后在线");
  await expect(offline.locator(".hive-machine-uptime.offline")).toHaveText("3h 0m");
  await expect(offline.locator(".hive-machine-load")).toHaveText("");
  // A rig that never reported says so.
  await expect(page.locator(".hive-machine-row", { hasText: "rig-2" })).toContainText("掉线 · 从未上报");
  // Both branches' controls survive the merge: pool alerts and the selected-only filter.
  const online = page.locator(".hive-machine-row", { hasText: "rig-1" });
  await expect(online.locator(".hive-pool-offline")).toHaveText("矿池离线");
  await online.getByRole("checkbox").check();
  await page.getByRole("button", { name: /已选择/ }).click();
  await expect(page.locator(".hive-machine-row")).toHaveCount(1);
  await summary.getByRole("link", { name: /矿池离线/ }).click();
  await expect(page.getByRole("group", { name: "状态筛选" }).getByRole("button", { name: /矿池离线/ })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".hive-machine-row")).toHaveCount(1);
  await expect(page.locator(".hive-machine-row")).toContainText("rig-1");
});
