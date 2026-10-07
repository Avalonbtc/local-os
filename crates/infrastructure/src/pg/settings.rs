use super::*;
use serde_json::json;

#[async_trait::async_trait]
impl SettingsRepository for PgStore {
    async fn farm_settings(&self) -> Result<Option<Value>> {
        sqlx::query_scalar("SELECT data FROM farm_settings WHERE id")
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }
    async fn save_farm_settings(&self, settings: &Value, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let previous: Option<Value> =
            sqlx::query_scalar("SELECT data FROM farm_settings WHERE id FOR UPDATE")
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
        sqlx::query("INSERT INTO farm_settings(id,data) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET data=EXCLUDED.data,updated_at=now()")
            .bind(settings)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "settings.farm",
            "farm",
            json!({"before":previous,"after":settings}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
}
