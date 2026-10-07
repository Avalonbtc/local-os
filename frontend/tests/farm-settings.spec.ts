import { test, expect } from "@playwright/test";

// The daily cost estimate uses the farm's electricity price, which the user sets under 设定 → 矿场.
test("electricity price is a farm setting used by the daily cost estimate", async ({ page }) => {
  const id = "8d0f4c2e-5b7a-4e19-a3c6-1f2e9d8b7a60";
  let price = 1;
  const saved: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/settings/farm")) {
      if (route.request().method() === "PUT") {
        saved.push(route.request().postDataJSON());
        price = (saved.at(-1) as { electricity_price: number }).electricity_price;
      }
      return route.fulfill({ json: { electricity_price: price } });
    }
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "test", expires_at: "2099-01-01" };
    if (path.endsWith("/machines"))
      body = [{ id, name: "rig-1", host: "10.0.0.2", port: 22, username: "root", tags: [], policy: {}, bmc: null, group: "", is_controller: false }];
    if (path.includes("telemetry"))
      body = [{ machine_id: id, kind: "system", observed_at: new Date().toISOString(), error: null, data: { cpu_pct: 90, cpu_power_w: 500 } }];
    await route.fulfill({ json: body });
  });

  await page.goto("/machines");
  const card = page.getByRole("link", { name: /预估电费/ });
  // 0.5 kW × 24 h × 1 元/度
  await expect(card).toContainText("¥12");
  await expect(card).toContainText("1 元/度");

  await card.click();
  await expect(page).toHaveURL(/\/settings\?tab=farm$/);
  const input = page.getByLabel("电价");
  await expect(input).toHaveValue("1.0000");
  await input.fill("0.35");
  await page.getByRole("button", { name: /保\s*存/ }).click();
  await expect.poll(() => saved).toEqual([{ electricity_price: 0.35 }]);

  await page.locator('nav.rd-tabs a[href="/machines"]').click();
  // 0.5 kW × 24 h × 0.35 元/度
  await expect(card).toContainText("¥4.2");
  await expect(card).toContainText("0.35 元/度");
});
