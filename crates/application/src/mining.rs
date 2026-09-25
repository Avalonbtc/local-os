use crate::App;
use rig_domain::*;
use serde_json::json;
use std::time::Duration;

impl App {
    async fn finish_with_retry(
        &self,
        target: uuid::Uuid,
        lease: uuid::Uuid,
        status: &str,
        remote: &RemoteOperation,
    ) -> Result<()> {
        let started = tokio::time::Instant::now();
        loop {
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                self.repository.finish(target, lease, status, remote),
            )
            .await
            .unwrap_or_else(|_| Err(Error::Unavailable("保存任务结果超时".into())));
            match result {
                Err(Error::Unavailable(_)) if started.elapsed() < Duration::from_secs(45) => {
                    tokio::time::sleep(Duration::from_secs(2)).await
                }
                other => return other,
            }
        }
    }
    pub(crate) async fn execute_task_inner(
        &self,
        task: &ClaimedTask,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let machine = self.repository.machine(task.target.machine_id).await?;
        let kind = task.action["kind"].as_str().unwrap_or("");
        if ["bios_read", "bios_write"].contains(&kind) {
            let credential = self.credential(machine.id, true).await?;
            let provider = self
                .bios
                .as_ref()
                .ok_or_else(|| Error::Unavailable("SUM 未配置".into()))?;
            let mut remote = provider
                .execute(
                    &machine,
                    &credential,
                    task.target.operation_id,
                    &task.action,
                    task.reconcile,
                )
                .await?;
            redact_operation(&mut remote, &credential);
            return self
                .finish_with_retry(task.target.id, task.lease, &remote.status, &remote)
                .await;
        }
        if kind == "power" {
            let config = machine
                .bmc
                .as_ref()
                .ok_or_else(|| Error::Validation("未配置 BMC".into()))?;
            let credential = self.credential(machine.id, true).await?;
            let provider = self
                .bmc
                .get(&config.provider)
                .ok_or_else(|| Error::Validation("BMC Provider 不存在".into()))?;
            if task.reconcile {
                let state = provider.read(config, &credential, "power").await?;
                let remote = RemoteOperation {
                    status: "unknown".into(),
                    output: String::new(),
                    truncated: false,
                    result: Some(state),
                    error: Some(
                        "电源请求结果不明；已查询当前状态，不自动重复发送动作。核实后解除锁。"
                            .into(),
                    ),
                };
                return self
                    .finish_with_retry(task.target.id, task.lease, "unknown", &remote)
                    .await;
            }
            let operation = task.action["operation"].as_str().unwrap_or("");
            let result = provider.power(config, &credential, operation).await?;
            let remote = RemoteOperation {
                status: "succeeded".into(),
                output: String::new(),
                truncated: false,
                result: Some(result),
                error: None,
            };
            return self
                .finish_with_retry(task.target.id, task.lease, "succeeded", &remote)
                .await;
        }
        let credential = self.credential(machine.id, false).await?;
        if kind == "bootstrap" {
            // Bootstrap is a versioned, atomic, idempotent installation and does not start miners.
            let result = self.runtime.bootstrap(&machine, &credential).await?;
            return self
                .finish_with_retry(
                    task.target.id,
                    task.lease,
                    "succeeded",
                    &RemoteOperation {
                        status: "succeeded".into(),
                        output: String::new(),
                        truncated: false,
                        result: Some(result),
                        error: None,
                    },
                )
                .await;
        }
        let action = if kind == "apply" {
            json!({"kind":"apply","deployment":task.action["snapshot"]["hosts"][machine.id.to_string()]})
        } else {
            task.action.clone()
        };
        if !task.reconcile {
            self.runtime
                .start_operation(&machine, &credential, task.target.operation_id, &action)
                .await?;
        }
        let mut last_progress = Vec::new();
        loop {
            let cancel = *cancel_rx.borrow();
            if cancel {
                self.runtime
                    .cancel(&machine, &credential, task.target.operation_id)
                    .await?;
            }
            let mut remote = self
                .runtime
                .operation(&machine, &credential, task.target.operation_id)
                .await?;
            redact_operation(&mut remote, &credential);
            if ["succeeded", "failed", "cancelled", "unknown", "missing"]
                .contains(&remote.status.as_str())
            {
                let status = if remote.status == "missing" {
                    "unknown"
                } else {
                    &remote.status
                };
                return self
                    .finish_with_retry(task.target.id, task.lease, status, &remote)
                    .await;
            }
            let progress =
                serde_json::to_vec(&remote).map_err(|e| Error::Internal(e.to_string()))?;
            if progress != last_progress {
                match self
                    .repository
                    .task_progress(task.target.id, task.lease, &remote)
                    .await
                {
                    Ok(()) => last_progress = progress,
                    Err(Error::Unavailable(error)) => {
                        tracing::warn!(%error,"progress will be retried")
                    }
                    Err(error) => return Err(error),
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn redact_operation(remote: &mut RemoteOperation, credential: &Credential) {
    let secrets: Vec<&str> = match credential {
        Credential::Password {
            password,
            sudo_password,
        } => {
            let mut values = vec![password.as_str()];
            if let Some(sudo_password) = sudo_password {
                values.push(sudo_password);
            }
            values
        }
        Credential::PrivateKey {
            private_key,
            passphrase,
            sudo_password,
        } => {
            let mut values = vec![private_key.as_str()];
            if let Some(passphrase) = passphrase {
                values.push(passphrase);
            }
            if let Some(sudo_password) = sudo_password {
                values.push(sudo_password);
            }
            values
        }
    };
    for secret in secrets.into_iter().filter(|s| !s.is_empty()) {
        remote.output = remote.output.replace(secret, "[REDACTED]");
        if let Some(error) = &mut remote.error {
            *error = error.replace(secret, "[REDACTED]");
        }
    }
}
