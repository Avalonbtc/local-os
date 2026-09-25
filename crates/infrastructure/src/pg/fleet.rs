use super::*;
use serde_json::json;
use uuid::Uuid;

#[async_trait::async_trait]
impl FleetRepository for PgStore {
    async fn machines(&self) -> Result<Vec<Machine>> {
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(m)-'ssh_secret'-'bmc_secret' FROM machines m WHERE deleted_at IS NULL ORDER BY name",
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
                "SELECT to_jsonb(m)-'ssh_secret'-'bmc_secret' FROM machines m WHERE id=$1 AND deleted_at IS NULL",
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
        bmc: Option<&str>,
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
        sqlx::query("INSERT INTO machines(id,name,host,port,username,host_key,\"group\",tags,is_controller,bmc,policy,ssh_secret,bmc_secret) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,COALESCE($12,''),$13) ON CONFLICT(id) DO UPDATE SET name=excluded.name,host=excluded.host,port=excluded.port,username=excluded.username,host_key=excluded.host_key,\"group\"=excluded.\"group\",tags=excluded.tags,is_controller=excluded.is_controller,bmc=excluded.bmc,policy=excluded.policy,ssh_secret=COALESCE($12,machines.ssh_secret),bmc_secret=COALESCE($13,machines.bmc_secret)")
        .bind(id).bind(&i.name).bind(&i.host).bind(i.port as i32).bind(&i.username).bind(&i.host_key).bind(&i.group).bind(json!(i.tags)).bind(i.is_controller).bind(i.bmc.as_ref().map(|v|json!(v))).bind(&i.policy).bind(ssh).bind(bmc).execute(&mut *tx).await.map_err(db)?;
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
    async fn machine_secret(&self, id: Uuid, bmc: bool) -> Result<String> {
        let query = if bmc {
            "SELECT bmc_secret FROM machines WHERE id=$1 AND deleted_at IS NULL"
        } else {
            "SELECT ssh_secret FROM machines WHERE id=$1 AND deleted_at IS NULL"
        };
        sqlx::query_scalar::<_, Option<String>>(query)
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
