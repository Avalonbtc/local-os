import { test, expect } from "@playwright/test";

const token = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "默认安装密钥",
  token: "rgk_" + "a".repeat(64),
  command: "curl -fsSL https://panel.example.com/agent/install.sh | sudo bash -s -- rgk_" + "a".repeat(64),
  group: "",
  tags: [],
  expires_at: null,
  max_uses: null,
  uses: 0,
  revoked_at: null,
  created_by: "fixture",
  created_at: "2026-10-21T00:00:00Z",
};
const rig = {
  id: "22222222-2222-4222-8222-222222222222",
  name: "rig-new",
  host: "10.0.0.7",
  port: 22,
  username: "root",
  host_key: "agent",
  tags: [],
  policy: {},
  group: "",
  is_controller: false,
  transport: "agent",
  created_at: "2026-10-21T00:00:00Z",
};

test("添加矿机 creates the first install key, shows the command and the rig that joins", async ({ page }) => {
  let tokens: any[] = [];
  let machines: any[] = [];
  const created: any[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: any = [];
    if (path.endsWith("/me")) body = { actor: { id: rig.id, name: "fixture", token_id: null }, csrf: "fixture", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
    if (path.endsWith("/machines")) body = machines;
    if (path.endsWith("/agents")) body = machines.map((m) => ({ machine_id: m.id, connected_at: new Date().toISOString(), version: "0.5.0", hostname: m.name, address: m.host }));
    if (path.endsWith("/enrollment-tokens")) {
      if (request.method() === "POST") {
        created.push(request.postDataJSON());
        tokens = [token];
        body = token;
      } else body = tokens;
    }
    await route.fulfill({ json: body });
  });
  await page.goto("/machines");
  await page.getByRole("button", { name: "添加第一台矿机" }).click();
  const dialog = page.getByRole("dialog", { name: "添加矿机" });
  await expect(dialog.getByText(token.command)).toBeVisible();
  expect(created).toEqual([{ name: "默认安装密钥", group: "", tags: [] }]);
  await expect(dialog.getByText("不需要开放任何端口")).toBeVisible();
  // The rig runs the command: it shows up in the dialog without a reload.
  machines = [rig];
  await expect(dialog.getByRole("button", { name: "rig-new" })).toBeVisible({ timeout: 15000 });
  await expect(dialog.getByText("已加入 1 台")).toBeVisible();
  await dialog.getByRole("button", { name: /^完\s?成$/ }).click();
  await expect(page.getByRole("region", { name: "矿场统计" })).toContainText("主动连接");
});

test("设定 → 安装密钥 lists keys, copies their command and revokes", async ({ page }) => {
  let revoked = false;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: any = [];
    if (path.endsWith("/me")) body = { actor: { id: rig.id, name: "fixture", token_id: null }, csrf: "fixture", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
    if (path.endsWith("/enrollment-tokens")) body = [{ ...token, uses: 3, max_uses: 10, revoked_at: revoked ? "2026-10-21T01:00:00Z" : null }];
    if (path.includes("/enrollment-tokens/") && request.method() === "DELETE") {
      revoked = true;
      return route.fulfill({ status: 204, body: "" });
    }
    await route.fulfill({ json: body });
  });
  await page.goto("/settings?tab=install");
  const row = page.getByRole("row").filter({ hasText: "默认安装密钥" });
  await expect(row).toContainText("3 / 10");
  await expect(row).toContainText("可用");
  await row.getByRole("button", { name: /展开|Expand/ }).click();
  await expect(page.getByText(token.command)).toBeVisible();
  await row.getByRole("button", { name: /^撤\s?销$/ }).click();
  await page.locator(".ant-popconfirm").getByRole("button", { name: /^撤\s?销$/ }).click();
  await expect(page.getByRole("row").filter({ hasText: "默认安装密钥" })).toContainText("已撤销");
});
