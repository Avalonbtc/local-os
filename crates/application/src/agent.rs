//! HiveOS-style control channel. Each rig's `rigdeck-agent` keeps one WebSocket open to the
//! controller (`/agent/v1/connect`), so rigs behind NAT need no inbound port and the controller
//! stores no SSH credentials for them.
//!
//! The hub only moves JSON messages; `AgentExecutor` turns them into the same four primitives
//! the SSH executor offers (run a command, upload a file, open a terminal). Everything above it
//! — the screen runtime, durable jobs, reconciliation by operation id — is unchanged, so a
//! machine can switch transport without any job or runtime code noticing.
//!
//! Messages, controller → agent: `exec`, `upload`, `term_open`, `term_input`, `term_resize`,
//! `term_close`. Agent → controller: `result` (answers exec/upload by `id`), `term_data`,
//! `term_exit`.
use crate::App;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use rig_domain::*;
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Largest upload sent in one message (base64 adds a third on the wire).
pub const MAX_UPLOAD: usize = 16 * 1024 * 1024;
/// Largest WebSocket message either side accepts.
pub const MAX_MESSAGE: usize = 24 * 1024 * 1024;

/// What `GET /agent/install.sh` and `GET /agent/v1/runtime.py` serve.
#[derive(Clone)]
pub struct Installer {
    pub script: String,
    pub runtime: &'static str,
}

#[derive(Default)]
pub struct AgentHub {
    /// Public base URL of this controller (`https://panel.example.com`); empty disables
    /// enrollment.
    public_url: String,
    installer: Option<Installer>,
    connections: Mutex<HashMap<Uuid, Connection>>,
    next: AtomicU64,
}
struct Connection {
    session: u64,
    outgoing: mpsc::Sender<Value>,
    pending: HashMap<u64, oneshot::Sender<Value>>,
    terminals: HashMap<u64, mpsc::Sender<Value>>,
    status: AgentStatus,
}

impl AgentHub {
    pub fn new(public_url: &str, installer: Option<Installer>) -> Self {
        Self {
            public_url: public_url.trim_end_matches('/').to_owned(),
            installer,
            ..Default::default()
        }
    }
    pub fn public_url(&self) -> &str {
        &self.public_url
    }
    pub fn installer(&self) -> Option<&Installer> {
        self.installer.as_ref()
    }
    /// `wss://…/agent/v1/connect` for an `https://` controller.
    pub fn connect_url(&self) -> String {
        let base = if let Some(rest) = self.public_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.public_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.public_url.clone()
        };
        format!("{base}/agent/v1/connect")
    }
    /// Start a session; a previous connection of the same machine is dropped (its pending
    /// requests fail, its socket closes when the receiver it pumps from goes away).
    pub fn register(&self, status: AgentStatus) -> (u64, mpsc::Receiver<Value>) {
        let (outgoing, receiver) = mpsc::channel(256);
        let session = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.connections.lock().unwrap().insert(
            status.machine_id,
            Connection {
                session,
                outgoing,
                pending: HashMap::new(),
                terminals: HashMap::new(),
                status,
            },
        );
        (session, receiver)
    }
    pub fn unregister(&self, machine: Uuid, session: u64) {
        let mut connections = self.connections.lock().unwrap();
        if connections
            .get(&machine)
            .is_some_and(|c| c.session == session)
        {
            connections.remove(&machine);
        }
    }
    pub fn connected(&self, machine: Uuid) -> bool {
        self.connections.lock().unwrap().contains_key(&machine)
    }
    pub fn statuses(&self) -> Vec<AgentStatus> {
        let mut list: Vec<_> = self
            .connections
            .lock()
            .unwrap()
            .values()
            .map(|c| c.status.clone())
            .collect();
        list.sort_by_key(|s| s.machine_id);
        list
    }
    /// A message from the agent of `session`.
    pub async fn deliver(&self, machine: Uuid, session: u64, message: Value) {
        let id = message["id"].as_u64().unwrap_or(0);
        let terminal = {
            let mut connections = self.connections.lock().unwrap();
            let Some(connection) = connections
                .get_mut(&machine)
                .filter(|c| c.session == session)
            else {
                return;
            };
            match message["type"].as_str() {
                Some("result") => {
                    if let Some(waiter) = connection.pending.remove(&id) {
                        let _ = waiter.send(message);
                    }
                    return;
                }
                Some("term_data" | "term_exit") => {
                    let sender = connection.terminals.get(&id).cloned();
                    if message["type"] == "term_exit" {
                        connection.terminals.remove(&id);
                    }
                    sender
                }
                _ => return,
            }
        };
        if let Some(terminal) = terminal {
            let _ = terminal.send(message).await;
        }
    }
    fn outgoing(&self, machine: Uuid) -> Result<mpsc::Sender<Value>> {
        self.connections
            .lock()
            .unwrap()
            .get(&machine)
            .map(|c| c.outgoing.clone())
            .ok_or_else(offline)
    }
    async fn send(&self, machine: Uuid, message: Value) -> Result<()> {
        self.outgoing(machine)?
            .send(message)
            .await
            .map_err(|_| offline())
    }
    /// Send a request and wait for its `result`.
    pub async fn request(
        &self,
        machine: Uuid,
        mut message: Value,
        wait: Duration,
    ) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        message["id"] = json!(id);
        let (waiter, answer) = oneshot::channel();
        let outgoing = {
            let mut connections = self.connections.lock().unwrap();
            let connection = connections.get_mut(&machine).ok_or_else(offline)?;
            connection.pending.insert(id, waiter);
            connection.outgoing.clone()
        };
        if outgoing.send(message).await.is_err() {
            return Err(offline());
        }
        let reply = match tokio::time::timeout(wait, answer).await {
            Ok(Ok(reply)) => reply,
            // The connection went away with the request outstanding: whether it ran is unknown.
            Ok(Err(_)) => {
                return Err(Error::Unavailable(
                    "与矿机的连接中断；操作是否完成需对账".into(),
                ));
            }
            Err(_) => {
                if let Some(connection) = self.connections.lock().unwrap().get_mut(&machine) {
                    connection.pending.remove(&id);
                }
                return Err(Error::Unavailable(
                    "矿机未在时限内回复；操作是否完成需对账".into(),
                ));
            }
        };
        match reply["error"].as_str() {
            Some(error) => Err(Error::Unavailable(format!("矿机：{error}"))),
            None => Ok(reply),
        }
    }
    fn open_terminal(&self, machine: Uuid) -> Result<(u64, mpsc::Receiver<Value>)> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel(256);
        self.connections
            .lock()
            .unwrap()
            .get_mut(&machine)
            .ok_or_else(offline)?
            .terminals
            .insert(id, sender);
        Ok((id, receiver))
    }
    fn close_terminal(&self, machine: Uuid, id: u64) {
        if let Some(connection) = self.connections.lock().unwrap().get_mut(&machine) {
            connection.terminals.remove(&id);
        }
    }
}
fn offline() -> Error {
    Error::Unavailable("矿机未连接（rigdeck-agent 离线）".into())
}

/// `RemoteExecutor` over the agent connection. The agent runs as root, so commands need no
/// sudo and credentials are never used.
pub struct AgentExecutor {
    pub hub: Arc<AgentHub>,
}
fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .and_then(|data| STANDARD.decode(data).ok())
        .unwrap_or_default()
}
#[async_trait::async_trait]
impl RemoteExecutor for AgentExecutor {
    async fn execute(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: &str,
        timeout: u64,
    ) -> Result<ExecOutput> {
        self.execute_input(machine, credential, command, &[], timeout)
            .await
    }
    async fn execute_input(
        &self,
        machine: &Machine,
        _: &Credential,
        command: &str,
        input: &[u8],
        timeout: u64,
    ) -> Result<ExecOutput> {
        let reply = self
            .hub
            .request(
                machine.id,
                json!({"type":"exec","command":command,"stdin":STANDARD.encode(input),"timeout":timeout}),
                Duration::from_secs(timeout + 30),
            )
            .await?;
        if reply["timed_out"] == true {
            return Err(Error::Unavailable(
                "矿机上的命令超时；远端是否完成需对账".into(),
            ));
        }
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&bytes(&reply["stdout"])).into_owned(),
            stderr: String::from_utf8_lossy(&bytes(&reply["stderr"])).into_owned(),
            exit_code: reply["exit_code"].as_u64().map(|code| code as u32),
            truncated: reply["truncated"] == true,
        })
    }
    async fn upload(
        &self,
        machine: &Machine,
        _: &Credential,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<()> {
        if content.len() > MAX_UPLOAD {
            return Err(Error::Validation(
                "经矿机连接上传的文件不能超过 16 MiB".into(),
            ));
        }
        self.hub
            .request(
                machine.id,
                json!({"type":"upload","path":path,"mode":mode,"data":STANDARD.encode(content)}),
                Duration::from_secs(120),
            )
            .await
            .map(|_| ())
    }
    async fn terminal_root(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: &str,
        input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        self.terminal(machine, credential, Some(command), input, output)
            .await
    }
    async fn terminal(
        &self,
        machine: &Machine,
        _: &Credential,
        command: Option<&str>,
        mut input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        let (id, mut events) = self.hub.open_terminal(machine.id)?;
        let opened = self
            .hub
            .send(
                machine.id,
                json!({"type":"term_open","id":id,"command":command,"cols":120,"rows":32}),
            )
            .await;
        if let Err(error) = opened {
            self.hub.close_terminal(machine.id, id);
            return Err(error);
        }
        loop {
            tokio::select! {
                message = input.recv() => {
                    let message = match message {
                        Some(TerminalInput::Data(data)) => json!({"type":"term_input","id":id,"data":STANDARD.encode(data)}),
                        Some(TerminalInput::Resize(cols, rows)) => json!({"type":"term_resize","id":id,"cols":cols.clamp(1, 500),"rows":rows.clamp(1, 200)}),
                        Some(TerminalInput::Close) | None => {
                            let _ = self.hub.send(machine.id, json!({"type":"term_close","id":id})).await;
                            break;
                        }
                    };
                    if self.hub.send(machine.id, message).await.is_err() {
                        let _ = output.send(TerminalOutput::Error("与矿机的连接已断开".into())).await;
                        break;
                    }
                }
                event = events.recv() => match event {
                    Some(event) if event["type"] == "term_data" => {
                        if output.send(TerminalOutput::Data(bytes(&event["data"]))).await.is_err() {
                            let _ = self.hub.send(machine.id, json!({"type":"term_close","id":id})).await;
                            break;
                        }
                    }
                    Some(event) => {
                        let code = event["code"].as_u64().map(|code| code as u32);
                        let _ = output.send(TerminalOutput::Closed(code)).await;
                        break;
                    }
                    None => {
                        let _ = output.send(TerminalOutput::Error("与矿机的连接已断开".into())).await;
                        break;
                    }
                }
            }
        }
        self.hub.close_terminal(machine.id, id);
        Ok(())
    }
}

/// Picks the transport per machine: agent machines through their connection, the rest over SSH.
pub struct RoutedExecutor {
    pub ssh: Arc<dyn RemoteExecutor>,
    pub agent: Arc<AgentExecutor>,
}
impl RoutedExecutor {
    fn pick(&self, machine: &Machine) -> &dyn RemoteExecutor {
        if machine.is_agent() {
            self.agent.as_ref()
        } else {
            self.ssh.as_ref()
        }
    }
}
#[async_trait::async_trait]
impl RemoteExecutor for RoutedExecutor {
    async fn probe_host_key(&self, host: &str, port: u16) -> Result<SshHostKey> {
        self.ssh.probe_host_key(host, port).await
    }
    async fn execute(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        timeout: u64,
    ) -> Result<ExecOutput> {
        self.pick(m).execute(m, c, command, timeout).await
    }
    async fn execute_input(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        input: &[u8],
        timeout: u64,
    ) -> Result<ExecOutput> {
        self.pick(m)
            .execute_input(m, c, command, input, timeout)
            .await
    }
    async fn upload(
        &self,
        m: &Machine,
        c: &Credential,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<()> {
        self.pick(m).upload(m, c, path, content, mode).await
    }
    async fn upload_file(
        &self,
        m: &Machine,
        c: &Credential,
        path: &str,
        local: &std::path::Path,
        mode: u32,
    ) -> Result<()> {
        self.pick(m).upload_file(m, c, path, local, mode).await
    }
    async fn terminal_root(
        &self,
        m: &Machine,
        c: &Credential,
        command: &str,
        input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        self.pick(m)
            .terminal_root(m, c, command, input, output)
            .await
    }
    async fn terminal(
        &self,
        m: &Machine,
        c: &Credential,
        command: Option<&str>,
        input: mpsc::Receiver<TerminalInput>,
        output: mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        self.pick(m).terminal(m, c, command, input, output).await
    }
}

impl App {
    /// Check an agent's answer to the connection challenge: HMAC-SHA256 with the machine's key
    /// over `rigdeck-agent/v1|<machine>|<nonce>`.
    pub async fn authenticate_agent(
        &self,
        machine: Uuid,
        nonce: &str,
        mac: &str,
    ) -> Result<Machine> {
        let key = self
            .vault
            .push_key(machine)
            .and_then(|key| hex::decode(key).ok())
            .ok_or(Error::Unauthorized)?;
        let mac = hex::decode(mac.trim()).map_err(|_| Error::Unauthorized)?;
        let mut expected = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| Error::Unauthorized)?;
        expected.update(format!("rigdeck-agent/v1|{machine}|{nonce}").as_bytes());
        expected
            .verify_slice(&mac)
            .map_err(|_| Error::Unauthorized)?;
        let found = self.repository.machine(machine).await?;
        if !found.is_agent() {
            return Err(Error::Forbidden("这台机器不是主动连接模式".into()));
        }
        Ok(found)
    }
    pub fn agent_statuses(&self) -> Vec<AgentStatus> {
        self.agents.statuses()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(machine: Uuid) -> AgentStatus {
        AgentStatus {
            machine_id: machine,
            connected_at: chrono::Utc::now(),
            version: "0.5.0".into(),
            hostname: "rig".into(),
            address: None,
        }
    }
    fn machine(id: Uuid) -> Machine {
        serde_json::from_value(json!({"id":id,"name":"r","host":"10.0.0.2","port":22,"username":"root","host_key":"agent","group":"","tags":[],"is_controller":false,"policy":{},"created_at":"2026-01-01T00:00:00Z","transport":"agent"})).unwrap()
    }
    const NONE: Credential = Credential::Password {
        password: String::new(),
        sudo_password: None,
    };

    /// Answers every request like the Python agent would.
    fn fake_agent(hub: Arc<AgentHub>, id: Uuid) -> tokio::task::JoinHandle<Vec<Value>> {
        let (session, mut outgoing) = hub.register(status(id));
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(message) = outgoing.recv().await {
                seen.push(message.clone());
                let reply = match message["type"].as_str().unwrap() {
                    "exec" => {
                        let stdin = String::from_utf8(bytes(&message["stdin"])).unwrap();
                        json!({"type":"result","id":message["id"],"exit_code":3,
                            "stdout":STANDARD.encode(format!("ran {} with {stdin}", message["command"].as_str().unwrap())),
                            "stderr":STANDARD.encode("warn"),"truncated":false})
                    }
                    "upload" if message["path"] == "/bad" => {
                        json!({"type":"result","id":message["id"],"error":"Permission denied"})
                    }
                    "upload" => json!({"type":"result","id":message["id"],"ok":true}),
                    "term_open" => {
                        json!({"type":"term_data","id":message["id"],"data":STANDARD.encode("$ ")})
                    }
                    "term_input" => {
                        json!({"type":"term_data","id":message["id"],"data":message["data"]})
                    }
                    "term_close" => json!({"type":"term_exit","id":message["id"],"code":0}),
                    _ => continue,
                };
                hub.deliver(id, session, reply).await;
            }
            seen
        })
    }

    #[tokio::test]
    async fn exec_upload_and_errors_round_trip_through_the_connection() {
        let hub = Arc::new(AgentHub::default());
        let id = Uuid::new_v4();
        let executor = AgentExecutor { hub: hub.clone() };
        let offline = executor.execute(&machine(id), &NONE, "true", 5).await;
        assert!(offline.unwrap_err().to_string().contains("未连接"));

        let agent = fake_agent(hub.clone(), id);
        assert!(hub.connected(id));
        let output = executor
            .execute_input(&machine(id), &NONE, "cat", b"input", 5)
            .await
            .unwrap();
        assert_eq!(output.stdout, "ran cat with input");
        assert_eq!(
            (output.stderr.as_str(), output.exit_code),
            ("warn", Some(3))
        );
        executor
            .upload(&machine(id), &NONE, "/tmp/x", b"data", 0o600)
            .await
            .unwrap();
        let error = executor
            .upload(&machine(id), &NONE, "/bad", b"data", 0o600)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Permission denied"));

        // A newer connection of the same machine replaces the old one.
        let (_, _replacement) = hub.register(status(id));
        let seen = agent.await.unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[1]["mode"], 0o600);
    }

    #[tokio::test]
    async fn unanswered_request_after_disconnect_is_reported_as_unknown() {
        let hub = Arc::new(AgentHub::default());
        let id = Uuid::new_v4();
        let (session, mut outgoing) = hub.register(status(id));
        let request = {
            let hub = hub.clone();
            tokio::spawn(async move {
                hub.request(id, json!({"type":"exec"}), Duration::from_secs(5))
                    .await
            })
        };
        assert!(outgoing.recv().await.is_some());
        hub.unregister(id, session);
        let error = request.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("对账"), "{error}");
        assert!(!hub.connected(id));
    }

    #[tokio::test]
    async fn terminal_streams_both_ways_and_closes() {
        let hub = Arc::new(AgentHub::default());
        let id = Uuid::new_v4();
        let _agent = fake_agent(hub.clone(), id);
        let executor = AgentExecutor { hub: hub.clone() };
        let (input, input_rx) = mpsc::channel(8);
        let (output_tx, mut output) = mpsc::channel(8);
        let session = tokio::spawn(async move {
            executor
                .terminal(&machine(id), &NONE, None, input_rx, output_tx)
                .await
        });
        let first = output.recv().await.unwrap();
        assert!(matches!(first, TerminalOutput::Data(ref d) if d == b"$ "));
        input
            .send(TerminalInput::Data(b"ls\n".to_vec()))
            .await
            .unwrap();
        assert!(
            matches!(output.recv().await.unwrap(), TerminalOutput::Data(ref d) if d == b"ls\n")
        );
        input.send(TerminalInput::Close).await.unwrap();
        session.await.unwrap().unwrap();
    }

    #[test]
    fn connect_url_follows_the_public_scheme() {
        assert_eq!(
            AgentHub::new("https://panel.example.com/", None).connect_url(),
            "wss://panel.example.com/agent/v1/connect"
        );
        assert_eq!(
            AgentHub::new("http://127.0.0.1:8080", None).connect_url(),
            "ws://127.0.0.1:8080/agent/v1/connect"
        );
    }
}
