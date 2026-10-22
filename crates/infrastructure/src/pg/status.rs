use super::*;
use uuid::Uuid;

#[async_trait::async_trait]
impl StatusRepository for PgStore {
    async fn machine_statuses(&self) -> Result<Vec<MachineStatus>> {
        sqlx::query_scalar::<_, Value>("SELECT s.data FROM machine_status s JOIN machines m ON m.id=s.machine_id WHERE m.deleted_at IS NULL ORDER BY s.machine_id")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn save_statuses(&self, statuses: &[MachineStatus], alerts: &[NewAlert]) -> Result<()> {
        if statuses.is_empty() && alerts.is_empty() {
            return Ok(());
        }
        let ids: Vec<Uuid> = statuses.iter().map(|s| s.machine_id).collect();
        let data = statuses.iter().map(encode).collect::<Result<Vec<_>>>()?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        // A machine deleted between evaluation and save is skipped, not a foreign-key error.
        sqlx::query("INSERT INTO machine_status(machine_id,data) SELECT u.m,u.d FROM UNNEST($1::uuid[],$2::jsonb[]) AS u(m,d) JOIN machines ON machines.id=u.m ON CONFLICT(machine_id) DO UPDATE SET data=excluded.data,updated_at=now()")
            .bind(&ids)
            .bind(&data)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("INSERT INTO status_alerts(machine_id,kind,event,at,message) SELECT u.m,u.k,u.e,u.t,u.msg FROM UNNEST($1::uuid[],$2::text[],$3::text[],$4::timestamptz[],$5::text[]) WITH ORDINALITY AS u(m,k,e,t,msg,n) JOIN machines ON machines.id=u.m ORDER BY u.n")
            .bind(alerts.iter().map(|a| a.machine_id).collect::<Vec<_>>())
            .bind(alerts.iter().map(|a| text(&a.kind)).collect::<Result<Vec<_>>>()?)
            .bind(alerts.iter().map(|a| text(&a.event)).collect::<Result<Vec<_>>>()?)
            .bind(alerts.iter().map(|a| a.at).collect::<Vec<_>>())
            .bind(alerts.iter().map(|a| a.message.clone()).collect::<Vec<_>>())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }
    async fn alerts(&self, after: Option<i64>, limit: i64) -> Result<Vec<StatusAlert>> {
        let limit = limit.clamp(1, 500);
        let rows = sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('id',a.id,'machine_id',a.machine_id,'machine_name',m.name,'kind',a.kind,'event',a.event,'at',a.at,'message',a.message) FROM (SELECT * FROM status_alerts WHERE $1::bigint IS NULL OR id>$1 ORDER BY CASE WHEN $1::bigint IS NULL THEN -id ELSE id END LIMIT $2) a JOIN machines m ON m.id=a.machine_id ORDER BY a.id")
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.into_iter().map(decode).collect()
    }
    async fn current_deployments(&self) -> Result<Vec<(Uuid, Value)>> {
        // Same notion of "current" as flight targets: the newest deployment not known to have failed.
        sqlx::query_as("SELECT DISTINCT ON (d.machine_id) d.machine_id,d.snapshot FROM deployment_versions d JOIN job_targets t ON t.job_id=d.job_id AND t.machine_id=d.machine_id JOIN machines m ON m.id=d.machine_id AND m.deleted_at IS NULL WHERE t.status NOT IN ('failed','cancelled','skipped','blocked') ORDER BY d.machine_id,d.created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(db)
    }
}
fn encode<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| Error::Internal(e.to_string()))
}
/// The snake_case name serde gives a unit enum variant.
fn text<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(encode(value)?.as_str().unwrap_or_default().to_owned())
}
