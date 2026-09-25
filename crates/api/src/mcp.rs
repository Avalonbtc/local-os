use rig_application::App;
use rig_domain::*;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::result::Result;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct RigMcp {
    app: App,
    tool_router: ToolRouter<Self>,
}
#[derive(Deserialize, JsonSchema)]
pub struct ById {
    pub id: Uuid,
}
#[derive(Deserialize, JsonSchema)]
pub struct CatalogList {
    pub kind: CatalogKind,
}
#[derive(Deserialize, JsonSchema)]
pub struct CatalogSave {
    pub kind: CatalogKind,
    pub id: Option<Uuid>,
    pub input: CatalogInput,
}
#[derive(Deserialize, JsonSchema)]
pub struct FlightSave {
    pub id: Option<Uuid>,
    pub input: FlightInput,
}
#[derive(Deserialize, JsonSchema)]
pub struct MachineSave {
    pub id: Option<Uuid>,
    pub input: MachineInput,
}
#[derive(Deserialize, JsonSchema)]
pub struct TelemetryRequest {
    pub machine_id: Option<Uuid>,
}
#[derive(Deserialize, JsonSchema)]
pub struct MessagesRequest {
    pub machine_id: Uuid,
    pub limit: Option<i64>,
}
#[derive(Deserialize, JsonSchema)]
pub struct MinerLogRequest {
    pub machine_id: Uuid,
    pub instance: String,
    pub lines: Option<u32>,
}
#[derive(Deserialize, JsonSchema)]
pub struct BmcRead {
    pub machine_id: Uuid,
    pub kind: String,
}

fn response<T: Serialize>(
    result: rig_domain::Result<T>,
) -> std::result::Result<CallToolResult, ErrorData> {
    match result {
        Ok(value) => Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&value)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
        )])),
        Err(error) => Ok(CallToolResult::error(vec![Content::text(
            error.to_string(),
        )])),
    }
}
fn actor(ctx: &RequestContext<RoleServer>) -> std::result::Result<Actor, ErrorData> {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Session>())
        .map(|s| s.actor.clone())
        .ok_or_else(|| ErrorData::invalid_request("Authenticated request context missing", None))
}
impl RigMcp {
    pub fn new(app: App) -> Self {
        Self {
            app,
            tool_router: Self::tool_router(),
        }
    }
}
#[tool_router]
impl RigMcp {
    #[tool(
        description = "List managed Ubuntu machines, groups and capabilities. No credential values are returned."
    )]
    async fn fleet_list(&self) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.machines().await)
    }
    #[tool(
        description = "Add or update a machine. SSH SHA256 host fingerprint must be verified out of band."
    )]
    async fn fleet_save(
        &self,
        Parameters(p): Parameters<MachineSave>,
        ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .save_machine(&actor(&ctx)?, p.id.unwrap_or_else(Uuid::new_v4), p.input)
                .await,
        )
    }
    #[tool(description = "List coins, wallets, pools or miner package definitions.")]
    async fn catalog_list(
        &self,
        Parameters(p): Parameters<CatalogList>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.catalog(p.kind).await)
    }
    #[tool(
        description = "Create/update a catalog entry. Wallet data may include coin_symbol to create an arbitrary custom coin atomically."
    )]
    async fn catalog_save(
        &self,
        Parameters(p): Parameters<CatalogSave>,
        ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .save_catalog(
                    &actor(&ctx)?,
                    p.kind,
                    p.id.unwrap_or_else(Uuid::new_v4),
                    p.input,
                )
                .await,
        )
    }
    #[tool(description = "List versioned flight sheets.")]
    async fn flight_sheets_list(&self) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.flights().await)
    }
    #[tool(
        description = "Create or update a multi-miner flight sheet. Editing a sheet does not modify running machines."
    )]
    async fn flight_sheet_save(
        &self,
        Parameters(p): Parameters<FlightSave>,
        ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .save_flight(&actor(&ctx)?, p.id.unwrap_or_else(Uuid::new_v4), p.input)
                .await,
        )
    }
    #[tool(
        description = "Execute an authorized batch action: bootstrap, apply flight sheet, miner start/stop/restart, shell command, or BMC power. Requires idempotency_key. Returns job_id; poll job_get for individual outcomes. Unknown results must be reconciled, never blindly replayed."
    )]
    async fn job_submit(
        &self,
        Parameters(input): Parameters<JobInput>,
        ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .submit_job(&actor(&ctx)?, input)
                .await
                .map(|id| json!({"job_id":id})),
        )
    }
    #[tool(
        description = "Get durable batch job and per-machine output, status and validation evidence."
    )]
    async fn job_get(
        &self,
        Parameters(p): Parameters<ById>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.job(p.id).await)
    }
    #[tool(
        description = "Request cancellation. Running commands receive a best-effort cancellation; unknown outcomes remain locked."
    )]
    async fn job_cancel(
        &self,
        Parameters(p): Parameters<ById>,
        ctx: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.cancel_job(&actor(&ctx)?, p.id).await)
    }
    #[tool(
        description = "Read latest observations with timestamps and errors. A flight-sheet assignment does not prove mining; inspect fresh process, algorithm, hashrate and shares."
    )]
    async fn telemetry_latest(
        &self,
        Parameters(p): Parameters<TelemetryRequest>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.latest(p.machine_id).await)
    }
    #[tool(
        description = "Per-machine message feed, newest first: watchdog restarts, miner exits, flight sheet results, job outcomes and configuration changes."
    )]
    async fn machine_messages(
        &self,
        Parameters(p): Parameters<MessagesRequest>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .messages(p.machine_id, p.limit.unwrap_or(100))
                .await,
        )
    }
    #[tool(
        description = "Read the last lines of a miner instance console log (read-only, never attaches to the screen session)."
    )]
    async fn miner_log(
        &self,
        Parameters(p): Parameters<MinerLogRequest>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(
            self.app
                .miner_log(p.machine_id, &p.instance, p.lines.unwrap_or(200))
                .await,
        )
    }
    #[tool(
        description = "Query BMC power, sensors or hardware events. Missing readings are not zero."
    )]
    async fn bmc_read(
        &self,
        Parameters(p): Parameters<BmcRead>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.bmc_read(p.machine_id, &p.kind).await)
    }
    #[tool(
        description = "Read cached BIOS settings and pending changes for one machine. Refresh via bios_read job; modify via bios_write job with revision and changes. No automatic reboot."
    )]
    async fn bios_read(
        &self,
        Parameters(p): Parameters<ById>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        response(self.app.bios_cached(p.id).await)
    }
    #[tool(
        description = "List versioned miner form schemas and runtime compatibility capabilities."
    )]
    async fn adapters_list(&self) -> std::result::Result<CallToolResult, ErrorData> {
        response(Ok(self.app.adapter_descriptions()))
    }
}
#[tool_handler]
impl ServerHandler for RigMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo{instructions:Some("RigDeck manages user-owned servers. All writes share REST authorization, immutable snapshots, audit and persistent queue. Do not replay unknown SSH or power outcomes. Aggregate hashrate separately by algorithm.".into()),capabilities:ServerCapabilities::builder().enable_tools().build(),..Default::default()}
    }
}
pub fn service(app: App) -> StreamableHttpService<RigMcp, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(RigMcp::new(app.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig {
            stateful_mode: false,
            ..Default::default()
        },
    )
}
