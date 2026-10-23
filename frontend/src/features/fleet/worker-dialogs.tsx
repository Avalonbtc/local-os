// HiveOS worker command dialog ("矿机命令") and the flight sheet picker that drops down from the
// rocket icon. Both act on one worker (its toolbar) or every ticked worker (the bulk bar).
import { useMemo, useState, type ReactNode } from "react";
import { App, Button, Input, Modal, Popover, Select } from "antd";
import { useQuery } from "@tanstack/react-query";
import { client, data, submitJob, unwrap, useFlights, useMachines, type Data } from "../../shared/api";
import { CoinBadge } from "../../shared/coins";
import { useWalletCoins } from "../wallets";
import { REBOOT, SHUTDOWN } from "./worker";

/** Commands that work on a RigDeck Ubuntu rig (HiveOS lists its own hive tools here). */
const COMMON: [string, string][] = [
  ["nvidia-smi", "显示 NVIDIA 显卡的相关信息"],
  ["nvidia-smi --query-gpu=index,name,clocks.sm,clocks.mem,power.draw --format=csv", "显示 NVIDIA GPU 时钟与功耗"],
  ["grep -a Xid /var/log/syslog | tail -n 10", "显示 NVIDIA GPU 错误"],
  ["rig-miner status", "显示 RigDeck 托管矿工的状态"],
  ["journalctl -u rigdeck-watchdog --no-pager -n 100", "显示 RigDeck 看门狗最近的日志"],
  ["sensors", "显示主板与 CPU 传感器（需要 lm-sensors）"],
  ["lscpu | grep -E 'Model name|Socket|Thread|NUMA'", "显示 CPU 型号与拓扑"],
  ["free -h && df -h /", "显示内存与磁盘空间"],
  ["ip -br addr", "显示网卡地址"],
];
const TIMEOUTS = [
  { value: 60, label: "1 分钟" },
  { value: 300, label: "5 分钟" },
  { value: 1800, label: "30 分钟" },
  { value: 3600, label: "60 分钟" },
];

export function CommandDialog({ ids, open, onClose }: { ids: string[]; open: boolean; onClose: () => void }) {
  const { message } = App.useApp();
  const machines = useMachines();
  const [script, setScript] = useState("");
  const [timeout, setTimeout] = useState(300);
  const [busy, setBusy] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const jobs = useQuery({
    queryKey: ["jobs"],
    enabled: open,
    queryFn: ({ signal }) => unwrap(client.GET("/api/v1/jobs", { signal })),
  });
  // Recently run commands, newest first, without the toolbar's own reboot / shutdown scripts.
  const recent = useMemo(() => {
    const seen = new Set<string>();
    for (const job of jobs.data ?? []) {
      const action = data(job.action);
      if (action.kind !== "command" || typeof action.script !== "string") continue;
      if ([REBOOT, SHUTDOWN].includes(action.script)) continue;
      seen.add(action.script);
    }
    return [...seen];
  }, [jobs.data]);
  const controller = (machines.data ?? []).some((m) => ids.includes(m.id) && m.is_controller);
  const run = async () => {
    if (!script.trim()) return;
    setBusy(true);
    try {
      await submitJob({
        machine_ids: ids,
        action: { kind: "command", script: script.trim(), timeout_seconds: timeout },
        include_controller: controller,
      });
      message.success(ids.length === 1 ? "已提交，输出会出现在矿机的「消息」里" : `已下发到 ${ids.length} 台矿机，输出见各矿机的「消息」`);
      setScript("");
      onClose();
    } catch (e) {
      message.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const pick = (text: string) => (
    <button key={text} type="button" className="cmd-pick" onClick={() => setScript(text)}>
      {text}
    </button>
  );
  return (
    <Modal open={open} onCancel={onClose} footer={null} width={720} title="矿机命令" className="cmd-dialog" destroyOnHidden>
      <div className="cmd-input">
        <span aria-hidden>&gt;_</span>
        <Input.TextArea
          aria-label="命令"
          autoSize={{ minRows: 2, maxRows: 8 }}
          value={script}
          onChange={(e) => setScript(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void run();
          }}
        />
        <Button className="cmd-run" loading={busy} disabled={!script.trim()} onClick={run}>
          运行
        </Button>
      </div>
      <div className="cmd-meta">
        {ids.length > 1 ? `将在 ${ids.length} 台矿机上同时运行` : "在矿机上以 root 运行"}
        {controller && "（包含主控机器）"} · 超时
        <Select size="small" variant="borderless" value={timeout} onChange={setTimeout} options={TIMEOUTS} />
        · Ctrl+Enter 运行
      </div>
      {recent.length > 0 && (
        <section className="cmd-section">
          <h4>最近使用的命令</h4>
          {(showAll ? recent : recent.slice(0, 10)).map(pick)}
          {recent.length > 10 && (
            <button type="button" className="cmd-more" onClick={() => setShowAll(!showAll)}>
              {showAll ? "Show less" : "Show more"}
            </button>
          )}
        </section>
      )}
      <section className="cmd-section">
        <h4>常用命令</h4>
        <p className="cmd-warn">长时间、占用资源多的命令可能让矿机暂时掉线，请耐心等待任务结束。</p>
        {COMMON.map(([text, help]) => (
          <div key={text} className="cmd-common">
            {pick(text)}
            <span> - {help}</span>
          </div>
        ))}
      </section>
    </Modal>
  );
}

/** HiveOS "Select flight sheet": a panel under the rocket; clicking a sheet applies it. */
export function FlightSheetPicker({ ids, children }: { ids: string[]; children: ReactNode }) {
  const { message, modal } = App.useApp();
  const flights = useFlights();
  const machines = useMachines();
  const { symbol, wallets } = useWalletCoins();
  const [open, setOpen] = useState(false);
  const [coin, setCoin] = useState<string>();
  const coins = [...new Set((flights.data ?? []).flatMap((f) => f.tasks.map((t) => symbol(t.wallet_id))))];
  const shown = (flights.data ?? []).filter((f) => !coin || f.tasks.some((t) => symbol(t.wallet_id) === coin));
  const controller = (machines.data ?? []).some((m) => ids.includes(m.id) && m.is_controller);
  const apply = (sheet: { id: string; name: string }) => {
    setOpen(false);
    modal.confirm({
      title: `应用飞行表「${sheet.name}」？`,
      content: ids.length === 1 ? "矿机会下载所需的挖矿软件并切换。" : `同时下发到 ${ids.length} 台矿机。`,
      okText: "应用",
      onOk: async () => {
        try {
          await submitJob({ machine_ids: ids, action: { kind: "apply", sheet_id: sheet.id }, include_controller: controller });
          message.success(ids.length === 1 ? "已下发，结果见矿机的「消息」" : `已下发到 ${ids.length} 台矿机，结果见各矿机的「消息」`);
        } catch (e) {
          message.error((e as Error).message);
        }
      },
    });
  };
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      trigger="click"
      placement="bottomRight"
      arrow={false}
      rootClassName="fs-picker"
      content={
        <div className="fs-picker-body" role="dialog" aria-label="选择飞行表">
          <b className="fs-picker-title">
            选择飞行表给 {ids.length} 已选择 矿机
          </b>
          {coins.length > 0 && (
            <div className="fs-picker-coins">
              {coins.map((c) => (
                <button key={c} type="button" className={coin === c ? "active" : ""} onClick={() => setCoin(coin === c ? undefined : c)}>
                  <CoinBadge coin={c} size={13} /> {c}
                </button>
              ))}
            </div>
          )}
          {!shown.length && <div className="muted">还没有飞行表，先在「飞行表」页创建</div>}
          {shown.map((f) => (
            <button key={f.id} type="button" className="fs-picker-sheet" onClick={() => apply(f)}>
              <b>{f.name}</b>
              {f.tasks.map((t) => (
                <span key={t.instance} className="fs-picker-task">
                  <CoinBadge coin={symbol(t.wallet_id)} size={13} /> <b>{symbol(t.wallet_id)}</b>{" "}
                  {wallets.data?.find((w) => w.id === t.wallet_id)?.name ?? ""}
                  <small>{String(data(t.miner as Data).name ?? "")}</small>
                </span>
              ))}
            </button>
          ))}
        </div>
      }
    >
      {children}
    </Popover>
  );
}
