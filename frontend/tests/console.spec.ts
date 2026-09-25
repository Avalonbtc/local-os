import { test, expect } from "@playwright/test";
import type { Page } from "@playwright/test";
const password = process.env.RIGDECK_TEST_PASSWORD ?? "RigDeck-local-test-2026";
async function login(page: Page) {
  await page.goto("/");
  await page.getByLabel("用户名").fill("admin");
  await page.getByLabel("密码", { exact: false }).fill(password);
  await page.getByRole("button", { name: "登录", exact: false }).click();
  await expect(page.locator('nav.rd-tabs a[href="/machines"]')).toBeVisible();
}

test("login, custom coin from wallet, navigation, token revocation and responsive layout", async ({
  page,
}) => {
  const exceptions: string[] = [];
  page.on("pageerror", (e) => exceptions.push(e.message));
  await login(page);
  await page.screenshot({ path: "../screenshots/fleet.png", fullPage: true });
  await page.locator('nav.rd-tabs a[href="/wallets"]').click();
  await page.getByRole("button", { name: "添加钱包" }).click();
  const dialog = page.getByRole("dialog");
  const coin = `TEST-${Date.now()}`;
  const wallet = `验收钱包-${Date.now()}`;
  await dialog.getByLabel("名称", { exact: true }).fill(wallet);
  await dialog.getByLabel("数字货币").fill(coin);
  await dialog.getByLabel("地址", { exact: true }).fill("test-pool-account");
  await dialog.getByRole("button", { name: "创 建" }).click();
  await expect(dialog).not.toBeVisible();
  await page.getByRole("searchbox", { name: "搜索钱包" }).fill(wallet);
  await expect(page.getByText(wallet, { exact: true })).toBeVisible();
  // The wallet cell shows the coin above the wallet name, as on HiveOS.
  await expect(
    page.getByRole("row").filter({ hasText: wallet }).getByText(coin, { exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: "../screenshots/wallets.png", fullPage: true });
  // Coins, pools, miner software and the jobs page are gone: rigs download miners themselves
  // and job results live in each rig's messages.
  for (const [label, heading] of [
    ["飞行表", "添加新的飞行表"],
    ["超频", "超频模板"],
    ["BMC 设备", "BMC"],
    ["BIOS", "BIOS 管理"],
    ["多机终端", "多机终端"],
    ["设定", "设置"],
  ]) {
    await page.locator("nav.rd-tabs").getByText(label, { exact: true }).click();
    await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible();
  }
  for (const gone of ["/coins", "/pools", "/miners", "/jobs"])
    await expect(page.locator(`nav.rd-tabs a[href="${gone}"]`)).toHaveCount(0);
  const tokenName = `browser-validation-${Date.now()}`;
  await page.getByRole("textbox", { name: "令牌名称" }).fill(tokenName);
  await page.getByRole("button", { name: "创建令牌" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.locator(".token-value")).toContainText("rd_");
  await page.getByRole("dialog").getByRole("button", { name: "确 定" }).click();
  const row = page.getByRole("row").filter({ hasText: tokenName });
  await row.getByRole("button", { name: /撤\s*销/ }).click();
  await page.getByRole("button", { name: "确 定" }).click();
  await expect(row).toContainText("已撤销");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('nav.rd-tabs a[href="/machines"]').click();
  await page.screenshot({ path: "../screenshots/mobile.png", fullPage: true });
  expect(exceptions).toEqual([]);
});

test("API CSRF, encrypted credentials omission, idempotency, shared MCP auth", async ({
  request,
}) => {
  const loginResponse = await request.post("/api/v1/login", {
    data: { username: "admin", password },
  });
  expect(loginResponse.ok()).toBeTruthy();
  const session = await loginResponse.json();
  const headers = { "x-csrf-token": session.csrf };
  expect(
    (
      await request.post("/api/v1/tokens", {
        data: { name: "must-fail-without-csrf" },
      })
    ).status(),
  ).toBe(403);
  expect(
    (
      await request.post("/api/v1/tokens", {
        headers: { ...headers, origin: "https://evil.example" },
        data: { name: "must-fail-origin" },
      })
    ).status(),
  ).toBe(403);
  const tokenResponse = await request.post("/api/v1/tokens", {
    headers,
    data: { name: "api-validation" },
  });
  expect(tokenResponse.ok()).toBeTruthy();
  const token = await tokenResponse.json();
  const auth = {
    authorization: `Bearer ${token.token}`,
    accept: "application/json, text/event-stream",
  };
  const init = await request.post("/mcp", {
    headers: auth,
    data: {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: "rigdeck-test", version: "1" },
      },
    },
  });
  expect(init.ok()).toBeTruthy();
  expect(await init.text()).toContain("serverInfo");
  const tools = await request.post("/mcp", {
    headers: { ...auth, "mcp-protocol-version": "2025-03-26" },
    data: { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
  });
  expect(tools.ok()).toBeTruthy();
  expect(await tools.text()).toContain("job_submit");
  const mcpName = `MCP-wallet-${Date.now()}`;
  const saved = await request.post("/mcp", {
    headers: { ...auth, "mcp-protocol-version": "2025-03-26" },
    data: {
      jsonrpc: "2.0",
      id: 4,
      method: "tools/call",
      params: {
        name: "catalog_save",
        arguments: {
          kind: "wallet",
          input: {
            name: mcpName,
            data: { coin_symbol: `MCP-${Date.now()}`, address: "test-account" },
          },
        },
      },
    },
  });
  expect(saved.ok()).toBeTruthy();
  expect(await saved.text()).not.toContain('"isError":true');
  const wallets = await (
    await request.get("/api/v1/catalog/wallets", { headers })
  ).json();
  expect(
    wallets.some((w: { name: string }) => w.name === mcpName),
  ).toBeTruthy();
  const machineResponse = await request.post("/api/v1/machines", {
    headers,
    data: {
      name: `offline-test-${Date.now()}`,
      host: "127.0.0.1",
      port: 9,
      username: "test",
      host_key: "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
      credential: {
        kind: "password",
        password: "never-return-this-credential",
      },
      group: "验收隔离节点",
      tags: ["test"],
      policy: {},
      is_controller: false,
    },
  });
  expect(machineResponse.ok()).toBeTruthy();
  const machine = await machineResponse.json();
  expect(JSON.stringify(machine)).not.toContain("never-return-this-credential");
  const command = {
    machine_ids: [machine.id],
    action: {
      kind: "command",
      script: "printf validation",
      timeout_seconds: 5,
    },
    idempotency_key: crypto.randomUUID(),
    concurrency: 1,
    canary: true,
    include_controller: false,
  };
  const first = await request.post("/api/v1/jobs", { headers, data: command });
  expect(first.ok()).toBeTruthy();
  const job = await first.json();
  const second = await request.post("/api/v1/jobs", { headers, data: command });
  expect((await second.json()).job_id).toBe(job.job_id);
  const conflict = await request.post("/api/v1/jobs", {
    headers,
    data: { ...command, concurrency: 2 },
  });
  expect(conflict.status()).toBe(409);
  expect(
    (await request.delete(`/api/v1/tokens/${token.id}`, { headers })).ok(),
  ).toBeTruthy();
  expect(
    (
      await request.post("/mcp", {
        headers: auth,
        data: { jsonrpc: "2.0", id: 3, method: "tools/list" },
      })
    ).status(),
  ).toBe(401);
});

test("real SSH PTY and machine detail tabs on an isolated lab node", async ({
  page,
}) => {
  test.skip(process.env.RIGDECK_LAB !== "1", "requires tests/lab/run.py");
  await login(page);
  const machines = await (await page.request.get("/api/v1/machines")).json();
  const machine = machines.find(
    (m: { name: string; port: number; tags: string[] }) =>
      m.name === "LAB-ubuntu-01" && m.port === 22221 && m.tags.includes("lab"),
  );
  expect(machine).toBeTruthy();
  let received = "";
  page.on("websocket", (socket) =>
    socket.on("framereceived", (event) => {
      received += event.payload.toString();
    }),
  );
  await page.getByRole("link", { name: "LAB-ubuntu-01", exact: true }).click();
  for (const tab of ["消息", "飞行表", "运行软件", "性能", "传感器", "日志", "设置"]) {
    await page.getByRole("tab", { name: tab, exact: true }).click();
  }
  await page.getByRole("tab", { name: "SSH 终端", exact: true }).click();
  await expect(page.getByText("已连接", { exact: true })).toBeVisible();
  await expect.poll(() => received).toContain("root@");
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("printf 'RIG%s\\n' 'DECK_TERMINAL_OK'");
  await page.keyboard.press("Enter");
  await expect.poll(() => received).toContain("RIGDECK_TERMINAL_OK");
  await page.screenshot({
    path: "../screenshots/terminal.png",
    fullPage: true,
  });
  await page.getByRole("tab", { name: "概览", exact: true }).click();
  await page.screenshot({ path: "../screenshots/machine.png", fullPage: true });
  const session = await (await page.request.get("/api/v1/me")).json();
  const headers = { "x-csrf-token": session.csrf };
  const control = async (operation: string) => {
    const response = await page.request.post("/api/v1/jobs", {
      headers,
      data: {
        machine_ids: [machine.id],
        action: { kind: "miner", operation, instances: ["lab-cpu"] },
        idempotency_key: crypto.randomUUID(),
        concurrency: 1,
        canary: true,
        include_controller: false,
      },
    });
    expect(response.ok()).toBeTruthy();
    const job = await response.json();
    await expect
      .poll(
        async () =>
          (await (await page.request.get(`/api/v1/jobs/${job.job_id}`)).json())
            .status,
        { timeout: 15000 },
      )
      .toBe("succeeded");
  };
  await control("start");
  try {
    received = "";
    await page.getByRole("tab", { name: "矿工控制台", exact: true }).click();
    await expect(page.getByText("已连接", { exact: true })).toBeVisible();
    await expect.poll(() => received).toContain("\x1b");
    expect(received).not.toContain("There is no screen");
    await page.getByRole("tab", { name: "概览", exact: true }).click();
    await expect
      .poll(
        async () => {
          const observations = await (
            await page.request.get(`/api/v1/telemetry?machine_id=${machine.id}`)
          ).json();
          const sample = observations.find(
            (o: { kind: string }) => o.kind === "mining",
          );
          return sample?.data?.instances?.some(
            (i: {
              instance: string;
              process_alive: boolean;
              stats_observed_at: number;
            }) =>
              i.instance === "lab-cpu" &&
              i.process_alive &&
              Date.now() / 1000 - i.stats_observed_at < 30,
          );
        },
        { timeout: 30000 },
      )
      .toBeTruthy();
  } finally {
    await control("stop");
  }
});

test("expired session returns to login instead of leaving stale authenticated pages", async ({ page }) => {
  await login(page);
  await page.route("**/api/v1/machines", route => route.fulfill({status:401,contentType:"application/json",body:JSON.stringify({error:"登录已过期"})}));
  await page.getByRole("button", { name:"刷新数据" }).click();
  await expect(page.getByLabel("用户名")).toBeVisible();
  await expect(page.getByRole("button", { name:"登录", exact:false })).toBeVisible();
});

test("late close from previous terminal cannot erase the replacement sender", async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any;
    w.fixtureSockets=[];
    const OriginalSocket=w.WebSocket;
    class FixtureSocket {
      static OPEN=1; static CLOSED=3;
      readyState=0; binaryType=""; onopen:any=null; onmessage:any=null; onerror:any=null; onclose:any=null; sent:any[]=[];
      constructor(_url:string) { if (!_url.includes("/api/v1/machines/")) return new OriginalSocket(_url); w.fixtureSockets.push(this);setTimeout(()=>{this.readyState=1;this.onopen?.({});},10); }
      send(value:any){this.sent.push(value);}
      close(){this.readyState=3;setTimeout(()=>this.onclose?.({}),100);}
    }
    w.WebSocket=FixtureSocket;
  });
  const ids=["11111111-1111-4111-8111-111111111111","22222222-2222-4222-8222-222222222222"];
  await page.route("**/api/v1/machines",route=>route.fulfill({json:ids.map((id,n)=>({id,name:`fixture-${n}`,host:"127.0.0.1",port:22,username:"fixture",host_key:"fixture",group:"",tags:[],policy:{},is_controller:false,created_at:new Date().toISOString()}))}));
  await login(page);
  await page.goto(`/terminals?ids=${ids.join(",")}`);
  await expect(page.getByText("已连接",{exact:true})).toHaveCount(2);
  await page.evaluate(()=>{const s=(window as any).fixtureSockets.find((s:any)=>s.onclose && s.readyState===1);s.readyState=3;s.onclose?.({});});
  await page.getByRole("button",{name:"重新连接"}).first().click();
  await expect(page.getByText("已连接",{exact:true})).toHaveCount(2);
  await page.waitForTimeout(180); // Deliver the queued old close after replacement has opened.
  await expect(page.getByText("已连接",{exact:true})).toHaveCount(2);
  await page.getByRole("checkbox",{name:"开启同步输入"}).check();
  for (const checkbox of await page.getByRole("checkbox",{name:"接收同步输入"}).all()) await checkbox.check();
  await page.locator(".xterm-helper-textarea").nth(1).press("a");
  await expect.poll(()=>page.evaluate(()=> (window as any).fixtureSockets.at(-1).sent.some((x:any)=>x instanceof Uint8Array && new TextDecoder().decode(x)==="a"))).toBe(true);
});
