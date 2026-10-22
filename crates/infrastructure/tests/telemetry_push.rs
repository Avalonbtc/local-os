use flate2::{Compression, write::ZlibEncoder};
use hmac::{Hmac, Mac};
use rig_application::{
    App,
    push::{PushHub, PushReply, PushSettings},
};
use rig_domain::*;
use rig_infrastructure::{adapters, pg::PgStore, ssh::SshExecutor, vault::Vault};
use serde_json::{Value, json};
use sha2::Sha256;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use uuid::Uuid;

/// Counts SSH reads; while the rig pushes, the monitor must not make any.
struct QuietRig {
    target: Uuid,
    reads: AtomicUsize,
}
#[async_trait::async_trait]
impl MinerRuntime for QuietRig {
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
    async fn collect(&self, m: &Machine, _: &Credential, _: &str) -> Result<Value> {
        if m.id == self.target {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        Err(Error::Unavailable("not part of this test".into()))
    }
    async fn snapshot(&self, m: &Machine, _: &Credential) -> Result<Option<Value>> {
        if m.id == self.target {
            self.reads.fetch_add(1, Ordering::SeqCst);
        }
        Err(Error::Unavailable("not part of this test".into()))
    }
}

struct Sender {
    machine: Uuid,
    key: Vec<u8>,
    epoch: Option<String>,
    seq: i64,
    digest: String,
}
impl Sender {
    fn body(&mut self, extra: Value) -> (String, Vec<u8>) {
        self.seq += 1000;
        let mut message = json!({"v":1,"machine":self.machine,"epoch":self.epoch,"seq":self.seq,
            "boot_id":"boot","sent_uptime":self.seq as f64 / 1000.0 + 1.0});
        message
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(message.to_string().as_bytes()).unwrap();
        let body = encoder.finish().unwrap();
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).unwrap();
        mac.update(&body);
        (hex::encode(mac.finalize().into_bytes()), body)
    }
    fn snapshot(&self, cpu: f64) -> Value {
        let uptime = self.seq as f64 / 1000.0;
        json!({"schema":1,"runtime_version":"0.4.0","boot_id":"boot","uptime":uptime,"written_at":0.0,
            "policy_digest":null,"push_digest":self.digest,
            "system":{"cpu_pct":cpu,"gpus":[{"temperature":50},{"temperature":51}]},
            "mining":{"instances":[{"instance":"xmr","boot_id":"boot","sample_uptime":uptime - 1.0,
                "process_alive":true,"stats_observed_at":1.0,"observed_at":1.0,"stats":{"hashrate_hs":1000.0}}]}})
    }
}

async fn push(app: &App, sender: &mut Sender, extra: Value) -> Result<PushReply> {
    let (signature, body) = sender.body(extra);
    app.ingest_push(&sender.machine.to_string(), &signature, &body)
        .await
}

#[tokio::test]
#[ignore = "requires isolated rigdeck_test database"]
async fn pushed_snapshots_replace_ssh_reads_with_patches_events_and_policy() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let name = format!("push-{}", Uuid::new_v4());
    store.create_admin(&name, "unused").await.unwrap();
    let (id, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let machine = Uuid::new_v4();
    let input: MachineInput = serde_json::from_value(json!({"name":format!("push-{machine}"),"host":"127.0.0.1","port":22,"username":"miner","host_key":"SHA256:fixture","policy":{"max_restarts":3}})).unwrap();
    let secret = vault
        .encrypt(&Credential::Password {
            password: "fixture".into(),
            sudo_password: None,
        })
        .unwrap();
    store
        .save_machine(machine, &input, Some(&secret), &actor)
        .await
        .unwrap();
    let rig = Arc::new(QuietRig {
        target: machine,
        reads: AtomicUsize::new(0),
    });
    let app = App {
        repository: store.clone(),
        vault: vault.clone(),
        remote: Arc::new(SshExecutor::default()),
        runtime: rig.clone(),
        adapters: Arc::new(adapters::registry()),
        agents: Default::default(),
        push: Arc::new(PushHub::new(Some(PushSettings {
            url: None,
            port: 8091,
        }))),
        pools: None,
    };
    let target = app.push_target(machine).unwrap();
    assert_eq!(target.key, vault.push_key(machine).unwrap());
    let mut sender = Sender {
        machine,
        key: hex::decode(&target.key).unwrap(),
        epoch: None,
        seq: 100_000,
        digest: target.digest,
    };

    // A forged signature or another machine's key is refused before anything is parsed.
    let (_, body) = sender.body(json!({}));
    assert!(matches!(
        app.ingest_push(&machine.to_string(), &"00".repeat(32), &body)
            .await,
        Err(Error::Unauthorized)
    ));
    let mut stranger = Sender {
        machine,
        key: hex::decode(vault.push_key(Uuid::new_v4()).unwrap()).unwrap(),
        epoch: None,
        seq: 0,
        digest: String::new(),
    };
    assert!(matches!(
        push(&app, &mut stranger, json!({})).await,
        Err(Error::Unauthorized)
    ));

    // No epoch yet: resync.
    let snapshot = sender.snapshot(10.0);
    let PushReply::Resync(reply) = push(&app, &mut sender, json!({"snapshot":snapshot}))
        .await
        .unwrap()
    else {
        panic!("first push must resync")
    };
    sender.epoch = reply["epoch"].as_str().map(str::to_owned);

    // A failed transaction must not consume the sequence/base or mark push as live.
    let function = format!("fail_push_{}", machine.simple());
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.machine_id = '{machine}'::uuid THEN RAISE EXCEPTION 'injected telemetry failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER {function} BEFORE INSERT OR UPDATE ON latest_observations FOR EACH ROW EXECUTE FUNCTION {function}();"
    )).execute(&store.pool).await.unwrap();
    let snapshot = sender.snapshot(9.0);
    let (signature, body) = sender.body(json!({"snapshot":snapshot}));
    let failed = app
        .ingest_push(&machine.to_string(), &signature, &body)
        .await;
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {function} ON latest_observations; DROP FUNCTION {function}();"
    ))
    .execute(&store.pool)
    .await
    .unwrap();
    assert!(failed.is_err());
    assert!(matches!(
        app.ingest_push(&machine.to_string(), &signature, &body)
            .await
            .unwrap(),
        PushReply::Accepted(_)
    ));

    // A stale packet from a different boot must not retire the current valid boot.
    let mut stale = sender.snapshot(0.0);
    stale["boot_id"] = json!("bad-boot");
    stale["uptime"] = json!(0);
    assert!(matches!(
        push(
            &app,
            &mut sender,
            json!({"boot_id":"bad-boot", "snapshot":stale})
        )
        .await,
        Err(Error::Validation(_))
    ));

    let event = json!({"id":Uuid::new_v4(),"seq":7,"boot_id":"boot","uptime":99.0,"at":5.0,
        "level":"warning","kind":"watchdog_restart","instance":"xmr","message":"看门狗重启矿工"});
    let snapshot = sender.snapshot(10.0);
    let PushReply::Accepted(reply) = push(
        &app,
        &mut sender,
        json!({"snapshot":snapshot,"events":[event],"software":{"processes":[]},"hardware":{"cpu_model":"EPYC"}}),
    )
    .await
    .unwrap() else {
        panic!("full snapshot must be accepted")
    };
    let base = sender.seq;
    assert_eq!(reply["events_seq"], 7);
    assert_eq!(reply["policy"]["policy"]["max_restarts"], 3);
    assert_eq!(reply["policy"]["digest"].as_str().unwrap().len(), 64);

    // A patch against the acknowledged base; a replay of it, or a patch on an unknown base, is not.
    let monitor = tokio::spawn(app.clone().run_monitor());
    let (signature, body) = sender.body(json!({"base":base,
        "patch":{"system":{"cpu_pct":55.5,"gpus":{"$items":{"1":{"temperature":70}}}}}}));
    assert!(matches!(
        app.ingest_push(&machine.to_string(), &signature, &body)
            .await
            .unwrap(),
        PushReply::Accepted(_)
    ));
    assert!(matches!(
        app.ingest_push(&machine.to_string(), &signature, &body)
            .await,
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        push(&app, &mut sender, json!({"base":1,"patch":{}}))
            .await
            .unwrap(),
        PushReply::Resync(_)
    ));
    // Keep pushing for two monitor ticks: no SSH read may happen meanwhile.
    for cpu in [60.0, 61.0, 62.0] {
        let snapshot = sender.snapshot(cpu);
        assert!(matches!(
            push(&app, &mut sender, json!({"snapshot":snapshot}))
                .await
                .unwrap(),
            PushReply::Accepted(_)
        ));
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    monitor.abort();
    let _ = monitor.await;
    assert_eq!(rig.reads.load(Ordering::SeqCst), 0, "monitor skipped SSH");

    let latest = app.latest(Some(machine)).await.unwrap();
    let system = latest.iter().find(|o| o.kind == "system").unwrap();
    assert_eq!(system.data["cpu_pct"], 62.0);
    assert_eq!(system.data["transport"], "push");
    assert!((chrono::Utc::now() - system.observed_at).num_seconds() < 15);
    let mining = latest.iter().find(|o| o.kind == "mining").unwrap();
    assert_eq!(mining.data["instances"][0]["stats"]["hashrate_hs"], 1000.0);
    let messages = app.messages(machine, 20).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.kind == "watchdog_restart")
            .count(),
        1
    );

    // Another controller run (new epoch) never accepts this run's messages.
    let other = App {
        push: Arc::new(PushHub::new(Some(PushSettings {
            url: None,
            port: 8091,
        }))),
        ..app.clone()
    };
    let snapshot = sender.snapshot(1.0);
    assert!(matches!(
        push(&other, &mut sender, json!({"snapshot":snapshot}))
            .await
            .unwrap(),
        PushReply::Resync(_)
    ));
}
