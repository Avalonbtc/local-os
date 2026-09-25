use async_trait::async_trait;
use rig_domain::*;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub struct SumBios {
    pub root: PathBuf,
    pub binary: PathBuf,
}

#[async_trait]
impl BiosProvider for SumBios {
    async fn cached(&self, machine: Uuid) -> Result<Value> {
        let path = self.root.join(machine.to_string()).join("snapshot.json");
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::Internal(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(
                json!({"machine_id":machine,"settings":[],"pending":{},"licenses":[],"revision":null,"observed_at":null}),
            ),
            Err(e) => Err(Error::Internal(e.to_string())),
        }
    }
    async fn execute(
        &self,
        machine: &Machine,
        credential: &Credential,
        operation: Uuid,
        action: &Value,
        reconcile: bool,
    ) -> Result<RemoteOperation> {
        let config = machine
            .bmc
            .as_ref()
            .ok_or_else(|| Error::Validation("未配置 BMC".into()))?;
        let Credential::Password { password, .. } = credential else {
            return Err(Error::Validation("BMC 需要密码认证".into()));
        };
        let request = json!({"root":self.root,"binary":self.binary,"machine_id":machine.id,"operation_id":operation,"action":action,"reconcile":reconcile,"host":config.url,"username":config.username,"password":password});
        // Secrets only on stdin; the child finishes its durable journal if the request is dropped.
        let mut child = tokio::process::Command::new("python3")
            .args(["-c", include_str!("../../../runtime/bios-sum.py")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Unavailable(format!("无法启动 SUM 适配层: {e}")))?;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&serde_json::to_vec(&request).map_err(|e| Error::Internal(e.to_string()))?)
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| Error::Unavailable(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr
                .chars()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            tracing::warn!(machine = %machine.name, %operation, stderr = %tail.replace(password.as_str(), "[REDACTED]"), "SUM adapter exited abnormally");
            return Err(Error::Unavailable(
                "SUM 适配层中断，请在任务页核实结果，不要重复写入".into(),
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| Error::Unavailable("SUM 返回无效状态，请核实操作记录".into()))
    }
}
