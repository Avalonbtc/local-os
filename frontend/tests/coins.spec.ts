import { test, expect } from "@playwright/test";

// Coin icons come from the controller's icon library first, then the bundled set, then a letter.
// Uploads, links and CoinGecko picks all go to the controller; the browser never calls CoinGecko.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFklEQVR4nGP8z8DwnwEJMDGgAQYGBgB+8gIB2e4z3wAAAABJRU5ErkJggg==",
  "base64",
);

test("icon library: bundled, letter, upload, link and CoinGecko through the controller", async ({ page }) => {
  const library: Record<string, any>[] = [];
  const puts: { symbol: string; body: Record<string, any> }[] = [];
  const external: string[] = [];
  page.on("request", (r) => {
    if (!new URL(r.url()).host.startsWith("127.0.0.1") && !r.url().startsWith("data:")) external.push(r.url());
  });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id: "a", name: "fixture", token_id: null }, csrf: "t", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.42 };
    if (path.endsWith("/catalog/wallets"))
      body = ["XMR", "TSC", "KAS"].map((s, i) => ({ id: `w${i}`, kind: "wallets", name: `${s} 钱包`, revision: 1, data: { coin_symbol: s, address: `addr${i}` } }));
    if (path === "/api/v1/coin-icons") body = { icons: library, directory: "/var/lib/rigdeck/coin-icons", search: true };
    if (path === "/api/v1/coin-icons/search")
      body = [
        { id: "kaspa", name: "Kaspa", symbol: "KAS", market_cap_rank: 30, thumb: `data:image/png;base64,${PNG.toString("base64")}` },
        { id: "kas-clone", name: "Clone", symbol: "KAS", market_cap_rank: null, thumb: null },
      ];
    const symbol = /^\/api\/v1\/coin-icons\/([A-Z0-9._-]+)$/.exec(path)?.[1];
    if (symbol && request.method() === "PUT") {
      const input = request.postDataJSON();
      puts.push({ symbol, body: input });
      const icon = { symbol, source: input.from, content_type: "image/png", source_ref: input.url ?? input.id ?? null, updated_at: new Date().toISOString() };
      library.splice(0, library.length, ...library.filter((i) => i.symbol !== symbol), icon);
      body = icon;
    } else if (symbol) return route.fulfill({ status: 200, contentType: "image/png", body: PNG });
    await route.fulfill({ json: body });
  });
  await page.goto("/wallets");
  const table = page.locator(".catalog-table");
  await expect(table.locator("img.rd-coin-img[alt=XMR]")).toBeVisible();
  await expect(table.locator(".rd-coin[title=TSC]")).toHaveText("T");

  await page.getByRole("button", { name: /币种图标/ }).click();
  const dialog = page.getByRole("dialog", { name: "币种图标" });
  await expect(dialog).toContainText("/var/lib/rigdeck/coin-icons");

  // Upload: the file goes to the controller as is.
  const tsc = dialog.locator("li", { hasText: "TSC" });
  await expect(tsc).toContainText("无图标");
  const chooser = page.waitForEvent("filechooser");
  await tsc.getByRole("button", { name: "上传" }).click();
  await (await chooser).setFiles({ name: "tsc.png", mimeType: "image/png", buffer: PNG });
  await expect.poll(() => puts.length).toBe(1);
  expect(puts[0]).toMatchObject({ symbol: "TSC", body: { from: "upload", content_type: "image/png", data_base64: PNG.toString("base64") } });
  await expect(tsc).toContainText("上传");
  await expect(table.locator("img.rd-coin-img[alt=TSC]")).toHaveAttribute("src", /^\/api\/v1\/coin-icons\/TSC\?v=\d+$/);

  // Link: the controller downloads it.
  const xmr = dialog.locator("li", { hasText: "XMR" });
  await xmr.getByRole("button", { name: "链接" }).click();
  await page.getByLabel("图片链接").fill("https://example.com/xmr.png");
  await page.getByRole("button", { name: "下载到主控" }).click();
  await expect.poll(() => puts.length).toBe(2);
  expect(puts[1]).toEqual({ symbol: "XMR", body: { from: "url", url: "https://example.com/xmr.png" } });

  // CoinGecko: candidates come from the controller (thumbnails as data URLs); pick by name.
  const kas = dialog.locator("li", { hasText: "KAS" });
  await kas.getByRole("button", { name: "CoinGecko" }).click();
  await page.getByRole("button", { name: /Kaspa/ }).click();
  await expect.poll(() => puts.length).toBe(3);
  expect(puts[2]).toEqual({ symbol: "KAS", body: { from: "coingecko", id: "kaspa" } });
  await expect(kas).toContainText("CoinGecko");
  expect(external.filter((u) => /coingecko|example\.com/.test(u))).toEqual([]);
});
