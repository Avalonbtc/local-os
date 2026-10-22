pub mod agent;
pub mod audit;
pub mod catalog;
pub mod enroll;
pub mod fleet;
pub mod flights;
pub mod identity;
pub mod jobs;
pub mod mining;
pub mod push;
pub mod settings;
pub mod status;
pub mod telemetry;
use rig_domain::*;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
pub struct App {
    pub repository: Arc<dyn Repository>,
    pub vault: Arc<dyn SecretVault>,
    pub remote: Arc<dyn RemoteExecutor>,
    pub runtime: Arc<dyn MinerRuntime>,
    pub adapters: Arc<HashMap<String, Arc<dyn MinerAdapter>>>,
    /// Rig-initiated telemetry; `Default` disables it.
    pub push: Arc<push::PushHub>,
    /// Live agent connections and the one-command installer; `Default` disables enrollment.
    pub agents: Arc<agent::AgentHub>,
    /// Pool miner APIs for pool-side offline detection; `None` checks local status only.
    pub pools: Option<Arc<dyn PoolStats>>,
}
impl App {
    pub async fn credential(&self, machine: uuid::Uuid) -> Result<Credential> {
        self.vault
            .decrypt(&self.repository.machine_secret(machine).await?)
    }
}
pub fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value.as_bytes()))
}
