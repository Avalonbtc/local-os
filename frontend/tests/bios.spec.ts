import { test, expect } from "@playwright/test";

if (process.env.RIGDECK_BIOS_TEST_URL) {
  test.use({ baseURL: process.env.RIGDECK_BIOS_TEST_URL });
  test.beforeEach(async ({ page }) => {
    await page.route("**/bios**", async route => {
      if (route.request().isNavigationRequest()) {
        const response = await route.fetch({ url: process.env.RIGDECK_BIOS_TEST_URL + "/" });
        return route.fulfill({ response });
      }
      return route.fallback();
    });
  });
}

test("independent BIOS page saves only changed fields and shows pending state", async ({ page }) => {
  const id = "d66d5724-2240-462e-9943-44bd9fa529cf";
  const key = '["Advanced", "ACPI Settings", "L3 NUMA"]';
  const calls: any[] = [];
  let pending = {};
  const errors: string[] = [];
  page.on("pageerror", e => errors.push(e.message));
  await page.route("**/api/v1/**", async route => {
    const path = new URL(route.request().url()).pathname;
    let response: any = [];
    if (path.endsWith("/me")) response = { actor: { id, name: "fixture", token_id: null }, csrf: "fixture-csrf", expires_at: "2099-01-01" };
    if (path.endsWith("/machines")) response = [{ id, name: "7702-02", host: "10.0.0.2", tags: [], bmc: { url: "10.0.0.202", username: "ADMIN", provider: "ipmi" } }];
    if (path.endsWith("/bios")) response = { machine_id: id, licenses: ["SFT-OOB-LIC"], revision: "a".repeat(64), observed_at: new Date().toISOString(), pending, settings: [
      { id: key, name: "L3 NUMA", group: "Advanced / ACPI Settings", kind: "Option", value: "Auto", options: ["Auto", "Disabled"], help: "Controls NUMA", condition: "", license: "" },
      { id: "power", name: "cTDP", group: "Advanced / NB", kind: "Numeric", value: "150", options: [], minimum: 100, maximum: 200, help: "Power in W", condition: "", license: "" },
    ] };
    if (path.endsWith("/jobs") && route.request().method() === "POST") {
      expect(route.request().headers()["x-csrf-token"]).toBe("fixture-csrf");
      calls.push(route.request().postDataJSON()); pending = calls.at(-1).action.changes;
      response = { job_id: "fixture-job" };
    }
    if (path.endsWith("/jobs/fixture-job")) response = { id: "fixture-job", status: "succeeded", targets: [{ id: "target", output: "SUM 已接受指定设置。", error: null }] };
    await route.fulfill({ json: response });
  });
  await page.goto(`/bios/${id}`);
  await expect(page.getByRole("heading", { name: "BIOS 管理" })).toBeVisible();
  await page.getByLabel("L3 NUMA", { exact: true }).first().click();
  await page.getByTitle("Disabled", { exact: true }).click();
  await page.getByRole("button", { name: "检查并保存（1）" }).click();
  const modal = page.getByRole("dialog");
  await expect(modal).toContainText("Auto → Disabled");
  await expect(modal).not.toContainText("cTDP");
  await modal.getByRole("button", { name: "保存，不重启" }).click();
  await expect(page.getByText("有待生效或待核实的修改", { exact: true })).toBeVisible();
  expect(calls).toHaveLength(1);
  expect(calls[0].action).toEqual({ kind: "bios_write", revision: "a".repeat(64), changes: { [key]: "Disabled" } });
  expect(calls[0].machine_ids).toEqual([id]);
  await expect(page.getByRole("combobox", { name: "L3 NUMA", exact: true })).toBeDisabled();
  await page.screenshot({ path: "../screenshots/bios-pending.png", fullPage: true });
  await page.reload();
  await expect(page.getByText("有待生效或待核实的修改", { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole("button", { name: "读取 BIOS" })).toBeVisible();
  expect(errors).toEqual([]);
});

test("BIOS empty state and API errors are explicit", async ({ page }) => {
  await page.route("**/api/v1/**", async route => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/me")) return route.fulfill({ json: { actor: { name: "fixture" }, csrf: "fixture", expires_at: "2099-01-01" } });
    if (path.endsWith("/bios")) return route.fulfill({ status: 503, json: { error: "SUM 未配置" } });
    return route.fulfill({ json: [] });
  });
  await page.goto("/bios");
  await expect(page.getByText("选择一台机器以管理 BIOS")).toBeVisible();
  await page.goto("/bios/test-machine");
  await expect(page.getByText("SUM 未配置", { exact: true })).toBeVisible();
});
