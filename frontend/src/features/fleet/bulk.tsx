// HiveOS bulk bar: with workers ticked, the top bar shows one icon per action that applies to
// all of them. Only actions RigDeck can carry out are offered.
import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { App, Dropdown, Modal, Select, Space, Tooltip, Typography } from "antd";
import type { MenuProps } from "antd";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { client, submitJob, unwrap, useCatalog, useMachines, type CatalogItem, type Machine } from "../../shared/api";
import {
  IconCommand,
  IconPickaxe,
  IconPower,
  IconRocket,
  IconTag,
  IconTerminal,
  IconTrash,
  IconTune,
  IconUpload,
} from "../../shared/icons";
import { setSelection } from "../../shared/selection";
import { summary, type OcConfig } from "../overclock";
import { REBOOT, SHUTDOWN, updateMachine } from "./worker";
import { CommandDialog, FlightSheetPicker } from "./worker-dialogs";

const BMC_ACTIONS: Record<string, string> = {
  on: "开机",
  shutdown: "优雅关机",
  force_restart: "断电重启",
  force_off: "强制断电",
};

/** Renders into the top bar on desktop (like HiveOS) and inline above the list on phones. */
function TopSlot({ children }: { children: ReactNode }) {
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const [wide, setWide] = useState(() => window.matchMedia("(min-width: 861px)").matches);
  useEffect(() => {
    setSlot(document.getElementById("rd-top-tools"));
    const query = window.matchMedia("(min-width: 861px)");
    const change = () => setWide(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  return slot && wide ? createPortal(children, slot) : <div className="hive-batch-bar">{children}</div>;
}

export function BulkActions({ ids, onBatch }: { ids: string[]; onBatch: (kind: string) => void }) {
  const { message, modal } = App.useApp();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const machines = useMachines();
  const [tags, setTags] = useState(false);
  const [overclock, setOverclock] = useState(false);
  const [command, setCommand] = useState(false);
  const chosen = (machines.data ?? []).filter((m) => ids.includes(m.id));
  const withBmc = chosen.filter((m) => m.bmc);
  const hosts = chosen.filter((m) => !m.is_controller);
  const job = (title: string, content: string, targets: string[], action: Parameters<typeof submitJob>[0]["action"], danger = false, controller = false) =>
    modal.confirm({
      title,
      content,
      okButtonProps: { danger },
      onOk: async () => {
        try {
          await submitJob({ machine_ids: targets, action, include_controller: controller });
          message.success(`已下发到 ${targets.length} 台矿机，结果见各矿机的「消息」`);
        } catch (e) {
          message.error((e as Error).message);
        }
      },
    });
  const skipped = chosen.length - hosts.length ? `（主控机器不在内，需在它自己的页面操作）` : "";
  const tool = (title: string, icon: ReactNode, onClick?: () => void, disabled = false, danger = false) => (
    <Tooltip title={title}>
      <button type="button" className={`wk-tool ${danger ? "danger" : ""}`} aria-label={title} disabled={disabled} onClick={onClick}>
        {icon}
      </button>
    </Tooltip>
  );
  const menuTool = (title: string, icon: ReactNode, items: MenuProps["items"], onClick: MenuProps["onClick"]) => (
    <Dropdown trigger={["click"]} menu={{ items, onClick }}>
      <Tooltip title={title}>
        <button type="button" className="wk-tool" aria-label={title}>
          {icon}
        </button>
      </Tooltip>
    </Dropdown>
  );
  return (
    <TopSlot>
      <div className="wk-tools" role="toolbar" aria-label={`批量操作 ${ids.length} 台矿机`}>
        {menuTool(
          "电源动作",
          <IconPower />,
          [
            { key: "reboot", label: "重启", disabled: !hosts.length },
            { key: "shutdown", label: "关机", danger: true, disabled: !hosts.length },
            { type: "divider" },
            {
              type: "group",
              label: `BMC（${withBmc.length}/${chosen.length} 台已配置）`,
              children: Object.entries(BMC_ACTIONS).map(([key, label]) => ({
                key: `bmc:${key}`,
                label,
                danger: key.startsWith("force"),
                disabled: !withBmc.length,
              })),
            },
          ],
          ({ key }) => {
            const ids = hosts.map((m) => m.id);
            if (key === "reboot")
              job(`重启 ${ids.length} 台矿机？`, `矿工会在开机后按期望状态自动恢复。${skipped}`, ids, { kind: "command", script: REBOOT, timeout_seconds: 60 });
            else if (key === "shutdown")
              job(`关机 ${ids.length} 台矿机？`, `关机后只能通过 BMC 或现场开机。${skipped}`, ids, { kind: "command", script: SHUTDOWN, timeout_seconds: 60 }, true);
            else {
              const operation = key.slice(4);
              job(
                `通过 BMC 对 ${withBmc.length} 台执行「${BMC_ACTIONS[operation]}」？`,
                withBmc.length < chosen.length ? `未配置 BMC 的 ${chosen.length - withBmc.length} 台不受影响。` : "",
                withBmc.map((m) => m.id),
                { kind: "power", operation },
                operation.startsWith("force"),
                withBmc.some((m) => m.is_controller),
              );
            }
          },
        )}
        {tool("部署 / 更新运行层", <IconUpload />, () => onBatch("bootstrap"))}
        <FlightSheetPicker ids={ids}>{tool("选择飞行表", <IconRocket />)}</FlightSheetPicker>
        {tool("超频模版", <IconTune />, () => setOverclock(true))}
        {tool("运行命令", <IconCommand />, () => setCommand(true))}
        {tool("编辑标签", <IconTag />, () => setTags(true))}
        {tool("多机终端", <IconTerminal />, () => navigate(`/terminals?ids=${ids.join(",")}`))}
        {tool("挖矿软件动作", <IconPickaxe />, () => onBatch("miner"))}
        {tool(
          "移除",
          <IconTrash />,
          () =>
            modal.confirm({
              title: `从面板移除 ${chosen.length} 台矿机？`,
              content: "只删除面板中的记录和保存的凭据，不会停止矿机上正在运行的矿工。",
              okText: "移除",
              okButtonProps: { danger: true },
              onOk: async () => {
                const failed: string[] = [];
                for (const machine of chosen) {
                  try {
                    await unwrap(client.DELETE("/api/v1/machines/{id}", { params: { path: { id: machine.id } } }));
                  } catch (e) {
                    failed.push(`${machine.name}：${(e as Error).message}`);
                  }
                }
                await queryClient.invalidateQueries({ queryKey: ["machines"] });
                setSelection([]);
                if (failed.length) message.error(failed.join("；"));
                else message.success(`已移除 ${chosen.length} 台`);
              },
            }),
          false,
          true,
        )}
      </div>
      <BulkTags machines={chosen} open={tags} onClose={() => setTags(false)} />
      <CommandDialog ids={ids} open={command} onClose={() => setCommand(false)} />
      {overclock && <BulkOverclock ids={ids} onClose={() => setOverclock(false)} />}
    </TopSlot>
  );
}

/** Add or remove tags on every ticked worker (HiveOS "#"). */
function BulkTags({ machines, open, onClose }: { machines: Machine[]; open: boolean; onClose: () => void }) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const all = useMachines();
  const [add, setAdd] = useState<string[]>([]);
  const [remove, setRemove] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const known = [...new Set((all.data ?? []).flatMap((m) => m.tags))].sort();
  const present = [...new Set(machines.flatMap((m) => m.tags))].sort();
  return (
    <Modal
      open={open}
      title={`编辑标签 · ${machines.length} 台矿机`}
      okText="保存"
      confirmLoading={busy}
      okButtonProps={{ disabled: !add.length && !remove.length }}
      onCancel={onClose}
      afterOpenChange={(visible) => {
        if (!visible) {
          setAdd([]);
          setRemove([]);
        }
      }}
      onOk={async () => {
        setBusy(true);
        const failed: string[] = [];
        for (const machine of machines) {
          const next = [...new Set([...machine.tags.filter((t) => !remove.includes(t)), ...add.map((t) => t.trim()).filter(Boolean)])];
          if (next.join("\n") === machine.tags.join("\n")) continue;
          try {
            await updateMachine(machine, { tags: next });
          } catch (e) {
            failed.push(`${machine.name}：${(e as Error).message}`);
          }
        }
        await queryClient.invalidateQueries({ queryKey: ["machines"] });
        setBusy(false);
        if (failed.length) message.error(failed.join("；"));
        else {
          message.success("标签已更新");
          onClose();
        }
      }}
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        <Typography.Text>添加标签</Typography.Text>
        <Select
          mode="tags"
          aria-label="添加标签"
          style={{ width: "100%" }}
          placeholder="选择或开始输入"
          value={add}
          onChange={setAdd}
          options={known.map((t) => ({ value: t, label: t }))}
        />
        <Typography.Text>移除标签</Typography.Text>
        <Select
          mode="multiple"
          aria-label="移除标签"
          style={{ width: "100%" }}
          placeholder={present.length ? "选择要移除的标签" : "所选矿机没有标签"}
          disabled={!present.length}
          value={remove}
          onChange={setRemove}
          options={present.map((t) => ({ value: t, label: t }))}
        />
      </Space>
    </Modal>
  );
}

/** Apply a saved overclocking template to every ticked worker. */
function BulkOverclock({ ids, onClose }: { ids: string[]; onClose: () => void }) {
  const { message } = App.useApp();
  const templates = useCatalog("oc-profiles");
  const [chosen, setChosen] = useState<string>();
  const template = templates.data?.find((t: CatalogItem) => t.id === chosen);
  return (
    <Modal
      open
      title={`超频模版 · ${ids.length} 台矿机`}
      okText="应用"
      okButtonProps={{ disabled: !template }}
      onCancel={onClose}
      onOk={async () => {
        try {
          await submitJob({
            machine_ids: ids,
            action: { kind: "gpu_oc", profile_name: template!.name, config: template!.data as OcConfig },
            include_controller: true,
          });
          message.success(`已下发到 ${ids.length} 台矿机，结果见各矿机的「消息」`);
          onClose();
        } catch (e) {
          message.error((e as Error).message);
        }
      }}
    >
      <Space direction="vertical" style={{ width: "100%" }}>
        <Select
          aria-label="超频模版"
          style={{ width: "100%" }}
          placeholder={templates.data?.length ? "选择模版" : "还没有超频模版，先在「超频」页创建"}
          loading={templates.isLoading}
          value={chosen}
          onChange={setChosen}
          options={(templates.data ?? []).map((t: CatalogItem) => ({ value: t.id, label: t.name }))}
        />
        {template && <Typography.Text type="secondary">{summary(template.data as OcConfig)}</Typography.Text>}
      </Space>
    </Modal>
  );
}
