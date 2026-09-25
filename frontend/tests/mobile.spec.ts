import { test, expect } from "@playwright/test";

// Phone width: no sideways page scroll; fleet rows become cards; worker tools stay reachable.
test.use({ viewport: { width: 390, height: 844 } });

test("fleet list and worker page fit a phone screen", async ({ page }) => {
  const id = "7c2d9a41-3e5b-4f6a-8b1c-0d9e8f7a6b55";
  const now = new Date().toISOString();
  const seconds = Date.now() / 1000;
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/machines"))
      body = [{ id, name: "7702-01", host: "10.168.2.120", port: 22, username: "root", tags: [], policy: {}, bmc: null, group: "", is_controller: false }];
    if (path.includes("telemetry"))
      body = [
        { machine_id: id, kind: "system", observed_at: now, error: null, data: {
          cpu_pct: 100, cpu_temperature: 58, logical_cpus: 256, load: [256.1, 255.9, 255.7], cpu_power_w: 307, uptime: 36000,
          cpu_model: "AMD EPYC 7702 64-Core Processor", memory_total: 5e11, memory_used: 2e11,
          topology: [{ cpu: 0, socket: 0, core: 0 }, { cpu: 1, socket: 1, core: 0 }], gpus: [] } },
        { machine_id: id, kind: "mining", observed_at: now, error: null, data: { instances: [{
          instance: "cpu-1", desired: "running", process_alive: true, stats_observed_at: seconds, configured_coin: "XMR",
          miner_name: "XMRig", package_version: "6.26.0", stats: { coin: "XMR", algorithm: "rx/0", hashrate_hs: 68210, accepted: 48, uptime: 15000 } }] } },
      ];
    await route.fulfill({ json: body });
  });
  const noSidewaysScroll = async () =>
    expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(1);

  await page.goto("/machines");
  const row = page.locator(".hive-machine-row").filter({ hasText: "7702-01" });
  await expect(row).toContainText("68.21 kH/s");
  await noSidewaysScroll();

  await page.goto(`/machines/${id}`);
  await expect(page.getByRole("toolbar", { name: "矿机操作" })).toBeVisible();
  await expect(page.getByRole("button", { name: "矿工日志" })).toBeVisible();
  // One CPU row for a dual-socket machine.
  const cpuRows = page.locator(".wk-dev-row.cpu");
  await expect(cpuRows).toHaveCount(1);
  await expect(cpuRows).toContainText("2 × AMD EPYC 7702");
  await noSidewaysScroll();
});
