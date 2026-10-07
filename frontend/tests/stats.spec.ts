import { test, expect } from "@playwright/test";

// Miner numbers pass through three 10 s loops (rig sample, controller poll, page poll) before
// they are shown; a normal 30–45 s age must still show the hashrate, not "等待统计".
test("hashrate stays visible through normal pipeline delay and expires when really stale", async ({ page }) => {
  const id = "3c9b2f60-1a4d-4e7b-9f21-6d8e0a5b7c42";
  let statsAge = 45;
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const now = Date.now() / 1000;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/machines"))
      body = [{ id, name: "7b12-2", host: "192.168.5.36", port: 22, username: "root", tags: [], policy: {}, bmc: null, group: "", is_controller: false }];
    if (path.includes("telemetry"))
      body = [
        { machine_id: id, kind: "system", observed_at: new Date().toISOString(), error: null, data: { cpu_pct: 99, logical_cpus: 128 } },
        { machine_id: id, kind: "mining", observed_at: new Date((now - 20) * 1000).toISOString(), error: null, data: { instances: [{
          instance: "cpu-1", desired: "running", process_alive: true, observed_at: now - 20, stats_observed_at: now - statsAge,
          configured_coin: "XMR", miner_name: "XMRig", stats: { coin: "XMR", algorithm: "rx/0", hashrate_hs: 55000 } }] } },
      ];
    await route.fulfill({ json: body });
  });
  await page.goto("/machines");
  const row = page.locator(".hive-machine-row").filter({ hasText: "7b12-2" });
  await expect(row).toContainText("55 kH/s");
  await expect(row).not.toContainText("等待统计");

  statsAge = 90;
  await page.reload();
  await expect(row).toContainText("等待统计");
});
