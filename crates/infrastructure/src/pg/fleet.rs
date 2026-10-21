use super::*;
use serde_json::json;
use uuid::Uuid;

#[async_trait::async_trait]
impl FleetRepository for PgStore {
    async fn machines(&self) -> Result<Vec<Machine>> {
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(m)-'ssh_secret'-'fingerprint' FROM machines m WHERE deleted_at IS NULL ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(decode)
        .collect()
    }
    async fn machine(&self, id: Uuid) -> Result<Machine> {
        decode(
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(m)-'ssh_secret'-'fingerprint' FROM machines m WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?,
        )
    }
    async fn save_machine(
        &self,
        id: Uuid,
        i: &MachineInput,
        ssh: Option<&str>,
        actor: &Actor,
    ) -> Result<Machine> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let retired: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM machines WHERE id=$1 AND deleted_at IS NOT NULL)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if retired {
            return Err(Error::NotFound);
        }
        if ssh.is_none() {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM machines WHERE id=$1)")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
            if !exists {
                return Err(Error::Validation("新机器需要 SSH 凭据".into()));
            }
        }
        // An agent machine keeps the connection details its rig reported; only the descriptive
        // fields are edited.
        sqlx::query("INSERT INTO machines(id,name,host,port,username,host_key,\"group\",tags,is_controller,policy,ssh_secret) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,COALESCE($11,'')) ON CONFLICT(id) DO UPDATE SET name=excluded.name,\"group\"=excluded.\"group\",tags=excluded.tags,is_controller=excluded.is_controller,policy=excluded.policy,host=CASE WHEN machines.transport='agent' THEN machines.host ELSE excluded.host END,port=CASE WHEN machines.transport='agent' THEN machines.port ELSE excluded.port END,username=CASE WHEN machines.transport='agent' THEN machines.username ELSE excluded.username END,host_key=CASE WHEN machines.transport='agent' THEN machines.host_key ELSE excluded.host_key END,ssh_secret=CASE WHEN machines.transport='agent' THEN machines.ssh_secret ELSE COALESCE($11,machines.ssh_secret) END")
        .bind(id).bind(&i.name).bind(&i.host).bind(i.port as i32).bind(&i.username).bind(&i.host_key).bind(&i.group).bind(json!(i.tags)).bind(i.is_controller).bind(&i.policy).bind(ssh).execute(&mut *tx).await.map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "fleet.save",
            &id.to_string(),
            json!({"name":i.name,"host":i.host}),
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.machine(id).await
    }
    async fn machine_secret(&self, id: Uuid) -> Result<String> {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT ssh_secret FROM machines WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::Validation("机器缺少凭据".into()))
    }
    async fn delete_machine(&self, id: Uuid, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        super::jobs::queue_lock(&mut tx).await?;
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM job_targets WHERE machine_id=$1 AND status IN ('queued','running','reconciling','unknown')) OR EXISTS(SELECT 1 FROM machine_locks WHERE machine_id=$1)")
            .bind(id).fetch_one(&mut *tx).await.map_err(db)?;
        if busy {
            return Err(Error::Conflict(
                "机器仍有未完成或待核实任务，请先处理任务".into(),
            ));
        }
        sqlx::query("UPDATE machines SET deleted_at=now() WHERE id=$1 AND deleted_at IS NULL")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(&mut tx, actor, "fleet.delete", &id.to_string(), json!({})).await?;
        tx.commit().await.map_err(db)
    }
}
