use super::*;
use serde_json::json;
use sqlx::Row;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
const JOB: &str = "SELECT jsonb_build_object('id',j.id,'action',j.action,'status',j.status,'concurrency',j.concurrency,'canary',j.canary,'created_at',j.created_at,'actor',j.actor,'targets',COALESCE((SELECT jsonb_agg(to_jsonb(t)-'lease'-'lease_until'-'started_at'-'finished_at' ORDER BY ordinal) FROM job_targets t WHERE t.job_id=j.id),'[]'::jsonb)) FROM jobs j";

const JOB_LIST: &str = "SELECT jsonb_build_object('id',j.id,'action',j.action-'snapshot','status',j.status,'concurrency',j.concurrency,'canary',j.canary,'created_at',j.created_at,'actor',j.actor,'targets',COALESCE((SELECT jsonb_agg(jsonb_build_object('id',t.id,'job_id',t.job_id,'machine_id',t.machine_id,'machine_name',t.machine_name,'status',t.status,'ordinal',t.ordinal,'operation_id',t.operation_id,'output','','output_truncated',t.output_truncated,'result',NULL,'error',t.error) ORDER BY t.ordinal) FROM job_targets t WHERE t.job_id=j.id),'[]'::jsonb)) FROM jobs j ORDER BY j.created_at DESC LIMIT $1";

#[async_trait::async_trait]
impl JobRepository for PgStore {
    async fn enqueue(&self, input: &JobInput, snapshot: &Value, actor: &Actor) -> Result<Uuid> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        queue_lock(&mut tx).await?;
        let id = enqueue_tx(&mut tx, input, snapshot, actor).await?;
        tx.commit().await.map_err(db)?;
        Ok(id)
    }
    async fn flight_targets(&self, sheet: Uuid) -> Result<Vec<Uuid>> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let ids = flight_targets_tx(&mut tx, sheet).await?;
        tx.commit().await.map_err(db)?;
        Ok(ids)
    }
    async fn jobs(&self, limit: i64) -> Result<Vec<Job>> {
        // The list is polled every few seconds: never read per-target output (up to 1 MiB each,
        // TOASTed) or the compiled per-host apply snapshot. Both are served by `job(id)`.
        sqlx::query_scalar::<_, Value>(JOB_LIST)
            .bind(limit.clamp(1, 250))
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn job(&self, id: Uuid) -> Result<Job> {
        decode(
            sqlx::query_scalar::<_, Value>(&format!("{JOB} WHERE j.id=$1"))
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?,
        )
    }
    async fn cancel_job(&self, id: Uuid, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        queue_lock(&mut tx).await?;
        sqlx::query("UPDATE jobs SET cancel_requested=true WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("UPDATE job_targets SET status='cancelled',finished_at=now() WHERE job_id=$1 AND status='queued'").bind(id).execute(&mut *tx).await.map_err(db)?;
        audit_tx(&mut tx, actor, "jobs.cancel", &id.to_string(), json!({})).await?;
        update_job(&mut tx, id).await?;
        tx.commit().await.map_err(db)
    }
    async fn claim(&self) -> Result<Option<ClaimedTask>> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        // Serialize only the short scheduling decision; execution never holds a transaction.
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(726947311)")
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if !locked {
            return Ok(None);
        }
        let row=sqlx::query("SELECT to_jsonb(t)-'lease'-'lease_until'-'started_at'-'finished_at' target,j.action,t.status FROM job_targets t JOIN jobs j ON j.id=t.job_id JOIN machines m ON m.id=t.machine_id WHERE m.deleted_at IS NULL AND (t.status='reconciling' OR (t.status='queued' AND NOT j.cancel_requested AND NOT EXISTS(SELECT 1 FROM machine_locks l WHERE l.machine_id=t.machine_id) AND (NOT j.canary OR t.ordinal=0 OR EXISTS(SELECT 1 FROM job_targets c WHERE c.job_id=t.job_id AND c.ordinal=0 AND c.status='succeeded')) AND (SELECT count(*) FROM job_targets c WHERE c.job_id=t.job_id AND c.status IN ('running','reconciling')) < j.concurrency AND (NOT m.is_controller OR j.action->>'kind'<>'power' OR NOT EXISTS(SELECT 1 FROM job_targets c WHERE c.job_id=t.job_id AND c.id<>t.id AND c.status NOT IN ('succeeded','failed','cancelled','blocked'))))) ORDER BY CASE WHEN t.status='reconciling' THEN 0 ELSE 1 END,j.created_at,t.ordinal FOR UPDATE OF t SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await.map_err(db)?;
        let Some(row) = row else { return Ok(None) };
        let target: JobTarget = decode(row.get("target"))?;
        let reconcile = row.get::<String, _>("status") == "reconciling";
        let lease = Uuid::new_v4();
        sqlx::query("INSERT INTO machine_locks(machine_id,target_id) VALUES($1,$2) ON CONFLICT(machine_id) DO NOTHING").bind(target.machine_id).bind(target.id).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE job_targets SET status='running',lease=$2,lease_until=now()+interval '60 seconds',started_at=COALESCE(started_at,now()) WHERE id=$1").bind(target.id).bind(lease).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE jobs SET status='running' WHERE id=$1")
            .bind(target.job_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Some(ClaimedTask {
            target,
            action: row.get("action"),
            lease,
            reconcile,
        }))
    }
    async fn heartbeat(&self, target: Uuid, lease: Uuid) -> Result<bool> {
        let job:Uuid=sqlx::query_scalar("UPDATE job_targets SET lease_until=now()+interval '60 seconds' WHERE id=$1 AND lease=$2 AND status='running' RETURNING job_id").bind(target).bind(lease).fetch_optional(&self.pool).await.map_err(db)?.ok_or_else(||Error::Conflict("任务租约已失效，必须对账".into()))?;
        sqlx::query_scalar("SELECT cancel_requested FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&self.pool)
            .await
            .map_err(db)
    }
    async fn task_progress(&self, target: Uuid, lease: Uuid, r: &RemoteOperation) -> Result<()> {
        let (output, truncated) = capped(&r.output, r.truncated);
        sqlx::query("UPDATE job_targets SET output=$3,output_truncated=$4,result=$5,error=$6 WHERE id=$1 AND lease=$2 AND (output,output_truncated,result,error) IS DISTINCT FROM ($3,$4,$5,$6)").bind(target).bind(lease).bind(output).bind(truncated).bind(&r.result).bind(&r.error).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }
    async fn finish(
        &self,
        target: Uuid,
        lease: Uuid,
        status: &str,
        r: &RemoteOperation,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        queue_lock(&mut tx).await?;
        let (output, truncated) = capped(&r.output, r.truncated);
        let job:Option<Uuid>=sqlx::query_scalar("UPDATE job_targets SET status=$3,output=$4,output_truncated=$5,result=$6,error=$7,finished_at=now(),lease_until=NULL WHERE id=$1 AND lease=$2 AND status='running' RETURNING job_id").bind(target).bind(lease).bind(status).bind(output).bind(truncated).bind(&r.result).bind(&r.error).fetch_optional(&mut *tx).await.map_err(db)?;
        if let Some(job) = job {
            if status != "unknown" {
                sqlx::query("DELETE FROM machine_locks WHERE target_id=$1")
                    .bind(target)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            }
            sqlx::query("UPDATE job_targets SET status='blocked',error='首台验证未成功，后续机器未执行',finished_at=now() WHERE job_id=$1 AND status='queued' AND EXISTS(SELECT 1 FROM jobs j JOIN job_targets t ON t.job_id=j.id WHERE j.id=$1 AND j.canary AND t.ordinal=0 AND t.status IN ('failed','unknown','cancelled'))").bind(job).execute(&mut *tx).await.map_err(db)?;
            update_job(&mut tx, job).await?;
            sqlx::query("INSERT INTO audit_events(actor,action,target,detail) VALUES('worker','jobs.result',$1,$2)").bind(target.to_string()).bind(json!({"status":status,"result":r.result,"error":r.error})).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }
    async fn expire_leases(&self) -> Result<()> {
        sqlx::query("UPDATE job_targets SET status='reconciling',lease=NULL WHERE status='running' AND lease_until<now()").execute(&self.pool).await.map_err(db)?;
        // Repair interrupted aggregate updates from older versions without redispatching targets.
        let jobs:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM jobs j WHERE status IN ('queued','running') AND NOT EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status IN ('queued','running','reconciling'))").fetch_all(&self.pool).await.map_err(db)?;
        if !jobs.is_empty() {
            let mut tx = self.pool.begin().await.map_err(db)?;
            queue_lock(&mut tx).await?;
            for id in jobs {
                update_job(&mut tx, id).await?;
            }
            tx.commit().await.map_err(db)?;
        }
        Ok(())
    }
    async fn resolve_target(&self, id: Uuid, input: &ResolveTarget, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        queue_lock(&mut tx).await?;
        let state = if input.decision == "reconcile" {
            "reconciling"
        } else {
            input.decision.as_str()
        };
        let job:Uuid=sqlx::query_scalar("UPDATE job_targets SET status=$2,lease=NULL,error=$3 WHERE id=$1 AND status='unknown' RETURNING job_id").bind(id).bind(state).bind(&input.note).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||Error::Conflict("只有结果不明的任务可对账或人工核实".into()))?;
        if state != "reconciling" {
            sqlx::query("DELETE FROM machine_locks WHERE target_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        audit_tx(
            &mut tx,
            actor,
            "jobs.resolve",
            &id.to_string(),
            json!(input),
        )
        .await?;
        update_job(&mut tx, job).await?;
        tx.commit().await.map_err(db)
    }
}
pub(super) async fn queue_lock(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(726947311)")
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}
fn capped(output: &str, truncated: bool) -> (&str, bool) {
    let mut end = output.len().min(1024 * 1024);
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    (&output[..end], truncated || end < output.len())
}
async fn update_job(tx: &mut Transaction<'_, Postgres>, job: Uuid) -> Result<()> {
    sqlx::query("UPDATE jobs j SET status=CASE WHEN EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status IN ('running','reconciling')) THEN 'running' WHEN EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status='unknown') THEN 'unknown' WHEN EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status='queued') THEN 'queued' WHEN EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status IN ('failed','blocked')) THEN 'failed' WHEN EXISTS(SELECT 1 FROM job_targets t WHERE t.job_id=j.id AND t.status='cancelled') THEN 'cancelled' ELSE 'succeeded' END WHERE j.id=$1").bind(job).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}

pub(super) async fn enqueue_tx(
    tx: &mut Transaction<'_, Postgres>,
    input: &JobInput,
    snapshot: &Value,
    actor: &Actor,
) -> Result<Uuid> {
    let hash = rig_application::digest(
        &serde_json::to_string(input).map_err(|e| Error::Internal(e.to_string()))?,
    );
    let id = Uuid::new_v4();
    let row=sqlx::query("INSERT INTO jobs(id,actor_id,actor,action,request_hash,idempotency_key,concurrency,canary) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(actor_id,idempotency_key) DO UPDATE SET idempotency_key=jobs.idempotency_key RETURNING id,request_hash")
            .bind(id).bind(actor.id).bind(&actor.name).bind(snapshot).bind(&hash).bind(&input.idempotency_key).bind(input.concurrency as i32).bind(input.canary).fetch_one(&mut **tx).await.map_err(db)?;
    let actual: Uuid = row.get("id");
    if row.get::<String, _>("request_hash") != hash {
        return Err(Error::Conflict("同一幂等键不能用于不同请求".into()));
    }
    if actual != id {
        return Ok(actual);
    }
    for (ordinal, machine) in input.machine_ids.iter().enumerate() {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM machines WHERE id=$1 AND deleted_at IS NULL)",
        )
        .bind(machine)
        .fetch_one(&mut **tx)
        .await
        .map_err(db)?;
        if !exists {
            return Err(Error::NotFound);
        }
        sqlx::query("INSERT INTO job_targets(id,job_id,machine_id,machine_name,ordinal,operation_id) SELECT $1,$2,id,name,$4,$5 FROM machines WHERE id=$3").bind(Uuid::new_v4()).bind(id).bind(machine).bind(ordinal as i32).bind(Uuid::new_v4()).execute(&mut **tx).await.map_err(db)?;
        if snapshot["kind"] == "apply" {
            sqlx::query("INSERT INTO deployment_versions(id,job_id,machine_id,snapshot) VALUES($1,$2,$3,$4)").bind(Uuid::new_v4()).bind(id).bind(machine).bind(&snapshot["snapshot"]["hosts"][machine.to_string()]).execute(&mut **tx).await.map_err(db)?;
        }
    }
    audit_tx(
        tx,
        actor,
        "jobs.submit",
        &id.to_string(),
        json!({"kind":snapshot["kind"],"targets":input.machine_ids}),
    )
    .await?;
    Ok(id)
}

pub(super) async fn flight_targets_tx(
    tx: &mut Transaction<'_, Postgres>,
    sheet: Uuid,
) -> Result<Vec<Uuid>> {
    sqlx::query_scalar("SELECT machine_id FROM (SELECT DISTINCT ON (d.machine_id) d.machine_id,d.snapshot FROM deployment_versions d JOIN job_targets t ON t.job_id=d.job_id AND t.machine_id=d.machine_id WHERE EXISTS(SELECT 1 FROM machines m WHERE m.id=d.machine_id AND m.deleted_at IS NULL) AND t.status NOT IN ('failed','cancelled','skipped','blocked') ORDER BY d.machine_id,d.created_at DESC) current WHERE snapshot->>'sheet_id'=$1 ORDER BY machine_id")
        .bind(sheet.to_string()).fetch_all(&mut **tx).await.map_err(db)
}
