use axum::{
    Extension, Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, Path, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use rig_application::App;
use rig_domain::*;
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct ApiState {
    pub app: App,
    pub origin: String,
    pub secure_cookie: bool,
}
pub struct ApiError(pub Error);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::Validation(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = if matches!(self.0, Error::Internal(_)) {
            tracing::error!(error=%self.0,"API internal error");
            "服务内部错误".to_string()
        } else {
            self.0.to_string()
        };
        (status, Json(json!({"error":message}))).into_response()
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;

async fn bios_config(State(s): State<ApiState>, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    Ok(Json(s.app.bios_cached(id).await?))
}

pub fn router(state: ApiState, static_dir: &str) -> Router {
    let mcp = crate::mcp::service(state.app.clone());
    let protected = Router::new()
        .route("/me", get(me))
        .route("/logout", post(logout))
        .route("/tokens", get(tokens).post(token_create))
        .route("/tokens/{id}", axum::routing::delete(token_revoke))
        .route("/machines", get(machines).post(machine_create))
        .route("/ssh/host-key", post(ssh_host_key))
        .route("/ssh/host-keys", post(ssh_host_keys))
        .route("/machines/batch", post(machines_batch))
        .route("/machines/{id}", put(machine_update).delete(machine_delete))
        .route("/machines/{id}/test", post(machine_test))
        .route("/machines/{id}/bios", get(bios_config))
        .route("/machines/{id}/terminal", get(terminal))
        .route("/catalog/{kind}", get(catalog).post(catalog_create))
        .route(
            "/catalog/{kind}/{id}",
            put(catalog_update).delete(catalog_delete),
        )
        .route("/adapters", get(adapters))
        .route(
            "/packages",
            post(package_upload).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route("/flight-sheets", get(flights).post(flight_create))
        .route(
            "/flight-sheets/{id}",
            put(flight_update).delete(flight_delete),
        )
        .route("/jobs", get(jobs).post(job_create))
        .route("/jobs/{id}", get(job))
        .route("/jobs/{id}/cancel", post(job_cancel))
        .route("/job-targets/{id}/resolve", post(target_resolve))
        .route("/telemetry", get(telemetry))
        .route("/machines/{id}/history", get(history))
        .route("/machines/{id}/bmc/{kind}", get(bmc))
        .route("/machines/{id}/messages", get(messages))
        .route("/machines/{id}/messages/dismiss", post(messages_dismiss))
        .route("/machines/{id}/instances/{instance}/log", get(miner_log))
        .route("/audit", get(audit))
        .route(
            "/openapi.json",
            get(|| async { Json(crate::openapi::document()) }),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    let mcp_router = Router::new()
        .nest_service("/mcp", mcp)
        .route_layer(middleware::from_fn_with_state(state.clone(), mcp_auth));
    Router::new()
        .nest("/api/v1", protected)
        .route("/api/v1/login", post(login))
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .merge(mcp_router)
        .fallback_service(
            ServeDir::new(static_dir)
                .not_found_service(ServeFile::new(format!("{static_dir}/index.html"))),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(
            |request: Request, next: Next| async move {
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    "x-rigdeck-time",
                    chrono::Utc::now()
                        .timestamp_millis()
                        .to_string()
                        .parse()
                        .unwrap(),
                );
                response
            },
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|s| s.strip_prefix("rigdeck_session="))
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
fn origin_ok(state: &ApiState, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .is_none_or(|h| h.to_str().ok() == Some(state.origin.as_str()))
}
async fn auth(State(s): State<ApiState>, mut request: Request, next: Next) -> ApiResult<Response> {
    if !origin_ok(&s, request.headers()) {
        return Err(Error::Forbidden("请求来源不匹配".into()).into());
    }
    let session = s
        .app
        .authenticate(bearer(request.headers()), cookie(request.headers()))
        .await?;
    if session.actor.token_id.is_none()
        && !["GET", "HEAD", "OPTIONS"].contains(&request.method().as_str())
    {
        let csrf = request
            .headers()
            .get("x-csrf-token")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if !bool::from(csrf.as_bytes().ct_eq(session.csrf.as_bytes())) {
            return Err(Error::Forbidden("CSRF 校验失败".into()).into());
        }
    }
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}
async fn mcp_auth(
    State(s): State<ApiState>,
    mut request: Request,
    next: Next,
) -> ApiResult<Response> {
    if !origin_ok(&s, request.headers()) {
        return Err(Error::Forbidden("MCP Origin 不匹配".into()).into());
    }
    let token = bearer(request.headers()).ok_or(Error::Unauthorized)?;
    let session = s.app.authenticate(Some(token), None).await?;
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}
async fn login(
    State(s): State<ApiState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    Json(input): Json<Login>,
) -> ApiResult<Response> {
    if !origin_ok(&s, &headers) {
        return Err(Error::Forbidden("请求来源不匹配".into()).into());
    }
    let peer = peer.map(|p| p.0.0.ip());
    // Opt-in only for a port exclusively reachable through the trusted proxy.
    let source = login_source(
        &headers,
        peer,
        std::env::var("RIGDECK_TRUST_PROXY_HEADERS").as_deref() == Ok("1"),
    );
    let (token, session) = s.app.login(input, &source).await?;
    let mut response = Json(session).into_response();
    let value = format!(
        "rigdeck_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200{}",
        if s.secure_cookie { "; Secure" } else { "" }
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        value
            .parse()
            .map_err(|_| Error::Internal("Cookie 格式错误".into()))?,
    );
    Ok(response)
}
async fn me(Extension(session): Extension<Session>) -> Json<Session> {
    Json(session)
}
async fn logout(State(s): State<ApiState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(cookie) = cookie(&headers) {
        s.app.logout(cookie).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        "rigdeck_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
            .parse()
            .unwrap(),
    );
    Ok(response)
}
async fn machines(State(s): State<ApiState>) -> ApiResult<Json<Vec<Machine>>> {
    Ok(Json(s.app.machines().await?))
}
async fn ssh_host_key(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(input): Json<SshProbeInput>,
) -> ApiResult<Json<SshHostKey>> {
    Ok(Json(s.app.probe_ssh_host_key(&session.actor, input).await?))
}
async fn ssh_host_keys(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(input): Json<SshProbeBatch>,
) -> ApiResult<Json<Vec<SshProbeResult>>> {
    Ok(Json(
        s.app.probe_ssh_host_keys(&session.actor, input).await?,
    ))
}
async fn machines_batch(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(input): Json<MachineBatchInput>,
) -> ApiResult<Json<MachineBatchResult>> {
    Ok(Json(
        s.app.save_machines_batch(&session.actor, input).await?,
    ))
}
async fn machine_create(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(i): Json<MachineInput>,
) -> ApiResult<Json<Machine>> {
    Ok(Json(
        s.app
            .save_machine(&session.actor, Uuid::new_v4(), i)
            .await?,
    ))
}
async fn machine_update(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    Json(i): Json<MachineInput>,
) -> ApiResult<Json<Machine>> {
    Ok(Json(s.app.save_machine(&session.actor, id, i).await?))
}
async fn machine_delete(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    s.app.delete_machine(&session.actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn machine_test(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        serde_json::to_value(s.app.test_machine(&session.actor, id).await?).unwrap(),
    ))
}
async fn catalog(
    State(s): State<ApiState>,
    Path(kind): Path<String>,
) -> ApiResult<Json<Vec<CatalogItem>>> {
    Ok(Json(s.app.catalog(CatalogKind::parse(&kind)?).await?))
}
async fn catalog_create(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(kind): Path<String>,
    Json(i): Json<CatalogInput>,
) -> ApiResult<Json<CatalogItem>> {
    Ok(Json(
        s.app
            .save_catalog(
                &session.actor,
                CatalogKind::parse(&kind)?,
                Uuid::new_v4(),
                i,
            )
            .await?,
    ))
}
async fn catalog_update(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path((kind, id)): Path<(String, Uuid)>,
    Json(i): Json<CatalogInput>,
) -> ApiResult<Json<CatalogItem>> {
    Ok(Json(
        s.app
            .save_catalog(&session.actor, CatalogKind::parse(&kind)?, id, i)
            .await?,
    ))
}
async fn catalog_delete(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path((kind, id)): Path<(String, Uuid)>,
) -> ApiResult<StatusCode> {
    s.app
        .delete_catalog(&session.actor, CatalogKind::parse(&kind)?, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn flights(State(s): State<ApiState>) -> ApiResult<Json<Vec<FlightSheet>>> {
    Ok(Json(s.app.flights().await?))
}
async fn flight_create(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(i): Json<FlightInput>,
) -> ApiResult<Json<FlightSheet>> {
    Ok(Json(
        s.app.save_flight(&session.actor, Uuid::new_v4(), i).await?,
    ))
}
async fn flight_update(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    Json(i): Json<FlightInput>,
) -> ApiResult<Json<FlightSheet>> {
    Ok(Json(s.app.save_flight(&session.actor, id, i).await?))
}
async fn flight_delete(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    s.app.delete_flight(&session.actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn jobs(State(s): State<ApiState>) -> ApiResult<Json<Vec<Job>>> {
    Ok(Json(s.app.jobs().await?))
}
async fn job(State(s): State<ApiState>, Path(id): Path<Uuid>) -> ApiResult<Json<Job>> {
    Ok(Json(s.app.job(id).await?))
}
async fn job_create(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(i): Json<JobInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({"job_id":s.app.submit_job(&session.actor,i).await?}),
    ))
}
async fn job_cancel(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    s.app.cancel_job(&session.actor, id).await?;
    Ok(StatusCode::ACCEPTED)
}
async fn target_resolve(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
    Json(input): Json<ResolveTarget>,
) -> ApiResult<StatusCode> {
    s.app.resolve_target(&session.actor, id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct TelemetryQuery {
    machine_id: Option<Uuid>,
    #[serde(default)]
    summary: bool,
}
async fn telemetry(
    State(s): State<ApiState>,
    Query(q): Query<TelemetryQuery>,
) -> ApiResult<Json<Vec<Observation>>> {
    Ok(Json(if q.summary {
        s.app.fleet_summary(q.machine_id).await?
    } else {
        s.app.latest(q.machine_id).await?
    }))
}
#[derive(Deserialize)]
struct HistoryQuery {
    kind: String,
    hours: Option<u32>,
    #[serde(default)]
    summary: bool,
}
async fn history(
    State(s): State<ApiState>,
    Path(id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Observation>>> {
    Ok(Json(if q.summary {
        s.app
            .history_summary(id, &q.kind, q.hours.unwrap_or(6))
            .await?
    } else {
        s.app.history(id, &q.kind, q.hours.unwrap_or(6)).await?
    }))
}
#[derive(Deserialize)]
struct MessagesQuery {
    limit: Option<i64>,
    /// Only the header chips: not closed and newer than the last "clear all".
    unread: Option<bool>,
}
async fn messages(
    State(s): State<ApiState>,
    Path(id): Path<Uuid>,
    Query(q): Query<MessagesQuery>,
) -> ApiResult<Json<Vec<MachineMessage>>> {
    Ok(Json(if q.unread.unwrap_or(false) {
        s.app.unread_messages(id, q.limit.unwrap_or(6)).await?
    } else {
        s.app.messages(id, q.limit.unwrap_or(200)).await?
    }))
}
async fn messages_dismiss(
    State(s): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(input): Json<MessageDismiss>,
) -> ApiResult<StatusCode> {
    s.app.dismiss_messages(id, input).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
struct LogQuery {
    lines: Option<u32>,
}
async fn miner_log(
    State(s): State<ApiState>,
    Path((id, instance)): Path<(Uuid, String)>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Json<MinerLog>> {
    Ok(Json(
        s.app
            .miner_log(id, &instance, q.lines.unwrap_or(200))
            .await?,
    ))
}
async fn bmc(
    State(s): State<ApiState>,
    Path((id, kind)): Path<(Uuid, String)>,
) -> ApiResult<Json<Value>> {
    Ok(Json(s.app.bmc_read(id, &kind).await?))
}
async fn audit(State(s): State<ApiState>) -> ApiResult<Json<Vec<AuditEvent>>> {
    Ok(Json(s.app.audit_events().await?))
}
async fn adapters(State(s): State<ApiState>) -> Json<Vec<AdapterDescription>> {
    Json(s.app.adapter_descriptions())
}
async fn tokens(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
) -> ApiResult<Json<Vec<TokenInfo>>> {
    Ok(Json(s.app.tokens(&session.actor).await?))
}
async fn token_create(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        s.app
            .issue_token(&session.actor, input["name"].as_str().unwrap_or(""))
            .await?,
    ))
}
async fn token_revoke(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    s.app.revoke_token(&session.actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn package_upload(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    request: Request,
) -> ApiResult<Json<Value>> {
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let producer = async move {
        let mut body = request.into_body().into_data_stream();
        let mut size = 0usize;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| Error::Validation("安装包上传中断".into()))?;
            size += chunk.len();
            if size > 512 * 1024 * 1024 {
                return Err(Error::Validation("安装包超过 512 MiB".into()));
            }
            for part in chunk.chunks(65536) {
                tx.send(Ok(part.to_vec()))
                    .await
                    .map_err(|_| Error::Unavailable("安装包接收已停止".into()))?;
            }
        }
        Ok::<_, Error>(())
    };
    let (_, artifact) = tokio::try_join!(producer, s.app.cache_package_stream(&session.actor, rx))?;
    Ok(Json(artifact))
}

#[derive(Deserialize)]
struct TerminalQuery {
    csrf: Option<String>,
    instance: Option<String>,
}
async fn terminal(
    State(s): State<ApiState>,
    Extension(session): Extension<Session>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !origin_ok(&s, &headers)
        || session.actor.token_id.is_none()
            && !bool::from(
                q.csrf
                    .as_deref()
                    .unwrap_or("")
                    .as_bytes()
                    .ct_eq(session.csrf.as_bytes()),
            )
    {
        return Err(Error::Forbidden("终端 CSRF 校验失败".into()).into());
    }
    if q.instance.as_ref().is_some_and(|name| {
        name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    }) {
        return Err(Error::Validation("实例名称无效".into()).into());
    }
    let machine = s.app.machine(id).await?;
    let credential = s.app.credential(id, false).await?;
    s.app
        .record_terminal(
            &session.actor,
            id,
            if q.instance.is_some() { "miner" } else { "ssh" },
        )
        .await?;
    let authorization = (
        bearer(&headers).map(str::to_owned),
        cookie(&headers).map(str::to_owned),
    );
    Ok(ws.max_message_size(65536).on_upgrade(move |socket| {
        terminal_bridge(
            s.app,
            machine,
            credential,
            q.instance,
            authorization,
            socket,
        )
    }))
}
async fn terminal_bridge(
    app: App,
    machine: Machine,
    credential: Credential,
    instance: Option<String>,
    authorization: (Option<String>, Option<String>),
    socket: WebSocket,
) {
    let (mut sender, mut receiver) = socket.split();
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(64);
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(64);
    let remote = app.remote.clone();
    let command =
        instance.map(|name| format!("/usr/local/bin/rig-miner attach {}", shell_quote(&name)));
    let task = tokio::spawn(async move {
        let result = if let Some(command) = command {
            remote
                .terminal_root(&machine, &credential, &command, input_rx, output_tx.clone())
                .await
        } else {
            remote
                .terminal(&machine, &credential, None, input_rx, output_tx.clone())
                .await
        };
        if let Err(error) = result {
            let _ = output_tx
                .send(TerminalOutput::Error(error.to_string()))
                .await;
        }
    });
    let mut authorization_check = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = authorization_check.tick() => {
                if app.authenticate(authorization.0.as_deref(), authorization.1.as_deref()).await.is_err() {
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            },
            message=receiver.next()=>{match message{Some(Ok(Message::Binary(bytes)))=>{if input_tx.send(TerminalInput::Data(bytes.to_vec())).await.is_err(){break;}},Some(Ok(Message::Text(text)))=>{if let Ok(v)=serde_json::from_str::<Value>(&text)&& v["type"]=="resize"{let _=input_tx.send(TerminalInput::Resize(v["cols"].as_u64().unwrap_or(120) as u32,v["rows"].as_u64().unwrap_or(32) as u32)).await;}},Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{}}},
            output=output_rx.recv()=>{match output{Some(TerminalOutput::Data(bytes))=>{if sender.send(Message::Binary(bytes.into())).await.is_err(){break;}},Some(TerminalOutput::Error(error))=>{let _=sender.send(Message::Text(json!({"error":error}).to_string().into())).await;break;},_=>break}}
        }
    }
    drop(input_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
}

fn login_source(headers: &HeaderMap, peer: Option<std::net::IpAddr>, trust_proxy: bool) -> String {
    if trust_proxy
        && let Some(ip) = headers
            .get("cf-connecting-ip")
            .and_then(|s| s.to_str().ok())
            .and_then(|s| s.parse::<std::net::IpAddr>().ok())
    {
        return ip.to_string();
    }
    peer.map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown-transport".into())
}
#[cfg(test)]
mod login_source_tests {
    use super::*;
    #[test]
    fn forwarded_identity_requires_explicit_trust() {
        let mut h = HeaderMap::new();
        h.insert("cf-connecting-ip", "203.0.113.2".parse().unwrap());
        let peer = Some("127.0.0.1".parse().unwrap());
        assert_eq!(login_source(&h, peer, false), "127.0.0.1");
        assert_eq!(login_source(&h, peer, true), "203.0.113.2");
        h.insert("cf-connecting-ip", "invalid".parse().unwrap());
        assert_eq!(login_source(&h, peer, true), "127.0.0.1");
    }
}
