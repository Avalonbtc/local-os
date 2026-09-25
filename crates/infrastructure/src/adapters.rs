use rig_domain::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub fn registry() -> HashMap<String, Arc<dyn MinerAdapter>> {
    [
        Arc::new(Xmrig) as Arc<dyn MinerAdapter>,
        Arc::new(SrbMiner),
        Arc::new(CpuMiner),
        Arc::new(HiveCustom),
    ]
    .into_iter()
    .map(|a| (a.describe().id.clone(), a))
    .collect()
}
fn description(id: &str, name: &str) -> AdapterDescription {
    AdapterDescription {
        id: id.into(),
        name: name.into(),
        schema_version: 1,
        parameters: json!({"type":"object","properties":{"algorithm":{"type":"string","title":"算法"},"threads":{"type":"integer","minimum":1,"title":"线程数"},"api_port":{"type":"integer","minimum":1024,"maximum":65535,"title":"本地 API 端口"},"wallet_template":{"type":"string","default":"%WAL%.%WORKER_NAME%","title":"钱包与 Worker 模板"},"password":{"type":"string","default":"x","title":"矿池密码"},"extra_args":{"type":"array","items":{"type":"string"},"title":"额外命令参数"},"user_config":{"type":"string","title":"自定义配置"}}}),
        capabilities: json!({"schema_version":1,"hardware":["cpu"],"multi_instance":true,"stats_contract":1}),
        releases: Vec::new(),
    }
}
/// Official Linux x86_64 builds, newest first. Rigs download these directly from GitHub.
/// No SHA256 is pinned (the projects do not all publish one); integrity comes from HTTPS, and
/// every rig reports the digest it actually installed.
fn releases(adapter: &str) -> Vec<MinerRelease> {
    let release = |version: &str, url: String, executable: &str| MinerRelease {
        version: version.into(),
        url,
        executable: executable.into(),
        sha256: None,
    };
    match adapter {
        "xmrig" => ["6.26.0", "6.25.0", "6.24.0"]
            .iter()
            .map(|v| {
                release(
                    v,
                    format!("https://github.com/xmrig/xmrig/releases/download/v{v}/xmrig-{v}-linux-static-x64.tar.gz"),
                    "xmrig",
                )
            })
            .collect(),
        "srbminer" => ["3.4.6", "3.4.5", "3.4.4"]
            .iter()
            .map(|v| {
                let dashed = v.replace('.', "-");
                release(
                    v,
                    format!("https://github.com/doktor83/SRBMiner-Multi/releases/download/{v}/SRBMiner-Multi-{dashed}-Linux.tar.gz"),
                    "SRBMiner-MULTI",
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}
fn value<'a>(task: &'a FlightTask, miner: &'a CatalogItem, key: &str, default: &'a str) -> &'a str {
    task.config[key]
        .as_str()
        .or(miner.data[key].as_str())
        .unwrap_or(default)
}
fn expand(template: &str, values: &HashMap<&str, String>) -> String {
    let mut out = template.to_owned();
    let mut keys: Vec<_> = values.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for key in keys {
        out = out.replace(key, &values[key]);
    }
    out
}
struct Context {
    base: Value,
    wallet: String,
    url: String,
    password: String,
    algorithm: String,
    extra: Vec<String>,
}
fn resolve_urls(selection: &Value, directory: Option<&Value>) -> Result<Value> {
    let Some(directory) = directory else {
        return Ok(selection.clone());
    };
    let available = directory
        .as_array()
        .filter(|urls| !urls.is_empty() && urls.iter().all(Value::is_string))
        .ok_or_else(|| Error::Validation("矿池目录缺少有效的服务器地址".into()))?;
    if selection.is_null() {
        return Ok(directory.clone());
    }
    let selected = selection
        .as_array()
        .filter(|urls| !urls.is_empty() && urls.iter().all(Value::is_string))
        .ok_or_else(|| Error::Validation("至少选择一个矿池服务器".into()))?;
    let mut seen = HashSet::new();
    for url in selected {
        let url = url.as_str().unwrap();
        if !available.iter().any(|item| item.as_str() == Some(url)) || !seen.insert(url) {
            return Err(Error::Validation("矿池服务器选择无效或重复".into()));
        }
    }
    Ok(selection.clone())
}
fn context(
    miner: &CatalogItem,
    task: &FlightTask,
    wallet: &CatalogItem,
    coin: &CatalogItem,
    pool: Option<&CatalogItem>,
    machine: &Machine,
) -> Result<Context> {
    let selected_urls = if pool.is_none() && task.config["urls"].is_null() {
        &miner.data["urls"]
    } else {
        &task.config["urls"]
    };
    let urls = resolve_urls(selected_urls, pool.map(|p| &p.data["urls"]))?;
    let url = urls
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .or(task.config["url"].as_str())
        .unwrap_or("")
        .to_owned();
    if url.is_empty() && miner.data["adapter"] != "hive-custom" {
        return Err(Error::Validation("飞行表缺少矿池地址".into()));
    }
    let address = wallet.data["address"].as_str().unwrap_or("");
    let coin_symbol = coin.data["symbol"].as_str().unwrap_or("");
    let without_scheme = url.split("://").last().unwrap_or(&url);
    let (host, port) = without_scheme
        .rsplit_once(':')
        .unwrap_or((without_scheme, ""));
    let variables = HashMap::from([
        ("%WAL%", address.into()),
        ("%WORKER_NAME%", machine.name.clone()),
        ("%COIN%", coin_symbol.into()),
        ("%URL%", url.clone()),
        ("%POOL%", url.clone()),
        ("%URL_HOST%", host.into()),
        ("%URL_PORT%", port.into()),
    ]);
    let wallet = expand(
        value(task, miner, "wallet_template", "%WAL%.%WORKER_NAME%"),
        &variables,
    );
    let password = expand(value(task, miner, "password", "x"), &variables);
    let algorithm = value(task, miner, "algorithm", "").to_owned();
    if algorithm.is_empty() && miner.data["adapter"] != "hive-custom" {
        return Err(Error::Validation("请为矿工任务填写算法".into()));
    }
    let extra = task.config["extra_args"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| expand(s, &variables))
                        .ok_or_else(|| Error::Validation("额外参数必须为字符串数组".into()))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let mut base = json!({"schema_version":1,"adapter":miner.data["adapter"],"url":miner.data["url"],"sha256":miner.data["sha256"],"package_version":miner.data["version"],"coin":coin_symbol,"algorithm":algorithm,"cpus":task.config["cpus"],"warmup_seconds":task.config["warmup_seconds"].as_u64().unwrap_or(60),"verification_seconds":task.config["verification_seconds"].as_u64().unwrap_or(300),"capabilities":miner.data["capabilities"].as_object().cloned().unwrap_or_default()});
    base["pool_urls"] = urls.clone();
    base["environment"] = json!({"CUSTOM_TEMPLATE":wallet,"CUSTOM_URL":if let Some(a)=urls.as_array(){a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")}else{url.clone()},"CUSTOM_PASS":password,"CUSTOM_ALGO":algorithm,"CUSTOM_USER_CONFIG":expand(value(task,miner,"user_config",""),&variables),"CUSTOM_COIN":coin_symbol,"WORKER_NAME":machine.name,"CUSTOM_VERSION":miner.data["version"].as_str().unwrap_or("unknown")});
    Ok(Context {
        base,
        wallet,
        url,
        password,
        algorithm,
        extra,
    })
}
fn executable(miner: &CatalogItem) -> Result<String> {
    let s = miner.data["executable"]
        .as_str()
        .ok_or_else(|| Error::Validation("包内执行文件未指定".into()))?;
    if s.starts_with('/') || s.contains("..") || s.contains('\\') {
        return Err(Error::Validation("执行文件必须为包内相对路径".into()));
    }
    Ok(s.into())
}
fn append_threads(argv: &mut Vec<String>, task: &FlightTask, flag: &str) {
    if let Some(threads) = task.config["threads"].as_u64() {
        argv.extend([flag.into(), threads.to_string()]);
    }
}
pub struct Xmrig;
impl MinerAdapter for Xmrig {
    fn describe(&self) -> AdapterDescription {
        AdapterDescription {
            releases: releases("xmrig"),
            ..description("xmrig", "XMRig")
        }
    }
    fn render(
        &self,
        m: &CatalogItem,
        t: &FlightTask,
        w: &CatalogItem,
        c: &CatalogItem,
        p: Option<&CatalogItem>,
        host: &Machine,
    ) -> Result<Value> {
        let mut x = context(m, t, w, c, p, host)?;
        let mut args = vec![
            executable(m)?,
            "--algo".into(),
            x.algorithm,
            "--url".into(),
            x.url,
            "--user".into(),
            x.wallet,
            "--pass".into(),
            x.password,
            "--http-enabled".into(),
            "--http-host=127.0.0.1".into(),
            "--http-port=%API_PORT%".into(),
            "--no-color".into(),
        ];
        append_threads(&mut args, t, "--threads");
        args.extend(x.extra);
        x.base["argv"] = json!(args);
        Ok(x.base)
    }
}
pub struct SrbMiner;
impl MinerAdapter for SrbMiner {
    fn describe(&self) -> AdapterDescription {
        AdapterDescription {
            releases: releases("srbminer"),
            ..description("srbminer", "SRBMiner-MULTI")
        }
    }
    fn render(
        &self,
        m: &CatalogItem,
        t: &FlightTask,
        w: &CatalogItem,
        c: &CatalogItem,
        p: Option<&CatalogItem>,
        host: &Machine,
    ) -> Result<Value> {
        let mut x = context(m, t, w, c, p, host)?;
        let mut args = vec![
            executable(m)?,
            "--algorithm".into(),
            x.algorithm,
            "--pool".into(),
            x.url,
            "--wallet".into(),
            x.wallet,
            "--password".into(),
            x.password,
            "--api-enable".into(),
            "--api-port".into(),
            "%API_PORT%".into(),
            "--disable-gpu".into(),
        ];
        append_threads(&mut args, t, "--cpu-threads");
        args.extend(x.extra);
        x.base["argv"] = json!(args);
        Ok(x.base)
    }
}
pub struct CpuMiner;
impl MinerAdapter for CpuMiner {
    fn describe(&self) -> AdapterDescription {
        description("cpuminer-opt", "cpuminer-opt")
    }
    fn render(
        &self,
        m: &CatalogItem,
        t: &FlightTask,
        w: &CatalogItem,
        c: &CatalogItem,
        p: Option<&CatalogItem>,
        host: &Machine,
    ) -> Result<Value> {
        let mut x = context(m, t, w, c, p, host)?;
        let mut args = vec![
            executable(m)?,
            "-a".into(),
            x.algorithm,
            "-o".into(),
            x.url,
            "-u".into(),
            x.wallet,
            "-p".into(),
            x.password,
            "--api-bind=127.0.0.1:%API_PORT%".into(),
        ];
        append_threads(&mut args, t, "-t");
        args.extend(x.extra);
        x.base["argv"] = json!(args);
        Ok(x.base)
    }
}
pub struct HiveCustom;
impl MinerAdapter for HiveCustom {
    fn describe(&self) -> AdapterDescription {
        let mut d = description("hive-custom", "HiveOS 自定义包");
        d.capabilities["required_files"] =
            json!(["h-manifest.conf", "h-config.sh", "h-run.sh", "h-stats.sh"]);
        d.capabilities["helpers"] = json!(["mkfile_from_symlink", "miner_ver", "message"]);
        d
    }
    fn render(
        &self,
        m: &CatalogItem,
        t: &FlightTask,
        w: &CatalogItem,
        c: &CatalogItem,
        p: Option<&CatalogItem>,
        host: &Machine,
    ) -> Result<Value> {
        let mut x = context(m, t, w, c, p, host)?;
        x.base["custom_name"] = json!(value(t, m, "custom_name", "custom"));
        Ok(x.base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hive_custom_allows_empty_algorithm_and_embedded_pool_config() {
        let id = uuid::Uuid::nil();
        let miner = CatalogItem {
            id,
            kind: CatalogKind::Miner,
            name: "custom".into(),
            data: json!({"adapter":"hive-custom","url":"https://example.invalid/miner.tar.gz","sha256":"a".repeat(64)}),
            revision: 1,
        };
        let wallet = CatalogItem {
            id,
            kind: CatalogKind::Wallet,
            name: "wallet".into(),
            data: json!({"address":"wallet1"}),
            revision: 1,
        };
        let coin = CatalogItem {
            id,
            kind: CatalogKind::Coin,
            name: "coin".into(),
            data: json!({"symbol":"TEST"}),
            revision: 1,
        };
        let task = FlightTask {
            instance: "custom1".into(),
            wallet_id: id,
            miner: miner.data.clone(),
            config: json!({"wallet_template":"%WAL%.%WORKER_NAME%", "user_config":"pool configured here"}),
        };
        let host: Machine = serde_json::from_value(json!({"id":id,"name":"worker1","host":"127.0.0.1","port":22,"username":"test","host_key":"test","group":"","tags":[],"is_controller":false,"bmc":null,"policy":{},"created_at":"2026-01-01T00:00:00Z"})).unwrap();
        let rendered = HiveCustom
            .render(&miner, &task, &wallet, &coin, None, &host)
            .unwrap();
        assert_eq!(rendered["environment"]["CUSTOM_ALGO"], "");
        assert_eq!(
            rendered["environment"]["CUSTOM_TEMPLATE"],
            "wallet1.worker1"
        );
        assert_eq!(
            rendered["environment"]["CUSTOM_USER_CONFIG"],
            "pool configured here"
        );
        assert!(rendered["capabilities"].is_object());
    }
    #[test]
    fn replace_literal_values_without_shell_interpolation() {
        let vars = HashMap::from([
            ("%WAL%", "$(touch /oops)".into()),
            ("%WORKER_NAME%", "epyc-1".into()),
        ]);
        assert_eq!(
            expand("%WAL%.%WORKER_NAME%", &vars),
            "$(touch /oops).epyc-1"
        );
    }
    #[test]
    fn every_adapter_has_schema() {
        let r = registry();
        assert_eq!(r.len(), 4);
        for a in r.values() {
            assert_eq!(a.describe().schema_version, 1);
            assert!(a.describe().parameters["properties"].is_object());
            for release in a.describe().releases {
                assert!(release.url.starts_with("https://github.com/"));
                assert!(release.url.contains(&release.version));
                assert!(!release.executable.contains('/'));
            }
        }
        assert!(!r["xmrig"].describe().releases.is_empty());
    }
    #[test]
    fn selected_pool_servers_keep_order_and_must_belong_to_directory() {
        let directory = json!(["stratum+tcp://a:3333", "stratum+tcp://b:3333"]);
        assert_eq!(
            resolve_urls(
                &json!(["stratum+tcp://b:3333", "stratum+tcp://a:3333"]),
                Some(&directory)
            )
            .unwrap(),
            json!(["stratum+tcp://b:3333", "stratum+tcp://a:3333"])
        );
        assert!(resolve_urls(&json!(["stratum+tcp://other:3333"]), Some(&directory)).is_err());
        assert!(
            resolve_urls(
                &json!(["stratum+tcp://a:3333", "stratum+tcp://a:3333"]),
                Some(&directory)
            )
            .is_err()
        );
        assert!(resolve_urls(&json!([]), Some(&directory)).is_err());
        assert_eq!(
            resolve_urls(&Value::Null, Some(&directory)).unwrap(),
            directory
        );
    }
}
