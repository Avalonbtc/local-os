import { useEffect, useRef, useState } from "react";
import { Alert, Button, Checkbox, Select, Space, Tag } from "antd";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { getCsrf, useMachines } from "../../shared/api";
import { PageHeader } from "../../shared/ui";
import { useSearchParams } from "react-router-dom";
export type TerminalSender = (value: string) => void;
export function TerminalPane({
  id,
  instance,
  onSender,
  onInput,
}: {
  id: string;
  instance?: string;
  onSender?: (sender: TerminalSender | null) => void;
  onInput?: (text: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [generation, setGeneration] = useState(0);
  const [status, setStatus] = useState("连接中");
  const [error, setError] = useState("");
  const inputRef = useRef(onInput);
  inputRef.current = onInput;
  const senderRef = useRef(onSender);
  senderRef.current = onSender;
  useEffect(() => {
    if (!ref.current) return;
    const term = new Terminal({
      theme: {
        background: "#0b0d11",
        foreground: "#d5d8de",
        cursor: "#c6dc62",
      },
      fontFamily: "Cascadia Code, Consolas, monospace",
      fontSize: 13,
      convertEol: false,
      scrollback: 3000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(ref.current);
    fit.fit();
    const params = new URLSearchParams({ csrf: getCsrf() });
    if (instance) params.set("instance", instance);
    const socket = new WebSocket(
      `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/v1/machines/${id}/terminal?${params}`,
    );
    socket.binaryType = "arraybuffer";
    const send = (text: string) => {
      if (socket.readyState === WebSocket.OPEN)
        socket.send(new TextEncoder().encode(text));
    };
    socket.onopen = () => {
      setStatus("已连接");
      setError("");
      senderRef.current?.(send);
      socket.send(
        JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }),
      );
    };
    socket.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer)
        term.write(new Uint8Array(event.data));
      else {
        try {
          setError(JSON.parse(event.data).error ?? "终端连接结束");
        } catch {
          setError(String(event.data));
        }
      }
    };
    socket.onclose = () => {
      setStatus("已断开");
      senderRef.current?.(null);
    };
    socket.onerror = () => setError("终端连接失败，请检查 SSH 和登录状态");
    const subscription = term.onData((text) => {
      if (socket.readyState === WebSocket.OPEN) {
        send(text);
        inputRef.current?.(text);
      }
    });
    const observer = new ResizeObserver(() => {
      fit.fit();
      if (socket.readyState === WebSocket.OPEN)
        socket.send(
          JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }),
        );
    });
    observer.observe(ref.current);
    return () => {
      senderRef.current?.(null);
      observer.disconnect();
      subscription.dispose();
      socket.onopen = null;
      socket.onclose = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.close();
      term.dispose();
    };
  }, [id, instance, generation]);
  return (
    <div className="terminal-pane">
      <div className="terminal-toolbar">
        <Tag color={status === "已连接" ? "success" : "default"}>{status}</Tag>
        <span className="muted">
          {instance ? `screen · ${instance}` : "SSH shell"}
        </span>
        <Button
          size="small"
          disabled={status === "已连接"}
          onClick={() => {
            setStatus("连接中");
            setGeneration((g) => g + 1);
          }}
        >
          重新连接
        </Button>
      </div>
      {error && <Alert type="error" message={error} />}
      <div className="terminal-container" ref={ref} />
    </div>
  );
}
export function TerminalsPage() {
  const machines = useMachines();
  const [params] = useSearchParams();
  const [ids, setIds] = useState<string[]>(
    params.get("ids")?.split(",").filter(Boolean) ?? [],
  );
  const [broadcast, setBroadcast] = useState(false);
  const [targets, setTargets] = useState<string[]>([]);
  const senders = useRef(new Map<string, TerminalSender>());
  return (
    <>
      <PageHeader title="多机终端" description="断线后不缓存、不补发键盘输入" />
      <Space className="terminal-controls" wrap>
        <Select
          mode="multiple"
          style={{ width: "min(400px, 100%)", minWidth: 0 }}
          placeholder="选择机器"
          value={ids}
          onChange={(values) => {
            setIds(values);
            setTargets((t) => t.filter((id) => values.includes(id)));
            setBroadcast(false);
          }}
          options={machines.data?.map((m) => ({ value: m.id, label: m.name }))}
        />
        <Checkbox
          checked={broadcast}
          onChange={(e) => setBroadcast(e.target.checked)}
        >
          开启同步输入
        </Checkbox>
      </Space>
      {broadcast && (
        <Alert
          type="warning"
          showIcon
          message="同步输入已开启"
          description="从已勾选的终端输入时，会同时发送到其他勾选且在线的终端。"
        />
      )}
      <div className="terminal-grid">
        {ids.map((id) => (
          <section key={id}>
            <div className="terminal-label">
              <b>{machines.data?.find((m) => m.id === id)?.name ?? id}</b>
              <Checkbox
                disabled={!broadcast}
                checked={targets.includes(id)}
                onChange={(e) =>
                  setTargets((t) =>
                    e.target.checked ? [...t, id] : t.filter((v) => v !== id),
                  )
                }
              >
                接收同步输入
              </Checkbox>
            </div>
            <TerminalPane
              id={id}
              onSender={(sender) => {
                if (sender) senders.current.set(id, sender);
                else senders.current.delete(id);
              }}
              onInput={(text) => {
                if (broadcast && targets.includes(id)) {
                  for (const target of targets) {
                    if (target !== id) senders.current.get(target)?.(text);
                  }
                }
              }}
            />
          </section>
        ))}
      </div>
    </>
  );
}
