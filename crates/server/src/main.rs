use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{
    adapters, bmc, pg::PgStore, runtime::ScreenRuntime, ssh::SshExecutor, vault::Vault,
};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let command = std::env::args().nth(1).unwrap_or_else(|| "serve".into());
    if command == "generate-key" {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        println!("{}", STANDARD.encode(bytes));
        return Ok(());
    }
    if command == "openapi" {
        println!(
            "{}",
            serde_json::to_string_pretty(&rig_api::openapi::document())?
        );
        return Ok(());
    }
    let url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let store = Arc::new(PgStore::connect(&url).await?);
    store.migrate().await?;
    if command == "migrate" {
        return Ok(());
    }
    let master = std::env::var("RIGDECK_MASTER_KEY").or_else(|_| {
        std::env::var("RIGDECK_MASTER_KEY_FILE")
            .and_then(|p| std::fs::read_to_string(p).map_err(|_| std::env::VarError::NotPresent))
    })?;
    let vault = Arc::new(Vault::new(master.trim())?);
    if command == "create-admin" {
        let name = std::env::args().nth(2).unwrap_or_else(|| "admin".into());
        let password = std::env::var("RIGDECK_ADMIN_PASSWORD")
            .map_err(|_| anyhow::anyhow!("Set RIGDECK_ADMIN_PASSWORD for this command only"))?;
        if password.len() < 12 {
            anyhow::bail!("Admin password must have at least 12 characters");
        }
        store
            .create_admin(&name, &vault.password_hash(&password)?)
            .await?;
        println!("Administrator {name} created");
        return Ok(());
    }
    if command != "serve" {
        anyhow::bail!("Unknown command: {command}");
    }
    let remote = Arc::new(SshExecutor::default());
    let root = PathBuf::from(std::env::var("RIGDECK_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let runtime = Arc::new(ScreenRuntime::new(remote.clone(), root.join("packages"))?);
    let app = App {
        bios: Some(Arc::new(rig_infrastructure::bios::SumBios {
            root: root.join("bios"),
            binary: PathBuf::from(
                std::env::var("RIGDECK_SUM_PATH").unwrap_or_else(|_| "/opt/supermicro/sum".into()),
            ),
        })),
        repository: store,
        vault,
        remote,
        runtime,
        adapters: Arc::new(adapters::registry()),
        bmc: Arc::new(bmc::registry()),
    };
    let bind = std::env::var("RIGDECK_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let origin = std::env::var("RIGDECK_ORIGIN").unwrap_or_else(|_| "https://rigdeck.local".into());
    let secure_cookie = std::env::var("RIGDECK_INSECURE_LOCALHOST").as_deref() != Ok("1");
    if !secure_cookie
        && (!bind.starts_with("127.0.0.1:")
            || !(origin.starts_with("http://localhost:")
                || origin.starts_with("http://127.0.0.1:")))
    {
        anyhow::bail!("Insecure cookie mode is limited to a loopback bind and origin");
    }
    let router = rig_api::http::router(
        rig_api::http::ApiState {
            app: app.clone(),
            origin,
            secure_cookie,
        },
        &std::env::var("RIGDECK_STATIC_DIR").unwrap_or_else(|_| "frontend/dist".into()),
    );
    let worker = tokio::spawn(app.clone().run_worker());
    let monitor = tokio::spawn(app.run_monitor());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind,"RigDeck started");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        shutdown_signal().await;
    })
    .await?;
    worker.abort();
    monitor.abort();
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
