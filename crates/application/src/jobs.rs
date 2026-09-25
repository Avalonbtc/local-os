use crate::App;
use rig_domain::*;
use std::{collections::HashSet, time::Duration};
use uuid::Uuid;

impl App {
    pub async fn submit_job(&self, actor: &Actor, mut input: JobInput) -> Result<Uuid> {
        if input.machine_ids.is_empty()
            || input.machine_ids.len() > 128
            || input.concurrency == 0
            || input.concurrency > 8
            || input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 128
        {
            return Err(Error::Validation("需选择机器、1–8 并发和有效幂等键".into()));
        }
        let mut seen = HashSet::new();
        input.machine_ids.retain(|id| seen.insert(*id));
        match &input.action {
            Action::BiosRead { .. } | Action::BiosWrite { .. } => {
                if input.machine_ids.len() != 1 || self.bios.is_none() {
                    return Err(Error::Validation("BIOS 操作需要单台机器及 SUM 支持".into()));
                }
                let machine = self.repository.machine(input.machine_ids[0]).await?;
                if machine.bmc.is_none() {
                    return Err(Error::Validation("请先配置 BMC".into()));
                }
                if let Action::BiosWrite { revision, changes } = &input.action
                    && (revision.len() != 64
                        || !revision.bytes().all(|b| b.is_ascii_hexdigit())
                        || changes.is_empty()
                        || changes.len() > 64
                        || changes.iter().any(|(k, v)| k.len() > 2048 || v.len() > 256))
                {
                    return Err(Error::Validation("BIOS 修改或配置版本无效".into()));
                }
            }
            Action::Command {
                script,
                timeout_seconds,
            } if script.trim().is_empty()
                || script.len() > 65536
                || !(1..=86400).contains(timeout_seconds) =>
            {
                return Err(Error::Validation("命令为空、超过 64 KiB 或超时无效".into()));
            }
            Action::Miner {
                operation,
                instances,
            } => {
                if !["start", "stop", "restart"].contains(&operation.as_str())
                    || instances.is_empty()
                    || instances.iter().any(|s| {
                        s.is_empty()
                            || !s
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                    })
                {
                    return Err(Error::Validation("矿工操作或实例无效".into()));
                }
            }
            Action::Power { operation }
                if !["on", "shutdown", "reboot", "force_off", "force_restart"]
                    .contains(&operation.as_str()) =>
            {
                return Err(Error::Validation("电源动作无效".into()));
            }
            Action::Adopt {
                name,
                pid,
                start_identity,
                start_script,
                stop_script,
                pid_script,
            } => {
                if input.machine_ids.len() != 1
                    || *pid <= 1
                    || name.is_empty()
                    || name.len() > 40
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                    || start_identity.is_empty()
                    || [start_script, stop_script, pid_script]
                        .iter()
                        .any(|s| s.trim().is_empty() || s.len() > 65536)
                {
                    return Err(Error::Validation(
                        "纳管需单台机器、有效 PID 身份及精确的启停和 PID 查询命令".into(),
                    ));
                }
            }
            _ => {}
        }
        let mut machines = Vec::new();
        for id in &input.machine_ids {
            machines.push(self.repository.machine(*id).await?);
        }
        if matches!(input.action, Action::Power { .. }) {
            if !input.include_controller {
                machines.retain(|m| !m.is_controller);
            }
            machines.sort_by_key(|m| m.is_controller);
        }
        if machines.is_empty() {
            return Err(Error::Validation("排除主控后没有目标机器".into()));
        }
        input.machine_ids = machines.iter().map(|m| m.id).collect();
        let mut snapshot =
            serde_json::to_value(&input.action).map_err(|e| Error::Internal(e.to_string()))?;
        if let Action::Apply { sheet_id } = input.action {
            snapshot["snapshot"] = self.compile_flight(sheet_id, &machines).await?;
        }
        self.repository.enqueue(&input, &snapshot, actor).await
    }

    pub async fn run_worker(self) {
        let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        loop {
            if let Err(error) = self.repository.expire_leases().await {
                tracing::warn!(%error,"queue unavailable");
            }
            let permit = match limit.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            match self.repository.claim().await {
                Ok(Some(task)) => {
                    let app = self.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        app.execute_task(task).await;
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => {
                    drop(permit);
                    tracing::warn!(%error,"claim failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
    async fn execute_task(&self, task: ClaimedTask) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let execution = self.execute_task_inner(&task, cancel_rx);
        let mut renewed_at = tokio::time::Instant::now();
        tokio::pin!(execution);
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        let result = loop {
            tokio::select! {
                result = &mut execution => break result,
                _ = interval.tick() => {
                    let attempt_started = tokio::time::Instant::now();
                    let heartbeat = tokio::time::timeout(Duration::from_secs(5), self.repository.heartbeat(task.target.id,task.lease)).await
                        .unwrap_or_else(|_| Err(Error::Unavailable("任务心跳超时".into())));
                    match heartbeat {
                        Ok(cancel) => { renewed_at = attempt_started; let _ = cancel_tx.send(cancel); }
                        Err(error) if matches!(error, Error::Unavailable(_)) && renewed_at.elapsed() < Duration::from_secs(50) => {
                            tracing::warn!(%error,target=%task.target.id,"transient heartbeat failure; retry before lease expiry");
                        }
                        Err(error) => break Err(error),
                    }
                }
            }
        };
        if let Err(error) = result {
            let status = if matches!(
                error,
                Error::Validation(_) | Error::Forbidden(_) | Error::NotFound
            ) {
                "failed"
            } else {
                "unknown"
            };
            let remote = RemoteOperation {
                status: status.into(),
                output: String::new(),
                truncated: false,
                result: None,
                error: Some(error.to_string()),
            };
            // A transport error never proves that the remote operation failed. Keep the host lock.
            if let Err(e) = self
                .repository
                .finish(task.target.id, task.lease, status, &remote)
                .await
            {
                tracing::warn!(error=%e,"cannot persist task result");
            }
        }
    }

    pub async fn jobs(&self) -> Result<Vec<Job>> {
        self.repository.jobs(100).await
    }
    pub async fn job(&self, id: Uuid) -> Result<Job> {
        self.repository.job(id).await
    }
    pub async fn cancel_job(&self, actor: &Actor, id: Uuid) -> Result<()> {
        self.repository.cancel_job(id, actor).await
    }
}

impl App {
    pub async fn resolve_target(
        &self,
        actor: &Actor,
        id: uuid::Uuid,
        input: ResolveTarget,
    ) -> Result<()> {
        if !["reconcile", "succeeded", "failed", "cancelled"].contains(&input.decision.as_str())
            || input.note.trim().len() < 10
        {
            return Err(Error::Validation(
                "选择对账或核实后的结果，并填写至少 10 字节的依据".into(),
            ));
        }
        self.repository.resolve_target(id, &input, actor).await
    }
}
