use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{adapters, pg::PgStore, ssh::SshExecutor, vault::Vault};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

#[derive(Default)]
struct HeldRuntime {
    started: Mutex<HashSet<Uuid>>,
    released: Mutex<HashSet<Uuid>>,
    cancelled: Mutex<HashSet<Uuid>>,
}

#[async_trait::async_trait]
impl MinerRuntime for HeldRuntime {
    async fn cache_package(&self, _: &[u8]) -> Result<Value> {
        unreachable!()
    }
    async fn import_package(&self, _: &str) -> Result<Value> {
        unreachable!()
    }
    async fn bootstrap(&self, _: &Machine, _: &Credential) -> Result<Value> {
        unreachable!()
    }
    async fn start_operation(
        &self,
        _: &Machine,
        _: &Credential,
        id: Uuid,
        _: &Value,
    ) -> Result<()> {
        assert!(
            self.started.lock().unwrap().insert(id),
            "duplicate dispatch"
        );
        Ok(())
    }
    async fn operation(&self, _: &Machine, _: &Credential, id: Uuid) -> Result<RemoteOperation> {
        let status = if self.cancelled.lock().unwrap().contains(&id) {
            "cancelled"
        } else if self.released.lock().unwrap().contains(&id) {
            "succeeded"
        } else {
            "running"
        };
        Ok(RemoteOperation {
            status: status.into(),
            output: String::new(),
            truncated: false,
            result: None,
            error: None,
        })
    }
    async fn cancel(&self, _: &Machine, _: &Credential, id: Uuid) -> Result<()> {
        self.cancelled.lock().unwrap().insert(id);
        Ok(())
    }
    async fn collect(&self, _: &Machine, _: &Credential, _: &str) -> Result<Value> {
        unreachable!()
    }
}

async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("dispatch did not progress while other machines were still running");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated rigdeck_test database"]
async fn all_selected_machines_start_without_slots_or_canary_but_same_machine_waits() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("dispatch-{}", Uuid::new_v4());
    store.create_admin(&name, "fixture").await.unwrap();
    let actor = Actor {
        id: store.password_hash(&name).await.unwrap().unwrap().0,
        name,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let runtime = Arc::new(HeldRuntime::default());
    let app = App {
        repository: store.clone(),
        vault: vault.clone(),
        remote: Arc::new(SshExecutor::default()),
        runtime: runtime.clone(),
        adapters: Arc::new(adapters::registry()),
        agents: Default::default(),
        push: Default::default(),
    };
    let secret = vault
        .encrypt(&Credential::Password {
            password: "fixture".into(),
            sudo_password: None,
        })
        .unwrap();
    let mut hosts = Vec::new();
    for _ in 0..12 {
        let id = Uuid::new_v4();
        let input: MachineInput = serde_json::from_value(json!({
            "name": format!("dispatch-{id}"), "host": "127.0.0.1", "username": "root",
            "host_key": "SHA256:fixture", "policy": {}
        }))
        .unwrap();
        store
            .save_machine(id, &input, Some(&secret), &actor)
            .await
            .unwrap();
        hosts.push(id);
    }
    // An old client may still send these properties. Neither can hold back the selection.
    let input: JobInput = serde_json::from_value(json!({
        "machine_ids": hosts, "idempotency_key": Uuid::new_v4(),
        "action": {"kind": "command", "script": "fixture", "timeout_seconds": 60},
        "concurrency": 1, "canary": true
    }))
    .unwrap();
    let job_id = app.submit_job(&actor, input.clone()).await.unwrap();
    assert_eq!(app.submit_job(&actor, input).await.unwrap(), job_id);
    // Also cover jobs already queued by an older controller.
    sqlx::query("UPDATE jobs SET concurrency=1,canary=true WHERE id=$1")
        .bind(job_id)
        .execute(&store.pool)
        .await
        .unwrap();
    let worker = tokio::spawn(app.clone().run_worker());
    wait_for(|| runtime.started.lock().unwrap().len() == 12).await;
    let job = store.job(job_id).await.unwrap();
    assert!(job.targets.iter().all(|t| t.status == "running"));
    assert!(runtime.released.lock().unwrap().is_empty());

    let first = &job.targets[0];
    let followup = JobInput {
        machine_ids: vec![first.machine_id],
        action: Action::Command {
            script: "second".into(),
            timeout_seconds: 60,
        },
        idempotency_key: Uuid::new_v4().to_string(),
        include_controller: false,
    };
    let followup_id = app.submit_job(&actor, followup.clone()).await.unwrap();
    let cancelled_id = app
        .submit_job(
            &actor,
            JobInput {
                idempotency_key: Uuid::new_v4().to_string(),
                ..followup
            },
        )
        .await
        .unwrap();
    store.cancel_job(cancelled_id, &actor).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(runtime.started.lock().unwrap().len(), 12);
    assert_eq!(store.job(followup_id).await.unwrap().status, "queued");
    assert_eq!(store.job(cancelled_id).await.unwrap().status, "cancelled");

    runtime.released.lock().unwrap().insert(first.operation_id);
    wait_for(|| runtime.started.lock().unwrap().len() == 13).await;
    let other = store.job(followup_id).await.unwrap();
    assert_eq!(other.targets[0].status, "running");
    // Cancelling an active batch still reaches every outstanding remote operation.
    store.cancel_job(job_id, &actor).await.unwrap();
    runtime
        .released
        .lock()
        .unwrap()
        .insert(other.targets[0].operation_id);
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let job = store.job(job_id).await.unwrap();
            let other = store.job(followup_id).await.unwrap();
            if job.status == "cancelled" && other.status == "succeeded" {
                assert_eq!(
                    job.targets
                        .iter()
                        .filter(|t| t.status == "cancelled")
                        .count(),
                    11
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    worker.abort();
    let _ = worker.await;
    for id in hosts {
        store.delete_machine(id, &actor).await.unwrap();
    }
}
