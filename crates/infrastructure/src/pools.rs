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
        upstream_down: false,
        exact_names: false,
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
        upstream_down: false,
        exact_names: false,
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
        upstream_down: false,
        exact_names: false,
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
        upstream_down: false,
        exact_names: false,
    })
}

/// xmrig-proxy `GET /1/summary` and `GET /1/workers`. Each worker is
/// `[name, ip, connections, accepted, rejected, invalid, hashes, last share (ms), hashrate 1 m,
/// 10 m, 1 h, 12 h, 24 h]` with hashrates in kH/s. The name follows the proxy's `workers` mode:
/// rig-id (falling back to the miner's login), login, password, user agent or IP.
pub fn parse_xmrig_proxy(summary: &Value, workers: &Value) -> Result<PoolWorkers> {
    let list = workers["workers"]
        .as_array()
        .ok_or_else(|| invalid("xmrig-proxy"))?;
    let khs = |v: &Value| rate(v) * 1000.0;
    Ok(PoolWorkers {
        workers: list
            .iter()
            .filter_map(Value::as_array)
            .map(|w| PoolWorker {
                name: w
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                hashrate_hs: w.get(8).map_or(0.0, khs),
                last_share_at: w.get(7).and_then(time),
            })
            .collect(),
        total_hs: summary["hashrate"]["total"]
            .get(0)
            .or(workers["hashrate"]["total"].get(0))
            .map(khs),
        upstream_down: summary["upstreams"]["active"].as_u64() == Some(0),
        // rig_id falls back to the login for miners without --rig-id (RigDeck sets none).
        exact_names: matches!(workers["mode"].as_str(), Some("rig_id" | "user")),
    })
}

impl PublicPools {
    async fn proxy_get(&self, url: &str, token: Option<&str>) -> Result<Value> {
        let mut request = self.client.get(url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("xmrig-proxy 接口不可达：{e}")))?;
        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(Error::Unavailable(format!(
                "xmrig-proxy 拒绝访问（HTTP {status}），检查访问令牌"
            )));
        }
        if !status.is_success() {
            return Err(Error::Unavailable(format!(
                "xmrig-proxy 接口返回 HTTP {status}"
            )));
        }
        response
            .json()
            .await
            .map_err(|e| Error::Unavailable(format!("xmrig-proxy 接口返回无效 JSON：{e}")))
    }
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
    async fn proxy_workers(&self, url: &str, token: Option<&str>) -> Result<PoolWorkers> {
        let base = url.trim().trim_end_matches('/');
        let summary = self.proxy_get(&format!("{base}/1/summary"), token).await?;
        let workers = self.proxy_get(&format!("{base}/1/workers"), token).await?;
        parse_xmrig_proxy(&summary, &workers)
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

    // Shaped after xmrig-proxy's ApiRouter (getWorkers / getMinersSummary): hashrates in kH/s,
    // last share in milliseconds, rigs without --rig-id listed under their full login.
    #[test]
    fn xmrig_proxy_lists_rigs_with_hashrate_last_share_and_upstream_state() {
        let workers = json!({
            "hashrate": {"total": [15.2, 15.0, 14.8, 14.9, 14.9, 14.9]},
            "mode": "rig_id",
            "workers": [
                [format!("{WALLET}.rig01"), "192.168.1.21", 1, 812, 0, 0, 98_000_000_i64, 1792646795928_i64, 7.61, 7.5, 7.4, 7.4, 7.4, 7.4],
                [format!("{WALLET}.rig02"), "192.168.1.22", 0, 300, 1, 0, 40_000_000_i64, 1792640000000_i64, 0.0, 0.0, 2.1, 6.9, 7.0, 7.0]
            ]
        });
        let summary = json!({"hashrate": {"total": [15.2, 15.0]}, "upstreams": {"active": 1, "sleep": 0, "error": 0, "total": 1}});
        let parsed = parse_xmrig_proxy(&summary, &workers).unwrap();
        assert_eq!(parsed.workers.len(), 2);
        assert_eq!(parsed.workers[0].name, format!("{WALLET}.rig01"));
        assert!((parsed.workers[0].hashrate_hs - 7610.0).abs() < 1e-6);
        assert_eq!(
            parsed.workers[0].last_share_at.unwrap().timestamp_millis(),
            1792646795928
        );
        assert_eq!(parsed.workers[1].hashrate_hs, 0.0);
        assert!((parsed.total_hs.unwrap() - 15200.0).abs() < 1e-6);
        assert!(!parsed.upstream_down);
        assert!(parsed.exact_names);

        // The proxy lost its pool: rigs still connect, but nothing reaches the pool.
        let down = json!({"hashrate": {"total": [0.0]}, "upstreams": {"active": 0, "sleep": 0, "error": 1, "total": 1}});
        assert!(parse_xmrig_proxy(&down, &workers).unwrap().upstream_down);
        // Per-worker statistics disabled (`"workers": false`): an empty list, not an error.
        let none = json!({"hashrate": {"total": [15.2]}, "mode": "none", "workers": []});
        let none = parse_xmrig_proxy(&summary, &none).unwrap();
        assert!(none.workers.is_empty() && !none.exact_names);
        // Named by password (often just `x`) or user agent: names cannot be trusted.
        let by_password = json!({"hashrate": {"total": [15.2]}, "mode": "password", "workers": [["x", "192.168.1.21", 2, 1, 0, 0, 1, 0, 1.0, 1.0, 1.0, 1.0, 1.0]]});
        assert!(
            !parse_xmrig_proxy(&summary, &by_password)
                .unwrap()
                .exact_names
        );
        assert!(parse_xmrig_proxy(&summary, &json!({"error": "x"})).is_err());
    }

    /// A local stand-in for xmrig-proxy's HTTP API that only answers the right bearer token.
    async fn fake_proxy(token: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buffer = vec![0; 4096];
                let n = socket.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                let authorized = request
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case(&format!("authorization: Bearer {token}")));
                let body = if request.starts_with("GET /1/summary ") {
                    json!({"upstreams": {"active": 1, "total": 1}, "hashrate": {"total": [7.6]}})
                } else {
                    json!({"mode": "rig_id", "hashrate": {"total": [7.6]}, "workers": [
                        [format!("{WALLET}.rig01"), "192.168.1.21", 1, 5, 0, 0, 1, 1792646795928_i64, 7.6, 7.6, 7.6, 7.6, 7.6]
                    ]})
                };
                let (status, body) = if authorized {
                    ("200 OK", body.to_string())
                } else {
                    ("401 Unauthorized", "{}".to_owned())
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        format!("http://{address}/")
    }

    #[tokio::test]
    async fn xmrig_proxy_is_asked_with_its_access_token() {
        let url = fake_proxy("proxy-secret").await;
        let pools = PublicPools::default();
        let workers = pools
            .proxy_workers(&url, Some("proxy-secret"))
            .await
            .unwrap();
        assert_eq!(workers.workers[0].name, format!("{WALLET}.rig01"));
        assert!((workers.workers[0].hashrate_hs - 7600.0).abs() < 1e-6);
        assert!(workers.exact_names && !workers.upstream_down);
        for token in [Some("wrong"), None] {
            let error = pools.proxy_workers(&url, token).await.unwrap_err();
            assert!(error.to_string().contains("检查访问令牌"), "{error}");
        }
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
