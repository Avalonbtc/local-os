use super::*;
use chrono::{DateTime, Utc};
use serde_json::json;

type Row = (String, String, String, Option<String>, DateTime<Utc>);
fn icon((symbol, source, content_type, source_ref, updated_at): Row) -> CoinIcon {
    CoinIcon {
        symbol,
        source,
        content_type,
        source_ref,
        updated_at,
    }
}

#[async_trait::async_trait]
impl CoinIconRepository for PgStore {
    async fn coin_icons(&self) -> Result<Vec<CoinIcon>> {
        Ok(sqlx::query_as::<_, Row>(
            "SELECT symbol,source,content_type,source_ref,updated_at FROM coin_icons ORDER BY symbol",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?
        .into_iter()
        .map(icon)
        .collect())
    }
    async fn coin_icon(&self, symbol: &str) -> Result<Option<(CoinIcon, Vec<u8>)>> {
        Ok(sqlx::query_as::<_, (String, String, String, Option<String>, DateTime<Utc>, Vec<u8>)>(
            "SELECT symbol,source,content_type,source_ref,updated_at,data FROM coin_icons WHERE symbol=$1",
        )
        .bind(symbol)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .map(|(symbol, source, content_type, source_ref, updated_at, data)| {
            (icon((symbol, source, content_type, source_ref, updated_at)), data)
        }))
    }
    async fn save_coin_icon(&self, icon: &CoinIcon, bytes: &[u8], actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO coin_icons(symbol,content_type,data,source,source_ref,updated_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(symbol) DO UPDATE SET content_type=EXCLUDED.content_type,data=EXCLUDED.data,source=EXCLUDED.source,source_ref=EXCLUDED.source_ref,updated_at=EXCLUDED.updated_at")
            .bind(&icon.symbol)
            .bind(&icon.content_type)
            .bind(bytes)
            .bind(&icon.source)
            .bind(&icon.source_ref)
            .bind(icon.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "coin_icon.save",
            &icon.symbol,
            json!({"source":icon.source,"source_ref":icon.source_ref,"content_type":icon.content_type,"bytes":bytes.len()}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
    async fn delete_coin_icon(&self, symbol: &str, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let deleted = sqlx::query("DELETE FROM coin_icons WHERE symbol=$1")
            .bind(symbol)
            .execute(&mut *tx)
            .await
            .map_err(db)?
            .rows_affected();
        if deleted == 0 {
            return Err(Error::NotFound);
        }
        audit_tx(&mut tx, actor, "coin_icon.delete", symbol, json!({})).await?;
        tx.commit().await.map_err(db)
    }
}
