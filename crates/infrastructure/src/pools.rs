//! Public miner APIs of the pools RigDeck can ask whether a worker is hashing. Anything else
//! (own proxies, bare IP:port, unknown hosts) has no account and is shown as unsupported.
use chrono::{DateTime, Utc};
use rig_domain::*;
use serde_json::Value;
use std::time::Duration;

pub struct PublicPools {
    client: reqwest::Client,
}
impl Default for PublicPools {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("RigDeck/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("HTTP client"),
        }
    }
}

/// Pool host of a stratum URL (`stratum+ssl://host:port`, `host:port` or `host`), lowercased.
fn host(url: &str) -> String {
    let rest = url.trim().split("://").last().unwrap_or("");
    let rest = rest.split(['/', '?']).next().unwrap_or("");
    let rest = rest.rsplit('@').next().unwrap_or("");
    let host = match rest.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => rest,
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}
fn under(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}
/// Wallet part of a stratum login: `WALLET.worker`, `WALLET/worker`, `WALLET+diff`.
fn wallet(login: &str) -> Option<String> {
    let wallet = login.trim().split(['.', '/', '+', ':']).next()?;
    (20..=128)
        .contains(&wallet.len())
        .then_some(wallet)
        .filter(|w| w.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(str::to_owned)
}
fn provider(host: &str, coin: &str) -> Option<(&'static str, String)> {
    let coin = coin.trim().to_ascii_lowercase();
    if coin.is_empty() || !coin.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    if under(host, "supportxmr.com") {
        return (coin == "xmr").then_some(("supportxmr", coin));
    }
    if under(host, "hashvault.pro") {
        // HashVault names coins in its API paths; only Monero is mapped.
        return (coin == "xmr").then(|| ("hashvault", "monero".to_owned()));
    }
    if under(host, "kryptex.network") || under(host, "kryptex.com") {
        return Some(("kryptex", coin));
    }
    if under(host, "nanopool.org") {
        return Some(("nanopool", coin));
    }
    None
}
pub fn account(url: &str, coin: &str, login: &str) -> Option<PoolAccount> {
    let (provider, coin) = provider(&host(url), coin)?;
    let wallet = wallet(login)?;
    // Kryptex `krx…` usernames are account names, not wallets; the worker API takes wallets.
    if provider == "kryptex" && wallet.to_ascii_lowercase().starts_with("krx") {
        return None;
    }
    Some(PoolAccount {
        provider: provider.into(),
        coin,
        wallet,
    })
}

/// Unix time in seconds or milliseconds; 0 and absent mean "never".
fn time(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
    if raw <= 0.0 {
        return None;
    }
    let millis = if raw > 1e11 { raw } else { raw * 1000.0 };
    DateTime::from_timestamp_millis(millis as i64)
}
fn rate(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|r| r.is_finite() && *r >= 0.0)
        .unwrap_or(0.0)
}
fn invalid(provider: &str) -> Error {
    Error::Unavailable(format!("{provider} 矿池接口返回格式无法识别"))
}

/// `GET pool.kryptex.com/{coin}/api/v3/miner/workers/{wallet}`.
pub fn parse_kryptex(body: &Value) -> Result<PoolWorkers> {
    let results = body["results"]
        .as_array()
        .ok_or_else(|| invalid("Kryptex"))?;
    let workers: Vec<PoolWorker> = results
        .iter()
        .map(|w| PoolWorker {
            name: w["worker"].as_str().unwrap_or_default().to_owned(),
            // The 30-minute average lingers after a miner stops; trust the pool's own status.
            hashrate_hs: if w["status"] == "online" {
                rate(&w["avg_hashrate_30m"])
            } else {
                0.0
            },
            last_share_at: time(&w["last_share"]),
        })
        .collect();
    Ok(PoolWorkers {
        total_hs: Some(workers.iter().map(|w| w.hashrate_hs).sum()),
        workers,
    })
}
/// `GET www.supportxmr.com/api/miner/{wallet}/stats/allWorkers`: `global` plus one object per
/// worker (`hash`, `lts` = last share in seconds).
pub fn parse_supportxmr(body: &Value) -> Result<PoolWorkers> {
    let object = body.as_object().ok_or_else(|| invalid("SupportXMR"))?;
    Ok(PoolWorkers {
        total_hs: object.get("global").map(|g| rate(&g["hash"])),
        workers: object
            .iter()
            .filter(|(name, _)| *name != "global")
            .map(|(name, w)| PoolWorker {
                name: name.clone(),
                hashrate_hs: rate(&w["hash"]),
                last_share_at: time(&w["lts"]),
            })
            .collect(),
    })
}
/// `GET api.nanopool.org/v1/{coin}/workers/{wallet}`.
pub fn parse_nanopool(body: &Value) -> Result<PoolWorkers> {
    if body["status"] != true {
        // Nanopool answers unknown accounts and its own failures alike; neither is evidence.
        let error = body["error"].as_str().unwrap_or("未知错误");
        return Err(Error::Unavailable(format!(
            "Nanopool 接口返回错误：{error}"
        )));
    }
    let data = body["data"].as_array().ok_or_else(|| invalid("Nanopool"))?;
    let workers: Vec<PoolWorker> = data
        .iter()
        .map(|w| PoolWorker {
            name: w["id"].as_str().unwrap_or_default().to_owned(),
            hashrate_hs: rate(&w["hashrate"]),
            last_share_at: time(&w["lastShare"]),
        })
        .collect();
    Ok(PoolWorkers {
        total_hs: Some(workers.iter().map(|w| w.hashrate_hs).sum()),
        workers,
    })
}
/// `GET api.hashvault.pro/v3/{coin}/wallet/{wallet}/stats?workers=true`: `collective` totals and
/// `collectiveWorkers` (null when the wallet has no active worker).
pub fn parse_hashvault(body: &Value) -> Result<PoolWorkers> {
    let collective = body
        .get("collective")
        .filter(|c| c.is_object())
        .ok_or_else(|| invalid("HashVault"))?;
    let worker = |name: Option<&str>, w: &Value| PoolWorker {
        name: w["name"].as_str().or(name).unwrap_or_default().to_owned(),
        hashrate_hs: rate(&w["hashRate"]),
        last_share_at: time(&w["lastShare"]),
    };
    let workers = match &body["collectiveWorkers"] {
        Value::Array(items) => items.iter().map(|w| worker(None, w)).collect(),
        Value::Object(items) => items.iter().map(|(n, w)| worker(Some(n), w)).collect(),
        _ => Vec::new(),
    };
    Ok(PoolWorkers {
        total_hs: Some(rate(&collective["hashRate"])),
        workers,
    })
}

#[async_trait::async_trait]
impl PoolStats for PublicPools {
    fn account(&self, url: &str, coin: &str, login: &str) -> Option<PoolAccount> {
        account(url, coin, login)
    }
    async fn workers(&self, account: &PoolAccount) -> Result<PoolWorkers> {
        let (coin, wallet) = (&account.coin, &account.wallet);
        let (url, parse): (String, fn(&Value) -> Result<PoolWorkers>) = match account
            .provider
            .as_str()
        {
            "kryptex" => (
                format!("https://pool.kryptex.com/{coin}/api/v3/miner/workers/{wallet}"),
                parse_kryptex,
            ),
            "supportxmr" => (
                format!("https://www.supportxmr.com/api/miner/{wallet}/stats/allWorkers"),
                parse_supportxmr,
            ),
            "nanopool" => (
                format!("https://api.nanopool.org/v1/{coin}/workers/{wallet}"),
                parse_nanopool,
            ),
            "hashvault" => (
                format!(
                    "https://api.hashvault.pro/v3/{coin}/wallet/{wallet}/stats?poolType=false&workers=true"
                ),
                parse_hashvault,
            ),
            _ => return Err(Error::Validation("不支持的矿池".into())),
        };
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("矿池接口不可达：{e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Unavailable(format!("矿池接口返回 HTTP {status}")));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| Error::Unavailable(format!("矿池接口返回无效 JSON：{e}")))?;
        parse(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WALLET: &str = "44AFFq5kSiGBoZ4NMDwYtN18obc8AemS33DBLWs3H7otXft3XjrpDtQGv7SqSsaBYBb98uNbr2VBBEt7f2wfn3RVGQBEP3A";

    #[test]
    fn supported_pools_are_recognised_by_host_and_coin() {
        let login = format!("{WALLET}.rig01");
        for (url, coin, provider, api_coin) in [
            (
                "stratum+ssl://pool.supportxmr.com:443",
                "XMR",
                "supportxmr",
                "xmr",
            ),
            ("pool.supportxmr.com:3333", "xmr", "supportxmr", "xmr"),
            (
                "stratum+tcp://xmr.kryptex.network:7029",
                "XMR",
                "kryptex",
                "xmr",
            ),
            ("xmr-eu1.nanopool.org:10343", "XMR", "nanopool", "xmr"),
            (
                "stratum+ssl://pool.hashvault.pro:443",
                "XMR",
                "hashvault",
                "monero",
            ),
        ] {
            assert_eq!(
                account(url, coin, &login),
                Some(PoolAccount {
                    provider: provider.into(),
                    coin: api_coin.into(),
                    wallet: WALLET.into()
                }),
                "{url}"
            );
        }
    }

    #[test]
    fn own_proxies_and_unknown_pools_are_unsupported() {
        let login = format!("{WALLET}.rig01");
        for (url, coin) in [
            ("mac.icuicu.de:6666", "XMR"),
            ("stratum+tcp://149.104.4.41:8888", "XMR"),
            ("stratum+tcp://supportxmr.com.evil.example:3333", "XMR"),
            ("notsupportxmr.com:3333", "XMR"),
            // SupportXMR and HashVault are only wired for Monero.
            ("pool.supportxmr.com:3333", "RVN"),
            ("pool.hashvault.pro:443", "ETC"),
            ("", "XMR"),
        ] {
            assert_eq!(account(url, coin, &login), None, "{url}");
        }
        // Logins that are not a wallet cannot be looked up either.
        let pool = "xmr.kryptex.network:7029";
        assert_eq!(account(pool, "XMR", "krx1abcdefghijklmnopqrstuv.rig"), None);
        assert_eq!(account(pool, "XMR", "short.rig"), None);
        assert_eq!(
            account(pool, "XMR", "$(touch /x)aaaaaaaaaaaaaaaaaaaaaaaa"),
            None
        );
    }

    #[test]
    fn wallet_is_taken_before_worker_and_difficulty_suffixes() {
        for login in [
            format!("{WALLET}.rig01"),
            format!("{WALLET}/rig01"),
            format!("{WALLET}+50000"),
            WALLET.to_owned(),
        ] {
            assert_eq!(wallet(&login).as_deref(), Some(WALLET));
        }
    }

    // Response bodies below are trimmed copies of what each pool returned on 2026-10-22.
    #[test]
    fn kryptex_offline_status_overrides_a_lingering_average() {
        let body = json!({"results":[
            {"miner":WALLET,"worker":"w5090ref","status":"offline","last_share":1792632285194_i64,"avg_hashrate_24h":"187.13","avg_hashrate_30m":"12.5"},
            {"miner":WALLET,"worker":"rig01","status":"online","last_share":1792632560489_i64,"avg_hashrate_30m":"5230.4"}
        ]});
        let parsed = parse_kryptex(&body).unwrap();
        assert_eq!(parsed.workers.len(), 2);
        assert_eq!(parsed.workers[0].hashrate_hs, 0.0);
        assert_eq!(
            parsed.workers[0].last_share_at.unwrap().timestamp_millis(),
            1792632285194
        );
        assert_eq!(parsed.workers[1].hashrate_hs, 5230.4);
        assert_eq!(parsed.total_hs, Some(5230.4));
        assert!(parse_kryptex(&json!({"error":"x"})).is_err());
    }

    #[test]
    fn supportxmr_workers_are_keys_beside_global() {
        let body = json!({
            "global":{"lts":1792638317,"identifer":"global","hash":7400,"totalHash":83088048155_i64},
            "rig01":{"lts":1792638310,"identifer":"rig01","hash":7400,"totalHash":1000},
            "rig02":{"lts":1792600000,"identifer":"rig02","hash":0,"totalHash":1000}
        });
        let parsed = parse_supportxmr(&body).unwrap();
        assert_eq!(parsed.total_hs, Some(7400.0));
        let rig02 = parsed.workers.iter().find(|w| w.name == "rig02").unwrap();
        assert_eq!(rig02.hashrate_hs, 0.0);
        assert_eq!(rig02.last_share_at.unwrap().timestamp(), 1792600000);
        // A wallet the pool has never seen: only `global`, no workers.
        let empty = parse_supportxmr(&json!({"global":{"lts":0,"hash":0}})).unwrap();
        assert!(empty.workers.is_empty());
        assert_eq!(empty.total_hs, Some(0.0));
    }

    #[test]
    fn nanopool_errors_are_not_an_empty_worker_list() {
        let body = json!({"status":true,"data":[{"uid":22568176,"id":"Rig01","hashrate":0,"lastShare":1792482782,"rating":208}]});
        let parsed = parse_nanopool(&body).unwrap();
        assert_eq!(parsed.workers[0].name, "Rig01");
        assert_eq!(
            parsed.workers[0].last_share_at.unwrap().timestamp(),
            1792482782
        );
        assert!(parse_nanopool(&json!({"status":false,"error":"Error"})).is_err());
    }

    #[test]
    fn hashvault_reads_collective_workers_in_either_shape() {
        let stats = json!({"collective":{"hashRate":9100,"lastShare":1792573020010_i64},"solo":{},"collectiveWorkers":null});
        let parsed = parse_hashvault(&stats).unwrap();
        assert!(parsed.workers.is_empty());
        assert_eq!(parsed.total_hs, Some(9100.0));
        let list = json!({"collective":{"hashRate":9100},"collectiveWorkers":[{"name":"rig01","hashRate":9100,"lastShare":1792573020010_i64}]});
        let parsed = parse_hashvault(&list).unwrap();
        assert_eq!(parsed.workers[0].name, "rig01");
        assert_eq!(
            parsed.workers[0].last_share_at.unwrap().timestamp_millis(),
            1792573020010
        );
        let map = json!({"collective":{"hashRate":0},"collectiveWorkers":{"rig02":{"hashRate":0}}});
        assert_eq!(parse_hashvault(&map).unwrap().workers[0].name, "rig02");
        assert!(parse_hashvault(&json!({"error":"bad wallet"})).is_err());
    }

    /// Against the real pools: `cargo test -p rig-infrastructure pools -- --ignored`.
    #[tokio::test]
    #[ignore = "calls the public pool APIs over the internet"]
    async fn live_pool_apis_parse() {
        let pools = PublicPools::default();
        for url in [
            "pool.supportxmr.com:443",
            "xmr.kryptex.network:7029",
            "xmr-eu1.nanopool.org:10343",
            "pool.hashvault.pro:443",
        ] {
            let account = pools.account(url, "XMR", WALLET).unwrap();
            let workers = pools.workers(&account).await;
            println!("{url}: {workers:?}");
            assert!(workers.is_ok(), "{url}: {workers:?}");
        }
    }
}
