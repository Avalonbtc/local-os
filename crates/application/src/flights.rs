use crate::App;
use rig_domain::*;
use serde_json::{Value, json};
use std::collections::HashSet;
use uuid::Uuid;

impl App {
    pub async fn save_flight(
        &self,
        actor: &Actor,
        id: Uuid,
        input: FlightInput,
    ) -> Result<FlightSheet> {
        valid_name(&input.name)?;
        if input.tasks.is_empty() || input.tasks.len() > 32 {
            return Err(Error::Validation("飞行表需包含 1–32 个矿工任务".into()));
        }
        let mut names = HashSet::new();
        for task in &input.tasks {
            if task.instance.is_empty()
                || task.instance.len() > 40
                || !task
                    .instance
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !names.insert(&task.instance)
            {
                return Err(Error::Validation(
                    "实例名称需唯一，且仅含英文字母、数字、短横线和下划线".into(),
                ));
            }
            validate_miner(&task.miner, |id| self.adapters.contains_key(id))?;
            validate_pool_urls(&task.config)?;
            self.repository
                .catalog_item(CatalogKind::Wallet, task.wallet_id)
                .await?;
        }
        let machine_ids = self.repository.flight_targets(id).await?;
        let propagation = if machine_ids.is_empty() {
            None
        } else {
            let previous = self.repository.flight(id).await?;
            if input.expected_version != Some(previous.version) {
                return Err(Error::Conflict("飞行表已修改，请刷新后重试".into()));
            }
            let flight = FlightSheet {
                id,
                name: input.name.clone(),
                version: previous.version + 1,
                tasks: input.tasks.clone(),
                apply_job_id: None,
            };
            let mut machines = Vec::new();
            for machine_id in &machine_ids {
                machines.push(self.repository.machine(*machine_id).await?);
            }
            let snapshot = self.compile_flight_value(flight, &machines).await?;
            Some(FlightPropagation {
                job: JobInput {
                    machine_ids,
                    action: Action::Apply { sheet_id: id },
                    idempotency_key: format!("flight-edit-{id}-{}", previous.version + 1),
                    concurrency: 4,
                    canary: true,
                    include_controller: false,
                },
                snapshot: json!({"kind":"apply", "sheet_id":id, "snapshot":snapshot}),
            })
        };
        self.repository
            .save_flight(id, &input, actor, propagation.as_ref())
            .await
    }
    pub async fn compile_flight(&self, sheet_id: Uuid, machines: &[Machine]) -> Result<Value> {
        let flight = self.repository.flight(sheet_id).await?;
        self.compile_flight_value(flight, machines).await
    }
    async fn compile_flight_value(
        &self,
        flight: FlightSheet,
        machines: &[Machine],
    ) -> Result<Value> {
        let mut resolved = Vec::new();
        for task in &flight.tasks {
            validate_miner(&task.miner, |id| self.adapters.contains_key(id))?;
            validate_pool_urls(&task.config)?;
            // Adapters take catalog-shaped inputs; the miner now lives inline in the task.
            let miner = CatalogItem {
                id: Uuid::nil(),
                kind: CatalogKind::Miner,
                name: task.miner["name"].as_str().unwrap_or("miner").into(),
                data: task.miner.clone(),
                revision: 0,
            };
            let wallet = self
                .repository
                .catalog_item(CatalogKind::Wallet, task.wallet_id)
                .await?;
            let coin_id = wallet.data["coin_id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| Error::Validation("钱包缺少币种".into()))?;
            let coin = self
                .repository
                .catalog_item(CatalogKind::Coin, coin_id)
                .await?;
            let pool: Option<CatalogItem> = None;
            resolved.push((task.clone(), miner, wallet, coin, pool));
        }
        let customs: Vec<_> = resolved
            .iter()
            .filter(|(_, miner, _, _, _)| miner.data["adapter"] == "hive-custom")
            .collect();
        if customs.len() > 1
            && customs
                .iter()
                .any(|(_, miner, _, _, _)| miner.data["capabilities"]["instance_api_port"] != true)
        {
            return Err(Error::Validation(
                "这些自定义安装包尚未确认支持独立统计端口，不能在同一台机器同时运行多个任务".into(),
            ));
        }
        let mut hosts = serde_json::Map::new();
        for machine in machines {
            let mut instances = Vec::new();
            let mut ports = HashSet::new();
            let mut cpu_sets = HashSet::new();
            for (index, (task, miner, wallet, coin, pool)) in resolved.iter().enumerate() {
                let adapter = self
                    .adapters
                    .get(miner.data["adapter"].as_str().unwrap_or(""))
                    .ok_or_else(|| Error::Validation("适配器未注册".into()))?;
                let mut rendered =
                    adapter.render(miner, task, wallet, coin, pool.as_ref(), machine)?;
                let port = task.config["api_port"]
                    .as_u64()
                    .unwrap_or(18080 + index as u64);
                if !(1024..=65535).contains(&port) || !ports.insert(port) {
                    return Err(Error::Validation(
                        "矿工 API 端口冲突或不在 1024–65535".into(),
                    ));
                }
                if let Some(cpus) = task.config["cpus"].as_array() {
                    for cpu in cpus {
                        if let Some(c) = cpu.as_u64()
                            && !cpu_sets.insert(c)
                        {
                            return Err(Error::Validation(format!("CPU {c} 被多个任务占用")));
                        }
                    }
                }
                rendered["api_port"] = json!(port);
                rendered["instance"] = json!(task.instance);
                rendered["policy"] = machine.policy.clone();
                instances.push(rendered);
            }
            hosts.insert(machine.id.to_string(), json!({"instances":instances,"sheet_id":flight.id,"sheet_name":flight.name,"sheet_version":flight.version}));
        }
        Ok(
            json!({"schema_version":1,"flight_sheet":flight,"hosts":hosts,"catalog_snapshot":resolved.iter().map(|(_,m,w,c,p)|json!({"miner":m,"wallet":w,"coin":c,"pool":p})).collect::<Vec<_>>()}),
        )
    }
}

/// The rig downloads the package itself, so only a URL it can fetch (or, for sheets created
/// before rigs downloaded packages, a digest already cached on the rig) is accepted.
pub fn validate_miner(miner: &Value, known_adapter: impl Fn(&str) -> bool) -> Result<()> {
    let text = |key: &str| miner[key].as_str().unwrap_or("");
    let adapter = text("adapter");
    if !known_adapter(adapter) {
        return Err(Error::Validation("请选择挖矿软件".into()));
    }
    if text("name").trim().is_empty() || text("name").len() > 64 || text("version").len() > 64 {
        return Err(Error::Validation("挖矿软件名称或版本无效".into()));
    }
    let hash = text("sha256");
    if !hash.is_empty() && (hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(Error::Validation("SHA256 需为 64 位十六进制".into()));
    }
    let url = text("url");
    let fetchable = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    let legacy_cached = url.starts_with("cache:") && !hash.is_empty();
    if !fetchable && !legacy_cached {
        return Err(Error::Validation(
            "安装链接需为 http(s) 地址，矿机会自己下载".into(),
        ));
    }
    if url.starts_with("http://") && hash.is_empty() {
        return Err(Error::Validation(
            "http 安装链接必须填写 SHA256，或改用 https".into(),
        ));
    }
    if adapter != "hive-custom" && text("executable").is_empty() {
        return Err(Error::Validation("请填写安装包内的执行文件".into()));
    }
    Ok(())
}

fn validate_pool_urls(config: &Value) -> Result<()> {
    match &config["urls"] {
        Value::Null => Ok(()),
        Value::Array(urls)
            if urls.len() <= 16
                && urls.iter().all(|u| {
                    u.as_str().is_some_and(|u| {
                        !u.trim().is_empty() && u.len() <= 512 && !u.contains(char::is_whitespace)
                    })
                }) =>
        {
            let mut seen = HashSet::new();
            if urls.iter().all(|u| seen.insert(u.as_str())) {
                Ok(())
            } else {
                Err(Error::Validation("矿池地址重复".into()))
            }
        }
        _ => Err(Error::Validation(
            "矿池地址每行一个，最多 16 个，不能含空格".into(),
        )),
    }
}

impl App {
    pub async fn flights(&self) -> Result<Vec<FlightSheet>> {
        self.repository.flights().await
    }
    pub async fn delete_flight(&self, actor: &Actor, id: uuid::Uuid) -> Result<()> {
        self.repository.delete_flight(id, actor).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn known(id: &str) -> bool {
        ["xmrig", "hive-custom"].contains(&id)
    }
    #[test]
    fn miner_must_be_downloadable_by_the_rig() {
        let ok = json!({"adapter":"xmrig","name":"XMRig","version":"6.26.0","url":"https://github.com/x.tar.gz","executable":"xmrig"});
        assert!(validate_miner(&ok, known).is_ok());
        let mut m = ok.clone();
        m["adapter"] = json!("unknown");
        assert!(validate_miner(&m, known).is_err());
        let mut m = ok.clone();
        m["url"] = json!("http://mirror/x.tar.gz");
        assert!(validate_miner(&m, known).is_err());
        m["sha256"] = json!("a".repeat(64));
        assert!(validate_miner(&m, known).is_ok());
        let mut m = ok.clone();
        m["url"] = json!("cache:abc");
        assert!(validate_miner(&m, known).is_err());
        m["sha256"] = json!("b".repeat(64));
        assert!(
            validate_miner(&m, known).is_ok(),
            "legacy cached packages keep working"
        );
        let mut m = ok.clone();
        m["executable"] = json!("");
        assert!(validate_miner(&m, known).is_err());
        let custom =
            json!({"adapter":"hive-custom","name":"mypkg","url":"https://example.com/p.tar.gz"});
        assert!(validate_miner(&custom, known).is_ok());
    }
    #[test]
    fn pool_urls_are_a_short_unique_list() {
        assert!(validate_pool_urls(&json!({})).is_ok());
        assert!(
            validate_pool_urls(&json!({"urls":["stratum+tcp://a:3333","stratum+ssl://b:443"]}))
                .is_ok()
        );
        assert!(validate_pool_urls(&json!({"urls":["a b"]})).is_err());
        assert!(validate_pool_urls(&json!({"urls":["x","x"]})).is_err());
        assert!(validate_pool_urls(&json!({"urls":"x"})).is_err());
    }
}
