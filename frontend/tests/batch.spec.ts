import { test, expect } from "@playwright/test";

const machines = Array.from({ length: 12 }, (_, n) => ({
  id: `00000000-0000-4000-8000-${String(n + 1).padStart(12, "0")}`,
  name: `batch-${String(n + 1).padStart(2, "0")}`,
  host: `10.0.0.${n + 1}`,
  tags: [], policy: {}, group: "", is_controller: false,
  bmc: { provider: "ipmi", url: `10.0.1.${n + 1}`, username: "ADMIN" },
}));

for (const [kind, button] of [
  ["command", "运行命令"],
  ["miner", "挖矿软件动作"],
  ["apply", "选择飞行表"],
  ["bootstrap", "部署 / 更新运行层"],
  ["power", "电源动作"],
]) {
  test(`${kind} sends every selected machine without concurrency or canary controls`, async ({ page }) => {
    const requests: any[] = [];
    await page.route("**/api/v1/**", async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      let body: any = [];
      if (path.endsWith("/me")) body = { actor: { id: machines[0].id, name: "fixture" }, csrf: "fixture", expires_at: "2099-01-01" };
      if (path.endsWith("/machines")) body = machines;
      if (path.endsWith("/flight-sheets")) body = [{ id: "fixture-flight", name: "Test flight", version: 1, tasks: [] }];
      if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
      if (path.endsWith("/jobs") && request.method() === "POST") {
        requests.push(request.postDataJSON());
        body = { job_id: "fixture-job" };
      }
      await route.fulfill({ json: body });
    });
    await page.goto("/machines");
    await page.getByRole("checkbox", { name: "选择本页所有矿机" }).check();
    // The command case also proves that an unchecked machine is excluded.
    if (kind === "command") await page.getByRole("checkbox", { name: "选择 batch-02", exact: true }).uncheck();
    // HiveOS bulk mode: the top bar shows the action icons.
    const bar = page.getByRole("toolbar", { name: /批量操作/ });
    await bar.getByRole("button", { name: button, exact: true }).click();
    if (kind === "power") {
      await page.getByRole("menuitem", { name: "开机" }).click();
      await page.getByRole("dialog").getByRole("button", { name: /确\s*定|OK/ }).click();
    }
    const dialog = page.getByRole("dialog").last();
    await expect(dialog.getByText("并发机器数", { exact: true })).toHaveCount(0);
    await expect(dialog.getByText("分批执行", { exact: true })).toHaveCount(0);
    if (kind === "command") {
      // HiveOS "矿机命令": common commands fill the prompt, 运行 sends it.
      await dialog.getByRole("button", { name: "free -h && df -h /" }).click();
      await dialog.getByLabel("命令").fill("uname -a");
      await dialog.getByRole("button", { name: "运行" }).click();
    }
    if (kind === "miner") await dialog.getByPlaceholder("cpu-1,cpu-2").fill("cpu-1");
    if (kind === "apply") {
      // HiveOS picker under the rocket: click a sheet, then confirm.
      await page.getByRole("dialog", { name: "选择飞行表" }).getByRole("button", { name: /Test flight/ }).click();
      await page.getByRole("button", { name: /应\s*用/ }).last().click();
    }
    if (kind === "miner" || kind === "bootstrap") await dialog.getByRole("button", { name: "创建任务" }).click();
    await expect.poll(() => requests.length).toBe(1);
    const expected = machines.filter((m) => kind !== "command" || m.name !== "batch-02").map((m) => m.id);
    expect(requests[0].machine_ids).toEqual(expected);
    expect(requests[0].action.kind).toBe(kind);
    expect(requests[0]).not.toHaveProperty("concurrency");
    expect(requests[0]).not.toHaveProperty("canary");
  });
}

test("bulk mode: crumb, selected filter and tags for every ticked worker", async ({ page }) => {
  const puts: Record<string, any>[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: any = [];
    if (path.endsWith("/me")) body = { actor: { id: machines[0].id, name: "fixture" }, csrf: "fixture", expires_at: "2099-01-01" };
    if (path.endsWith("/machines")) body = machines;
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
    if (request.method() === "PUT") {
      puts.push({ path, ...request.postDataJSON() });
      body = {};
    }
    await route.fulfill({ json: body });
  });
  await page.goto("/machines");
  await page.getByRole("checkbox", { name: "选择 batch-03", exact: true }).check();
  await page.getByRole("checkbox", { name: "选择 batch-05", exact: true }).check();
  await expect(page.locator(".rd-unselect")).toHaveText("‹ 取消选定 2 矿机");
  await page.getByRole("button", { name: /已选择/ }).click();
  await expect(page.locator(".hive-machine-row")).toHaveCount(2);
  await page.getByRole("toolbar", { name: /批量操作/ }).getByRole("button", { name: "编辑标签" }).click();
  const dialog = page.getByRole("dialog", { name: /编辑标签/ });
  await dialog.getByRole("combobox", { name: "添加标签" }).fill("rack-b");
  await page.keyboard.press("Enter");
  await dialog.getByRole("button", { name: /保\s*存/ }).click();
  await expect.poll(() => puts.length).toBe(2);
  expect(puts.map((p) => p.tags)).toEqual([["rack-b"], ["rack-b"]]);
  expect(puts.map((p) => p.path.split("/").pop())).toEqual([machines[2].id, machines[4].id]);
  await page.locator(".rd-unselect").click();
  await expect(page.getByRole("toolbar", { name: /批量操作/ })).toHaveCount(0);
});
