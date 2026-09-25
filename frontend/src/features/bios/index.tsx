import { useEffect, useRef, useState } from "react";
import { Alert, App, Button, Empty, Input, Modal, Select, Space, Table, Tag } from "antd";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams, useNavigate } from "react-router-dom";
import { client, unwrap, useMachines, submitJob, type JobInput } from "../../shared/api";
import type { components } from "../../shared/schema";
import { PageHeader, QueryState } from "../../shared/ui";

type Setting = components["schemas"]["BiosSetting"];
const terminal = ["succeeded", "failed", "cancelled", "unknown", "blocked"];
export function BiosPage() {
  const { id } = useParams();
  const machines = useMachines();
  const navigate = useNavigate();
  return <>
    <PageHeader title="BIOS 管理" description="通过 BMC / Supermicro SUM 读取和修改配置" />
    <Space wrap style={{ marginBottom: 20 }}>
      <Select aria-label="选择 BIOS 机器" placeholder="选择机器" showSearch optionFilterProp="label" style={{ width: "min(320px, 100%)" }} value={id}
        onChange={value => navigate(`/bios/${value}`)} options={machines.data?.map(m => ({ value: m.id, label: `${m.name} · ${m.bmc ? m.bmc.url : "未配置 BMC"}`, disabled: !m.bmc }))} />
      {id && <Link to={`/machines/${id}`}>返回机器详情</Link>}
    </Space>
    <QueryState loading={machines.isLoading} error={machines.error}>
      {id ? <BiosEditor key={id} id={id} name={machines.data?.find(m => m.id === id)?.name ?? id} /> : <Empty description="选择一台机器以管理 BIOS" />}
    </QueryState>
  </>;
}

function BiosEditor({ id, name }: { id: string; name: string }) {
  const { message, modal } = App.useApp();
  const cache = useQueryClient();
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [search, setSearch] = useState("");
  const [group, setGroup] = useState<string>();
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [jobId, setJobId] = useState<string | null>(() => { try { return localStorage.getItem(`bios-job-${id}`); } catch { return null; } });
  const handled = useRef<string>();
  const idem = useRef(crypto.randomUUID());
  const config = useQuery({ queryKey: ["bios", id], queryFn: () => unwrap(client.GET("/api/v1/machines/{id}/bios", { params: { path: { id } } })), retry: false });
  const job = useQuery({ queryKey: ["job", jobId], enabled: !!jobId,
    queryFn: () => unwrap(client.GET("/api/v1/jobs/{id}", { params: { path: { id: jobId! } } })),
    refetchInterval: q => terminal.includes(q.state.data?.status ?? "") ? false : 2000 });
  useEffect(() => {
    if (!job.data || !terminal.includes(job.data.status) || handled.current === job.data.id) return;
    handled.current = job.data.id;
    void cache.invalidateQueries({ queryKey: ["bios", id] });
    if (job.data.status === "succeeded") setEdits({});
  }, [job.data, cache, id]);
  const running = !!jobId && !job.error && !terminal.includes(job.data?.status ?? "");
  const pending = config.data?.pending ?? {};
  const locked = busy || running || Object.keys(pending).length > 0;
  const fields = config.data?.settings ?? [];
  const changes = fields.filter(f => edits[f.id] !== undefined && edits[f.id] !== f.value);
  async function submit(action: JobInput["action"]) {
    setBusy(true);
    try {
      const result = await submitJob({ machine_ids: [id], action, concurrency: 1, canary: false, include_controller: true }, idem.current);
      idem.current = crypto.randomUUID();
      setJobId(result.job_id); try { localStorage.setItem(`bios-job-${id}`, result.job_id); } catch { /* private mode */ }
      setConfirm(false); message.success("已提交，可在本页或机器消息中查看进度");
    } catch (e) { message.error((e as Error).message); }
    finally { setBusy(false); }
  }
  return <section className="bios-panel">
    <div className="bios-toolbar"><Space wrap>
      <strong>{name}</strong>
      {config.data?.licenses.map(l => <Tag key={l} color="green">{l}</Tag>)}
      <span className="muted">{config.data?.observed_at ? `读取于 ${new Date(config.data.observed_at).toLocaleString()}` : "尚未读取"}</span>
    </Space><Space wrap>
      <Button loading={busy || running} disabled={busy || running || changes.length > 0} onClick={() => { idem.current = crypto.randomUUID(); void submit({ kind: "bios_read", discard_pending: false }); }}>读取 BIOS</Button>
      <Button disabled={!changes.length || locked} onClick={() => { setEdits({}); idem.current = crypto.randomUUID(); }}>放弃编辑</Button>
      <Button type="primary" disabled={!changes.length || locked} onClick={() => setConfirm(true)}>检查并保存（{changes.length}）</Button>
    </Space></div>
    <Alert type="info" showIcon message="保存只写入修改项，并自动备份原配置。不会自动重启；NPS、功耗等设置需重启后生效。" />
    {Object.keys(pending).length > 0 && <Alert style={{ marginTop: 12 }} type="warning" showIcon message="有待生效或待核实的修改" description={<>
      {Object.entries(pending).map(([key, value]) => <div key={key}>{fields.find(f => f.id === key)?.name ?? key} → <strong>{value}</strong></div>)}
      <p>安排重启后点击“读取 BIOS”验证。回读未匹配前，不会标记为已生效。</p>
      <p>如果已经重启、固件仍保持原值（例如生效条件不满足），可以清除标记后重新修改。</p>
      <Space wrap><Link to={`/machines/${id}?tab=sensors`}>BMC 电源操作</Link>
        <Button size="small" disabled={busy || running} onClick={() => modal.confirm({ title: "清除待生效标记？", content: "会重新读取 BIOS，并把当前回读值当作最终结果。只在机器已重启后使用。", okText: "读取并清除", onOk: () => { idem.current = crypto.randomUUID(); return submit({ kind: "bios_read", discard_pending: true }); } })}>已重启，清除标记</Button></Space>
    </>} />}
    {jobId && <Alert style={{ marginTop: 12 }} type={job.error || ["failed", "unknown"].includes(job.data?.status ?? "") ? "error" : running ? "info" : "success"}
      message={job.error?.message ?? (running ? "SUM 操作进行中…" : job.data?.status === "succeeded" ? "任务完成" : `任务状态：${job.data?.status}`)}
      description={<>{job.data?.targets.map(t => <div key={t.id}>{t.error || t.output}</div>)}<Link to={`/machines/${id}?tab=messages`}>在机器消息中查看记录</Link></>} />}
    <QueryState loading={config.isLoading} error={config.error}>
      {!fields.length ? <Empty description="点击“读取 BIOS”获取真实配置，通常需要十几秒" /> : <>
        <div className="bios-filters"><Input.Search aria-label="搜索 BIOS 设置" placeholder="搜索 NPS、NUMA、cTDP、Power…" value={search} onChange={e => setSearch(e.target.value)} />
          <Select aria-label="BIOS 菜单分组" allowClear placeholder="全部菜单" value={group} onChange={setGroup} options={[...new Set(fields.map(f => f.group))].map(g => ({ value: g, label: g }))} />
        </div>
        <Table<Setting> rowKey="id" size="small" pagination={{ pageSize: 30, showSizeChanger: true }} scroll={{ x: 760 }}
          dataSource={fields.filter(f => (!group || f.group === group) && `${f.name} ${f.group} ${f.help}`.toLowerCase().includes(search.toLowerCase()))}
          columns={[
            { title: "设置", key: "name", width: "42%", render: (_, f) => <><strong>{f.name}</strong><div className="muted bios-path">{f.group}</div>{f.help && <details><summary>说明与条件</summary><p>{f.help}</p>{f.condition && <p>生效条件：{f.condition}</p>}{f.license && <p>授权：{f.license}</p>}</details>}</> },
            { title: "当前回读值", dataIndex: "value", width: "18%" },
            { title: "修改值", key: "edit", render: (_, f) => {
              const set = (value: string) => { setEdits(e => ({ ...e, [f.id]: value })); idem.current = crypto.randomUUID(); };
              return <Space direction="vertical" style={{ width: "100%" }}>{f.kind === "Option"
                ? <Select aria-label={f.name} style={{ minWidth: 170, width: "100%" }} disabled={locked} value={edits[f.id] ?? f.value} onChange={set} options={f.options.map(v => ({ value: v, label: v }))} />
                : <Input aria-label={f.name} disabled={locked} value={edits[f.id] ?? f.value} onChange={e => set(e.target.value)} />}
                {f.kind === "Numeric" && <span className="muted">BIOS 范围：{f.minimum ?? "—"}～{f.maximum ?? "—"}</span>}
                {pending[f.id] !== undefined && <Tag color="orange">待验证：{pending[f.id]}</Tag>}
              </Space>;
            } },
          ]} />
      </>}
    </QueryState>
    <Modal title={`确认修改 ${name} 的 BIOS`} open={confirm} onCancel={() => setConfirm(false)} confirmLoading={busy} okText="保存，不重启" okButtonProps={{ disabled: locked || !changes.length }} onOk={() => config.data?.revision && void submit({ kind: "bios_write", revision: config.data.revision, changes: Object.fromEntries(changes.map(f => [f.id, edits[f.id]])) })}>
      <p>只提交以下修改，原配置会保留为备份。</p>
      {changes.map(f => <p key={f.id}><strong>{f.name}</strong><br />{f.group}<br />{f.value} → {edits[f.id]}</p>)}
    </Modal>
  </section>;
}
