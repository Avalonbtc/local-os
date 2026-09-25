use crate::{App, digest};
use rig_domain::*;
use serde_json::json;
use uuid::Uuid;

impl App {
    pub async fn login(&self, input: Login, source: &str) -> Result<(String, Session)> {
        let name = input.username.trim();
        if name.is_empty() || name.len() > 128 || input.password.len() > 1024 {
            return Err(Error::Unauthorized);
        }
        let limit_key = digest(&format!("{source}\0{name}"));
        if !self.repository.login_attempt(&limit_key, false).await? {
            return Err(Error::Forbidden("登录尝试过多，请 15 分钟后重试".into()));
        }
        let record = self.repository.password_hash(name).await?;
        let Some((id, hash)) = record else {
            return Err(Error::Unauthorized);
        };
        let vault = self.vault.clone();
        let password = input.password;
        let verified = tokio::task::spawn_blocking(move || vault.verify_password(&password, &hash))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if !verified {
            return Err(Error::Unauthorized);
        }
        self.repository.login_attempt(&limit_key, true).await?;
        let token = self.vault.random_token();
        let csrf = self.vault.random_token();
        let session = self
            .repository
            .create_session(id, &digest(&token), &csrf)
            .await?;
        self.repository
            .audit(&session.actor, "identity.login", name, json!({}))
            .await?;
        Ok((token, session))
    }
    pub async fn authenticate(
        &self,
        bearer: Option<&str>,
        cookie: Option<&str>,
    ) -> Result<Session> {
        if let Some(token) = bearer {
            let actor = self.repository.token_actor(&digest(token)).await?;
            return Ok(Session {
                actor,
                csrf: String::new(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            });
        }
        self.repository
            .session(&digest(cookie.ok_or(Error::Unauthorized)?))
            .await
    }
    pub async fn logout(&self, cookie: &str) -> Result<()> {
        self.repository.delete_session(&digest(cookie)).await
    }
    pub async fn issue_token(&self, actor: &Actor, name: &str) -> Result<serde_json::Value> {
        valid_name(name)?;
        let token = format!("rd_{}", self.vault.random_token());
        let id = self
            .repository
            .create_token(actor, name, &digest(&token))
            .await?;
        Ok(json!({"id":id,"token":token}))
    }
    pub async fn revoke_token(&self, actor: &Actor, id: Uuid) -> Result<()> {
        self.repository.revoke_token(actor, id).await
    }
}

impl App {
    pub async fn tokens(&self, actor: &Actor) -> Result<Vec<TokenInfo>> {
        self.repository.tokens(actor).await
    }
}
