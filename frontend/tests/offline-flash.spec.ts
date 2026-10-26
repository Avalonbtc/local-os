import { test, expect, type Page, type Route } from "@playwright/test";

// Rigs must not flash "掉线" while telemetry is merely late: before the first sample, or when the
// refresh is pending (tab was in the background, request in flight). A failed refresh still
// ages them out.
const id = "00000000-0000-4000-8000-000000000001";

async function mock(page: Page, telemetry: (route: Route) => Promise<void> | void) {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.includes("telemetry")) return telemetry(route);
    let body: unknown = [];
    if (path.endsWith("/me")) body = { actor: { id, name: "fixture", token_id: null }, csrf: "t", expires_at: "2099-01-01" };
    if (path.endsWith("/settings/farm")) body = { electricity_price: 0.56 };
    if (path.endsWith("/machines")) body = [{ id, name: "rig-1", host: "10.0.0.1", tags: [], policy: {}, bmc: null, group: "" }];
    await route.fulfill({ json: body });
  });
}
const sample = (at: number) => [
  { machine_id: id, kind: "system", observed_at: new Date(at).toISOString(), error: null, data: { cpu_pct: 50, load: [1, 1, 1], gpus: [] } },
];

test("no offline rows before the first telemetry arrives", async ({ page }) => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  await mock(page, async (route) => {
    await gate;
    await route.fulfill({ json: sample(Date.now()) });
  });
  await page.goto("/machines");
  // Machines are known, telemetry is not: loading, never "掉线".
  await expect(page.locator(".ant-spin")).toBeVisible();
  await expect(page.getByText(/掉线 ·/)).toHaveCount(0);
  await expect(page.locator(".hive-stat-ratio em")).toHaveText("—");
  release();
  const row = page.locator(".hive-machine-row", { hasText: "rig-1" });
  await expect(row).toBeVisible();
  await expect(row).not.toContainText("掉线");
  await expect(page.locator(".hive-stat-ratio em")).toHaveText("0");
});

/** Pretend the tab went to the background / came back (browsers pause polling while hidden). */
async function setHidden(page: Page, hidden: boolean) {
  await page.evaluate((hidden) => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (hidden ? "hidden" : "visible") });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event("visibilitychange", { bubbles: true }));
  }, hidden);
}

test("rigs stay online while the tab was in the background, then refresh on return", async ({ page }) => {
  await page.clock.install();
  let calls = 0;
  let hold: Promise<void> | undefined;
  await mock(page, async (route) => {
    calls += 1;
    if (hold) await hold;
    await route.fulfill({ json: sample(await page.evaluate(() => Date.now())) });
  });
  await page.goto("/machines");
  const row = page.locator(".hive-machine-row", { hasText: "rig-1" });
  await expect(row).toBeVisible();
  await setHidden(page, true);
  const before = calls;
  // Five minutes in the background: no polling, and the last data is judged when it arrived.
  await page.clock.fastForward("05:00");
  expect(calls).toBe(before);
  // Back in front: refreshed at once (not on the next 10 s tick). While that refresh is still on
  // its way the rig must not read as offline.
  let release!: () => void;
  hold = new Promise<void>((resolve) => (release = resolve));
  await setHidden(page, false);
  await expect.poll(() => calls).toBeGreaterThan(before);
  // Any re-render while waiting (here: the 在线 filter) must still count the rig as online.
  await page.getByRole("group", { name: "状态筛选" }).getByRole("button", { name: /在线/ }).click();
  await expect(row).toBeVisible({ timeout: 1500 });
  await expect(row).not.toContainText("掉线", { timeout: 1500 });
  release();
  hold = undefined;
  await expect(row).not.toContainText("掉线");
});

test("a controller that stops answering still ages rigs out", async ({ page }) => {
  await page.clock.install();
  let mode: "ok" | "hang" | "fail" = "ok";
  await mock(page, async (route) => {
    if (mode === "hang") return; // never answered
    if (mode === "fail") return route.fulfill({ status: 503, json: { error: "down" } });
    await route.fulfill({ json: sample(await page.evaluate(() => Date.now())) });
  });
  await page.goto("/machines");
  const row = page.locator(".hive-machine-row", { hasText: "rig-1" });
  await expect(row).toBeVisible();
  await expect(row).not.toContainText("掉线");
  // Requests hang: after the 15 s deadline the refresh counts as failed and the rig ages out.
  mode = "hang";
  await page.clock.runFor("01:00");
  await expect(row).toContainText("掉线 · 最后在线");
  // Answers again: back online; then errors age it out the same way.
  mode = "ok";
  await page.clock.runFor("00:20");
  await expect(row).not.toContainText("掉线");
  mode = "fail";
  await page.clock.runFor("01:00");
  await expect(row).toContainText("掉线 · 最后在线");
});
