mod audit;
pub(crate) use audit::audit_tx;
mod catalog;
mod fleet;
mod flights;
mod identity;
mod jobs;
mod telemetry;
use rig_domain::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgStore {
    pub pool: PgPool,
}
impl PgStore {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .map_err(db)?;
        Ok(Self { pool })
    }
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations")
            .run(&self.pool)
            .await
            .map_err(|e| Error::Internal(e.to_string()))
    }
}
pub(crate) fn db(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::RowNotFound => Error::NotFound,
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            Error::Conflict("名称或幂等键已存在".into())
        }
        sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
            Error::Conflict("记录仍被引用，或引用的记录不存在".into())
        }
        _ => {
            tracing::error!(error=%e,"database error");
            Error::Unavailable("PostgreSQL 不可用或操作失败".into())
        }
    }
}
fn decode<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error::Internal(e.to_string()))
}
