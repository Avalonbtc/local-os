use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{adapters, bmc, pg::PgStore, ssh::SshExecutor, vault::Vault};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

struct FixtureRuntime {
    started: Instant,
    offline_mining: AtomicUsize,
    starts: AtomicUsize,
}
#[async_trait::async_trait]
impl MinerRuntime for FixtureRuntime {
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
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn operation(&self, _: &Machine, _: &Credential, _: Uuid) -> Result<RemoteOperation> {
        Ok(RemoteOperation {
            status: if self.started.elapsed().as_secs() >= 32 {
                "succeeded"
            } else {
                "running"
            }
            .into(),
            output: "fixture output".into(),
            truncated: false,
            result: None,
            error: None,
        })
    }
    async fn cancel(&self, _: &Machine, _: &Credential, _: Uuid) -> Result<()> {
        Ok(())
    }
    async fn collect(&self, m: &Machine, _: &Credential, kind: &str) -> Result<Value> {
        if m.name.starts_with("offline-review-") {
            if kind == "mining" {
                self.offline_mining.fetch_add(1, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
            return Err(Error::Unavailable("fixture offline".into()));
        }
        Ok(match kind {
            "system" => json!({"cpu_pct":25}),
            "mining" => json!({"instances":[]}),
            _ => json!({}),
        })
    }
}
#[tokio::test]
#[ignore = "requires isolated rigdeck_test database; 40-second offline/heartbeat regression"]
async fn offline_hosts_and_transient_heartbeat_failure_do_not_interrupt_healthy_work() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("resilience-{}", Uuid::new_v4());
    store.create_admin(&name, "fixture").await.unwrap();
    let (id, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let runtime = Arc::new(FixtureRuntime {
        started: Instant::now(),
        offline_mining: AtomicUsize::new(0),
        starts: AtomicUsize::new(0),
    });
    let app = App {
        bios: None,
        repository: store.clone(),
        vault: vault.clone(),
        remote: Arc::new(SshExecutor::default()),
        runtime: runtime.clone(),
        adapters: Arc::new(adapters::registry()),
        bmc: Arc::new(bmc::registry()),
    };
    let secret = vault
        .encrypt(&Credential::Password {
            password: "fixture".into(),
            sudo_password: None,
        })
        .unwrap();
    let mut ids = Vec::new();
    for n in 0..9 {
        let id = Uuid::new_v4();
        let input:MachineInput=serde_json::from_value(json!({"name":format!("{}-{}",if n==8 {"online-review"} else {"offline-review"},id),"host":"127.0.0.1","port":22,"username":"fixture","host_key":"SHA256:fixture","group":"test","tags":[],"is_controller":false,"bmc":null,"policy":{}})).unwrap();
        store
            .save_machine(id, &input, Some(&secret), None, &actor)
            .await
            .unwrap();
        ids.push(id);
    }
    let input = JobInput {
        machine_ids: vec![ids[8]],
        action: Action::Command {
            script: "fixture-only".into(),
            timeout_seconds: 60,
        },
        concurrency: 1,
        canary: false,
        include_controller: false,
        idempotency_key: Uuid::new_v4().to_string(),
    };
    let job = app.submit_job(&actor, input).await.unwrap();
    // Inject only renewal failures for this job for 15s. Claim/progress/finish remain available.
    let ddl = format!(
        "CREATE OR REPLACE FUNCTION review_heartbeat_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.job_id='{job}' AND OLD.status='running' AND NEW.lease_until IS DISTINCT FROM OLD.lease_until AND clock_timestamp() < '{}'::timestamptz THEN RAISE EXCEPTION 'temporary heartbeat outage'; END IF; RETURN NEW; END $$",
        chrono::Utc::now() + chrono::Duration::seconds(15)
    );
    sqlx::query(&ddl).execute(&store.pool).await.unwrap();
    sqlx::query("CREATE TRIGGER review_heartbeat_fault BEFORE UPDATE ON job_targets FOR EACH ROW EXECUTE FUNCTION review_heartbeat_fault()").execute(&store.pool).await.unwrap();
    let worker = tokio::spawn(app.clone().run_worker());
    let monitor = tokio::spawn(app.run_monitor());
    tokio::time::sleep(Duration::from_secs(38)).await;
    worker.abort();
    monitor.abort();
    let _ = worker.await;
    let _ = monitor.await;
    sqlx::query("DROP TRIGGER review_heartbeat_fault ON job_targets")
        .execute(&store.pool)
        .await
        .unwrap();
    let result = store.job(job).await.unwrap();
    assert_eq!(result.status, "succeeded", "{:?}", result);
    assert_eq!(runtime.starts.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.offline_mining.load(Ordering::SeqCst), 0);
    let latest = store.latest(Some(ids[8])).await.unwrap();
    let system = latest.iter().find(|o| o.kind == "system").unwrap();
    assert!((chrono::Utc::now() - system.observed_at).num_seconds() < 15);
    for id in ids {
        store.delete_machine(id, &actor).await.unwrap();
    }
}
