use chrono::{Duration, Utc};
use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{
    adapters, bmc, pg::PgStore, pools, runtime::ScreenRuntime, ssh::SshExecutor, vault::Vault,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

const WALLET: &str = "44AFFq5kSiGBoZ4NMDwYtN18obc8AemS33DBLWs3H7otXft3XjrpDtQGv7SqSsaBYBb98uNbr2VBBEt7f2wfn3RVGQBEP3A";
/// A wallet whose rigs reach the pool through xmrig-proxy: one aggregated worker, named after
/// the proxy's own login.
const PROXIED: &str =
    "48ProxiedWa11etProxiedWa11etProxiedWa11etProxiedWa11etProxiedWa11etProxiedWa11etProxied";

/// Real pool recognition, canned answers: nothing for WALLET, one proxy worker for PROXIED.
#[derive(Default)]
struct EmptyPool {
    asked: Mutex<Vec<PoolAccount>>,
    /// (API URL, token) of each xmrig-proxy request.
    proxied: Mutex<Vec<(String, Option<String>)>>,
}
#[async_trait::async_trait]
impl PoolStats for EmptyPool {
    fn account(&self, url: &str, coin: &str, login: &str) -> Option<PoolAccount> {
        pools::account(url, coin, login)
    }
    async fn workers(&self, account: &PoolAccount) -> Result<PoolWorkers> {
        self.asked.lock().unwrap().push(account.clone());
        if account.wallet == PROXIED {
            return Ok(PoolWorkers {
                workers: vec![PoolWorker {
                    name: "proxy".into(),
                    hashrate_hs: 52000.0,
                    last_share_at: Some(Utc::now()),
                }],
                total_hs: Some(52000.0),
                upstream_down: false,
                exact_names: false,
            });
        }
        Ok(PoolWorkers {
            workers: vec![],
            total_hs: Some(0.0),
            upstream_down: false,
            exact_names: false,
        })
    }
    /// The user's own proxy: lists rig08 (hashing) by full login, but not rig07.
    async fn proxy_workers(&self, url: &str, token: Option<&str>) -> Result<PoolWorkers> {
        self.proxied
            .lock()
            .unwrap()
            .push((url.into(), token.map(str::to_owned)));
        Ok(PoolWorkers {
            workers: vec![PoolWorker {
                name: format!("{WALLET}.rig08"),
                hashrate_hs: 7600.0,
                last_share_at: Some(Utc::now()),
            }],
            total_hs: Some(7600.0),
            upstream_down: false,
            // Default `rig_id` mode: every rig listed by its full login.
            exact_names: true,
        })
    }
}

fn status(machine: Uuid, local: LocalState, pool: PoolState) -> MachineStatus {
    let at = Utc::now() - Duration::minutes(6);
    MachineStatus {
        machine_id: machine,
        state: if local == LocalState::Offline {
            MachineState::LocalOffline
        } else {
            MachineState::Online
        },
        local,
        last_seen_at: Some(at),
        local_since: at,
        pool,
        pool_since: at,
        pool_bad_since: None,
        pool_checks: vec![],
        checked_at: at,
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated rigdeck_test database"]
async fn statuses_alerts_and_one_evaluation_pass() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("status-{}", Uuid::new_v4());
    store.create_admin(&name, "unused").await.unwrap();
    let actor = Actor {
        id: store.password_hash(&name).await.unwrap().unwrap().0,
        name: name.clone(),
        token_id: None,
    };
    let mut machines = Vec::new();
    for role in ["pool", "recover", "proxy", "aggregated", "via-proxy"] {
        let id = Uuid::new_v4();
        let input: MachineInput = serde_json::from_value(json!({"name":format!("{role}-{id}"),"host":"127.0.0.1","port":22,"username":"root","host_key":"SHA256:fixture","policy":{}})).unwrap();
        store
            .save_machine(id, &input, Some("secret"), None, &actor)
            .await
            .unwrap();
        machines.push(id);
    }
    let (pool_rig, recovering, proxy_rig, aggregated, via_proxy) = (
        machines[0],
        machines[1],
        machines[2],
        machines[3],
        machines[4],
    );

    // Statuses round-trip; alerts for an unknown machine are dropped, not a foreign-key error.
    let mut seeded = status(pool_rig, LocalState::Online, PoolState::Online);
    seeded.pool_bad_since = Some(Utc::now() - Duration::minutes(6));
    // Same history for the proxied rig: it must still not be reported.
    let mut seeded_proxied = status(aggregated, LocalState::Online, PoolState::Online);
    seeded_proxied.pool_bad_since = seeded.pool_bad_since;
    // The rig behind the user's own xmrig-proxy has been missing there for 6 minutes too.
    let mut seeded_via_proxy = status(via_proxy, LocalState::Online, PoolState::Online);
    seeded_via_proxy.pool_bad_since = seeded.pool_bad_since;
    let before = store.alerts(None, 1).await.unwrap();
    let baseline = before.last().map_or(0, |a| a.id);
    let alert = |machine, message: &str| NewAlert {
        machine_id: machine,
        kind: AlertKind::Local,
        event: AlertEvent::Offline,
        at: Utc::now(),
        message: message.into(),
    };
    store
        .save_statuses(
            &[
                seeded.clone(),
                seeded_proxied,
                seeded_via_proxy,
                status(recovering, LocalState::Offline, PoolState::Unknown),
                status(Uuid::new_v4(), LocalState::Online, PoolState::Unknown),
            ],
            &[
                alert(recovering, "first"),
                alert(Uuid::new_v4(), "ghost"),
                alert(recovering, "second"),
            ],
        )
        .await
        .unwrap();
    let stored = store.machine_statuses().await.unwrap();
    assert_eq!(
        stored.iter().find(|s| s.machine_id == pool_rig),
        Some(&seeded)
    );
    let newer = store.alerts(Some(baseline), 50).await.unwrap();
    let messages: Vec<_> = newer.iter().map(|a| a.message.as_str()).collect();
    assert_eq!(messages, ["first", "second"]);
    assert!(newer[0].id < newer[1].id);
    assert!(newer[0].machine_name.starts_with("recover-"));
    // Without `after`: the newest ones, still oldest first.
    let last = store.alerts(None, 1).await.unwrap();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].message, "second");

    // Deployments: the pool rig mines on SupportXMR, the other on the user's own proxy.
    let wallet = store
        .save_catalog(
            CatalogKind::Wallet,
            Uuid::new_v4(),
            &CatalogInput {
                name: name.clone(),
                data: json!({"coin_symbol":"XMR","address":WALLET}),
                expected_revision: None,
            },
            &actor,
        )
        .await
        .unwrap();
    let sheet_id = Uuid::new_v4();
    store
        .save_flight(
            sheet_id,
            &FlightInput {
                name: name.clone(),
                expected_version: None,
                tasks: vec![FlightTask {
                    instance: "xmr".into(),
                    wallet_id: wallet.id,
                    miner: json!({"adapter":"xmrig","name":"XMRig","url":"https://example.com/x.tar.gz"}),
                    config: json!({}),
                }],
            },
            &actor,
            None,
        )
        .await
        .unwrap();
    let rendered = |url: &str, login: String| {
        json!({"sheet_id":sheet_id,"instances":[{"instance":"xmr","coin":"XMR","environment":{
            "CUSTOM_TEMPLATE":login,"CUSTOM_PASS":"x","CUSTOM_URL":url}}]})
    };
    let mut jobs = Vec::new();
    let supportxmr = "stratum+ssl://pool.supportxmr.com:443";
    for (machine, url, wallet) in [
        (pool_rig, "mac.icuicu.de:6666", WALLET),
        (pool_rig, supportxmr, WALLET),
        (proxy_rig, "mac.icuicu.de:6666", WALLET),
        // The rig is configured for SupportXMR, but its traffic goes through xmrig-proxy.
        (aggregated, supportxmr, PROXIED),
        // The user's own xmrig-proxy, configured below with its API.
        (via_proxy, "stratum+tcp://149.104.4.41:8888", WALLET),
    ] {
        let job = store
            .enqueue(
                &JobInput {
                    machine_ids: vec![machine],
                    action: Action::Apply { sheet_id },
                    idempotency_key: Uuid::new_v4().to_string(),
                    include_controller: false,
                },
                &json!({"kind":"apply","snapshot":{"hosts":{machine.to_string():rendered(url, format!("{wallet}.{}", if machine == via_proxy { "rig07" } else { "rig01" }))}}}),
                &actor,
            )
            .await
            .unwrap();
        jobs.push(job);
    }
    let deployments = store.current_deployments().await.unwrap();
    let current = |id| deployments.iter().find(|(m, _)| *m == id).map(|(_, d)| d);
    assert_eq!(
        current(pool_rig).unwrap()["instances"][0]["environment"]["CUSTOM_URL"],
        "stratum+ssl://pool.supportxmr.com:443",
        "the newest deployment wins"
    );

    // Every machine reports fresh telemetry with its miner running.
    let now = Utc::now();
    let mut samples = Vec::new();
    for machine in &machines {
        samples.push(Observation {
            machine_id: *machine,
            kind: "system".into(),
            observed_at: now,
            data: json!({"cpu_pct": 10}),
            error: None,
        });
        samples.push(Observation {
            machine_id: *machine,
            kind: "mining".into(),
            observed_at: now,
            data: json!({"instances":[{"instance":"xmr","desired":"running","process_alive":true}]}),
            error: None,
        });
    }
    store.observe_many(&samples).await.unwrap();

    let pool = Arc::new(EmptyPool::default());
    let remote = Arc::new(SshExecutor::default());
    let app = App {
        bios: None,
        icons: None,
        repository: store.clone(),
        vault: Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap()),
        remote: remote.clone(),
        runtime: Arc::new(ScreenRuntime::new(remote, std::env::temp_dir()).unwrap()),
        adapters: Arc::new(adapters::registry()),
        bmc: Arc::new(bmc::registry()),
        push: Default::default(),
        pools: Some(pool.clone()),
    };

    // Proxy settings: the token is stored encrypted and only reported as present.
    let mut settings = FarmSettings::default();
    settings.alerts.pool_offline_minutes = 5;
    settings.alerts.proxies = vec![ProxyApi {
        pool: "149.104.4.41:8888".into(),
        api_url: "http://149.104.4.41:4048/".into(),
        token: Some("proxy-token-1".into()),
        has_token: false,
        token_secret: Some("forged by a client".into()),
    }];
    let saved = app.save_farm_settings(&actor, settings).await.unwrap();
    assert!(saved.alerts.proxies[0].has_token);
    assert_eq!(saved.alerts.proxies[0].token, None);
    assert_eq!(saved.alerts.proxies[0].token_secret, None);
    let raw = store.farm_settings().await.unwrap().unwrap().to_string();
    assert!(
        !raw.contains("proxy-token-1") && !raw.contains("forged"),
        "{raw}"
    );
    let read = app.farm_settings().await.unwrap();
    assert!(read.alerts.proxies[0].has_token);
    assert!(read.alerts.proxies[0].token_secret.is_none());
    // Saving the form back without a token keeps the stored one.
    let mut again = read.clone();
    again.alerts.proxies[0].has_token = false;
    app.save_farm_settings(&actor, again).await.unwrap();
    assert!(app.farm_settings().await.unwrap().alerts.proxies[0].has_token);
    let after_seed = store.alerts(None, 1).await.unwrap()[0].id;
    // The first pass runs at once; the second (10 s later) must not repeat any alert.
    let evaluator = tokio::spawn(app.clone().run_status());
    tokio::time::sleep(std::time::Duration::from_secs(13)).await;
    evaluator.abort();
    let _ = evaluator.await;

    let ours = |a: &&StatusAlert| machines.contains(&a.machine_id);
    let raised: Vec<_> = store
        .alerts(Some(after_seed), 500)
        .await
        .unwrap()
        .into_iter()
        .filter(|a| ours(&a))
        .collect();
    let summary: Vec<_> = raised
        .iter()
        .map(|a| (a.machine_id, a.kind, a.event))
        .collect();
    assert_eq!(
        summary,
        {
            let mut expected = vec![
                (pool_rig, AlertKind::Pool, AlertEvent::Offline),
                (recovering, AlertKind::Local, AlertEvent::Recovered),
                (via_proxy, AlertKind::Pool, AlertEvent::Offline),
            ];
            expected.sort_by_key(|(id, ..)| raised.iter().position(|a| a.machine_id == *id));
            expected
        },
        "{raised:#?}"
    );
    let message = |id| &raised.iter().find(|a| a.machine_id == id).unwrap().message;
    assert!(
        message(pool_rig).contains("找不到 worker「rig01」，这个钱包的总算力也为 0"),
        "{}",
        message(pool_rig)
    );
    assert!(
        message(via_proxy).contains("xmrig-proxy 上看不到 worker「rig07」"),
        "{}",
        message(via_proxy)
    );

    let statuses = app.machine_statuses().await.unwrap();
    let of = |id| statuses.iter().find(|s| s.machine_id == id).unwrap();
    assert_eq!(of(pool_rig).state, MachineState::PoolOffline);
    assert_eq!(
        of(pool_rig).pool_checks[0].provider.as_deref(),
        Some("supportxmr")
    );
    assert_eq!(of(recovering).state, MachineState::Online);
    // Proxied: the pool only lists the proxy, but the wallet hashes: no per-rig verdict.
    assert_eq!(of(aggregated).state, MachineState::Online);
    assert_eq!(of(aggregated).pool, PoolState::Unsupported);
    assert_eq!(
        of(aggregated).pool_checks[0].result,
        PoolCheckResult::Ambiguous
    );
    assert_eq!(of(proxy_rig).pool, PoolState::Unsupported);
    assert_eq!(
        of(proxy_rig).pool_checks[0].result,
        PoolCheckResult::Unsupported
    );
    // One wallet on one supported pool: asked once (cached for two minutes), the proxy never.
    let asked = pool.asked.lock().unwrap().clone();
    assert_eq!(
        asked.iter().filter(|a| a.wallet == WALLET).count(),
        1,
        "{asked:?}"
    );
    assert_eq!(asked.iter().filter(|a| a.wallet == PROXIED).count(), 1);
    // The proxy rig never went to a public pool; its proxy was asked once, with the token.
    assert_eq!(
        of(via_proxy).pool_checks[0].provider.as_deref(),
        Some("xmrig-proxy")
    );
    assert_eq!(
        pool.proxied.lock().unwrap().clone(),
        [(
            "http://149.104.4.41:4048/".to_owned(),
            Some("proxy-token-1".to_owned())
        )]
    );
    assert!(asked.iter().all(|a| a.provider == "supportxmr"));

    // No worker claims these; leave nothing queued for tests sharing the database.
    for job in jobs {
        store.cancel_job(job, &actor).await.unwrap();
    }
}
