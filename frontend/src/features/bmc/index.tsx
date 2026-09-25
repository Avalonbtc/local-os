import { useState } from "react";
import { Alert, Button, Select, Space, Table, Tag } from "antd";
import { PlusOutlined } from "@ant-design/icons";
import { Link } from "react-router-dom";
import {
  useMachines,
  useTelemetry,
  latest,
  isFresh,
  data,
  measuredPower,
  type Machine,
} from "../../shared/api";
import { PageHeader, PowerState, QueryState, number } from "../../shared/ui";
import { BatchModal, MachineEditor } from "../fleet";
export function BmcPage() {
  const machines = useMachines();
  const telemetry = useTelemetry();
  const [selected, setSelected] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  const [machineEditor, setMachineEditor] = useState(false);
  const [editingMachine, setEditingMachine] = useState<Machine>();
  const [targetId, setTargetId] = useState<string>();
  const unconfigured = machines.data?.filter((machine) => !machine.bmc) ?? [];
  return (
    <>
      <PageHeader
        title="BMC"
        description="BMC 绑定到矿机；配置后可读取传感器并控制电源"
        extra={
          <Space>
            <Button
              type="primary"
              icon={<PlusOutlined />}
              onClick={() => {
                setEditingMachine(undefined);
                setMachineEditor(true);
              }}
            >
              添加矿机并配置 BMC
            </Button>
            <Button disabled={!selected.length} onClick={() => setOpen(true)}>
              批量电源操作
            </Button>
          </Space>
        }
      />
      {telemetry.error && (
        <Alert type="warning" message={telemetry.error.message} />
      )}
      <QueryState loading={machines.isLoading} error={machines.error}>
        {unconfigured.length > 0 && (
          <div className="bmc-connect-card">
            <div>
              <strong>为已有矿机添加 BMC</strong>
              <p>选择矿机后填写独立的 BMC 地址和账号。</p>
            </div>
            <Space wrap>
              <Select
                showSearch
                optionFilterProp="label"
                aria-label="选择要配置 BMC 的矿机"
                placeholder="选择矿机"
                value={targetId}
                onChange={setTargetId}
                style={{ minWidth: 220 }}
                options={unconfigured.map((machine) => ({
                  value: machine.id,
                  label: `${machine.name} · ${machine.host}`,
                }))}
              />
              <Button
                disabled={!targetId}
                onClick={() => {
                  const machine = unconfigured.find((item) => item.id === targetId);
                  if (machine) {
                    setEditingMachine(machine);
                    setMachineEditor(true);
                  }
                }}
              >
                配置所选矿机
              </Button>
            </Space>
          </div>
        )}
        <Table<Machine>
          rowKey="id"
          dataSource={machines.data?.filter((m) => m.bmc)}
          locale={{ emptyText: "尚未配置 BMC，可点击上方按钮添加矿机并配置" }}
          rowSelection={{
            selectedRowKeys: selected,
            onChange: (keys) => setSelected(keys as string[]),
          }}
          columns={[
            {
              title: "机器",
              dataIndex: "name",
              render: (name, m) => (
                <Link to={`/machines/${m.id}?tab=sensors`}>{name}</Link>
              ),
            },
            {
              title: "带外地址",
              key: "address",
              render: (_, m) => <code>{m.bmc?.url}</code>,
            },
            {
              title: "协议",
              key: "provider",
              render: (_, m) => <Tag>{m.bmc?.provider}</Tag>,
            },
            {
              title: "配置",
              key: "configure",
              render: (_, m) => (
                <Button
                  size="small"
                  onClick={() => {
                    setEditingMachine(m);
                    setMachineEditor(true);
                  }}
                >
                  编辑 BMC
                </Button>
              ),
            },
            {
              title: "电源",
              key: "state",
              render: (_, m) => <PowerState observation={latest(telemetry.data, m.id, "power")} />,
            },
            {
              title: "整机功耗",
              key: "power_w",
              render: (_, m) => {
                const watts = measuredPower(
                  latest(telemetry.data, m.id, "power"),
                  latest(telemetry.data, m.id, "sensors"),
                );
                return watts === undefined ? <span className="muted">未采集</span> : `${number(watts, 0)} W`;
              },
            },
            {
              title: "传感器",
              key: "sensors",
              render: (_, m) => {
                const o = latest(telemetry.data, m.id, "sensors");
                return (
                  <Space>
                    {isFresh(o, 150)
                      ? `${data(o?.data).items?.length ?? 0} 项`
                      : "未读取"}
                    <Link to={`/machines/${m.id}?tab=sensors`}>查看</Link>
                  </Space>
                );
              },
            },
          ]}
        />
      </QueryState>
      {open && (
        <BatchModal
          ids={selected}
          kind="power"
          onClose={() => setOpen(false)}
        />
      )}
      <MachineEditor
        key={editingMachine?.id ?? "new"}
        machine={editingMachine}
        initialBmcEnabled
        open={machineEditor}
        onClose={() => {
          setMachineEditor(false);
          setTargetId(undefined);
        }}
      />
    </>
  );
}
