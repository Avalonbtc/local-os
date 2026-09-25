use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{adapters, bmc, pg::PgStore, ssh::SshExecutor, vault::Vault};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use uuid::Uuid;

/// A rig whose watchdog publishes a snapshot; privileged `collect` must not be used for
/// system/mining while that snapshot is fresh.
struct SnapshotRig {
    // CI shares one database between integration tests; only this test's machine is a snapshot rig.
    target: Uuid,
    collects: Mutex<Vec<String>>,
    policies: AtomicUsize,
    uptime: Mutex<f64>,
}
#[async_trait::async_trait]
impl MinerRuntime for SnapshotRig {
    async fn cache_package(&self, _: &[u8]) -> Result<Value> {
        unreachable!()
    }
    async fn import_package(&self, _: &str) -> Result<Value> {
        unreachable!()
    }
    async fn bootstrap(&self, _: &Machine, _: &Credential) -> Result<Value> {
        unreachable!()
    }
    async fn start_operation(&self, _: &Machine, _: &Credential, _: Uuid, _: &Value) -> Result<()> {
        unreachable!()
    }
    async fn operation(&self, _: &Machine, _: &Credential, _: Uuid) -> Result<RemoteOperation> {
        unreachable!()
    }
    async fn cancel(&self, _: &Machine, _: &Credential, _: Uuid) -> Result<()> {
        unreachable!()
    }
    async fn collect(&self, m: &Machine, _: &Credential, kind: &str) -> Result<Value> {
        if m.id != self.target {
            return Err(Error::Unavailable("not part of this test".into()));
        }
        self.collects.lock().unwrap().push(kind.into());
        Ok(match kind {
            "software" => json!({"processes": []}),
            _ => json!({}),
        })
    }
    async fn snapshot(&self, m: &Machine, _: &Credential) -> Result<Option<Value>> {
        if m.id != self.target {
            return Err(Error::Unavailable("not part of this test".into()));
        }
        let mut uptime = self.uptime.lock().unwrap();
        *uptime += 10.0;
        let now = *uptime;
        // The rig wall clock is a day off; only uptime deltas may be trusted.
        Ok(Some(json!({"now_uptime": now + 3.0, "snapshot": {
            "schema": 1, "boot_id": "boot", "uptime": now, "written_at": 0.0, "policy_digest": null,
            "system": {"cpu_pct": 42.0, "cpu_temperature": 61.0},
            "mining": {"instances": [{"instance": "xmr", "boot_id": "boot", "sample_uptime": now - 1.0,
                "process_alive": true, "desired": "running", "observed_at": 1.0, "stats_observed_at": 1.0,
                "stats": {"hashrate_hs": 1234.0, "algorithm": "rx/0"}}]},
            "events": [{"id": "6a0e2d5e-6d49-4a5c-9b8f-3b8f4a2b9c11", "seq": 7, "boot_id": "boot",
                "uptime": now - 5.0, "at": 86400.0, "level": "warning", "kind": "watchdog_restart",
                "instance": "xmr", "message": "看门狗重启矿工"}]
        }})))
    }
    async fn set_policy(
        &self,
        m: &Machine,
        _: &Credential,
        digest: &str,
        policy: &Value,
    ) -> Result<()> {
        assert_eq!(m.id, self.target);
        assert_eq!(digest.len(), 64);
        assert_eq!(policy["max_restarts"], 3);
        self.policies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires isolated rigdeck_test database"]
async fn fresh_snapshot_replaces_privileged_collection_and_delivers_events_and_policy() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("snapshot-{}", Uuid::new_v4());
    store.create_admin(&name, "unused").await.unwrap();
    let (id, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let machine = Uuid::new_v4();
    let rig = Arc::new(SnapshotRig {
        target: machine,
        collects: Mutex::new(Vec::new()),
        policies: AtomicUsize::new(0),
        uptime: Mutex::new(1000.0),
    });
    let input: MachineInput = serde_json::from_value(json!({"name":format!("snapshot-{machine}"),"host":"127.0.0.1","port":22,"username":"miner","host_key":"SHA256:fixture","policy":{"max_restarts":3}})).unwrap();
    let secret = vault
        .encrypt(&Credential::Password {
            password: "fixture".into(),
            sudo_password: None,
        })
        .unwrap();
    store
        .save_machine(machine, &input, Some(&secret), None, &actor)
        .await
        .unwrap();
    let app = App {
        bios: None,
        repository: store.clone(),
        vault,
        remote: Arc::new(SshExecutor::default()),
        runtime: rig.clone(),
        adapters: Arc::new(adapters::registry()),
        bmc: Arc::new(bmc::registry()),
    };
    let monitor = tokio::spawn(app.clone().run_monitor());
    tokio::time::sleep(Duration::from_secs(23)).await;
    monitor.abort();
    let _ = monitor.await;

    let collects = rig.collects.lock().unwrap().clone();
    assert!(
        !collects.iter().any(|k| k == "system" || k == "mining"),
        "{collects:?}"
    );
    assert!(collects.iter().any(|k| k == "software"));
    assert_eq!(
        rig.policies.load(Ordering::SeqCst),
        1,
        "policy pushed once, then rate limited"
    );

    let latest = app.latest(Some(machine)).await.unwrap();
    let system = latest.iter().find(|o| o.kind == "system").unwrap();
    assert_eq!(system.data["cpu_pct"], 42.0);
    let age = chrono::Utc::now() - system.observed_at;
    assert!(
        age.num_seconds() >= 2 && age.num_seconds() < 20,
        "sample aged by uptime: {age}"
    );
    let mining = latest.iter().find(|o| o.kind == "mining").unwrap();
    let stats_at = mining.data["instances"][0]["stats_observed_at"]
        .as_f64()
        .unwrap();
    assert!(
        (chrono::Utc::now().timestamp() as f64 - stats_at).abs() < 30.0,
        "rig wall clock ignored"
    );

    let messages = app.messages(machine, 20).await.unwrap();
    let event = messages
        .iter()
        .find(|m| m.kind == "watchdog_restart")
        .unwrap();
    assert!(
        (chrono::Utc::now() - event.at).num_seconds() < 60,
        "event time from uptime, not the rig clock"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.kind == "watchdog_restart")
            .count(),
        1,
        "re-sent tail is idempotent"
    );
}
