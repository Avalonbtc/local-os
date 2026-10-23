import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";

// HiveOS flight sheet page: wallet dialog opened from a task keeps its coin, the row menu
// exports a sheet, "添加自 → 文件" loads it back, and Applied / Not Applied filter by rig reports.
test("flight sheet export, import, applied filter and locked wallet coin", async ({ page }) => {
  const posts: Record<string, any>[] = [];
  const sheets = [
    { id: "f1", name: "btx", version: 2, tasks: [{ instance: "cpu-1", wallet_id: "w1", config: { urls: ["stratum+tcp://pool:3333"], algorithm: "rx/0" }, miner: { adapter: "xmrig", name: "XMRig", version: "6.26.0", url: "https://x/xmrig.tar.gz", executable: "xmrig" } }] },
    { id: "f2", name: "idle", version: 1, tasks: [{ instance: "cpu-1", wallet_id: "w1", config: { urls: [] }, miner: { adapter: "hive-custom", name: "btxgpuminer", url: "https://x/b.tar.gz" } }] },
  ];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id: "a", name: "fixture", token_id: null }, csrf: "t", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56, coin_icons: {} };
    if (path.endsWith("/catalog/wallets")) {
      if (request.method() === "POST") {
        posts.push(request.postDataJSON());
        body = { id: "w9", kind: "wallets", name: "new", revision: 1, data: request.postDataJSON().data };
      } else body = [{ id: "w1", kind: "wallets", name: "main", revision: 1, data: { coin_symbol: "BTX", address: "btx1q" } }];
    }
    if (path.endsWith("/adapters")) body = [{ id: "xmrig", name: "XMRig", releases: [{ version: "6.26.0", url: "https://x/xmrig.tar.gz", executable: "xmrig" }] }];
    if (path.endsWith("/flight-sheets")) body = sheets;
    if (path.includes("telemetry"))
      body = [{ machine_id: "m", kind: "mining", observed_at: new Date().toISOString(), error: null, data: { instances: [{ instance: "cpu-1", sheet_id: "f1" }] } }];
    await route.fulfill({ json: body });
  });
  await page.goto("/flight-sheets");
  await expect(page.locator(".flight-card")).toHaveCount(2);
  await page.getByRole("button", { name: "Not Applied" }).click();
  await expect(page.locator(".flight-card")).toHaveCount(1);
  await expect(page.locator(".flight-card")).toContainText("idle");
  await page.getByRole("button", { name: "Applied", exact: true }).click();
  await expect(page.locator(".flight-card")).toContainText("btx");

  // Row menu → 导出 downloads a RigDeck export of the sheet.
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "btx 操作" }).click();
  await page.getByRole("menuitem", { name: "导出" }).click();
  const file = await (await download).path();
  const exported = JSON.parse(readFileSync(file, "utf8"));
  expect(exported).toMatchObject({ rigdeck_flight_sheet: 1, name: "btx", tasks: [{ coin: "BTX", wallet_id: "w1", wallet_name: "main" }] });

  // 添加自 → 文件 loads it into the editor.
  const chooser = page.waitForEvent("filechooser");
  await page.getByRole("button", { name: "文件" }).click();
  await (await chooser).setFiles({ name: "btx.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(exported)) });
  await expect(page.locator(".composer-bottom input")).toHaveValue("btx");
  await expect(page.locator(".flight-field").nth(1)).toContainText("main");
  await expect(page.locator(".fs-pool")).toHaveText("pool:3333");

  // 添加钱包 from the task keeps the task's coin, like HiveOS.
  await page.locator(".fs-card-link", { hasText: "添加钱包" }).click();
  const dialog = page.getByRole("dialog", { name: "新的钱包" });
  await expect(dialog.locator(".wallet-locked-coin")).toContainText("BTX");
  await dialog.getByPlaceholder("输入钱包地址").fill("btx1qnew");
  await dialog.getByPlaceholder("输入钱包名称").fill("new");
  await dialog.getByRole("button", { name: "创建" }).click();
  await expect.poll(() => posts.length).toBe(1);
  expect(posts[0].data).toMatchObject({ coin_symbol: "BTX", address: "btx1qnew" });
});
