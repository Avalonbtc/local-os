use rig_domain::*;
use rig_infrastructure::ssh::SshExecutor;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable SSH fixture on 127.0.0.1:22239"]
async fn password_sudo_terminal_does_not_leak_secret_and_stream_upload_works() {
    let remote = SshExecutor::default();
    let key = remote.probe_host_key("127.0.0.1", 22239).await.unwrap();
    let machine: Machine = serde_json::from_value(serde_json::json!({
        "id":Uuid::new_v4(),"name":"fixture","host":"127.0.0.1","port":22239,
        "username":"fixture","host_key":key.fingerprint,"group":"test","tags":[],
        "is_controller":false,"bmc":null,"policy":{},"created_at":chrono::Utc::now()
    }))
    .unwrap();
    let credential = Credential::Password {
        password: "fixture-only-password".into(),
        sudo_password: None,
    };
    let (_input, rx) = mpsc::channel(4);
    let (tx, mut output) = mpsc::channel(64);
    tokio::time::timeout(
        Duration::from_secs(40),
        remote.terminal_root(&machine, &credential, "id -u", rx, tx),
    )
    .await
    .unwrap()
    .unwrap();
    let mut bytes = Vec::new();
    while let Some(message) = output.recv().await {
        if let TerminalOutput::Data(data) = message {
            bytes.extend(data);
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains('0'), "{text}");
    assert!(!text.contains("fixture-only-password"));
    let invalid = Credential::Password {
        password: "fixture-only-password".into(),
        sudo_password: Some("wrong-sudo-secret".into()),
    };
    let (_input, rx) = mpsc::channel(4);
    let (tx, _out) = mpsc::channel(64);
    let error = remote
        .terminal_root(&machine, &invalid, "id -u", rx, tx)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("wrong-sudo-secret"));
    let local = std::env::temp_dir().join(format!("rig-upload-{}", Uuid::new_v4()));
    tokio::fs::write(&local, vec![42u8; 3 * 1024 * 1024])
        .await
        .unwrap();
    remote
        .upload_file(&machine, &credential, "/tmp/upload-fixture", &local, 0o600)
        .await
        .unwrap();
    let result = remote
        .execute(&machine, &credential, "wc -c < /tmp/upload-fixture", 10)
        .await
        .unwrap();
    assert_eq!(result.stdout.trim(), "3145728");
    tokio::fs::remove_file(local).await.unwrap();
}
