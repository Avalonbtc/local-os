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
        let credential = self.credential(machine.id).await?;
        if kind == "bootstrap" {
            // Bootstrap is a versioned, atomic, idempotent installation and does not start miners.
            let mut result = self.runtime.bootstrap(&machine, &credential).await?;
            // Best effort: the monitor retries over SSH until the rig reports this target.
            if let Some(target) = self.push_target(machine.id) {
                let push = match self
                    .runtime
                    .configure_push(&machine, &credential, &target)
                    .await
                {
                    Ok(()) => json!("configured"),
                    Err(error) => json!(format!("主动上报未配置：{error}")),
                };
                if let Some(object) = result.as_object_mut() {
                    object.insert("push".into(), push);
                }
            }
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
                let status = match remote.status.as_str() {
                    // Re-queried after a lost connection and the rig has no record: the start
                    // request never arrived (the first re-query waits 30 s for one in flight).
                    "missing" if task.reconcile => {
                        remote.error = Some("操作没有送达矿机，未执行；可以重新下发".into());
                        "failed"
                    }
                    "missing" => "unknown",
                    other => other,
                }
                .to_string();
                let status = status.as_str();
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
