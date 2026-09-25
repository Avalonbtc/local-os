use super::*;
use serde_json::json;
use uuid::Uuid;
const FLIGHT: &str = "SELECT jsonb_build_object('id',s.id,'name',s.name,'version',s.version,'tasks',COALESCE((SELECT jsonb_agg(to_jsonb(t)-'sheet_id'-'ordinal' ORDER BY ordinal) FROM flight_tasks t WHERE t.sheet_id=s.id),'[]'::jsonb)) FROM flight_sheets s";
#[async_trait::async_trait]
impl FlightRepository for PgStore {
    async fn flights(&self) -> Result<Vec<FlightSheet>> {
        sqlx::query_scalar::<_, Value>(&format!("{FLIGHT} ORDER BY s.name"))
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn flight(&self, id: Uuid) -> Result<FlightSheet> {
        decode(
            sqlx::query_scalar::<_, Value>(&format!("{FLIGHT} WHERE s.id=$1"))
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?,
        )
    }
    async fn save_flight(
        &self,
        id: Uuid,
        i: &FlightInput,
        actor: &Actor,
        propagation: Option<&FlightPropagation>,
    ) -> Result<FlightSheet> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        super::jobs::queue_lock(&mut tx).await?;
        let targets = super::jobs::flight_targets_tx(&mut tx, id).await?;
        let expected = propagation
            .map(|p| p.job.machine_ids.clone())
            .unwrap_or_default();
        if targets != expected {
            return Err(Error::Conflict("飞行表关联机器发生变化，请重新保存".into()));
        }
        let changed=sqlx::query("INSERT INTO flight_sheets(id,name) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET name=excluded.name,version=flight_sheets.version+1 WHERE flight_sheets.version=$3").bind(id).bind(&i.name).bind(i.expected_version).execute(&mut *tx).await.map_err(db)?.rows_affected();
        if changed == 0 {
            return Err(Error::Conflict("飞行表已修改，请刷新后重试".into()));
        }
        sqlx::query("DELETE FROM flight_tasks WHERE sheet_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for (index, t) in i.tasks.iter().enumerate() {
            sqlx::query("INSERT INTO flight_tasks(sheet_id,instance,ordinal,miner,wallet_id,config) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(&t.instance).bind(index as i32).bind(&t.miner).bind(t.wallet_id).bind(&t.config).execute(&mut *tx).await.map_err(db)?;
        }
        audit_tx(
            &mut tx,
            actor,
            "flight_sheets.save",
            &id.to_string(),
            json!({"name":i.name,"task_count":i.tasks.len()}),
        )
        .await?;
        let apply_job_id = if let Some(p) = propagation {
            Some(super::jobs::enqueue_tx(&mut tx, &p.job, &p.snapshot, actor).await?)
        } else {
            None
        };
        let mut sheet: FlightSheet = decode(
            sqlx::query_scalar::<_, Value>(&format!("{FLIGHT} WHERE s.id=$1"))
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?,
        )?;
        sheet.apply_job_id = apply_job_id;
        tx.commit().await.map_err(db)?;
        Ok(sheet)
    }
    async fn delete_flight(&self, id: Uuid, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("DELETE FROM flight_sheets WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "flight_sheets.delete",
            &id.to_string(),
            json!({}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
}
