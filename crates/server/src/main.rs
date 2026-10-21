use base64::{Engine, engine::general_purpose::STANDARD};
use rand::RngCore;
use rig_application::App;
use rig_domain::*;
use rig_infrastructure::{
    adapters, pg::PgStore, runtime::ScreenRuntime, ssh::SshExecutor, vault::Vault,
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
    let bind = std::env::var("RIGDECK_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let origin = std::env::var("RIGDECK_ORIGIN").unwrap_or_else(|_| "https://rigdeck.local".into());
    // The address rigs use to reach this controller: install command, agent channel, pushes.
    let public_url = std::env::var("RIGDECK_PUBLIC_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| origin.clone())
        .trim_end_matches('/')
        .to_owned();
    if !(public_url.starts_with("https://") || public_url.starts_with("http://")) {
        anyhow::bail!("RIGDECK_PUBLIC_URL (or RIGDECK_ORIGIN) must be an http(s) URL");
    }
    let root = PathBuf::from(std::env::var("RIGDECK_DATA_DIR").unwrap_or_else(|_| "data".into()));
    // Agent machines are reached through the connection their rig keeps open; the rest over SSH.
    let agents = Arc::new(rig_application::agent::AgentHub::new(
        &public_url,
        Some(rig_application::agent::Installer {
            script: rig_infrastructure::runtime::agent_installer(&public_url),
            runtime: rig_infrastructure::runtime::runtime_source(),
        }),
    ));
    let remote = Arc::new(rig_application::agent::RoutedExecutor {
        ssh: Arc::new(SshExecutor::default()),
        agent: Arc::new(rig_application::agent::AgentExecutor {
            hub: agents.clone(),
        }),
    });
    let runtime = Arc::new(ScreenRuntime::new(remote.clone(), root.join("packages"))?);
    // Telemetry push. By default rigs push to <public URL>/ingest/v1/push. RIGDECK_INGEST_BIND
    // adds a separate plain-HTTP listener for a LAN; "off" reads every rig on demand instead.
    let ingest_setting = std::env::var("RIGDECK_INGEST_BIND").unwrap_or_default();
    let ingest_bind =
        (!ingest_setting.is_empty() && ingest_setting != "off").then_some(ingest_setting.clone());
    let ingest_url = std::env::var("RIGDECK_INGEST_URL")
        .ok()
        .filter(|url| !url.is_empty());
    if let Some(url) = &ingest_url
        && !(url.starts_with("http://") || url.starts_with("https://"))
    {
        anyhow::bail!("RIGDECK_INGEST_URL must be an http(s) URL");
    }
    let push = match (&ingest_bind, ingest_setting.as_str()) {
        (_, "off") => None,
        (Some(bind), _) => Some(rig_application::push::PushSettings {
            url: ingest_url,
            port: bind
                .parse::<std::net::SocketAddr>()
                .map_err(|_| anyhow::anyhow!("RIGDECK_INGEST_BIND must be host:port or off"))?
                .port(),
        }),
        (None, _) => Some(rig_application::push::PushSettings {
            url: Some(ingest_url.unwrap_or_else(|| format!("{public_url}/ingest/v1/push"))),
            port: 0,
        }),
    };
    let app = App {
        push: Arc::new(rig_application::push::PushHub::new(push)),
        agents,
        repository: store,
        vault,
        remote,
        runtime,
        adapters: Arc::new(adapters::registry()),
    };
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
    let monitor = tokio::spawn(app.clone().run_monitor());
    let ingest = match ingest_bind {
        Some(bind) => {
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!(%bind, "telemetry ingest listening");
            let router = rig_api::http::ingest_router(app.clone());
            Some(tokio::spawn(async move {
                if let Err(error) = axum::serve(listener, router).await {
                    tracing::error!(%error, "telemetry ingest stopped");
                }
            }))
        }
        None => None,
    };
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
    if let Some(ingest) = ingest {
        ingest.abort();
    }
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
