use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{adapters, bmc, pg::PgStore, vault::Vault};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

/// Hosts ending in .9 are "offline"; others answer with a deterministic ed25519 key.
struct Probe;
#[async_trait::async_trait]
impl RemoteExecutor for Probe {
    async fn probe_host_key(&self, host: &str, _: u16) -> Result<SshHostKey> {
        if host.ends_with(".9") {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            return Err(Error::Unavailable("SSH 指纹获取超时".into()));
        }
        Ok(SshHostKey {
            fingerprint: format!("SHA256:{:0<43}", host.replace('.', "")),
            algorithm: "ssh-ed25519".into(),
        })
    }
    async fn execute(&self, _: &Machine, _: &Credential, _: &str, _: u64) -> Result<ExecOutput> {
        unreachable!()
    }
    async fn upload(&self, _: &Machine, _: &Credential, _: &str, _: &[u8], _: u32) -> Result<()> {
        unreachable!()
    }
    async fn terminal(
        &self,
        _: &Machine,
        _: &Credential,
        _: Option<&str>,
        _: tokio::sync::mpsc::Receiver<TerminalInput>,
        _: tokio::sync::mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        unreachable!()
    }
}
struct NoRuntime;
#[async_trait::async_trait]
impl MinerRuntime for NoRuntime {
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
    async fn collect(&self, _: &Machine, _: &Credential, _: &str) -> Result<Value> {
        unreachable!()
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated rigdeck_test database"]
async fn batch_probe_and_create_report_per_row_and_queue_one_bootstrap() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("batch-{}", Uuid::new_v4());
    store.create_admin(&name, "unused").await.unwrap();
    let (id, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let app = App {
        bios: None,
        repository: store.clone(),
        vault,
        remote: Arc::new(Probe),
        runtime: Arc::new(NoRuntime),
        adapters: Arc::new(adapters::registry()),
        bmc: Arc::new(bmc::registry()),
    };
    let hosts = ["10.9.0.1", "10.9.0.9", "10.9.0.2"];
    let probed = app
        .probe_ssh_host_keys(
            &actor,
            SshProbeBatch {
                targets: hosts
                    .iter()
                    .map(|h| SshProbeInput {
                        host: h.to_string(),
                        port: 22,
                    })
                    .collect(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        probed.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
        hosts,
        "results keep input order"
    );
    assert!(probed[1].fingerprint.is_none() && probed[1].error.is_some());
    assert!(
        probed[0]
            .fingerprint
            .as_deref()
            .unwrap()
            .starts_with("SHA256:")
    );
    assert!(
        app.probe_ssh_host_keys(&actor, SshProbeBatch { targets: vec![] })
            .await
            .is_err()
    );

    let tag = Uuid::new_v4().simple().to_string();
    let row = |name: &str, host: &str, key: &Option<String>, credential: bool| -> MachineInput {
        serde_json::from_value(json!({
            "name": format!("{name}-{tag}"), "host": host, "port": 22, "username": "root",
            "host_key": key.clone().unwrap_or_default(), "group": "rack-a", "tags": ["batch"],
            "credential": if credential { json!({"kind":"password","password":"shared"}) } else { Value::Null },
        }))
        .unwrap()
    };
    let result = app
        .save_machines_batch(
            &actor,
            MachineBatchInput {
                machines: vec![
                    row("rig-1", hosts[0], &probed[0].fingerprint, true),
                    row("rig-1", hosts[2], &probed[2].fingerprint, true),
                    row("rig-2", hosts[2], &probed[2].fingerprint, false),
                    row("rig-3", hosts[2], &None, true),
                    row("rig-4", hosts[2], &probed[2].fingerprint, true),
                ],
                bootstrap: true,
            },
        )
        .await
        .unwrap();
    let errors: Vec<_> = result.results.iter().map(|r| r.error.is_some()).collect();
    assert_eq!(
        errors,
        vec![false, true, true, true, false],
        "{:?}",
        result.results
    );
    assert!(result.results[1].error.as_deref().unwrap().contains("重复"));
    let job = app.job(result.bootstrap_job_id.unwrap()).await.unwrap();
    assert_eq!(job.targets.len(), 2);
    let machines = app.machines().await.unwrap();
    let created = machines
        .iter()
        .find(|m| m.name == format!("rig-4-{tag}"))
        .unwrap();
    assert_eq!(created.group, "rack-a");
    assert!(
        app.credential(created.id, false).await.is_ok(),
        "shared credential stored encrypted per machine"
    );
    app.cancel_job(&actor, job.id).await.unwrap();
}
