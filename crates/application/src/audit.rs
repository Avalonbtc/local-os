use crate::App;
use rig_domain::*;
use serde_json::json;
use uuid::Uuid;

impl App {
    pub async fn audit_events(&self) -> Result<Vec<AuditEvent>> {
        self.repository.audit_events(250).await
    }
    pub async fn record_terminal(&self, actor: &Actor, machine: Uuid, kind: &str) -> Result<()> {
        self.repository
            .audit(
                actor,
                "terminal.open",
                &machine.to_string(),
                json!({"kind":kind}),
            )
            .await
    }
}
