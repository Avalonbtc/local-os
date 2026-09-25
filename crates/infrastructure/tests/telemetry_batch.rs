use chrono::{Duration, Utc};
use rig_domain::*;
use rig_infrastructure::pg::PgStore;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to an isolated rigdeck_test database"]
async fn batched_observations_events_messages_and_deltas() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/rigdeck_test"));
    let store = PgStore::connect(&url).await.unwrap();
    store.migrate().await.unwrap();
    let name = format!("batch-{}", Uuid::new_v4());
    store.create_admin(&name, "unused").await.unwrap();
    let (user, _) = store.password_hash(&name).await.unwrap().unwrap();
    let actor = Actor {
        id: user,
        name,
        token_id: None,
    };
    let machine = Uuid::new_v4();
    let input: MachineInput = serde_json::from_value(json!({"name":format!("batch-{machine}"),"host":"127.0.0.1","port":22,"username":"root","host_key":"SHA256:fixture","policy":{}})).unwrap();
    store
        .save_machine(machine, &input, Some("secret"), None, &actor)
        .await
        .unwrap();

    let now = Utc::now();
    let before = now - Duration::seconds(30);
    let sample = |kind: &str, at, data| Observation {
        machine_id: machine,
        kind: kind.into(),
        observed_at: at,
        data,
        error: None,
    };
    // Two samples for the same machine/kind in one batch must not violate the single upsert,
    // and the newest one wins; the old runtime's inventory keys are split into `hardware`.
    store
        .observe_many(&[
            sample(
                "system",
                now,
                json!({"cpu_pct": 50, "topology": [{"cpu": 0}], "cpu_model": "EPYC"}),
            ),
            sample("system", before, json!({"cpu_pct": 10})),
            sample("mining", now, json!({"instances": []})),
        ])
        .await
        .unwrap();
    let latest = store.latest(Some(machine)).await.unwrap();
    let system = latest.iter().find(|o| o.kind == "system").unwrap();
    assert_eq!(system.data["cpu_pct"], 50);
    assert_eq!(system.data["cpu_model"], "EPYC");
    let summary = store.latest_summary(Some(machine)).await.unwrap();
    assert!(summary.iter().all(|o| o.data.get("topology").is_none()));
    let history = store.history(machine, "system", 1).await.unwrap();
    assert_eq!(
        history.len(),
        2,
        "every sample is kept in the metrics history"
    );

    // Runtime events are idempotent (the rig re-sends its tail every snapshot).
    let event = json!({"id": Uuid::new_v4(), "seq": 1, "at": now.timestamp() as f64, "level": "warning", "kind": "watchdog_restart", "instance": "xmr", "message": "看门狗重启矿工"});
    assert_eq!(
        store
            .record_events(machine, &[event.clone(), json!({"id":"not-a-uuid"})])
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.record_events(machine, &[event]).await.unwrap(), 0);

    let job = store
        .enqueue(
            &JobInput {
                machine_ids: vec![machine],
                action: Action::Bootstrap,
                idempotency_key: Uuid::new_v4().to_string(),
                concurrency: 1,
                canary: false,
                include_controller: false,
            },
            &json!({"kind":"bootstrap"}),
            &actor,
        )
        .await
        .unwrap();
    let messages = store.messages(machine, 50).await.unwrap();
    assert!(messages.iter().any(|m| m.source == "runtime"
        && m.level == "warning"
        && m.instance.as_deref() == Some("xmr")));
    assert!(
        messages
            .iter()
            .any(|m| m.source == "job" && m.message.starts_with("部署运行层"))
    );
    // With no jobs page, the message carries what cancel / reconcile and the output view need.
    let job_message = messages.iter().find(|m| m.source == "job").unwrap();
    let detail = job_message.detail.as_ref().unwrap();
    assert!(detail["target_id"].is_string() && detail["job_id"].is_string());
    assert!(detail.get("output").is_some() && detail["output_truncated"] == false);

    // Header chips: closing one hides only that one; the bin clears all; history stays.
    let unread = store.unread_messages(machine, 50).await.unwrap();
    assert_eq!(unread.len(), messages.len().min(50));
    let first = format!("{}:{}", unread[0].source, unread[0].id);
    store.dismiss_messages(machine, Some(&first)).await.unwrap();
    let after = store.unread_messages(machine, 50).await.unwrap();
    assert_eq!(after.len(), unread.len() - 1);
    assert!(
        after
            .iter()
            .all(|m| format!("{}:{}", m.source, m.id) != first)
    );
    store.dismiss_messages(machine, None).await.unwrap();
    assert!(store.unread_messages(machine, 50).await.unwrap().is_empty());
    assert_eq!(
        store.messages(machine, 50).await.unwrap().len(),
        messages.len()
    );
    assert!(
        messages
            .iter()
            .any(|m| m.source == "audit" && m.kind == "fleet.save")
    );
    assert!(
        messages.windows(2).all(|w| w[0].at >= w[1].at),
        "newest first"
    );

    // The polled job list never carries per-target output or compiled snapshots.
    sqlx::query("UPDATE job_targets SET output='large output' WHERE job_id=$1")
        .bind(job)
        .execute(&store.pool)
        .await
        .unwrap();
    let listed = store
        .jobs(250)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id == job)
        .unwrap();
    assert_eq!(listed.targets[0].output, "");
    assert_eq!(
        store.job(job).await.unwrap().targets[0].output,
        "large output"
    );
    store.cancel_job(job, &actor).await.unwrap();
}
