use crate::*;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait BiosProvider: Send + Sync {
    async fn cached(&self, machine: Uuid) -> Result<Value>;
    async fn execute(
        &self,
        machine: &Machine,
        credential: &Credential,
        operation: Uuid,
        action: &Value,
        reconcile: bool,
    ) -> Result<RemoteOperation>;
}

#[async_trait]
pub trait IdentityRepository: Send + Sync {
    async fn create_admin(&self, name: &str, password_hash: &str) -> Result<()>;
    async fn password_hash(&self, name: &str) -> Result<Option<(Uuid, String)>>;
    async fn login_attempt(&self, key: &str, success: bool) -> Result<bool>;
    async fn create_session(&self, user: Uuid, hash: &str, csrf: &str) -> Result<Session>;
    async fn session(&self, hash: &str) -> Result<Session>;
    async fn delete_session(&self, hash: &str) -> Result<()>;
    async fn token_actor(&self, hash: &str) -> Result<Actor>;
    async fn create_token(&self, actor: &Actor, name: &str, hash: &str) -> Result<Uuid>;
    async fn tokens(&self, actor: &Actor) -> Result<Vec<TokenInfo>>;
    async fn revoke_token(&self, actor: &Actor, id: Uuid) -> Result<()>;
}
#[async_trait]
pub trait FleetRepository: Send + Sync {
    async fn machines(&self) -> Result<Vec<Machine>>;
    async fn machine(&self, id: Uuid) -> Result<Machine>;
    async fn save_machine(
        &self,
        id: Uuid,
        input: &MachineInput,
        ssh_secret: Option<&str>,
        bmc_secret: Option<&str>,
        actor: &Actor,
    ) -> Result<Machine>;
    async fn machine_secret(&self, id: Uuid, bmc: bool) -> Result<String>;
    async fn delete_machine(&self, id: Uuid, actor: &Actor) -> Result<()>;
}
#[async_trait]
pub trait CatalogRepository: Send + Sync {
    async fn register_artifact(&self, hash: &str, size: i64, actor: &Actor) -> Result<()>;
    async fn catalog(&self, kind: CatalogKind) -> Result<Vec<CatalogItem>>;
    async fn catalog_item(&self, kind: CatalogKind, id: Uuid) -> Result<CatalogItem>;
    async fn save_catalog(
        &self,
        kind: CatalogKind,
        id: Uuid,
        input: &CatalogInput,
        actor: &Actor,
    ) -> Result<CatalogItem>;
    async fn delete_catalog(&self, kind: CatalogKind, id: Uuid, actor: &Actor) -> Result<()>;
}
#[async_trait]
pub trait FlightRepository: Send + Sync {
    async fn flights(&self) -> Result<Vec<FlightSheet>>;
    async fn flight(&self, id: Uuid) -> Result<FlightSheet>;
    async fn save_flight(
        &self,
        id: Uuid,
        input: &FlightInput,
        actor: &Actor,
        propagation: Option<&FlightPropagation>,
    ) -> Result<FlightSheet>;
    async fn delete_flight(&self, id: Uuid, actor: &Actor) -> Result<()>;
}
#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn flight_targets(&self, sheet: Uuid) -> Result<Vec<Uuid>>;
    async fn enqueue(&self, input: &JobInput, snapshot: &Value, actor: &Actor) -> Result<Uuid>;
    async fn jobs(&self, limit: i64) -> Result<Vec<Job>>;
    async fn job(&self, id: Uuid) -> Result<Job>;
    async fn cancel_job(&self, id: Uuid, actor: &Actor) -> Result<()>;
    async fn claim(&self) -> Result<Option<ClaimedTask>>;
    async fn heartbeat(&self, target: Uuid, lease: Uuid) -> Result<bool>;
    async fn task_progress(
        &self,
        target: Uuid,
        lease: Uuid,
        remote: &RemoteOperation,
    ) -> Result<()>;
    async fn finish(
        &self,
        target: Uuid,
        lease: Uuid,
        status: &str,
        remote: &RemoteOperation,
    ) -> Result<()>;
    async fn expire_leases(&self) -> Result<()>;
    async fn resolve_target(&self, id: Uuid, input: &ResolveTarget, actor: &Actor) -> Result<()>;
}
#[async_trait]
pub trait TelemetryRepository: Send + Sync {
    async fn observe(&self, observation: &Observation) -> Result<()>;
    async fn latest(&self, machine: Option<Uuid>) -> Result<Vec<Observation>>;
    async fn latest_summary(&self, machine: Option<Uuid>) -> Result<Vec<Observation>> {
        self.latest(machine).await
    }
    async fn history(&self, machine: Uuid, kind: &str, hours: u32) -> Result<Vec<Observation>>;
    async fn maintain(&self) -> Result<()>;
    /// Write many observations in one round trip (the monitor batches across machines).
    async fn observe_many(&self, observations: &[Observation]) -> Result<()> {
        for observation in observations {
            self.observe(observation).await?;
        }
        Ok(())
    }
    /// Store runtime events idempotently (the rig re-sends its recent tail). Returns rows inserted.
    async fn record_events(&self, _machine: Uuid, _events: &[Value]) -> Result<u64> {
        Ok(0)
    }
    async fn messages(&self, _machine: Uuid, _limit: i64) -> Result<Vec<MachineMessage>> {
        Ok(Vec::new())
    }
    /// Messages still shown as header chips: newer than the last "clear all" and not closed.
    async fn unread_messages(&self, machine: Uuid, limit: i64) -> Result<Vec<MachineMessage>> {
        self.messages(machine, limit).await
    }
    /// Close one chip (`key` = `source:id`) or, with `None`, clear all chips for the machine.
    async fn dismiss_messages(&self, _machine: Uuid, _key: Option<&str>) -> Result<()> {
        Ok(())
    }
}
#[async_trait]
pub trait AuditRepository: Send + Sync {
    async fn audit(&self, actor: &Actor, action: &str, target: &str, detail: Value) -> Result<()>;
    async fn audit_events(&self, limit: i64) -> Result<Vec<AuditEvent>>;
}
pub trait Repository:
    IdentityRepository
    + FleetRepository
    + CatalogRepository
    + FlightRepository
    + JobRepository
    + TelemetryRepository
    + AuditRepository
{
}
impl<T> Repository for T where
    T: IdentityRepository
        + FleetRepository
        + CatalogRepository
        + FlightRepository
        + JobRepository
        + TelemetryRepository
        + AuditRepository
{
}

pub trait SecretVault: Send + Sync {
    fn encrypt(&self, value: &Credential) -> Result<String>;
    fn decrypt(&self, value: &str) -> Result<Credential>;
    fn password_hash(&self, value: &str) -> Result<String>;
    fn verify_password(&self, value: &str, hash: &str) -> bool;
    fn random_token(&self) -> String;
}
#[async_trait]
pub trait RemoteExecutor: Send + Sync {
    async fn probe_host_key(&self, _host: &str, _port: u16) -> Result<SshHostKey> {
        Err(Error::Unavailable("SSH 指纹探测不可用".into()))
    }
    async fn execute(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: &str,
        timeout: u64,
    ) -> Result<ExecOutput>;
    async fn execute_input(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: &str,
        input: &[u8],
        timeout: u64,
    ) -> Result<ExecOutput> {
        if !input.is_empty() {
            return Err(Error::Unavailable("SSH 标准输入不可用".into()));
        }
        self.execute(machine, credential, command, timeout).await
    }
    async fn upload(
        &self,
        machine: &Machine,
        credential: &Credential,
        path: &str,
        content: &[u8],
        mode: u32,
    ) -> Result<()>;
    async fn upload_file(
        &self,
        _machine: &Machine,
        _credential: &Credential,
        _path: &str,
        _local: &std::path::Path,
        _mode: u32,
    ) -> Result<()> {
        Err(Error::Unavailable("流式 SSH 文件上传不可用".into()))
    }
    async fn terminal_root(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: &str,
        input: tokio::sync::mpsc::Receiver<TerminalInput>,
        output: tokio::sync::mpsc::Sender<TerminalOutput>,
    ) -> Result<()> {
        if machine.username != "root" {
            return Err(Error::Unavailable("需要支持 sudo 的终端实现".into()));
        }
        self.terminal(machine, credential, Some(command), input, output)
            .await
    }
    async fn terminal(
        &self,
        machine: &Machine,
        credential: &Credential,
        command: Option<&str>,
        input: tokio::sync::mpsc::Receiver<TerminalInput>,
        output: tokio::sync::mpsc::Sender<TerminalOutput>,
    ) -> Result<()>;
}
#[async_trait]
pub trait MinerRuntime: Send + Sync {
    async fn cache_stream(
        &self,
        _chunks: tokio::sync::mpsc::Receiver<Result<Vec<u8>>>,
    ) -> Result<Value> {
        Err(Error::Unavailable("流式安装包不可用".into()))
    }

    async fn cache_package(&self, bytes: &[u8]) -> Result<Value>;
    async fn import_package(&self, url: &str) -> Result<Value>;
    async fn bootstrap(&self, machine: &Machine, credential: &Credential) -> Result<Value>;
    async fn start_operation(
        &self,
        machine: &Machine,
        credential: &Credential,
        id: Uuid,
        action: &Value,
    ) -> Result<()>;
    async fn operation(
        &self,
        machine: &Machine,
        credential: &Credential,
        id: Uuid,
    ) -> Result<RemoteOperation>;
    async fn cancel(&self, machine: &Machine, credential: &Credential, id: Uuid) -> Result<()>;
    async fn collect(
        &self,
        machine: &Machine,
        credential: &Credential,
        kind: &str,
    ) -> Result<Value>;
    /// Read the watchdog-published snapshot with a plain `cat` (no sudo, no interpreter).
    /// `Ok(None)` means the runtime does not publish one; callers fall back to `collect`.
    async fn snapshot(
        &self,
        _machine: &Machine,
        _credential: &Credential,
    ) -> Result<Option<Value>> {
        Ok(None)
    }
    async fn log_tail(
        &self,
        _machine: &Machine,
        _credential: &Credential,
        _instance: &str,
        _lines: u32,
    ) -> Result<Value> {
        Err(Error::Unavailable("运行层不支持日志查看".into()))
    }
    async fn set_policy(
        &self,
        _machine: &Machine,
        _credential: &Credential,
        _digest: &str,
        _policy: &Value,
    ) -> Result<()> {
        Err(Error::Unavailable("运行层不支持策略下发".into()))
    }
}
pub trait MinerAdapter: Send + Sync {
    fn describe(&self) -> AdapterDescription;
    fn render(
        &self,
        miner: &CatalogItem,
        task: &FlightTask,
        wallet: &CatalogItem,
        coin: &CatalogItem,
        pool: Option<&CatalogItem>,
        machine: &Machine,
    ) -> Result<Value>;
}
#[async_trait]
pub trait BmcProvider: Send + Sync {
    fn id(&self) -> &str;
    async fn read(&self, config: &BmcConfig, credential: &Credential, kind: &str) -> Result<Value>;
    async fn power(
        &self,
        config: &BmcConfig,
        credential: &Credential,
        action: &str,
    ) -> Result<Value>;
}
