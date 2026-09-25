use crate::App;
use rig_domain::*;
use uuid::Uuid;

impl App {
    pub async fn catalog(&self, kind: CatalogKind) -> Result<Vec<CatalogItem>> {
        self.repository.catalog(kind).await
    }
    pub async fn save_catalog(
        &self,
        actor: &Actor,
        kind: CatalogKind,
        id: Uuid,
        mut input: CatalogInput,
    ) -> Result<CatalogItem> {
        valid_name(&input.name)?;
        if !input.data.is_object() {
            return Err(Error::Validation("配置必须是 JSON 对象".into()));
        }
        match kind {
            CatalogKind::Coin => {
                let symbol = input.data["symbol"].as_str().unwrap_or("");
                valid_name(symbol)?;
            }
            CatalogKind::Wallet => {
                let address = input.data["address"].as_str().unwrap_or("");
                if address.trim().is_empty() || address.len() > 4096 {
                    return Err(Error::Validation("请填写收款地址或矿池账号".into()));
                }
                if input.data["coin_id"].as_str().is_none()
                    && input.data["coin_symbol"].as_str().is_none()
                {
                    return Err(Error::Validation("选择或输入自定义币种".into()));
                }
            }
            CatalogKind::Pool => {
                if input.data["urls"].as_array().is_none_or(|x| x.is_empty()) {
                    return Err(Error::Validation("至少填写一个矿池地址".into()));
                }
            }
            CatalogKind::OcProfile => {
                serde_json::from_value::<GpuOcConfig>(input.data.clone())
                    .map_err(|e| Error::Validation(format!("超频参数格式无效：{e}")))?
                    .validate()?;
            }
            CatalogKind::Miner => {
                if input.data["adapter"].as_str().is_none() {
                    input.data["adapter"] = "hive-custom".into();
                }
                let adapter = input.data["adapter"].as_str().unwrap_or("");
                if !self.adapters.contains_key(adapter) {
                    return Err(Error::Validation("未知矿工适配器".into()));
                }
                let url = input.data["url"].as_str().unwrap_or("");
                if !url.starts_with("https://") && !url.starts_with("cache:") {
                    return Err(Error::Validation(
                        "安装包使用 HTTPS URL 或已上传缓存".into(),
                    ));
                }
                if adapter != "hive-custom" && input.data["executable"].as_str().is_none() {
                    return Err(Error::Validation("填写包内可执行文件的相对路径".into()));
                }
                let hash = input.data["sha256"].as_str().unwrap_or("");
                if hash.is_empty() {
                    if let Some(hash) = url.strip_prefix("cache:") {
                        input.data["sha256"] = hash.to_string().into();
                    } else {
                        let artifact = self.runtime.import_package(url).await?;
                        self.repository
                            .register_artifact(
                                artifact["sha256"].as_str().unwrap_or(""),
                                artifact["size_bytes"].as_i64().unwrap_or(0),
                                actor,
                            )
                            .await?;
                        input.data["sha256"] = artifact["sha256"].clone();
                    }
                }
                let hash = input.data["sha256"].as_str().unwrap_or("");
                if hash.len() != 64 || !hash.bytes().all(|x| x.is_ascii_hexdigit()) {
                    return Err(Error::Validation("安装包校验信息无效".into()));
                }
            }
        }
        self.repository.save_catalog(kind, id, &input, actor).await
    }
}

impl App {
    pub async fn delete_catalog(
        &self,
        actor: &Actor,
        kind: CatalogKind,
        id: uuid::Uuid,
    ) -> Result<()> {
        self.repository.delete_catalog(kind, id, actor).await
    }
    pub async fn cache_package_stream(
        &self,
        actor: &Actor,
        chunks: tokio::sync::mpsc::Receiver<Result<Vec<u8>>>,
    ) -> Result<serde_json::Value> {
        let result = self.runtime.cache_stream(chunks).await?;
        self.repository
            .register_artifact(
                result["sha256"].as_str().unwrap_or(""),
                result["size_bytes"].as_i64().unwrap_or(0),
                actor,
            )
            .await?;
        Ok(result)
    }
    pub async fn cache_package(&self, actor: &Actor, bytes: &[u8]) -> Result<serde_json::Value> {
        let result = self.runtime.cache_package(bytes).await?;
        self.repository
            .register_artifact(
                result["sha256"].as_str().unwrap_or(""),
                bytes.len() as i64,
                actor,
            )
            .await?;
        Ok(result)
    }
    pub fn adapter_descriptions(&self) -> Vec<AdapterDescription> {
        let mut values: Vec<_> = self.adapters.values().map(|x| x.describe()).collect();
        values.sort_by(|a, b| a.id.cmp(&b.id));
        values
    }
}
