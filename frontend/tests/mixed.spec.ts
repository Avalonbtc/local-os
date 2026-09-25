import { test, expect } from "@playwright/test";

// CPU mines XMR while the GPUs mine another coin: one line per coin, one tile per GPU.
test("fleet row shows CPU and GPU coins separately with GPU tiles", async ({ page }) => {
  const id = "0b7c61f2-8a57-4b8f-9d0e-4f2c3f6d1a11";
  const now = new Date().toISOString();
  const seconds = Date.now() / 1000;
  const instance = (name: string, coin: string, algorithm: string, hs: number, extra: object) => ({
    instance: name, desired: "running", process_alive: true, stats_observed_at: seconds, configured_coin: coin,
    miner_name: name === "cpu-1" ? "XMRig" : "SRBMiner-MULTI", package_version: "1.0",
    stats: { coin, algorithm, hashrate_hs: hs, accepted: 10, rejected: 0, ...extra },
  });
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/machines")) body = [{ id, name: "mixed-1", host: "10.0.0.9", tags: [], policy: {}, bmc: null, group: "" }];
    if (path.includes("telemetry"))
      body = [
        { machine_id: id, kind: "system", observed_at: now, error: null, data: {
          cpu_pct: 97, cpu_temperature: 60, logical_cpus: 128, cpu_power_w: 300,
          gpus: [
            { id: "0000:01:00.0", bus: 1, model: "RTX 4090", temperature_c: 62, fan_pct: 55, power_w: 280 },
            { id: "0000:02:00.0", bus: 2, model: "RTX 4090", temperature_c: 90, fan_pct: 100, power_w: 300 },
          ] } },
        { machine_id: id, kind: "mining", observed_at: now, error: null, data: { instances: [
          instance("cpu-1", "XMR", "rx/0", 68220, { device: "cpu" }),
          instance("gpu-1", "RVN", "kawpow", 120e6, { device: "gpu", gpu_hs: [{ bus: 1, hs: 60e6 }, { bus: 2, hs: 60e6 }] }),
        ] } },
      ];
    await route.fulfill({ json: body });
  });
  await page.goto("/machines");
  const row = page.locator(".hive-machine-row").filter({ hasText: "mixed-1" });
  await expect(row.locator(".hive-miner-line")).toHaveCount(2);
  await expect(row).toContainText("XMR68.22 kH/s");
  await expect(row).toContainText("RVN120 MH/s");
  await expect(row.locator("i.hive-gpu")).toHaveCount(2);
  await expect(row.locator("i.hive-gpu.hot")).toHaveText("90");
  await expect(row.locator(".hive-machine-power")).toContainText("880 W");
  const summary = page.getByRole("region", { name: "矿场统计" });
  await expect(summary).toContainText("XMR");
  await expect(summary).toContainText("RVN");
  await expect(summary).toContainText("GPU");
});
