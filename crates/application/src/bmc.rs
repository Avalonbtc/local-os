use crate::App;
use rig_domain::*;
use serde_json::Value;
use uuid::Uuid;

impl App {
    pub async fn bios_cached(&self, id: Uuid) -> Result<Value> {
        self.repository.machine(id).await?;
        self.bios
            .as_ref()
            .ok_or_else(|| Error::Unavailable("SUM 未配置".into()))?
            .cached(id)
            .await
    }
    pub async fn bmc_read(&self, id: Uuid, kind: &str) -> Result<Value> {
        let machine = self.repository.machine(id).await?;
        let config = machine.bmc.as_ref().ok_or(Error::NotFound)?;
        let credential = self.credential(id, true).await?;
        self.bmc
            .get(&config.provider)
            .ok_or(Error::NotFound)?
            .read(config, &credential, kind)
            .await
    }
}
