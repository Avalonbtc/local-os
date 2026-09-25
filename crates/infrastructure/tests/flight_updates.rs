use rig_domain::*;
use rig_infrastructure::pg::PgStore;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires isolated rigdeck_test database"]
async fn flight_edit_enqueues_snapshot_atomically_and_rejects_stale_edits() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = PgStore::connect(&url).await.unwrap();
    store.migrate().await.unwrap();
    let name = Uuid::new_v4().to_string();
    store.create_admin(&name, "unused").await.unwrap();
    let actor = Actor {
        id: store.password_hash(&name).await.unwrap().unwrap().0,
        name: name.clone(),
        token_id: None,
    };
    let machine = Uuid::new_v4();
    let input: MachineInput = serde_json::from_value(json!({"name":name,"host":"127.0.0.1","port":22,"username":"test","host_key":"SHA256:test","group":"","tags":[],"is_controller":false,"policy":{}})).unwrap();
    store
        .save_machine(machine, &input, Some("fixture"), None, &actor)
        .await
        .unwrap();
    let wallet = store
        .save_catalog(
            CatalogKind::Wallet,
            Uuid::new_v4(),
            &CatalogInput {
                name: name.clone(),
                data: json!({"coin_symbol":name,"address":"fixture"}),
                expected_revision: None,
            },
            &actor,
        )
        .await
        .unwrap();
    let sheet_id = Uuid::new_v4();
    let mut flight = FlightInput {
        name: name.clone(),
        expected_version: None,
        tasks: vec![FlightTask {
            instance: "cpu-1".into(),
            wallet_id: wallet.id,
            miner: json!({"adapter":"hive-custom","name":"pkg","url":"https://example.com/pkg.tar.gz"}),
            config: json!({"user_config":"old"}),
        }],
    };
    store
        .save_flight(sheet_id, &flight, &actor, None)
        .await
        .unwrap();
    // The miner is stored inline with the task (no miner catalog row).
    assert_eq!(
        store.flight(sheet_id).await.unwrap().tasks[0].miner["name"],
        "pkg"
    );
    let job_input = JobInput {
        machine_ids: vec![machine],
        action: Action::Apply { sheet_id },
        idempotency_key: name.clone(),
        concurrency: 1,
        canary: false,
        include_controller: false,
    };
    let snapshot = json!({"kind":"apply","snapshot":{"hosts":{machine.to_string():{"sheet_id":sheet_id,"sheet_version":1}}}});
    let old_job = store.enqueue(&job_input, &snapshot, &actor).await.unwrap();
    assert_eq!(store.flight_targets(sheet_id).await.unwrap(), vec![machine]);
    flight.expected_version = Some(1);
    flight.tasks[0].config = json!({"user_config":"new"});
    let propagation = FlightPropagation {
        job: JobInput {
            idempotency_key: format!("{name}-edit"),
            ..job_input.clone()
        },
        snapshot: json!({"kind":"apply","snapshot":{"hosts":{machine.to_string():{"sheet_id":sheet_id,"sheet_version":2,"instances":[{"user_config":"new"}]}}}}),
    };
    let saved = store
        .save_flight(sheet_id, &flight, &actor, Some(&propagation))
        .await
        .unwrap();
    assert_eq!(saved.version, 2);
    let job = store.job(saved.apply_job_id.unwrap()).await.unwrap();
    assert_eq!(
        job.action["snapshot"]["hosts"][machine.to_string()]["sheet_version"],
        2
    );
    assert_eq!(
        store.job(old_job).await.unwrap().action["snapshot"]["hosts"][machine.to_string()]["sheet_version"],
        1
    );
    assert!(matches!(
        store
            .save_flight(sheet_id, &flight, &actor, Some(&propagation))
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(store.flight(sheet_id).await.unwrap().version, 2);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE actor_id=$1")
        .bind(actor.id)
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let other = Uuid::new_v4();
    let switched = JobInput {
        action: Action::Apply { sheet_id: other },
        idempotency_key: format!("{name}-switch"),
        ..job_input
    };
    store
        .enqueue(
            &switched,
            &json!({"kind":"apply","snapshot":{"hosts":{machine.to_string():{"sheet_id":other}}}}),
            &actor,
        )
        .await
        .unwrap();
    flight.expected_version = Some(2);
    assert!(matches!(
        store
            .save_flight(sheet_id, &flight, &actor, Some(&propagation))
            .await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(store.flight(sheet_id).await.unwrap().version, 2);
}
