import { test, expect } from "@playwright/test";

const machines = Array.from({ length: 12 }, (_, n) => ({
  id: `00000000-0000-4000-8000-${String(n + 1).padStart(12, "0")}`,
  name: `batch-${String(n + 1).padStart(2, "0")}`,
  host: `10.0.0.${n + 1}`,
  tags: [], policy: {}, group: "", is_controller: false,
}));

for (const [kind, button] of [
  ["command", "执行命令"],
  ["miner", "矿工控制"],
  ["apply", "应用飞行表"],
  ["bootstrap", "部署 / 更新运行层"],
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
    if (kind === "bootstrap") {
      await page.getByRole("button", { name: /更多/ }).hover();
      await page.getByRole("menuitem", { name: button }).click();
    } else {
      await page.locator(".hive-batch-bar").getByRole("button", { name: new RegExp(button) }).click();
    }
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("并发机器数", { exact: true })).toHaveCount(0);
    await expect(dialog.getByText("分批执行", { exact: true })).toHaveCount(0);
    if (kind === "command") await dialog.getByLabel("Shell 脚本").fill("uname -a");
    if (kind === "miner") await dialog.getByPlaceholder("cpu-1,cpu-2").fill("cpu-1");
    if (kind === "apply") {
      await dialog.getByRole("combobox").click();
      await page.getByTitle("Test flight · v1 · 0 个任务").click();
    }
    await dialog.getByRole("button", { name: "创建任务" }).click();
    await expect.poll(() => requests.length).toBe(1);
    const expected = machines.filter((m) => kind !== "command" || m.name !== "batch-02").map((m) => m.id);
    expect(requests[0].machine_ids).toEqual(expected);
    expect(requests[0].action.kind).toBe(kind);
    expect(requests[0]).not.toHaveProperty("concurrency");
    expect(requests[0]).not.toHaveProperty("canary");
  });
}
