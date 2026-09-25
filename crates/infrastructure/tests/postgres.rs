use rig_domain::*;
use rig_infrastructure::pg::PgStore;
use serde_json::json;
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated rigdeck_test database"]
async fn postgres_migrations_catalog_queue_leases_and_retention() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(
        url.ends_with("/rigdeck_test"),
        "integration tests require the dedicated rigdeck_test database"
    );
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    let name = format!("test-{}", Uuid::new_v4());
    store
        .create_admin(&name, "not-used-by-login-tests")
        .await
        .unwrap();
    let (id, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name,
        token_id: None,
    };
    let symbol = format!("COIN-{}", Uuid::new_v4());
    let mut saves = Vec::new();
    for n in 0..4 {
        let store = store.clone();
        let actor = actor.clone();
        let symbol = symbol.clone();
        saves.push(tokio::spawn(async move{store.save_catalog(CatalogKind::Wallet,Uuid::new_v4(),&CatalogInput{name:format!("wallet-{n}"),data:json!({"coin_symbol":symbol,"address":"pool-account-no-format-restriction"}),expected_revision:None},&actor).await.unwrap()}));
    }
    let mut wallets = Vec::new();
    for task in saves {
        wallets.push(task.await.unwrap());
    }
    assert!(
        wallets
            .iter()
            .all(|w| w.data["coin_id"] == wallets[0].data["coin_id"])
    );
    let coin_id: Uuid = wallets[0].data["coin_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(matches!(
        store
            .delete_catalog(CatalogKind::Coin, coin_id, &actor)
            .await,
        Err(Error::Conflict(_))
    ));
    let wallet = &wallets[0];
    let edit = CatalogInput {
        name: "changed".into(),
        data: wallet.data.clone(),
        expected_revision: Some(wallet.revision),
    };
    store
        .save_catalog(CatalogKind::Wallet, wallet.id, &edit, &actor)
        .await
        .unwrap();
    assert!(matches!(
        store
            .save_catalog(CatalogKind::Wallet, wallet.id, &edit, &actor)
            .await,
        Err(Error::Conflict(_))
    ));
    let mut hosts = Vec::new();
    for index in 0..8 {
        let id = Uuid::new_v4();
        store
            .save_machine(
                id,
                &MachineInput {
                    name: format!("test-{id}-{index}"),
                    host: "127.0.0.1".into(),
                    port: 22,
                    username: "test".into(),
                    host_key: "SHA256:test-only".into(),
                    group: "integration".into(),
                    tags: vec![],
                    is_controller: false,
                    bmc: None,
                    policy: json!({}),
                    credential: None,
                    bmc_credential: None,
                    sudo_password: None,
                },
                Some("encrypted-test-fixture"),
                None,
                &actor,
            )
            .await
            .unwrap();
        hosts.push(id);
    }
    let mut request = JobInput {
        machine_ids: hosts.clone(),
        action: Action::Command {
            script: "printf test".into(),
            timeout_seconds: 10,
        },
        idempotency_key: Uuid::new_v4().to_string(),
        concurrency: 4,
        canary: true,
        include_controller: false,
    };
    let snapshot = json!({"kind":"command","script":"printf test","timeout_seconds":10});
    let job = store.enqueue(&request, &snapshot, &actor).await.unwrap();
    assert_eq!(
        store.enqueue(&request, &snapshot, &actor).await.unwrap(),
        job
    );
    request.concurrency = 8;
    assert!(matches!(
        store.enqueue(&request, &snapshot, &actor).await,
        Err(Error::Conflict(_))
    ));
    request.concurrency = 4;
    // The rejected enqueue above rolls back when its pooled connection is recycled, which can
    // briefly keep the queue's advisory lock; claim() then yields None and the scheduler retries.
    let mut first = None;
    for _ in 0..50 {
        first = store.claim().await.unwrap();
        if first.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let first = first.unwrap();
    assert_eq!(first.target.ordinal, 0);
    assert!(store.claim().await.unwrap().is_none());
    assert!(!first.reconcile);
    let done = RemoteOperation {
        status: "succeeded".into(),
        output: "done".into(),
        truncated: false,
        result: Some(json!({"exit_code":0})),
        error: None,
    };
    store
        .finish(first.target.id, first.lease, "succeeded", &done)
        .await
        .unwrap();
    let mut active = Vec::new();
    let mut all = HashSet::from([first.target.id]);
    for _ in 0..4 {
        let task = store.claim().await.unwrap().unwrap();
        assert!(all.insert(task.target.id));
        active.push(task);
    }
    assert!(store.claim().await.unwrap().is_none());
    let expired = active.remove(0);
    sqlx::query("UPDATE job_targets SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(expired.target.id)
        .execute(&store.pool)
        .await
        .unwrap();
    store.expire_leases().await.unwrap();
    let resumed = store.claim().await.unwrap().unwrap();
    assert!(resumed.reconcile);
    assert_eq!(resumed.target.operation_id, expired.target.operation_id);
    assert_ne!(resumed.lease, expired.lease);
    assert!(
        store
            .heartbeat(expired.target.id, expired.lease)
            .await
            .is_err()
    );
    store
        .finish(
            resumed.target.id,
            resumed.lease,
            "unknown",
            &RemoteOperation {
                status: "unknown".into(),
                ..done.clone()
            },
        )
        .await
        .unwrap();
    let locked: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM machine_locks WHERE target_id=$1)")
            .bind(resumed.target.id)
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(locked);
    store
        .resolve_target(
            resumed.target.id,
            &ResolveTarget {
                decision: "reconcile".into(),
                note: "test remote operation persisted".into(),
            },
            &actor,
        )
        .await
        .unwrap();
    let reconciled = store.claim().await.unwrap().unwrap();
    assert!(reconciled.reconcile);
    assert_eq!(reconciled.target.operation_id, expired.target.operation_id);
    active.push(reconciled);
    let mut completions = Vec::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(active.len()));
    for task in active {
        let store = store.clone();
        let done = done.clone();
        let barrier = barrier.clone();
        completions.push(tokio::spawn(async move {
            barrier.wait().await;
            store
                .finish(task.target.id, task.lease, "succeeded", &done)
                .await
                .unwrap();
        }));
    }
    for task in completions {
        task.await.unwrap();
    }
    let mut parallel = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        parallel.push(tokio::spawn(async move {
            for _ in 0..10 {
                if let Some(task) = store.claim().await.unwrap() {
                    return Some(task);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            None
        }));
    }
    let mut count = 0;
    let mut final_tasks = Vec::new();
    for future in parallel {
        if let Some(task) = future.await.unwrap() {
            assert!(all.insert(task.target.id));
            count += 1;
            final_tasks.push(task);
        }
    }
    assert_eq!(count, 3);
    let barrier = Arc::new(tokio::sync::Barrier::new(final_tasks.len()));
    let mut completions = Vec::new();
    for task in final_tasks {
        let store = store.clone();
        let done = done.clone();
        let barrier = barrier.clone();
        completions.push(tokio::spawn(async move {
            barrier.wait().await;
            store
                .finish(task.target.id, task.lease, "succeeded", &done)
                .await
                .unwrap();
        }));
    }
    for task in completions {
        task.await.unwrap();
    }
    assert_eq!(all.len(), 8);
    assert_eq!(store.job(job).await.unwrap().status, "succeeded");
    let observed = Observation {
        machine_id: hosts[0],
        kind: "system".into(),
        observed_at: chrono::Utc::now() - chrono::Duration::minutes(2),
        data: json!({"cpu_pct":25.0,"memory_pct":50.0,"topology":[{"cpu":0}],"board":"fixture","interfaces":[{"name":"eth0"}],"disks":[{"mount":"/"}],"network":{"eth0":{"rx_bytes":1}}}),
        error: None,
    };
    store.observe(&observed).await.unwrap();
    let raw: serde_json::Value = sqlx::query_scalar("SELECT data FROM metrics WHERE machine_id=$1 AND kind='system' ORDER BY observed_at DESC LIMIT 1").bind(hosts[0]).fetch_one(&store.pool).await.unwrap();
    assert!(raw.get("topology").is_none());
    assert!(raw.get("interfaces").is_none());
    assert!(raw.get("disks").is_none() && raw.get("network").is_none());
    let latest = store.latest(Some(hosts[0])).await.unwrap();
    assert_eq!(
        latest.iter().find(|o| o.kind == "system").unwrap().data["board"],
        "fixture"
    );
    let summary = store.latest_summary(Some(hosts[0])).await.unwrap();
    assert!(
        summary
            .iter()
            .find(|o| o.kind == "system")
            .unwrap()
            .data
            .get("topology")
            .is_none()
    );
    let next = Observation {
        observed_at: chrono::Utc::now(),
        data: json!({"cpu_pct":26,"runtime_version":"0.1.7"}),
        ..observed.clone()
    };
    store.observe(&next).await.unwrap();
    let latest = store.latest(Some(hosts[0])).await.unwrap();
    assert_eq!(
        latest.iter().find(|o| o.kind == "system").unwrap().data["board"],
        "fixture"
    );
    assert_eq!(
        latest.iter().find(|o| o.kind == "system").unwrap().data["runtime_version"],
        "0.1.7"
    );
    store.maintain().await.unwrap();
    let minutes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM metric_minutes WHERE machine_id=$1")
            .bind(hosts[0])
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert_eq!(minutes, 1);
    sqlx::query("INSERT INTO metric_minutes(machine_id,kind,observed_at,data) VALUES($1,'expired-test',now()-interval '100 days','{}')").bind(hosts[0]).execute(&store.pool).await.unwrap();
    sqlx::query("UPDATE job_targets SET output='expired-output',finished_at=now()-interval '31 days' WHERE job_id=$1").bind(job).execute(&store.pool).await.unwrap();
    sqlx::query("INSERT INTO audit_events(actor,action,target,created_at) VALUES($1,'expired-test','test',now()-interval '181 days')").bind(&actor.name).execute(&store.pool).await.unwrap();
    store.maintain().await.unwrap();
    let old_metrics: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM metric_minutes WHERE machine_id=$1 AND kind='expired-test'",
    )
    .bind(hosts[0])
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(old_metrics, 0);
    assert!(
        store
            .job(job)
            .await
            .unwrap()
            .targets
            .iter()
            .all(|t| t.output.is_empty() && t.output_truncated)
    );
    let old_audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE actor=$1 AND action='expired-test'",
    )
    .bind(&actor.name)
    .fetch_one(&store.pool)
    .await
    .unwrap();
    assert_eq!(old_audit, 0);
    request.idempotency_key = Uuid::new_v4().to_string();
    let cancelled = store.enqueue(&request, &snapshot, &actor).await.unwrap();
    store.cancel_job(cancelled, &actor).await.unwrap();
    assert_eq!(store.job(cancelled).await.unwrap().status, "cancelled");
    assert!(!store.audit_events(1000).await.unwrap().is_empty());
    // Completed job history survives retiring its machine; list no longer exposes it.
    store.delete_machine(hosts[0], &actor).await.unwrap();
    assert!(matches!(
        store.machine(hosts[0]).await,
        Err(Error::NotFound)
    ));
    assert_eq!(store.job(job).await.unwrap().targets.len(), 8);
    assert!(
        !store
            .machines()
            .await
            .unwrap()
            .iter()
            .any(|m| m.id == hosts[0])
    );
    assert!(store.latest(Some(hosts[0])).await.unwrap().is_empty());
    store.pool.close().await;
    assert!(store.enqueue(&request, &snapshot, &actor).await.is_err());
}
