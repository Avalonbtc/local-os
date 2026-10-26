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
/// One pool of a task, in flight-sheet order (the first is the main pool, the rest are backups),
/// with the wallet template and password expanded for that pool's address.
struct PoolLogin {
    url: String,
    wallet: String,
    password: String,
}
struct Context {
    base: Value,
    wallet: String,
    url: String,
    password: String,
    /// Every pool of the task; `url` / `wallet` / `password` are the first one's.
    pools: Vec<PoolLogin>,
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
    let variables_for = |url: &str| {
        let without_scheme = url.split("://").last().unwrap_or(url);
        let (host, port) = without_scheme
            .rsplit_once(':')
            .unwrap_or((without_scheme, ""));
        HashMap::from([
            ("%WAL%", address.to_string()),
            ("%WORKER_NAME%", machine.name.clone()),
            ("%COIN%", coin_symbol.to_string()),
            ("%URL%", url.to_string()),
            ("%POOL%", url.to_string()),
            ("%URL_HOST%", host.to_string()),
            ("%URL_PORT%", port.to_string()),
        ])
    };
    let login = |url: &str| {
        let variables = variables_for(url);
        PoolLogin {
            url: url.to_string(),
            wallet: expand(
                value(task, miner, "wallet_template", "%WAL%.%WORKER_NAME%"),
                &variables,
            ),
            password: expand(value(task, miner, "password", "x"), &variables),
        }
    };
    let mut pools: Vec<PoolLogin> = urls
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(&login).collect())
        .unwrap_or_default();
    if pools.is_empty() {
        pools.push(login(&url));
    }
    let variables = variables_for(&url);
    let wallet = pools[0].wallet.clone();
    let password = pools[0].password.clone();
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
    // The rig fails start-up verification when the miner reports another version, so only a
    // built-in release pins one. For a mirror or custom package the version is a display label
    // ("custom" by default), which would never match.
    let builtin = releases(miner.data["adapter"].as_str().unwrap_or(""))
        .iter()
        .any(|r| {
            miner.data["version"].as_str() == Some(&r.version)
                && miner.data["url"].as_str() == Some(&r.url)
        });
    let pinned_version = if builtin {
        miner.data["version"].clone()
    } else {
        Value::Null
    };
    let mut base = json!({"schema_version":1,"adapter":miner.data["adapter"],"url":miner.data["url"],"sha256":miner.data["sha256"],"package_version":pinned_version,"coin":coin_symbol,"algorithm":algorithm,"cpus":task.config["cpus"],"warmup_seconds":task.config["warmup_seconds"].as_u64().unwrap_or(60),"verification_seconds":task.config["verification_seconds"].as_u64().unwrap_or(300),"capabilities":miner.data["capabilities"].as_object().cloned().unwrap_or_default()});
    base["pool_urls"] = urls.clone();
    base["environment"] = json!({"CUSTOM_TEMPLATE":wallet,"CUSTOM_URL":if let Some(a)=urls.as_array(){a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")}else{url.clone()},"CUSTOM_PASS":password,"CUSTOM_ALGO":algorithm,"CUSTOM_USER_CONFIG":expand(value(task,miner,"user_config",""),&variables),"CUSTOM_COIN":coin_symbol,"WORKER_NAME":machine.name,"CUSTOM_VERSION":miner.data["version"].as_str().unwrap_or("unknown")});
    Ok(Context {
        base,
        wallet,
        url,
        password,
        pools,
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
        let mut args = vec![executable(m)?, "--algo".into(), x.algorithm];
        // Each --url starts a new pool and --user / --pass apply to it: XMRig fails over to the
        // next pool after its retries and returns to the first one once it is back.
        for pool in &x.pools {
            args.extend([
                "--url".into(),
                pool.url.clone(),
                "--user".into(),
                pool.wallet.clone(),
                "--pass".into(),
                pool.password.clone(),
            ]);
        }
        args.extend([
            "--http-enabled".into(),
            "--http-host=127.0.0.1".into(),
            "--http-port=%API_PORT%".into(),
            "--no-color".into(),
        ]);
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
        // One algorithm with failover pools: pools and wallets are separated with ',', passwords
        // with '!' (';' separates algorithms; '#!' and '#;' escape them in a password).
        let (pool, wallet, password) = if x.pools.len() > 1 {
            (
                x.pools
                    .iter()
                    .map(|p| p.url.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                x.pools
                    .iter()
                    .map(|p| p.wallet.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                x.pools
                    .iter()
                    .map(|p| p.password.replace(';', "#;").replace('!', "#!"))
                    .collect::<Vec<_>>()
                    .join("!"),
            )
        } else {
            (x.url, x.wallet, x.password)
        };
        let mut args = vec![
            executable(m)?,
            "--algorithm".into(),
            x.algorithm,
            "--pool".into(),
            pool,
            "--wallet".into(),
            wallet,
            "--password".into(),
            password,
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
        let executable = executable(m)?;
        let argv = |pool: &PoolLogin| {
            let mut args = vec![
                executable.clone(),
                "-a".into(),
                x.algorithm.clone(),
                "-o".into(),
                pool.url.clone(),
                "-u".into(),
                pool.wallet.clone(),
                "-p".into(),
                pool.password.clone(),
                "--api-bind=127.0.0.1:%API_PORT%".into(),
            ];
            append_threads(&mut args, t, "-t");
            args.extend(x.extra.iter().cloned());
            args
        };
        // cpuminer-opt takes a single -o and never switches pools, so the rig runtime does:
        // `failover_argv` holds the command line for each pool, `argv` stays the main pool's
        // (a runtime without failover support keeps mining on it, as before).
        x.base["argv"] = json!(argv(&x.pools[0]));
        if x.pools.len() > 1 {
            x.base["failover_argv"] = json!(x.pools.iter().map(argv).collect::<Vec<_>>());
        }
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
        assert!(rendered["package_version"].is_null());
    }
    #[test]
    fn only_builtin_releases_pin_the_version_to_verify() {
        let id = uuid::Uuid::nil();
        let item = |kind, data| CatalogItem {
            id,
            kind,
            name: "x".into(),
            data,
            revision: 1,
        };
        let wallet = item(CatalogKind::Wallet, json!({"address":"wallet1"}));
        let coin = item(CatalogKind::Coin, json!({"symbol":"XMR"}));
        let host: Machine = serde_json::from_value(json!({"id":id,"name":"worker1","host":"127.0.0.1","port":22,"username":"test","host_key":"test","group":"","tags":[],"is_controller":false,"bmc":null,"policy":{},"created_at":"2026-01-01T00:00:00Z"})).unwrap();
        let render = |data: Value| {
            let miner = item(CatalogKind::Miner, data);
            let task = FlightTask {
                instance: "cpu-1".into(),
                wallet_id: id,
                miner: miner.data.clone(),
                config: json!({"algorithm":"rx/0","urls":["149.104.4.41:8888"]}),
            };
            Xmrig
                .render(&miner, &task, &wallet, &coin, None, &host)
                .unwrap()
        };
        let release = &releases("xmrig")[0];
        let builtin = render(
            json!({"adapter":"xmrig","version":release.version,"url":release.url,"executable":"xmrig"}),
        );
        assert_eq!(builtin["package_version"], json!(release.version));
        let mirror = render(
            json!({"adapter":"xmrig","version":"custom","url":"https://gitee.com/a/b/xmrig-zero-6.26.0.tar.gz","executable":"xmrig"}),
        );
        assert!(mirror["package_version"].is_null());
        assert_eq!(mirror["environment"]["CUSTOM_VERSION"], "custom");
    }
    fn render_with_pools(adapter: &dyn MinerAdapter, data: Value) -> Value {
        let id = uuid::Uuid::nil();
        let item = |kind, data| CatalogItem {
            id,
            kind,
            name: "x".into(),
            data,
            revision: 1,
        };
        let wallet = item(CatalogKind::Wallet, json!({"address":"wallet1"}));
        let coin = item(CatalogKind::Coin, json!({"symbol":"XMR"}));
        let host: Machine = serde_json::from_value(json!({"id":id,"name":"worker1","host":"127.0.0.1","port":22,"username":"test","host_key":"test","group":"","tags":[],"is_controller":false,"bmc":null,"policy":{},"created_at":"2026-01-01T00:00:00Z"})).unwrap();
        let miner = item(CatalogKind::Miner, data);
        let task = FlightTask {
            instance: "cpu-1".into(),
            wallet_id: id,
            miner: miner.data.clone(),
            config: json!({"algorithm":"rx/0","password":"x;d=%URL_PORT%!","urls":["stratum+tcp://main:3333","stratum+tcp://backup:4444"]}),
        };
        adapter
            .render(&miner, &task, &wallet, &coin, None, &host)
            .unwrap()
    }
    fn args(rendered: &Value) -> Vec<&str> {
        rendered["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect()
    }
    #[test]
    fn xmrig_gets_every_pool_in_order() {
        let r = render_with_pools(
            &Xmrig,
            json!({"adapter":"xmrig","version":"custom","url":"https://x/x.tar.gz","executable":"xmrig"}),
        );
        let argv = args(&r);
        let urls: Vec<_> = argv
            .windows(6)
            .filter(|w| w[0] == "--url")
            .map(|w| (w[1], w[3], w[5]))
            .collect();
        assert_eq!(
            urls,
            [
                ("stratum+tcp://main:3333", "wallet1.worker1", "x;d=3333!"),
                ("stratum+tcp://backup:4444", "wallet1.worker1", "x;d=4444!"),
            ]
        );
        assert!(r["failover_argv"].is_null(), "XMRig fails over by itself");
    }
    #[test]
    fn srbminer_gets_failover_pools_in_its_separators() {
        let r = render_with_pools(
            &SrbMiner,
            json!({"adapter":"srbminer","version":"custom","url":"https://x/s.tar.gz","executable":"SRBMiner-MULTI"}),
        );
        let argv = args(&r);
        let after = |flag: &str| argv[argv.iter().position(|a| *a == flag).unwrap() + 1];
        assert_eq!(
            after("--pool"),
            "stratum+tcp://main:3333,stratum+tcp://backup:4444"
        );
        assert_eq!(after("--wallet"), "wallet1.worker1,wallet1.worker1");
        assert_eq!(after("--password"), "x#;d=3333#!!x#;d=4444#!");
    }
    #[test]
    fn cpuminer_keeps_the_main_pool_and_lists_one_command_per_pool() {
        let r = render_with_pools(
            &CpuMiner,
            json!({"adapter":"cpuminer-opt","version":"custom","url":"https://x/c.tar.gz","executable":"cpuminer"}),
        );
        let argv = args(&r);
        assert_eq!(
            argv[argv.iter().position(|a| *a == "-o").unwrap() + 1],
            "stratum+tcp://main:3333"
        );
        let failover = r["failover_argv"].as_array().unwrap();
        assert_eq!(failover.len(), 2);
        assert_eq!(failover[0], r["argv"]);
        assert_eq!(failover[1][4], "stratum+tcp://backup:4444");
        assert_eq!(failover[1][8], "x;d=4444!");
        assert_eq!(
            r["pool_urls"],
            json!(["stratum+tcp://main:3333", "stratum+tcp://backup:4444"])
        );
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
