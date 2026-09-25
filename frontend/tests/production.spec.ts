import { test, expect } from "@playwright/test";

// The fixture CA is independently verified by deploy_smoke.py; it is not installed
// into the developer's OS/browser trust store.
test.use({ baseURL: "https://localhost:18443", ignoreHTTPSErrors: true });
test("production static assets, CSP, secure session and navigation", async ({
  page,
}) => {
  test.skip(process.env.RIGDECK_PROD !== "1", "requires tests/deploy_smoke.py");
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("response", (response) => {
    if (response.url().includes("/assets/") && response.status() >= 400)
      errors.push(`${response.status()} ${response.url()}`);
  });
  await page.goto("/");
  await expect(page.getByLabel("用户名")).toBeVisible();
  await page.screenshot({ path: "../screenshots/login.png", fullPage: true });
  await page.getByLabel("用户名").fill("smoke-admin");
  await page
    .getByLabel("密码", { exact: false })
    .fill("RigDeck-smoke-test-2026");
  await page.getByRole("button", { name: "登录", exact: false }).click();
  await expect(page.locator('nav.rd-tabs a[href="/machines"]')).toBeVisible();
  for (const [label, heading] of [
    ["钱包", "钱包"],
    ["飞行表", "添加新的飞行表"],
    ["BMC 设备", "BMC"],
    ["设定", "设置"],
  ]) {
    await page.locator("nav.rd-tabs").getByText(label, { exact: true }).click();
    await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible();
  }
  expect(errors).toEqual([]);
});
