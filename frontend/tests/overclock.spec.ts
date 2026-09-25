import { test, expect } from "@playwright/test";

const id = "5b0e7a8e-0c62-4d0e-8d0e-3070aaaaaaaa";
const gpus = [
  { id: "0000:01:00.0", vendor: "NVIDIA", model: "NVIDIA GeForce RTX 3070 Ti", bus: 1, temperature_c: 54, fan_pct: 60, power_w: 180, core_mhz: 1500, mem_mhz: 9501, power_limit_w: 220 },
  { id: "0000:02:00.0", vendor: "NVIDIA", model: "NVIDIA GeForce RTX 3070 Ti", bus: 2, temperature_c: 57, fan_pct: 60, power_w: 182, core_mhz: 1500, mem_mhz: 9501, power_limit_w: 220 },
];

function mockApi(page: import("@playwright/test").Page, calls: any[], templates: any[] = []) {
  return page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let response: any = [];
    if (path.endsWith("/me")) response = { actor: { id, name: "fixture", token_id: null }, csrf: "fixture-csrf", expires_at: "2099-01-01" };
    if (path.endsWith("/machines")) response = [{ id, name: "rig-3070ti", host: "10.0.0.9", port: 22, username: "root", tags: [], group: "", is_controller: false, bmc: null, policy: {}, created_at: "2026-01-01T00:00:00Z" }];
    if (path.endsWith("/telemetry")) response = [{ machine_id: id, kind: "system", observed_at: new Date().toISOString(), error: null,
      data: { gpus, gpu_drivers: { NVIDIA: "580.95" }, gpu_oc: { profile_name: "eth", applied_at: Date.now() / 1000, config: { nvidia: { clock: [1500], mem: [2400] } },
        results: [{ vendor: "NVIDIA", index: 0, bus_id: "0000:01:00.0", applied: ["锁定核心 1500 MHz", "显存偏移 +2400"], errors: [] }] } } }];
    if (path.endsWith("/catalog/oc-profiles")) {
      if (request.method() === "POST") {
        calls.push({ template: request.postDataJSON() });
        response = { id: "t1", kind: "oc_profile", ...request.postDataJSON(), revision: 1 };
      } else response = templates;
    }
    if (path.endsWith("/jobs") && request.method() === "POST") {
      expect(request.headers()["x-csrf-token"]).toBe("fixture-csrf");
      calls.push(request.postDataJSON());
      response = { job_id: "fixture-job" };
    }
    await route.fulfill({ json: response });
  });
}

test("worker overclock tab shows cards and applies HiveOS values", async ({ page }) => {
  const calls: any[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page, calls);
  await page.goto(`/machines/${id}?tab=overclock`);
  await expect(page.getByText("锁定核心 1500 MHz")).toBeVisible();
  await expect(page.getByLabel("CLOCK", { exact: true })).toHaveValue("1500");
  await page.getByLabel("CLOCK", { exact: true }).fill("1400 1450");
  await page.getByLabel("PLIMIT", { exact: true }).fill("200");
  await expect(page.getByText("核心 1450，显存 2400，功耗墙 200")).toBeVisible();
  await page.screenshot({ path: "../screenshots/overclock.png", fullPage: true });
  await page.getByRole("button", { name: "应用超频" }).click();
  await page.getByRole("dialog").getByRole("button", { name: /^应\s?用$/ }).click();
  await expect.poll(() => calls.length).toBe(1);
  expect(calls[0].machine_ids).toEqual([id]);
  expect(calls[0].action).toEqual({ kind: "gpu_oc", profile_name: null, config: { nvidia: { clock: [1400, 1450], mem: [2400], plimit: [200] } } });
  expect(errors).toEqual([]);
});

test("HiveOS conf import fills the form and reset submits a reset job", async ({ page }) => {
  const calls: any[] = [];
  await mockApi(page, calls);
  await page.goto(`/machines/${id}?tab=overclock`);
  await page.getByRole("button", { name: "导入 HiveOS 配置" }).click();
  await page.getByLabel("HiveOS 配置").fill('CLOCK="-200 -150"\nMEM="2600"\nPLIMIT="0"\nFAN="70"\nRUNNING_DELAY="30"\nLOGO_BRIGHTNESS="0"');
  await page.getByRole("dialog").getByRole("button", { name: /^导\s?入$/ }).click();
  await expect(page.getByLabel("CLOCK", { exact: true })).toHaveValue("-200 -150");
  await expect(page.getByLabel("FAN", { exact: true }).first()).toHaveValue("70");
  await expect(page.getByLabel("RUNNING_DELAY")).toHaveValue("30");
  await page.getByRole("button", { name: "恢复默认" }).click();
  await page.getByRole("button", { name: /^(确\s?定|OK)$/ }).click();
  await expect.poll(() => calls.length).toBe(1);
  expect(calls[0].action).toEqual({ kind: "gpu_oc_reset" });
});

test("templates page applies a template to selected machines with canary", async ({ page }) => {
  const calls: any[] = [];
  await mockApi(page, calls, [{ id: "t1", kind: "oc_profile", name: "3070Ti-ETC", revision: 1, data: { nvidia: { clock: [1500], mem: [2400], plimit: [220] } } }]);
  await page.goto("/overclocking");
  await expect(page.getByText("NVIDIA：核心 1500，显存 2400，功耗墙 220")).toBeVisible();
  await page.getByRole("button", { name: "应用到机器" }).click();
  await page.getByRole("combobox", { name: "目标机器" }).click();
  await page.getByTitle("rig-3070ti").click();
  await page.getByRole("dialog").getByRole("button", { name: /^应\s?用$/ }).click();
  await expect.poll(() => calls.length).toBe(1);
  expect(calls[0].canary).toBe(true);
  expect(calls[0].action).toEqual({ kind: "gpu_oc", profile_name: "3070Ti-ETC", config: { nvidia: { clock: [1500], mem: [2400], plimit: [220] } } });
});
