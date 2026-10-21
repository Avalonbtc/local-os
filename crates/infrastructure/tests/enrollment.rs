use hmac::{Hmac, Mac};
use rig_application::{
    App,
    agent::AgentHub,
    push::{PushHub, PushSettings},
};
use rig_domain::*;
use rig_infrastructure::{adapters, pg::PgStore, ssh::SshExecutor, vault::Vault};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;
use uuid::Uuid;

fn request(token: &str, fingerprint: &str, hostname: &str) -> EnrollRequest {
    EnrollRequest {
        token: token.into(),
        fingerprint: fingerprint.into(),
        hostname: hostname.into(),
        addresses: vec!["10.9.8.7".into()],
        version: "0.5.0".into(),
    }
}
fn fingerprint() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[tokio::test]
#[ignore = "requires isolated rigdeck_test database"]
async fn install_keys_enroll_reinstall_limit_revoke_and_authenticate_agents() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/rigdeck_test"));
    let store = Arc::new(PgStore::connect(&url).await.unwrap());
    store.migrate().await.unwrap();
    let admin = format!("enroll-{}", Uuid::new_v4());
    store.create_admin(&admin, "unused").await.unwrap();
    let (id, _) = store.password_hash(&admin).await.unwrap().unwrap();
    let actor = Actor {
        id,
        name: admin,
        token_id: None,
    };
    let vault = Arc::new(Vault::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap());
    let app = App {
        repository: store.clone(),
        vault: vault.clone(),
        remote: Arc::new(SshExecutor::default()),
        runtime: Arc::new(
            rig_infrastructure::runtime::ScreenRuntime::new(
                Arc::new(SshExecutor::default()),
                std::env::temp_dir().join("rigdeck-enroll-unused"),
            )
            .unwrap(),
        ),
        adapters: Arc::new(adapters::registry()),
        push: Arc::new(PushHub::new(Some(PushSettings {
            url: Some("https://panel.example.com/ingest/v1/push".into()),
            port: 0,
        }))),
        agents: Arc::new(AgentHub::new("https://panel.example.com", None)),
    };
    let host = format!("rig{}", &Uuid::new_v4().simple().to_string()[..8]);

    let token = app
        .create_enrollment_token(
            &actor,
            EnrollmentTokenInput {
                name: "batch".into(),
                group: "farm-b".into(),
                tags: vec!["gpu".into()],
                expires_in_hours: Some(24),
                max_uses: Some(2),
            },
        )
        .await
        .unwrap();
    assert!(token.token.starts_with("rgk_") && token.token.len() == 68);
    assert_eq!(
        token.command,
        format!(
            "curl -fsSL https://panel.example.com/agent/install.sh | sudo bash -s -- {}",
            token.token
        )
    );
    // The key is re-derived, so listing shows it again.
    let listed = app.enrollment_tokens().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|t| t.id == token.id && t.token == token.token)
    );

    let first = fingerprint();
    let joined = app
        .enroll(request(&token.token, &first, &host), "203.0.113.5")
        .await
        .unwrap();
    assert!(joined.created);
    assert_eq!(joined.name, host);
    assert_eq!(
        joined.connect_url,
        "wss://panel.example.com/agent/v1/connect"
    );
    assert_eq!(joined.key, vault.push_key(joined.machine_id).unwrap());
    let target = app.push_target(joined.machine_id).unwrap();
    assert_eq!(
        joined.push,
        Some(
            json!({"url":"https://panel.example.com/ingest/v1/push","machine_id":joined.machine_id,"key":target.key,"digest":target.digest})
        )
    );
    let machine = app.machine(joined.machine_id).await.unwrap();
    assert!(machine.is_agent());
    assert_eq!(
        (
            machine.host.as_str(),
            machine.group.as_str(),
            machine.tags.clone()
        ),
        ("10.9.8.7", "farm-b", vec!["gpu".to_string()])
    );
    // The placeholder credential decrypts, so the runtime layer can pass it through.
    assert!(app.credential(machine.id).await.is_ok());

    // Reinstalling the same rig keeps its machine (and uses the second slot).
    let again = app
        .enroll(request(&token.token, &first, "renamed"), "203.0.113.5")
        .await
        .unwrap();
    assert_eq!(
        (again.machine_id, again.created),
        (joined.machine_id, false)
    );
    // Both uses are spent.
    let full = app
        .enroll(request(&token.token, &fingerprint(), &host), "203.0.113.5")
        .await;
    assert!(
        matches!(full, Err(Error::Forbidden(ref m)) if m.contains("安装密钥")),
        "{full:?}"
    );

    // A second key; the same hostname gets a suffix, an address-less rig its source address.
    let other = app
        .create_enrollment_token(
            &actor,
            EnrollmentTokenInput {
                name: "open".into(),
                group: String::new(),
                tags: vec![],
                expires_in_hours: None,
                max_uses: None,
            },
        )
        .await
        .unwrap();
    let mut second = request(&other.token, &fingerprint(), &host);
    second.addresses.clear();
    let twin = app.enroll(second, "198.51.100.9").await.unwrap();
    assert_eq!(twin.name, format!("{host}-2"));
    assert_eq!(
        app.machine(twin.machine_id).await.unwrap().host,
        "198.51.100.9"
    );

    // Forged or malformed keys and identities.
    let mut forged = other.token.clone();
    forged.replace_range(
        forged.len() - 1..,
        if forged.ends_with('0') { "1" } else { "0" },
    );
    for (key, print) in [
        (forged.as_str(), fingerprint()),
        ("rgk_short", fingerprint()),
        (other.token.as_str(), "NOT-HEX".to_string()),
    ] {
        assert!(
            app.enroll(request(key, &print, "x"), "198.51.100.9")
                .await
                .is_err()
        );
    }
    app.revoke_enrollment_token(&actor, other.id).await.unwrap();
    assert!(matches!(
        app.enroll(request(&other.token, &fingerprint(), "x"), "198.51.100.9")
            .await,
        Err(Error::Forbidden(_))
    ));

    // Editing an agent machine renames it but cannot repoint its connection.
    let edited = app
        .save_machine(
            &actor,
            machine.id,
            serde_json::from_value(json!({"name":format!("{host}-edited"),"host":"192.0.2.66","port":2222,"username":"evil","host_key":"SHA256:x","group":"g2","policy":{}})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edited.name, format!("{host}-edited"));
    assert_eq!(
        (
            edited.host.as_str(),
            edited.port,
            edited.username.as_str(),
            edited.group.as_str()
        ),
        ("10.9.8.7", 22, "root", "g2")
    );

    // The agent's answer to the connection challenge.
    let nonce = "abc123";
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(&hex::decode(&joined.key).unwrap()).unwrap();
    mac.update(format!("rigdeck-agent/v1|{}|{nonce}", machine.id).as_bytes());
    let answer = hex::encode(mac.finalize().into_bytes());
    assert_eq!(
        app.authenticate_agent(machine.id, nonce, &answer)
            .await
            .unwrap()
            .id,
        machine.id
    );
    assert!(matches!(
        app.authenticate_agent(machine.id, "other-nonce", &answer)
            .await,
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        app.authenticate_agent(twin.machine_id, nonce, &answer)
            .await,
        Err(Error::Unauthorized)
    ));
}
