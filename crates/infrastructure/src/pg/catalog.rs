use super::*;
use serde_json::json;
use uuid::Uuid;

#[async_trait::async_trait]
impl CatalogRepository for PgStore {
    async fn register_artifact(&self, hash: &str, size: i64, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO package_artifacts(sha256,filename,size_bytes) VALUES($1,$2,$3) ON CONFLICT(sha256) DO NOTHING").bind(hash).bind(format!("{hash}.tar")).bind(size).execute(&mut *tx).await.map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "catalog.package_upload",
            hash,
            json!({"size_bytes":size}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
    async fn catalog(&self, kind: CatalogKind) -> Result<Vec<CatalogItem>> {
        let query = format!(
            "SELECT jsonb_build_object('id',id,'kind',$1::text,'name',name,'data',data,'revision',revision) FROM {} ORDER BY name",
            kind.table()
        );
        sqlx::query_scalar::<_, Value>(&query)
            .bind(serde_json::to_value(kind).unwrap().as_str().unwrap())
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(decode)
            .collect()
    }
    async fn catalog_item(&self, kind: CatalogKind, id: Uuid) -> Result<CatalogItem> {
        let query = format!(
            "SELECT jsonb_build_object('id',id,'kind',$1::text,'name',name,'data',data,'revision',revision) FROM {} WHERE id=$2",
            kind.table()
        );
        decode(
            sqlx::query_scalar::<_, Value>(&query)
                .bind(serde_json::to_value(kind).unwrap().as_str().unwrap())
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(db)?,
        )
    }
    async fn save_catalog(
        &self,
        kind: CatalogKind,
        id: Uuid,
        input: &CatalogInput,
        actor: &Actor,
    ) -> Result<CatalogItem> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let mut data = input.data.clone();
        if kind == CatalogKind::Wallet {
            let coin = if let Some(id) = data["coin_id"].as_str() {
                Uuid::parse_str(id).map_err(|_| Error::Validation("币种 ID 无效".into()))?
            } else {
                let symbol = data["coin_symbol"].as_str().unwrap_or("").trim();
                valid_name(symbol)?;
                sqlx::query_scalar::<_,Uuid>("INSERT INTO coins(id,name,data) VALUES($1,$2,$3) ON CONFLICT(symbol) DO UPDATE SET name=coins.name RETURNING id").bind(Uuid::new_v4()).bind(symbol).bind(json!({"symbol":symbol})).fetch_one(&mut *tx).await.map_err(db)?
            };
            data["coin_id"] = json!(coin);
            data.as_object_mut().unwrap().remove("coin_symbol");
            let changed=sqlx::query("INSERT INTO wallets(id,name,coin_id,data) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO UPDATE SET name=excluded.name,coin_id=excluded.coin_id,data=excluded.data,revision=wallets.revision+1 WHERE wallets.revision=$5").bind(id).bind(&input.name).bind(coin).bind(&data).bind(input.expected_revision).execute(&mut *tx).await.map_err(db)?.rows_affected();
            if changed == 0 {
                return Err(Error::Conflict("记录已被修改，请刷新后重试".into()));
            }
        } else {
            let table = kind.table();
            let query = format!(
                "INSERT INTO {table}(id,name,data) VALUES($1,$2,$3) ON CONFLICT(id) DO UPDATE SET name=excluded.name,data=excluded.data,revision={table}.revision+1 WHERE {table}.revision=$4"
            );
            let changed = sqlx::query(&query)
                .bind(id)
                .bind(&input.name)
                .bind(&data)
                .bind(input.expected_revision)
                .execute(&mut *tx)
                .await
                .map_err(db)?
                .rows_affected();
            if changed == 0 {
                return Err(Error::Conflict("记录已被修改，请刷新后重试".into()));
            }
        }
        audit_tx(
            &mut tx,
            actor,
            "catalog.save",
            &id.to_string(),
            json!({"kind":kind,"name":input.name}),
        )
        .await?;
        tx.commit().await.map_err(db)?;
        self.catalog_item(kind, id).await
    }
    async fn delete_catalog(&self, kind: CatalogKind, id: Uuid, actor: &Actor) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(&format!("DELETE FROM {} WHERE id=$1", kind.table()))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "catalog.delete",
            &id.to_string(),
            json!({"kind":kind}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
}
