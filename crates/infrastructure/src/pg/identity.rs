use super::*;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

#[async_trait::async_trait]
impl IdentityRepository for PgStore {
    async fn create_admin(&self, name: &str, hash: &str) -> Result<()> {
        sqlx::query("INSERT INTO users(id,name,password_hash) VALUES($1,$2,$3)")
            .bind(Uuid::new_v4())
            .bind(name)
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }
    async fn password_hash(&self, name: &str) -> Result<Option<(Uuid, String)>> {
        sqlx::query_as("SELECT id,password_hash FROM users WHERE name=$1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }
    async fn login_attempt(&self, key: &str, success: bool) -> Result<bool> {
        if success {
            sqlx::query("DELETE FROM login_limits WHERE name=$1")
                .bind(key)
                .execute(&self.pool)
                .await
                .map_err(db)?;
            return Ok(true);
        }
        let attempts:i32=sqlx::query_scalar("INSERT INTO login_limits(name,attempts,window_start) VALUES($1,1,now()) ON CONFLICT(name) DO UPDATE SET attempts=CASE WHEN login_limits.window_start < now()-interval '15 minutes' THEN 1 ELSE login_limits.attempts+1 END,window_start=CASE WHEN login_limits.window_start < now()-interval '15 minutes' THEN now() ELSE login_limits.window_start END RETURNING attempts").bind(key).fetch_one(&self.pool).await.map_err(db)?;
        Ok(attempts <= 10)
    }
    async fn create_session(&self, user: Uuid, hash: &str, csrf: &str) -> Result<Session> {
        sqlx::query("INSERT INTO sessions(token_hash,user_id,csrf,expires_at) VALUES($1,$2,$3,now()+interval '12 hours')").bind(hash).bind(user).bind(csrf).execute(&self.pool).await.map_err(db)?;
        self.session(hash).await
    }
    async fn session(&self, hash: &str) -> Result<Session> {
        let row=sqlx::query("SELECT u.id,u.name,s.csrf,s.expires_at FROM sessions s JOIN users u ON u.id=s.user_id WHERE token_hash=$1 AND expires_at>now()").bind(hash).fetch_optional(&self.pool).await.map_err(db)?.ok_or(Error::Unauthorized)?;
        Ok(Session {
            actor: Actor {
                id: row.get("id"),
                name: row.get("name"),
                token_id: None,
            },
            csrf: row.get("csrf"),
            expires_at: row.get("expires_at"),
        })
    }
    async fn delete_session(&self, hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
            .bind(hash)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }
    async fn token_actor(&self, hash: &str) -> Result<Actor> {
        let row=sqlx::query("SELECT u.id,u.name,t.id token_id FROM api_tokens t JOIN users u ON t.user_id=u.id WHERE token_hash=$1 AND revoked_at IS NULL").bind(hash).fetch_optional(&self.pool).await.map_err(db)?.ok_or(Error::Unauthorized)?;
        Ok(Actor {
            id: row.get("id"),
            name: row.get("name"),
            token_id: Some(row.get("token_id")),
        })
    }
    async fn create_token(&self, actor: &Actor, name: &str, hash: &str) -> Result<Uuid> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO api_tokens(id,user_id,name,token_hash) VALUES($1,$2,$3,$4)")
            .bind(id)
            .bind(actor.id)
            .bind(name)
            .bind(hash)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "identity.token_create",
            &id.to_string(),
            json!({"name":name}),
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(id)
    }
    async fn tokens(&self, actor: &Actor) -> Result<Vec<TokenInfo>> {
        sqlx::query_scalar::<_,Value>("SELECT to_jsonb(t)-'token_hash'-'user_id' FROM api_tokens t WHERE user_id=$1 ORDER BY created_at DESC").bind(actor.id).fetch_all(&self.pool).await.map_err(db)?.into_iter().map(decode).collect()
    }
    async fn revoke_token(&self, actor: &Actor, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("UPDATE api_tokens SET revoked_at=now() WHERE id=$1 AND user_id=$2")
            .bind(id)
            .bind(actor.id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        audit_tx(
            &mut tx,
            actor,
            "identity.token_revoke",
            &id.to_string(),
            json!({}),
        )
        .await?;
        tx.commit().await.map_err(db)
    }
}
