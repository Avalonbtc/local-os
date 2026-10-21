//! One-command join: `curl -fsSL <panel>/agent/install.sh | sudo bash -s -- <install key>`.
//!
//! An install key is `rgk_` + token id + HMAC(master-derived, token id): it can be shown again
//! at any time, nothing secret is stored, and revoking the token row disables it. A key can only
//! add machines; it never controls existing ones.
use crate::App;
use rig_domain::*;
use serde_json::json;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Shown on the rig by the installer: say what to do rather than "log in again".
fn key_refused() -> Error {
    Error::Forbidden(
        "安装密钥无效、已撤销、已过期或可加入台数已用完；请在面板「添加矿机」复制新的安装命令"
            .into(),
    )
}

impl App {
    fn install_key(&self, token: Uuid) -> Option<String> {
        let mac = self.vault.derive(&format!("enrollment:{token}"))?;
        Some(format!("rgk_{}{}", token.simple(), &mac[..32]))
    }
    fn install_command(&self, key: &str) -> String {
        format!(
            "curl -fsSL {}/agent/install.sh | sudo bash -s -- {key}",
            self.agents.public_url()
        )
    }
    fn with_key(&self, mut token: EnrollmentToken) -> EnrollmentToken {
        token.token = self.install_key(token.id).unwrap_or_default();
        token.command = self.install_command(&token.token);
        token
    }
    /// The token row an install key belongs to, if the key is genuine.
    fn token_id(&self, key: &str) -> Result<Uuid> {
        let body = key
            .trim()
            .strip_prefix("rgk_")
            .filter(|b| b.len() == 64)
            .ok_or(key_refused())?;
        let id = Uuid::parse_str(&body[..32]).map_err(|_| key_refused())?;
        let expected = self.install_key(id).ok_or(key_refused())?;
        if bool::from(expected.as_bytes().ct_eq(key.trim().as_bytes())) {
            Ok(id)
        } else {
            Err(key_refused())
        }
    }
    pub async fn enrollment_tokens(&self) -> Result<Vec<EnrollmentToken>> {
        Ok(self
            .repository
            .enrollment_tokens()
            .await?
            .into_iter()
            .map(|t| self.with_key(t))
            .collect())
    }
    pub async fn create_enrollment_token(
        &self,
        actor: &Actor,
        input: EnrollmentTokenInput,
    ) -> Result<EnrollmentToken> {
        valid_name(&input.name)?;
        if input.group.len() > 128
            || input.tags.len() > 32
            || input.tags.iter().any(|t| t.is_empty() || t.len() > 64)
            || input.max_uses.is_some_and(|n| n < 1)
            || input
                .expires_in_hours
                .is_some_and(|h| h == 0 || h > 24 * 3650)
        {
            return Err(Error::Validation("分组、标签、有效期或使用次数无效".into()));
        }
        if self.agents.public_url().is_empty() {
            return Err(Error::Unavailable(
                "主控未设置对外地址（RIGDECK_ORIGIN），矿机无法连接".into(),
            ));
        }
        let token = self
            .repository
            .create_enrollment_token(Uuid::new_v4(), &input, actor)
            .await?;
        Ok(self.with_key(token))
    }
    pub async fn revoke_enrollment_token(&self, actor: &Actor, id: Uuid) -> Result<()> {
        self.repository.revoke_enrollment_token(id, actor).await
    }

    /// Called by `rig-miner enroll` during installation. `source` is the address the request
    /// came from, used as the machine's address when the rig reports none.
    pub async fn enroll(&self, input: EnrollRequest, source: &str) -> Result<EnrollReply> {
        if self.agents.public_url().is_empty() {
            return Err(Error::Unavailable("主控未开放矿机接入".into()));
        }
        let token = self.token_id(&input.token)?;
        if input.fingerprint.len() != 64
            || !input
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Validation("机器标识无效".into()));
        }
        let mut name: String = input
            .hostname
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c))
            .take(48)
            .collect();
        if name.is_empty() {
            name = format!("rig-{}", &input.fingerprint[..8]);
        }
        let host = input
            .addresses
            .iter()
            .find(|a| a.parse::<std::net::IpAddr>().is_ok())
            .cloned()
            .unwrap_or_else(|| source.to_owned());
        let secret = self.vault.encrypt(&Credential::Password {
            password: String::new(),
            sudo_password: None,
        })?;
        let (machine, created) = self
            .repository
            .enroll_machine(
                token,
                &NewAgentMachine {
                    fingerprint: input.fingerprint,
                    name,
                    host,
                    group: String::new(),
                    tags: Vec::new(),
                    ssh_secret: secret,
                },
            )
            .await
            .map_err(|error| match error {
                Error::Unauthorized => key_refused(),
                other => other,
            })?;
        let key = self
            .vault
            .push_key(machine.id)
            .ok_or_else(|| Error::Internal("无法生成机器密钥".into()))?;
        let push = self.push_target(machine.id).and_then(|target| {
            target.url.map(|url| {
                json!({"url":url,"machine_id":machine.id,"key":target.key,"digest":target.digest})
            })
        });
        tracing::info!(machine = %machine.id, name = %machine.name, created, "machine enrolled");
        Ok(EnrollReply {
            machine_id: machine.id,
            name: machine.name,
            created,
            key,
            connect_url: self.agents.connect_url(),
            push,
        })
    }
}
