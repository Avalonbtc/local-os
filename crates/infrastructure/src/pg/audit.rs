use super::*;
use sqlx::{Postgres, Transaction};

pub(crate) async fn audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: &Actor,
    action: &str,
    target: &str,
    detail: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_events(actor,action,target,detail) VALUES($1,$2,$3,$4)")
        .bind(&actor.name)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}

#[async_trait::async_trait]
impl AuditRepository for PgStore {
    async fn audit(&self, actor: &Actor, action: &str, target: &str, detail: Value) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        audit_tx(&mut tx, actor, action, target, detail).await?;
        tx.commit().await.map_err(db)
    }
    async fn audit_events(&self, limit: i64) -> Result<Vec<AuditEvent>> {
        let values = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(a) FROM audit_events a ORDER BY id DESC LIMIT $1",
        )
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        values.into_iter().map(decode).collect()
    }
}
