use rig_domain::{BmcConfig, BmcProvider, Credential};
use rig_infrastructure::bmc::Redfish;

#[tokio::test]
#[ignore = "run through python tests/redfish/run.py (isolated TLS fixture)"]
async fn redfish_tls_capabilities_and_power_contract() {
    let config = BmcConfig {
        url: std::env::var("TEST_REDFISH_URL").unwrap(),
        username: "test".into(),
        provider: "redfish".into(),
        ca_pem: Some(std::fs::read_to_string(std::env::var("TEST_REDFISH_CA").unwrap()).unwrap()),
    };
    assert!(config.url.starts_with("https://127.0.0.1:"));
    let secret = Credential::Password {
        password: "test-only".into(),
        sudo_password: None,
    };
    let provider = Redfish;
    let power = provider.read(&config, &secret, "power").await.unwrap();
    assert_eq!(power["state"], "On");
    let sensors = provider.read(&config, &secret, "sensors").await.unwrap();
    assert_eq!(sensors["items"].as_array().unwrap().len(), 2);
    assert_eq!(sensors["items"][0]["reading"], 55);
    assert!(sensors["items"][1]["reading"].is_null());
    assert_eq!(
        provider.read(&config, &secret, "events").await.unwrap()["items"][0]["Message"],
        "Test-only event"
    );
    assert!(provider.power(&config, &secret, "force_off").await.is_err());
    let result = provider.power(&config, &secret, "shutdown").await.unwrap();
    assert_eq!(result["request_accepted"], true);
    assert_eq!(result["task_monitor"], "/tasks/1");
    let untrusted = BmcConfig {
        ca_pem: None,
        ..config.clone()
    };
    assert!(provider.read(&untrusted, &secret, "power").await.is_err());
    let wrong = Credential::Password {
        password: "wrong".into(),
        sudo_password: None,
    };
    assert!(provider.read(&config, &wrong, "power").await.is_err());
}
