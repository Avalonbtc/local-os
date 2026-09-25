use super::*;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// Not kept at all (runtime 0.2.0 no longer sends them; older runtimes still do).
const DROPPED: [&str; 4] = ["disks", "network", "interfaces", "network_interfaces"];
const INVENTORY: [&str; 11] = [
    "topology",
    "cpu_model",
    "hostname",
    "os",
    "kernel",
    "architecture",
    "board",
    "bios",
    "hardware",
    "dmi",
    "numa",
];

#[derive(Default)]
struct Columns {
    ids: Vec<Uuid>,
    kinds: Vec<String>,
    times: Vec<DateTime<Utc>>,
    data: Vec<Value>,
    errors: Vec<Option<String>>,
}
impl Columns {
    fn push(&mut self, o: &Observation, data: Value) {
        self.ids.push(o.machine_id);
        self.kinds.push(o.kind.clone());
        self.times.push(o.observed_at);
        self.data.push(data);
        self.errors.push(o.error.clone());
    }
}
struct TelemetryBatch {
    hardware: Vec<(Uuid, DateTime<Utc>, Value)>,
    latest: Columns,
    metrics: Columns,
}
impl TelemetryBatch {
    /// One upsert may not touch a row twice: keep the newest sample per machine/kind for
    /// `latest_observations`, while every sample still lands in the metrics history.
    fn new(observations: &[Observation]) -> Self {
        let mut hardware: HashMap<Uuid, (DateTime<Utc>, serde_json::Map<String, Value>)> =
            HashMap::new();
        let mut newest: HashMap<(Uuid, &str), (usize, Value)> = HashMap::new();
        let mut metrics = Columns::default();
        for (index, o) in observations.iter().enumerate() {
            let mut data = o.data.clone();
            // Accept older runtimes while they are rolled out independently.
            if o.kind == "system"
                && let Some(object) = data.as_object_mut()
            {
                for key in DROPPED {
                    object.remove(key);
                }
                for key in INVENTORY {
                    if let Some(v) = object.remove(key) {
                        let entry = hardware
                            .entry(o.machine_id)
                            .or_insert_with(|| (o.observed_at, serde_json::Map::new()));
                        entry.0 = entry.0.max(o.observed_at);
                        entry.1.insert(key.into(), v);
                    }
                }
            }
            if ["system", "mining", "power", "sensors"].contains(&o.kind.as_str()) {
                metrics.push(o, data.clone());
            }
            match newest.get(&(o.machine_id, o.kind.as_str())) {
                Some((current, _)) if observations[*current].observed_at > o.observed_at => {}
                _ => {
                    newest.insert((o.machine_id, o.kind.as_str()), (index, data));
                }
            }
        }
        let mut latest = Columns::default();
        for (index, data) in newest.into_values() {
            latest.push(&observations[index], data);
        }
        Self {
            hardware: hardware
                .into_iter()
                .map(|(id, (at, data))| (id, at, Value::Object(data)))
                .collect(),
            latest,
            metrics,
        }
    }
}
fn unzip3<A, B, C>(items: impl Iterator<Item = (A, B, C)>) -> (Vec<A>, Vec<B>, Vec<C>) {
    let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
    for (x, y, z) in items {
        a.push(x);
        b.push(y);
        c.push(z);
    }
    (a, b, c)
}

const MESSAGES: &str = "SELECT row FROM (\
 (SELECT jsonb_build_object('id',e.id,'at',e.at,'level',e.level,'source','runtime','kind',e.kind,'instance',e.instance,'message',e.message,'detail',e.detail) AS row, e.at FROM machine_events e WHERE e.machine_id=$1 ORDER BY e.at DESC LIMIT $2)\
 UNION ALL\
 (SELECT jsonb_build_object('id',t.id,'at',COALESCE(t.finished_at,t.started_at,j.created_at),'status',t.status,'source','job','kind',j.action->>'kind','action',j.action-'snapshot','actor',j.actor,'job_id',j.id,'target_id',t.id,'error',t.error,'output',left(t.output,4096),'output_truncated',t.output_truncated OR length(t.output)>4096) AS row, COALESCE(t.finished_at,t.started_at,j.created_at) AS at FROM job_targets t JOIN jobs j ON j.id=t.job_id WHERE t.machine_id=$1 ORDER BY 2 DESC LIMIT $2)\
 UNION ALL\
 (SELECT jsonb_build_object('id',a.id::text,'at',a.created_at,'source','audit','kind',a.action,'actor',a.actor,'detail',a.detail) AS row, a.created_at AS at FROM audit_events a WHERE a.target=$1::text AND a.action NOT IN ('terminal.open','fleet.connection_test') ORDER BY a.created_at DESC LIMIT $2)\
) m ORDER BY at DESC LIMIT $2";

fn job_label(row: &Value) -> String {
    let action = &row["action"];
    match row["kind"].as_str().unwrap_or("") {
        "apply" => "应用飞行表".into(),
        "bootstrap" => "部署运行层".into(),
        "command" => "执行命令".into(),
        "adopt" => "纳管已有进程".into(),
        "miner" => format!(
            "矿工{} {}",
            match action["operation"].as_str().unwrap_or("") {
                "start" => "启动",
                "stop" => "停止",
                "restart" => "重启",
                other => other,
            },
            action["instances"]
                .as_array()
                .map(|v| v
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ),
        "bios_read" if action["discard_pending"] == true => "读取 BIOS 并清除待生效标记".into(),
        "bios_read" => "读取 BIOS".into(),
        "bios_write" => format!(
            "修改 BIOS（{} 项）",
            action["changes"].as_object().map_or(0, |c| c.len())
        ),
        "power" => format!(
            "电源操作：{}",
            match action["operation"].as_str().unwrap_or("") {
                "on" => "开机",
                "shutdown" => "关机",
                "reboot" => "重启",
                "force_off" => "强制断电",
                "force_restart" => "强制重启",
                other => other,
            }
        ),
        other => other.into(),
    }
}
fn message_from_row(row: Value) -> Result<MachineMessage> {
    let source = row["source"].as_str().unwrap_or("runtime").to_string();
    let at =
        serde_json::from_value(row["at"].clone()).map_err(|e| Error::Internal(e.to_string()))?;
    let id = row["id"].as_str().unwrap_or_default().to_string();
    Ok(match source.as_str() {
        "job" => {
            let status = row["status"].as_str().unwrap_or("");
            let (level, text) = match status {
                "succeeded" => ("success", "成功"),
                "failed" => ("error", "失败"),
                "unknown" => ("warning", "结果不明，需要核实"),
                "blocked" => ("warning", "首台验证未通过，未执行"),
                "cancelled" => ("warning", "已取消"),
                "running" | "reconciling" => ("info", "执行中"),
                _ => ("info", "排队中"),
            };
            let mut message = format!("{}：{}", job_label(&row), text);
            if let Some(error) = row["error"].as_str().filter(|e| !e.is_empty()) {
                message.push_str(&format!(
                    "（{}）",
                    error.chars().take(300).collect::<String>()
                ));
            }
            MachineMessage {
                id,
                at,
                level: level.into(),
                source,
                kind: row["kind"].as_str().unwrap_or("job").into(),
                instance: None,
                message,
                detail: Some(
                    // Like HiveOS, a command's result lives with the rig's messages (no jobs page).
                    serde_json::json!({"job_id":row["job_id"],"target_id":row["target_id"],"status":status,"actor":row["actor"],"output":row["output"],"output_truncated":row["output_truncated"]}),
                ),
            }
        }
        "audit" => {
            let kind = row["kind"].as_str().unwrap_or("").to_string();
            let label = match kind.as_str() {
                "fleet.save" => "机器配置已保存",
                "fleet.delete" => "机器已删除",
                "fleet.policy_push" => "恢复策略已下发",
                other => other,
            };
            MachineMessage {
                id,
                at,
                level: "info".into(),
                source,
                message: format!("{label}（{}）", row["actor"].as_str().unwrap_or("")),
                kind,
                instance: None,
                detail: row.get("detail").cloned(),
            }
        }
        _ => MachineMessage {
            id,
            at,
            level: row["level"].as_str().unwrap_or("info").into(),
            source,
            kind: row["kind"].as_str().unwrap_or("event").into(),
            instance: row["instance"].as_str().map(str::to_owned),
            message: row["message"].as_str().unwrap_or("").into(),
            detail: row.get("detail").filter(|v| !v.is_null()).cloned(),
        },
    })
}
#[async_trait::async_trait]
impl TelemetryRepository for PgStore {
    async fn observe(&self, o: &Observation) -> Result<()> {
        self.observe_many(std::slice::from_ref(o)).await
    }
    async fn observe_many(&self, observations: &[Observation]) -> Result<()> {
        if observations.is_empty() {
            return Ok(());
        }
        let batch = TelemetryBatch::new(observations);
        let mut tx = self.pool.begin().await.map_err(db)?;
        if !batch.hardware.is_empty() {
            let (ids, times, data): (Vec<Uuid>, Vec<DateTime<Utc>>, Vec<Value>) =
                unzip3(batch.hardware.into_iter());
            sqlx::query("INSERT INTO latest_observations(machine_id,kind,observed_at,data,error) SELECT m,'hardware',t,d,NULL FROM UNNEST($1::uuid[],$2::timestamptz[],$3::jsonb[]) AS u(m,t,d) ON CONFLICT(machine_id,kind) DO UPDATE SET observed_at=excluded.observed_at,data=latest_observations.data || excluded.data,error=NULL")
                .bind(ids).bind(times).bind(data).execute(&mut *tx).await.map_err(db)?;
        }
        let latest = batch.latest;
        sqlx::query("INSERT INTO latest_observations(machine_id,kind,observed_at,data,error) SELECT * FROM UNNEST($1::uuid[],$2::text[],$3::timestamptz[],$4::jsonb[],$5::text[]) ON CONFLICT(machine_id,kind) DO UPDATE SET observed_at=excluded.observed_at,data=excluded.data,error=excluded.error WHERE latest_observations.observed_at<=excluded.observed_at")
            .bind(latest.ids).bind(latest.kinds).bind(latest.times).bind(latest.data).bind(latest.errors)
            .execute(&mut *tx).await.map_err(db)?;
        let metrics = batch.metrics;
        if !metrics.ids.is_empty() {
            sqlx::query("INSERT INTO metrics(machine_id,kind,observed_at,data,error) SELECT m,k,t,metric_values(k,d),e FROM UNNEST($1::uuid[],$2::text[],$3::timestamptz[],$4::jsonb[],$5::text[]) AS u(m,k,t,d,e)")
                .bind(metrics.ids).bind(metrics.kinds).bind(metrics.times).bind(metrics.data).bind(metrics.errors)
                .execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }
    async fn record_events(&self, machine: Uuid, events: &[Value]) -> Result<u64> {
        let mut ids = Vec::new();
        let mut seqs = Vec::new();
        let mut times = Vec::new();
        let mut levels = Vec::new();
        let mut kinds = Vec::new();
        let mut instances: Vec<Option<String>> = Vec::new();
        let mut messages = Vec::new();
        let mut details: Vec<Option<Value>> = Vec::new();
        for event in events {
            let (Some(id), Some(seq), Some(at)) = (
                event["id"].as_str().and_then(|v| Uuid::parse_str(v).ok()),
                event["seq"].as_i64(),
                event["at"]
                    .as_f64()
                    .and_then(|v| DateTime::<Utc>::from_timestamp_millis((v * 1000.0) as i64)),
            ) else {
                continue;
            };
            let level = event["level"].as_str().unwrap_or("info");
            ids.push(id);
            seqs.push(seq);
            times.push(at);
            levels.push(
                if ["info", "success", "warning", "error"].contains(&level) {
                    level.to_string()
                } else {
                    "info".into()
                },
            );
            kinds.push(
                event["kind"]
                    .as_str()
                    .unwrap_or("event")
                    .chars()
                    .take(64)
                    .collect::<String>(),
            );
            instances.push(
                event["instance"]
                    .as_str()
                    .map(|v| v.chars().take(64).collect()),
            );
            messages.push(
                event["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(2000)
                    .collect::<String>(),
            );
            details.push(event.get("detail").filter(|v| !v.is_null()).cloned());
        }
        if ids.is_empty() {
            return Ok(0);
        }
        Ok(sqlx::query("INSERT INTO machine_events(id,machine_id,seq,at,level,kind,instance,message,detail) SELECT i,$1,s,a,l,k,n,m,d FROM UNNEST($2::uuid[],$3::bigint[],$4::timestamptz[],$5::text[],$6::text[],$7::text[],$8::text[],$9::jsonb[]) AS u(i,s,a,l,k,n,m,d) ON CONFLICT(id) DO NOTHING")
            .bind(machine).bind(ids).bind(seqs).bind(times).bind(levels).bind(kinds).bind(instances).bind(messages).bind(details)
            .execute(&self.pool).await.map_err(db)?.rows_affected())
    }
    async fn messages(&self, machine: Uuid, limit: i64) -> Result<Vec<MachineMessage>> {
        let limit = limit.clamp(1, 500);
        let rows = sqlx::query_scalar::<_, Value>(MESSAGES)
            .bind(machine)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.into_iter().map(message_from_row).collect()
    }
    async fn unread_messages(&self, machine: Uuid, limit: i64) -> Result<Vec<MachineMessage>> {
        let limit = limit.clamp(1, 50);
        let cleared: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT cleared_at FROM message_clears WHERE machine_id=$1")
                .bind(machine)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        let dismissed: Vec<String> =
            sqlx::query_scalar("SELECT message_key FROM message_dismissals WHERE machine_id=$1")
                .bind(machine)
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        // Chips are only the most recent messages; look a little further back to skip closed ones.
        let recent = self.messages(machine, limit + 50).await?;
        Ok(recent
            .into_iter()
            .filter(|m| cleared.is_none_or(|at| m.at > at))
            .filter(|m| !dismissed.contains(&format!("{}:{}", m.source, m.id)))
            .take(limit as usize)
            .collect())
    }
    async fn dismiss_messages(&self, machine: Uuid, key: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        match key {
            Some(key) => {
                sqlx::query("INSERT INTO message_dismissals(machine_id,message_key) VALUES($1,$2) ON CONFLICT DO NOTHING")
                    .bind(machine)
                    .bind(key)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                // Closed chips older than the newest messages can never show again: keep the table small.
                sqlx::query("DELETE FROM message_dismissals WHERE machine_id=$1 AND dismissed_at<now()-interval '30 days'")
                    .bind(machine)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            }
            None => {
                sqlx::query("INSERT INTO message_clears(machine_id,cleared_at) VALUES($1,now()) ON CONFLICT(machine_id) DO UPDATE SET cleared_at=excluded.cleared_at")
                    .bind(machine)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                sqlx::query("DELETE FROM message_dismissals WHERE machine_id=$1")
                    .bind(machine)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            }
        }
        tx.commit().await.map_err(db)
    }
    async fn latest(&self, machine: Option<Uuid>) -> Result<Vec<Observation>> {
        sqlx::query_scalar::<_,Value>("SELECT to_jsonb(o) || jsonb_build_object('data',CASE WHEN o.kind='system' AND jsonb_typeof(o.data)='object' THEN COALESCE((SELECT h.data FROM latest_observations h WHERE h.machine_id=o.machine_id AND h.kind='hardware'),'{}'::jsonb) || o.data ELSE o.data END) FROM latest_observations o JOIN machines m ON m.id=o.machine_id WHERE m.deleted_at IS NULL AND ($1::uuid IS NULL OR machine_id=$1) ORDER BY machine_id,kind").bind(machine).fetch_all(&self.pool).await.map_err(db)?.into_iter().map(decode).collect()
    }
    async fn latest_summary(&self, machine: Option<Uuid>) -> Result<Vec<Observation>> {
        sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('machine_id',o.machine_id,'kind',o.kind,'observed_at',o.observed_at,'error',o.error,'data',metric_values(o.kind,o.data)) FROM latest_observations o JOIN machines m ON m.id=o.machine_id WHERE m.deleted_at IS NULL AND ($1::uuid IS NULL OR o.machine_id=$1) AND o.kind IN ('system','mining','power','sensors') ORDER BY o.machine_id,o.kind")
            .bind(machine).fetch_all(&self.pool).await.map_err(db)?.into_iter().map(decode).collect()
    }
    async fn history(&self, machine: Uuid, kind: &str, hours: u32) -> Result<Vec<Observation>> {
        let table = if hours <= 24 {
            "metrics"
        } else {
            "metric_minutes"
        };
        let query = format!(
            "SELECT jsonb_build_object('machine_id',o.machine_id,'kind',o.kind,'observed_at',o.observed_at,'error',o.error,'data',metric_values(o.kind,o.data)) FROM (SELECT * FROM {table} WHERE machine_id=$1 AND kind=$2 AND observed_at>now()-make_interval(hours=>$3) ORDER BY observed_at DESC LIMIT 10000) o ORDER BY observed_at"
        );
        sqlx::query_scalar::<_, Value>(&query)
            .bind(machine)
            .bind(kind)
            .bind(hours.min(2160) as i32)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn maintain(&self) -> Result<()> {
        // Partition creation commits FIRST and cannot be rolled back by aggregation.
        sqlx::query("SELECT create_metric_partitions()")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        for _ in 0..192 {
            let mut tx = self.pool.begin().await.map_err(db)?;
            let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(726947312)")
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
            if !locked {
                break;
            }
            if locked {
                sqlx::query("SET LOCAL statement_timeout='120s'")
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
                sqlx::query("INSERT INTO telemetry_watermarks VALUES('minutes',now()-interval '7 days') ON CONFLICT DO NOTHING").execute(&mut *tx).await.map_err(db)?;
                // One hour per transaction, retry from the committed watermark.
                // A two-minute overlap admits observations committed just after a prior pass.
                sqlx::query("INSERT INTO metric_minutes(machine_id,kind,observed_at,data,error) SELECT DISTINCT ON (machine_id,kind,date_trunc('minute',observed_at)) machine_id,kind,date_trunc('minute',observed_at),CASE WHEN jsonb_typeof(data)='object' THEN metric_values(kind,data) || jsonb_build_object('_sample_observed_at',observed_at) ELSE data END,error FROM metrics WHERE observed_at >= (SELECT through_at-interval '2 minutes' FROM telemetry_watermarks WHERE name='minutes') AND observed_at < LEAST(date_trunc('minute',now())-interval '1 minute',(SELECT through_at+interval '1 hour' FROM telemetry_watermarks WHERE name='minutes')) ORDER BY machine_id,kind,date_trunc('minute',observed_at),observed_at DESC ON CONFLICT(machine_id,kind,observed_at) DO UPDATE SET data=excluded.data,error=excluded.error")
                    .execute(&mut *tx).await.map_err(db)?;
                sqlx::query("UPDATE telemetry_watermarks SET through_at=LEAST(date_trunc('minute',now())-interval '1 minute',through_at+interval '1 hour') WHERE name='minutes'").execute(&mut *tx).await.map_err(db)?;
            }
            let caught_up: bool = sqlx::query_scalar("SELECT through_at >= date_trunc('minute',now())-interval '1 minute' FROM telemetry_watermarks WHERE name='minutes'").fetch_one(&mut *tx).await.map_err(db)?;
            tx.commit().await.map_err(db)?;
            if caught_up {
                break;
            }
        }
        // DDL locks live only for this short transaction, never across aggregation.
        let old: Vec<String> = sqlx::query_scalar("SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid=i.inhrelid WHERE i.inhparent='metrics'::regclass AND c.relname ~ '^metrics_[0-9]{8}$' AND right(c.relname,8)<to_char(current_date-7,'YYYYMMDD')").fetch_all(&self.pool).await.map_err(db)?;
        for table in old {
            if !table
                .strip_prefix("metrics_")
                .is_some_and(|s| s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let mut tx = self.pool.begin().await.map_err(db)?;
            sqlx::query("SET LOCAL lock_timeout='2s'")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
        }
        for query in [
            "DELETE FROM metric_minutes WHERE observed_at<now()-interval '90 days'",
            "UPDATE job_targets SET output='',output_truncated=true WHERE finished_at<now()-interval '30 days' AND output<>''",
            "DELETE FROM audit_events WHERE created_at<now()-interval '180 days'",
            "DELETE FROM sessions WHERE expires_at<now()",
            "DELETE FROM login_limits WHERE window_start<now()-interval '1 day'",
            "DELETE FROM machine_events WHERE at<now()-interval '90 days'",
        ] {
            sqlx::query(query).execute(&self.pool).await.map_err(db)?;
        }
        Ok(())
    }
}
