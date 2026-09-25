import { test, expect } from "@playwright/test";

// HiveOS-style worker page: header band, miner panel with device tiles, status line,
// message chips, device table (CPU + each GPU) and system tiles.
test("worker page shows the HiveOS layout for a CPU + GPU rig", async ({ page }) => {
  const id = "5a1f0c3e-7d2b-4e8a-9c61-2b7d9e0f4a33";
  const now = new Date().toISOString();
  const seconds = Date.now() / 1000;
  const dismissed: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = [];
    if (path.endsWith("/messages/dismiss")) {
      dismissed.push(route.request().postDataJSON());
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/log")) body = { instance: "gpu-1", text: "[2026-09-25] speed 72.33 MH/s", size: 30 };
    if (path.endsWith("/machines"))
      body = [{ id, name: "test", host: "10.5.30.216", port: 22, username: "root", tags: [], policy: {}, bmc: null, group: "", is_controller: false }];
    if (path.endsWith("/messages"))
      body = dismissed.length
        ? []
        : [{ id: "1", at: now, level: "success", source: "job", kind: "command", message: "执行命令：成功", instance: null,
            detail: { job_id: "j", target_id: "t", status: "succeeded", output: "Selfupgrade successful", output_truncated: false } }];
    if (path.includes("telemetry"))
      body = [
        { machine_id: id, kind: "system", observed_at: now, error: null, data: {
          cpu_pct: 12, cpu_temperature: 40, logical_cpus: 4, load: [0.79, 0.68, 0.45], uptime: 3600, cpu_power_w: 12,
          memory_total: 4e9, memory_used: 1.1e9, runtime_version: "0.2.2", cpu_model: "Atom Z36xxx",
          addresses: [{ interface: "eth0", address: "10.5.30.216" }], gpu_drivers: { AMD: "20.40" },
          gpus: [
            { id: "0000:05:00.0", bus: 5, vendor: "AMD", model: "Radeon RX 6600 XT", vram_mb: 8176, temperature_c: 40, memory_temperature_c: 54, fan_pct: 60, power_w: 55, core_mhz: 1050, core_mv: 700, mem_mhz: 1120 },
            { id: "0000:0c:00.0", bus: 12, vendor: "AMD", model: "Radeon RX 6700 XT", vram_mb: 12272, temperature_c: 53, memory_temperature_c: 86, fan_pct: 60, power_w: 97, core_mhz: 1050, core_mv: 700, mem_mhz: 1075 },
          ] } },
        { machine_id: id, kind: "mining", observed_at: now, error: null, data: { instances: [{
          instance: "gpu-1", desired: "running", process_alive: true, stats_observed_at: seconds, configured_coin: "ETH",
          miner_name: "teamredminer", package_version: "0.8.6", sheet_id: "s1", sheet_name: "test", pool: "stratum+tcp://pool:4444",
          stats: { coin: "ETH", algorithm: "ethash", hashrate_hs: 72.33e6, accepted: 5, rejected: 0, uptime: 271, device: "gpu",
            gpu_hs: [{ bus: 5, hs: 28.43e6 }, { bus: 12, hs: 43.9e6 }] } }] } },
      ];
    await route.fulfill({ json: body });
  });
  await page.goto(`/machines/${id}`);
  await expect(page.locator(".wk-band")).toContainText("test");
  await expect(page.locator(".wk-band")).toContainText("LA 0.79 0.68 0.45");
  await expect(page.locator(".wk-band")).toContainText("164W");
  await expect(page.locator(".wk-miner")).toContainText("teamredminer");
  await expect(page.locator(".wk-miner")).toContainText("72.33 MH/s");
  await expect(page.locator(".wk-tile")).toHaveCount(3);
  await expect(page.locator(".wk-status")).toContainText("挖矿软件正常运行");
  // HiveOS chips: open shows the full message and output; × closes it on the server.
  await expect(page.locator(".wk-chip")).toContainText("执行命令：成功");
  await page.locator(".wk-chip-open").click();
  await expect(page.getByRole("dialog")).toContainText("Selfupgrade successful");
  await page.getByRole("dialog").getByRole("button", { name: /确\s*定/ }).click();
  await page.getByRole("button", { name: "关闭消息 执行命令：成功" }).click();
  await expect(page.locator(".wk-chip")).toHaveCount(0);
  expect(dismissed).toEqual([{ key: "job:1" }]);
  const rows = page.locator(".wk-dev-row:not(.head)");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(1)).toContainText("Radeon RX 6600 XT");
  await expect(rows.nth(1)).toContainText("28.43 MH/s");
  await expect(rows.nth(2)).toContainText("86°");
  await expect(page.locator(".wk-sys")).toContainText("剩余内存");
  // Toolbar icons like HiveOS; the miner log opens in a dialog.
  await page.getByRole("button", { name: "矿工日志" }).click();
  await expect(page.getByRole("dialog", { name: /矿工日志/ })).toContainText("speed 72.33 MH/s");
  await page.keyboard.press("Escape");
  for (const tab of ["概述", "飞行表", "统计", "活动", "设定"])
    await expect(page.getByRole("tab", { name: tab })).toBeVisible();
});
