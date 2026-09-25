use crate::App;
use rig_domain::*;
use uuid::Uuid;

impl App {
    pub async fn probe_ssh_host_key(
        &self,
        actor: &Actor,
        input: SshProbeInput,
    ) -> Result<SshHostKey> {
        validate_ssh_endpoint(&input.host, input.port)?;
        self.repository
            .audit(
                actor,
                "fleet.host_key_probe",
                &format!("{}:{}", input.host, input.port),
                serde_json::json!({}),
            )
            .await?;
        self.remote.probe_host_key(&input.host, input.port).await
    }
    /// Fetch many host keys at once (16 in flight) so a whole rack can be reviewed on one screen.
    /// Nothing is trusted here: keys are only shown for confirmation before machines are saved.
    pub async fn probe_ssh_host_keys(
        &self,
        actor: &Actor,
        input: SshProbeBatch,
    ) -> Result<Vec<SshProbeResult>> {
        if input.targets.is_empty() || input.targets.len() > 256 {
            return Err(Error::Validation("一次获取 1–256 台机器的指纹".into()));
        }
        for target in &input.targets {
            validate_ssh_endpoint(&target.host, target.port)?;
        }
        self.repository
            .audit(
                actor,
                "fleet.host_key_probe",
                "batch",
                serde_json::json!({"count": input.targets.len()}),
            )
            .await?;
        use futures_util::StreamExt;
        let results = futures_util::stream::iter(input.targets.into_iter().enumerate())
            .map(|(index, target)| async move {
                let result = self.remote.probe_host_key(&target.host, target.port).await;
                let (fingerprint, algorithm, error) = match result {
                    Ok(key) => (Some(key.fingerprint), Some(key.algorithm), None),
                    Err(error) => (None, None, Some(error.to_string())),
                };
                (
                    index,
                    SshProbeResult {
                        host: target.host,
                        port: target.port,
                        fingerprint,
                        algorithm,
                        error,
                    },
                )
            })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
        let mut results = results;
        results.sort_by_key(|(index, _)| *index);
        Ok(results.into_iter().map(|(_, result)| result).collect())
    }
    /// Create many machines with per-row results; one bad row never blocks the others.
    pub async fn save_machines_batch(
        &self,
        actor: &Actor,
        input: MachineBatchInput,
    ) -> Result<MachineBatchResult> {
        if input.machines.is_empty() || input.machines.len() > 256 {
            return Err(Error::Validation("一次添加 1–256 台机器".into()));
        }
        let mut names = std::collections::HashSet::new();
        let mut results = Vec::with_capacity(input.machines.len());
        for machine in input.machines {
            let (name, host) = (machine.name.clone(), machine.host.clone());
            let outcome = if !names.insert(name.clone()) {
                Err(Error::Validation("名称在本批次中重复".into()))
            } else if machine.credential.is_none() {
                Err(Error::Validation("新机器需要 SSH 凭据".into()))
            } else {
                self.save_machine(actor, Uuid::new_v4(), machine).await
            };
            results.push(match outcome {
                Ok(saved) => MachineBatchRow {
                    name,
                    host,
                    machine_id: Some(saved.id),
                    error: None,
                },
                Err(error) => MachineBatchRow {
                    name,
                    host,
                    machine_id: None,
                    error: Some(error.to_string()),
                },
            });
        }
        let created: Vec<Uuid> = results.iter().filter_map(|r| r.machine_id).collect();
        let (mut bootstrap_job_id, mut bootstrap_error) = (None, None);
        if input.bootstrap && !created.is_empty() {
            // The machines already exist: report a queueing failure instead of failing the batch,
            // otherwise a retry would only hit duplicate names.
            match self
                .submit_job(
                    actor,
                    JobInput {
                        machine_ids: created,
                        action: Action::Bootstrap,
                        idempotency_key: Uuid::new_v4().to_string(),
                        concurrency: 8,
                        canary: true,
                        include_controller: false,
                    },
                )
                .await
            {
                Ok(id) => bootstrap_job_id = Some(id),
                Err(error) => bootstrap_error = Some(error.to_string()),
            }
        }
        Ok(MachineBatchResult {
            results,
            bootstrap_job_id,
            bootstrap_error,
        })
    }
    pub async fn machines(&self) -> Result<Vec<Machine>> {
        self.repository.machines().await
    }
    pub async fn save_machine(
        &self,
        actor: &Actor,
        id: Uuid,
        input: MachineInput,
    ) -> Result<Machine> {
        valid_name(&input.name)?;
        validate_ssh_endpoint(&input.host, input.port)?;
        if !input.host_key.starts_with("SHA256:") || input.host_key.len() < 40 {
            return Err(Error::Validation(
                "需要通过可信渠道核对的 SSH SHA256 主机指纹".into(),
            ));
        }
        if input.username.is_empty()
            || input.username.starts_with('-')
            || !input
                .username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        {
            return Err(Error::Validation("SSH 用户名无效".into()));
        }
        if input.sudo_password.as_deref().is_some_and(|password| {
            password.is_empty()
                || password.len() > 1024
                || password.chars().any(|c| c == '\n' || c == '\r')
        }) {
            return Err(Error::Validation("sudo 密码格式无效".into()));
        }
        if let Some(bmc) = &input.bmc {
            if !self.bmc.contains_key(&bmc.provider) {
                return Err(Error::Validation("未知 BMC Provider".into()));
            }
            if bmc.provider == "redfish" && !bmc.url.starts_with("https://") {
                return Err(Error::Validation(
                    "Redfish 使用 HTTPS；自签证书请填写 CA PEM".into(),
                ));
            }
        }
        let mut ssh_credential = if let Some(credential) = input.credential.clone() {
            Some(credential)
        } else if input.sudo_password.is_some() {
            Some(self.credential(id, false).await?)
        } else {
            None
        };
        if let Some(credential) = ssh_credential.as_mut() {
            if let Some(password) = input.sudo_password.as_ref() {
                credential.set_sudo_password(Some(password.clone()));
            } else if input.credential.is_some() {
                // Replacing SSH credentials should not silently discard a saved sudo password.
                if let Ok(existing) = self.credential(id, false).await
                    && let Some(password) = existing.explicit_sudo_password()
                {
                    credential.set_sudo_password(Some(password.to_owned()));
                }
            }
        }
        let ssh = ssh_credential
            .as_ref()
            .map(|c| self.vault.encrypt(c))
            .transpose()?;
        let bmc = input
            .bmc_credential
            .as_ref()
            .map(|c| self.vault.encrypt(c))
            .transpose()?;
        self.repository
            .save_machine(id, &input, ssh.as_deref(), bmc.as_deref(), actor)
            .await
    }
    pub async fn test_machine(&self, actor: &Actor, id: Uuid) -> Result<ExecOutput> {
        let machine = self.repository.machine(id).await?;
        let credential = self.credential(id, false).await?;
        self.repository
            .audit(
                actor,
                "fleet.connection_test",
                &id.to_string(),
                serde_json::json!({}),
            )
            .await?;
        let mut check = self
            .remote
            .execute(
                &machine,
                &credential,
                "id; uname -srmo; command -v python3; command -v screen",
                20,
            )
            .await?;
        if machine.username != "root" {
            let mut sudo = self
                .remote
                .execute(&machine, &credential, "sudo -n true", 20)
                .await?;
            if sudo.exit_code != Some(0)
                && let Some(password) = credential
                    .sudo_password()
                    .filter(|password| !password.is_empty())
            {
                sudo = self
                    .remote
                    .execute_input(
                        &machine,
                        &credential,
                        "sudo -S -p '' true",
                        format!("{password}\n").as_bytes(),
                        20,
                    )
                    .await?;
            }
            if sudo.exit_code != Some(0) {
                check
                    .stdout
                    .push_str("\nsudo: 需要正确的 sudo 密码或免密码 sudo\n");
                check.exit_code = sudo.exit_code;
            } else {
                check.stdout.push_str("\nsudo: 可用\n");
            }
        }
        Ok(check)
    }
}

fn validate_ssh_endpoint(host: &str, port: u16) -> Result<()> {
    if host.is_empty()
        || host.len() > 253
        || port == 0
        || host
            .chars()
            .any(|c| c.is_whitespace() || "'\"/;\\".contains(c))
    {
        return Err(Error::Validation("SSH 地址或端口无效".into()));
    }
    Ok(())
}

impl App {
    pub async fn machine(&self, id: uuid::Uuid) -> Result<Machine> {
        self.repository.machine(id).await
    }
    pub async fn delete_machine(&self, actor: &Actor, id: uuid::Uuid) -> Result<()> {
        self.repository.delete_machine(id, actor).await
    }
}
