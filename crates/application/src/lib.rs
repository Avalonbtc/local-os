pub mod audit;
pub mod bmc;
pub mod catalog;
pub mod coin_icons;
pub mod fleet;
pub mod flights;
pub mod identity;
pub mod jobs;
pub mod mining;
pub mod push;
pub mod settings;
pub mod telemetry;
use rig_domain::*;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
pub struct App {
    pub bios: Option<Arc<dyn BiosProvider>>,
    pub repository: Arc<dyn Repository>,
    pub vault: Arc<dyn SecretVault>,
    pub remote: Arc<dyn RemoteExecutor>,
    pub runtime: Arc<dyn MinerRuntime>,
    pub adapters: Arc<HashMap<String, Arc<dyn MinerAdapter>>>,
    pub bmc: Arc<HashMap<String, Arc<dyn BmcProvider>>>,
    /// Rig-initiated telemetry; `Default` disables it.
    pub push: Arc<push::PushHub>,
    /// Coin icon directory, links and CoinGecko; `None` leaves only console-added icons.
    pub icons: Option<Arc<dyn IconSource>>,
}
impl App {
    pub async fn credential(&self, machine: uuid::Uuid, bmc: bool) -> Result<Credential> {
        self.vault
            .decrypt(&self.repository.machine_secret(machine, bmc).await?)
    }
}
pub fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(value.as_bytes()))
}
