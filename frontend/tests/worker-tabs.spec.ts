import { test, expect, type Page } from "@playwright/test";

// HiveOS worker tabs: Autofan critical temperature, settings rows, tags and the day-by-day stats.
const id = "5a1f0c3e-7d2b-4e8a-9c61-2b7d9e0f4a33";

async function mock(page: Page) {
  const now = new Date().toISOString();
  const machine = {
    id, name: "test", host: "10.5.30.216", port: 22, username: "root", tags: ["rack-a"], group: "",
    is_controller: false, bmc: null, host_key: "SHA256:x", policy: { gpu_temp_limit_c: 85 },
  };
  const puts: Record<string, any>[] = [];
  const history: URL[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    let body: unknown = [];
    if (route.request().method() === "PUT" && path.endsWith(`/machines/${id}`)) {
      const input = route.request().postDataJSON();
      puts.push(input);
      Object.assign(machine, { name: input.name, tags: input.tags, policy: input.policy });
      return route.fulfill({ json: machine });
    }
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/machines")) body = [machine];
    if (path.endsWith("/history")) {
      history.push(url);
      body = [{ machine_id: id, kind: url.searchParams.get("kind"), observed_at: now, error: null, data: { cpu_pct: 20, cpu_temperature: 61 } }];
    }
    if (path.endsWith("/messages"))
      body = [{ id: "1", at: now, level: "error", source: "runtime", kind: "thermal_stop", message: "温度 91°C 达到临界", instance: null, detail: {} }];
    if (path.includes("telemetry"))
      body = [{ machine_id: id, kind: "system", observed_at: now, error: null, data: {
        cpu_temperature: 61, load: [1, 1, 1], gpus: [{ id: "0000:05:00.0", bus: 5, vendor: "NVIDIA", model: "RTX 3070 Ti", temperature_c: 72, power_w: 200 }],
        cpu_power_w: 50 } }];
    await route.fulfill({ json: body });
  });
  return { puts, history };
}

test("Autofan saves the critical temperature and action into the policy", async ({ page }) => {
  const { puts } = await mock(page);
  await page.goto(`/machines/${id}?tab=autofan`);
  await expect(page.locator(".wk-settings")).toContainText("72°C");
  await expect(page.locator(".wk-settings")).toContainText("GPU 0");
  const save = page.getByRole("button", { name: "Save settings" });
  await expect(save).toBeDisabled();
  await page.locator(".wk-set-row", { hasText: "温度保护" }).getByRole("switch").click();
  await page.locator(".wk-set-row", { hasText: "Critical temp" }).getByRole("spinbutton").fill("88");
  await page.locator(".wk-set-row", { hasText: "Critical action" }).locator(".ant-select").click();
  await page.getByTitle("重启").click();
  await save.click();
  await expect.poll(() => puts.length).toBe(1);
  expect(puts[0].policy).toEqual({ gpu_temp_limit_c: 85, critical_temp_c: 88, critical_action: "reboot" });
  expect(puts[0].credential).toBeNull();
});

test("settings rows update name, tags, maintenance mode and power", async ({ page }) => {
  const { puts } = await mock(page);
  await page.goto(`/machines/${id}?tab=settings`);
  await page.locator(".wk-set-row", { hasText: "名称" }).getByRole("textbox").fill("rig-01");
  await page.locator(".wk-set-row", { hasText: "描述" }).getByRole("textbox").fill("机架 A 第 3 层");
  await page.getByRole("button", { name: /更新矿机/ }).click();
  await expect.poll(() => puts.length).toBe(1);
  expect(puts[0]).toMatchObject({ name: "rig-01", tags: ["rack-a"], policy: { gpu_temp_limit_c: 85, description: "机架 A 第 3 层" } });
  await page.locator(".wk-set-row", { hasText: "维护模式" }).getByRole("switch").click();
  await page.locator(".wk-set-row", { hasText: "维护模式" }).getByRole("button", { name: /更\s*新/ }).click();
  await expect.poll(() => puts.length).toBe(2);
  expect(puts[1].policy.maintenance_mode).toBe(true);
  await page.locator(".wk-set-row", { hasText: "硬件功耗" }).getByRole("spinbutton").fill("80");
  await page.locator(".wk-set-row", { hasText: "供电单元效率" }).getByRole("spinbutton").fill("90");
  await page.locator(".wk-set-row", { hasText: "供电单元效率" }).getByRole("button", { name: /更\s*新/ }).click();
  await expect.poll(() => puts.length).toBe(3);
  expect(puts[2].policy).toMatchObject({ hardware_power_w: 80, psu_efficiency_pct: 90 });
  // (50 W CPU + 200 W GPU + 80 W hardware) / 0.9
  await expect(page.locator(".wk-band .wk-power")).toContainText("366.67");
});

test("toolbar edits tags in a dialog", async ({ page }) => {
  const { puts } = await mock(page);
  await page.goto(`/machines/${id}`);
  await page.getByRole("button", { name: "编辑标签" }).click();
  const dialog = page.getByRole("dialog", { name: /编辑标签/ });
  await dialog.getByRole("combobox").fill("hot-aisle");
  await page.keyboard.press("Enter");
  await dialog.getByRole("button", { name: /保\s*存/ }).click();
  await expect.poll(() => puts.length).toBe(1);
  expect(puts[0].tags).toEqual(["rack-a", "hot-aisle"]);
});

test("stats page steps back one day and requests that window", async ({ page }) => {
  const { history } = await mock(page);
  await page.goto(`/machines/${id}?tab=performance`);
  await expect(page.locator(".wk-event.error")).toHaveCount(1);
  const until = () => new Date(history[history.length - 1].searchParams.get("until")!).getTime();
  await expect.poll(() => history.length).toBeGreaterThan(1);
  const first = until();
  await expect(page.getByRole("button", { name: "下一个" })).toBeDisabled();
  await page.getByRole("button", { name: "上一个" }).click();
  await expect.poll(until).toBe(first - 86400000);
  // Yesterday has no events from "now".
  await expect(page.locator(".wk-event")).toHaveCount(0);
  await page.getByText("3d", { exact: true }).click();
  await expect.poll(() => history[history.length - 1].searchParams.get("hours")).toBe("72");
});
